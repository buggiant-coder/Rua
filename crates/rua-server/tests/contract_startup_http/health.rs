use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::{router, AppState, ServerProfile};

fn build_app() -> (axum::Router, AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let app = router::build_app(state.clone(), None);
    (app, state, tmp)
}

async fn request(app: &axum::Router, method: Method, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

fn assert_workspace_health_shape(body: &Value, agent_id: &str) {
    let workspace = body["workspaces"][agent_id]
        .as_object()
        .unwrap_or_else(|| panic!("workspace {agent_id} missing from {body:?}"));
    assert_eq!(workspace["agent_id"], agent_id);
    assert!(workspace["lifecycle"]
        .as_str()
        .is_some_and(|v| !v.is_empty()));
    assert!(workspace["active_sessions"].as_u64().is_some());
    assert!(workspace["background_tasks"].as_u64().is_some());

    let services = workspace["services"]
        .as_object()
        .unwrap_or_else(|| panic!("services missing for {agent_id}: {workspace:?}"));
    // `services` forwards the WorkspaceRuntime component table verbatim; only
    // resources with a real lifecycle are registered, so these are the
    // capabilities that must be accounted for.
    for service in ["agent", "mcp", "memory"] {
        let state = services
            .get(service)
            .and_then(|value| value.get("state"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("service {service}.state missing for {agent_id}"));
        assert!(
            !state.is_empty(),
            "service {service}.state must be non-empty for {agent_id}"
        );
    }

    // Capabilities without a real initialization, stop, or cleanup behavior
    // must not appear in the accounting table at all: a presence-only entry
    // would be a placeholder lifecycle.
    for absent in ["provider", "tools", "cron", "session_store"] {
        assert!(
            !services.contains_key(absent),
            "service {absent} has no real lifecycle and must not be reported for {agent_id}: {services:?}"
        );
    }

    assert!(workspace["overall"]["state"]
        .as_str()
        .is_some_and(|state| !state.is_empty()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_health_reports_registered_workspace_health_shape() {
    let (app, state, _tmp) = build_app();
    state
        .registry
        .get_or_create("default")
        .await
        .expect("create default workspace");
    state
        .registry
        .get_or_create("health_agent_two")
        .await
        .expect("create second workspace");

    let resp = request(&app, Method::GET, "/api/health").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["overall"], "healthy");

    let workspaces = body["workspaces"].as_object().expect("workspaces object");
    assert!(
        workspaces.contains_key("default"),
        "default must be reported: {workspaces:?}"
    );
    assert!(
        workspaces.contains_key("health_agent_two"),
        "second workspace must be reported: {workspaces:?}"
    );
    assert_workspace_health_shape(&body, "default");
    assert_workspace_health_shape(&body, "health_agent_two");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_health_is_accessible_without_agent_header_or_agent_context() {
    let (app, state, _tmp) = build_app();
    state
        .registry
        .get_or_create("default")
        .await
        .expect("create default workspace");

    let resp = request(&app, Method::GET, "/api/health").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let workspaces = body["workspaces"].as_object().expect("workspaces object");
    assert_eq!(
        workspaces.len(),
        1,
        "health must not create an implicit header-selected agent: {workspaces:?}"
    );
    assert_workspace_health_shape(&body, "default");
}
