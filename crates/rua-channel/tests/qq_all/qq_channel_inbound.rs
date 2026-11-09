use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::get;
use rua_channel::core::channel::Channel;
use rua_channel::core::content::IncomingContentPart;
use rua_channel::qq::QqChannel;
use tempfile::tempdir;

async fn start_qq_attachment_fixture() -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().route(
        "/{filename}",
        get(
            |axum::extract::Path(filename): axum::extract::Path<String>| async move {
                let (mime, body) = match filename.as_str() {
                    "file.jpg" => ("image/jpeg", "jpg-bytes"),
                    "report.pdf" => ("application/pdf", "pdf-bytes"),
                    "voice.mp3" => ("audio/mpeg", "audio-bytes"),
                    "clip.mp4" => ("video/mp4", "video-bytes"),
                    other => panic!("unexpected QQ attachment fixture: {other}"),
                };
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, mime)
                    .body(Body::from(body))
                    .unwrap()
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
fn channel_id_is_qq() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);
    assert_eq!(channel.id(), "qq");
}

#[test]
fn build_request_from_native_c2c() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let native = serde_json::json!({
        "sender_id": "user_openid_abc",
        "content": "hello from qq",
        "msg_id": "msg-001",
        "message_type": "c2c",
        "user_name": "Alice",
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.channel_id, "qq");
    assert_eq!(req.user_id, "user_openid_abc");
    assert_eq!(req.conversation_id, "user_openid_abc");
    assert_eq!(req.message_id, Some("msg-001".to_string()));
    assert_eq!(req.input.len(), 1);

    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "hello from qq");
        }
        _ => panic!("expected Text part"),
    }

    assert_eq!(
        req.metadata.get("message_type").and_then(|v| v.as_str()),
        Some("c2c")
    );
    assert_eq!(req.metadata["is_group"], false);
    assert_eq!(req.metadata["is_dm"], true);
    assert_eq!(
        req.metadata.get("user_name").and_then(|v| v.as_str()),
        Some("Alice")
    );
}

#[test]
fn build_request_from_native_c2c_resolves_sender_from_author_fallback_keys() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let primary = channel
        .build_request_from_native(serde_json::json!({
            "message_type": "c2c",
            "author": {
                "user_openid": "openid-primary",
                "id": "fallback-id",
                "username": "Primary User"
            },
            "content": "hi",
            "id": "msg-primary"
        }))
        .expect("primary author request");
    assert_eq!(primary.user_id, "openid-primary");
    assert_eq!(primary.session_id, "openid-primary");
    assert_eq!(primary.metadata["sender_id"], "openid-primary");
    assert_eq!(primary.metadata["user_name"], "Primary User");

    let fallback = channel
        .build_request_from_native(serde_json::json!({
            "message_type": "c2c",
            "author": {
                "id": "fallback-id"
            },
            "content": "hi"
        }))
        .expect("fallback author request");
    assert_eq!(
        fallback.user_id, "fallback-id",
        "QwenPaw's QQ sender fallback signal maps to Rust's native adapter: c2c author.id remains a stable fallback when user_openid is absent"
    );
    assert_eq!(fallback.session_id, "fallback-id");
    assert_eq!(fallback.metadata["sender_id"], "fallback-id");
}

#[test]
fn build_request_from_native_empty_text() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let native = serde_json::json!({
        "sender_id": "user1",
        "content": "",
        "message_type": "c2c",
    });

    let req = channel.build_request_from_native(native).unwrap();
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "");
        }
        _ => panic!("expected Text part"),
    }
}

#[test]
fn build_request_from_native_empty_msg_id() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let native = serde_json::json!({
        "sender_id": "user1",
        "content": "hi",
        "msg_id": "",
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.message_id, None);
}

#[test]
fn build_request_from_native_rejects_duplicate_message_id() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let native = serde_json::json!({
        "sender_id": "user1",
        "content": "hi",
        "msg_id": "msg-dup",
        "message_type": "c2c",
    });

    channel
        .build_request_from_native(native.clone())
        .expect("first message should parse");
    let err = channel
        .build_request_from_native(native)
        .expect_err("duplicate message should be rejected");

    assert!(
        err.to_string().contains("duplicate qq event"),
        "unexpected duplicate error: {err}"
    );
}

#[test]
fn build_request_from_native_group_preserves_group_route_metadata() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let native = serde_json::json!({
        "message_type": "group",
        "author": {
            "member_openid": "member_1",
            "id": "fallback_id",
            "nick": "Group User"
        },
        "content": "hi group",
        "id": "msg_003",
        "group_openid": "grp_300"
    });

    let req = channel.build_request_from_native(native).unwrap();

    assert_eq!(req.user_id, "member_1");
    assert_eq!(req.conversation_id, "group:grp_300");
    assert_eq!(req.session_id, "group:grp_300");
    assert_eq!(req.message_id, Some("msg_003".to_string()));
    assert_eq!(req.metadata["message_type"], "group");
    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["is_dm"], false);
    assert_eq!(req.metadata["group_openid"], "grp_300");
    assert_eq!(req.metadata["user_name"], "Group User");
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "hi group");
        }
        _ => panic!("expected Text part"),
    }
}

#[test]
fn build_request_from_native_group_bot_mention_sets_gate_metadata_and_strips_token() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let native = serde_json::json!({
        "message_type": "group",
        "author": {
            "member_openid": "member_1",
            "id": "fallback_id",
            "nick": "Group User"
        },
        "content": "<@app123> deploy now",
        "id": "msg_004",
        "group_openid": "grp_300",
        "mentions": [
            {"id": "app123"}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();

    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["bot_mentioned"], true);
    assert_eq!(req.metadata["mentioned"], true);
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "deploy now");
        }
        other => panic!("expected Text part, got {other:?}"),
    }
}

#[test]
fn build_request_from_native_group_non_bot_mention_preserves_text() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let native = serde_json::json!({
        "message_type": "group",
        "author": {
            "member_openid": "member_1",
            "id": "fallback_id",
            "nick": "Group User"
        },
        "content": "<@other_bot> keep this",
        "id": "msg_005",
        "group_openid": "grp_300",
        "mentions": [
            {"id": "other_bot"}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();

    assert_eq!(req.metadata["bot_mentioned"], false);
    assert_eq!(req.metadata["mentioned"], false);
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "<@other_bot> keep this");
        }
        other => panic!("expected Text part, got {other:?}"),
    }
}

#[test]
fn build_request_from_native_guild_preserves_channel_and_guild_metadata() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let native = serde_json::json!({
        "message_type": "guild",
        "author": {
            "id": "user_100",
            "username": "Guild User"
        },
        "content": "hello guild",
        "id": "msg_002",
        "channel_id": "ch_100",
        "guild_id": "g_200"
    });

    let req = channel.build_request_from_native(native).unwrap();

    assert_eq!(req.user_id, "user_100");
    assert_eq!(req.conversation_id, "channel:ch_100");
    assert_eq!(req.session_id, "channel:ch_100");
    assert_eq!(req.metadata["message_type"], "guild");
    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["is_dm"], false);
    assert_eq!(req.metadata["channel_id"], "ch_100");
    assert_eq!(req.metadata["guild_id"], "g_200");
    assert_eq!(req.metadata["user_name"], "Guild User");
}

#[test]
fn build_request_from_native_interaction_create_normalizes_approval_command() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);
    let button_data = serde_json::json!({
        "p": "tg_",
        "a": "approve",
        "rid": "req-123",
        "tool": "Bash",
        "sev": "high",
        "sid": "group:grp_300",
        "sender": "member_1",
        "mt": "group",
        "goid": "grp_300",
        "mid": "msg_003"
    })
    .to_string();

    let req = channel
        .build_request_from_native(serde_json::json!({
            "event_type": "INTERACTION_CREATE",
            "id": "interaction-1",
            "group_member_openid": "operator_1",
            "data": {
                "resolved": {
                    "button_data": button_data
                }
            }
        }))
        .expect("approval interaction");

    assert_eq!(req.channel_id, "qq");
    assert_eq!(req.user_id, "member_1");
    assert_eq!(req.session_id, "group:grp_300");
    assert_eq!(req.conversation_id, "group:grp_300");
    assert_eq!(req.metadata["approval_request_id"], "req-123");
    assert_eq!(req.metadata["approval_action"], "approve");
    assert_eq!(req.metadata["approval_interaction_id"], "interaction-1");
    assert_eq!(req.metadata["approval_operator_id"], "operator_1");
    assert_eq!(req.metadata["from_card_action"], true);
    assert_eq!(req.metadata["message_type"], "group");
    assert_eq!(req.metadata["group_openid"], "grp_300");
    assert!(matches!(
        &req.input[0],
        IncomingContentPart::Text { text } if text == "/approval approve req-123"
    ));
}

#[test]
fn build_request_from_native_interaction_create_rejects_duplicate_approval() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);
    let button_data = serde_json::json!({
        "p": "tg_",
        "a": "approve",
        "rid": "req-dup",
        "tool": "Bash",
        "sev": "high",
        "sid": "user_1",
        "sender": "user_1",
        "mt": "c2c"
    })
    .to_string();
    let native = serde_json::json!({
        "event_type": "INTERACTION_CREATE",
        "id": "interaction-dup-1",
        "user_openid": "operator_1",
        "data": {
            "resolved": {
                "button_data": button_data
            }
        }
    });

    channel
        .build_request_from_native(native.clone())
        .expect("first approval interaction should pass");
    let err = channel
        .build_request_from_native(native)
        .expect_err("duplicate approval interaction should be rejected");
    assert!(
        err.to_string().contains("duplicate approval callback"),
        "unexpected duplicate error: {err}"
    );
}

#[test]
fn build_request_from_native_appends_attachment_urls() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let native = serde_json::json!({
        "sender_id": "user1",
        "content": "see attachment",
        "message_type": "c2c",
        "attachments": [
            {
                "url": "https://example.com/file.jpg",
                "filename": "file.jpg"
            },
            {
                "url": "",
                "filename": "empty.jpg"
            }
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();

    assert_eq!(req.input.len(), 2);
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "see attachment");
        }
        _ => panic!("expected Text part"),
    }
    match &req.input[1] {
        rua_channel::core::content::IncomingContentPart::Url { url, title } => {
            assert_eq!(url, "https://example.com/file.jpg");
            assert_eq!(title.as_deref(), Some("file.jpg"));
        }
        _ => panic!("expected Url part"),
    }
}

#[test]
fn build_request_from_native_attachment_only_omits_empty_text_part() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let native = serde_json::json!({
        "sender_id": "user1",
        "content": "",
        "message_type": "c2c",
        "attachments": [
            {
                "url": "https://example.com/file.jpg",
                "filename": "file.jpg"
            }
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();

    assert_eq!(req.input.len(), 1);
    assert!(matches!(
        &req.input[0],
        rua_channel::core::content::IncomingContentPart::Url { .. }
    ));
}

#[tokio::test]
async fn download_inbound_attachment_injects_local_path_for_typed_image() {
    let temp = tempdir().unwrap();
    let (base_url, server) = start_qq_attachment_fixture().await;
    let channel = QqChannel::new("app123".into(), "secret".into(), false)
        .with_media_dir(temp.path().to_string_lossy().to_string());
    let mut native = serde_json::json!({
        "sender_id": "user1",
        "content": "see attachment",
        "message_type": "c2c",
        "attachments": [
            {
                "url": format!("{base_url}/file.jpg"),
                "filename": "file.jpg",
                "content_type": "image/jpeg"
            }
        ]
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("qq attachment download");

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.input.len(), 2);
    match &req.input[1] {
        IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert_eq!(file_id, "file.jpg");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("image/jpeg"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "jpg-bytes");
        }
        other => panic!("expected image part, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn download_inbound_attachment_injects_local_paths_for_file_audio_and_video() {
    let temp = tempdir().unwrap();
    let (base_url, server) = start_qq_attachment_fixture().await;
    let channel = QqChannel::new("app123".into(), "secret".into(), false)
        .with_media_dir(temp.path().to_string_lossy().to_string());
    let mut native = serde_json::json!({
        "sender_id": "user1",
        "content": "",
        "message_type": "c2c",
        "attachments": [
            {
                "url": format!("{base_url}/report.pdf"),
                "filename": "report.pdf",
                "content_type": "application/pdf"
            },
            {
                "url": format!("{base_url}/voice.mp3"),
                "filename": "voice.mp3",
                "content_type": "audio/mpeg"
            },
            {
                "url": format!("{base_url}/clip.mp4"),
                "filename": "clip.mp4",
                "content_type": "video/mp4"
            }
        ]
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("qq attachment download");

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.input.len(), 3);
    match &req.input[0] {
        IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "report.pdf");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("application/pdf"));
            assert_eq!(name.as_deref(), Some("report.pdf"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "pdf-bytes");
        }
        other => panic!("expected file part, got {other:?}"),
    }
    match &req.input[1] {
        IncomingContentPart::Audio {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "voice.mp3");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("audio/mpeg"));
            assert_eq!(name.as_deref(), Some("voice.mp3"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "audio-bytes");
        }
        other => panic!("expected audio part, got {other:?}"),
    }
    match &req.input[2] {
        IncomingContentPart::Video {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "clip.mp4");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("video/mp4"));
            assert_eq!(name.as_deref(), Some("clip.mp4"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "video-bytes");
        }
        other => panic!("expected video part, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn download_inbound_attachment_failure_keeps_url_fallback() {
    let app = axum::Router::new().route("/missing.jpg", get(|| async { StatusCode::BAD_GATEWAY }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let temp = tempdir().unwrap();
    let channel = QqChannel::new("app123".into(), "secret".into(), false)
        .with_media_dir(temp.path().to_string_lossy().to_string());
    let mut native = serde_json::json!({
        "sender_id": "user1",
        "content": "",
        "message_type": "c2c",
        "attachments": [
            {
                "url": format!("{base_url}/missing.jpg"),
                "filename": "missing.jpg"
            }
        ]
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("download failure should not abort fallback parsing");

    let req = channel.build_request_from_native(native).unwrap();
    match &req.input[0] {
        IncomingContentPart::Url { url, title } => {
            assert!(url.ends_with("/missing.jpg"));
            assert_eq!(title.as_deref(), Some("missing.jpg"));
        }
        other => panic!("expected URL fallback, got {other:?}"),
    }

    server.abort();
}
