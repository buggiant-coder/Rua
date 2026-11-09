use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{mcp, mcp_oauth};
use rua_server::middleware::agent_context_middleware;
use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    pub state: AppState,
    tmp: TempDir,
}

impl TestApp {
    pub fn data_dir(&self) -> &Path {
        self.tmp.path()
    }
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    build_app_from_tempdir(tmp)
}

pub fn build_sync_enabled_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("sync enabled flag");
    rua_sync::identity::NetworkSecret::generate()
        .save(&sync_dir)
        .expect("network secret");
    build_app_from_tempdir(tmp)
}

fn build_app_from_tempdir(tmp: TempDir) -> TestApp {
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let api = Router::new()
        .merge(mcp::root_router())
        .merge(mcp::agent_scoped_router())
        .merge(mcp_oauth::root_router())
        .merge(mcp_oauth::agent_scoped_router());
    let app = Router::new()
        .nest("/api", api)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());

    TestApp { app, state, tmp }
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
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).expect("response json")
}

pub async fn read_text(resp: Response) -> String {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}
