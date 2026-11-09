use axum::http::{Method, StatusCode};
use serde_json::json;
use serial_test::serial;

use super::support::{build_app, login, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn register_after_first_user_returns_forbidden_fastapi_error_shape() {
    let ctx = build_app();
    login(&ctx.app, "alice", "correct-password").await;

    let resp = request(
        &ctx.app,
        Method::POST,
        "/api/auth/register",
        Some(json!({
            "username": "bob",
            "password": "other-password",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let body = read_json(resp).await;
    assert_eq!(body, json!({ "detail": "User already registered" }));
}
