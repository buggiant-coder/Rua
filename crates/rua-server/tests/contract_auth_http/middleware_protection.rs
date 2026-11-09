use axum::http::{Method, StatusCode};
use serde_json::json;
use serial_test::serial;

use super::support::{
    authed_get, build_app, build_full_no_auth_app, login, read_json, request, request_with_token,
    request_with_token_and_headers,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn auth_middleware_protects_non_auth_api_routes_after_first_user_exists() {
    let ctx = build_app();
    let login_body = login(&ctx.app, "alice", "correct-password").await;
    let token = login_body["token"].as_str().expect("token");

    let public_status = request(&ctx.app, Method::GET, "/api/auth/status", None).await;
    assert_eq!(public_status.status(), StatusCode::OK);

    let unauthenticated = request(&ctx.app, Method::GET, "/api/models/active", None).await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        read_json(unauthenticated).await,
        json!({ "detail": "Authentication required" })
    );

    let invalid = authed_get(&ctx.app, "/api/models/active", "not-a-valid-token").await;
    assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        read_json(invalid).await,
        json!({ "detail": "Invalid or expired token" })
    );

    let authenticated = authed_get(&ctx.app, "/api/models/active", token).await;
    assert_eq!(authenticated.status(), StatusCode::OK);
    assert!(read_json(authenticated).await["default_llm"].is_object());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn auth_middleware_protects_representative_console_api_entrypoints() {
    let ctx = build_app();
    let login_body = login(&ctx.app, "alice", "correct-password").await;
    let token = login_body["token"].as_str().expect("token");

    for path in ["/api/agents", "/api/config/heartbeat"] {
        let unauthenticated = request(&ctx.app, Method::GET, path, None).await;
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(
            read_json(unauthenticated).await,
            json!({ "detail": "Authentication required" }),
            "{path}"
        );

        let invalid = authed_get(&ctx.app, path, "not-a-valid-token").await;
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(
            read_json(invalid).await,
            json!({ "detail": "Invalid or expired token" }),
            "{path}"
        );

        let authenticated = authed_get(&ctx.app, path, token).await;
        assert_eq!(authenticated.status(), StatusCode::OK, "{path}");
    }

    for heartbeat_run in [
        "/api/config/heartbeat/run",
        "/api/agents/default/config/heartbeat/run",
    ] {
        let unauthenticated = request(&ctx.app, Method::POST, heartbeat_run, None).await;
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            read_json(unauthenticated).await,
            json!({ "detail": "Authentication required" })
        );

        let invalid = request_with_token(
            &ctx.app,
            Method::POST,
            heartbeat_run,
            None,
            Some("not-a-valid-token"),
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            read_json(invalid).await,
            json!({ "detail": "Invalid or expired token" })
        );

        let authenticated =
            request_with_token(&ctx.app, Method::POST, heartbeat_run, None, Some(token)).await;
        assert_eq!(authenticated.status(), StatusCode::OK);
        assert_eq!(read_json(authenticated).await, json!({ "started": true }));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn auth_middleware_allows_configured_no_auth_client_hosts() {
    let ctx = build_app();
    let login_body = login(&ctx.app, "alice", "correct-password").await;
    let token = login_body["token"].as_str().expect("token");

    let update = request_with_token(
        &ctx.app,
        Method::PUT,
        "/api/config/security/allow-no-auth-hosts",
        Some(json!({"hosts": ["203.0.113.7"]})),
        Some(token),
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    assert_eq!(read_json(update).await, json!({"hosts": ["203.0.113.7"]}));

    let allowed = request_with_token_and_headers(
        &ctx.app,
        Method::GET,
        "/api/models/active",
        None,
        None,
        &[("x-forwarded-for", "203.0.113.7")],
    )
    .await;
    assert_eq!(allowed.status(), StatusCode::OK);
    assert!(read_json(allowed).await["default_llm"].is_object());

    let denied_after_update = request_with_token_and_headers(
        &ctx.app,
        Method::GET,
        "/api/models/active",
        None,
        None,
        &[("x-forwarded-for", "127.0.0.1")],
    )
    .await;
    assert_eq!(denied_after_update.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        read_json(denied_after_update).await,
        json!({"detail": "Authentication required"})
    );

    let invalid_header_ip = request_with_token_and_headers(
        &ctx.app,
        Method::GET,
        "/api/models/active",
        None,
        None,
        &[("x-forwarded-for", "not-an-ip")],
    )
    .await;
    assert_eq!(invalid_header_ip.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn no_auth_full_router_reports_disabled_status_and_allows_console_api_without_token() {
    let ctx = build_full_no_auth_app();

    let status = request(&ctx.app, Method::GET, "/api/auth/status", None).await;
    assert_eq!(status.status(), StatusCode::OK);
    let status_body = read_json(status).await;
    assert_eq!(status_body["enabled"], false);
    assert_eq!(status_body["has_users"], false);

    for path in ["/api/models/active", "/api/agents", "/api/config/heartbeat"] {
        let resp = request(&ctx.app, Method::GET, path, None).await;
        assert_eq!(resp.status(), StatusCode::OK, "{path}");
        assert!(
            !read_json(resp).await.is_null(),
            "no-auth console API should still return JSON: {path}"
        );
    }

    for path in [
        "/api/config/heartbeat/run",
        "/api/agents/default/config/heartbeat/run",
    ] {
        let heartbeat_run = request(&ctx.app, Method::POST, path, None).await;
        assert_eq!(heartbeat_run.status(), StatusCode::OK, "{path}");
        assert_eq!(read_json(heartbeat_run).await, json!({ "started": true }));
    }
}
