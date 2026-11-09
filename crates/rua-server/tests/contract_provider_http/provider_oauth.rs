use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;

use super::support::{build_app, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openrouter_oauth_start_returns_browser_redirect_contract_and_pending_status() {
    let test = build_app();

    let start = request(
        &test.app,
        Method::POST,
        "/api/providers/openrouter/oauth/start",
        None,
    )
    .await;

    assert_eq!(start.status(), StatusCode::OK);
    let body = read_json(start).await;
    assert_eq!(body["flow_type"], "browser_redirect");
    let state = body["state"].as_str().expect("state");
    assert!(!state.is_empty());
    let authorize_url = body["authorize_url"].as_str().expect("authorize_url");
    assert!(authorize_url.starts_with("https://openrouter.ai/auth?"));
    assert!(
        authorize_url.contains("callback_url="),
        "authorize_url should carry the callback URL: {authorize_url}"
    );
    assert!(
        authorize_url.contains("%2Fapi%2Fproviders%2Fopenrouter%2Foauth%2Fcallback"),
        "callback should route back to the Rust provider OAuth callback: {authorize_url}"
    );

    let status = request(
        &test.app,
        Method::GET,
        &format!("/api/providers/openrouter/oauth/status?state={state}"),
        None,
    )
    .await;
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(
        read_json(status).await,
        serde_json::json!({"status": "pending", "error": Value::Null})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_oauth_rejects_unsupported_provider_and_missing_state() {
    let test = build_app();

    let start = request(
        &test.app,
        Method::POST,
        "/api/providers/dashscope/oauth/start",
        None,
    )
    .await;
    assert_eq!(start.status(), StatusCode::NOT_FOUND);
    assert!(read_json(start).await["detail"]
        .as_str()
        .is_some_and(|detail| detail.contains("does not support OAuth")));

    let status = request(
        &test.app,
        Method::GET,
        "/api/providers/openrouter/oauth/status",
        None,
    )
    .await;
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(
        read_json(status).await,
        serde_json::json!({"status": "failed", "error": "Missing state"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_oauth_callback_error_returns_popup_html() {
    let test = build_app();

    let callback = request(
        &test.app,
        Method::GET,
        "/api/providers/openrouter/oauth/callback?error=access_denied&error_description=Denied+by+test",
        None,
    )
    .await;

    assert_eq!(callback.status(), StatusCode::BAD_REQUEST);
    assert!(callback
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/html")));
    let body = callback
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let body = String::from_utf8(body.to_vec()).expect("html utf8");
    assert!(body.contains("Denied by test"));
    assert!(body.contains("provider-oauth"));
    assert!(body.contains("provider_oauth_result"));
}
