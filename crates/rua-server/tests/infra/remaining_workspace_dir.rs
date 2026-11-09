use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints;
use rua_server::AppState;

fn build_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    (state, tmp)
}

fn build_app(state: AppState) -> Router {
    Router::new()
        .merge(endpoints::memory::agent_scoped_router())
        .with_state(state)
}

async fn read_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("body is json")
    }
}

fn post(uri: &str, agent_id: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(id) = agent_id {
        builder = builder.header("X-Agent-Id", id);
    }
    builder.body(Body::empty()).expect("request")
}

fn get(uri: &str, agent_id: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(id) = agent_id {
        builder = builder.header("X-Agent-Id", id);
    }
    builder.body(Body::empty()).expect("request")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_agent_path_resolves_via_registry() {
    let (state, guard) = build_state();
    let app = build_app(state);

    let resp = app
        .clone()
        .oneshot(post("/agents/agent-B/memory/repair", None))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let expected_ws = guard.path().join("workspaces").join("agent-B");
    assert!(
        expected_ws.join("memory.db").is_file(),
        "agent-scoped repair should write memory.db under {:?}",
        expected_ws
    );
    assert!(
        expected_ws.join("memory_vec.db").is_file(),
        "agent-scoped repair should write memory_vec.db under {:?}",
        expected_ws
    );
    assert!(!expected_ws.join("MEMORY.md").exists());
    assert!(!expected_ws.join("MEMORY").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_agent_workspaces_are_isolated() {
    let (state, guard) = build_state();
    let app = build_app(state);

    let resp_a = app
        .clone()
        .oneshot(post("/agents/agent-A/memory/repair", None))
        .await
        .expect("oneshot");
    assert_eq!(resp_a.status(), StatusCode::OK);

    let resp_b = app
        .clone()
        .oneshot(post("/agents/agent-B/memory/repair", None))
        .await
        .expect("oneshot");
    assert_eq!(resp_b.status(), StatusCode::OK);

    let ws_a = guard.path().join("workspaces").join("agent-A");
    let ws_b = guard.path().join("workspaces").join("agent-B");

    assert!(
        ws_a.join("memory.db").is_file(),
        "agent-A workspace must own its memory.db at {:?}",
        ws_a
    );
    assert!(
        ws_b.join("memory.db").is_file(),
        "agent-B workspace must own its memory.db at {:?}",
        ws_b
    );
    assert!(!ws_a.join("MEMORY.md").exists());
    assert!(!ws_b.join("MEMORY.md").exists());
    assert_ne!(
        ws_a, ws_b,
        "per-agent workspace paths must differ (path-id is the source of truth)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_route_ignores_header_id() {
    let (state, guard) = build_state();
    let app = build_app(state);

    let resp = app
        .clone()
        .oneshot(post(
            "/agents/agent-B/memory/repair",
            Some("agent-A-header-decoy"),
        ))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let ws_b = guard.path().join("workspaces").join("agent-B");
    let ws_decoy = guard.path().join("workspaces").join("agent-A-header-decoy");
    assert!(
        ws_b.join("memory.db").is_file(),
        "path-id agent-B should win, memory.db expected at {:?}",
        ws_b
    );
    assert!(
        !ws_decoy.join("memory.db").exists(),
        "header decoy id must NOT have been honoured for agent-scoped route; \
         memory.db should not appear under {:?}",
        ws_decoy
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnose_agent_path_resolves_without_memory_markdown() {
    let (state, guard) = build_state();
    let app = build_app(state);

    let resp_b = app
        .clone()
        .oneshot(get("/agents/agent-B/memory/diagnose", None))
        .await
        .expect("oneshot");
    assert_eq!(resp_b.status(), StatusCode::OK);
    let plan_b: serde_json::Value = read_json(resp_b).await;

    let issues_b = plan_b["issues"].as_array().expect("plan_b.issues");
    assert!(
        issues_b.is_empty(),
        "runtime initialization should make structured Memory diagnose healthy, got {plan_b}"
    );

    let ws_b = guard.path().join("workspaces").join("agent-B");
    assert!(ws_b.join("memory.db").is_file());
    assert!(ws_b.join("memory_vec.db").is_file());
    assert!(!ws_b.join("MEMORY.md").exists());
    assert!(!ws_b.join("MEMORY").exists());
}
