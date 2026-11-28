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
async fn login_never_mints_an_empty_token_even_when_auth_is_explicitly_off() {
    // Login used to answer HTTP 200 with an empty token, which the console
    // treated as valid access. That is unrelated to whether authentication is
    // switched on: an empty password must be rejected outright.
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
    assert_ne!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert!(
        body.get("token").is_none(),
        "login must never return a token for an empty password: {body}"
    );
    assert_eq!(body["detail"], "Password is required for first login");

    let status = request(&ctx.app, Method::GET, "/api/auth/status", None).await;
    assert_eq!(status.status(), StatusCode::OK);
    let status_body = read_json(status).await;
    // This fixture mirrors `--no-auth`, the only thing that may disable auth,
    // so the status endpoint reports it honestly rather than claiming a mode
    // the service is not in.
    assert_eq!(status_body["enabled"], false);
    assert_eq!(status_body["has_users"], false);
}
