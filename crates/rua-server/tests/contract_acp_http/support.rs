use axum::body::Body;
use axum::http::{Method, Request};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    pub state: AppState,
    _tmp: TempDir,
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    state.auth = std::sync::Arc::new(rua_server::auth::AuthService::new_disabled());
    let app = rua_server::router::build_app(state.clone(), None);
    TestApp {
        app,
        state,
        _tmp: tmp,
    }
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

pub async fn create_agent(app: &Router, id: &str, name: &str) {
    let resp = request(
        app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": id,
            "name": name,
            "description": ""
        })),
        Some("default"),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::CREATED);
}

pub fn runner_body(key: &str, command: &str) -> Value {
    json!({
        "key": key,
        "enabled": false,
        "command": command,
        "args": ["--version"],
        "env": {},
        "trusted": false,
        "tool_parse_mode": "call_detail",
        "stdio_buffer_limit_bytes": 1048576
    })
}

pub fn mock_acp_agent_path() -> String {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rua-acp/tests/fixtures/mock_acp_agent.py");
    path.to_string_lossy().to_string()
}

pub fn mock_acp_command() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}
