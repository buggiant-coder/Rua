use axum::http::{Method, StatusCode};
use serde_json::json;
use serial_test::serial;

use super::support::{authed_get, build_app, login, read_json, request_with_token};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn revoke_token_without_body_revokes_current_token() {
    let ctx = build_app();
    let login_body = login(&ctx.app, "alice", "correct-password").await;
    let token = login_body["token"].as_str().expect("token").to_string();

    let revoke = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/revoke-token",
        Some(json!({})),
        Some(&token),
    )
    .await;
    assert_eq!(revoke.status(), StatusCode::OK);
    let body = read_json(revoke).await;
    assert_eq!(body["revoked"], true);
    assert_eq!(body["revoked_current_token"], true);
    assert!(body["message"]
        .as_str()
        .is_some_and(|message| message.contains("Current token")));

    let verify = authed_get(&ctx.app, "/api/auth/verify", &token).await;
    assert_eq!(verify.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        read_json(verify).await,
        json!({ "detail": "Invalid or expired token" })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn revoke_token_can_revoke_specific_token_without_revoking_caller() {
    let ctx = build_app();
    let first = login(&ctx.app, "alice", "correct-password").await;
    let caller = first["token"].as_str().expect("caller token").to_string();
    let second = login(&ctx.app, "alice", "correct-password").await;
    let target = second["token"].as_str().expect("target token").to_string();

    let revoke = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/revoke-token",
        Some(json!({ "token": target })),
        Some(&caller),
    )
    .await;
    assert_eq!(revoke.status(), StatusCode::OK);
    let body = read_json(revoke).await;
    assert_eq!(body["revoked"], true);
    assert_eq!(body["revoked_current_token"], false);

    let caller_verify = authed_get(&ctx.app, "/api/auth/verify", &caller).await;
    assert_eq!(caller_verify.status(), StatusCode::OK);
    assert_eq!(read_json(caller_verify).await["username"], "alice");

    let target_verify = authed_get(&ctx.app, "/api/auth/verify", &target).await;
    assert_eq!(target_verify.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        read_json(target_verify).await,
        json!({ "detail": "Invalid or expired token" })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn revoke_all_tokens_invalidates_every_existing_token() {
    let ctx = build_app();
    let first = login(&ctx.app, "alice", "correct-password").await;
    let first_token = first["token"].as_str().expect("first token").to_string();
    let second = login(&ctx.app, "alice", "correct-password").await;
    let second_token = second["token"].as_str().expect("second token").to_string();

    let revoke = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/revoke-all-tokens",
        None,
        Some(&first_token),
    )
    .await;
    assert_eq!(revoke.status(), StatusCode::OK);
    let body = read_json(revoke).await;
    assert_eq!(body["revoked"], true);
    assert!(body["message"]
        .as_str()
        .is_some_and(|message| message.contains("All tokens")));

    for token in [first_token, second_token] {
        let verify = authed_get(&ctx.app, "/api/auth/verify", &token).await;
        assert_eq!(verify.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            read_json(verify).await,
            json!({ "detail": "Invalid or expired token" })
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn revoke_routes_require_authentication() {
    let ctx = build_app();
    login(&ctx.app, "alice", "correct-password").await;

    let revoke_one = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/revoke-token",
        Some(json!({})),
        None,
    )
    .await;
    assert_eq!(revoke_one.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        read_json(revoke_one).await,
        json!({ "detail": "Authentication required" })
    );

    let revoke_all = request_with_token(
        &ctx.app,
        Method::POST,
        "/api/auth/revoke-all-tokens",
        None,
        None,
    )
    .await;
    assert_eq!(revoke_all.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        read_json(revoke_all).await,
        json!({ "detail": "Authentication required" })
    );
}
