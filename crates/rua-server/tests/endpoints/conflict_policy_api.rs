use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::sync::router as sync_router;
use rua_server::AppState;

fn build_app() -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", sync_router())
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_conflict_policy_defaults_to_silent() {
    let (app, _tmp, _state) = build_app();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/sync/conflict-policy")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["policy"], "silent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_conflict_policy_to_review() {
    let (app, _tmp, _state) = build_app();

    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/sync/conflict-policy")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"policy":"review"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["policy"], "review");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_conflicts_empty_by_default() {
    let (app, _tmp, _state) = build_app();

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/sync/conflicts/pending")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["count"], 0);
    assert!(body["conflicts"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_and_resolve_conflict_via_api() {
    let (app, tmp, state) = build_app();

    let live_file = tmp.path().join("test_prompt.txt");
    std::fs::write(&live_file, "merged winner").unwrap();

    state
        .sidecar_store
        .record_conflict(rua_sync::sidecar::ConflictEntry {
            id: "api-conflict-1".into(),
            live_path: live_file.to_string_lossy().into_owned(),
            record_id: "prompt:test".into(),
            domain: "prompt".into(),
            winner_text: "merged winner".into(),
            loser_text: "loser text".into(),
            winner_hlc: rua_sync::hlc::HlcTimestamp {
                wall_ms: 1000,
                counter: 0,
                device_id: [0; 8],
            },
            created_at_ms: 1000,
        })
        .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/sync/conflicts/pending")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["count"], 1);
    assert_eq!(body["conflicts"][0]["id"], "api-conflict-1");
    assert_eq!(body["conflicts"][0]["winner_text"], "merged winner");
    assert_eq!(body["conflicts"][0]["loser_text"], "loser text");

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sync/conflicts/resolve")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"id":"api-conflict-1","chosen":"user resolved content"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["resolved"], true);

    let content = std::fs::read_to_string(&live_file).unwrap();
    assert_eq!(content, "user resolved content");

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/sync/conflicts/pending")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = json_body(resp).await;
    assert_eq!(body["count"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_policy_back_to_silent() {
    let (app, _tmp, _state) = build_app();

    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/sync/conflict-policy")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"policy":"review"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/sync/conflict-policy")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"policy":"silent"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["policy"], "silent");

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/sync/conflict-policy")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = json_body(resp).await;
    assert_eq!(body["policy"], "silent");
}
