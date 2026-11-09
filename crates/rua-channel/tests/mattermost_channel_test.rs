use std::sync::Arc;

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use rua_channel::core::channel::Channel;
use rua_channel::core::content::{
    AgentChannelRequest, ChannelTarget, IncomingContentPart, OutgoingContentPart, OutgoingMessage,
};
use rua_channel::core::ProcessingIndicator;
use rua_channel::mattermost::{
    parser, MattermostChannel, MattermostConfig, MattermostSender, MattermostTypingIndicator,
};
use tempfile::tempdir;

fn make_channel(base_url: &str) -> MattermostChannel {
    let config = MattermostConfig {
        url: base_url.to_string(),
        bot_token: "test-token".to_string(),
        ..Default::default()
    };
    MattermostChannel::new(config)
}

fn indicator_request() -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "agent-1".into(),
        user_id: "user-1".into(),
        session_id: "session-1".into(),
        input: vec![IncomingContentPart::Text {
            text: "hello".into(),
        }],
        channel_id: "mattermost".into(),
        conversation_id: "conv-1".into(),
        message_id: None,
        metadata: serde_json::json!({}),
    }
}

async fn start_mattermost_file_fixture() -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().route(
        "/api/v4/files/{file_id}",
        get(
            |Path(file_id): Path<String>, headers: HeaderMap| async move {
                let authorized = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    == Some("Bearer test-token");
                if authorized && file_id == "file_123" {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "image/png")
                        .body(Body::from("png-bytes"))
                        .unwrap()
                } else if authorized && file_id == "file_456" {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "application/pdf")
                        .body(Body::from("pdf-bytes"))
                        .unwrap()
                } else if authorized && file_id == "file_audio" {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "audio/ogg")
                        .body(Body::from("ogg-bytes"))
                        .unwrap()
                } else if authorized && file_id == "file_video" {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "video/mp4")
                        .body(Body::from("mp4-bytes"))
                        .unwrap()
                } else {
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::empty())
                        .unwrap()
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{}", addr.port()), server)
}

#[test]
fn channel_id_is_mattermost() {
    let ch = make_channel("http://localhost:8065");
    assert_eq!(ch.id(), "mattermost");
}

#[test]
fn build_request_from_native_ok() {
    let ch = make_channel("http://localhost:8065");
    let native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "D",
            "post": serde_json::json!({
                "id": "post-001",
                "channel_id": "ch-123",
                "user_id": "user-456",
                "message": "hello",
                "root_id": ""
            }).to_string()
        }
    });
    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.channel_id, "mattermost");
    assert_eq!(req.user_id, "user-456");
    assert_eq!(req.session_id, "mattermost_dm:ch-123");
    assert_eq!(req.conversation_id, "ch-123");
}

#[test]
fn build_request_from_native_preserves_post_attachments_as_placeholders() {
    let ch = make_channel("http://localhost:8065");
    let native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "D",
            "post": serde_json::json!({
                "id": "post-attachments",
                "channel_id": "ch-123",
                "user_id": "user-456",
                "message": "see attachments",
                "root_id": "",
                "file_ids": ["file_123", "file_456", "missing_name"],
                "metadata": {
                    "files": [
                        {"id": "file_123", "name": "image.png"},
                        {"id": "file_456", "name": "document.pdf"},
                        {"id": "missing_name", "name": ""}
                    ]
                }
            }).to_string()
        }
    });

    let req = ch.build_request_from_native(native).unwrap();

    assert_eq!(
        req.input,
        vec![
            IncomingContentPart::Text {
                text: "see attachments".into()
            },
            IncomingContentPart::Text {
                text: "[file: image.png]".into()
            },
            IncomingContentPart::Text {
                text: "[file: document.pdf]".into()
            },
            IncomingContentPart::Text {
                text: "[file: missing_name]".into()
            }
        ]
    );
}

#[test]
fn build_request_from_native_preserves_attachment_only_posts() {
    let ch = make_channel("http://localhost:8065");
    let native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "O",
            "post": serde_json::json!({
                "id": "post-file-only",
                "channel_id": "ch-789",
                "user_id": "user-111",
                "message": "",
                "root_id": "",
                "file_ids": ["file_only"],
                "metadata": {
                    "files": [{"id": "file_only", "name": "photo.jpg"}]
                }
            }).to_string()
        }
    });

    let req = ch.build_request_from_native(native).unwrap();

    assert_eq!(
        req.input,
        vec![IncomingContentPart::Text {
            text: "[file: photo.jpg]".into()
        }]
    );
    assert_eq!(req.session_id, "mattermost_thread:post-file-only");
}

#[tokio::test]
async fn download_inbound_attachments_injects_local_paths_for_typed_parts() {
    let temp = tempdir().unwrap();
    let (base_url, server) = start_mattermost_file_fixture().await;
    let ch = MattermostChannel::new(MattermostConfig {
        url: base_url,
        bot_token: "test-token".into(),
        media_dir: temp.path().to_string_lossy().to_string(),
    });
    let mut native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "D",
            "post": serde_json::json!({
                "id": "post-attachments",
                "channel_id": "ch-123",
                "user_id": "user-456",
                "message": "see attachments",
                "root_id": "",
                "file_ids": ["file_123", "file_456", "file_audio", "file_video"],
                "metadata": {
                    "files": [
                        {"id": "file_123", "name": "image.png", "mime_type": "image/png"},
                        {"id": "file_456", "name": "document.pdf", "mime_type": "application/pdf"},
                        {"id": "file_audio", "name": "voice.ogg", "mime_type": "audio/ogg"},
                        {"id": "file_video", "name": "clip.mp4", "mime_type": "video/mp4"}
                    ]
                }
            }).to_string()
        }
    });

    ch.download_inbound_media(&mut native)
        .await
        .expect("mattermost media download");

    let req = ch
        .build_request_from_native(native)
        .expect("typed mattermost attachments");
    assert_eq!(req.input.len(), 5);
    match &req.input[1] {
        IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert_eq!(file_id, "file_123");
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
            assert_eq!(file_id, "file_456");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("application/pdf"));
            assert_eq!(name.as_deref(), Some("document.pdf"));
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
            assert_eq!(file_id, "file_audio");
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
            assert_eq!(file_id, "file_video");
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
async fn download_inbound_attachment_failure_keeps_placeholder_payload() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/api/v4/files/{file_id}",
        get(|| async { StatusCode::BAD_GATEWAY }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let temp = tempdir().unwrap();
    let ch = MattermostChannel::new(MattermostConfig {
        url: format!("http://127.0.0.1:{}", addr.port()),
        bot_token: "test-token".into(),
        media_dir: temp.path().to_string_lossy().to_string(),
    });
    let mut native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "D",
            "post": serde_json::json!({
                "id": "post-attachments",
                "channel_id": "ch-123",
                "user_id": "user-456",
                "message": "",
                "root_id": "",
                "file_ids": ["file_123"],
                "metadata": {
                    "files": [{"id": "file_123", "name": "image.png"}]
                }
            }).to_string()
        }
    });

    ch.download_inbound_media(&mut native)
        .await
        .expect("download failure should not abort fallback parsing");

    let req = ch
        .build_request_from_native(native)
        .expect("fallback mattermost attachment");
    assert_eq!(
        req.input,
        vec![IncomingContentPart::Text {
            text: "[file: image.png]".into()
        }]
    );

    server.abort();
}

#[test]
fn build_request_thread_session_id() {
    let ch = make_channel("http://localhost:8065");
    let native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "O",
            "post": serde_json::json!({
                "id": "post-002",
                "channel_id": "ch-789",
                "user_id": "user-111",
                "message": "thread reply",
                "root_id": "root-abc"
            }).to_string()
        }
    });
    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.session_id, "mattermost_thread:root-abc");
    assert_eq!(req.metadata["root_id"], "root-abc");
}

#[test]
fn build_request_rejects_duplicate_post_id() {
    let ch = make_channel("http://localhost:8065");
    let native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "O",
            "post": serde_json::json!({
                "id": "post-dup",
                "channel_id": "ch-789",
                "user_id": "user-111",
                "message": "hello",
                "root_id": ""
            }).to_string()
        }
    });

    ch.build_request_from_native(native.clone())
        .expect("first post should parse");
    let err = ch
        .build_request_from_native(native)
        .expect_err("duplicate post should be rejected");

    assert!(
        err.to_string().contains("duplicate mattermost event"),
        "unexpected duplicate error: {err}"
    );
}

#[test]
fn build_request_new_thread_uses_post_id_as_root() {
    let ch = make_channel("http://localhost:8065");
    let native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "O",
            "post": serde_json::json!({
                "id": "post-new",
                "channel_id": "ch-789",
                "user_id": "user-111",
                "message": "@bot hello",
                "root_id": ""
            }).to_string()
        }
    });
    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.session_id, "mattermost_thread:post-new");
    assert_eq!(req.metadata["root_id"], "post-new");
}

#[test]
fn parse_with_routing_strips_bot_mention_and_preserves_thread_root() {
    let native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "O",
            "post": serde_json::json!({
                "id": "post-mention",
                "channel_id": "ch-789",
                "user_id": "user-111",
                "message": "@rua-bot deploy now",
                "root_id": "root-abc"
            }).to_string()
        }
    });

    let req = parser::parse_with_routing(native, "bot-1", "rua-bot")
        .unwrap()
        .expect("mentioned group post should route");

    assert_eq!(req.session_id, "mattermost_thread:root-abc");
    assert_eq!(req.metadata["root_id"], "root-abc");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "deploy now"),
        _ => panic!("expected Text content"),
    }
}

#[test]
fn parse_with_routing_drops_unmentioned_group_and_bot_self_posts() {
    let group_native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "O",
            "post": serde_json::json!({
                "id": "post-unmentioned",
                "channel_id": "ch-789",
                "user_id": "user-111",
                "message": "deploy now",
                "root_id": ""
            }).to_string()
        }
    });
    assert!(parser::parse_with_routing(group_native, "bot-1", "rua-bot")
        .unwrap()
        .is_none());

    let self_native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "D",
            "post": serde_json::json!({
                "id": "post-self",
                "channel_id": "dm-1",
                "user_id": "bot-1",
                "message": "self",
                "root_id": ""
            }).to_string()
        }
    });
    assert!(parser::parse_with_routing(self_native, "bot-1", "rua-bot")
        .unwrap()
        .is_none());
}

#[test]
fn parse_with_routing_routes_direct_messages_without_mention() {
    let native = serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "D",
            "post": serde_json::json!({
                "id": "post-dm",
                "channel_id": "dm-1",
                "user_id": "user-111",
                "message": "hello",
                "root_id": ""
            }).to_string()
        }
    });

    let req = parser::parse_with_routing(native, "bot-1", "rua-bot")
        .unwrap()
        .expect("DM should route without mention");
    assert_eq!(req.session_id, "mattermost_dm:dm-1");
    assert_eq!(
        req.input,
        vec![IncomingContentPart::Text {
            text: "hello".into()
        }]
    );
}

#[tokio::test]
async fn integration_send_to_mock_server() {
    use tokio::sync::Mutex;

    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/api/v4/posts",
        axum::routing::post(
            move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                let store = received_clone.clone();
                async move {
                    store.lock().await.push(body);
                    (axum::http::StatusCode::CREATED, "")
                }
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let ch = make_channel(&base_url);

    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "ch-123".into(),
        sender_id: Some("user-456".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "mm_channel_id": "ch-123",
            "root_id": "root-abc",
        }),
    };

    let msg = OutgoingMessage {
        channel_id: "mattermost".into(),
        conversation_id: "ch-123".into(),
        receiver_id: Some("user-456".into()),
        content: vec![OutgoingContentPart::Text {
            text: "hello from bot".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    ch.send(target, msg).await.unwrap();

    let messages = received.lock().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["channel_id"], "ch-123");
    assert_eq!(messages[0]["message"], "hello from bot");
    assert_eq!(messages[0]["root_id"], "root-abc");

    server.abort();
}

#[tokio::test]
async fn integration_send_uploads_local_image_and_file_after_text() {
    let received = Arc::new(tokio::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let upload_bodies = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let post_received = received.clone();
    let upload_received = upload_bodies.clone();

    let app = axum::Router::new()
        .route(
            "/api/v4/files",
            post(move |headers: HeaderMap, body: axum::body::Bytes| {
                let upload_received = upload_received.clone();
                async move {
                    assert_eq!(
                        headers
                            .get(header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok()),
                        Some("Bearer test-token")
                    );
                    let mut uploads = upload_received.lock().await;
                    uploads.push(String::from_utf8_lossy(&body).to_string());
                    let id = format!("file_{}", uploads.len());
                    (
                        StatusCode::CREATED,
                        axum::Json(serde_json::json!({
                            "file_infos": [{"id": id}]
                        })),
                    )
                }
            }),
        )
        .route(
            "/api/v4/posts",
            axum::routing::post(
                move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                    let store = post_received.clone();
                    async move {
                        store.lock().await.push(body);
                        (StatusCode::CREATED, "")
                    }
                },
            ),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let ch = make_channel(&base_url);
    let temp = tempdir().unwrap();
    let image_path = temp.path().join("image.png");
    let file_path = temp.path().join("report.pdf");
    let audio_path = temp.path().join("voice.ogg");
    let video_path = temp.path().join("clip.mp4");
    std::fs::write(&image_path, b"png-bytes").unwrap();
    std::fs::write(&file_path, b"pdf-bytes").unwrap();
    std::fs::write(&audio_path, b"ogg-bytes").unwrap();
    std::fs::write(&video_path, b"mp4-bytes").unwrap();

    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "ch-123".into(),
        sender_id: Some("user-456".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "mm_channel_id": "ch-123",
            "root_id": "root-abc",
        }),
    };

    let msg = OutgoingMessage {
        channel_id: "mattermost".into(),
        conversation_id: "ch-123".into(),
        receiver_id: Some("user-456".into()),
        content: vec![
            OutgoingContentPart::Text {
                text: "summary".into(),
            },
            OutgoingContentPart::Image {
                url: None,
                path: Some(image_path),
                alt: Some("diagram".into()),
            },
            OutgoingContentPart::File {
                url: None,
                path: Some(file_path),
                name: Some("report.pdf".into()),
            },
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
    };

    ch.send(target, msg).await.unwrap();

    let uploads = upload_bodies.lock().await;
    assert_eq!(uploads.len(), 4);
    assert!(uploads[0].contains("png-bytes"));
    assert!(uploads[1].contains("pdf-bytes"));
    assert!(uploads[2].contains("ogg-bytes"));
    assert!(uploads[3].contains("mp4-bytes"));
    let posts = received.lock().await;
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[0]["message"], "summary");
    assert_eq!(posts[0]["root_id"], "root-abc");
    assert_eq!(posts[1]["message"], "");
    assert_eq!(posts[1]["file_ids"][0], "file_1");
    assert_eq!(posts[1]["file_ids"][1], "file_2");
    assert_eq!(posts[1]["file_ids"][2], "file_3");
    assert_eq!(posts[1]["file_ids"][3], "file_4");

    server.abort();
}

#[tokio::test]
async fn integration_send_empty_content_skips_post() {
    let ch = make_channel("http://127.0.0.1:1");

    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({
            "mm_channel_id": "ch-1",
            "root_id": "",
        }),
    };

    let msg = OutgoingMessage {
        channel_id: "mattermost".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Thinking {
            text: "internal only".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    ch.send(target, msg).await.unwrap();
}

#[tokio::test]
async fn integration_send_missing_mm_channel_id_errors() {
    let ch = make_channel("http://127.0.0.1:1");

    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    let msg = OutgoingMessage {
        channel_id: "mattermost".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "test".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    let err = ch.send(target, msg).await.unwrap_err();
    assert!(matches!(err, rua_core::RuaError::Validation { .. }));
}

#[tokio::test]
async fn integration_retry_on_server_error() {
    use std::sync::atomic::{AtomicU32, Ordering};

    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = call_count.clone();

    let app = axum::Router::new().route(
        "/api/v4/posts",
        axum::routing::post(move || {
            let count = call_count_clone.clone();
            async move {
                let n = count.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR
                } else {
                    axum::http::StatusCode::CREATED
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let ch = make_channel(&base_url);

    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({
            "mm_channel_id": "ch-retry",
            "root_id": "",
        }),
    };

    let msg = OutgoingMessage {
        channel_id: "mattermost".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "retry test".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    ch.send(target, msg).await.unwrap();

    use std::sync::atomic::Ordering as O;
    assert_eq!(call_count.load(O::SeqCst), 3);

    server.abort();
}

#[tokio::test]
async fn integration_send_long_text_posts_multiple_chunks() {
    use tokio::sync::Mutex;

    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/api/v4/posts",
        axum::routing::post(
            move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                let store = received_clone.clone();
                async move {
                    store.lock().await.push(body);
                    (axum::http::StatusCode::CREATED, "")
                }
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let ch = make_channel(&base_url);

    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({
            "mm_channel_id": "ch-long",
            "root_id": "",
        }),
    };

    let msg = OutgoingMessage {
        channel_id: "mattermost".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: format!("{} {}", "a".repeat(2500), "b".repeat(2500)),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    ch.send(target, msg).await.unwrap();

    let messages = received.lock().await;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["channel_id"], "ch-long");
    assert_eq!(messages[0]["message"], "a".repeat(2500));
    assert_eq!(messages[1]["channel_id"], "ch-long");
    assert_eq!(messages[1]["message"], "b".repeat(2500));

    server.abort();
}

#[tokio::test]
async fn integration_send_long_text_prefers_newline_chunk_boundaries() {
    use tokio::sync::Mutex;

    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/api/v4/posts",
        axum::routing::post(
            move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                let store = received_clone.clone();
                async move {
                    store.lock().await.push(body);
                    (axum::http::StatusCode::CREATED, "")
                }
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let ch = make_channel(&base_url);

    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({
            "mm_channel_id": "ch-newline",
            "root_id": "root-newline",
        }),
    };

    let first = "a".repeat(3000);
    let second = "b".repeat(3000);
    let msg = OutgoingMessage {
        channel_id: "mattermost".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: format!("{first}\n{second}"),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    ch.send(target, msg).await.unwrap();

    let messages = received.lock().await;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["channel_id"], "ch-newline");
    assert_eq!(messages[0]["root_id"], "root-newline");
    assert_eq!(messages[0]["message"], first);
    assert_eq!(messages[1]["message"], second);

    server.abort();
}

#[tokio::test]
async fn integration_send_long_unicode_text_respects_char_boundaries() {
    use tokio::sync::Mutex;

    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/api/v4/posts",
        axum::routing::post(
            move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                let store = received_clone.clone();
                async move {
                    store.lock().await.push(body);
                    (axum::http::StatusCode::CREATED, "")
                }
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let ch = make_channel(&base_url);

    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({
            "mm_channel_id": "ch-unicode",
            "root_id": "",
        }),
    };

    let msg = OutgoingMessage {
        channel_id: "mattermost".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "你".repeat(1400),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    ch.send(target, msg).await.unwrap();

    let messages = received.lock().await;
    assert_eq!(messages.len(), 2);

    let combined = messages
        .iter()
        .map(|body| body["message"].as_str().unwrap())
        .collect::<String>();
    assert_eq!(combined, "你".repeat(1400));
    assert!(messages.iter().all(|body| {
        body["message"]
            .as_str()
            .is_some_and(|message| message.len() <= 4000)
    }));

    server.abort();
}

#[tokio::test]
async fn sender_get_bot_info_reads_users_me_and_sends_bearer_token() {
    use tokio::sync::Mutex;

    let auth_headers = Arc::new(Mutex::new(Vec::<String>::new()));
    let auth_headers_clone = auth_headers.clone();

    let app = axum::Router::new().route(
        "/api/v4/users/me",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let store = auth_headers_clone.clone();
            async move {
                let auth = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                store.lock().await.push(auth);
                axum::Json(serde_json::json!({
                    "id": "bot-123",
                    "username": "rua-bot",
                }))
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://127.0.0.1:{}/", addr.port());
    let sender = MattermostSender::new(&base_url, "test-token");

    let (bot_id, bot_username) = sender.get_bot_info().await.unwrap();

    assert_eq!(bot_id, "bot-123");
    assert_eq!(bot_username, "rua-bot");
    assert_eq!(auth_headers.lock().await.as_slice(), ["Bearer test-token"]);

    server.abort();
}

#[tokio::test]
async fn sender_get_bot_info_reports_non_success_status() {
    let app = axum::Router::new().route(
        "/api/v4/users/me",
        axum::routing::get(|| async { axum::http::StatusCode::UNAUTHORIZED }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let sender = MattermostSender::new(&base_url, "test-token");

    let err = sender
        .get_bot_info()
        .await
        .expect_err("401 /users/me should be surfaced");

    assert!(
        err.to_string()
            .contains("mattermost /users/me returned 401"),
        "unexpected error: {err}",
    );

    server.abort();
}

#[tokio::test]
async fn sender_fetch_thread_context_reads_bounded_thread_posts() {
    use tokio::sync::Mutex;

    let auth_headers = Arc::new(Mutex::new(Vec::<String>::new()));
    let auth_headers_clone = auth_headers.clone();

    let app = axum::Router::new().route(
        "/api/v4/posts/{post_id}/thread",
        axum::routing::get(move |Path(post_id): Path<String>, headers: HeaderMap| {
            let auth_headers = auth_headers_clone.clone();
            async move {
                let auth = headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                auth_headers.lock().await.push(auth);
                assert_eq!(post_id, "root-abc");
                axum::Json(serde_json::json!({
                    "order": ["root-abc", "reply-1", "reply-2"],
                    "posts": {
                        "root-abc": {
                            "id": "root-abc",
                            "user_id": "user-1",
                            "message": "root question",
                            "create_at": 10
                        },
                        "reply-1": {
                            "id": "reply-1",
                            "user_id": "user-2",
                            "message": "first reply",
                            "create_at": 20
                        },
                        "reply-2": {
                            "id": "reply-2",
                            "user_id": "user-3",
                            "message": "second reply",
                            "create_at": 30
                        }
                    }
                }))
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let sender = MattermostSender::new(&format!("http://127.0.0.1:{}", addr.port()), "test-token");
    let context = sender
        .fetch_thread_context("root-abc", 2)
        .await
        .expect("thread context");

    assert_eq!(context.len(), 2);
    assert_eq!(context[0]["id"], "reply-1");
    assert_eq!(context[0]["user_id"], "user-2");
    assert_eq!(context[0]["message"], "first reply");
    assert_eq!(context[1]["id"], "reply-2");
    assert_eq!(auth_headers.lock().await.as_slice(), ["Bearer test-token"]);

    server.abort();
}

#[tokio::test]
async fn typing_indicator_posts_mattermost_typing_body_and_stops_keepalive() {
    use tokio::sync::Mutex;

    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let auth_headers = Arc::new(Mutex::new(Vec::<String>::new()));
    let received_clone = received.clone();
    let auth_headers_clone = auth_headers.clone();

    let app = axum::Router::new().route(
        "/api/v4/users/{bot_id}/typing",
        axum::routing::post(
            move |Path(bot_id): Path<String>,
                  headers: HeaderMap,
                  axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                let received = received_clone.clone();
                let auth_headers = auth_headers_clone.clone();
                async move {
                    let auth = headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    auth_headers.lock().await.push(auth);
                    received.lock().await.push(serde_json::json!({
                        "bot_id": bot_id,
                        "body": body,
                    }));
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

    let sender = MattermostSender::new(&format!("http://127.0.0.1:{}", addr.port()), "test-token");
    let indicator = MattermostTypingIndicator::new(
        sender,
        "bot-123".into(),
        "channel-123".into(),
        "root-456".into(),
    );
    let req = indicator_request();

    indicator.start(&req).await.expect("start typing");
    indicator.stop(&req).await.expect("stop typing");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let received = received.lock().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["bot_id"], "bot-123");
    assert_eq!(received[0]["body"]["channel_id"], "channel-123");
    assert_eq!(received[0]["body"]["parent_id"], "root-456");
    assert_eq!(auth_headers.lock().await.as_slice(), ["Bearer test-token"]);

    server.abort();
}
