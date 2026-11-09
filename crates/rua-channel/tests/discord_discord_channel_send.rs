use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use rua_channel::core::channel::Channel;
use rua_channel::core::content::{
    ChannelTarget, IncomingContentPart, OutgoingContentPart, OutgoingMessage,
};
use rua_channel::core::streaming::{PlatformStreamPolicy, StreamHandler, StreamType};
use rua_channel::core::{AgentChannelRequest, ProcessingIndicator};
use rua_channel::discord::{
    DiscordChannel, DiscordConfig, DiscordStreamHandler, DiscordTypingIndicator,
};
use std::time::Duration;
use tempfile::tempdir;
use tokio::sync::Mutex;

#[derive(Clone)]
struct WebhookState {
    bodies: Arc<Mutex<Vec<serde_json::Value>>>,
}

#[derive(Clone)]
struct MultipartState {
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
}

#[derive(Debug, Clone)]
struct CapturedRequest {
    content_type: String,
    body: Vec<u8>,
}

async fn webhook_handler(
    State(state): State<WebhookState>,
    Json(body): Json<serde_json::Value>,
) -> axum::http::StatusCode {
    state.bodies.lock().await.push(body);
    axum::http::StatusCode::NO_CONTENT
}

async fn webhook_handler_error() -> axum::http::StatusCode {
    axum::http::StatusCode::FORBIDDEN
}

async fn discord_bot_create_handler(
    State(state): State<WebhookState>,
    Path(channel_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    state.bodies.lock().await.push(serde_json::json!({
        "method": "create",
        "channel_id": channel_id,
        "body": body,
    }));
    Json(serde_json::json!({
        "id": "msg-1",
        "channel_id": channel_id,
        "content": ""
    }))
}

async fn discord_bot_edit_handler(
    State(state): State<WebhookState>,
    Path((channel_id, message_id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    state.bodies.lock().await.push(serde_json::json!({
        "method": "edit",
        "channel_id": channel_id,
        "message_id": message_id,
        "body": body,
    }));
    Json(serde_json::json!({
        "id": message_id,
        "channel_id": channel_id,
        "content": ""
    }))
}

async fn discord_bot_typing_handler(
    State(state): State<WebhookState>,
    Path(channel_id): Path<String>,
) -> axum::http::StatusCode {
    state.bodies.lock().await.push(serde_json::json!({
        "method": "typing",
        "channel_id": channel_id,
    }));
    axum::http::StatusCode::NO_CONTENT
}

async fn multipart_webhook_handler(
    State(state): State<MultipartState>,
    headers: axum::http::HeaderMap,
    body: Body,
) -> axum::http::StatusCode {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .expect("multipart body")
        .to_vec();
    state
        .requests
        .lock()
        .await
        .push(CapturedRequest { content_type, body });
    axum::http::StatusCode::NO_CONTENT
}

async fn start_mock_server(handler: Router) -> (tokio::task::JoinHandle<()>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/webhook");
    let handle = tokio::spawn(async move {
        axum::serve(listener, handler).await.unwrap();
    });
    (handle, url)
}

async fn start_discord_attachment_server() -> (tokio::task::JoinHandle<()>, String) {
    let app = Router::new()
        .route(
            "/attachments/diagram.png",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/png")
                    .body(Body::from("png-bytes"))
                    .unwrap()
            }),
        )
        .route(
            "/attachments/report.pdf",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/pdf")
                    .body(Body::from("pdf-bytes"))
                    .unwrap()
            }),
        )
        .route(
            "/attachments/voice.ogg",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "audio/ogg")
                    .body(Body::from("ogg-bytes"))
                    .unwrap()
            }),
        )
        .route(
            "/attachments/clip.mp4",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "video/mp4")
                    .body(Body::from("mp4-bytes"))
                    .unwrap()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (handle, base_url)
}

fn make_target(conversation_id: &str) -> ChannelTarget {
    ChannelTarget {
        agent_id: "test-agent".into(),
        conversation_id: conversation_id.into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::Value::Null,
    }
}

fn make_outgoing_msg(text: &str) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "discord".into(),
        conversation_id: "test-conv".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text { text: text.into() }],
        reply_to: None,
        metadata: serde_json::Value::Null,
    }
}

fn indicator_request() -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "agent-1".into(),
        user_id: "user-1".into(),
        session_id: "session-1".into(),
        input: vec![IncomingContentPart::Text {
            text: "hello".into(),
        }],
        channel_id: "discord".into(),
        conversation_id: "channel-1".into(),
        message_id: None,
        metadata: serde_json::json!({}),
    }
}

fn make_mixed_outgoing_msg(content: Vec<OutgoingContentPart>) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "discord".into(),
        conversation_id: "test-conv".into(),
        receiver_id: None,
        content,
        reply_to: None,
        metadata: serde_json::Value::Null,
    }
}

#[tokio::test]
async fn send_hello_posts_content_to_webhook() {
    let state = WebhookState {
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(state.clone());
    let (_handle, url) = start_mock_server(app).await;

    let config = DiscordConfig {
        webhook_url: url,
        bot_token: String::new(),
        ..Default::default()
    };
    let ch = DiscordChannel::new(config);

    let msg = make_outgoing_msg("hello");
    let to = make_target("test-conv");
    ch.send(to, msg).await.unwrap();

    let bodies = state.bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["content"], "hello");
}

#[tokio::test]
async fn send_empty_rendered_content_skips_webhook_post() {
    let state = WebhookState {
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(state.clone());
    let (_handle, url) = start_mock_server(app).await;

    let config = DiscordConfig {
        webhook_url: url,
        bot_token: String::new(),
        ..Default::default()
    };
    let ch = DiscordChannel::new(config);

    let msg = OutgoingMessage {
        channel_id: "discord".into(),
        conversation_id: "test-conv".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Thinking {
            text: "internal only".into(),
        }],
        reply_to: None,
        metadata: serde_json::Value::Null,
    };
    ch.send(make_target("test-conv"), msg).await.unwrap();

    let bodies = state.bodies.lock().await;
    assert!(bodies.is_empty(), "empty rendered content must not post");
}

#[tokio::test]
async fn webhook_4xx_returns_err_with_status() {
    let app = Router::new().route("/webhook", post(webhook_handler_error));
    let (_handle, url) = start_mock_server(app).await;

    let config = DiscordConfig {
        webhook_url: url,
        bot_token: String::new(),
        ..Default::default()
    };
    let ch = DiscordChannel::new(config);

    let msg = make_outgoing_msg("hello");
    let to = make_target("test-conv");
    let result = ch.send(to, msg).await;

    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("403"),
        "error should contain status code, got: {err_msg}"
    );
}

#[tokio::test]
async fn webhook_send_splits_long_content_at_discord_limit() {
    let state = WebhookState {
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(state.clone());
    let (_handle, url) = start_mock_server(app).await;

    let config = DiscordConfig {
        webhook_url: url,
        bot_token: String::new(),
        ..Default::default()
    };
    let ch = DiscordChannel::new(config);

    let msg = make_outgoing_msg(&"a".repeat(5000));
    let to = make_target("test-conv");
    ch.send(to, msg).await.unwrap();

    let bodies = state.bodies.lock().await;
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[0]["content"], "a".repeat(2000));
    assert_eq!(bodies[1]["content"], "a".repeat(2000));
    assert_eq!(bodies[2]["content"], "a".repeat(1000));
}

#[tokio::test]
async fn webhook_send_splits_unicode_content_without_losing_characters() {
    let state = WebhookState {
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(state.clone());
    let (_handle, url) = start_mock_server(app).await;

    let config = DiscordConfig {
        webhook_url: url,
        bot_token: String::new(),
        ..Default::default()
    };
    let ch = DiscordChannel::new(config);

    let text = "你".repeat(2501);
    let msg = make_outgoing_msg(&text);
    let to = make_target("test-conv");
    ch.send(to, msg).await.unwrap();

    let bodies = state.bodies.lock().await;
    assert_eq!(bodies.len(), 2);

    let combined = bodies
        .iter()
        .map(|body| body["content"].as_str().unwrap())
        .collect::<String>();
    assert_eq!(combined, text);
    assert!(bodies.iter().all(|body| {
        body["content"]
            .as_str()
            .is_some_and(|content| content.chars().count() <= 2000)
    }));
}

#[tokio::test]
async fn webhook_send_closes_and_reopens_markdown_fences_across_chunks() {
    let state = WebhookState {
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(state.clone());
    let (_handle, url) = start_mock_server(app).await;

    let config = DiscordConfig {
        webhook_url: url,
        bot_token: String::new(),
        ..Default::default()
    };
    let ch = DiscordChannel::new(config);

    let fenced_body = "let value = 42;\n".repeat(180);
    let text = format!("before\n```rust\n{fenced_body}```\nafter");
    let msg = make_outgoing_msg(&text);
    let to = make_target("test-conv");
    ch.send(to, msg).await.unwrap();

    let bodies = state.bodies.lock().await;
    assert!(
        bodies.len() > 1,
        "fixture should force Discord chunking, got {} body",
        bodies.len()
    );
    assert!(bodies.iter().all(|body| {
        body["content"]
            .as_str()
            .is_some_and(|content| content.chars().count() <= 2000)
    }));

    let first = bodies[0]["content"].as_str().unwrap();
    let second = bodies[1]["content"].as_str().unwrap();
    assert!(
        first.ends_with("\n```"),
        "chunk ending inside a code fence should close it: {first:?}"
    );
    assert!(
        second.starts_with("```rust\n"),
        "following chunk should reopen the same fence: {second:?}"
    );
    assert!(
        bodies.last().unwrap()["content"]
            .as_str()
            .unwrap()
            .ends_with("\n```\nafter"),
        "final chunk should preserve the original fence close and trailing text"
    );
}

#[tokio::test]
async fn webhook_send_posts_text_first_then_local_media_as_multipart_attachment() {
    let multipart_state = MultipartState {
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/webhook", post(multipart_webhook_handler))
        .with_state(multipart_state.clone());
    let (_handle, webhook_url) = start_mock_server(app).await;

    let temp = tempdir().unwrap();
    let image_path = temp.path().join("chart.png");
    std::fs::write(&image_path, "png-bytes").unwrap();
    let config = DiscordConfig {
        webhook_url,
        bot_token: String::new(),
        ..Default::default()
    };
    let ch = DiscordChannel::new(config);

    let msg = make_mixed_outgoing_msg(vec![
        OutgoingContentPart::Text {
            text: "summary".into(),
        },
        OutgoingContentPart::Image {
            url: None,
            path: Some(image_path),
            alt: Some("chart.png".into()),
        },
    ]);
    ch.send(make_target("test-conv"), msg).await.unwrap();

    let requests = multipart_state.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let text_body = std::str::from_utf8(&requests[0].body).unwrap();
    assert!(
        requests[0].content_type.starts_with("application/json"),
        "text should be sent first as JSON, got {}",
        requests[0].content_type
    );
    assert!(text_body.contains(r#""content":"summary""#));
    let media_body = std::str::from_utf8(&requests[1].body).unwrap();
    assert!(requests[1].content_type.starts_with("multipart/form-data"));
    assert!(media_body.contains("payload_json"));
    assert!(media_body.contains("files[0]"));
    assert!(media_body.contains("filename=\"chart.png\""));
    assert!(media_body.contains("png-bytes"));
}

#[tokio::test]
async fn webhook_send_downloads_url_media_before_discord_attachment_upload() {
    let multipart_state = MultipartState {
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let media_app = Router::new().route(
        "/files/report.pdf",
        get(|| async {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/pdf")
                .body(Body::from("pdf-bytes"))
                .unwrap()
        }),
    );
    let (asset_server, asset_url) = start_mock_server(media_app).await;
    let asset_base = asset_url.trim_end_matches("/webhook").to_string();

    let app = Router::new()
        .route("/webhook", post(multipart_webhook_handler))
        .with_state(multipart_state.clone());
    let (webhook_server, webhook_url) = start_mock_server(app).await;
    let config = DiscordConfig {
        webhook_url,
        bot_token: String::new(),
        ..Default::default()
    };
    let ch = DiscordChannel::new(config);
    let msg = make_mixed_outgoing_msg(vec![OutgoingContentPart::File {
        url: Some(format!("{asset_base}/files/report.pdf")),
        path: None,
        name: Some("report.pdf".into()),
    }]);

    ch.send(make_target("test-conv"), msg).await.unwrap();

    let requests = multipart_state.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let body = std::str::from_utf8(&requests[0].body).unwrap();
    assert!(body.contains("filename=\"report.pdf\""));
    assert!(body.contains("pdf-bytes"));

    asset_server.abort();
    webhook_server.abort();
}

#[tokio::test]
async fn webhook_send_uploads_local_audio_and_video_as_multipart_attachments() {
    let multipart_state = MultipartState {
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/webhook", post(multipart_webhook_handler))
        .with_state(multipart_state.clone());
    let (_handle, webhook_url) = start_mock_server(app).await;

    let temp = tempdir().unwrap();
    let audio_path = temp.path().join("voice.ogg");
    let video_path = temp.path().join("clip.mp4");
    std::fs::write(&audio_path, "ogg-bytes").unwrap();
    std::fs::write(&video_path, "mp4-bytes").unwrap();
    let config = DiscordConfig {
        webhook_url,
        bot_token: String::new(),
        ..Default::default()
    };
    let ch = DiscordChannel::new(config);
    let msg = make_mixed_outgoing_msg(vec![
        OutgoingContentPart::Audio {
            url: None,
            path: Some(audio_path),
            name: Some("voice.ogg".into()),
        },
        OutgoingContentPart::Video {
            url: None,
            path: Some(video_path),
            name: Some("clip.mp4".into()),
        },
    ]);

    ch.send(make_target("test-conv"), msg).await.unwrap();

    let requests = multipart_state.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let audio_body = std::str::from_utf8(&requests[0].body).unwrap();
    assert!(audio_body.contains("filename=\"voice.ogg\""));
    assert!(audio_body.contains("ogg-bytes"));
    let video_body = std::str::from_utf8(&requests[1].body).unwrap();
    assert!(video_body.contains("filename=\"clip.mp4\""));
    assert!(video_body.contains("mp4-bytes"));
}

#[tokio::test]
async fn webhook_send_unresolved_media_uses_visible_text_fallback() {
    let state = WebhookState {
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/webhook", post(webhook_handler))
        .with_state(state.clone());
    let (_handle, url) = start_mock_server(app).await;
    let config = DiscordConfig {
        webhook_url: url,
        bot_token: String::new(),
        ..Default::default()
    };
    let ch = DiscordChannel::new(config);
    let msg = make_mixed_outgoing_msg(vec![OutgoingContentPart::Audio {
        url: None,
        path: None,
        name: Some("voice.ogg".into()),
    }]);

    ch.send(make_target("test-conv"), msg).await.unwrap();

    let bodies = state.bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["content"], "[Audio: voice.ogg]");
}

#[tokio::test]
async fn channel_id_is_discord() {
    let ch = DiscordChannel::new(DiscordConfig::default());
    assert_eq!(ch.id(), "discord");
}

#[tokio::test]
async fn streaming_handler_creates_placeholder_and_final_edit() {
    let state = WebhookState {
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route(
            "/api/v10/channels/{channel_id}/messages",
            post(discord_bot_create_handler),
        )
        .route(
            "/api/v10/channels/{channel_id}/messages/{message_id}",
            axum::routing::patch(discord_bot_edit_handler),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}/api/v10", addr.port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let handler = DiscordStreamHandler::new(
        reqwest::Client::new(),
        base_url,
        "test-token".into(),
        "channel-1".into(),
        PlatformStreamPolicy::placeholder_edit(Duration::ZERO, 2000),
    );

    handler
        .on_delta(StreamType::Message, "hello")
        .await
        .expect("create placeholder");
    handler
        .on_delta(StreamType::Message, "hello there")
        .await
        .expect("delta edit");
    handler
        .on_end(StreamType::Message, "hello world")
        .await
        .expect("final edit");

    let bodies = state.bodies.lock().await;
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[0]["method"], "create");
    assert_eq!(bodies[0]["channel_id"], "channel-1");
    assert_eq!(bodies[0]["body"]["content"], "hello");
    assert_eq!(bodies[1]["method"], "edit");
    assert_eq!(bodies[1]["message_id"], "msg-1");
    assert_eq!(bodies[1]["body"]["content"], "hello there");
    assert_eq!(bodies[2]["method"], "edit");
    assert_eq!(bodies[2]["message_id"], "msg-1");
    assert_eq!(bodies[2]["body"]["content"], "hello world");

    server.abort();
}

#[tokio::test]
async fn streaming_handler_overlong_text_falls_back_to_final_message() {
    let state = WebhookState {
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route(
            "/api/v10/channels/{channel_id}/messages",
            post(discord_bot_create_handler),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}/api/v10", addr.port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let handler = DiscordStreamHandler::new(
        reqwest::Client::new(),
        base_url,
        "test-token".into(),
        "channel-1".into(),
        PlatformStreamPolicy::placeholder_edit(Duration::ZERO, 5),
    );

    handler
        .on_delta(StreamType::Message, "too long")
        .await
        .expect("overlong skip");
    handler
        .on_end(StreamType::Message, "too long final")
        .await
        .expect("final fallback");

    let bodies = state.bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["method"], "create");
    assert_eq!(bodies[0]["body"]["content"], "too long final");

    server.abort();
}

#[tokio::test]
async fn typing_indicator_posts_typing_and_stops_keepalive() {
    let state = WebhookState {
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route(
            "/api/v10/channels/{channel_id}/typing",
            post(discord_bot_typing_handler),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}/api/v10", addr.port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let indicator = DiscordTypingIndicator::new(
        reqwest::Client::new(),
        base_url,
        "test-token".into(),
        "channel-1".into(),
    );
    let req = indicator_request();

    indicator.start(&req).await.expect("start typing");
    indicator.stop(&req).await.expect("stop typing");
    tokio::time::sleep(Duration::from_millis(20)).await;

    let bodies = state.bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["method"], "typing");
    assert_eq!(bodies[0]["channel_id"], "channel-1");

    server.abort();
}

#[tokio::test]
async fn bot_send_posts_with_auth_header() {
    let state = WebhookState {
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let auth_header = Arc::new(Mutex::new(String::new()));
    let auth_clone = auth_header.clone();
    let state_clone = state.clone();

    let app = Router::new().route(
        "/api/v10/channels/{channel_id}/messages",
        post(
            move |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| {
                let store = state_clone.clone();
                let auth = auth_clone.clone();
                async move {
                    if let Some(h) = headers.get("authorization") {
                        *auth.lock().await = h.to_str().unwrap_or("").to_string();
                    }
                    store.bodies.lock().await.push(body);
                    axum::http::StatusCode::OK
                }
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let body = serde_json::json!({ "content": "hello bot" });
    let url = format!("{base_url}/api/v10/channels/12345/messages");

    let resp = reqwest::Client::new()
        .post(&url)
        .header("Authorization", "Bot test-bot-token")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let bodies = state.bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["content"], "hello bot");

    let auth = auth_header.lock().await;
    assert_eq!(*auth, "Bot test-bot-token");

    server.abort();
}

#[tokio::test]
async fn build_request_from_native_parses_message_create() {
    let ch = DiscordChannel::new(DiscordConfig::default());
    let native = serde_json::json!({
        "id": "msg-123",
        "channel_id": "ch-456",
        "guild_id": "guild-789",
        "author": {
            "id": "user-001",
            "username": "TestUser",
            "bot": false
        },
        "content": "hello world"
    });

    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.channel_id, "discord");
    assert_eq!(req.user_id, "user-001");
    assert_eq!(req.conversation_id, "ch-456");
    assert_eq!(req.message_id.as_deref(), Some("msg-123"));
    assert_eq!(req.metadata["guild_id"], "guild-789");
    assert_eq!(req.metadata["username"], "TestUser");

    assert_eq!(
        req.input,
        vec![IncomingContentPart::Text {
            text: "hello world".into()
        }]
    );
}

#[tokio::test]
async fn build_request_from_native_rejects_duplicate_message_id() {
    let ch = DiscordChannel::new(DiscordConfig::default());
    let native = serde_json::json!({
        "id": "msg-dup",
        "channel_id": "ch-456",
        "guild_id": "guild-789",
        "author": {
            "id": "user-001",
            "username": "TestUser",
            "bot": false
        },
        "content": "hello world"
    });

    ch.build_request_from_native(native.clone())
        .expect("first message should parse");
    let err = ch
        .build_request_from_native(native)
        .expect_err("duplicate message should be rejected");

    assert!(
        err.to_string().contains("duplicate discord event"),
        "unexpected duplicate error: {err}"
    );
}

#[tokio::test]
async fn build_request_from_native_group_bot_mention_sets_gate_metadata_and_strips_token() {
    let ch = DiscordChannel::new(DiscordConfig {
        bot_user_id: Some("bot-123".into()),
        ..Default::default()
    });
    let native = serde_json::json!({
        "id": "msg-mention",
        "channel_id": "ch-456",
        "guild_id": "guild-789",
        "author": {
            "id": "user-001",
            "username": "TestUser",
            "bot": false
        },
        "content": "<@bot-123> deploy now",
        "mentions": [
            {"id": "bot-123", "username": "Rua", "bot": true}
        ]
    });

    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["bot_mentioned"], true);
    assert_eq!(req.metadata["mentioned"], true);
    assert_eq!(
        req.input,
        vec![IncomingContentPart::Text {
            text: "deploy now".into()
        }]
    );
}

#[tokio::test]
async fn build_request_from_native_group_non_bot_mention_does_not_trigger_gate() {
    let ch = DiscordChannel::new(DiscordConfig {
        bot_user_id: Some("bot-123".into()),
        ..Default::default()
    });
    let native = serde_json::json!({
        "id": "msg-mention",
        "channel_id": "ch-456",
        "guild_id": "guild-789",
        "author": {
            "id": "user-001",
            "username": "TestUser",
            "bot": false
        },
        "content": "<@user-999> deploy now",
        "mentions": [
            {"id": "user-999", "username": "Alice", "bot": false}
        ]
    });

    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["bot_mentioned"], false);
    assert_eq!(req.metadata["mentioned"], false);
    assert_eq!(
        req.input,
        vec![IncomingContentPart::Text {
            text: "<@user-999> deploy now".into()
        }]
    );
}

#[tokio::test]
async fn build_request_from_native_preserves_discord_attachment_urls() {
    let ch = DiscordChannel::new(DiscordConfig::default());
    let native = serde_json::json!({
        "id": "msg-media-123",
        "channel_id": "ch-media",
        "author": {
            "id": "user-media",
            "username": "MediaUser"
        },
        "content": "Look at this",
        "attachments": [
            {
                "id": "att-1",
                "filename": "diagram.png",
                "content_type": "image/png",
                "url": "https://cdn.discordapp.com/attachments/diagram.png",
                "proxy_url": "https://media.discordapp.net/attachments/diagram.png"
            },
            {
                "id": "att-2",
                "filename": "",
                "content_type": "application/pdf",
                "url": "",
                "proxy_url": "https://media.discordapp.net/attachments/report.pdf"
            },
            {
                "id": "att-ignored",
                "filename": "missing.bin",
                "url": ""
            }
        ]
    });

    let req = ch.build_request_from_native(native).unwrap();

    assert_eq!(
        req.input,
        vec![
            IncomingContentPart::Text {
                text: "Look at this".into()
            },
            IncomingContentPart::Url {
                url: "https://cdn.discordapp.com/attachments/diagram.png".into(),
                title: Some("diagram.png".into()),
            },
            IncomingContentPart::Url {
                url: "https://media.discordapp.net/attachments/report.pdf".into(),
                title: Some("application/pdf".into()),
            }
        ]
    );
}

#[tokio::test]
async fn build_request_from_native_allows_attachment_only_messages() {
    let ch = DiscordChannel::new(DiscordConfig::default());
    let native = serde_json::json!({
        "id": "msg-attachment-only",
        "channel_id": "ch-media",
        "author": {
            "id": "user-media",
            "username": "MediaUser"
        },
        "content": "   ",
        "attachments": [
            {
                "id": "att-1",
                "filename": "photo.jpg",
                "content_type": "image/jpeg",
                "url": "https://cdn.discordapp.com/photo.jpg"
            }
        ]
    });

    let req = ch.build_request_from_native(native).unwrap();

    assert_eq!(
        req.input,
        vec![IncomingContentPart::Url {
            url: "https://cdn.discordapp.com/photo.jpg".into(),
            title: Some("photo.jpg".into()),
        }]
    );
}

#[tokio::test]
async fn download_inbound_attachments_injects_local_paths_for_typed_parts() {
    let temp = tempdir().unwrap();
    let (server, base_url) = start_discord_attachment_server().await;
    let ch = DiscordChannel::new(DiscordConfig {
        media_dir: temp.path().to_string_lossy().to_string(),
        ..Default::default()
    });
    let mut native = serde_json::json!({
        "id": "msg-media-123",
        "channel_id": "ch-media",
        "author": {
            "id": "user-media",
            "username": "MediaUser"
        },
        "content": "Look at this",
        "attachments": [
            {
                "id": "att-1",
                "filename": "diagram.png",
                "content_type": "image/png",
                "url": format!("{base_url}/attachments/diagram.png")
            },
            {
                "id": "att-2",
                "filename": "report.pdf",
                "content_type": "application/pdf",
                "url": format!("{base_url}/attachments/report.pdf")
            },
            {
                "id": "att-3",
                "filename": "voice.ogg",
                "content_type": "audio/ogg",
                "url": format!("{base_url}/attachments/voice.ogg")
            },
            {
                "id": "att-4",
                "filename": "clip.mp4",
                "content_type": "video/mp4",
                "url": format!("{base_url}/attachments/clip.mp4")
            }
        ]
    });

    ch.download_inbound_media(&mut native)
        .await
        .expect("discord attachment download");

    let req = ch
        .build_request_from_native(native)
        .expect("typed discord attachments");
    assert_eq!(req.input.len(), 5);
    match &req.input[1] {
        IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert_eq!(file_id, "att-1");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("image/png"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "png-bytes");
        }
        other => panic!("expected image part, got {other:?}"),
    }
    match &req.input[2] {
        IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "att-2");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("application/pdf"));
            assert_eq!(name.as_deref(), Some("report.pdf"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "pdf-bytes");
        }
        other => panic!("expected file part, got {other:?}"),
    }
    match &req.input[3] {
        IncomingContentPart::Audio {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "att-3");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("audio/ogg"));
            assert_eq!(name.as_deref(), Some("voice.ogg"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "ogg-bytes");
        }
        other => panic!("expected audio part, got {other:?}"),
    }
    match &req.input[4] {
        IncomingContentPart::Video {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "att-4");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("video/mp4"));
            assert_eq!(name.as_deref(), Some("clip.mp4"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "mp4-bytes");
        }
        other => panic!("expected video part, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn download_inbound_attachment_failure_keeps_url_payload() {
    let app = Router::new().route(
        "/attachments/missing.png",
        get(|| async { StatusCode::BAD_GATEWAY }),
    );
    let (server, webhook_url) = start_mock_server(app).await;
    let base_url = webhook_url.trim_end_matches("/webhook");
    let temp = tempdir().unwrap();
    let ch = DiscordChannel::new(DiscordConfig {
        media_dir: temp.path().to_string_lossy().to_string(),
        ..Default::default()
    });
    let mut native = serde_json::json!({
        "id": "msg-media-123",
        "channel_id": "ch-media",
        "author": {
            "id": "user-media",
            "username": "MediaUser"
        },
        "content": "",
        "attachments": [
            {
                "id": "att-1",
                "filename": "missing.png",
                "content_type": "image/png",
                "url": format!("{base_url}/attachments/missing.png")
            }
        ]
    });

    ch.download_inbound_media(&mut native)
        .await
        .expect("download failure should not abort fallback parsing");

    let req = ch
        .build_request_from_native(native)
        .expect("fallback discord attachment");
    match &req.input[0] {
        IncomingContentPart::Url { url, title } => {
            assert!(url.ends_with("/attachments/missing.png"));
            assert_eq!(title.as_deref(), Some("missing.png"));
        }
        other => panic!("expected fallback URL part, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn build_request_from_native_dm_no_guild() {
    let ch = DiscordChannel::new(DiscordConfig::default());
    let native = serde_json::json!({
        "id": "dm-001",
        "channel_id": "dm-ch-001",
        "author": {
            "id": "user-002",
            "username": "DMUser"
        },
        "content": "direct message"
    });

    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.conversation_id, "dm-ch-001");
    assert!(req.metadata["guild_id"].is_null());
}

#[tokio::test]
async fn build_request_from_native_invalid_payload() {
    let ch = DiscordChannel::new(DiscordConfig::default());
    let result = ch.build_request_from_native(serde_json::json!({"foo": "bar"}));
    assert!(result.is_err());
}

#[test]
fn gateway_opcode_round_trip() {
    use rua_channel::discord::gateway::OpCode;
    assert_eq!(OpCode::from_u8(0), Some(OpCode::Dispatch));
    assert_eq!(OpCode::from_u8(1), Some(OpCode::Heartbeat));
    assert_eq!(OpCode::from_u8(2), Some(OpCode::Identify));
    assert_eq!(OpCode::from_u8(6), Some(OpCode::Resume));
    assert_eq!(OpCode::from_u8(7), Some(OpCode::Reconnect));
    assert_eq!(OpCode::from_u8(9), Some(OpCode::InvalidSession));
    assert_eq!(OpCode::from_u8(10), Some(OpCode::Hello));
    assert_eq!(OpCode::from_u8(11), Some(OpCode::HeartbeatAck));
    assert_eq!(OpCode::from_u8(255), None);
}

#[test]
fn gateway_payload_parse_hello() {
    use rua_channel::discord::gateway::{GatewayPayload, HelloData};
    let raw = r#"{"op":10,"d":{"heartbeat_interval":41250}}"#;
    let payload: GatewayPayload = serde_json::from_str(raw).unwrap();
    assert_eq!(payload.op, 10);
    let hello: HelloData = serde_json::from_value(payload.d.unwrap()).unwrap();
    assert_eq!(hello.heartbeat_interval, 41250);
}

#[test]
fn gateway_payload_parse_dispatch_message_create() {
    use rua_channel::discord::gateway::{GatewayPayload, MessageCreateData};
    let raw = r#"{"op":0,"s":42,"t":"MESSAGE_CREATE","d":{"id":"msg-1","channel_id":"ch-1","author":{"id":"u-1","username":"Alice"},"content":"hello"}}"#;
    let payload: GatewayPayload = serde_json::from_str(raw).unwrap();
    assert_eq!(payload.op, 0);
    assert_eq!(payload.s, Some(42));
    assert_eq!(payload.t.as_deref(), Some("MESSAGE_CREATE"));

    let msg: MessageCreateData = serde_json::from_value(payload.d.unwrap()).unwrap();
    assert_eq!(msg.id, "msg-1");
    assert_eq!(msg.channel_id, "ch-1");
    assert_eq!(msg.author.id, "u-1");
    assert_eq!(msg.author.username, "Alice");
    assert_eq!(msg.content, "hello");
    assert!(msg.guild_id.is_none());
}

#[test]
fn gateway_payload_parse_ready() {
    use rua_channel::discord::gateway::{GatewayPayload, ReadyData};
    let raw = r#"{"op":0,"s":1,"t":"READY","d":{"session_id":"sess-abc","resume_gateway_url":"wss://resume.discord.gg"}}"#;
    let payload: GatewayPayload = serde_json::from_str(raw).unwrap();
    assert_eq!(payload.t.as_deref(), Some("READY"));

    let ready: ReadyData = serde_json::from_value(payload.d.unwrap()).unwrap();
    assert_eq!(ready.session_id, "sess-abc");
    assert_eq!(
        ready.resume_gateway_url.as_deref(),
        Some("wss://resume.discord.gg")
    );
}

#[test]
fn default_intents_value() {
    use rua_channel::discord::gateway::DEFAULT_INTENTS;

    assert_eq!(DEFAULT_INTENTS, 512 | 4096 | 32768);
    assert_eq!(DEFAULT_INTENTS, 37376);
}

#[test]
fn message_create_with_bot_flag() {
    use rua_channel::discord::gateway::MessageCreateData;
    let raw = r#"{"id":"msg-bot","channel_id":"ch-1","author":{"id":"bot-1","username":"BotUser","bot":true},"content":"bot message"}"#;
    let msg: MessageCreateData = serde_json::from_str(raw).unwrap();
    assert_eq!(msg.author.bot, Some(true));
}

#[test]
fn send_body_format() {
    let body = serde_json::json!({ "content": "hello discord" });
    assert_eq!(body["content"], "hello discord");
    assert!(body.get("tts").is_none());
}
