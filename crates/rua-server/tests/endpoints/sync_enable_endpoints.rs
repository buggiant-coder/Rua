use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use bytes::Bytes;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::sync::router as sync_router;
use rua_server::endpoints::sync_devices::router as devices_router;
use rua_server::AppState;
use rua_sync::device_list::{DeviceEntry, DeviceList};

#[derive(Debug)]
struct EchoDeviceSessionHandler;

#[async_trait]
impl rua_device_network::DeviceSessionCommandHandler for EchoDeviceSessionHandler {
    async fn handle_command(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        Ok(payload.to_vec())
    }
}

fn build_app() -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", sync_router())
        .nest("/api", devices_router())
        .with_state(state.clone());
    (app, tmp, state)
}

fn build_sync_enabled_app() -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("write flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    assert!(
        state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst),
        "sync should be enabled for v4 emit regression test"
    );
    let app = Router::new()
        .nest("/api", sync_router())
        .nest("/api", devices_router())
        .with_state(state.clone());
    (app, tmp, state)
}

async fn json_body(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response is JSON")
}

async fn request_empty(app: &Router, method: &str, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
}

fn local_endpoint_addr(node: &rua_sync::endpoint::SyncNode) -> iroh::EndpointAddr {
    let mut endpoint_addr = iroh::EndpointAddr::new(node.node_id());
    for socket in node.endpoint().bound_sockets() {
        let ip = match socket.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        endpoint_addr
            .addrs
            .insert(iroh::TransportAddr::Ip(SocketAddr::new(ip, socket.port())));
    }
    endpoint_addr
}

async fn attach_remote_prompt_peer(
    state: &AppState,
) -> (
    tempfile::TempDir,
    rua_sync::endpoint::SyncNode,
    String,
    rua_sync::identity::DeviceIdentity,
) {
    state
        .ensure_device_network_runtime()
        .await
        .expect("start local Device Network Runtime");
    state
        .ensure_sync_runtime_active()
        .await
        .expect("start local Sync Runtime");
    let network = state
        .sync_network_secret
        .read()
        .unwrap()
        .as_ref()
        .cloned()
        .expect("local network secret");
    let local_device = state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .cloned()
        .expect("local Device");
    let local_node = state
        .sync_node
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("local SyncNode");

    let remote_dir = tempfile::tempdir().expect("remote tempdir");
    let remote_device = rua_sync::identity::DeviceIdentity::generate();
    let remote_node = rua_sync::endpoint::SyncNode::bind(
        remote_dir.path(),
        network.namespace_secret(),
        &remote_device,
        None,
    )
    .await
    .expect("bind remote SyncNode");
    let remote_log = rua_sync::opaque_replica_log::OpaqueReplicaLog::new(
        remote_node.doc().clone(),
        remote_device.to_iroh_author(),
        network.derive_sync_key(),
        remote_node.blobs_store().clone(),
    );
    let change = rua_sync::prompt_adapter::PromptLibrarySyncAdapter::upsert_change(
        "sync-now-op",
        "sync-now.md",
        "received before Sync now returned",
        hex::encode(remote_device.device_id_short()),
        rua_sync::hlc::HlcTimestamp {
            wall_ms: 42,
            counter: 0,
            device_id: remote_device.device_id_short(),
        },
    );
    remote_log
        .append_change(&change)
        .await
        .expect("append remote Prompt");

    let remote_addr = local_endpoint_addr(&remote_node);
    local_node
        .register_peer(remote_addr.clone())
        .await
        .expect("register remote peer");
    let entry =
        |device: &rua_sync::identity::DeviceIdentity, direct_addrs: Vec<String>| -> DeviceEntry {
            DeviceEntry {
                device_id: device.device_id_short(),
                public_key: *device.public_key().as_bytes(),
                label: "test".to_string(),
                friendly_name: hex::encode(device.device_id_short()),
                added_at_ms: 1,
                last_seen_ms: 1,
                direct_addrs,
            }
        };
    rua_sync::control_store::SyncControlStore::from_data_dir(&state.rua_root)
        .write_device_list(&DeviceList {
            devices: vec![
                entry(&local_device, Vec::new()),
                entry(
                    &remote_device,
                    remote_addr.ip_addrs().map(ToString::to_string).collect(),
                ),
            ],
        })
        .expect("write paired Device List");

    let remote_node_id = remote_node.node_id().to_string();
    (remote_dir, remote_node, remote_node_id, remote_device)
}

async fn collect_v4_changes(state: &AppState) -> Vec<rua_sync::opaque_replica_log::SyncChange> {
    let (sync_doc, blobs_store) = {
        let guard = state.sync_node.lock().await;
        let node = guard.as_ref().expect("sync node");
        (node.doc().clone(), node.blobs_store().clone())
    };
    let network_secret = state
        .sync_network_secret
        .read()
        .unwrap()
        .as_ref()
        .expect("network secret")
        .clone();
    let device = state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .expect("device")
        .clone();
    let log = rua_sync::opaque_replica_log::OpaqueReplicaLog::new(
        sync_doc.clone(),
        device.to_iroh_author(),
        network_secret.derive_sync_key(),
        blobs_store,
    );

    let entries = sync_doc
        .get_many(iroh_docs::store::Query::all().build())
        .await
        .expect("query Replica Log");
    let entries = futures_util::TryStreamExt::try_collect::<Vec<_>>(entries)
        .await
        .expect("Replica Log entries");
    let mut changes = Vec::new();
    for entry in entries {
        if let Some(frame) = log
            .read_frame(entry.author(), entry.key())
            .await
            .expect("read sync frame")
        {
            changes.extend(frame.into_changes());
        }
    }
    changes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_returns_disabled_by_default() {
    let (app, _tmp, _state) = build_app();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/sync/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["enabled"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_cycle_returns_422_when_disabled() {
    let (app, _tmp, _state) = build_app();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/cycle")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(resp).await;
    assert_eq!(body["reason"], "not_enabled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_now_returns_only_after_remote_prompt_is_applied_locally() {
    let (app, _tmp, state) = build_sync_enabled_app();
    let (_remote_dir, remote_node, remote_node_id, _remote_device) =
        attach_remote_prompt_peer(&state).await;

    let resp = request_empty(&app, "POST", "/api/sync/cycle").await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["peer_node_id"], remote_node_id);
    assert!(body["repair"].is_object());
    assert_eq!(body["repair"]["errors"], serde_json::json!([]));
    assert!(body["transport_duration_ms"].is_number());
    assert!(body["local_apply_duration_ms"].is_number());
    assert_eq!(
        std::fs::read_to_string(state.rua_root.join("prompt_library/sync-now.md"))
            .expect("Prompt must exist before Sync now returns"),
        "received before Sync now returned"
    );
    let sync_now = state
        .sync_stage_recorder
        .snapshot()
        .last_sync_now
        .expect("Sync now diagnostics");
    assert_eq!(
        sync_now.selected_peer_node_id.as_deref(),
        Some(remote_node_id.as_str())
    );
    assert_eq!(
        sync_now.transport_status,
        rua_sync::stage_diagnostics::SyncStageStatus::Success
    );
    assert_eq!(
        sync_now.local_completion_status,
        rua_sync::stage_diagnostics::SyncNowLocalCompletionStatus::Success
    );

    remote_node.shutdown().await;
    state.clear_sync_node().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_now_reports_local_inbox_drain_failure() {
    let (app, _tmp, state) = build_sync_enabled_app();
    let (_remote_dir, remote_node, _remote_node_id, remote_device) =
        attach_remote_prompt_peer(&state).await;
    let network = state
        .sync_network_secret
        .read()
        .unwrap()
        .as_ref()
        .cloned()
        .expect("network secret");
    let log = rua_sync::opaque_replica_log::OpaqueReplicaLog::new(
        remote_node.doc().clone(),
        remote_device.to_iroh_author(),
        network.derive_sync_key(),
        remote_node.blobs_store().clone(),
    );
    let key = log.record_entry_key("prompts", "unreadable.md");
    let tag = remote_node
        .blobs_store()
        .add_bytes(Bytes::from_static(b"bad"))
        .await
        .expect("add malformed replica payload");
    remote_node
        .doc()
        .set_hash(
            remote_device.to_iroh_author().id(),
            Bytes::from(key),
            tag.hash,
            3,
        )
        .await
        .expect("append malformed replica entry");

    let resp = request_empty(&app, "POST", "/api/sync/cycle").await;

    let status = resp.status();
    let body = json_body(resp).await;
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "response={body:?}, inbox={:?}",
        state.inbound_inbox.diagnostics(1).unwrap()
    );
    assert_eq!(body["reason"], "inbox_drain_failed");
    assert!(body["detail"]
        .as_str()
        .expect("detail")
        .contains("drain incomplete"));

    remote_node.shutdown().await;
    state.clear_sync_node().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_now_reports_no_eligible_peer_when_device_list_has_no_remote() {
    let (app, _tmp, state) = build_sync_enabled_app();
    state
        .ensure_device_network_runtime()
        .await
        .expect("start Device Network Runtime");
    state
        .ensure_sync_runtime_active()
        .await
        .expect("start Sync Runtime");

    let resp = request_empty(&app, "POST", "/api/sync/cycle").await;

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(resp).await;
    assert_eq!(body["reason"], "no_eligible_peer");
    assert!(body["detail"]
        .as_str()
        .expect("detail")
        .contains("no remote peers"));

    state.clear_sync_node().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_now_does_not_fallback_after_selected_peer_failure() {
    let (app, _tmp, state) = build_sync_enabled_app();
    let (_remote_dir, remote_node, _remote_node_id, _remote_device) =
        attach_remote_prompt_peer(&state).await;
    let fake_device = rua_sync::identity::DeviceIdentity::generate();
    let fake_node_id =
        iroh::PublicKey::from_bytes(fake_device.public_key().as_bytes()).expect("fake node id");

    let sync_dir = state.rua_root.join("sync");
    let mut device_list = rua_server::sync_peer_directory::load_device_list(&sync_dir);
    device_list.devices.insert(
        1,
        DeviceEntry {
            device_id: fake_device.device_id_short(),
            public_key: *fake_device.public_key().as_bytes(),
            label: "unreachable".to_string(),
            friendly_name: "unreachable-best-peer".to_string(),
            added_at_ms: 10_000,
            last_seen_ms: 10_000,
            direct_addrs: Vec::new(),
        },
    );
    rua_sync::control_store::SyncControlStore::from_sync_dir(&sync_dir)
        .write_device_list(&device_list)
        .expect("write best-peer ordering");
    let mut health = rua_server::sync_cycle::PeerHealthState::default();
    health.peers.insert(
        fake_node_id.to_string(),
        rua_server::sync_cycle::PeerHealthEntry {
            consecutive_failures: 1,
            last_failure_ms: Some(1),
            last_success_ms: None,
        },
    );
    rua_server::sync_peer_directory::save_peer_health(&sync_dir, &health)
        .expect("write fake peer health");
    let fake_short = fake_node_id.fmt_short().to_string();

    let resp = request_empty(&app, "POST", "/api/sync/cycle").await;

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = json_body(resp).await;
    assert_eq!(body["reason"], "timeout");
    assert!(body["detail"]
        .as_str()
        .expect("detail")
        .contains(&fake_short));
    let transport = rua_server::sync_cycle::load_transport_stats(&sync_dir);
    assert_eq!(
        transport
            .last_cycle
            .as_ref()
            .expect("failed selected-peer attempt")
            .peer_node_id,
        fake_node_id.to_string(),
        "Sync now must stop after the selected best peer fails"
    );

    remote_node.shutdown().await;
    state.clear_sync_node().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn targeted_sync_cycle_returns_422_when_disabled() {
    let (app, _tmp, _state) = build_app();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/cycle/target")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "target_device_id": "0102030405060708"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(resp).await;
    assert_eq!(body["reason"], "not_enabled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn targeted_sync_cycle_rejects_unknown_target_device() {
    let (app, _tmp, _state) = build_sync_enabled_app();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/cycle/target")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "target_device_id": "0102030405060708"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(resp).await;
    assert_eq!(body["reason"], "invalid_target");
    assert!(
        body["detail"]
            .as_str()
            .expect("detail")
            .contains("target device not found"),
        "detail should explain target lookup failure: {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_returns_local_device_when_sync_is_disabled() {
    let (app, _tmp, state) = build_app();
    let local_device_id = state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("test Device identity");

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/sync/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let devices = body["devices"].as_array().expect("devices array");
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["device_id"], local_device_id);
    assert_eq!(devices[0]["is_self"], true);
    assert_eq!(devices[0]["connectivity_state"], "connected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_include_local_device_when_sync_is_enabled() {
    let (app, _tmp, state) = build_sync_enabled_app();
    let local_device_id = state
        .sync_device
        .read()
        .expect("sync device lock")
        .as_ref()
        .map(|device| hex::encode(device.device_id_short()))
        .expect("test Device identity");

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/sync/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(body["devices"].as_array().is_some_and(|devices| devices
        .iter()
        .any(|device| device["device_id"] == local_device_id && device["is_self"] == true)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_use_the_shared_connectivity_projection() {
    let (app, tmp, state) = build_sync_enabled_app();
    seed_device_list(
        tmp.path(),
        vec![
            local_device_entry(&state, "desktop"),
            sample_device(2, "idle-phone"),
        ],
    );

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/sync/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let devices = body["devices"].as_array().expect("devices");
    assert!(devices.iter().any(|device| {
        device["is_self"] == true && device["connectivity_state"] == "connected"
    }));
    assert!(devices.iter().any(|device| {
        device["device_id"] == "0202020202020202"
            && device["connectivity_state"] == "not_connected"
            && device["session_path"] == serde_json::Value::Null
            && device["session_rtt_ms"] == serde_json::Value::Null
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_activity_exposes_an_event_stream() {
    let (app, _tmp, _state) = build_sync_enabled_app();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/sync/devices/activity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enable_flag_persisted_across_restarts() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");

    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("write flag");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);

    assert!(
        state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst),
        "sync_enabled should be true when flag file exists"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enabling_existing_network_installs_one_coherent_device_runtime() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    let network_secret = rua_sync::identity::NetworkSecret::generate();
    network_secret.save(&sync_dir).expect("save network secret");
    let expected_network_id = hex::encode(network_secret.namespace_id().as_bytes());

    let state =
        AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Desktop);
    assert!(state.device_network.lock().await.is_none());
    let app = Router::new()
        .nest("/api", sync_router())
        .with_state(state.clone());

    let resp = request_empty(&app, "POST", "/api/sync/enable").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let runtime = state
        .device_network
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("Device Network Runtime");
    let runtime_endpoint_id = runtime.endpoint().id();
    assert_eq!(runtime.diagnostics().state, "active");
    assert_eq!(runtime.diagnostics().network_id, expected_network_id);
    assert_eq!(
        state
            .sync_node
            .lock()
            .await
            .as_ref()
            .expect("Sync consumer")
            .endpoint()
            .id(),
        runtime_endpoint_id
    );
    assert!(state
        .sync_transfer_session_manager
        .read()
        .expect("transfer session manager lock")
        .is_some());

    let resp = request_empty(&app, "GET", "/api/sync/diagnostics").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["device_network"]["state"], "active");
    assert_eq!(
        body["device_network"]["endpoint_id"],
        runtime_endpoint_id.to_string()
    );
    assert_eq!(body["device_network"]["network_id"], expected_network_id);
    assert_eq!(
        body["device_network"]["device_sessions"]["physical_dial_attempts"],
        0
    );
    assert_eq!(
        body["device_network"]["device_sessions"]["sessions"],
        serde_json::json!([])
    );

    state.clear_sync_node().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn creating_then_disabling_network_closes_and_clears_device_runtime() {
    let (app, _tmp, state) = build_app();
    let resp = request_empty(&app, "POST", "/api/sync/enable").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(state.device_network.lock().await.is_none());

    let resp = request_empty(&app, "POST", "/api/sync/network/create").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let endpoint = state
        .device_network
        .lock()
        .await
        .as_ref()
        .expect("Device Network Runtime")
        .endpoint()
        .clone();
    assert!(!endpoint.is_closed());

    let resp = request_empty(&app, "POST", "/api/sync/disable").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(endpoint.is_closed());
    assert!(state.device_network.lock().await.is_none());
    assert!(state.sync_node.lock().await.is_none());
    assert!(state
        .sync_transfer_session_manager
        .read()
        .expect("transfer session manager lock")
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paired_network_restart_rebuilds_runtime_with_stable_identity_and_membership() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("write flag");
    let network_secret = rua_sync::identity::NetworkSecret::generate();
    network_secret.save(&sync_dir).expect("save network secret");

    let first = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let first_runtime = first
        .device_network
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("first runtime");
    let first_endpoint = first_runtime.endpoint().clone();
    let first_diagnostics = first_runtime.diagnostics();
    first.clear_sync_node().await;
    assert!(first_endpoint.is_closed());
    drop(first);

    let restarted =
        AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let restarted_runtime = restarted
        .device_network
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("restarted runtime");
    let restarted_diagnostics = restarted_runtime.diagnostics();
    assert_eq!(
        restarted_diagnostics.local_device_id,
        first_diagnostics.local_device_id
    );
    assert_eq!(
        restarted_diagnostics.network_id,
        first_diagnostics.network_id
    );
    assert_eq!(restarted_diagnostics.state, "active");
    assert!(!restarted_runtime.endpoint().is_closed());

    restarted.clear_sync_node().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_device_updates_local_device_list_and_publishes_tombstone() {
    let (app, tmp, state) = build_sync_enabled_app();
    let old_namespace_id = state
        .sync_network_secret
        .read()
        .unwrap()
        .as_ref()
        .expect("network secret")
        .namespace_id();
    seed_device_list(
        tmp.path(),
        vec![
            local_device_entry(&state, "desktop"),
            sample_device(2, "old-phone"),
        ],
    );

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/devices/remove")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"device_id":"0202020202020202"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["mode"], "device_list");
    let device_list = read_device_list(tmp.path());
    assert_eq!(device_list.devices.len(), 1);
    assert!(device_list.contains_device(
        &state
            .sync_device
            .read()
            .unwrap()
            .as_ref()
            .expect("sync device")
            .device_id_short()
    ));
    assert!(!device_list.contains_device(&[2; 8]));
    let new_namespace_id = state
        .bound_sync_network()
        .await
        .expect("runtime rebound after remove")
        .namespace_id();
    assert_eq!(
        old_namespace_id, new_namespace_id,
        "device-list removal must not rotate namespace through a custom control protocol"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_device_closes_the_shared_session_and_prevents_new_streams() {
    let (app, tmp, state) = build_sync_enabled_app();
    let local_runtime = state
        .device_network
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("local Device Network Runtime");
    let local_device = local_device_entry(&state, "desktop");
    let network_secret = state
        .sync_network_secret
        .read()
        .expect("network secret lock")
        .as_ref()
        .expect("network secret")
        .to_bytes();

    let remote_dir = TempDir::new().expect("remote tempdir");
    let remote_sync_dir = remote_dir.path().join("sync");
    std::fs::create_dir_all(&remote_sync_dir).expect("remote sync dir");
    let mut local_peer = local_device.clone();
    local_peer.direct_addrs = endpoint_direct_addrs(local_runtime.endpoint());
    std::fs::write(
        remote_sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&DeviceList {
            devices: vec![local_peer],
        })
        .expect("serialize remote peer list"),
    )
    .expect("write remote peer list");
    let remote_secret = iroh::SecretKey::from_bytes(&[91; 32]);
    let remote_identity = rua_device_network::DeviceNetworkIdentity::new(remote_secret.clone());
    let remote_device_id = hex::encode(remote_identity.local_device_id());
    let remote_bootstrap = rua_device_network::DeviceNetworkBootstrap::bind(
        rua_device_network::DeviceNetworkPreset::Local,
        remote_identity,
        rua_device_network::BoundDeviceNetworkMembership::new(network_secret),
        rua_device_network::DevicePeerDirectory::from_sync_dir(&remote_sync_dir),
    )
    .await
    .expect("bind remote runtime");
    let remote_runtime =
        remote_bootstrap.spawn_with_command_handler(Vec::new(), Arc::new(EchoDeviceSessionHandler));
    let mut remote_addr = iroh::EndpointAddr::new(remote_runtime.endpoint().id());
    for addr in endpoint_direct_addrs(remote_runtime.endpoint()) {
        remote_addr
            .addrs
            .insert(iroh::TransportAddr::Ip(addr.parse().expect("socket addr")));
    }
    let remote_entry = DeviceEntry {
        device_id: remote_runtime.identity().local_device_id(),
        public_key: *remote_secret.public().as_bytes(),
        label: "remote-phone".to_string(),
        friendly_name: "Remote phone".to_string(),
        added_at_ms: 1,
        last_seen_ms: 1,
        direct_addrs: endpoint_direct_addrs(remote_runtime.endpoint()),
    };
    seed_device_list(tmp.path(), vec![local_device, remote_entry]);

    let response = local_runtime
        .session_manager()
        .command(
            &remote_device_id,
            remote_addr.clone(),
            b"before-remove",
            Duration::from_secs(5),
        )
        .await
        .expect("establish shared session");
    assert_eq!(response, b"before-remove");
    assert_eq!(
        local_runtime.connectivity_state(&remote_device_id).unwrap(),
        rua_device_network::DeviceConnectivityState::Connected
    );
    let devices_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sync/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let devices_body = json_body(devices_resp).await;
    let remote_device = devices_body["devices"]
        .as_array()
        .and_then(|devices| {
            devices
                .iter()
                .find(|device| device["device_id"] == remote_device_id)
        })
        .expect("remote Device projection");
    assert_eq!(remote_device["connectivity_state"], "connected");
    match remote_device["session_path"].as_str() {
        Some(path) => {
            assert!(matches!(path, "direct" | "relay" | "other"));
            assert!(remote_device["session_rtt_ms"].as_u64().is_some());
        }
        None => assert!(remote_device["session_rtt_ms"].is_null()),
    }

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/devices/remove")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "device_id": remote_device_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(local_runtime.connectivity().is_empty());

    let error = local_runtime
        .session_manager()
        .command(
            &hex::encode(remote_runtime.identity().local_device_id()),
            remote_addr,
            b"after-remove",
            Duration::from_secs(2),
        )
        .await
        .expect_err("removed Device must not open a new logical stream");
    assert!(matches!(
        error,
        rua_device_network::DeviceSessionError::PeerNotPaired(_)
    ));

    remote_runtime.shutdown().await.expect("shutdown remote");
    state.clear_sync_node().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_device_keeps_unreachable_devices_and_publishes_removed_device_tombstone() {
    let (app, tmp, state) = build_sync_enabled_app();
    let old_namespace_id = state
        .sync_network_secret
        .read()
        .unwrap()
        .as_ref()
        .expect("network secret")
        .namespace_id();
    let local_public_key = *state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .expect("sync device")
        .public_key()
        .as_bytes();
    seed_device_list(
        tmp.path(),
        vec![
            local_device_entry(&state, "desktop"),
            sample_device(2, "old-phone"),
            sample_device_with_public_key(4, local_public_key, "offline-tablet"),
        ],
    );

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/devices/remove")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"device_id":"0202020202020202"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["mode"], "device_list");
    let device_list = read_device_list(tmp.path());
    assert!(device_list.contains_device(&[4; 8]));
    assert!(!device_list.contains_device(&[2; 8]));
    assert!(!tmp.path().join("sync").join("pending_rekeys.json").exists());
    let new_namespace_id = state
        .bound_sync_network()
        .await
        .expect("runtime rebound after remove")
        .namespace_id();
    assert_eq!(old_namespace_id, new_namespace_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_device_updates_local_device_list() {
    let (app, tmp, state) = build_app();
    state
        .sync_enabled
        .store(true, std::sync::atomic::Ordering::SeqCst);
    seed_device_list(tmp.path(), vec![sample_device(2, "old-phone")]);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/devices/rename")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"device_id":"0202020202020202","friendly_name":"daily phone"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let device_list = read_device_list(tmp.path());
    assert_eq!(device_list.devices[0].friendly_name, "daily phone");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_device_emits_device_list_v4_when_sync_enabled() {
    let (app, tmp, state) = build_sync_enabled_app();
    seed_device_list(tmp.path(), vec![sample_device(2, "old-phone")]);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/devices/rename")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"device_id":"0202020202020202","friendly_name":"daily phone"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let changes = collect_v4_changes(&state).await;
    let rename_change = changes.iter().find(|change| {
        change.domain == "device_list"
            && change.record_id == "0202020202020202"
            && change.kind == "rename"
    });
    assert!(
        rename_change.is_some_and(|change| {
            change.payload["device_id"] == "0202020202020202"
                && change.payload["friendly_name"] == "daily phone"
        }),
        "device rename must append a device_list v4 rename change; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_device_publishes_device_list_tombstone() {
    let (app, tmp, state) = build_sync_enabled_app();
    seed_device_list(
        tmp.path(),
        vec![
            local_device_entry(&state, "desktop"),
            sample_device(2, "old-phone"),
        ],
    );

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/devices/remove")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"device_id":"0202020202020202"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let changes = collect_v4_changes(&state).await;
    let remove_change = changes.iter().find(|change| {
        change.domain == "device_list"
            && change.record_id == "0202020202020202"
            && change.kind == "remove"
    });
    assert!(
        remove_change.is_some_and(|change| change.payload["device_id"] == "0202020202020202"),
        "device removal must append a device_list tombstone for explicit iroh-docs fanout; got {changes:#?}"
    );
}

fn sample_device(id_byte: u8, friendly_name: &str) -> DeviceEntry {
    sample_device_with_public_key(id_byte, [id_byte; 32], friendly_name)
}

fn sample_device_with_public_key(
    id_byte: u8,
    public_key: [u8; 32],
    friendly_name: &str,
) -> DeviceEntry {
    DeviceEntry {
        device_id: [id_byte; 8],
        public_key,
        label: friendly_name.to_string(),
        friendly_name: friendly_name.to_string(),
        added_at_ms: 1_000,
        last_seen_ms: 1_000,
        direct_addrs: Vec::new(),
    }
}

fn local_device_entry(state: &AppState, friendly_name: &str) -> DeviceEntry {
    let device = state
        .sync_device
        .read()
        .unwrap()
        .as_ref()
        .expect("sync device")
        .clone();
    DeviceEntry {
        device_id: device.device_id_short(),
        public_key: *device.public_key().as_bytes(),
        label: friendly_name.to_string(),
        friendly_name: friendly_name.to_string(),
        added_at_ms: 1_000,
        last_seen_ms: 1_000,
        direct_addrs: Vec::new(),
    }
}

fn endpoint_direct_addrs(endpoint: &iroh::Endpoint) -> Vec<String> {
    endpoint
        .bound_sockets()
        .into_iter()
        .map(|socket| match socket {
            std::net::SocketAddr::V4(socket) if socket.ip().is_unspecified() => {
                std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, socket.port()))
            }
            std::net::SocketAddr::V6(socket) if socket.ip().is_unspecified() => {
                std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, socket.port()))
            }
            socket => socket,
        })
        .map(|socket| socket.to_string())
        .collect()
}

fn seed_device_list(root: &std::path::Path, devices: Vec<DeviceEntry>) {
    let path = root.join("sync").join("device_list.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&DeviceList { devices }).unwrap(),
    )
    .unwrap();
}

fn read_device_list(root: &std::path::Path) -> DeviceList {
    serde_json::from_str(
        &std::fs::read_to_string(root.join("sync").join("device_list.json")).unwrap(),
    )
    .unwrap()
}
