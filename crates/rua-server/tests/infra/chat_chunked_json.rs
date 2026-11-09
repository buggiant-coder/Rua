use std::convert::Infallible;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use futures::stream;
use futures::StreamExt;
use http_body_util::BodyExt;
use tower::ServiceExt;

use rua_server::endpoints::dto::ChatEvent;

fn mock_chat_events() -> Vec<Result<ChatEvent, String>> {
    vec![
        Ok(ChatEvent::Start {
            chat_id: "chat-1".into(),
            agent_id: Some("agent-1".into()),
        }),
        Ok(ChatEvent::TextDelta {
            message_id: "msg-text-1".into(),
            delta: "Hello".into(),
        }),
        Ok(ChatEvent::TextDelta {
            message_id: "msg-text-1".into(),
            delta: " world".into(),
        }),
        Ok(ChatEvent::ToolCall {
            message_id: "msg-tc-tc-1".into(),
            id: "tc-1".into(),
            name: "search".into(),
            arguments: r#"{"q":"test"}"#.into(),
        }),
        Ok(ChatEvent::ToolResult {
            message_id: "msg-tr-tc-1".into(),
            id: "tc-1".into(),
            name: Some("search".into()),
            content: serde_json::json!({"text": "result"}),
        }),
        Ok(ChatEvent::Done {
            finish_reason: Some("stop".into()),
        }),
    ]
}

fn mock_chat_events_with_error() -> Vec<Result<ChatEvent, String>> {
    vec![
        Ok(ChatEvent::Start {
            chat_id: "chat-err".into(),
            agent_id: Some("agent-1".into()),
        }),
        Err("LLM provider timeout".to_string()),
    ]
}

async fn mock_stream_handler() -> Response {
    let events = mock_chat_events();
    let json_stream = stream::iter(events).map(|ev| -> Result<String, Infallible> {
        match ev {
            Ok(event) => {
                let line = serde_json::to_string(&event).unwrap_or_default();
                Ok(format!("{line}\n"))
            }
            Err(e) => {
                let line = serde_json::json!({"type":"error","message": e}).to_string();
                Ok(format!("{line}\n"))
            }
        }
    });

    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from_stream(json_stream))
        .unwrap()
}

async fn mock_error_stream_handler() -> Response {
    let events = mock_chat_events_with_error();
    let json_stream = stream::iter(events).map(|ev| -> Result<String, Infallible> {
        match ev {
            Ok(event) => {
                let line = serde_json::to_string(&event).unwrap_or_default();
                Ok(format!("{line}\n"))
            }
            Err(e) => {
                let line = serde_json::json!({"type":"error","message": e}).to_string();
                Ok(format!("{line}\n"))
            }
        }
    });

    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from_stream(json_stream))
        .unwrap()
}

fn build_app() -> Router {
    Router::new()
        .route("/chat", post(mock_stream_handler))
        .route("/chat-error", post(mock_error_stream_handler))
}

fn chat_post(uri: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({"message": "hello"}).to_string(),
        ))
        .expect("request")
}

async fn collect_lines(resp: Response) -> Vec<serde_json::Value> {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let text = String::from_utf8(bytes.to_vec()).expect("utf8");
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("each line must be valid JSON"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_stream_returns_application_json_content_type() {
    let app = build_app();
    let resp = app.oneshot(chat_post("/chat")).await.expect("oneshot");

    assert_eq!(resp.status(), StatusCode::OK);

    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("Content-Type header")
        .to_str()
        .expect("header to str");
    assert_eq!(ct, "application/json", "must not be text/event-stream");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_stream_returns_multiple_json_lines() {
    let app = build_app();
    let resp = app.oneshot(chat_post("/chat")).await.expect("oneshot");

    let lines = collect_lines(resp).await;
    assert_eq!(lines.len(), 6, "expected 6 JSON lines from mock events");

    assert_eq!(lines[0]["type"], "start");
    assert_eq!(lines[0]["chat_id"], "chat-1");

    assert_eq!(lines[1]["type"], "text_delta");
    assert_eq!(lines[1]["delta"], "Hello");

    assert_eq!(lines[2]["type"], "text_delta");
    assert_eq!(lines[2]["delta"], " world");

    assert_eq!(lines[3]["type"], "tool_call");
    assert_eq!(lines[3]["name"], "search");

    assert_eq!(lines[4]["type"], "tool_result");
    assert_eq!(lines[4]["id"], "tc-1");

    assert_eq!(lines[5]["type"], "done");
    assert_eq!(lines[5]["finish_reason"], "stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_stream_each_line_is_independently_parseable() {
    let app = build_app();
    let resp = app.oneshot(chat_post("/chat")).await.expect("oneshot");

    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let text = String::from_utf8(bytes.to_vec()).expect("utf8");

    for (i, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
        assert!(
            parsed.is_ok(),
            "line {i} is not valid JSON: {line}\nerror: {}",
            parsed.unwrap_err()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_stream_ends_with_done_frame() {
    let app = build_app();
    let resp = app.oneshot(chat_post("/chat")).await.expect("oneshot");

    let lines = collect_lines(resp).await;
    let last = lines.last().expect("at least one line");
    assert_eq!(last["type"], "done", "last frame must be done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn error_stream_produces_error_frame() {
    let app = build_app();
    let resp = app
        .oneshot(chat_post("/chat-error"))
        .await
        .expect("oneshot");

    assert_eq!(resp.status(), StatusCode::OK);

    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("Content-Type header")
        .to_str()
        .expect("header to str");
    assert_eq!(ct, "application/json");

    let lines = collect_lines(resp).await;
    assert_eq!(lines.len(), 2, "expected start + error");

    assert_eq!(lines[0]["type"], "start");
    assert_eq!(lines[1]["type"], "error");
    assert_eq!(lines[1]["message"], "LLM provider timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn content_type_is_not_event_stream() {
    let app = build_app();
    let resp = app.oneshot(chat_post("/chat")).await.expect("oneshot");

    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("Content-Type header")
        .to_str()
        .expect("header to str");
    assert!(
        !ct.contains("text/event-stream"),
        "Content-Type must not contain text/event-stream, got: {ct}"
    );
    assert!(
        ct.contains("application/json"),
        "Content-Type must be application/json, got: {ct}"
    );
}
