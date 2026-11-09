use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::Query;
use axum::http::HeaderMap;
use axum::{routing::get, routing::post, Json, Router};
use rua_channel::core::channel::{AgentRequest, Channel, Process};
use rua_channel::core::content::{ChannelTarget, OutgoingContentPart, OutgoingMessage};
use rua_channel::core::streaming::{PlatformStreamPolicy, StreamHandler, StreamType};
use rua_channel::core::{
    AgentChannelRequest, ApprovalCardAction, IncomingContentPart, ProcessingIndicator,
};
use rua_channel::telegram::{TelegramChannel, TelegramConfig};
use std::time::Duration;
use tokio::sync::{Mutex, Notify};

struct RecordingProcess {
    requests: Arc<Mutex<Vec<AgentRequest>>>,
    notify: Arc<Notify>,
}

#[async_trait]
impl Process for RecordingProcess {
    async fn handle(
        &self,
        req: AgentRequest,
    ) -> futures::stream::BoxStream<'static, rua_core::RuntimeEvent> {
        self.requests.lock().await.push(req);
        self.notify.notify_one();
        Box::pin(futures::stream::empty())
    }
}

async fn start_telegram_sink() -> (
    String,
    Arc<Mutex<Vec<serde_json::Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let bodies = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let bodies_clone = bodies.clone();
    let media_bodies = bodies.clone();
    let doc_bodies = bodies.clone();
    let audio_bodies = bodies.clone();
    let video_bodies = bodies.clone();
    let edit_bodies = bodies.clone();
    let action_bodies = bodies.clone();
    let callback_bodies = bodies.clone();
    let app = Router::new()
        .route(
            "/bottest-token/sendMessage",
            post(move |Json(body): Json<serde_json::Value>| {
                let store = bodies_clone.clone();
                async move {
                    store.lock().await.push(body);
                    Json(serde_json::json!({"ok": true, "result": {"message_id": 1}}))
                }
            }),
        )
        .route(
            "/bottest-token/editMessageText",
            post(move |Json(body): Json<serde_json::Value>| {
                let store = edit_bodies.clone();
                async move {
                    store.lock().await.push(serde_json::json!({
                        "method": "editMessageText",
                        "body": body,
                    }));
                    Json(serde_json::json!({"ok": true, "result": {"message_id": 1}}))
                }
            }),
        )
        .route(
            "/bottest-token/sendChatAction",
            post(move |Json(body): Json<serde_json::Value>| {
                let store = action_bodies.clone();
                async move {
                    store.lock().await.push(serde_json::json!({
                        "method": "sendChatAction",
                        "body": body,
                    }));
                    Json(serde_json::json!({"ok": true, "result": true}))
                }
            }),
        )
        .route(
            "/bottest-token/answerCallbackQuery",
            post(move |Json(body): Json<serde_json::Value>| {
                let store = callback_bodies.clone();
                async move {
                    store.lock().await.push(serde_json::json!({
                        "method": "answerCallbackQuery",
                        "body": body,
                    }));
                    Json(serde_json::json!({"ok": true, "result": true}))
                }
            }),
        )
        .route(
            "/bottest-token/sendPhoto",
            post(move |headers: HeaderMap, body: Bytes| {
                let store = media_bodies.clone();
                async move {
                    store.lock().await.push(serde_json::json!({
                        "method": "sendPhoto",
                        "content_type": headers
                            .get("content-type")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default(),
                        "body": String::from_utf8_lossy(&body).to_string(),
                    }));
                    Json(serde_json::json!({"ok": true, "result": {"message_id": 2}}))
                }
            }),
        )
        .route(
            "/bottest-token/sendDocument",
            post(move |headers: HeaderMap, body: Bytes| {
                let store = doc_bodies.clone();
                async move {
                    store.lock().await.push(serde_json::json!({
                        "method": "sendDocument",
                        "content_type": headers
                            .get("content-type")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default(),
                        "body": String::from_utf8_lossy(&body).to_string(),
                    }));
                    Json(serde_json::json!({"ok": true, "result": {"message_id": 3}}))
                }
            }),
        )
        .route(
            "/bottest-token/sendAudio",
            post(move |headers: HeaderMap, body: Bytes| {
                let store = audio_bodies.clone();
                async move {
                    store.lock().await.push(serde_json::json!({
                        "method": "sendAudio",
                        "content_type": headers
                            .get("content-type")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default(),
                        "body": String::from_utf8_lossy(&body).to_string(),
                    }));
                    Json(serde_json::json!({"ok": true, "result": {"message_id": 4}}))
                }
            }),
        )
        .route(
            "/bottest-token/sendVideo",
            post(move |headers: HeaderMap, body: Bytes| {
                let store = video_bodies.clone();
                async move {
                    store.lock().await.push(serde_json::json!({
                        "method": "sendVideo",
                        "content_type": headers
                            .get("content-type")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default(),
                        "body": String::from_utf8_lossy(&body).to_string(),
                    }));
                    Json(serde_json::json!({"ok": true, "result": {"message_id": 5}}))
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://127.0.0.1:{}", addr.port()), bodies, server)
}

async fn start_telegram_polling_sink() -> (
    String,
    Arc<Mutex<Vec<serde_json::Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let bodies = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let bodies_for_updates = bodies.clone();
    let callback_bodies = bodies.clone();
    let edit_bodies = bodies.clone();
    let action_bodies = bodies.clone();
    let app = Router::new()
        .route(
            "/bottest-token/getUpdates",
            get(
                move |Query(params): Query<std::collections::HashMap<String, String>>| {
                    let store = bodies_for_updates.clone();
                    async move {
                        let offset = params
                            .get("offset")
                            .and_then(|value| value.parse::<i64>().ok())
                            .unwrap_or(0);
                        store.lock().await.push(serde_json::json!({
                            "method": "getUpdates",
                            "offset": offset,
                        }));
                        let result = if offset <= 0 {
                            serde_json::json!([{
                                "update_id": 1,
                                "callback_query": {
                                    "id": "cb-runtime",
                                    "from": {"id": 300, "is_bot": false, "username": "alice"},
                                    "message": {
                                        "message_id": 77,
                                        "chat": {"id": 100, "type": "private"}
                                    },
                                    "data": "tga:req-runtime"
                                }
                            }])
                        } else {
                            serde_json::json!([])
                        };
                        Json(serde_json::json!({"ok": true, "result": result}))
                    }
                },
            ),
        )
        .route(
            "/bottest-token/answerCallbackQuery",
            post(move |Json(body): Json<serde_json::Value>| {
                let store = callback_bodies.clone();
                async move {
                    store.lock().await.push(serde_json::json!({
                        "method": "answerCallbackQuery",
                        "body": body,
                    }));
                    Json(serde_json::json!({"ok": true, "result": true}))
                }
            }),
        )
        .route(
            "/bottest-token/editMessageText",
            post(move |Json(body): Json<serde_json::Value>| {
                let store = edit_bodies.clone();
                async move {
                    store.lock().await.push(serde_json::json!({
                        "method": "editMessageText",
                        "body": body,
                    }));
                    Json(serde_json::json!({"ok": true, "result": {"message_id": 77}}))
                }
            }),
        )
        .route(
            "/bottest-token/sendChatAction",
            post(move |Json(body): Json<serde_json::Value>| {
                let store = action_bodies.clone();
                async move {
                    store.lock().await.push(serde_json::json!({
                        "method": "sendChatAction",
                        "body": body,
                    }));
                    Json(serde_json::json!({"ok": true, "result": true}))
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://127.0.0.1:{}", addr.port()), bodies, server)
}

fn make_channel(base_url: String) -> TelegramChannel {
    TelegramChannel::new(TelegramConfig {
        bot_token: "test-token".into(),
        chat_id: "100".into(),
        allowed_users: vec![],
        ..TelegramConfig::default()
    })
    .with_base_url(base_url)
}

fn target() -> ChannelTarget {
    ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "100".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

fn text_message(text: String) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "telegram".into(),
        conversation_id: "100".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text { text }],
        reply_to: None,
        metadata: serde_json::json!({}),
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
        channel_id: "telegram".into(),
        conversation_id: "100".into(),
        message_id: None,
        metadata: serde_json::json!({}),
    }
}

fn approval_message() -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "telegram".into(),
        conversation_id: "100".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "Allow Bash?".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "tool_guard_approval",
            "approval_request_id": "req-123"
        }),
    }
}

fn fallback_media_message() -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "telegram".into(),
        conversation_id: "100".into(),
        receiver_id: None,
        content: vec![
            OutgoingContentPart::Text {
                text: "summary".into(),
            },
            OutgoingContentPart::Image {
                url: None,
                path: None,
                alt: Some("architecture diagram".into()),
            },
            OutgoingContentPart::File {
                url: None,
                path: None,
                name: Some("report.pdf".into()),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

fn native_media_message(
    image_path: std::path::PathBuf,
    file_path: std::path::PathBuf,
) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "telegram".into(),
        conversation_id: "100".into(),
        receiver_id: None,
        content: vec![
            OutgoingContentPart::Text {
                text: "summary".into(),
            },
            OutgoingContentPart::Image {
                url: None,
                path: Some(image_path),
                alt: Some("architecture diagram".into()),
            },
            OutgoingContentPart::File {
                url: None,
                path: Some(file_path),
                name: Some("report.pdf".into()),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

fn native_audio_video_message(
    audio_path: std::path::PathBuf,
    video_path: std::path::PathBuf,
) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "telegram".into(),
        conversation_id: "100".into(),
        receiver_id: None,
        content: vec![
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
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

#[tokio::test]
async fn send_splits_long_text_into_multiple_send_message_calls() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let channel = make_channel(base_url);

    channel
        .send(target(), text_message("a".repeat(9000)))
        .await
        .expect("telegram send");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[0]["chat_id"], "100");
    assert_eq!(bodies[0]["parse_mode"], "HTML");
    assert_eq!(bodies[0]["text"], "a".repeat(4000));
    assert_eq!(bodies[1]["text"], "a".repeat(4000));
    assert_eq!(bodies[2]["text"], "a".repeat(1000));

    server.abort();
}

#[tokio::test]
async fn streaming_handler_creates_placeholder_and_final_edit() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let handler = rua_channel::telegram::TelegramStreamHandler::new(
        reqwest::Client::new(),
        base_url,
        "test-token".into(),
        "100".into(),
        None,
        PlatformStreamPolicy::placeholder_edit(Duration::ZERO, 4000),
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

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[0]["chat_id"], "100");
    assert_eq!(bodies[0]["text"], "hello");
    assert_eq!(bodies[0]["parse_mode"], "HTML");
    assert_eq!(bodies[1]["method"], "editMessageText");
    assert_eq!(bodies[1]["body"]["chat_id"], "100");
    assert_eq!(bodies[1]["body"]["message_id"], 1);
    assert_eq!(bodies[1]["body"]["text"], "hello there");
    assert_eq!(bodies[2]["method"], "editMessageText");
    assert_eq!(bodies[2]["body"]["chat_id"], "100");
    assert_eq!(bodies[2]["body"]["message_id"], 1);
    assert_eq!(bodies[2]["body"]["text"], "hello world");

    server.abort();
}

#[tokio::test]
async fn streaming_handler_overlong_text_falls_back_to_final_send() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let handler = rua_channel::telegram::TelegramStreamHandler::new(
        reqwest::Client::new(),
        base_url,
        "test-token".into(),
        "100".into(),
        Some(serde_json::json!(42)),
        PlatformStreamPolicy::placeholder_edit(Duration::ZERO, 5),
    );

    handler
        .on_delta(StreamType::Message, "too long")
        .await
        .expect("overlong skip");
    handler
        .on_end(StreamType::Message, "too long final")
        .await
        .expect("final send fallback");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["chat_id"], "100");
    assert_eq!(bodies[0]["message_thread_id"], 42);
    assert_eq!(bodies[0]["text"], "too long final");

    server.abort();
}

#[tokio::test]
async fn typing_indicator_sends_chat_action_and_stops_keepalive() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let indicator = rua_channel::telegram::TelegramTypingIndicator::new(
        reqwest::Client::new(),
        base_url,
        "test-token".into(),
        "100".into(),
        Some(serde_json::json!(42)),
    );
    let req = indicator_request();

    indicator.start(&req).await.expect("start typing");
    indicator.stop(&req).await.expect("stop typing");

    tokio::time::sleep(Duration::from_millis(20)).await;

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["method"], "sendChatAction");
    assert_eq!(bodies[0]["body"]["chat_id"], "100");
    assert_eq!(bodies[0]["body"]["message_thread_id"], 42);
    assert_eq!(bodies[0]["body"]["action"], "typing");

    server.abort();
}

#[tokio::test]
async fn typing_indicator_restart_replaces_keepalive_and_stop_cleans_up() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let indicator = rua_channel::telegram::TelegramTypingIndicator::with_keepalive_interval(
        reqwest::Client::new(),
        base_url,
        "test-token".into(),
        "100".into(),
        None,
        Duration::from_millis(25),
    );
    let req = indicator_request();

    indicator.start(&req).await.expect("first start");
    indicator.start(&req).await.expect("restart typing");
    tokio::time::sleep(Duration::from_millis(65)).await;
    indicator.stop(&req).await.expect("stop typing");
    let count_after_stop = bodies.lock().await.len();
    tokio::time::sleep(Duration::from_millis(65)).await;

    let bodies = bodies.lock().await;
    assert!(
        count_after_stop >= 3,
        "restart plus keepalive should have sent at least 3 actions, got {count_after_stop}"
    );
    assert_eq!(
        bodies.len(),
        count_after_stop,
        "stop should abort keepalive without further typing actions"
    );
    assert!(bodies.iter().all(|body| {
        body["method"] == "sendChatAction"
            && body["body"]["chat_id"] == "100"
            && body["body"]["action"] == "typing"
    }));

    server.abort();
}

#[tokio::test]
async fn send_splits_long_text_at_space_boundaries_when_possible() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let channel = make_channel(base_url);
    let text = format!("{}tail", "word ".repeat(800));

    channel
        .send(target(), text_message(text))
        .await
        .expect("telegram send");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["chat_id"], "100");
    assert!(
        bodies[0]["text"].as_str().unwrap().ends_with(' '),
        "first Telegram chunk should end on a space boundary"
    );
    assert_eq!(bodies[1]["text"], "tail");

    server.abort();
}

#[tokio::test]
async fn send_tool_guard_approval_includes_inline_keyboard() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let channel = make_channel(base_url);

    channel
        .send(target(), approval_message())
        .await
        .expect("telegram approval send");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["chat_id"], "100");
    assert_eq!(bodies[0]["text"], "Allow Bash?");
    assert_eq!(
        bodies[0]["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
        "tga:req-123"
    );
    assert_eq!(
        bodies[0]["reply_markup"]["inline_keyboard"][0][1]["callback_data"],
        "tgd:req-123"
    );

    server.abort();
}

#[tokio::test]
async fn send_tool_guard_approval_preserves_message_thread_id() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let channel = make_channel(base_url);
    let mut to = target();
    to.metadata = serde_json::json!({
        "chat_id": "200",
        "message_thread_id": 789,
    });

    channel
        .send(to, approval_message())
        .await
        .expect("telegram threaded approval send");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["chat_id"], "200");
    assert_eq!(bodies[0]["message_thread_id"], 789);
    assert_eq!(
        bodies[0]["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
        "tga:req-123"
    );
    assert_eq!(
        bodies[0]["reply_markup"]["inline_keyboard"][0][1]["callback_data"],
        "tgd:req-123"
    );

    server.abort();
}

#[tokio::test]
async fn approval_callback_ack_and_update_use_native_telegram_methods() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let channel = make_channel(base_url);
    let text =
        TelegramChannel::approval_resolved_text("Bash", ApprovalCardAction::Approve, "alice");

    channel
        .answer_approval_callback("cb-123", "Approved")
        .await
        .expect("answer callback");
    channel
        .update_approval_message("100", 77, &text)
        .await
        .expect("update approval message");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["method"], "answerCallbackQuery");
    assert_eq!(bodies[0]["body"]["callback_query_id"], "cb-123");
    assert_eq!(bodies[0]["body"]["text"], "Approved");
    assert_eq!(bodies[1]["method"], "editMessageText");
    assert_eq!(bodies[1]["body"]["chat_id"], "100");
    assert_eq!(bodies[1]["body"]["message_id"], 77);
    assert_eq!(
        bodies[1]["body"]["text"],
        "✅ Approved by alice | Tool: Bash"
    );

    server.abort();
}

#[tokio::test]
async fn polling_callback_acknowledges_and_updates_approval_message() {
    let (base_url, bodies, server) = start_telegram_polling_sink().await;
    let channel = make_channel(base_url);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let notify = Arc::new(Notify::new());
    channel
        .start(Arc::new(RecordingProcess {
            requests: requests.clone(),
            notify: notify.clone(),
        }))
        .await
        .expect("start telegram polling");

    tokio::time::timeout(Duration::from_secs(3), notify.notified())
        .await
        .expect("callback should reach process");
    channel.stop().await.expect("stop telegram polling");

    let requests = requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].metadata["approval_request_id"], "req-runtime");
    assert_eq!(requests[0].metadata["approval_action"], "approve");

    let bodies = bodies.lock().await;
    let ack = bodies
        .iter()
        .find(|body| body["method"] == "answerCallbackQuery")
        .expect("answerCallbackQuery body");
    assert_eq!(ack["body"]["callback_query_id"], "cb-runtime");
    assert_eq!(ack["body"]["text"], "Approved");
    let edit = bodies
        .iter()
        .find(|body| body["method"] == "editMessageText")
        .expect("editMessageText body");
    assert_eq!(edit["body"]["chat_id"], "100");
    assert_eq!(edit["body"]["message_id"], 77);
    assert_eq!(edit["body"]["text"], "✅ Approved by alice | Tool: tool");

    server.abort();
}

#[tokio::test]
async fn send_uploads_local_image_and_file_after_text() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let channel = make_channel(base_url);
    let temp = tempfile::tempdir().unwrap();
    let image_path = temp.path().join("diagram.png");
    let file_path = temp.path().join("report.pdf");
    std::fs::write(&image_path, b"png-bytes").unwrap();
    std::fs::write(&file_path, b"pdf-bytes").unwrap();

    channel
        .send(target(), native_media_message(image_path, file_path))
        .await
        .expect("telegram native media send");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 3);
    assert_eq!(bodies[0]["chat_id"], "100");
    assert_eq!(bodies[0]["parse_mode"], "HTML");
    assert_eq!(bodies[0]["text"], "summary");
    assert_eq!(bodies[1]["method"], "sendPhoto");
    assert!(bodies[1]["content_type"]
        .as_str()
        .unwrap()
        .starts_with("multipart/form-data"));
    assert!(bodies[1]["body"]
        .as_str()
        .unwrap()
        .contains("name=\"chat_id\""));
    assert!(bodies[1]["body"]
        .as_str()
        .unwrap()
        .contains("name=\"photo\""));
    assert!(bodies[1]["body"].as_str().unwrap().contains("png-bytes"));
    assert_eq!(bodies[2]["method"], "sendDocument");
    assert!(bodies[2]["body"]
        .as_str()
        .unwrap()
        .contains("name=\"document\""));
    assert!(bodies[2]["body"].as_str().unwrap().contains("pdf-bytes"));

    server.abort();
}

#[tokio::test]
async fn send_uploads_local_audio_and_video_natively() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let channel = make_channel(base_url);
    let temp = tempfile::tempdir().unwrap();
    let audio_path = temp.path().join("voice.ogg");
    let video_path = temp.path().join("clip.mp4");
    std::fs::write(&audio_path, b"ogg-bytes").unwrap();
    std::fs::write(&video_path, b"mp4-bytes").unwrap();

    channel
        .send(target(), native_audio_video_message(audio_path, video_path))
        .await
        .expect("telegram native audio/video send");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["method"], "sendAudio");
    assert!(bodies[0]["content_type"]
        .as_str()
        .unwrap()
        .starts_with("multipart/form-data"));
    assert!(bodies[0]["body"]
        .as_str()
        .unwrap()
        .contains("name=\"audio\""));
    assert!(bodies[0]["body"].as_str().unwrap().contains("ogg-bytes"));
    assert_eq!(bodies[1]["method"], "sendVideo");
    assert!(bodies[1]["body"]
        .as_str()
        .unwrap()
        .contains("name=\"video\""));
    assert!(bodies[1]["body"].as_str().unwrap().contains("mp4-bytes"));

    server.abort();
}

#[tokio::test]
async fn send_keeps_visible_fallback_for_unresolved_media() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let channel = make_channel(base_url);

    channel
        .send(target(), fallback_media_message())
        .await
        .expect("telegram media fallback send");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(
        bodies[0]["text"],
        "summary\narchitecture diagram\n[File: report.pdf]"
    );

    server.abort();
}

#[tokio::test]
async fn send_splits_unicode_text_without_losing_characters() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let channel = make_channel(base_url);
    let text = "你".repeat(1800);

    channel
        .send(target(), text_message(text.clone()))
        .await
        .expect("telegram send");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 2);
    let combined = bodies
        .iter()
        .map(|body| body["text"].as_str().unwrap())
        .collect::<String>();
    assert_eq!(combined, text);
    assert!(bodies
        .iter()
        .all(|body| { body["text"].as_str().is_some_and(|text| text.len() <= 4000) }));

    server.abort();
}

#[tokio::test]
async fn send_uses_chat_id_from_target_metadata_when_present() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let channel = make_channel(base_url);
    let mut to = target();
    to.metadata = serde_json::json!({"chat_id": "200"});

    channel
        .send(to, text_message("hello".into()))
        .await
        .expect("telegram send");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["chat_id"], "200");
    assert_eq!(bodies[0]["text"], "hello");

    server.abort();
}

#[tokio::test]
async fn send_includes_message_thread_id_from_target_metadata() {
    let (base_url, bodies, server) = start_telegram_sink().await;
    let channel = make_channel(base_url);
    let mut to = target();
    to.metadata = serde_json::json!({
        "chat_id": "200",
        "message_thread_id": 789,
    });

    channel
        .send(to, text_message("thread hello".into()))
        .await
        .expect("telegram send");

    let bodies = bodies.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["chat_id"], "200");
    assert_eq!(bodies[0]["message_thread_id"], 789);
    assert_eq!(bodies[0]["text"], "thread hello");

    server.abort();
}
