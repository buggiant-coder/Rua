use std::sync::atomic::Ordering;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use iroh_blobs::Hash;
use iroh_docs::AuthorId;
use rua_server::endpoints::platform::router as platform_router;
use rua_server::endpoints::sync::router as sync_router;
use rua_server::state::{SyncRepairLane, SyncRepairSchedulerState};
use rua_server::sync_cycle::{
    load_transport_stats, record_transport_cycle, SyncTransportActiveCycle,
    SyncTransportCycleStats, SyncTransportCycleStatus, SyncTransportStatsState,
};
use rua_server::sync_peer_directory::SyncTransportFailureKind;
use rua_server::{AppState, ServerProfile};
use rua_sync::device_list::{DeviceEntry, DeviceList};
use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

fn build_app(profile: ServerProfile) -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), profile);
    let app = Router::new()
        .nest("/api", sync_router())
        .nest("/api", platform_router())
        .with_state(state.clone());
    (app, tmp, state)
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).expect("serialize body"))
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("oneshot")
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response JSON")
}

async fn enable_sync(app: &Router) {
    let resp = request(app, Method::POST, "/api/sync/enable", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = request(app, Method::POST, "/api/sync/network/create", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

async fn wait_for_no_active_repair(state: &AppState) {
    for _ in 0..40 {
        if state.sync_repair_diagnostics().active_lane.is_none() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("sync repair lane stayed active");
}

fn add_remote_peer(state: &AppState) {
    let remote = DeviceIdentity::generate();
    let device_list = DeviceList {
        devices: vec![DeviceEntry {
            device_id: remote.device_id_short(),
            public_key: remote.public_key().to_bytes(),
            label: "desktop".to_string(),
            friendly_name: "Remote".to_string(),
            added_at_ms: 1,
            last_seen_ms: 1,
            direct_addrs: Vec::new(),
        }],
    };
    let sync_dir = state.rua_root.join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(
        sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&device_list).expect("serialize device list"),
    )
    .expect("write device list");
}

fn author(byte: u8) -> AuthorId {
    AuthorId::from(&[byte; 32])
}

#[test]
fn repair_scheduler_keeps_highest_priority_pending_lane() {
    let mut scheduler = SyncRepairSchedulerState::default();

    assert!(scheduler.schedule(SyncRepairLane::Bulk));
    assert_eq!(scheduler.active_lane, Some(SyncRepairLane::Bulk));
    assert_eq!(scheduler.pending_lane, None);

    assert!(!scheduler.schedule(SyncRepairLane::Daily));
    assert_eq!(scheduler.pending_lane, Some(SyncRepairLane::Daily));

    assert!(!scheduler.schedule(SyncRepairLane::Interactive));
    assert_eq!(scheduler.pending_lane, Some(SyncRepairLane::Interactive));

    assert_eq!(scheduler.finish_active(), Some(SyncRepairLane::Interactive));
    assert_eq!(scheduler.active_lane, Some(SyncRepairLane::Interactive));
    assert_eq!(scheduler.pending_lane, None);
    assert_eq!(scheduler.finish_active(), None);
    assert_eq!(scheduler.active_lane, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_reports_configured_policy_effective_policy_and_runtime_state() {
    let (app, _tmp, state) = build_app(ServerProfile::MobileApk);
    enable_sync(&app).await;
    add_remote_peer(&state);

    let put = request(
        &app,
        Method::PUT,
        "/api/sync/power-policy",
        Some(json!({
            "mode": "balanced",
            "periodic_interval_secs": 300
        })),
    )
    .await;
    assert_eq!(put.status(), StatusCode::OK);

    state
        .sync_runtime
        .notify_background(std::time::Duration::from_secs(60))
        .await
        .expect("enter background grace");

    let body = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let resp = request(&app, Method::GET, "/api/sync/diagnostics", None).await;
            assert_eq!(resp.status(), StatusCode::OK);
            let body = read_json(resp).await;
            if body["inbox"]["repair_scan"]["last_successful_at_ms"]
                .as_u64()
                .is_some()
            {
                break body;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("initial Repair Scan appears in the nonblocking diagnostics snapshot");

    assert!(
        body["inbox"].is_object(),
        "keeps existing inbox diagnostics"
    );
    assert!(body["inbox"]["repair_scan"].is_object());
    assert!(body["inbox"]["repair_scan"]["last_successful_at_ms"]
        .as_u64()
        .is_some());
    assert!(body["inbox"]["repair_scan"]["next_due_at_ms"]
        .as_u64()
        .is_some());
    assert_eq!(body["inbox"]["repair_scan"]["last_scanned"], 0);
    assert_eq!(body["inbox"]["repair_scan"]["last_result"], "success");
    assert!(
        body["merge"].is_object(),
        "keeps existing merge diagnostics"
    );
    assert!(
        body["progress"].is_object(),
        "reports user-facing sync progress"
    );
    assert!(
        body["transport"].is_object(),
        "reports transport route statistics"
    );
    assert!(
        body["blob"].is_object(),
        "reports Blob download diagnostics separately"
    );
    assert!(
        body["stages"].is_object(),
        "reports correlated Sync stage diagnostics"
    );
    assert!(body["peers"].is_object(), "keeps existing peer diagnostics");
    assert_eq!(body["progress"]["state"], "up_to_date");
    assert_eq!(body["progress"]["is_caught_up"], true);
    assert_eq!(body["progress"]["pending_inbound"], 0);
    assert_eq!(body["progress"]["pending_conflicts"], 0);
    assert_eq!(body["transport"]["rolling"]["cycles"], 0);
    assert_eq!(body["blob"]["requests"], 0);
    assert_eq!(body["blob"]["successes"], 0);
    assert_eq!(body["blob"]["failures"], 0);
    assert_eq!(
        body["stages"]["recent_publications"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        body["stages"]["recent_inbound"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(body["device_network"]["state"], "active");
    assert_eq!(
        body["device_network"]["network_id"],
        hex::encode(
            state
                .sync_network_secret
                .read()
                .expect("network secret lock")
                .as_ref()
                .expect("network secret")
                .namespace_id()
                .as_bytes()
        )
    );
    assert_eq!(
        body["device_network"]["local_device_id"],
        hex::encode(
            state
                .sync_device
                .read()
                .expect("device lock")
                .as_ref()
                .expect("device")
                .device_id_short()
        )
    );
    assert_eq!(
        body["device_network"]["device_sessions"]["physical_dial_attempts"],
        0
    );
    assert_eq!(
        body["device_network"]["device_sessions"]["accepted_connections"],
        0
    );
    assert_eq!(body["device_network"]["gossip_neighbors"], 0);
    let connectivity = body["device_network"]["connectivity"]
        .as_array()
        .expect("connectivity projection");
    assert_eq!(connectivity.len(), 1);
    assert_eq!(connectivity[0]["state"], "not_connected");
    let sessions = body["device_network"]["device_sessions"]["sessions"]
        .as_array()
        .expect("Device Session diagnostics");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["state"], "not_connected");
    assert_eq!(sessions[0]["generation"], serde_json::Value::Null);
    assert_eq!(sessions[0]["active_logical_streams"], 0);
    assert_eq!(sessions[0]["path_class"], "unknown");
    assert_eq!(sessions[0]["tx_bytes"], 0);
    assert_eq!(sessions[0]["rx_bytes"], 0);
    assert_eq!(body["host_profile"], "mobile-apk");
    assert_eq!(body["policy"]["mode"], "balanced");
    assert_eq!(body["policy"]["periodic_interval_secs"], 300);
    assert_eq!(body["effective_policy"]["mode"], "balanced");
    assert_eq!(
        body["effective_policy"]["allow_background_scheduled_sync"],
        true
    );
    assert_eq!(body["runtime_state"]["state"], "warm_suspended");
    assert_eq!(body["runtime_state"]["pending_grace"], true);
    assert_eq!(body["background_grace"]["pending"], true);
    assert!(body["background_grace"]["remaining_ms"]
        .as_u64()
        .is_some_and(|remaining| remaining > 0));
    assert_eq!(body["warm_suspended"], true);
    assert_eq!(body["scheduled"]["allow_background_scheduled_sync"], true);
    assert_eq!(body["scheduled"]["allow_background_runtime"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_reports_syncing_when_transport_cycle_is_active() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    enable_sync(&app).await;

    let sync_dir = state.rua_root.join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(
        sync_dir.join("transport_stats.json"),
        serde_json::to_vec_pretty(&SyncTransportStatsState {
            active_cycle: Some(SyncTransportActiveCycle {
                started_at_ms: 12_000,
                peer_node_id: "peer-active".to_string(),
            }),
            ..Default::default()
        })
        .expect("serialize transport stats"),
    )
    .expect("write transport stats");

    let resp = request(&app, Method::GET, "/api/sync/diagnostics", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["progress"]["state"], "syncing");
    assert_eq!(body["progress"]["is_caught_up"], false);
    assert_eq!(
        body["transport"]["active_cycle"]["peer_node_id"],
        "peer-active"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_returns_while_sync_cycle_waits_on_unreachable_peer() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    enable_sync(&app).await;
    add_remote_peer(&state);

    let sync_node = std::sync::Arc::clone(&state.sync_node);
    let sync_dir = state.rua_root.join("sync");
    let cycle =
        tokio::spawn(
            async move { rua_server::sync_cycle::run_sync_cycle(&sync_node, &sync_dir).await },
        );

    let started = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if load_transport_stats(&state.rua_root.join("sync"))
                .active_cycle
                .is_some()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    assert!(
        started,
        "sync cycle did not record an active transport cycle"
    );

    let diagnostics = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        request(&app, Method::GET, "/api/sync/diagnostics", None),
    )
    .await;

    cycle.abort();
    let _ = cycle.await;

    let resp = diagnostics.expect("diagnostics should not wait for sync transport");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["progress"]["state"], "syncing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_reports_applying_changes_for_pending_inbound_work() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    enable_sync(&app).await;
    let _apply_guard = state.inbound_apply_lock.lock().await;

    state
        .inbound_inbox
        .record_remote_insert(author(7), b"pending", Hash::new(b"pending"))
        .expect("record pending inbound");

    let resp = request(&app, Method::GET, "/api/sync/diagnostics", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["progress"]["state"], "applying_changes");
    assert_eq!(body["progress"]["is_caught_up"], false);
    assert_eq!(body["progress"]["pending_inbound"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_drain_retries_then_dormants_unavailable_replica_log_content() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    enable_sync(&app).await;
    wait_for_no_active_repair(&state).await;
    state.sync_runtime.shutdown_unavailable().await;

    let orphaned_author = author(8);
    let orphaned_key = b"orphaned";
    state
        .inbound_inbox
        .record_remote_insert(orphaned_author, orphaned_key, Hash::new(b"orphaned"))
        .expect("record orphaned inbound");
    assert_eq!(
        state
            .inbound_inbox
            .diagnostics(10)
            .expect("inbox diagnostics")
            .counts
            .pending,
        1
    );

    let first_error = state
        .repair_scan_and_drain_sync_inbox()
        .await
        .expect_err("first unavailable content read remains retryable");
    assert!(first_error.to_string().contains("1 retryable"));
    let first_retry = state
        .inbound_inbox
        .diagnostics(10)
        .expect("inbox diagnostics after first retry");
    assert_eq!(first_retry.counts.retryable, 1);
    assert_eq!(first_retry.recent_errors[0].attempts, 1);
    let waiting_error = state
        .repair_scan_and_drain_sync_inbox()
        .await
        .expect_err("an outstanding retry must not become success during backoff");
    assert!(waiting_error.to_string().contains("1 retryable"));
    assert_eq!(
        state.inbound_inbox.diagnostics(1).unwrap().recent_errors[0].attempts,
        1
    );

    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let second_error = state
        .repair_scan_and_drain_sync_inbox()
        .await
        .expect_err("second unavailable content read remains retryable");
    assert!(second_error.to_string().contains("1 retryable"));
    let second_retry = state
        .inbound_inbox
        .diagnostics(10)
        .expect("inbox diagnostics after second retry");
    assert_eq!(second_retry.counts.retryable, 1);
    assert_eq!(second_retry.recent_errors[0].attempts, 2);

    tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
    state
        .repair_scan_and_drain_sync_inbox()
        .await
        .expect("source exhaustion converges to dormant");
    let diagnostics = state
        .inbound_inbox
        .diagnostics(10)
        .expect("inbox diagnostics after source exhaustion");
    assert_eq!(diagnostics.counts.retryable, 0);
    assert_eq!(diagnostics.counts.dormant, 1);
    assert_eq!(diagnostics.counts.discarded, 0);
    assert_eq!(diagnostics.recent_errors[0].attempts, 3);
    assert!(diagnostics.recent_errors[0]
        .last_error
        .contains("source content unavailable"));
    assert!(diagnostics.repair_scan.last_successful_at_ms.is_some());
    assert!(diagnostics.repair_scan.last_attempt_at_ms.is_some());
    assert!(diagnostics.repair_scan.last_duration_ms.is_some());
    assert_eq!(diagnostics.repair_scan.last_errors, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_retry_only_reactivates_failed_inbound_entries() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    enable_sync(&app).await;
    wait_for_no_active_repair(&state).await;
    state.sync_runtime.shutdown_unavailable().await;

    let failed_author = author(9);
    state
        .inbound_inbox
        .record_remote_insert(failed_author, b"failed", Hash::new(b"failed"))
        .expect("record failed inbound");
    state
        .inbound_inbox
        .mark_failed(failed_author, b"failed", "apply failed")
        .expect("mark failed inbound");
    state
        .inbound_inbox
        .record_remote_insert(author(10), b"dormant", Hash::new(b"dormant"))
        .expect("record dormant inbound");
    for _ in 0..3 {
        state
            .inbound_inbox
            .mark_retryable_limited(
                author(10),
                b"dormant",
                "content unavailable",
                0,
                3,
                rua_sync::inbound_inbox::RetryExhaustion::Dormant,
            )
            .expect("mark dormant inbound");
    }

    let resp = request(&app, Method::POST, "/api/sync/inbox/retry-failed", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["retried"], 1);

    let diagnostics = state
        .inbound_inbox
        .diagnostics(10)
        .expect("inbox diagnostics after manual retry");
    assert_eq!(diagnostics.counts.failed, 0);
    assert_eq!(diagnostics.counts.dormant, 1);
    assert_eq!(diagnostics.counts.retryable, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_reports_applying_changes_for_active_background_repair() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    enable_sync(&app).await;
    add_remote_peer(&state);
    {
        let mut guard = state
            .sync_repair_scheduler
            .lock()
            .expect("repair scheduler lock");
        guard.active_lane = Some(SyncRepairLane::Bulk);
        guard.pending_lane = Some(SyncRepairLane::Interactive);
    }

    let resp = request(&app, Method::GET, "/api/sync/diagnostics", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["progress"]["state"], "applying_changes");
    assert_eq!(body["progress"]["is_caught_up"], false);
    assert_eq!(body["progress"]["apply_active"], true);
    assert_eq!(body["progress"]["active_lane"], "bulk");
    assert_eq!(body["progress"]["pending_lane"], "interactive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_reports_waiting_for_content_for_dormant_inbound_work() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    enable_sync(&app).await;
    wait_for_no_active_repair(&state).await;
    state
        .sync_runtime
        .warm_suspend()
        .await
        .expect("stop background sync runtime");

    state
        .inbound_inbox
        .record_remote_insert(author(7), b"dormant", Hash::new(b"dormant"))
        .expect("record inbound");
    for _ in 0..24 {
        state
            .inbound_inbox
            .mark_retryable(author(7), b"dormant", "entry content not readable yet", 0)
            .expect("mark retryable");
    }
    let dormant = state
        .inbound_inbox
        .mark_dormant_retryable_backlog(u64::MAX, 24, 10)
        .expect("mark dormant");
    assert_eq!(dormant, 1);

    let resp = request(&app, Method::GET, "/api/sync/diagnostics", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["progress"]["state"], "waiting_for_content");
    assert_eq!(body["progress"]["is_caught_up"], false);
    assert_eq!(body["progress"]["pending_inbound"], 0);
    assert_eq!(body["progress"]["retryable_inbound"], 0);
    assert_eq!(body["progress"]["dormant_inbound"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_reports_needs_attention_for_failed_last_cycle() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    enable_sync(&app).await;
    add_remote_peer(&state);

    record_transport_cycle(
        &state.rua_root.join("sync"),
        SyncTransportCycleStats {
            completed_at_ms: 10_000,
            peer_node_id: "peer-failed".to_string(),
            status: SyncTransportCycleStatus::Failure,
            error: Some("timeout".to_string()),
            failure_kind: Some(SyncTransportFailureKind::Internal),
            events_received: 0,
            events_sent: 0,
            connect_ms: 0,
            process_ms: 0,
            duration_ms: 5_000,
            route: rua_sync::transport::SyncRouteKind::Unknown,
            direct_tx_bytes: 0,
            direct_rx_bytes: 0,
            relay_tx_bytes: 0,
            relay_rx_bytes: 0,
            direct_rtt_ms: None,
            relay_rtt_ms: None,
            warnings: Vec::new(),
        },
    );

    let resp = request(&app, Method::GET, "/api/sync/diagnostics", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["progress"]["state"], "needs_attention");
    assert_eq!(body["progress"]["is_caught_up"], true);
    assert_eq!(body["progress"]["last_cycle_status"], "failure");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_reports_waiting_for_device_when_peer_is_unavailable() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    enable_sync(&app).await;
    add_remote_peer(&state);

    record_transport_cycle(
        &state.rua_root.join("sync"),
        SyncTransportCycleStats {
            completed_at_ms: 10_000,
            peer_node_id: "peer-offline".to_string(),
            status: SyncTransportCycleStatus::Failure,
            error: Some("peer unreachable: connection refused".to_string()),
            failure_kind: Some(SyncTransportFailureKind::PeerUnavailable),
            events_received: 0,
            events_sent: 0,
            connect_ms: 0,
            process_ms: 0,
            duration_ms: 5_000,
            route: rua_sync::transport::SyncRouteKind::Unknown,
            direct_tx_bytes: 0,
            direct_rx_bytes: 0,
            relay_tx_bytes: 0,
            relay_rx_bytes: 0,
            direct_rtt_ms: None,
            relay_rtt_ms: None,
            warnings: Vec::new(),
        },
    );

    let resp = request(&app, Method::GET, "/api/sync/diagnostics", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["progress"]["state"], "waiting_for_device");
    assert_eq!(body["progress"]["is_caught_up"], true);
    assert_eq!(body["progress"]["last_cycle_status"], "failure");
    assert_eq!(
        body["transport"]["last_cycle"]["failure_kind"],
        "peer_unavailable"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_reports_identity_mismatch_instead_of_caught_up() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    enable_sync(&app).await;
    let sync_dir = state.rua_root.join("sync");
    let different_secret = NetworkSecret::generate();
    different_secret
        .save(&sync_dir)
        .expect("replace disk network secret");

    let resp = request(&app, Method::GET, "/api/sync/diagnostics", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["progress"]["state"], "identity_mismatch");
    assert_eq!(body["progress"]["is_caught_up"], false);
    assert_eq!(body["identity"]["status"], "identity_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_reports_disabled_progress_when_sync_is_off() {
    let (app, _tmp, _state) = build_app(ServerProfile::Desktop);

    let resp = request(&app, Method::GET, "/api/sync/diagnostics", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["progress"]["state"], "disabled");
    assert_eq!(body["progress"]["is_caught_up"], false);
    assert_eq!(body["transport"]["rolling"]["cycles"], 0);
    assert_eq!(body["peers"]["total_peers"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_reports_not_paired_for_enabled_device_without_network() {
    let (app, _tmp, _state) = build_app(ServerProfile::Desktop);

    let enable = request(&app, Method::POST, "/api/sync/enable", None).await;
    assert_eq!(enable.status(), StatusCode::OK);

    let resp = request(&app, Method::GET, "/api/sync/diagnostics", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["identity"]["status"], "not_paired");
    assert_eq!(body["progress"]["state"], "not_paired");
    assert_eq!(body["progress"]["is_caught_up"], false);
    assert_eq!(body["peers"]["total_peers"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_lifecycle_background_keeps_runtime_active_in_diagnostics() {
    let (app, _tmp, state) = build_app(ServerProfile::Desktop);
    enable_sync(&app).await;

    let background = request(
        &app,
        Method::POST,
        "/api/platform/lifecycle",
        Some(json!({ "event": "background" })),
    )
    .await;
    assert_eq!(background.status(), StatusCode::OK);

    let resp = request(&app, Method::GET, "/api/sync/diagnostics", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["host_profile"], "desktop");
    assert_eq!(body["runtime_state"]["state"], "active");
    assert_eq!(body["background_grace"]["pending"], false);
    assert_eq!(body["warm_suspended"], false);
    assert!(state.sync_enabled.load(Ordering::SeqCst));
}
