use axum::http::{Method, StatusCode};
use serde_json::json;
use serial_test::serial;

use super::support::{authed_get, build_app, login, read_json, request_with_token};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn update_profile_changes_username_and_password_and_returns_fresh_token() {
    let ctx = build_app();
    let login_body = login(&ctx.app, "alice", "old-password").await;
    let old_token = login_body["token"].as_str().expect("token").to_string();

    let resp = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/update-profile",
        Some(json!({
            "current_password": "old-password",
            "new_username": "alice-renamed",
            "new_password": "new-password",
        })),
        Some(&old_token),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["username"], "alice-renamed");
    let new_token = body["token"].as_str().expect("new token");
    assert_ne!(new_token, old_token);

    let old_token_verify = authed_get(&ctx.app, "/api/auth/verify", &old_token).await;
    assert_eq!(old_token_verify.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        read_json(old_token_verify).await,
        json!({ "detail": "Invalid or expired token" })
    );

    let new_token_verify = authed_get(&ctx.app, "/api/auth/verify", new_token).await;
    assert_eq!(new_token_verify.status(), StatusCode::OK);
    let verified = read_json(new_token_verify).await;
    assert_eq!(verified["valid"], true);
    assert_eq!(verified["username"], "alice-renamed");

    let old_login = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/login",
        Some(json!({
            "username": "alice",
            "password": "old-password",
        })),
        None,
    )
    .await;
    assert_eq!(old_login.status(), StatusCode::UNAUTHORIZED);

    let new_login = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/login",
        Some(json!({
            "username": "alice-renamed",
            "password": "new-password",
        })),
        None,
    )
    .await;
    assert_eq!(new_login.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn password_only_update_preserves_current_username_in_response() {
    let ctx = build_app();
    let login_body = login(&ctx.app, "alice", "old-password").await;
    let old_token = login_body["token"].as_str().expect("token").to_string();

    let resp = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/update-profile",
        Some(json!({
            "current_password": "old-password",
            "new_password": "new-password",
        })),
        Some(&old_token),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["username"], "alice");
    assert!(body["token"]
        .as_str()
        .is_some_and(|token| !token.is_empty()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn update_profile_validation_errors_use_fastapi_detail_shape() {
    let ctx = build_app();
    let login_body = login(&ctx.app, "alice", "correct-password").await;
    let token = login_body["token"].as_str().expect("token").to_string();

    let unauthenticated = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/update-profile",
        Some(json!({
            "current_password": "correct-password",
            "new_username": "alice2",
        })),
        None,
    )
    .await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        read_json(unauthenticated).await,
        json!({ "detail": "Authentication required" })
    );

    let nothing = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/update-profile",
        Some(json!({
            "current_password": "correct-password",
        })),
        Some(&token),
    )
    .await;
    assert_eq!(nothing.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(nothing).await,
        json!({ "detail": "Nothing to update" })
    );

    let empty_username = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/update-profile",
        Some(json!({
            "current_password": "correct-password",
            "new_username": "   ",
        })),
        Some(&token),
    )
    .await;
    assert_eq!(empty_username.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(empty_username).await,
        json!({ "detail": "Username cannot be empty" })
    );

    let empty_password = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/update-profile",
        Some(json!({
            "current_password": "correct-password",
            "new_password": "   ",
        })),
        Some(&token),
    )
    .await;
    assert_eq!(empty_password.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        read_json(empty_password).await,
        json!({ "detail": "Password cannot be empty" })
    );

    let wrong_password = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/update-profile",
        Some(json!({
            "current_password": "wrong-password",
            "new_username": "alice2",
        })),
        Some(&token),
    )
    .await;
    assert_eq!(wrong_password.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        read_json(wrong_password).await,
        json!({ "detail": "Current password is incorrect" })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn update_profile_rejects_non_object_json_body() {
    let ctx = build_app();
    let login_body = login(&ctx.app, "alice", "correct-password").await;
    let token = login_body["token"].as_str().expect("token").to_string();

    let resp = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/update-profile",
        Some(json!("not-an-object")),
        Some(&token),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = read_json(resp).await;
    assert!(
        body.get("detail").is_some(),
        "expected validation detail body, got {body:?}"
    );
}
