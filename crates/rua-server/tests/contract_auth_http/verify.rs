use axum::http::{Method, StatusCode};
use serde_json::json;
use serial_test::serial;

use super::support::{authed_get, build_app, login, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn verify_without_token_returns_fastapi_unauthorized() {
    let ctx = build_app();
    login(&ctx.app, "alice", "correct-password").await;

    let resp = request(&ctx.app, Method::GET, "/api/auth/verify", None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let body = read_json(resp).await;
    assert_eq!(body, json!({ "detail": "Authentication required" }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn verify_with_valid_token_returns_username() {
    let ctx = build_app();
    let login_body = login(&ctx.app, "alice", "correct-password").await;
    let token = login_body["token"].as_str().expect("token");

    let resp = authed_get(&ctx.app, "/api/auth/verify", token).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert_eq!(body["valid"], true);
    assert_eq!(body["username"], "alice");
}
