use async_trait::async_trait;
use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::routing::get;
use base64::Engine;
use rua_channel::core::channel::{AgentRequest, Channel, Process};
use rua_channel::core::content::{
    ChannelTarget, IncomingContentPart, OutgoingContentPart, OutgoingMessage,
};
use rua_channel::core::streaming::{
    PlatformStreamPolicy, ProcessingIndicator, StreamHandler, StreamType,
};
use rua_channel::wecom::channel::{WecomProcessingIndicator, WecomStreamHandler};
use rua_channel::wecom::client::{
    build_upload_chunk_frames, build_upload_finish_frame, build_upload_init_frame,
    build_wecom_media_message, create_ws_sender, spawn_ws_task_with_sender, UPLOAD_CHUNK_SIZE,
};
use rua_channel::wecom::types::{cmd, WecomBotConfig, WsFrame, WsHeaders};
use rua_channel::wecom::WecomChannel;
use tempfile::tempdir;
use tokio::sync::{mpsc, watch};

struct NoopProcess;

#[async_trait]
impl Process for NoopProcess {
    async fn handle(
        &self,
        _req: AgentRequest,
    ) -> futures::stream::BoxStream<'static, rua_core::RuntimeEvent> {
        Box::pin(futures::stream::empty())
    }
}

fn wecom() -> WecomChannel {
    WecomChannel::new("bot1".into(), "sec1".into())
}

fn native_text_frame(
    req_id: &str,
    sender_id: &str,
    chat_id: &str,
    chat_type: &str,
    msg_id: &str,
    content: &str,
) -> serde_json::Value {
    serde_json::json!({
        "cmd": "aibot_msg_callback",
        "headers": {"req_id": req_id},
        "body": {
            "msgtype": "text",
            "from": {"userid": sender_id, "name": "Test User"},
            "chatid": chat_id,
            "chattype": chat_type,
            "msgid": msg_id,
            "text": {"content": content}
        }
    })
}

#[test]
fn builds_single_chat_request_from_native_text_frame() {
    let req = wecom()
        .build_request_from_native(native_text_frame(
            "test_req_1",
            "user123",
            "",
            "single",
            "msg001",
            "hello bot",
        ))
        .expect("single chat text frame should parse");

    assert_eq!(req.channel_id, "wecom");
    assert_eq!(req.user_id, "user123");
    assert_eq!(req.session_id, "wecom:user123");
    assert_eq!(req.message_id.as_deref(), Some("msg001"));
    assert_eq!(req.metadata["wecom_req_id"], "test_req_1");
    assert_eq!(req.metadata["user_name"], "Test User");
    assert_eq!(
        req.metadata["wecom_frame"]["headers"]["req_id"],
        "test_req_1"
    );
    assert_eq!(
        req.metadata["wecom_frame"]["body"]["text"]["content"],
        "hello bot"
    );
    assert_eq!(req.input.len(), 1);
    assert!(matches!(
        &req.input[0],
        IncomingContentPart::Text { text } if text == "hello bot"
    ));
}

#[test]
fn builds_group_chat_request_with_group_session_and_blank_user_id() {
    let req = wecom()
        .build_request_from_native(native_text_frame(
            "test_req_2",
            "user456",
            "chat789",
            "group",
            "msg002",
            "hi group",
        ))
        .expect("group chat text frame should parse");

    assert_eq!(req.session_id, "wecom:group:chat789");
    assert_eq!(req.user_id, "");
    assert_eq!(req.conversation_id, "chat789");
    assert_eq!(req.metadata["wecom_sender_id"], "user456");
    assert_eq!(req.metadata["is_group"], true);
}

#[test]
fn group_bot_mention_sets_gate_metadata_and_strips_token() {
    let mut native = native_text_frame(
        "test_req_mention",
        "user456",
        "chat789",
        "group",
        "msg-mention",
        "@bot1 deploy now",
    );
    native["body"]["mentions"] = serde_json::json!([{ "userid": "bot1" }]);
    native["body"]["mentioned_list"] = serde_json::json!(["bot1"]);

    let req = wecom()
        .build_request_from_native(native)
        .expect("group bot mention should parse");

    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["bot_mentioned"], true);
    assert_eq!(req.metadata["mentioned"], true);
    assert!(matches!(
        &req.input[0],
        IncomingContentPart::Text { text } if text == "deploy now"
    ));
}

#[test]
fn group_non_bot_mention_preserves_text_without_gate_metadata() {
    let mut native = native_text_frame(
        "test_req_other_mention",
        "user456",
        "chat789",
        "group",
        "msg-other-mention",
        "@other_bot keep this",
    );
    native["body"]["mentions"] = serde_json::json!([{ "userid": "other_bot" }]);
    native["body"]["mentioned_list"] = serde_json::json!(["other_bot"]);

    let req = wecom()
        .build_request_from_native(native)
        .expect("group non-bot mention should parse");

    assert_eq!(req.metadata["bot_mentioned"], false);
    assert_eq!(req.metadata["mentioned"], false);
    assert!(matches!(
        &req.input[0],
        IncomingContentPart::Text { text } if text == "@other_bot keep this"
    ));
}

#[test]
fn rejects_duplicate_text_message_id() {
    let channel = wecom();
    let native = native_text_frame("test_req_dup", "user123", "", "single", "msg-dup", "hello");

    channel
        .build_request_from_native(native.clone())
        .expect("first message should parse");
    let err = channel
        .build_request_from_native(native)
        .expect_err("duplicate message should be rejected");

    assert!(
        err.to_string().contains("duplicate wecom event"),
        "unexpected duplicate error: {err}"
    );
}

#[test]
fn rejects_empty_text_frames() {
    let err = wecom()
        .build_request_from_native(native_text_frame(
            "test_req_3",
            "u1",
            "",
            "single",
            "m1",
            "  ",
        ))
        .expect_err("empty text should be rejected");

    assert!(err
        .to_string()
        .contains("wecom callback frame has no parseable content"));
}

#[test]
fn builds_request_from_voice_transcript() {
    let native = serde_json::json!({
        "cmd": "aibot_msg_callback",
        "headers": {"req_id": "test_req_4"},
        "body": {
            "msgtype": "voice",
            "from": {"userid": "u2"},
            "chatid": "",
            "chattype": "single",
            "msgid": "msg003",
            "voice": {"content": "transcribed text"}
        }
    });

    let req = wecom()
        .build_request_from_native(native)
        .expect("voice transcript should parse");

    assert!(matches!(
        &req.input[0],
        IncomingContentPart::Text { text } if text == "transcribed text"
    ));
}

#[test]
fn builds_request_from_image_and_file_media_placeholders() {
    let cases = [
        (
            serde_json::json!({
                "msgtype": "image",
                "image": {"url": "https://example.com/image.jpg"}
            }),
            "[image: https://example.com/image.jpg]",
        ),
        (
            serde_json::json!({
                "msgtype": "file",
                "file": {
                    "url": "https://example.com/file.pdf",
                    "filename": "document.pdf"
                }
            }),
            "[file: document.pdf]",
        ),
        (
            serde_json::json!({
                "msgtype": "video",
                "video": {"media_id": "video-media-id"}
            }),
            "[video: video-media-id]",
        ),
    ];

    for (media_body, expected) in cases {
        let mut body = serde_json::json!({
            "from": {"userid": "u2"},
            "chatid": "",
            "chattype": "single",
            "msgid": "msg-media"
        });
        let body_obj = body.as_object_mut().unwrap();
        for (key, value) in media_body.as_object().unwrap() {
            body_obj.insert(key.clone(), value.clone());
        }

        let native = serde_json::json!({
            "cmd": "aibot_msg_callback",
            "headers": {"req_id": "test_req_media"},
            "body": body
        });

        let req = wecom()
            .build_request_from_native(native)
            .unwrap_or_else(|err| panic!("{expected} should parse: {err}"));

        assert!(matches!(
            &req.input[0],
            IncomingContentPart::Text { text } if text == expected
        ));
    }
}

#[test]
fn builds_request_from_media_missing_provider_detail_hint() {
    let native = serde_json::json!({
        "cmd": "aibot_msg_callback",
        "headers": {"req_id": "test_req_missing_media"},
        "body": {
            "msgtype": "image",
            "from": {"userid": "u2"},
            "chatid": "",
            "chattype": "single",
            "msgid": "msg-media",
            "image": {}
        }
    });

    let req = wecom()
        .build_request_from_native(native)
        .expect("missing media detail should still produce diagnostic input");

    assert!(matches!(
        &req.input[0],
        IncomingContentPart::Text { text } if text == "[image: missing media]"
    ));
}

#[tokio::test]
async fn downloads_image_url_before_native_normalization() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom media fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let app = axum::Router::new().route(
        "/image.jpg",
        axum::routing::get(|| async {
            axum::response::Response::builder()
                .status(axum::http::StatusCode::OK)
                .header("content-type", "image/jpeg")
                .body(axum::body::Body::from("jpeg-bytes"))
                .unwrap()
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });
    let temp = tempdir().unwrap();
    let channel = wecom().with_media_dir(temp.path().to_string_lossy().to_string());
    let mut native = serde_json::json!({
        "cmd": "aibot_msg_callback",
        "headers": {"req_id": "test_req_media"},
        "body": {
            "msgtype": "image",
            "from": {"userid": "u2"},
            "chatid": "",
            "chattype": "single",
            "msgid": "msg-media",
            "image": {"url": format!("http://127.0.0.1:{}/image.jpg", addr.port())}
        }
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("wecom media download");

    let local_path = native["body"]["image"]["local_path"]
        .as_str()
        .expect("local path")
        .to_string();
    assert!(local_path.starts_with(temp.path().to_string_lossy().as_ref()));
    assert_eq!(native["body"]["image"]["mime"], "image/jpeg");
    assert_eq!(std::fs::read_to_string(&local_path).unwrap(), "jpeg-bytes");

    let req = channel
        .build_request_from_native(native)
        .expect("typed wecom media request");
    match &req.input[0] {
        IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert!(file_id.ends_with("/image.jpg"));
            assert_eq!(path.to_string_lossy(), local_path);
            assert_eq!(mime.as_deref(), Some("image/jpeg"));
        }
        other => panic!("expected image part, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn downloads_file_and_video_urls_before_native_normalization() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom media fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let app = axum::Router::new()
        .route(
            "/report.pdf",
            axum::routing::get(|| async {
                axum::response::Response::builder()
                    .status(axum::http::StatusCode::OK)
                    .header("content-type", "application/pdf")
                    .body(axum::body::Body::from("pdf-bytes"))
                    .unwrap()
            }),
        )
        .route(
            "/video.mp4",
            axum::routing::get(|| async {
                axum::response::Response::builder()
                    .status(axum::http::StatusCode::OK)
                    .header("content-type", "video/mp4")
                    .body(axum::body::Body::from("video-bytes"))
                    .unwrap()
            }),
        );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });
    let temp = tempdir().unwrap();
    let channel = wecom().with_media_dir(temp.path().to_string_lossy().to_string());

    let cases = [
        (
            "file",
            "file",
            serde_json::json!({
                "url": format!("http://127.0.0.1:{}/report.pdf", addr.port()),
                "filename": "report.pdf"
            }),
            "application/pdf",
            "pdf-bytes",
        ),
        (
            "video",
            "video",
            serde_json::json!({
                "url": format!("http://127.0.0.1:{}/video.mp4", addr.port())
            }),
            "video/mp4",
            "video-bytes",
        ),
    ];

    for (msgtype, body_key, media_body, expected_mime, expected_bytes) in cases {
        let mut body = serde_json::json!({
            "msgtype": msgtype,
            "from": {"userid": "u2"},
            "chatid": "",
            "chattype": "single",
            "msgid": format!("msg-{msgtype}")
        });
        body.as_object_mut()
            .unwrap()
            .insert(body_key.to_string(), media_body);
        let mut native = serde_json::json!({
            "cmd": "aibot_msg_callback",
            "headers": {"req_id": format!("test_req_{msgtype}")},
            "body": body
        });

        channel
            .download_inbound_media(&mut native)
            .await
            .expect("wecom media download");

        let local_path = native["body"][body_key]["local_path"]
            .as_str()
            .expect("local path")
            .to_string();
        assert!(local_path.starts_with(temp.path().to_string_lossy().as_ref()));
        assert_eq!(native["body"][body_key]["mime"], expected_mime);
        assert_eq!(
            std::fs::read_to_string(&local_path).unwrap(),
            expected_bytes
        );

        let req = channel
            .build_request_from_native(native)
            .expect("typed wecom media request");
        match (msgtype, &req.input[0]) {
            (
                "file",
                IncomingContentPart::File {
                    path, mime, name, ..
                },
            ) => {
                assert_eq!(path.to_string_lossy(), local_path);
                assert_eq!(mime.as_deref(), Some("application/pdf"));
                assert_eq!(name.as_deref(), Some("report.pdf"));
            }
            (
                "video",
                IncomingContentPart::Video {
                    path, mime, name, ..
                },
            ) => {
                assert_eq!(path.to_string_lossy(), local_path);
                assert_eq!(mime.as_deref(), Some("video/mp4"));
                assert_eq!(name.as_deref(), Some("video.mp4"));
            }
            (_, other) => panic!("expected {msgtype} part, got {other:?}"),
        }
    }

    server.abort();
}

#[tokio::test]
async fn downloads_and_decrypts_aes_image_url_before_native_normalization() {
    let encrypted = hex_bytes("249bb7a9409d089d3667fccbdc3d7dca");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom encrypted media fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let app = axum::Router::new().route(
        "/encrypted.jpg",
        axum::routing::get(move || {
            let encrypted = encrypted.clone();
            async move {
                axum::response::Response::builder()
                    .status(axum::http::StatusCode::OK)
                    .header("content-type", "image/jpeg")
                    .body(axum::body::Body::from(encrypted))
                    .unwrap()
            }
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });
    let temp = tempdir().unwrap();
    let channel = wecom().with_media_dir(temp.path().to_string_lossy().to_string());
    let mut native = serde_json::json!({
        "cmd": "aibot_msg_callback",
        "headers": {"req_id": "test_req_media"},
        "body": {
            "msgtype": "image",
            "from": {"userid": "u2"},
            "chatid": "",
            "chattype": "single",
            "msgid": "msg-media",
            "image": {
                "url": format!("http://127.0.0.1:{}/encrypted.jpg", addr.port()),
                "aeskey": "00112233445566778899aabbccddeeff"
            }
        }
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("wecom encrypted media download");

    let local_path = native["body"]["image"]["local_path"]
        .as_str()
        .expect("local path");
    assert_eq!(std::fs::read_to_string(local_path).unwrap(), "jpeg-bytes");

    server.abort();
}

#[tokio::test]
async fn download_failure_keeps_media_placeholder() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom failure fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let app = axum::Router::new().route(
        "/image.jpg",
        axum::routing::get(|| async { axum::http::StatusCode::BAD_GATEWAY }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });
    let temp = tempdir().unwrap();
    let channel = wecom().with_media_dir(temp.path().to_string_lossy().to_string());
    let url = format!("http://127.0.0.1:{}/image.jpg", addr.port());
    let mut native = serde_json::json!({
        "cmd": "aibot_msg_callback",
        "headers": {"req_id": "test_req_media"},
        "body": {
            "msgtype": "image",
            "from": {"userid": "u2"},
            "chatid": "",
            "chattype": "single",
            "msgid": "msg-media",
            "image": {"url": url}
        }
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("download failure should preserve fallback parsing");

    let req = channel
        .build_request_from_native(native)
        .expect("fallback wecom media request");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert!(text.starts_with("[image: http://")),
        other => panic!("expected fallback text, got {other:?}"),
    }

    server.abort();
}

fn hex_bytes(raw: &str) -> Vec<u8> {
    (0..raw.len())
        .step_by(2)
        .map(|idx| u8::from_str_radix(&raw[idx..idx + 2], 16).unwrap())
        .collect()
}

#[test]
fn local_media_paths_are_typed_parts_for_file_and_video() {
    let cases = [
        (
            serde_json::json!({
                "msgtype": "file",
                "file": {
                    "url": "https://example.com/file.pdf",
                    "local_path": "/tmp/report.pdf",
                    "mime": "application/pdf",
                    "filename": "report.pdf"
                }
            }),
            "file",
        ),
        (
            serde_json::json!({
                "msgtype": "video",
                "video": {
                    "url": "https://example.com/video.mp4",
                    "local_path": "/tmp/video.mp4",
                    "mime": "video/mp4",
                    "filename": "video.mp4"
                }
            }),
            "video",
        ),
    ];

    for (media_body, expected_kind) in cases {
        let mut body = serde_json::json!({
            "from": {"userid": "u2"},
            "chatid": "",
            "chattype": "single",
            "msgid": "msg-media"
        });
        let body_obj = body.as_object_mut().unwrap();
        for (key, value) in media_body.as_object().unwrap() {
            body_obj.insert(key.clone(), value.clone());
        }

        let req = wecom()
            .build_request_from_native(serde_json::json!({
                "cmd": "aibot_msg_callback",
                "headers": {"req_id": "test_req_media"},
                "body": body
            }))
            .expect("local media should parse");

        match (expected_kind, &req.input[0]) {
            (
                "file",
                IncomingContentPart::File {
                    file_id,
                    path,
                    mime,
                    name,
                },
            ) => {
                assert_eq!(file_id, "https://example.com/file.pdf");
                assert_eq!(path.to_string_lossy(), "/tmp/report.pdf");
                assert_eq!(mime.as_deref(), Some("application/pdf"));
                assert_eq!(name.as_deref(), Some("report.pdf"));
            }
            (
                "video",
                IncomingContentPart::Video {
                    file_id,
                    path,
                    mime,
                    name,
                },
            ) => {
                assert_eq!(file_id, "https://example.com/video.mp4");
                assert_eq!(path.to_string_lossy(), "/tmp/video.mp4");
                assert_eq!(mime.as_deref(), Some("video/mp4"));
                assert_eq!(name.as_deref(), Some("video.mp4"));
            }
            (_, other) => panic!("expected {expected_kind} part, got {other:?}"),
        }
    }
}

#[test]
fn rejects_malformed_native_frame() {
    let err = wecom()
        .build_request_from_native(serde_json::json!({"cmd": 123}))
        .expect_err("malformed frame should be rejected");

    assert!(err.to_string().contains("invalid wecom WS frame"));
}

#[test]
fn builds_qwenpaw_compatible_upload_frames_for_local_media() {
    let mut data = vec![b'a'; UPLOAD_CHUNK_SIZE];
    data.extend_from_slice(b"tail");

    let init = build_upload_init_frame("image", "chart.png", &data);
    assert_eq!(init.cmd.as_deref(), Some(cmd::UPLOAD_INIT));
    let init_body = init.body.as_ref().expect("init body");
    assert_eq!(init_body["type"], "image");
    assert_eq!(init_body["filename"], "chart.png");
    assert_eq!(init_body["total_size"], UPLOAD_CHUNK_SIZE + 4);
    assert_eq!(init_body["total_chunks"], 2);
    assert_eq!(
        init_body["md5"].as_str().unwrap().len(),
        32,
        "WeCom upload init must carry raw-data MD5 like QwenPaw"
    );

    let chunks = build_upload_chunk_frames("upload-123", &data);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].cmd.as_deref(), Some(cmd::UPLOAD_CHUNK));
    assert_eq!(chunks[0].body.as_ref().unwrap()["upload_id"], "upload-123");
    assert_eq!(chunks[0].body.as_ref().unwrap()["chunk_index"], 0);
    assert_eq!(chunks[1].body.as_ref().unwrap()["chunk_index"], 1);
    assert_eq!(
        chunks[1].body.as_ref().unwrap()["base64_data"],
        base64::engine::general_purpose::STANDARD.encode(b"tail")
    );

    let finish = build_upload_finish_frame("upload-123");
    assert_eq!(finish.cmd.as_deref(), Some(cmd::UPLOAD_FINISH));
    assert_eq!(finish.body.as_ref().unwrap()["upload_id"], "upload-123");
}

#[test]
fn builds_wecom_media_message_payload_with_media_id() {
    assert_eq!(
        build_wecom_media_message("image", "media-1"),
        serde_json::json!({
            "msgtype": "image",
            "image": { "media_id": "media-1" }
        })
    );
    assert_eq!(
        build_wecom_media_message("voice", "media-2"),
        serde_json::json!({
            "msgtype": "voice",
            "voice": { "media_id": "media-2" }
        })
    );
    assert_eq!(
        build_wecom_media_message("unknown", "media-3"),
        serde_json::json!({
            "msgtype": "file",
            "file": { "media_id": "media-3" }
        })
    );
}

#[tokio::test]
async fn ws_sender_uploads_media_and_sends_media_message() {
    let (captured_tx, mut captured_rx) = mpsc::unbounded_channel::<WsFrame>();
    let captured_for_route = captured_tx.clone();
    let app = axum::Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let captured = captured_for_route.clone();
            async move { ws.on_upgrade(move |socket| wecom_ws_fixture(socket, captured)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom ws fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });

    let (sender, outbound_rx) = create_ws_sender();
    let (_stop_tx, stop_rx) = watch::channel(false);
    let handle = spawn_ws_task_with_sender(
        WecomBotConfig {
            bot_id: "bot1".into(),
            secret: "sec1".into(),
            ws_url: format!("ws://127.0.0.1:{}/ws", addr.port()),
        },
        std::sync::Arc::new(|_| {}),
        stop_rx,
        outbound_rx,
    );
    let temp = tempdir().unwrap();
    let image_path = temp.path().join("chart.png");
    std::fs::write(&image_path, "png-bytes").unwrap();

    let media_id = sender
        .upload_media(&image_path, "image", std::time::Duration::from_secs(2))
        .await
        .expect("upload media");
    assert_eq!(media_id, "media-123");
    sender
        .send_message("chat-1", build_wecom_media_message("image", &media_id))
        .await
        .expect("send media message");

    let auth = recv_frame(&mut captured_rx).await;
    assert_eq!(auth.cmd.as_deref(), Some(cmd::SUBSCRIBE));
    let init = recv_frame(&mut captured_rx).await;
    assert_eq!(init.cmd.as_deref(), Some(cmd::UPLOAD_INIT));
    assert_eq!(init.body.as_ref().unwrap()["type"], "image");
    assert_eq!(init.body.as_ref().unwrap()["filename"], "chart.png");
    let chunk = recv_frame(&mut captured_rx).await;
    assert_eq!(chunk.cmd.as_deref(), Some(cmd::UPLOAD_CHUNK));
    assert_eq!(
        chunk.body.as_ref().unwrap()["base64_data"],
        base64::engine::general_purpose::STANDARD.encode(b"png-bytes")
    );
    let finish = recv_frame(&mut captured_rx).await;
    assert_eq!(finish.cmd.as_deref(), Some(cmd::UPLOAD_FINISH));
    let send_msg = recv_frame(&mut captured_rx).await;
    assert_eq!(send_msg.cmd.as_deref(), Some(cmd::SEND_MSG));
    assert_eq!(send_msg.body.as_ref().unwrap()["chatid"], "chat-1");
    assert_eq!(
        send_msg.body.as_ref().unwrap()["msg"],
        serde_json::json!({
            "msgtype": "image",
            "image": { "media_id": "media-123" }
        })
    );

    handle.abort();
    server.abort();
}

#[tokio::test]
async fn channel_send_uses_active_wecom_ws_sender_for_markdown_text() {
    let (captured_tx, mut captured_rx) = mpsc::unbounded_channel::<WsFrame>();
    let captured_for_route = captured_tx.clone();
    let app = axum::Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let captured = captured_for_route.clone();
            async move { ws.on_upgrade(move |socket| wecom_ws_fixture(socket, captured)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom ws fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });

    let channel = wecom().with_ws_url(format!("ws://127.0.0.1:{}/ws", addr.port()));
    channel
        .start(std::sync::Arc::new(NoopProcess))
        .await
        .expect("start wecom channel");
    let target = ChannelTarget {
        agent_id: "agent".into(),
        conversation_id: "wecom:group:chat-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({ "wecom_chatid": "chat-1" }),
    };
    let msg = OutgoingMessage {
        channel_id: "wecom".into(),
        conversation_id: "chat-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "hello wecom".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(target, msg).await.expect("send wecom text");

    let auth = recv_frame(&mut captured_rx).await;
    assert_eq!(auth.cmd.as_deref(), Some(cmd::SUBSCRIBE));
    let send_msg = recv_frame(&mut captured_rx).await;
    assert_eq!(send_msg.cmd.as_deref(), Some(cmd::SEND_MSG));
    assert_eq!(send_msg.body.as_ref().unwrap()["chatid"], "chat-1");
    assert_eq!(
        send_msg.body.as_ref().unwrap()["msg"],
        serde_json::json!({
            "msgtype": "markdown",
            "markdown": { "content": "hello wecom" }
        })
    );

    channel.stop().await.expect("stop wecom channel");
    server.abort();
}

#[tokio::test]
async fn channel_send_tool_guard_approval_replies_with_template_card() {
    let (captured_tx, mut captured_rx) = mpsc::unbounded_channel::<WsFrame>();
    let captured_for_route = captured_tx.clone();
    let app = axum::Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let captured = captured_for_route.clone();
            async move { ws.on_upgrade(move |socket| wecom_ws_fixture(socket, captured)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom ws fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });

    let source_frame = WsFrame {
        cmd: Some(cmd::CALLBACK.into()),
        headers: WsHeaders {
            req_id: "req-approval-1".into(),
        },
        body: Some(serde_json::json!({
            "msgtype": "text",
            "from": {"userid": "user-1", "name": "Alice"},
            "chatid": "chat-1",
            "chattype": "group",
            "msgid": "msg-1",
            "text": {"content": "run tool"}
        })),
        ..Default::default()
    };
    let channel = wecom().with_ws_url(format!("ws://127.0.0.1:{}/ws", addr.port()));
    channel
        .start(std::sync::Arc::new(NoopProcess))
        .await
        .expect("start wecom channel");
    let target = ChannelTarget {
        agent_id: "agent".into(),
        conversation_id: "wecom:group:chat-1".into(),
        sender_id: Some("user-1".into()),
        reply_to: Some("msg-1".into()),
        metadata: serde_json::json!({
            "wecom_chatid": "chat-1",
            "wecom_frame": source_frame,
        }),
    };
    let msg = OutgoingMessage {
        channel_id: "wecom".into(),
        conversation_id: "chat-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Markdown {
            text: "Approve Bash?".into(),
        }],
        reply_to: Some("msg-1".into()),
        metadata: serde_json::json!({
            "message_type": "tool_guard_approval",
            "approval_request_id": "req-123",
            "tool_name": "Bash",
            "severity": "high",
        }),
    };

    channel
        .send(target, msg)
        .await
        .expect("send wecom approval card");

    let auth = recv_frame(&mut captured_rx).await;
    assert_eq!(auth.cmd.as_deref(), Some(cmd::SUBSCRIBE));
    let response = recv_frame(&mut captured_rx).await;
    assert_eq!(response.cmd.as_deref(), Some(cmd::RESPONSE));
    assert_eq!(response.headers.req_id, "req-approval-1");
    let body = response.body.as_ref().unwrap();
    assert_eq!(body["msgtype"], "template_card");
    let card = &body["template_card"];
    assert_eq!(card["card_type"], "button_interaction");
    assert_eq!(card["task_id"], "tg_approval_req-123");
    assert_eq!(card["main_title"]["desc"], "Bash | high");
    let approve_key: serde_json::Value =
        serde_json::from_str(card["button_list"][0]["key"].as_str().unwrap()).unwrap();
    assert_eq!(approve_key["a"], "approve");
    assert_eq!(approve_key["rid"], "req-123");
    assert_eq!(approve_key["tool"], "Bash");
    assert_eq!(approve_key["sev"], "high");
    assert_eq!(approve_key["session_id"], "wecom:group:chat-1");
    assert_eq!(approve_key["sender_id"], "user-1");

    channel.stop().await.expect("stop wecom channel");
    server.abort();
}

#[tokio::test]
async fn template_card_callback_updates_card_to_resolved_state() {
    let (captured_tx, mut captured_rx) = mpsc::unbounded_channel::<WsFrame>();
    let captured_for_route = captured_tx.clone();
    let app = axum::Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let captured = captured_for_route.clone();
            async move {
                ws.on_upgrade(move |socket| {
                    wecom_ws_fixture_with_initial_frame(
                        socket,
                        captured,
                        template_card_event_frame("event-resolve-1", "req-resolve"),
                    )
                })
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom ws fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });

    let channel = wecom().with_ws_url(format!("ws://127.0.0.1:{}/ws", addr.port()));
    channel
        .start(std::sync::Arc::new(NoopProcess))
        .await
        .expect("start wecom channel");

    let auth = recv_frame(&mut captured_rx).await;
    assert_eq!(auth.cmd.as_deref(), Some(cmd::SUBSCRIBE));
    let update = recv_frame(&mut captured_rx).await;
    assert_eq!(update.cmd.as_deref(), Some(cmd::RESPONSE));
    assert_eq!(update.headers.req_id, "event-resolve-1");
    let body = update.body.as_ref().unwrap();
    assert_eq!(body["msgtype"], "template_card");
    let card = &body["template_card"];
    assert_eq!(card["card_type"], "text_notice");
    assert_eq!(card["task_id"], "tg_approval_req-resolve");
    assert_eq!(card["main_title"]["title"], "✅ Approved");
    assert_eq!(card["main_title"]["desc"], "by manager-1\nBash");

    channel.stop().await.expect("stop wecom channel");
    server.abort();
}

#[test]
fn builds_approval_command_from_template_card_event() {
    let key = serde_json::json!({
        "a": "approve",
        "rid": "req-123",
        "tool": "Bash",
        "sev": "high",
        "session_id": "wecom:group:chat-1",
        "sender_id": "user-1"
    })
    .to_string();
    let req = wecom()
        .build_request_from_native(serde_json::json!({
            "cmd": "aibot_event_callback",
            "headers": {"req_id": "event-1"},
            "body": {
                "msgtype": "template_card_event",
                "event": {
                    "template_card_event": {
                        "task_id": "tg_approval_req-123",
                        "event_key": key
                    }
                },
                "from": {"userid": "manager-1"},
                "chatid": "chat-1",
                "chattype": "group"
            }
        }))
        .expect("template card event should parse");

    assert_eq!(req.channel_id, "wecom");
    assert_eq!(req.session_id, "wecom:group:chat-1");
    assert_eq!(req.user_id, "");
    assert_eq!(req.conversation_id, "chat-1");
    assert_eq!(req.metadata["approval_request_id"], "req-123");
    assert_eq!(req.metadata["approval_action"], "approve");
    assert_eq!(req.metadata["approval_task_id"], "tg_approval_req-123");
    assert_eq!(req.metadata["from_card_action"], true);
    assert!(matches!(
        &req.input[0],
        IncomingContentPart::Text { text } if text == "/approval approve req-123"
    ));
}

fn template_card_event_frame(req_id: &str, request_id: &str) -> WsFrame {
    let key = serde_json::json!({
        "a": "approve",
        "rid": request_id,
        "tool": "Bash",
        "sev": "high",
        "session_id": "wecom:group:chat-1",
        "sender_id": "user-1"
    })
    .to_string();
    WsFrame {
        cmd: Some(cmd::EVENT_CALLBACK.into()),
        headers: WsHeaders {
            req_id: req_id.into(),
        },
        body: Some(serde_json::json!({
            "msgtype": "template_card_event",
            "event": {
                "template_card_event": {
                    "task_id": format!("tg_approval_{request_id}"),
                    "event_key": key
                }
            },
            "from": {"userid": "manager-1"},
            "chatid": "chat-1",
            "chattype": "group"
        })),
        ..Default::default()
    }
}

#[test]
fn rejects_duplicate_template_card_approval_event() {
    let key = serde_json::json!({
        "a": "approve",
        "rid": "req-dup",
        "tool": "Bash",
        "sev": "high",
        "session_id": "wecom:group:chat-1",
        "sender_id": "user-1"
    })
    .to_string();
    let native = serde_json::json!({
        "cmd": "aibot_event_callback",
        "headers": {"req_id": "event-dup"},
        "body": {
            "msgtype": "template_card_event",
            "event": {
                "template_card_event": {
                    "task_id": "tg_approval_req-dup",
                    "event_key": key
                }
            },
            "from": {"userid": "manager-1"},
            "chatid": "chat-1",
            "chattype": "group"
        }
    });
    let channel = wecom();

    channel
        .build_request_from_native(native.clone())
        .expect("first template card event should parse");
    let err = channel
        .build_request_from_native(native)
        .expect_err("duplicate template card event should be rejected");
    assert!(
        err.to_string().contains("duplicate approval callback"),
        "unexpected duplicate error: {err}"
    );
}

#[tokio::test]
async fn stream_handler_sends_wecom_reply_stream_frames() {
    let (captured_tx, mut captured_rx) = mpsc::unbounded_channel::<WsFrame>();
    let captured_for_route = captured_tx.clone();
    let app = axum::Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let captured = captured_for_route.clone();
            async move { ws.on_upgrade(move |socket| wecom_ws_fixture(socket, captured)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom ws fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });

    let (sender, outbound_rx) = create_ws_sender();
    let (_stop_tx, stop_rx) = watch::channel(false);
    let handle = spawn_ws_task_with_sender(
        WecomBotConfig {
            bot_id: "bot1".into(),
            secret: "sec1".into(),
            ws_url: format!("ws://127.0.0.1:{}/ws", addr.port()),
        },
        std::sync::Arc::new(|_| {}),
        stop_rx,
        outbound_rx,
    );
    let source = WsFrame {
        cmd: Some(cmd::CALLBACK.into()),
        headers: WsHeaders {
            req_id: "req-stream-1".into(),
        },
        body: Some(serde_json::json!({
            "msgtype": "text",
            "text": {"content": "hello"},
        })),
        ..Default::default()
    };
    let handler = WecomStreamHandler::new(
        sender,
        source,
        PlatformStreamPolicy::reply_stream(std::time::Duration::ZERO, 3000),
    );

    let auth = recv_frame(&mut captured_rx).await;
    assert_eq!(auth.cmd.as_deref(), Some(cmd::SUBSCRIBE));

    handler
        .on_delta(StreamType::Message, "hello")
        .await
        .expect("send stream delta");
    handler
        .on_delta(StreamType::Message, "hello there")
        .await
        .expect("send stream update");
    handler
        .on_end(StreamType::Message, "hello world")
        .await
        .expect("send stream final");

    let delta = recv_frame(&mut captured_rx).await;
    assert_eq!(delta.cmd.as_deref(), Some(cmd::RESPONSE));
    assert_eq!(delta.headers.req_id, "req-stream-1");
    assert_eq!(
        delta.body.as_ref().unwrap(),
        &serde_json::json!({
            "msgtype": "stream",
            "stream": {
                "id": "rua-req-stream-1",
                "content": "hello",
                "finish": false
            }
        })
    );

    let update = recv_frame(&mut captured_rx).await;
    assert_eq!(update.cmd.as_deref(), Some(cmd::RESPONSE));
    assert_eq!(update.headers.req_id, "req-stream-1");
    assert_eq!(
        update.body.as_ref().unwrap(),
        &serde_json::json!({
            "msgtype": "stream",
            "stream": {
                "id": "rua-req-stream-1",
                "content": "hello there",
                "finish": false
            }
        })
    );

    let final_frame = recv_frame(&mut captured_rx).await;
    assert_eq!(final_frame.cmd.as_deref(), Some(cmd::RESPONSE));
    assert_eq!(final_frame.headers.req_id, "req-stream-1");
    assert_eq!(
        final_frame.body.as_ref().unwrap(),
        &serde_json::json!({
            "msgtype": "stream",
            "stream": {
                "id": "rua-req-stream-1",
                "content": "hello world",
                "finish": true
            }
        })
    );

    handle.abort();
    server.abort();
}

#[tokio::test]
async fn processing_indicator_reuses_stream_id_and_does_not_override_streamed_final() {
    let (captured_tx, mut captured_rx) = mpsc::unbounded_channel::<WsFrame>();
    let captured_for_route = captured_tx.clone();
    let app = axum::Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let captured = captured_for_route.clone();
            async move { ws.on_upgrade(move |socket| wecom_ws_fixture(socket, captured)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom ws fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });

    let (sender, outbound_rx) = create_ws_sender();
    let (_stop_tx, stop_rx) = watch::channel(false);
    let handle = spawn_ws_task_with_sender(
        WecomBotConfig {
            bot_id: "bot1".into(),
            secret: "sec1".into(),
            ws_url: format!("ws://127.0.0.1:{}/ws", addr.port()),
        },
        std::sync::Arc::new(|_| {}),
        stop_rx,
        outbound_rx,
    );
    let source = WsFrame {
        cmd: Some(cmd::CALLBACK.into()),
        headers: WsHeaders {
            req_id: "req-processing-1".into(),
        },
        body: Some(serde_json::json!({
            "msgtype": "text",
            "text": {"content": "hello"},
        })),
        ..Default::default()
    };
    let stream_id = "rua-req-processing-1".to_string();
    let consumed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let indicator = WecomProcessingIndicator::new(
        sender.clone(),
        source.clone(),
        stream_id.clone(),
        consumed.clone(),
    );
    let handler = WecomStreamHandler::with_stream_id(
        sender,
        source,
        PlatformStreamPolicy::reply_stream(std::time::Duration::ZERO, 3000),
        stream_id.clone(),
        Some(consumed),
    );
    let req = wecom()
        .build_request_from_native(native_text_frame(
            "req-processing-1",
            "user123",
            "",
            "single",
            "msg001",
            "hello",
        ))
        .expect("request");

    let auth = recv_frame(&mut captured_rx).await;
    assert_eq!(auth.cmd.as_deref(), Some(cmd::SUBSCRIBE));

    indicator.start(&req).await.expect("start processing");
    handler
        .on_delta(StreamType::Message, "final text")
        .await
        .expect("stream final text");
    handler
        .on_end(StreamType::Message, "final text")
        .await
        .expect("finish stream");
    indicator.stop(&req).await.expect("stop processing");

    let processing = recv_frame(&mut captured_rx).await;
    assert_eq!(processing.cmd.as_deref(), Some(cmd::RESPONSE));
    assert_eq!(processing.body.as_ref().unwrap()["stream"]["id"], stream_id);
    assert_eq!(
        processing.body.as_ref().unwrap()["stream"]["content"],
        "🤔 Thinking..."
    );
    assert_eq!(processing.body.as_ref().unwrap()["stream"]["finish"], false);

    let delta = recv_frame(&mut captured_rx).await;
    assert_eq!(delta.body.as_ref().unwrap()["stream"]["id"], stream_id);
    assert_eq!(
        delta.body.as_ref().unwrap()["stream"]["content"],
        "final text"
    );
    assert_eq!(delta.body.as_ref().unwrap()["stream"]["finish"], false);

    let final_frame = recv_frame(&mut captured_rx).await;
    assert_eq!(
        final_frame.body.as_ref().unwrap()["stream"]["id"],
        stream_id
    );
    assert_eq!(
        final_frame.body.as_ref().unwrap()["stream"]["content"],
        "final text"
    );
    assert_eq!(final_frame.body.as_ref().unwrap()["stream"]["finish"], true);

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    while let Ok(frame) = captured_rx.try_recv() {
        assert_eq!(frame.cmd.as_deref(), Some(cmd::HEARTBEAT));
    }

    handle.abort();
    server.abort();
}

#[tokio::test]
async fn channel_send_uploads_local_image_and_sends_media_message() {
    let (captured_tx, mut captured_rx) = mpsc::unbounded_channel::<WsFrame>();
    let captured_for_route = captured_tx.clone();
    let app = axum::Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let captured = captured_for_route.clone();
            async move { ws.on_upgrade(move |socket| wecom_ws_fixture(socket, captured)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom ws fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });
    let temp = tempdir().unwrap();
    let image_path = temp.path().join("chart.png");
    std::fs::write(&image_path, "png-bytes").unwrap();

    let channel = wecom().with_ws_url(format!("ws://127.0.0.1:{}/ws", addr.port()));
    channel
        .start(std::sync::Arc::new(NoopProcess))
        .await
        .expect("start wecom channel");
    let target = ChannelTarget {
        agent_id: "agent".into(),
        conversation_id: "wecom:group:chat-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({ "wecom_chatid": "chat-1" }),
    };
    let msg = OutgoingMessage {
        channel_id: "wecom".into(),
        conversation_id: "chat-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Image {
            url: None,
            path: Some(image_path),
            alt: Some("chart.png".into()),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(target, msg).await.expect("send wecom image");

    let auth = recv_frame(&mut captured_rx).await;
    assert_eq!(auth.cmd.as_deref(), Some(cmd::SUBSCRIBE));
    let init = recv_frame(&mut captured_rx).await;
    assert_eq!(init.cmd.as_deref(), Some(cmd::UPLOAD_INIT));
    assert_eq!(init.body.as_ref().unwrap()["type"], "image");
    let chunk = recv_frame(&mut captured_rx).await;
    assert_eq!(chunk.cmd.as_deref(), Some(cmd::UPLOAD_CHUNK));
    assert_eq!(
        chunk.body.as_ref().unwrap()["base64_data"],
        base64::engine::general_purpose::STANDARD.encode(b"png-bytes")
    );
    let finish = recv_frame(&mut captured_rx).await;
    assert_eq!(finish.cmd.as_deref(), Some(cmd::UPLOAD_FINISH));
    let send_msg = recv_frame(&mut captured_rx).await;
    assert_eq!(send_msg.cmd.as_deref(), Some(cmd::SEND_MSG));
    assert_eq!(
        send_msg.body.as_ref().unwrap()["msg"],
        serde_json::json!({
            "msgtype": "image",
            "image": { "media_id": "media-123" }
        })
    );

    channel.stop().await.expect("stop wecom channel");
    server.abort();
}

#[tokio::test]
async fn channel_send_uploads_local_file_voice_and_video_media_messages() {
    let (captured_tx, mut captured_rx) = mpsc::unbounded_channel::<WsFrame>();
    let captured_for_route = captured_tx.clone();
    let app = axum::Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let captured = captured_for_route.clone();
            async move { ws.on_upgrade(move |socket| wecom_ws_fixture(socket, captured)) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind wecom ws fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });
    let temp = tempdir().unwrap();
    let file_path = temp.path().join("report.pdf");
    let voice_path = temp.path().join("voice.amr");
    let video_path = temp.path().join("clip.mp4");
    std::fs::write(&file_path, "pdf-bytes").unwrap();
    std::fs::write(&voice_path, "voice-bytes").unwrap();
    std::fs::write(&video_path, "video-bytes").unwrap();

    let channel = wecom().with_ws_url(format!("ws://127.0.0.1:{}/ws", addr.port()));
    channel
        .start(std::sync::Arc::new(NoopProcess))
        .await
        .expect("start wecom channel");
    let target = ChannelTarget {
        agent_id: "agent".into(),
        conversation_id: "wecom:group:chat-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({ "wecom_chatid": "chat-1" }),
    };
    let msg = OutgoingMessage {
        channel_id: "wecom".into(),
        conversation_id: "chat-1".into(),
        receiver_id: None,
        content: vec![
            OutgoingContentPart::File {
                url: None,
                path: Some(file_path),
                name: Some("report.pdf".into()),
            },
            OutgoingContentPart::Audio {
                url: None,
                path: Some(voice_path),
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

    channel.send(target, msg).await.expect("send wecom media");

    let auth = recv_frame(&mut captured_rx).await;
    assert_eq!(auth.cmd.as_deref(), Some(cmd::SUBSCRIBE));
    for (expected_upload_type, expected_msg) in [
        (
            "file",
            serde_json::json!({
                "msgtype": "file",
                "file": { "media_id": "media-123" }
            }),
        ),
        (
            "voice",
            serde_json::json!({
                "msgtype": "voice",
                "voice": { "media_id": "media-123" }
            }),
        ),
        (
            "video",
            serde_json::json!({
                "msgtype": "video",
                "video": { "media_id": "media-123" }
            }),
        ),
    ] {
        let init = recv_frame(&mut captured_rx).await;
        assert_eq!(init.cmd.as_deref(), Some(cmd::UPLOAD_INIT));
        assert_eq!(init.body.as_ref().unwrap()["type"], expected_upload_type);
        let chunk = recv_frame(&mut captured_rx).await;
        assert_eq!(chunk.cmd.as_deref(), Some(cmd::UPLOAD_CHUNK));
        assert!(chunk.body.as_ref().unwrap()["base64_data"]
            .as_str()
            .is_some_and(|value| !value.is_empty()));
        let finish = recv_frame(&mut captured_rx).await;
        assert_eq!(finish.cmd.as_deref(), Some(cmd::UPLOAD_FINISH));
        let send_msg = recv_frame(&mut captured_rx).await;
        assert_eq!(send_msg.cmd.as_deref(), Some(cmd::SEND_MSG));
        assert_eq!(send_msg.body.as_ref().unwrap()["msg"], expected_msg);
    }

    channel.stop().await.expect("stop wecom channel");
    server.abort();
}

async fn recv_frame(rx: &mut mpsc::UnboundedReceiver<WsFrame>) -> WsFrame {
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("frame timeout")
            .expect("frame");
        if frame.cmd.as_deref() != Some(cmd::HEARTBEAT) {
            return frame;
        }
    }
}

async fn wecom_ws_fixture(mut socket: WebSocket, captured: mpsc::UnboundedSender<WsFrame>) {
    while let Some(Ok(message)) = socket.recv().await {
        let AxumMessage::Text(text) = message else {
            continue;
        };
        let frame: WsFrame = serde_json::from_str(&text).expect("ws frame");
        let req_id = frame.headers.req_id.clone();
        let cmd = frame.cmd.clone().unwrap_or_default();
        captured.send(frame).expect("capture frame");

        let ack = match cmd.as_str() {
            cmd::SUBSCRIBE => serde_json::json!({
                "headers": { "req_id": req_id },
                "errcode": 0
            }),
            cmd::UPLOAD_INIT => serde_json::json!({
                "headers": { "req_id": req_id },
                "errcode": 0,
                "body": { "upload_id": "upload-123" }
            }),
            cmd::UPLOAD_CHUNK => serde_json::json!({
                "headers": { "req_id": req_id },
                "errcode": 0,
                "body": {}
            }),
            cmd::UPLOAD_FINISH => serde_json::json!({
                "headers": { "req_id": req_id },
                "errcode": 0,
                "body": { "media_id": "media-123" }
            }),
            _ => continue,
        };
        socket
            .send(AxumMessage::Text(ack.to_string().into()))
            .await
            .expect("send ack");
    }
}

async fn wecom_ws_fixture_with_initial_frame(
    mut socket: WebSocket,
    captured: mpsc::UnboundedSender<WsFrame>,
    initial_frame: WsFrame,
) {
    let mut sent_initial = false;
    while let Some(Ok(message)) = socket.recv().await {
        let AxumMessage::Text(text) = message else {
            continue;
        };
        let frame: WsFrame = serde_json::from_str(&text).expect("ws frame");
        let req_id = frame.headers.req_id.clone();
        let cmd_value = frame.cmd.clone().unwrap_or_default();
        captured.send(frame).expect("capture frame");

        if cmd_value.as_str() == cmd::SUBSCRIBE {
            let ack = serde_json::json!({
                "headers": { "req_id": req_id },
                "errcode": 0
            });
            socket
                .send(AxumMessage::Text(ack.to_string().into()))
                .await
                .expect("send ack");
            if !sent_initial {
                sent_initial = true;
                socket
                    .send(AxumMessage::Text(
                        serde_json::to_string(&initial_frame)
                            .expect("serialize initial frame")
                            .into(),
                    ))
                    .await
                    .expect("send initial callback");
            }
        }
    }
}
