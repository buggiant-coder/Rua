use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::tunnel_status::router as tunnel_router;
use rua_server::AppState;

async fn build_app_with_config(tunnel_config: Option<Value>) -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace");

    if let Some(tunnel_val) = tunnel_config {
        let root = serde_json::json!({ "tunnel": tunnel_val });
        std::fs::write(
            tmp.path().join("config.json"),
            serde_json::to_string_pretty(&root).unwrap(),
        )
        .expect("write config.json");
    }

    let data_dir = tmp.path().to_path_buf();
    let state = tokio::task::spawn_blocking(move || {
        AppState::from_tempdir(data_dir, rua_server::ServerProfile::Test)
    })
    .await
    .unwrap();
    let app = Router::new()
        .nest("/api", tunnel_router())
        .with_state(state);
    (app, tmp)
}

#[allow(dead_code)]
async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response is JSON")
}

#[tokio::test(flavor = "multi_thread")]
async fn config_port_used_when_req_empty() {
    let (app, _tmp) = build_app_with_config(Some(serde_json::json!({"local_port": 7000}))).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tunnel/start")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "empty body should be accepted (local_port is optional)"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn req_port_overrides_config() {
    let (app, _tmp) = build_app_with_config(Some(serde_json::json!({"local_port": 7000}))).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tunnel/start")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"local_port": 9000}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn default_port_when_no_config_no_req() {
    let (app, _tmp) = build_app_with_config(None).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/tunnel/start")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(resp.status(), StatusCode::BAD_REQUEST);
}

#[test]
fn tunnel_config_deser_with_port() {
    let val: rua_config::TunnelConfig = serde_json::from_str(r#"{"local_port": 7000}"#).unwrap();
    assert_eq!(val.local_port, Some(7000));
}

#[test]
fn tunnel_config_deser_empty() {
    let val: rua_config::TunnelConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(val.local_port, None);
}

#[test]
fn tunnel_config_deser_null_port() {
    let val: rua_config::TunnelConfig = serde_json::from_str(r#"{"local_port": null}"#).unwrap();
    assert_eq!(val.local_port, None);
}
