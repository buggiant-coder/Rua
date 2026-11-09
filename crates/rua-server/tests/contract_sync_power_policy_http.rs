use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::sync::router as sync_router;
use rua_server::{AppState, ServerProfile};
use rua_sync::identity::NetworkSecret;

fn build_app(profile: ServerProfile) -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), profile);
    let app = Router::new()
        .nest("/api", sync_router())
        .with_state(state.clone());
    (app, tmp, state)
}

fn build_sync_enabled_app(profile: ServerProfile) -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), profile);
    assert!(
        state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst),
        "sync should be enabled for power-policy device-local regression test"
    );
    let app = Router::new()
        .nest("/api", sync_router())
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
async fn mobile_apk_policy_get_returns_host_defaults_without_creating_file() {
    let (app, tmp, _state) = build_app(ServerProfile::MobileApk);

    let resp = request(&app, Method::GET, "/api/sync/power-policy", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;

    assert_eq!(body["host_profile"], "mobile-apk");
    assert!(body["stored_policy"].is_null());
    assert_eq!(body["effective_policy"]["mode"], "power_saver");
    assert_eq!(body["effective_policy"]["periodic_interval_secs"], 600);
    assert_eq!(body["effective_policy"]["background_flush_grace_secs"], 60);
    assert_eq!(body["effective_policy"]["allow_background_runtime"], false);
    assert_eq!(
        body["effective_policy"]["allow_background_scheduled_sync"],
        false
    );
    assert!(!tmp.path().join("sync").join("power_policy.json").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_put_persists_raw_config_and_get_returns_effective_policy() {
    let (app, tmp, _state) = build_app(ServerProfile::MobileApk);

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
    let put_body = read_json(put).await;
    assert_eq!(put_body["stored_policy"]["mode"], "balanced");
    assert_eq!(put_body["stored_policy"]["periodic_interval_secs"], 300);
    assert_eq!(put_body["effective_policy"]["mode"], "balanced");
    assert_eq!(put_body["effective_policy"]["periodic_interval_secs"], 300);
    assert_eq!(
        put_body["effective_policy"]["allow_background_scheduled_sync"],
        true
    );

    let path = tmp.path().join("sync").join("power_policy.json");
    let raw: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("saved policy")).unwrap();
    assert_eq!(raw["mode"], "balanced");
    assert_eq!(raw["periodic_interval_secs"], 300);

    let get = request(&app, Method::GET, "/api/sync/power-policy", None).await;
    assert_eq!(get.status(), StatusCode::OK);
    let get_body = read_json(get).await;
    assert_eq!(get_body, put_body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_put_does_not_emit_v4_when_sync_enabled() {
    let (app, _tmp, state) = build_sync_enabled_app(ServerProfile::MobileApk);

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

    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.is_empty(),
        "sync power policy is device-local and PUT must not append v4 sync changes; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_full_export_endpoint_emits_current_v4_state_when_sync_enabled() {
    let (app, _tmp, state) = build_sync_enabled_app(ServerProfile::Desktop);
    let workspace = state.rua_root.join("workspaces").join("default");
    std::fs::create_dir_all(workspace.join("prompts")).expect("prompt dir");
    std::fs::write(
        workspace.join("prompts/AGENT.md"),
        "# repair full export marker\n",
    )
    .expect("write repair marker");

    let resp = request(&app, Method::POST, "/api/sync/repair/full-export", None).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["exported"], true);
    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.iter().any(|change| {
            change.domain == "agent_prompt_file"
                && change.record_id == "default/root/AGENT.md"
                && change.payload["content"] == "# repair full export marker\n"
        }),
        "repair full export should append current portable v4 state; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_full_export_endpoint_requires_sync_enabled() {
    let (app, _tmp, _state) = build_app(ServerProfile::Desktop);

    let resp = request(&app, Method::POST, "/api/sync/repair/full-export", None).await;

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = read_json(resp).await;
    assert_eq!(body["reason"], "not_enabled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_full_export_endpoint_refuses_identity_mismatch() {
    let (app, tmp, state) = build_sync_enabled_app(ServerProfile::Desktop);
    NetworkSecret::generate()
        .save(&tmp.path().join("sync"))
        .expect("replace disk network secret");

    let resp = request(&app, Method::POST, "/api/sync/repair/full-export", None).await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["exported"], false);
    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.is_empty(),
        "identity mismatch must not append opaque changes encrypted for a different namespace"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_put_rejects_bad_mode_with_sync_error_shape() {
    let (app, _tmp, _state) = build_app(ServerProfile::Test);

    let resp = request(
        &app,
        Method::PUT,
        "/api/sync/power-policy",
        Some(json!({ "mode": "turbo" })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = read_json(resp).await;
    assert_eq!(body["reason"], "invalid_power_policy");
    assert!(body["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("turbo")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_includes_effective_power_policy() {
    let (app, _tmp, _state) = build_app(ServerProfile::Headless);

    let resp = request(&app, Method::GET, "/api/sync/status", None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["host_profile"], "headless");
    assert_eq!(body["effective_power_policy"]["mode"], "realtime");
    assert_eq!(
        body["effective_power_policy"]["allow_background_runtime"],
        true
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_returns_only_the_persisted_enabled_flag() {
    let (app, _tmp, _state) = build_app(ServerProfile::MobileApk);

    let disabled = request(&app, Method::GET, "/api/sync/config", None).await;
    assert_eq!(disabled.status(), StatusCode::OK);
    let disabled_body = read_json(disabled).await;
    assert_eq!(disabled_body, json!({ "enabled": false }));

    let enable = request(&app, Method::POST, "/api/sync/enable", None).await;
    assert_eq!(enable.status(), StatusCode::OK);

    let enabled = request(&app, Method::GET, "/api/sync/config", None).await;
    assert_eq!(enabled.status(), StatusCode::OK);
    let enabled_body = read_json(enabled).await;
    assert_eq!(enabled_body, json!({ "enabled": true }));
}

#[test]
fn legacy_mobile_profile_maps_to_mobile_apk_sync_host() {
    let profile: ServerProfile = "mobile".parse().expect("legacy mobile profile");
    assert_eq!(profile, ServerProfile::MobileApk);
    assert_eq!(profile.to_string(), "mobile-apk");
    assert_eq!(profile.sync_runtime_host_profile().as_str(), "mobile-apk");
}
