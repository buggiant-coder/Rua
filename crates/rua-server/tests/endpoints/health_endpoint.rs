use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::health::router as health_router;
use rua_server::AppState;

fn build_app() -> (Router, TempDir, AppState) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default"))
        .expect("seed default workspace dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", health_router())
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
    serde_json::from_slice(&bytes).expect("response is JSON")
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
async fn health_reports_every_registered_workspace() {
    let (app, _tmp, state) = build_app();
    wait_for_agent(&state, "default", 40).await;
    state
        .registry
        .get_or_create("agent2")
        .await
        .expect("create agent2");

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    let workspaces = body["workspaces"]
        .as_object()
        .expect("workspaces is an object");
    assert!(
        workspaces.contains_key("default"),
        "default must appear: {workspaces:?}"
    );
    assert!(
        workspaces.contains_key("agent2"),
        "agent2 must appear: {workspaces:?}"
    );

    for (id, ws) in workspaces {
        assert_eq!(ws["agent_id"].as_str(), Some(id.as_str()));
        let services = ws["services"]
            .as_object()
            .unwrap_or_else(|| panic!("services missing for {id}"));

        // `services` is the WorkspaceRuntime component table forwarded verbatim,
        // so only capabilities with a real lifecycle appear here.
        for required in ["agent", "mcp", "memory"] {
            assert!(
                services.contains_key(required),
                "service {required} missing for {id}"
            );
        }

        // No placeholder lifecycles: the shared provider, the toolkit that the
        // React runtime constructs, and the process-owned cron scheduler have no
        // per-Agent start/stop, so reporting them would be fabricated Ready.
        for absent in ["provider", "tools", "cron", "session_store"] {
            assert!(
                !services.contains_key(absent),
                "service {absent} has no per-Agent lifecycle but was reported for {id}"
            );
        }

        let agent_state = services["agent"]["state"]
            .as_str()
            .unwrap_or_else(|| panic!("agent.state missing for {id}"));
        assert!(
            matches!(
                agent_state,
                "ready" | "not_initialized" | "initializing" | "disabled"
            ),
            "unexpected agent state for {id}: {agent_state}"
        );

        let overall_state = ws["overall"]["state"]
            .as_str()
            .unwrap_or_else(|| panic!("overall.state missing for {id}"));
        assert!(
            !overall_state.is_empty(),
            "overall.state must not be empty for {id}"
        );
    }

    assert_eq!(
        body["overall"].as_str(),
        Some("healthy"),
        "no-failure cluster should be healthy: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_request_without_agent_header_succeeds() {
    let (app, _tmp, state) = build_app();
    wait_for_agent(&state, "default", 40).await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let workspaces = body["workspaces"].as_object().expect("workspaces object");
    assert_eq!(
        workspaces.len(),
        1,
        "only default should be present, got {workspaces:?}"
    );
}
