use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::security;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed default workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", security::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);
    (app, tmp)
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
async fn whitelist_add_then_remove_roundtrip() {
    let (app, _tmp) = build_app();

    let add_resp = app
        .clone()
        .oneshot(req(
            Method::POST,
            "/api/config/security/skill-scanner/whitelist",
            Some(json!({
                "skill_name": "trusted-tool",
                "content_hash": "deadbeef"
            })),
        ))
        .await
        .expect("POST add");
    assert_eq!(add_resp.status(), StatusCode::CREATED);

    let remove_resp = app
        .clone()
        .oneshot(req(
            Method::DELETE,
            "/api/config/security/skill-scanner/whitelist/trusted-tool",
            None,
        ))
        .await
        .expect("DELETE first");
    assert_eq!(remove_resp.status(), StatusCode::OK);

    let remove_again = app
        .oneshot(req(
            Method::DELETE,
            "/api/config/security/skill-scanner/whitelist/trusted-tool",
            None,
        ))
        .await
        .expect("DELETE second");
    assert_eq!(
        remove_again.status(),
        StatusCode::NOT_FOUND,
        "removing a missing entry should surface as 404 via StoreError::NotFound"
    );
}
