use axum::http::{Method, StatusCode};
use serde_json::json;
use serial_test::serial;

use super::support::{build_app, build_inactive_auth_app, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn first_login_auto_registers_first_user() {
    let ctx = build_app();

    let resp = request(
        &ctx.app,
        Method::POST,
        "/api/auth/login",
        Some(json!({
            "username": "alice",
            "password": "correct-password",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert_eq!(body["username"], "alice");
    assert!(
        body["token"]
            .as_str()
            .is_some_and(|token| !token.is_empty()),
        "first login must return a non-empty token: {body}"
    );

    let status = request(&ctx.app, Method::GET, "/api/auth/status", None).await;
    assert_eq!(status.status(), StatusCode::OK);
    let status_body = read_json(status).await;
    assert_eq!(status_body["enabled"], true);
    assert_eq!(status_body["has_users"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn inactive_auth_login_returns_empty_token_free_access_shape() {
    let ctx = build_inactive_auth_app();

    let resp = request(
        &ctx.app,
        Method::POST,
        "/api/auth/login",
        Some(json!({
            "username": "guest",
            "password": "",
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["username"], "guest");
    assert_eq!(body["token"], "");

    let status = request(&ctx.app, Method::GET, "/api/auth/status", None).await;
    assert_eq!(status.status(), StatusCode::OK);
    let status_body = read_json(status).await;
    assert_eq!(status_body["enabled"], false);
    assert_eq!(status_body["has_users"], false);
}
