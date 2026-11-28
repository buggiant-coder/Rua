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
async fn auth_middleware_ignores_caller_supplied_forwarding_headers_for_allowlist() {
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

    // The allowlist is keyed on the transport-level peer address only. A caller
    // that forges the very address it just allowlisted must still be rejected,
    // otherwise any local process could opt out of authentication. These
    // requests carry no `ConnectInfo`, so no peer address can be resolved.
    for spoofed in [
        Some("203.0.113.7"),
        Some("127.0.0.1"),
        Some("not-an-ip"),
        None,
    ] {
        let headers: &[(&str, &str)] = match spoofed {
            Some(value) => &[("x-forwarded-for", value)],
            None => &[],
        };
        let denied = request_with_token_and_headers(
            &ctx.app,
            Method::GET,
            "/api/models/active",
            None,
            None,
            headers,
        )
        .await;
        assert_eq!(
            denied.status(),
            StatusCode::UNAUTHORIZED,
            "spoofed={spoofed:?}"
        );
        assert_eq!(
            read_json(denied).await,
            json!({"detail": "Authentication required"}),
            "spoofed={spoofed:?}"
        );
    }

    // The real token still works, so the allowlist change did not break auth.
    let authenticated = authed_get(&ctx.app, "/api/models/active", token).await;
    assert_eq!(authenticated.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn console_api_requires_a_credential_unless_the_operator_opts_out() {
    // An unset `RUA_AUTH_ENABLED` used to leave the whole console API open,
    // because the variable itself was the switch. It no longer is: only the
    // explicit `--no-auth` flag turns authentication off, and these fixtures
    // mirror that flag rather than an unset environment.
    let ctx = build_full_no_auth_app();

    let status = request(&ctx.app, Method::GET, "/api/auth/status", None).await;
    assert_eq!(status.status(), StatusCode::OK);
    let status_body = read_json(status).await;
    assert_eq!(status_body["enabled"], false);
    assert_eq!(status_body["has_users"], false);

    // `--no-auth` means what it says, so the developer path stays usable.
    let open = request(&ctx.app, Method::GET, "/api/models/active", None).await;
    assert_ne!(open.status(), StatusCode::UNAUTHORIZED);
}
