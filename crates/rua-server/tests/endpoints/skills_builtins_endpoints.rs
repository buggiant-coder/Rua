use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::skills::{agent_scoped_router, shared_router};
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default/skills"))
        .expect("seed default workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    let app = Router::new()
        .nest("/api", shared_router())
        .nest("/api", agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state);
    (app, tmp)
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

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("X-Agent-Id", "default")
        .body(Body::empty())
        .unwrap()
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("X-Agent-Id", "default")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_builtin_sources_returns_array() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(get("/api/skills/pool/builtin-sources"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let arr = body.as_array().expect("response must be a JSON array");
    assert_eq!(arr.len(), 17, "should list 17 builtin candidates");
    assert!(
        arr.iter()
            .any(|item| item["name"].as_str() == Some("toolchain-setup")),
        "Desktop External Toolchain setup must be exposed as a packaged builtin Skill"
    );
    for item in arr {
        assert!(item.get("name").and_then(|v| v.as_str()).is_some());
        assert!(item.get("status").and_then(|v| v.as_str()).is_some());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_builtin_sources_all_current_after_init() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(get("/api/skills/pool/builtin-sources"))
        .await
        .unwrap();
    let body = read_json(resp).await;
    for item in body.as_array().unwrap() {
        assert_eq!(
            item["status"], "current",
            "pool auto-init imports all builtins"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_builtins_after_init_returns_unchanged() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(post_json(
            "/api/skills/pool/import-builtin",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let unchanged = body["unchanged"].as_array().expect("unchanged array");
    assert_eq!(
        unchanged.len(),
        17,
        "all 17 builtins should be unchanged after auto-init"
    );
    assert!(
        unchanged
            .iter()
            .any(|name| name.as_str() == Some("toolchain-setup")),
        "toolchain-setup should participate in the normal builtin import lifecycle"
    );
    assert!(body["imported"].as_array().unwrap().is_empty());
    assert!(body["conflicts"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_builtin_after_import() {
    let (app, _tmp) = build_app();

    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/skills/pool/import-builtin",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(post_json(
            "/api/skills/pool/cron/update-builtin",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["name"], "cron");
    assert_eq!(body["status"], "current");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_nonexistent_builtin_returns_400() {
    let (app, _tmp) = build_app();
    let resp = app
        .oneshot(post_json(
            "/api/skills/pool/no-such-builtin-xyz/update-builtin",
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert!(
        resp.status().is_client_error(),
        "non-existent builtin should return 4xx, got {}",
        resp.status()
    );
}
