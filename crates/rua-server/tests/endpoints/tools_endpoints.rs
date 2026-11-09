use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::tools::agent_scoped_router;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn seed_default_agent(tmp: &TempDir, body: Value) {
    let dir = tmp.path().join("workspaces").join("default");
    std::fs::create_dir_all(&dir).expect("seed default workspace");
    std::fs::write(
        dir.join("agent.json"),
        serde_json::to_string_pretty(&body).unwrap(),
    )
    .expect("write agent.json");
}

fn agent_skeleton(builtin_tools: Value) -> Value {
    json!({
        "id": "default",
        "name": "default",
        "description": "",
        "workspace_dir": "",
        "language": "",
        "system_prompt_files": [],
        "tools": {
            "builtin_tools": builtin_tools,
        },
    })
}

fn build_app(tmp: &TempDir) -> Router {
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    Router::new()
        .nest("/api", agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Agent-Id", "default");
    let body = match body {
        Some(json) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&json).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

fn read_agent_json(tmp: &TempDir) -> Value {
    let raw = std::fs::read_to_string(tmp.path().join("workspaces/default/agent.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_returns_explicitly_configured_tools() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req("GET", "/api/agents/default/tools", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"bash"), "expected bash in {names:?}");
    assert!(names.contains(&"read"), "expected read in {names:?}");
    assert!(names.contains(&"edit"), "expected edit in {names:?}");
    assert!(names.contains(&"python"), "expected python in {names:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn toggle_persists_disabled_state() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req(
            "PATCH",
            "/api/agents/default/tools/bash/toggle",
            Some(json!({"enabled": false})),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["name"], "bash");
    assert_eq!(body["enabled"], false);

    let on_disk = read_agent_json(&tmp);
    assert_eq!(on_disk["tools"]["builtin_tools"]["bash"]["enabled"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn toggle_without_body_flips_current_state() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req("PATCH", "/api/agents/default/tools/bash/toggle", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["enabled"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_async_execution_persists_change() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req(
            "PATCH",
            "/api/agents/default/tools/bash/async-execution",
            Some(json!({"async_execution": true})),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["async_execution"], true);

    let on_disk = read_agent_json(&tmp);
    assert_eq!(
        on_disk["tools"]["builtin_tools"]["bash"]["async_execution"],
        true
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn toggle_unknown_tool_returns_404() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req(
            "PATCH",
            "/api/agents/default/tools/totally_made_up/toggle",
            Some(json!({"enabled": false})),
        ))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_list_contains_canonical_builtin_tools() {
    let tmp = TempDir::new().expect("tempdir");
    seed_default_agent(
        &tmp,
        agent_skeleton(json!(rua_config::types::tools::default_builtin_tools())),
    );
    let app = build_app(&tmp);

    let resp = app
        .oneshot(req("GET", "/api/agents/default/tools", None))
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"bash"));
    assert!(names.contains(&"read"));
}
