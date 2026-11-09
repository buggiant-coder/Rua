use rua_channel::core::channel::Channel;
use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use rua_channel::telegram::{TelegramChannel, TelegramConfig};
use tempfile::tempdir;
use tokio::sync::Mutex;

#[derive(Clone)]
struct TelegramMediaFixture {
    get_file_calls: Arc<Mutex<Vec<String>>>,
}

async fn start_telegram_media_fixture(
) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let state = TelegramMediaFixture {
        get_file_calls: Arc::new(Mutex::new(Vec::new())),
    };
    let calls = state.get_file_calls.clone();
    let app = Router::new()
        .route(
            "/bottest-token/getFile",
            get(
                |State(state): State<TelegramMediaFixture>,
                 Query(params): Query<HashMap<String, String>>| async move {
                    let file_id = params.get("file_id").cloned().unwrap_or_default();
                    state.get_file_calls.lock().await.push(file_id.clone());
                    Json(serde_json::json!({
                        "ok": true,
                        "result": {
                            "file_id": file_id,
                            "file_path": "unsafe/photo.jpg"
                        }
                    }))
                },
            ),
        )
        .route(
            "/file/bottest-token/{*path}",
            get(|Path(path): Path<String>| async move {
                if path.ends_with("photo.jpg") {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "image/jpeg")
                        .body(Body::from("jpeg-bytes"))
                        .unwrap()
                } else {
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(Body::empty())
                        .unwrap()
                }
            }),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (format!("http://127.0.0.1:{}", addr.port()), calls, server)
}

fn make_update(chat_id: i64, from_id: i64, text: &str) -> serde_json::Value {
    serde_json::json!({
        "update_id": 12345,
        "message": {
            "message_id": 1,
            "chat": {
                "id": chat_id,
                "type": "private"
            },
            "from": {
                "id": from_id,
                "is_bot": false,
                "first_name": "Test"
            },
            "text": text
        }
    })
}

#[test]
fn parse_basic_text_message() {
    let update = make_update(100, 200, "hello");
    let req = TelegramChannel::parse_update(&update, &[]).unwrap();
    assert_eq!(req.channel_id, "telegram");
    assert_eq!(req.user_id, "200");
    assert_eq!(req.conversation_id, "100");
    assert_eq!(req.session_id, "100");
    assert_eq!(req.message_id, Some("1".into()));
    assert_eq!(req.metadata["chat_id"], "100");
    assert_eq!(req.metadata["chat_type"], "private");
    assert_eq!(req.metadata["is_group"], false);
    assert_eq!(req.metadata["has_bot_command"], false);
    assert_eq!(req.metadata["user_name"], "Test");
    assert_eq!(req.input.len(), 1);
}

#[test]
fn parse_skips_empty_text() {
    let update = make_update(100, 200, "");
    assert!(TelegramChannel::parse_update(&update, &[]).is_none());
}

#[test]
fn parse_skips_non_text_update() {
    let update = serde_json::json!({
        "update_id": 999,
        "edited_message": {
            "message_id": 1,
            "chat": { "id": 100, "type": "private" },
            "from": { "id": 200 },
            "text": "edited"
        }
    });
    assert!(TelegramChannel::parse_update(&update, &[]).is_none());
}

#[test]
fn parse_acl_blocks_unlisted_user() {
    let update = make_update(100, 200, "hello");
    let allowed = vec!["999".to_string()];
    assert!(TelegramChannel::parse_update(&update, &allowed).is_none());
}

#[test]
fn parse_acl_allows_listed_user() {
    let update = make_update(100, 200, "hello");
    let allowed = vec!["200".to_string()];
    let req = TelegramChannel::parse_update(&update, &allowed).unwrap();
    assert_eq!(req.user_id, "200");
}

#[test]
fn parse_group_message() {
    let update = serde_json::json!({
        "update_id": 100,
        "message": {
            "message_id": 5,
            "message_thread_id": 789,
            "chat": {
                "id": -1001234,
                "type": "supergroup",
                "title": "TestGroup"
            },
            "from": {
                "id": 300,
                "is_bot": false,
                "first_name": "Bob"
            },
            "text": "hi group"
        }
    });
    let req = TelegramChannel::parse_update(&update, &[]).unwrap();
    assert_eq!(req.conversation_id, "-1001234");
    assert_eq!(req.metadata["chat_type"], "supergroup");
    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["has_bot_command"], false);
    assert_eq!(req.metadata["user_name"], "Bob");
    assert_eq!(req.metadata["message_thread_id"], 789);
}

#[test]
fn parse_group_bot_command_marks_command_metadata() {
    let update = serde_json::json!({
        "update_id": 100,
        "message": {
            "message_id": 6,
            "chat": {
                "id": -1001234,
                "type": "supergroup"
            },
            "from": {
                "id": 300,
                "is_bot": false,
                "first_name": "Bob"
            },
            "text": "/start",
            "entities": [
                {"type": "bot_command", "offset": 0, "length": 6}
            ]
        }
    });
    let req = TelegramChannel::parse_update(&update, &[]).unwrap();
    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["has_bot_command"], true);
}

#[test]
fn build_request_from_native_group_bot_mention_sets_gate_metadata_and_strips_token() {
    let ch = TelegramChannel::new(TelegramConfig {
        bot_username: "RuaBot".into(),
        ..TelegramConfig::default()
    });
    let update = serde_json::json!({
        "update_id": 101,
        "message": {
            "message_id": 7,
            "chat": {
                "id": -1001234,
                "type": "supergroup"
            },
            "from": {
                "id": 300,
                "is_bot": false,
                "first_name": "Bob"
            },
            "text": "@RuaBot deploy now",
            "entities": [
                {"type": "mention", "offset": 0, "length": 7}
            ]
        }
    });

    let req = ch.build_request_from_native(update).unwrap();

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
fn build_request_from_native_non_bot_mention_preserves_text_and_does_not_trigger_gate() {
    let ch = TelegramChannel::new(TelegramConfig {
        bot_username: "RuaBot".into(),
        ..TelegramConfig::default()
    });
    let update = serde_json::json!({
        "update_id": 102,
        "message": {
            "message_id": 8,
            "chat": {
                "id": -1001234,
                "type": "supergroup"
            },
            "from": {
                "id": 300,
                "is_bot": false,
                "first_name": "Bob"
            },
            "text": "@OtherBot keep this",
            "entities": [
                {"type": "mention", "offset": 0, "length": 9}
            ]
        }
    });

    let req = ch.build_request_from_native(update).unwrap();

    assert_eq!(req.metadata["bot_mentioned"], false);
    assert_eq!(req.metadata["mentioned"], false);
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "@OtherBot keep this");
        }
        other => panic!("expected Text part, got {other:?}"),
    }
}

#[test]
fn build_request_from_native_bot_mention_uses_telegram_utf16_offsets() {
    let ch = TelegramChannel::new(TelegramConfig {
        bot_username: "RuaBot".into(),
        ..TelegramConfig::default()
    });
    let update = serde_json::json!({
        "update_id": 103,
        "message": {
            "message_id": 9,
            "chat": {
                "id": -1001234,
                "type": "supergroup"
            },
            "from": {
                "id": 300,
                "is_bot": false,
                "first_name": "Bob"
            },
            "text": "你好 @RuaBot 发布",
            "entities": [
                {"type": "mention", "offset": 3, "length": 7}
            ]
        }
    });

    let req = ch.build_request_from_native(update).unwrap();

    assert_eq!(req.metadata["bot_mentioned"], true);
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "你好 发布");
        }
        other => panic!("expected Text part, got {other:?}"),
    }
}

#[test]
fn parse_captioned_photo_preserves_caption_and_largest_photo_file_id() {
    let update = serde_json::json!({
        "update_id": 110,
        "message": {
            "message_id": 9,
            "chat": {
                "id": 100,
                "type": "private"
            },
            "from": {
                "id": 200,
                "is_bot": false,
                "first_name": "Alice"
            },
            "caption": "look",
            "caption_entities": [
                {"type": "bot_command", "offset": 0, "length": 5}
            ],
            "photo": [
                {"file_id": "small-photo", "width": 90, "height": 90},
                {"file_id": "large-photo", "width": 1024, "height": 768}
            ]
        }
    });

    let req = TelegramChannel::parse_update(&update, &[]).unwrap();

    assert_eq!(req.metadata["has_bot_command"], true);
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "look\n[photo: large-photo]");
        }
        other => panic!("expected Text part, got {other:?}"),
    }
}

#[test]
fn parse_media_only_documents_and_voice_messages_as_placeholders() {
    let cases = [
        (
            serde_json::json!({"document": {"file_id": "doc-id", "file_name": "report.pdf"}}),
            "[document: report.pdf]",
        ),
        (
            serde_json::json!({"video": {"file_id": "video-id", "file_name": ""}}),
            "[video: video-id]",
        ),
        (
            serde_json::json!({"audio": {"file_id": "audio-id", "file_name": "meeting.ogg"}}),
            "[audio: meeting.ogg]",
        ),
        (
            serde_json::json!({"voice": {"file_id": "voice-id"}}),
            "[voice: voice-id]",
        ),
    ];

    for (media, expected) in cases {
        let mut message = serde_json::json!({
            "message_id": 10,
            "chat": {
                "id": 100,
                "type": "private"
            },
            "from": {
                "id": 200,
                "is_bot": false,
                "first_name": "Alice"
            }
        });
        let message_obj = message.as_object_mut().unwrap();
        for (key, value) in media.as_object().unwrap() {
            message_obj.insert(key.clone(), value.clone());
        }
        let update = serde_json::json!({
            "update_id": 111,
            "message": message
        });

        let req = TelegramChannel::parse_update(&update, &[])
            .unwrap_or_else(|| panic!("telegram {expected} update should normalize"));

        match &req.input[0] {
            rua_channel::core::content::IncomingContentPart::Text { text } => {
                assert_eq!(text, expected);
            }
            other => panic!("expected Text part, got {other:?}"),
        }
    }
}

#[test]
fn parse_downloaded_media_as_typed_parts() {
    let cases = [
        (
            serde_json::json!({
                "photo": [
                    {"file_id": "small-photo", "width": 90, "height": 90},
                    {
                        "file_id": "large-photo",
                        "width": 1024,
                        "height": 768,
                        "local_path": "/tmp/rua-media/photo.jpg",
                        "mime": "image/jpeg"
                    }
                ]
            }),
            "photo",
        ),
        (
            serde_json::json!({
                "document": {
                    "file_id": "doc-id",
                    "file_name": "report.pdf",
                    "local_path": "/tmp/rua-media/report.pdf",
                    "mime": "application/pdf"
                }
            }),
            "document",
        ),
        (
            serde_json::json!({
                "audio": {
                    "file_id": "audio-id",
                    "file_name": "meeting.ogg",
                    "local_path": "/tmp/rua-media/meeting.ogg",
                    "mime": "audio/ogg"
                }
            }),
            "audio",
        ),
        (
            serde_json::json!({
                "video": {
                    "file_id": "video-id",
                    "file_name": "clip.mp4",
                    "local_path": "/tmp/rua-media/clip.mp4",
                    "mime": "video/mp4"
                }
            }),
            "video",
        ),
    ];

    for (media, kind) in cases {
        let mut message = serde_json::json!({
            "message_id": 12,
            "chat": {
                "id": 100,
                "type": "private"
            },
            "from": {
                "id": 200,
                "is_bot": false,
                "first_name": "Alice"
            },
            "caption": "caption"
        });
        let message_obj = message.as_object_mut().unwrap();
        for (key, value) in media.as_object().unwrap() {
            message_obj.insert(key.clone(), value.clone());
        }
        let update = serde_json::json!({
            "update_id": 112,
            "message": message
        });

        let req = TelegramChannel::parse_update(&update, &[])
            .unwrap_or_else(|| panic!("telegram {kind} update should normalize"));

        assert_eq!(req.input.len(), 2);
        match &req.input[0] {
            rua_channel::core::content::IncomingContentPart::Text { text } => {
                assert_eq!(text, "caption");
            }
            other => panic!("expected caption Text part, got {other:?}"),
        }
        match (&req.input[1], kind) {
            (
                rua_channel::core::content::IncomingContentPart::Image {
                    file_id,
                    path,
                    mime,
                },
                "photo",
            ) => {
                assert_eq!(file_id, "large-photo");
                assert_eq!(path.to_string_lossy(), "/tmp/rua-media/photo.jpg");
                assert_eq!(mime.as_deref(), Some("image/jpeg"));
            }
            (
                rua_channel::core::content::IncomingContentPart::File {
                    file_id,
                    path,
                    mime,
                    name,
                },
                "document",
            ) => {
                assert_eq!(file_id, "doc-id");
                assert_eq!(path.to_string_lossy(), "/tmp/rua-media/report.pdf");
                assert_eq!(mime.as_deref(), Some("application/pdf"));
                assert_eq!(name.as_deref(), Some("report.pdf"));
            }
            (
                rua_channel::core::content::IncomingContentPart::Audio {
                    file_id,
                    path,
                    mime,
                    name,
                },
                "audio",
            ) => {
                assert_eq!(file_id, "audio-id");
                assert_eq!(path.to_string_lossy(), "/tmp/rua-media/meeting.ogg");
                assert_eq!(mime.as_deref(), Some("audio/ogg"));
                assert_eq!(name.as_deref(), Some("meeting.ogg"));
            }
            (
                rua_channel::core::content::IncomingContentPart::Video {
                    file_id,
                    path,
                    mime,
                    name,
                },
                "video",
            ) => {
                assert_eq!(file_id, "video-id");
                assert_eq!(path.to_string_lossy(), "/tmp/rua-media/clip.mp4");
                assert_eq!(mime.as_deref(), Some("video/mp4"));
                assert_eq!(name.as_deref(), Some("clip.mp4"));
            }
            (other, _) => panic!("unexpected telegram {kind} media part: {other:?}"),
        }
    }
}

#[tokio::test]
async fn download_inbound_photo_injects_local_path_and_preserves_safe_filename() {
    let temp = tempdir().unwrap();
    let (base_url, calls, server) = start_telegram_media_fixture().await;
    let channel = TelegramChannel::new(TelegramConfig {
        bot_token: "test-token".into(),
        media_dir: temp.path().to_string_lossy().to_string(),
        ..TelegramConfig::default()
    })
    .with_base_url(base_url);
    let mut update = serde_json::json!({
        "update_id": 120,
        "message": {
            "message_id": 12,
            "chat": {"id": 100, "type": "private"},
            "from": {"id": 200, "is_bot": false},
            "caption": "see this",
            "photo": [
                {"file_id": "small-photo", "width": 90, "height": 90},
                {"file_id": "large-photo", "width": 1024, "height": 768}
            ]
        }
    });

    channel
        .download_inbound_media(&mut update)
        .await
        .expect("telegram media download");

    assert_eq!(*calls.lock().await, vec!["large-photo"]);
    let photo = update["message"]["photo"]
        .as_array()
        .and_then(|items| items.last())
        .expect("largest photo");
    let local_path = photo["local_path"].as_str().expect("local path");
    assert!(local_path.starts_with(temp.path().to_string_lossy().as_ref()));
    assert!(local_path.ends_with("photo.jpg"));
    assert!(!local_path.contains("unsafe"));
    assert_eq!(photo["mime"], "image/jpeg");
    assert_eq!(std::fs::read_to_string(local_path).unwrap(), "jpeg-bytes");

    let req = TelegramChannel::parse_update(&update, &[]).expect("typed media request");
    assert_eq!(req.input.len(), 2);
    match &req.input[1] {
        rua_channel::core::content::IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert_eq!(file_id, "large-photo");
            assert_eq!(path.to_string_lossy(), local_path);
            assert_eq!(mime.as_deref(), Some("image/jpeg"));
        }
        other => panic!("expected downloaded image part, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn download_inbound_media_failure_keeps_placeholder_payload() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/bottest-token/getFile",
        get(|| async {
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"ok": false, "description": "failed"})),
            )
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let temp = tempdir().unwrap();
    let channel = TelegramChannel::new(TelegramConfig {
        bot_token: "test-token".into(),
        media_dir: temp.path().to_string_lossy().to_string(),
        ..TelegramConfig::default()
    })
    .with_base_url(format!("http://127.0.0.1:{}", addr.port()));
    let mut update = serde_json::json!({
        "update_id": 121,
        "message": {
            "message_id": 13,
            "chat": {"id": 100, "type": "private"},
            "from": {"id": 200, "is_bot": false},
            "document": {"file_id": "doc-id", "file_name": "report.pdf"}
        }
    });

    channel
        .download_inbound_media(&mut update)
        .await
        .expect("download failure should not abort fallback parsing");
    assert!(update["message"]["document"]["local_path"].is_null());

    let req = TelegramChannel::parse_update(&update, &[]).expect("fallback request");
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "[document: report.pdf]");
        }
        other => panic!("expected fallback text, got {other:?}"),
    }

    server.abort();
}

#[test]
fn parse_tool_guard_callback_approve() {
    let update = serde_json::json!({
        "update_id": 101,
        "callback_query": {
            "id": "cb-1",
            "from": {
                "id": 300,
                "is_bot": false,
                "username": "alice"
            },
            "message": {
                "message_id": 7,
                "message_thread_id": 456,
                "chat": {"id": 100, "type": "private"}
            },
            "data": "tga:req-123"
        }
    });
    let req = TelegramChannel::parse_update(&update, &[]).unwrap();
    assert_eq!(req.user_id, "300");
    assert_eq!(req.conversation_id, "100");
    assert_eq!(req.metadata["telegram_callback_data"], "tga:req-123");
    assert_eq!(req.metadata["telegram_callback_query_id"], "cb-1");
    assert_eq!(req.metadata["approval_request_id"], "req-123");
    assert_eq!(req.metadata["approval_action"], "approve");
    assert_eq!(req.metadata["from_card_action"], true);
    assert_eq!(req.metadata["user_name"], "alice");
    assert_eq!(req.metadata["is_group"], false);
    assert_eq!(req.metadata["message_thread_id"], 456);
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "/approval approve req-123");
        }
        _ => panic!("expected Text part"),
    }
}

#[test]
fn parse_tool_guard_callback_deny() {
    let update = serde_json::json!({
        "update_id": 102,
        "callback_query": {
            "id": "cb-2",
            "from": {"id": 300, "is_bot": false, "first_name": "Alice"},
            "message": {
                "message_id": 8,
                "chat": {"id": 100, "type": "private"}
            },
            "data": "tgd:req-123"
        }
    });
    let req = TelegramChannel::parse_update(&update, &[]).unwrap();
    assert_eq!(req.metadata["approval_request_id"], "req-123");
    assert_eq!(req.metadata["approval_action"], "deny");
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "/approval deny req-123");
        }
        _ => panic!("expected Text part"),
    }
}

#[test]
fn build_request_from_native_rejects_duplicate_tool_guard_callback() {
    let channel = TelegramChannel::new(TelegramConfig {
        bot_token: "token".into(),
        chat_id: "100".into(),
        allowed_users: vec![],
        bot_username: String::new(),
        media_dir: String::new(),
    });
    let update = serde_json::json!({
        "update_id": 103,
        "callback_query": {
            "id": "cb-dup",
            "from": {"id": 300, "is_bot": false, "first_name": "Alice"},
            "message": {
                "message_id": 8,
                "chat": {"id": 100, "type": "private"}
            },
            "data": "tga:req-dup"
        }
    });

    channel
        .build_request_from_native(update.clone())
        .expect("first callback should parse");
    let err = channel
        .build_request_from_native(update)
        .expect_err("duplicate callback should be rejected");
    assert!(
        err.to_string().contains("duplicate approval callback"),
        "unexpected duplicate error: {err}"
    );
}

#[test]
fn build_tool_guard_approval_message_body_has_inline_keyboard() {
    let body = TelegramChannel::build_send_message_body("100", "风险操作", Some("req-123"));

    assert_eq!(body["chat_id"], "100");
    assert_eq!(
        body["reply_markup"]["inline_keyboard"][0][0]["callback_data"],
        "tga:req-123"
    );
    assert_eq!(
        body["reply_markup"]["inline_keyboard"][0][1]["callback_data"],
        "tgd:req-123"
    );
}

#[test]
fn build_request_from_native() {
    let ch = TelegramChannel::new(TelegramConfig::default());
    let update = make_update(100, 200, "test");
    let req = ch.build_request_from_native(update).unwrap();
    assert_eq!(req.channel_id, "telegram");
    assert_eq!(req.user_id, "200");
}

#[test]
fn build_request_from_native_rejects_duplicate_message_update() {
    let ch = TelegramChannel::new(TelegramConfig::default());
    let update = make_update(100, 200, "test");

    ch.build_request_from_native(update.clone())
        .expect("first update should parse");
    let err = ch
        .build_request_from_native(update)
        .expect_err("duplicate update should be rejected");

    assert!(
        err.to_string().contains("duplicate telegram event"),
        "unexpected duplicate error: {err}"
    );
}

#[test]
fn build_request_from_native_allows_distinct_message_updates() {
    let ch = TelegramChannel::new(TelegramConfig::default());
    let first = make_update(100, 200, "first");
    let mut second = make_update(100, 200, "second");
    second["update_id"] = serde_json::json!(12346);

    ch.build_request_from_native(first)
        .expect("first update should parse");
    ch.build_request_from_native(second)
        .expect("distinct update should parse");
}

#[test]
fn build_request_from_native_fails_on_no_message() {
    let ch = TelegramChannel::new(TelegramConfig::default());
    let result = ch.build_request_from_native(serde_json::json!({"update_id": 1}));
    assert!(result.is_err());
}
