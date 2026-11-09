use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{chats, console, messages};
use rua_server::middleware::agent_context_middleware;
use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    _tmp: TempDir,
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("default workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let app = Router::new()
        .merge(chats::root_router())
        .merge(chats::agent_scoped_router())
        .merge(messages::router())
        .merge(messages::channel_webhook_router())
        .merge(console::agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);

    TestApp { app, _tmp: tmp }
}

pub async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    agent_id: Option<&str>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(agent_id) = agent_id {
        builder = builder.header("X-Agent-Id", agent_id);
    }
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).expect("request json"))
        }
        None => Body::empty(),
    };

    app.clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("oneshot")
}

pub async fn read_json(resp: Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response json")
}
