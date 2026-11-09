use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn mobile_never_uses_disabled_auth_or_public_registration_as_control_bypass() {
    let temp = tempfile::tempdir().unwrap();
    let mut state =
        rua_server::AppState::from_tempdir(temp.path().into(), rua_server::ServerProfile::Test);
    state.profile = rua_server::ServerProfile::MobileApk;
    state.auth = Arc::new(rua_server::auth::AuthService::new_disabled());
    let app = rua_server::router::build_app(state, None);
    for (method, path, body) in [
        ("GET", "/api/platform/phone", ""),
        (
            "POST",
            "/api/auth/register",
            r#"{"username":"intruder","password":"secret"}"#,
        ),
        (
            "POST",
            "/api/auth/login",
            r#"{"username":"intruder","password":"secret"}"#,
        ),
        ("GET", "/api/agents", ""),
        ("GET", "/ws", ""),
        ("POST", "/voice/asr/transcribe", "{}"),
        ("POST", "/api/platform/lifecycle", "{}"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("x-forwarded-for", "127.0.0.1")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
    }
}

#[tokio::test]
async fn mobile_bootstrap_status_and_version_remain_public_through_nested_router() {
    let temp = tempfile::tempdir().unwrap();
    let mut state =
        rua_server::AppState::from_tempdir(temp.path().into(), rua_server::ServerProfile::Test);
    state.profile = rua_server::ServerProfile::MobileApk;
    state.auth = Arc::new(rua_server::auth::AuthService::new_disabled());
    let app = rua_server::router::build_app(state, None);
    for path in ["/api/auth/status", "/api/version", "/healthz"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn authenticated_mobile_status_fails_explicitly_without_native_host() {
    let temp = tempfile::tempdir().unwrap();
    let mut state =
        rua_server::AppState::from_tempdir(temp.path().into(), rua_server::ServerProfile::Test);
    state.profile = rua_server::ServerProfile::MobileApk;
    state.auth = Arc::new(rua_server::auth::AuthService::new_with_token(
        "test-token".into(),
    ));
    let app = rua_server::router::build_app(state, None);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/platform/phone")
                .header("authorization", "Bearer test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn mobile_never_accepts_bearer_credentials_in_query_strings() {
    let temp = tempfile::tempdir().unwrap();
    let mut state =
        rua_server::AppState::from_tempdir(temp.path().into(), rua_server::ServerProfile::Test);
    state.profile = rua_server::ServerProfile::MobileApk;
    state.auth = Arc::new(rua_server::auth::AuthService::new_with_token(
        "test-token".into(),
    ));
    let app = rua_server::router::build_app(state, None);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/platform/phone?token=test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
