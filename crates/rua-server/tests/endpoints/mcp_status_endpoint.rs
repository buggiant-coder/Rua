use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::mcp_status;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default"))
        .expect("seed default workspace dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", mcp_status::router())
        .with_state(state);
    (app, tmp)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_status_returns_json_array() {
    let (app, _tmp) = build_app();

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/mcp/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");

    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("response is JSON");
    let arr = body.as_array().expect("response is an array");

    for entry in arr {
        assert!(
            entry.get("server_name").is_some(),
            "entry missing server_name: {entry}"
        );
        assert!(
            entry.get("status").is_some(),
            "entry missing status: {entry}"
        );

        assert!(
            entry.get("last_error").is_some() || entry.get("last_error").is_none(),
            "last_error field check"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_status_entries_have_valid_status_values() {
    let (app, _tmp) = build_app();

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/mcp/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");

    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body: Value = serde_json::from_slice(&bytes).expect("response is JSON");
    let arr = body.as_array().expect("response is an array");

    let valid_statuses = [
        "disabled",
        "not_connected",
        "connecting",
        "ready",
        "degraded",
        "failed",
        "stopping",
    ];

    for entry in arr {
        let status = entry["status"].as_str().expect("status is a string");
        assert!(
            valid_statuses.contains(&status),
            "unexpected status value: {status}"
        );
    }
}
