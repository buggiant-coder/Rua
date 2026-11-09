use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::config_channels;
use rua_server::middleware::agent_context_middleware;
use rua_server::qrcode_auth::{PollResult, QRCodeAuthHandler, QRCodeResult};
use rua_server::AppState;

struct FakeHandler;

#[async_trait]
impl QRCodeAuthHandler for FakeHandler {
    async fn fetch_qrcode(&self, _agent_id: &str) -> Result<QRCodeResult, rua_server::ApiError> {
        Ok(QRCodeResult {
            scan_url: "https://example.com/scan".into(),
            poll_token: "tok-abc".into(),
        })
    }

    async fn poll_status(
        &self,
        token: &str,
        _agent_id: &str,
    ) -> Result<PollResult, rua_server::ApiError> {
        Ok(PollResult {
            status: if token == "tok-abc" {
                "success".into()
            } else {
                "waiting".into()
            },
            credentials: serde_json::json!({"secret": "s3cr3t"}),
        })
    }
}

fn build_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed default workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    (state, tmp)
}

fn app(state: AppState) -> Router {
    let sub = config_channels::root_router();
    Router::new()
        .nest("/api", sub)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .expect("request")
}

async fn json_body(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("non-JSON: {} (body={})", e, String::from_utf8_lossy(&bytes)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unregistered_channel_returns_404() {
    let (state, _tmp) = build_state();
    let router = app(state);

    let resp = router
        .oneshot(get("/api/config/channels/nonexistent/qrcode"))
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registered_handler_qrcode_returns_200() {
    let (state, _tmp) = build_state();
    state
        .qrcode_registry
        .register("test", Arc::new(FakeHandler))
        .await;
    let router = app(state);

    let resp = router
        .oneshot(get("/api/config/channels/test/qrcode"))
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = json_body(resp).await;
    assert!(body["qrcode_img"].is_string());
    assert!(!body["qrcode_img"].as_str().unwrap().is_empty());
    assert_eq!(body["poll_token"], "tok-abc");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registered_handler_poll_status_returns_200() {
    let (state, _tmp) = build_state();
    state
        .qrcode_registry
        .register("test", Arc::new(FakeHandler))
        .await;
    let router = app(state);

    let resp = router
        .oneshot(get("/api/config/channels/test/qrcode/status?token=tok-abc"))
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = json_body(resp).await;
    assert_eq!(body["status"], "success");
    assert_eq!(body["credentials"]["secret"], "s3cr3t");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poll_status_unregistered_channel_returns_404() {
    let (state, _tmp) = build_state();
    let router = app(state);

    let resp = router
        .oneshot(get("/api/config/channels/ghost/qrcode/status?token=x"))
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
