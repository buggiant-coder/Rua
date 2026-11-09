use axum::http::{Method, StatusCode};
use serial_test::serial;

use super::support::{build_app, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn status_returns_enabled_and_has_users_flags() {
    let ctx = build_app();

    let resp = request(&ctx.app, Method::GET, "/api/auth/status", None).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert_eq!(body["enabled"], true);
    assert_eq!(body["has_users"], false);
    assert!(body
        .as_object()
        .expect("auth status body is an object")
        .contains_key("enabled"));
    assert!(body
        .as_object()
        .expect("auth status body is an object")
        .contains_key("has_users"));
}
