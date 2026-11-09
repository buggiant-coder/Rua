use axum::http::{Method, StatusCode};
use serde_json::json;
use serial_test::serial;

use super::support::{build_app, login, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn existing_user_login_succeeds_with_correct_password() {
    let ctx = build_app();
    let first = login(&ctx.app, "alice", "correct-password").await;
    let first_token = first["token"].as_str().expect("first token");

    let second = login(&ctx.app, "alice", "correct-password").await;
    assert_eq!(second["username"], "alice");
    let second_token = second["token"].as_str().expect("second token");
    assert!(!second_token.is_empty());
    assert_ne!(second_token, first_token);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn existing_user_login_rejects_wrong_password_with_fastapi_error_shape() {
    let ctx = build_app();
    login(&ctx.app, "alice", "correct-password").await;

    let resp = request(
        &ctx.app,
        Method::POST,
        "/api/auth/login",
        Some(json!({
            "username": "alice",
            "password": "wrong-password",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let body = read_json(resp).await;
    assert_eq!(body, json!({ "detail": "Invalid credentials" }));
}
