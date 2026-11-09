use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use rua_server::desktop_host::{
    DesktopHostConfig, DESKTOP_SHUTDOWN_PATH, DESKTOP_SHUTDOWN_TOKEN_HEADER,
};
use rua_server::{router, AppState, ServerProfile};

fn app_with_token(token: Option<&str>) -> (axum::Router, AppState, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    let config = token
        .map(str::to_string)
        .and_then(DesktopHostConfig::from_shutdown_token);
    let app = router::build_app_with_desktop_host(state.clone(), None, config);
    (app, state, temp)
}

async fn shutdown_request(app: &axum::Router, token: Option<&str>) -> axum::response::Response {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(DESKTOP_SHUTDOWN_PATH)
        .header("host", "127.0.0.1:8765");
    if let Some(token) = token {
        builder = builder.header(DESKTOP_SHUTDOWN_TOKEN_HEADER, token);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("JSON response")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_shutdown_route_is_absent_without_desktop_launch_configuration() {
    let (app, state, _temp) = app_with_token(None);

    let response = shutdown_request(&app, Some("irrelevant")).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["detail"], "Not found");
    assert!(!*state.shutdown_rx().borrow());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_shutdown_rejects_missing_or_invalid_tokens_as_not_found() {
    let (app, state, _temp) = app_with_token(Some("launch-secret"));

    for token in [None, Some("wrong"), Some("launch-secret-extra")] {
        let response = shutdown_request(&app, token).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(response).await["detail"], "Not found");
    }
    assert!(!*state.shutdown_rx().borrow());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_shutdown_accepts_the_per_launch_token_and_signals_graceful_stop() {
    let (app, state, _temp) = app_with_token(Some("launch-secret"));
    let mut shutdown = state.shutdown_rx();

    let response = shutdown_request(&app, Some("launch-secret")).await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(body_json(response).await["status"], "shutting_down");
    shutdown.changed().await.expect("shutdown signal");
    assert!(*shutdown.borrow());
}
