use rua_channel::core::channel::{Channel, ToHandle};
use rua_channel::core::content::{OutgoingContentPart, OutgoingMessage};
use rua_channel::dingtalk::{AccessTokenManager, DingTalkChannel};
use std::sync::Arc;

#[test]
fn channel_exposes_stable_dingtalk_id() {
    let channel = DingTalkChannel::new();

    assert_eq!(channel.id(), "dingtalk");
}

#[test]
fn build_request_from_native_uses_dingtalk_parser() {
    let channel = DingTalkChannel::new();
    let native = serde_json::json!({
        "msgType": "text",
        "msgId": "msg-001",
        "conversationId": "conv-123",
        "conversationType": "1",
        "senderId": "user-456",
        "senderNick": "Alice",
        "robotCode": "bot-abc",
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xyz",
        "text": { "content": "hi" }
    });

    let req = channel.build_request_from_native(native).unwrap();

    assert_eq!(req.channel_id, "dingtalk");
    assert_eq!(req.user_id, "user-456");
}

#[test]
fn build_request_from_native_rejects_duplicate_message_id() {
    let channel = DingTalkChannel::new();
    let native = serde_json::json!({
        "msgType": "text",
        "msgId": "msg-001",
        "conversationId": "conv-123",
        "conversationType": "1",
        "senderId": "user-456",
        "senderNick": "Alice",
        "robotCode": "bot-abc",
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xyz",
        "text": { "content": "hi" }
    });

    channel.build_request_from_native(native.clone()).unwrap();

    let err = channel.build_request_from_native(native).unwrap_err();
    assert!(err.to_string().contains("duplicate dingtalk event"));
}

#[tokio::test]
async fn send_posts_rendered_markdown_to_session_webhook() {
    let received = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/robot/sendBySession",
        axum::routing::post(
            move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                let store = received_clone.clone();
                async move {
                    store.lock().await.push(body);
                    axum::http::StatusCode::OK
                }
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let webhook_url = format!("http://127.0.0.1:{}/robot/sendBySession", addr.port());
    let channel = DingTalkChannel::new();
    let target = ToHandle {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: Some("user-1".into()),
        reply_to: None,
        metadata: serde_json::json!({ "session_webhook": webhook_url }),
    };
    let message = OutgoingMessage {
        channel_id: "dingtalk".into(),
        conversation_id: "conv-1".into(),
        receiver_id: Some("user-1".into()),
        content: vec![
            OutgoingContentPart::Text {
                text: "hello".into(),
            },
            OutgoingContentPart::Markdown {
                text: "**user**".into(),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(target, message).await.unwrap();

    let messages = received.lock().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["msgtype"], "markdown");
    assert_eq!(messages[0]["markdown"]["text"], "hello\n**user**");

    server.abort();
}

#[tokio::test]
async fn send_public_image_url_posts_native_image_payload() {
    let received = Arc::new(tokio::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();
    let app = axum::Router::new().route(
        "/robot/sendBySession",
        axum::routing::post(
            move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                let store = received_clone.clone();
                async move {
                    store.lock().await.push(body);
                    axum::http::StatusCode::OK
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let webhook_url = format!("http://127.0.0.1:{}/robot/sendBySession", addr.port());
    let channel = DingTalkChannel::new();
    let target = ToHandle {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: Some("user-1".into()),
        reply_to: None,
        metadata: serde_json::json!({ "session_webhook": webhook_url }),
    };
    let message = OutgoingMessage {
        channel_id: "dingtalk".into(),
        conversation_id: "conv-1".into(),
        receiver_id: Some("user-1".into()),
        content: vec![
            OutgoingContentPart::Text {
                text: "summary".into(),
            },
            OutgoingContentPart::Image {
                url: Some("https://cdn.example.com/image.png".into()),
                path: None,
                alt: Some("diagram".into()),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(target, message).await.unwrap();

    let messages = received.lock().await;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["msgtype"], "markdown");
    assert_eq!(messages[0]["markdown"]["text"], "summary");
    assert_eq!(messages[1]["msgtype"], "image");
    assert_eq!(
        messages[1]["image"]["picURL"],
        "https://cdn.example.com/image.png"
    );

    server.abort();
}

#[tokio::test]
async fn send_local_media_uploads_then_posts_native_payloads() {
    let received = Arc::new(tokio::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let upload_seen = Arc::new(tokio::sync::Mutex::new(Vec::<(String, String)>::new()));
    let received_clone = received.clone();
    let upload_seen_clone = upload_seen.clone();
    let app = axum::Router::new()
        .route(
            "/v1.0/oauth2/accessToken",
            axum::routing::post(|| async {
                axum::Json(serde_json::json!({
                    "accessToken": "token-1",
                    "expireIn": 7200
                }))
            }),
        )
        .route(
            "/media/upload",
            axum::routing::post(
                move |axum::extract::Query(query): axum::extract::Query<
                    std::collections::HashMap<String, String>,
                >,
                      headers: axum::http::HeaderMap,
                      body: axum::body::Bytes| {
                    let seen = upload_seen_clone.clone();
                    async move {
                        let upload_type = query.get("type").cloned().unwrap_or_default();
                        let payload = format!(
                            "{}\n{}",
                            headers
                                .get("content-type")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default(),
                            String::from_utf8_lossy(&body)
                        );
                        seen.lock().await.push((upload_type, payload));
                        axum::Json(serde_json::json!({
                            "errcode": 0,
                            "media_id": "@media-file-1"
                        }))
                    }
                },
            ),
        )
        .route(
            "/robot/sendBySession",
            axum::routing::post(
                move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                    let store = received_clone.clone();
                    async move {
                        store.lock().await.push(body);
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
    let temp = tempfile::tempdir().unwrap();
    let image_path = temp.path().join("chart.png");
    let file_path = temp.path().join("report.pdf");
    let audio_path = temp.path().join("voice.amr");
    let video_path = temp.path().join("clip.mp4");
    std::fs::write(&image_path, b"png-bytes").unwrap();
    std::fs::write(&file_path, b"pdf-bytes").unwrap();
    std::fs::write(&audio_path, b"audio-bytes").unwrap();
    std::fs::write(&video_path, b"video-bytes").unwrap();

    let token_manager = Arc::new(
        AccessTokenManager::new("app".into(), "secret".into())
            .with_token_url(format!("{base_url}/v1.0/oauth2/accessToken")),
    );
    let channel = DingTalkChannel::new()
        .with_token_manager(token_manager)
        .with_upload_api_base_url(base_url.clone());
    let target = ToHandle {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: Some("user-1".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "session_webhook": format!("{base_url}/robot/sendBySession")
        }),
    };
    let message = OutgoingMessage {
        channel_id: "dingtalk".into(),
        conversation_id: "conv-1".into(),
        receiver_id: Some("user-1".into()),
        content: vec![
            OutgoingContentPart::Image {
                url: None,
                path: Some(image_path),
                alt: Some("chart.png".into()),
            },
            OutgoingContentPart::File {
                url: None,
                path: Some(file_path),
                name: Some("report.pdf".into()),
            },
            OutgoingContentPart::Audio {
                url: None,
                path: Some(audio_path),
                name: Some("voice.amr".into()),
            },
            OutgoingContentPart::Video {
                url: None,
                path: Some(video_path),
                name: Some("clip.mp4".into()),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(target, message).await.unwrap();

    let uploads = upload_seen.lock().await;
    assert_eq!(uploads.len(), 4);
    assert_eq!(uploads[0].0, "image");
    assert!(uploads[0].1.contains("multipart/form-data"));
    assert!(uploads[0].1.contains("png-bytes"));
    assert_eq!(uploads[1].0, "file");
    assert!(uploads[1].1.contains("pdf-bytes"));
    assert_eq!(uploads[2].0, "file");
    assert!(uploads[2].1.contains("audio-bytes"));
    assert_eq!(uploads[3].0, "file");
    assert!(uploads[3].1.contains("video-bytes"));
    let messages = received.lock().await;
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["msgtype"], "markdown");
    assert_eq!(messages[0]["markdown"]["title"], "Image");
    assert_eq!(
        messages[0]["markdown"]["text"],
        "![chart.png](@media-file-1)"
    );
    for message in messages.iter().skip(1) {
        assert_eq!(message["msgtype"], "file");
        assert_eq!(message["file"]["media_id"], "@media-file-1");
    }

    server.abort();
}

#[tokio::test]
async fn send_skips_empty_rendered_content() {
    let channel = DingTalkChannel::new();
    let target = ToHandle {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({
            "session_webhook": "http://127.0.0.1:1/nonexistent"
        }),
    };
    let message = OutgoingMessage {
        channel_id: "dingtalk".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Thinking {
            text: "internal only".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(target, message).await.unwrap();
}

#[tokio::test]
async fn send_requires_session_webhook_metadata() {
    let channel = DingTalkChannel::new();
    let target = ToHandle {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({}),
    };
    let message = OutgoingMessage {
        channel_id: "dingtalk".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "test".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    let err = channel.send(target, message).await.unwrap_err();

    assert!(matches!(err, rua_core::RuaError::Validation { .. }));
}

#[tokio::test]
async fn send_retries_transient_session_webhook_errors() {
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let call_count_clone = call_count.clone();

    let app = axum::Router::new().route(
        "/robot/sendBySession",
        axum::routing::post(move || {
            let count = call_count_clone.clone();
            async move {
                let n = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n < 2 {
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                } else {
                    axum::http::StatusCode::OK
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let webhook_url = format!("http://127.0.0.1:{}/robot/sendBySession", addr.port());
    let channel = DingTalkChannel::new();
    let target = ToHandle {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({ "session_webhook": webhook_url }),
    };
    let message = OutgoingMessage {
        channel_id: "dingtalk".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "retry test".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(target, message).await.unwrap();

    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);

    server.abort();
}
