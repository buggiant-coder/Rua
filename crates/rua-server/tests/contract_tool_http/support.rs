use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::path::PathBuf;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::tools::agent_scoped_router;
use rua_server::middleware::agent_context_middleware;
use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    pub state: AppState,
    _tmp: TempDir,
}

impl TestApp {
    pub fn agent_json_path(&self, agent_id: &str) -> PathBuf {
        self._tmp
            .path()
            .join("workspaces")
            .join(agent_id)
            .join("agent.json")
    }
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    seed_agent(
        &tmp,
        "tool_contract_agent",
        json!({
            "bash": {
                "name": "bash",
                "enabled": true,
                "description": "Execute shell commands",
                "display_to_user": true,
                "async_execution": false,
                "config": {
                    "cwd_policy": "workspace",
                    "max_output_chars": 12000
                }
            }
        }),
    );
    seed_agent(&tmp, "tool_contract_other_agent", json!({}));
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let app = Router::new()
        .nest("/api", agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());

    TestApp {
        app,
        state,
        _tmp: tmp,
    }
}

fn seed_agent(tmp: &TempDir, agent_id: &str, builtin_tools: Value) {
    let mut configured =
        rua_server::services::collaboration_runtime::server_tool_catalog().unwrap();
    configured.extend(builtin_tools.as_object().unwrap().clone());
    let builtin_tools = Value::Object(configured);
    let agent_dir = tmp.path().join("workspaces").join(agent_id);
    std::fs::create_dir_all(&agent_dir).expect("agent workspace");
    std::fs::write(
        agent_dir.join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": agent_id,
            "name": agent_id,
            "description": "",
            "workspace_dir": "",
            "language": "",
            "system_prompt_files": [],
            "tools": {
                "builtin_tools": builtin_tools,
            },
        }))
        .expect("agent json"),
    )
    .expect("write agent json");
}

pub async fn request(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Response {
    request_with_agent(app, method, uri, body, None).await
}

pub async fn request_with_agent(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    agent_id: Option<&str>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(agent_id) = agent_id {
        builder = builder.header("x-agent-id", agent_id);
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
