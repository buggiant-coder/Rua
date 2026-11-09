use std::path::PathBuf;

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde::Serialize;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::extractors::AgentExtractorContext;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

#[derive(Serialize)]
struct EchoResponse {
    agent_id: String,
    workspace_dir: String,
}

async fn echo(ctx: AgentExtractorContext) -> impl IntoResponse {
    Json(EchoResponse {
        agent_id: ctx.agent_id.clone(),
        workspace_dir: ctx.runtime.workspace_dir().display().to_string(),
    })
}

#[allow(dead_code)]
fn _no_agent_registry_field(_state: &AppState) {}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_tempdir_no_longer_carries_agent_registry() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    state.wait_platform_initialization().await.unwrap();

    let summaries = state.registry.list().await.expect("registry.list ok");
    assert_eq!(
        summaries.len(),
        1,
        "preheated workspace registry should expose exactly the default agent"
    );
    assert_eq!(summaries[0].agent_id, "default");

    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("get_or_create default ok");
    assert_eq!(runtime.agent_id, "default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn middleware_resolves_default_without_agent_registry_fallback() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);

    let app = Router::new()
        .route("/api/echo", get(echo))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);

    let req = Request::builder()
        .uri("/api/echo")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["agent_id"], "default");

    let ws_dir: String = json["workspace_dir"].as_str().unwrap().to_string();
    assert!(
        PathBuf::from(&ws_dir)
            .components()
            .any(|c| c.as_os_str() == "default"),
        "workspace_dir should include the default agent segment, got {ws_dir}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_state_can_reload_workspace() {
    use rua_contract::config::AppConfig;
    use rua_server::profiles::ServerProfile;

    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&secret_dir).expect("mkdir secret");

    std::fs::write(
        data_dir.join("config.json"),
        serde_json::json!({
            "agents": { "active_agent": "default" }
        })
        .to_string(),
    )
    .expect("write config.json");

    let workspace = data_dir.join("workspaces").join("default");
    std::fs::create_dir_all(&workspace).expect("mkdir workspaces/default");
    std::fs::write(
        workspace.join("agent.json"),
        serde_json::json!({
            "id": "default",
            "name": "Default Agent"
        })
        .to_string(),
    )
    .expect("write agent.json");

    let state = AppState::load(
        AppConfig::default(),
        data_dir,
        secret_dir,
        ServerProfile::Test,
    )
    .expect("valid test configuration");

    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("get_or_create default ok");
    state
        .registry
        .reload_agent("default")
        .await
        .expect("explicit workspace rebuild");
    assert!(!std::sync::Arc::ptr_eq(
        &runtime,
        &state.registry.get("default").await.unwrap()
    ));
}
