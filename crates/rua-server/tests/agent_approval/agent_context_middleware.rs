use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{any, get};
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde::Serialize;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::extractors::AgentExtractorContext;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

#[derive(Serialize)]
struct EchoResponse {
    agent_id: String,
    workspace_dir: String,
}

async fn echo(ctx: AgentExtractorContext) -> impl IntoResponse {
    Json(EchoResponse {
        agent_id: ctx.agent_id.clone(),
        workspace_dir: ctx.runtime.workspace_dir().display().to_string(),
    })
}

fn build_test_app(state: AppState) -> Router {
    Router::new()
        .route("/api/echo", get(echo))
        .route("/api/chats/{session_id}", any(echo))
        .route("/api/sessions/{session_id}", any(echo))
        .route("/api/channels/{channel}/echo", get(echo))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

async fn read_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("body is json")
}

fn build_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    (state, tmp)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_agent_id_from_header_wins() {
    let (state, _guard) = build_state();
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/echo")
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let json = read_json(resp).await;
    assert_eq!(json["agent_id"], "default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_agent_id_from_query_wins() {
    let (state, _guard) = build_state();
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/echo?agent_id=default&other=1")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let json = read_json(resp).await;
    assert_eq!(json["agent_id"], "default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_takes_precedence_over_query() {
    let (state, _guard) = build_state();
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/echo?agent_id=other-id")
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let json = read_json(resp).await;
    assert_eq!(json["agent_id"], "default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_id_from_path_does_not_override_active_agent() {
    let (state, _guard) = build_state();
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/chats/sess-abc")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let json = read_json(resp).await;
    assert_eq!(json["agent_id"], "default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_from_path_does_not_override_active_agent() {
    let (state, _guard) = build_state();
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/channels/dingtalk/echo")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let json = read_json(resp).await;
    assert_eq!(json["agent_id"], "default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fallback_to_active_agent_when_no_hint_present() {
    let (state, _guard) = build_state();
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/echo")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let json = read_json(resp).await;
    assert_eq!(json["agent_id"], "default");

    let ws_dir: String = json["workspace_dir"].as_str().unwrap().to_string();
    assert!(!ws_dir.is_empty(), "workspace_dir should not be empty");
    assert!(
        PathBuf::from(&ws_dir)
            .components()
            .any(|c| c.as_os_str() == "default"),
        "workspace_dir should include the default agent segment, got {ws_dir}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_header_value_is_treated_as_missing() {
    let (state, _guard) = build_state();
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/echo")
        .header("X-Agent-Id", "   ")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let json = read_json(resp).await;
    assert_eq!(json["agent_id"], "default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_path_returns_404_without_panic() {
    let (state, _guard) = build_state();
    let app = build_test_app(state);

    let req = Request::builder()
        .uri("/api/totally/unknown")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_context_extension_is_visible_to_classic_extension_extractor() {
    use axum::Extension;

    async fn extension_echo(Extension(ctx): Extension<AgentExtractorContext>) -> impl IntoResponse {
        Json(EchoResponse {
            agent_id: ctx.agent_id.clone(),
            workspace_dir: ctx.runtime.workspace_dir().display().to_string(),
        })
    }

    let (state, _guard) = build_state();
    let app = Router::new()
        .route("/api/legacy", get(extension_echo))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/legacy")
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let json = read_json(resp).await;
    assert_eq!(json["agent_id"], "default");
}

#[allow(dead_code)]
const fn _ctx_runtime_is_arc() {
    fn _assert<T: Send + Sync>(_: Arc<T>) {}
}
