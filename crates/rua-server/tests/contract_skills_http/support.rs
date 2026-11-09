use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{market, skill_update, skills, skills_hub};
use rua_server::middleware::agent_context_middleware;
use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    pub tmp: TempDir,
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    seed_agent(&tmp, "skills_alpha");
    seed_agent(&tmp, "skills_beta");

    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let app = Router::new()
        .nest("/api", skills::shared_router())
        .nest("/api", skills::agent_scoped_router())
        .nest("/api", skills_hub::register_hub_routes())
        .nest("/api", skill_update::router())
        .nest("/api", market::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);

    TestApp { app, tmp }
}

fn seed_agent(tmp: &TempDir, agent_id: &str) {
    let agent_dir = tmp.path().join("workspaces").join(agent_id);
    std::fs::create_dir_all(agent_dir.join("skills")).expect("agent skills dir");
    std::fs::write(
        agent_dir.join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": agent_id,
            "name": agent_id,
            "description": "",
            "workspace_dir": "",
            "language": "",
            "system_prompt_files": [],
            "tools": {},
        }))
        .expect("agent json"),
    )
    .expect("write agent json");
}

pub fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n")
}

pub async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    agent_id: Option<&str>,
    body: Option<Value>,
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

pub async fn request_json(
    app: &Router,
    method: Method,
    uri: &str,
    agent_id: Option<&str>,
    body: Option<Value>,
    expected: StatusCode,
) -> Value {
    let resp = request(app, method, uri, agent_id, body).await;
    assert_eq!(resp.status(), expected);
    read_json(resp).await
}

pub fn skill_names(body: &Value) -> Vec<String> {
    body.as_array()
        .expect("skills list")
        .iter()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}
