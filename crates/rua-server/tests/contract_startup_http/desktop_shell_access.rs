//! The Desktop Shell credential protects the *data* surface, not the documents.
//!
//! The console is a static SPA bundle. Its entry document is opened with
//! `window.location.replace`, and a document navigation cannot carry an
//! `Authorization` header, so the bundle has to stay publicly readable or the
//! Shell cannot render itself. What must not happen is a browser tab reading
//! user data through it. These tests pin that split.

use axum::body::Body;
use axum::extract::Request;
use axum::http::header::{AUTHORIZATION, HOST, ORIGIN};
use axum::http::{Method, StatusCode};
use tower::ServiceExt;

use rua_server::desktop_host::DesktopHostConfig;
use rua_server::{router, AppState, ServerProfile};

const SESSION_TOKEN: &str = "shell-session-token-with-enough-entropy-0123456789";
const SHUTDOWN_TOKEN: &str = "launch-secret";

fn build_shell_app() -> (axum::Router, AppState, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let config = DesktopHostConfig::from_shutdown_token(SHUTDOWN_TOKEN.to_string())
        .expect("config")
        .with_session_token(Some(SESSION_TOKEN.to_string()));
    let app = router::build_app_with_desktop_host(state.clone(), None, Some(config));
    (app, state, temp)
}

async fn request(
    app: &axum::Router,
    method: Method,
    path: &str,
    token: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(HOST, "127.0.0.1:8765");
    if let Some(token) = token {
        builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_tab_can_load_the_bundle_but_cannot_read_data() {
    let (app, _state, _temp) = build_shell_app();

    // Documents: public by design. They are the packaged bundle and contain no
    // user data, so serving them to a browser tab is not a leak.
    for path in ["/", "/console/", "/console/index.html"] {
        let response = request(&app, Method::GET, path, None).await;
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }

    // Data: everything that returns user content still needs the credential,
    // so an unauthenticated tab gets nothing to render.
    for path in ["/api/agents", "/api/config/heartbeat", "/ws"] {
        let response = request(&app, Method::GET, path, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
}

/// The Shell must still be able to *render* its own console.
///
/// This is the hardest constraint in the whole feature: a document navigation
/// (`window.location.replace`) cannot carry an `Authorization` header, so a
/// rule that 401s `/console/` without a bearer header also 401s the Shell's own
/// WebView and the app shows raw JSON instead of the console. Whatever mechanism
/// admits the Shell to `/console/` therefore cannot be the header check alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_navigation_can_still_load_the_console_document() {
    let (app, _state, _temp) = build_shell_app();

    // Exactly what `desktopConsoleUrl` produces, minus any header a document
    // navigation could never add.
    let response = request(&app, Method::GET, "/console/?desktop=1&_=123", None).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the Shell navigates here with window.location.replace, which cannot \
         attach a bearer header; 401 leaves the desktop app showing raw JSON"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_api_requires_the_shell_credential() {
    let (app, _state, _temp) = build_shell_app();

    let unauthenticated = request(&app, Method::GET, "/api/agents", None).await;
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let authenticated = request(&app, Method::GET, "/api/agents", Some(SESSION_TOKEN)).await;
    assert_eq!(authenticated.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bootstrap_probes_stay_public_so_the_shell_can_start_the_runtime() {
    let (app, _state, _temp) = build_shell_app();

    for path in ["/healthz", "/readyz", "/api/version", "/api/auth/status"] {
        let response = request(&app, Method::GET, path, None).await;
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credential_is_bearer_only_and_never_accepted_from_the_query_string() {
    let (app, _state, _temp) = build_shell_app();

    let via_query = request(
        &app,
        Method::GET,
        &format!("/api/agents?token={SESSION_TOKEN}"),
        None,
    )
    .await;
    assert_eq!(
        via_query.status(),
        StatusCode::UNAUTHORIZED,
        "query-string credentials leak through history, logs, and Referer"
    );

    // The document itself is public, so a query token there is merely inert:
    // what matters is that it does not become a working credential.
    let via_query_root = request(
        &app,
        Method::GET,
        &format!("/console/?token={SESSION_TOKEN}"),
        None,
    )
    .await;
    assert_ne!(via_query_root.status(), StatusCode::UNAUTHORIZED);

    let data_with_query_token = request(
        &app,
        Method::GET,
        &format!("/api/agents?token={SESSION_TOKEN}"),
        None,
    )
    .await;
    assert_eq!(data_with_query_token.status(), StatusCode::UNAUTHORIZED);
}

/// A Twilio webhook cannot present a Shell bearer header, and its handler
/// authenticates by request signature instead. The Shell layer must not shadow
/// that check, or every compliant inbound call is rejected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voice_webhooks_are_not_gated_by_the_shell_credential() {
    let (app, _state, _temp) = build_shell_app();

    let response = request(&app, Method::POST, "/voice/incoming", None).await;
    assert_ne!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "voice handlers validate a Twilio signature; a Shell bearer gate here \
         would reject every inbound call"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forged_forwarding_headers_do_not_substitute_for_the_shell_credential() {
    let (app, _state, _temp) = build_shell_app();

    for header in [
        ("x-forwarded-for", "127.0.0.1"),
        ("x-real-ip", "127.0.0.1"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/agents")
                    .header(HOST, "127.0.0.1:8765")
                    .header(header.0, header.1)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{header:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_origin_browser_traffic_still_needs_the_credential() {
    let (app, _state, _temp) = build_shell_app();

    // Passing the Origin gate is not the same as being the Shell.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/agents")
                .header(HOST, "127.0.0.1:8765")
                .header(ORIGIN, "http://127.0.0.1:8765")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_without_a_session_token_falls_back_to_the_normal_credential() {
    // A desktop host config that carries only the shutdown token must not become
    // an unauthenticated surface: without a shell token the ordinary credential
    // applies, so an unauthenticated caller is still rejected.
    let temp = tempfile::tempdir().expect("tempdir");
    let mut state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    state.auth = std::sync::Arc::new(rua_server::auth::AuthService::new_with_token(
        SESSION_TOKEN.to_string(),
    ));
    let config =
        DesktopHostConfig::from_shutdown_token(SHUTDOWN_TOKEN.to_string()).expect("config");
    let app = router::build_app_with_desktop_host(state, None, Some(config));

    let anonymous = request(&app, Method::GET, "/api/agents", None).await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let authenticated = request(&app, Method::GET, "/api/agents", Some(SESSION_TOKEN)).await;
    assert_eq!(authenticated.status(), StatusCode::OK);
}

#[test]
fn short_session_tokens_are_rejected_instead_of_silently_degrading() {
    let config = DesktopHostConfig::from_shutdown_token(SHUTDOWN_TOKEN.to_string())
        .expect("config")
        .with_session_token(Some("too-short".to_string()));
    assert!(!config.requires_session_token());
}

#[test]
fn configured_session_tokens_match_in_constant_time_semantics() {
    let config = DesktopHostConfig::from_shutdown_token(SHUTDOWN_TOKEN.to_string())
        .expect("config")
        .with_session_token(Some(SESSION_TOKEN.to_string()));
    assert!(config.requires_session_token());
    assert!(config.matches_session_token(SESSION_TOKEN.as_bytes()));
    assert!(!config.matches_session_token(b"wrong"));
    assert!(!config.matches_session_token(
        SESSION_TOKEN.trim_end_matches('9').as_bytes()
    ));
}

/// The Shell's own console must keep working.
///
/// A browser cannot attach an `Authorization` header to a WebSocket handshake,
/// which is why the Console Transport authenticates inside its bounded first
/// frame instead. If the blanket shell-credential layer also covers the upgrade,
/// the Shell's own transport 401s before that frame is ever read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_console_transport_upgrade_is_not_blocked_before_its_auth_frame() {
    use futures::{SinkExt, StreamExt};
    use serde_json::Value;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;

    let temp = tempfile::tempdir().expect("tempdir");
    let mut state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    state.auth = std::sync::Arc::new(rua_server::auth::AuthService::new_with_token(
        SESSION_TOKEN.to_string(),
    ));
    let config = DesktopHostConfig::from_shutdown_token(SHUTDOWN_TOKEN.to_string())
        .expect("config")
        .with_session_token(Some(SESSION_TOKEN.to_string()));
    let app = router::build_app_with_desktop_host(state.clone(), None, Some(config));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let mut request = format!("ws://{address}/api/console/transport")
        .into_client_request()
        .expect("request");
    request.headers_mut().insert(
        "Origin",
        format!("http://{address}").parse().expect("origin"),
    );
    // Deliberately no `Authorization` header: this mirrors the Shell's WebView.
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("the Shell's own transport upgrade must not be rejected");

    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "auth", "version": 1, "token": SESSION_TOKEN
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("send auth frame");

    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .expect("ready frame within 5s")
        .expect("frame")
        .expect("frame ok");
    let text = frame.into_text().expect("text frame");
    let parsed: Value = serde_json::from_str(text.as_str()).expect("json");
    assert_eq!(parsed["type"], "ready");

    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_credential_passes_the_console_startup_verification() {
    // The console calls `/api/auth/verify` before rendering any route. If the
    // shell credential were not accepted here, an authenticated Shell would be
    // bounced to the login page even though every other request succeeded.
    let (app, _state, _temp) = build_shell_app();

    let response = request(&app, Method::GET, "/api/auth/verify", Some(SESSION_TOKEN)).await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["valid"], true);

    let without = request(&app, Method::GET, "/api/auth/verify", None).await;
    assert_eq!(without.status(), StatusCode::UNAUTHORIZED);
}
