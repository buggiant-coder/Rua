use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::workspace_admin::register_workspace_admin_routes;
use rua_server::AppState;

fn build_app() -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");

    std::fs::create_dir_all(tmp.path().join("workspaces/default"))
        .expect("seed default workspace dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", register_workspace_admin_routes())
        .with_state(state.clone());
    (app, tmp, state)
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
    serde_json::from_slice(&bytes).expect("response is JSON")
}

fn post(uri: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
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
async fn reload_default_agent_returns_ok_and_keeps_entry() {
    let (app, _tmp, state) = build_app();
    wait_for_agent(&state, "default", 40).await;

    let resp = app
        .clone()
        .oneshot(post("/api/workspace/default/reload"))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK, "reload should succeed");

    let body = read_json(resp).await;
    assert_eq!(body["status"].as_str(), Some("reloaded"));
    assert_eq!(body["agent_id"].as_str(), Some("default"));

    let after = state.registry.get("default").await;
    assert!(
        after.is_some(),
        "reload_agent must keep the agent in the registry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_unknown_agent_returns_404() {
    let (app, _tmp, _state) = build_app();
    let resp = app
        .clone()
        .oneshot(post("/api/workspace/does-not-exist/reload"))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let body = read_json(resp).await;
    assert_eq!(body["agent_id"].as_str(), Some("does-not-exist"));
    assert_eq!(body["kind"].as_str(), Some("agent_not_found"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_removes_agent_from_registry() {
    let (app, _tmp, state) = build_app();
    wait_for_agent(&state, "default", 40).await;

    assert!(state.registry.get("default").await.is_some());

    let resp = app
        .clone()
        .oneshot(post("/api/workspace/default/shutdown"))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK, "shutdown should succeed");

    let body = read_json(resp).await;
    assert_eq!(body["status"].as_str(), Some("stopped"));
    assert_eq!(body["agent_id"].as_str(), Some("default"));

    let listed = state
        .registry
        .list()
        .await
        .expect("registry.list ok")
        .into_iter()
        .any(|s| s.agent_id == "default");
    assert!(
        !listed,
        "shutdown must remove the agent from registry.list()"
    );
    assert!(
        state.registry.get("default").await.is_none(),
        "registry.get must return None after shutdown"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_unknown_agent_returns_404() {
    let (app, _tmp, _state) = build_app();
    let resp = app
        .clone()
        .oneshot(post("/api/workspace/never-loaded/shutdown"))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let body = read_json(resp).await;
    assert_eq!(body["agent_id"].as_str(), Some("never-loaded"));
    assert_eq!(body["kind"].as_str(), Some("agent_not_found"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_drains_with_no_in_flight_work_immediately() {
    let (app, _tmp, state) = build_app();
    wait_for_agent(&state, "default", 40).await;
    assert_eq!(
        state
            .registry
            .get("default")
            .await
            .unwrap()
            .active_session_count(),
        0
    );

    let start = std::time::Instant::now();
    let resp = app
        .clone()
        .oneshot(post("/api/workspace/default/shutdown"))
        .await
        .expect("oneshot");
    let elapsed = start.elapsed();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "shutdown with no in-flight work must return promptly, took {:?}",
        elapsed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload_handles_second_agent_independently() {
    let (app, _tmp, state) = build_app();
    wait_for_agent(&state, "default", 40).await;

    let agent2: Arc<rua_workspace::WorkspaceRuntime> = state
        .registry
        .get_or_create("agent2")
        .await
        .expect("agent2 created");
    assert_eq!(agent2.agent_id, "agent2");

    let resp = app
        .clone()
        .oneshot(post("/api/workspace/agent2/reload"))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let summaries = state.registry.list().await.expect("list");
    let ids: Vec<_> = summaries.iter().map(|s| s.agent_id.as_str()).collect();
    assert!(
        ids.contains(&"default"),
        "default must remain after reload of agent2: {ids:?}"
    );
    assert!(
        ids.contains(&"agent2"),
        "agent2 must remain after its own reload: {ids:?}"
    );
}
