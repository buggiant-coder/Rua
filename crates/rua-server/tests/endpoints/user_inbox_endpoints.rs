use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use rua_server::services::user_inbox::{InboxDelivery, InboxSender};
use rua_server::{router, AppState, ServerProfile};
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

fn build_app() -> (axum::Router, AppState, TempDir) {
    let temp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(temp.path().to_path_buf(), ServerProfile::Test);
    (router::build_app(state.clone(), None), state, temp)
}

async fn request(
    app: &axum::Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json")
}

#[tokio::test]
async fn global_inbox_http_lifecycle_is_available_without_business_producers() {
    let (app, state, _temp) = build_app();
    let delivered = state
        .user_inbox
        .deliver(InboxDelivery {
            sender: InboxSender {
                kind: "test-producer".to_string(),
                id: Some("producer-1".to_string()),
                display_name: "Test Producer".to_string(),
            },
            title: "Infrastructure message".to_string(),
            body_markdown: "This is **Markdown**.".to_string(),
        })
        .await
        .expect("deliver through internal seam");
    let id = delivered.id;

    let response = request(
        &app,
        Method::GET,
        "/api/inbox/messages?limit=10&read=false",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = json_body(response).await;
    assert_eq!(page["items"][0]["id"], id);
    assert_eq!(page["unread_count"], 1);

    let response = request(
        &app,
        Method::PATCH,
        &format!("/api/inbox/messages/{id}"),
        Some(json!({ "read": true })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(json_body(response).await["read_at"].is_string());

    let response = request(&app, Method::GET, "/api/inbox/unread-count", None).await;
    assert_eq!(json_body(response).await["count"], 0);

    state
        .user_inbox
        .set_read(&id, false)
        .await
        .expect("mark unread through internal seam");
    let response = request(&app, Method::POST, "/api/inbox/read-all", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["updated_count"], 1);

    let response = request(
        &app,
        Method::DELETE,
        &format!("/api/inbox/messages/{id}"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let response = request(
        &app,
        Method::GET,
        &format!("/api/inbox/messages/{id}"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        json_body(response).await,
        json!({ "detail": "Inbox message not found" })
    );
}

#[tokio::test]
async fn inbox_http_rejects_invalid_delivery_cursor_and_repeat_delete() {
    let (app, _state, _temp) = build_app();
    let response = request(&app, Method::POST, "/api/inbox/messages", Some(json!({}))).await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

    let response = request(
        &app,
        Method::GET,
        "/api/inbox/messages?cursor=not-hex",
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        json_body(response).await,
        json!({ "detail": "invalid cursor" })
    );

    let response = request(&app, Method::DELETE, "/api/inbox/messages/missing", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        json_body(response).await,
        json!({ "detail": "Inbox message not found" })
    );
}
