use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD,
    CONNECTION, HOST, ORIGIN, UPGRADE,
};
use axum::http::{Method, StatusCode};
use tower::ServiceExt;

use rua_server::desktop_host::{
    allowed_loopback_host, origin_matches_host, DesktopHostConfig, DESKTOP_SHUTDOWN_PATH,
    DESKTOP_SHUTDOWN_TOKEN_HEADER,
};
use rua_server::{router, AppState, ServerProfile};

fn build_desktop_app() -> (axum::Router, AppState, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let config =
        DesktopHostConfig::from_shutdown_token("launch-secret".to_string()).expect("config");
    let app = router::build_app_with_desktop_host(state.clone(), None, Some(config));
    (app, state, temp)
}

async fn request(
    app: &axum::Router,
    method: Method,
    path: &str,
    host: &str,
    origin: Option<&str>,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(HOST, host);
    if let Some(origin) = origin {
        builder = builder.header(ORIGIN, origin);
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

#[test]
fn desktop_host_accepts_only_expected_loopback_authorities() {
    for host in [
        "localhost:8765",
        "127.0.0.1:8765",
        "127.12.1.2:8765",
        "[::1]:8765",
    ] {
        assert!(allowed_loopback_host(host), "host={host}");
    }
    for host in [
        "example.com:8765",
        "0.0.0.0:8765",
        "192.168.1.2:8765",
        "localhost.example:8765",
        "",
    ] {
        assert!(!allowed_loopback_host(host), "host={host}");
    }
}

#[test]
fn desktop_origin_requires_the_same_scheme_host_and_port() {
    assert!(origin_matches_host(
        "http://127.0.0.1:8765",
        "127.0.0.1:8765"
    ));
    assert!(origin_matches_host("http://[::1]:8765", "[::1]:8765"));
    assert!(!origin_matches_host(
        "http://127.0.0.1:9000",
        "127.0.0.1:8765"
    ));
    assert!(!origin_matches_host(
        "https://127.0.0.1:8765",
        "127.0.0.1:8765"
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_mode_rejects_non_loopback_host_headers() {
    let (app, _state, _temp) = build_desktop_app();

    let response = request(
        &app,
        Method::GET,
        "/readyz",
        "attacker.example:8765",
        None,
        &[],
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_origin_and_native_clients_reach_the_desktop_shutdown_contract() {
    for origin in [Some("http://127.0.0.1:8765"), None] {
        let (app, state, _temp) = build_desktop_app();
        let response = request(
            &app,
            Method::POST,
            DESKTOP_SHUTDOWN_PATH,
            "127.0.0.1:8765",
            origin,
            &[(DESKTOP_SHUTDOWN_TOKEN_HEADER, "launch-secret")],
        )
        .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(*state.shutdown_rx().borrow());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_origin_is_rejected_before_mutating_endpoint_behavior() {
    let (app, state, _temp) = build_desktop_app();

    let response = request(
        &app,
        Method::POST,
        DESKTOP_SHUTDOWN_PATH,
        "127.0.0.1:8765",
        Some("https://attacker.example"),
        &[(DESKTOP_SHUTDOWN_TOKEN_HEADER, "launch-secret")],
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!*state.shutdown_rx().borrow());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_readiness_and_version_paths_allow_bootstrap_cross_origin_reads() {
    let (app, _state, _temp) = build_desktop_app();

    for path in ["/healthz", "/readyz", "/api/version"] {
        let response = request(
            &app,
            Method::GET,
            path,
            "127.0.0.1:8765",
            Some("http://127.0.0.1:5174"),
            &[],
        )
        .await;
        assert_ne!(response.status(), StatusCode::FORBIDDEN, "path={path}");
        assert_eq!(
            response.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&"*".parse().expect("header")),
            "path={path}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreign_origin_cannot_open_console_sse_or_websocket_contracts() {
    let (app, _state, _temp) = build_desktop_app();

    let sse = request(
        &app,
        Method::GET,
        "/api/agents/default/chats/test-chat/console/activity",
        "127.0.0.1:8765",
        Some("https://attacker.example"),
        &[],
    )
    .await;
    assert_eq!(sse.status(), StatusCode::FORBIDDEN);

    let websocket = request(
        &app,
        Method::GET,
        "/ws",
        "127.0.0.1:8765",
        Some("https://attacker.example"),
        &[
            (CONNECTION.as_str(), "Upgrade"),
            (UPGRADE.as_str(), "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;
    assert_eq!(websocket.status(), StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_origin_console_sse_and_websocket_requests_pass_the_origin_gate() {
    let (app, _state, _temp) = build_desktop_app();
    let origin = Some("http://127.0.0.1:8765");

    let sse = request(
        &app,
        Method::GET,
        "/api/agents/default/chats/test-chat/console/activity",
        "127.0.0.1:8765",
        origin,
        &[],
    )
    .await;
    assert_ne!(sse.status(), StatusCode::FORBIDDEN);

    let websocket = request(
        &app,
        Method::GET,
        "/ws",
        "127.0.0.1:8765",
        origin,
        &[
            (CONNECTION.as_str(), "Upgrade"),
            (UPGRADE.as_str(), "websocket"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ],
    )
    .await;
    assert_ne!(websocket.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        websocket.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&"http://127.0.0.1:8765".parse().expect("origin header"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_preflight_allows_same_origin_and_rejects_foreign_api_origins() {
    let (app, _state, _temp) = build_desktop_app();
    let preflight_headers = [
        (ACCESS_CONTROL_REQUEST_METHOD.as_str(), "POST"),
        (
            ACCESS_CONTROL_REQUEST_HEADERS.as_str(),
            "content-type,x-agent-id",
        ),
    ];

    let same_origin = request(
        &app,
        Method::OPTIONS,
        "/api/agents/default/chats/test-chat/console/chat",
        "127.0.0.1:8765",
        Some("http://127.0.0.1:8765"),
        &preflight_headers,
    )
    .await;
    assert_eq!(same_origin.status(), StatusCode::NO_CONTENT);

    let foreign = request(
        &app,
        Method::OPTIONS,
        "/api/agents/default/chats/test-chat/console/chat",
        "127.0.0.1:8765",
        Some("https://attacker.example"),
        &preflight_headers,
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
}
