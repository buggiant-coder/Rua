use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use cell_store::FileSystemCellStore;
use http_body_util::BodyExt;
use rua_agent::AgentCells;
use rua_workspace::WorkspaceRuntime;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::config_channels;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed default workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    (state, tmp)
}

fn router_with_agent_ctx(state: AppState, sub: Router<AppState>) -> Router {
    Router::new()
        .nest("/api", sub)
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
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "non-JSON body: {} (body={})",
            e,
            String::from_utf8_lossy(&bytes)
        )
    })
}

fn req(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    builder.body(body).expect("request")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_create_endpoint_persists_then_external_read_sees() {
    let (state, tmp) = build_state();
    let app = router_with_agent_ctx(state, config_channels::root_router());

    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/config/channels/dingtalk",
            Some(json!({"client_id": "test_id_123", "client_secret": "sec456"})),
        ))
        .await
        .expect("put dingtalk");
    assert_eq!(resp.status(), StatusCode::OK);

    let agent_json_path = tmp.path().join("workspaces/default/agent.json");
    let raw = std::fs::read_to_string(&agent_json_path).expect("read agent.json");
    let doc: Value = serde_json::from_str(&raw).expect("parse agent.json");
    assert_eq!(doc["channels"]["dingtalk"]["client_id"], "test_id_123");

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/config/channels/dingtalk", None))
        .await
        .expect("get dingtalk");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["client_id"], "test_id_123");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_update_triggers_reload_path() {
    let (state, _tmp) = build_state();
    let app = router_with_agent_ctx(state.clone(), config_channels::root_router());

    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/config/channels/telegram",
            Some(json!({"bot_token": "tok1", "chat_id": "c1"})),
        ))
        .await
        .expect("put telegram");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/config/channels/telegram",
            Some(json!({"bot_token": "tok2", "chat_id": "c2"})),
        ))
        .await
        .expect("put telegram updated");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(Method::GET, "/api/config/channels/telegram", None))
        .await
        .expect("get telegram");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["bot_token"], "***");
    assert_eq!(body["chat_id"], "c2");

    let cell = state.agent_cells.section::<Value>("default", "channels");
    let raw = cell
        .read()
        .await
        .expect("read raw channels")
        .expect("channels value");
    assert_eq!(raw["telegram"]["bot_token"], "tok2");
    assert_eq!(raw["telegram"]["chat_id"], "c2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_simulated_restart_keeps_config() {
    let (state, tmp) = build_state();
    let app = router_with_agent_ctx(state, config_channels::root_router());

    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/config/channels/feishu",
            Some(json!({"webhook_url": "https://open.feishu.cn/hook/abc"})),
        ))
        .await
        .expect("put feishu");
    assert_eq!(resp.status(), StatusCode::OK);

    let fresh_cells = AgentCells::new(
        Arc::new(FileSystemCellStore::new()),
        tmp.path().join("workspaces"),
    );
    let cell = fresh_cells.section::<Value>("default", "channels");
    let read = cell.read().await.unwrap().unwrap();
    let map = read.as_object().expect("channels map");
    assert!(map.contains_key("feishu"));
    assert_eq!(
        map["feishu"]["webhook_url"],
        "https://open.feishu.cn/hook/abc"
    );
}

async fn wait_for_agent(state: &AppState, agent_id: &str, attempts: u32) {
    for _ in 0..attempts {
        if state.registry.get(agent_id).await.is_some() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("agent {agent_id} never appeared in registry");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channels_write_preserves_workspace_registry_instance() {
    let (state, _tmp) = build_state();
    let app = router_with_agent_ctx(state.clone(), config_channels::root_router());

    wait_for_agent(&state, "default", 40).await;

    let before: Arc<WorkspaceRuntime> = state
        .registry
        .get("default")
        .await
        .expect("default runtime must exist after preheat");

    let resp = app
        .clone()
        .oneshot(req(
            Method::PUT,
            "/api/config/channels/slack",
            Some(json!({"webhook_url": "https://hooks.slack.com/xxx"})),
        ))
        .await
        .expect("put slack");
    assert_eq!(resp.status(), StatusCode::OK);

    let after: Arc<WorkspaceRuntime> = state
        .registry
        .get("default")
        .await
        .expect("default runtime must still exist after reload");

    assert!(
        Arc::ptr_eq(&before, &after),
        "Channel configuration application must preserve the Workspace Runtime"
    );
}
