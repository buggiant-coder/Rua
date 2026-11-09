use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use std::sync::Arc;
use tempfile::TempDir;
use tower::util::ServiceExt;

use rua_server::endpoints::dto;
use rua_server::endpoints::local_models;
use rua_server::profiles::ServerProfile;
use rua_server::AppState;

fn build_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);

    state.local_model = Arc::new(
        rua_local_model::LocalModelManager::new_for_test(std::env::temp_dir())
            .with_scan_enabled(false)
            .with_llama_binary("sleep")
            .with_skip_health_check(true),
    );
    (state, tmp)
}

fn build_sync_enabled_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");

    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    state.local_model = Arc::new(
        rua_local_model::LocalModelManager::new_for_test(std::env::temp_dir())
            .with_scan_enabled(false)
            .with_llama_binary("sleep")
            .with_skip_health_check(true),
    );
    assert!(
        state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst),
        "sync should be enabled for device-local regression test"
    );
    (state, tmp)
}

fn router(state: AppState) -> Router {
    Router::new()
        .nest("/api", local_models::router())
        .with_state(state)
}

async fn read_json(body: Body) -> serde_json::Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
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
async fn server_status_initially_stopped() {
    let (state, _tmp) = build_state();
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/local-models/server")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp.into_body()).await;
    assert_eq!(body["running"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_then_status_then_stop() {
    let (state, _tmp) = build_state();
    let app = router(state.clone());

    let body = serde_json::json!({"port": 0});
    let start = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/local-models/server")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::OK);
    let started: dto::LocalModelServerStatus =
        serde_json::from_value(read_json(start.into_body()).await).unwrap();
    assert!(started.running);
    assert!(started.pid.is_some());

    let status_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/local-models/server")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status_body = read_json(status_resp.into_body()).await;
    assert_eq!(status_body["running"], true);

    let stop = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/local-models/server")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stop.status(), StatusCode::OK);

    let after = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/local-models/server")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let after_body = read_json(after.into_body()).await;
    assert_eq!(after_body["running"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_stop_server_does_not_emit_v4_when_sync_enabled() {
    let (state, _tmp) = build_sync_enabled_state();
    let app = router(state.clone());

    let body = serde_json::json!({"port": 0});
    let start = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/local-models/server")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::OK);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.is_empty(),
        "local model server start is device-local and must not append v4 sync changes; got {changes:#?}"
    );

    let stop = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/local-models/server")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stop.status(), StatusCode::OK);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let changes = collect_v4_changes(&state).await;
    assert!(
        changes.is_empty(),
        "local model server stop is device-local and must not append v4 sync changes; got {changes:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_returns_empty_when_scan_disabled() {
    let (state, _tmp) = build_state();
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/local-models/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp.into_body()).await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recommended_returns_non_empty_list() {
    let (state, _tmp) = build_state();
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/local-models/recommended")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp.into_body()).await;
    let arr = body.as_array().expect("response should be array");
    assert!(!arr.is_empty(), "should have recommended models");
    let first = &arr[0];
    assert!(first.get("id").is_some());
    assert!(first.get("name").is_some());
    assert!(first.get("size_bytes").is_some());
    assert!(first.get("source").is_some());
}
