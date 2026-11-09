use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use futures::stream::BoxStream;
use futures::StreamExt;
use rua_channel::core::channel::{AgentRequest, Channel, Process};
use rua_channel::core::content::IncomingContentPart;
use rua_channel::feishu::{FeishuChannel, FeishuConfig};
use rua_core::RuntimeEvent;
use std::collections::HashMap;
use tempfile::tempdir;
use tokio::sync::Mutex;

struct MockProcess {
    requests: Arc<Mutex<Vec<AgentRequest>>>,
}

#[async_trait]
impl Process for MockProcess {
    async fn handle(&self, req: AgentRequest) -> BoxStream<'static, RuntimeEvent> {
        self.requests.lock().await.push(req);
        Box::pin(futures::stream::empty())
    }
}

fn sample_feishu_event() -> serde_json::Value {
    serde_json::json!({
        "header": {
            "event_id": "ev-001",
            "event_type": "im.message.receive_v1",
            "create_time": "1700000000000",
            "app_id": "cli_test"
        },
        "event": {
            "sender": {
                "sender_id": {
                    "open_id": "ou_test_user",
                    "user_id": "uid_test",
                    "union_id": "on_test"
                },
                "sender_type": "user"
            },
            "message": {
                "message_id": "om_test_001",
                "chat_id": "oc_test_chat",
                "chat_type": "p2p",
                "message_type": "text",
                "content": "{\"text\":\"hello agent\"}"
            }
        }
    })
}

async fn start_feishu_media_fixture() -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().route(
        "/open-apis/im/v1/messages/{message_id}/resources/{file_key}",
        get(
            |Path((message_id, file_key)): Path<(String, String)>,
             Query(params): Query<HashMap<String, String>>| async move {
                if message_id.starts_with("om_test_") {
                    let resource_type = params.get("type").map(String::as_str);
                    let fixture = match (file_key.as_str(), resource_type) {
                        ("img_xxx", Some("image")) => Some(("image/png", "png-bytes")),
                        ("file_xxx", Some("file")) => Some(("application/pdf", "pdf-bytes")),
                        ("audio_xxx", Some("file")) => Some(("audio/mpeg", "audio-bytes")),
                        ("media_xxx", Some("file")) => Some(("video/mp4", "video-bytes")),
                        _ => None,
                    };
                    if let Some((mime, body)) = fixture {
                        return Response::builder()
                            .status(StatusCode::OK)
                            .header(header::CONTENT_TYPE, mime)
                            .body(Body::from(body))
                            .unwrap();
                    }
                }
                Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::empty())
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
fn build_request_from_native_p2p_text() {
    let ch = FeishuChannel::new(FeishuConfig::default());
    let req = ch.build_request_from_native(sample_feishu_event()).unwrap();

    assert_eq!(req.channel_id, "feishu");
    assert_eq!(req.user_id, "ou_test_user");
    assert_eq!(req.conversation_id, "oc_test_chat");
    assert_eq!(req.message_id, Some("om_test_001".into()));
    assert_eq!(req.input.len(), 1);
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "hello agent"),
        _ => panic!("expected Text content"),
    }
    assert_eq!(req.metadata["feishu_chat_id"], "oc_test_chat");
    assert_eq!(req.metadata["feishu_chat_type"], "p2p");
    assert_eq!(req.metadata["feishu_sender_id"], "ou_test_user");
    assert_eq!(req.metadata["sender_id"], "ou_test_user");
    assert_eq!(req.metadata["is_group"], false);
    assert_eq!(req.metadata["chat_type"], "p2p");
    assert_eq!(req.metadata["feishu_receive_id_type"], "open_id");
}

#[test]
fn build_request_from_native_rejects_duplicate_message_event() {
    let ch = FeishuChannel::new(FeishuConfig::default());
    let event = sample_feishu_event();

    ch.build_request_from_native(event.clone())
        .expect("first event should parse");
    let err = ch
        .build_request_from_native(event)
        .expect_err("duplicate event should be rejected");

    assert!(
        err.to_string().contains("duplicate feishu event"),
        "unexpected duplicate error: {err}"
    );
}

#[test]
fn build_request_from_native_group() {
    let mut event = sample_feishu_event();
    event["event"]["message"]["chat_type"] = serde_json::json!("group");
    event["event"]["message"]["chat_id"] = serde_json::json!("oc_group_123");

    let ch = FeishuChannel::new(FeishuConfig::default());
    let req = ch.build_request_from_native(event).unwrap();

    assert_eq!(req.conversation_id, "oc_group_123");
    assert_eq!(req.metadata["feishu_chat_type"], "group");
    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["chat_type"], "group");
    assert_eq!(req.metadata["feishu_receive_id"], "oc_group_123");
    assert_eq!(req.metadata["feishu_receive_id_type"], "chat_id");
}

#[test]
fn build_request_from_native_group_bot_mention_sets_gate_metadata_and_strips_key() {
    let mut event = sample_feishu_event();
    event["event"]["message"]["chat_type"] = serde_json::json!("group");
    event["event"]["message"]["chat_id"] = serde_json::json!("oc_group_123");
    event["event"]["message"]["content"] =
        serde_json::json!("{\"text\":\"<at id=bot_open_id>Bot</at> deploy now\"}");
    event["event"]["message"]["mentions"] = serde_json::json!([
        {
            "key": "<at id=bot_open_id>Bot</at>",
            "id": {"open_id": "ou_bot"}
        }
    ]);

    let ch = FeishuChannel::new(FeishuConfig {
        bot_open_id: Some("ou_bot".into()),
        ..Default::default()
    });
    let req = ch.build_request_from_native(event).unwrap();

    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["bot_mentioned"], true);
    assert_eq!(req.metadata["mentioned"], true);
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "deploy now"),
        _ => panic!("expected Text content"),
    }
}

#[test]
fn build_request_from_native_group_without_bot_mention_leaves_gate_metadata_absent() {
    let mut event = sample_feishu_event();
    event["event"]["message"]["chat_type"] = serde_json::json!("group");
    event["event"]["message"]["chat_id"] = serde_json::json!("oc_group_123");
    event["event"]["message"]["content"] =
        serde_json::json!("{\"text\":\"<at id=someone>Someone</at> deploy now\"}");
    event["event"]["message"]["mentions"] = serde_json::json!([
        {
            "key": "<at id=someone>Someone</at>",
            "id": {"open_id": "ou_someone"}
        }
    ]);

    let ch = FeishuChannel::new(FeishuConfig {
        bot_open_id: Some("ou_bot".into()),
        ..Default::default()
    });
    let req = ch.build_request_from_native(event).unwrap();

    assert!(req.metadata.get("bot_mentioned").is_none());
    assert!(req.metadata.get("mentioned").is_none());
    match &req.input[0] {
        IncomingContentPart::Text { text } => {
            assert_eq!(text, "<at id=someone>Someone</at> deploy now")
        }
        _ => panic!("expected Text content"),
    }
}

#[test]
fn build_request_from_native_group_shared_session_uses_group_sender() {
    let mut event = sample_feishu_event();
    event["event"]["message"]["chat_type"] = serde_json::json!("group");
    event["event"]["message"]["chat_id"] = serde_json::json!("oc_group_123");

    let ch = FeishuChannel::new(FeishuConfig {
        share_session_in_group: true,
        ..Default::default()
    });
    let req = ch.build_request_from_native(event).unwrap();

    assert_eq!(req.user_id, "group");
    assert_eq!(req.conversation_id, "oc_group_123");
    assert_eq!(req.metadata["feishu_sender_id"], "ou_test_user");
}

#[test]
fn build_request_from_native_thread_extracts_metadata_and_session() {
    let mut event = sample_feishu_event();
    event["event"]["message"]["message_id"] = serde_json::json!("om_thread_001");
    event["event"]["message"]["chat_type"] = serde_json::json!("group");
    event["event"]["message"]["chat_id"] = serde_json::json!("oc_thread_group");
    event["event"]["message"]["thread_id"] = serde_json::json!("omt_thread_root_abc123");
    event["event"]["message"]["content"] = serde_json::json!("{\"text\":\"Hello in thread\"}");

    let ch = FeishuChannel::new(FeishuConfig::default());
    let req = ch.build_request_from_native(event).unwrap();

    assert_eq!(req.user_id, "thread:abc123");
    assert_eq!(req.session_id, "thread:abc123");
    assert_eq!(req.conversation_id, "oc_thread_group");
    assert_eq!(req.message_id, Some("om_thread_001".into()));
    assert_eq!(req.metadata["feishu_thread_id"], "omt_thread_root_abc123");
    assert_eq!(req.metadata["feishu_message_id"], "om_thread_001");
    assert_eq!(req.metadata["feishu_receive_id"], "oc_thread_group");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "Hello in thread"),
        _ => panic!("expected Text content"),
    }
}

#[test]
fn build_request_from_native_thread_overrides_shared_group_session() {
    let mut event = sample_feishu_event();
    event["event"]["message"]["chat_type"] = serde_json::json!("group");
    event["event"]["message"]["chat_id"] = serde_json::json!("oc_thread_group");
    event["event"]["message"]["thread_id"] = serde_json::json!("omt_thread_root_abc123");

    let ch = FeishuChannel::new(FeishuConfig {
        share_session_in_group: true,
        ..Default::default()
    });
    let req = ch.build_request_from_native(event).unwrap();

    assert_eq!(req.user_id, "thread:abc123");
    assert_eq!(req.session_id, "thread:abc123");
}

#[test]
fn build_request_from_native_without_thread_omits_thread_metadata() {
    let ch = FeishuChannel::new(FeishuConfig::default());
    let req = ch.build_request_from_native(sample_feishu_event()).unwrap();

    assert!(req.metadata.get("feishu_thread_id").is_none());
    assert_eq!(req.metadata["feishu_message_id"], "om_test_001");
    assert_eq!(req.metadata["feishu_receive_id"], "oc_test_chat");
}

#[test]
fn build_request_from_native_post_extracts_text() {
    let mut event = sample_feishu_event();
    event["event"]["message"]["message_type"] = serde_json::json!("post");
    event["event"]["message"]["content"] = serde_json::json!(serde_json::json!({
        "title": "Daily",
        "content": [[
            {"tag": "text", "text": "hello"},
            {"tag": "a", "text": "docs", "href": "https://example.com"},
            {"tag": "at", "user_name": "Alice"}
        ]]
    })
    .to_string());

    let ch = FeishuChannel::new(FeishuConfig::default());
    let req = ch.build_request_from_native(event).unwrap();

    match &req.input[0] {
        IncomingContentPart::Text { text } => {
            assert_eq!(text, "Daily hello [docs](https://example.com) @Alice")
        }
        _ => panic!("expected Text content"),
    }
}

#[test]
fn build_request_from_native_interactive_extracts_card_text() {
    let mut event = sample_feishu_event();
    event["event"]["message"]["message_type"] = serde_json::json!("interactive");
    event["event"]["message"]["content"] = serde_json::json!(serde_json::json!({
        "header": {"title": {"content": "Approval"}},
        "body": {
            "elements": [
                {"tag": "markdown", "content": "**Deploy** prod"},
                {"tag": "button", "text": {"content": "Approve"}}
            ]
        }
    })
    .to_string());

    let ch = FeishuChannel::new(FeishuConfig::default());
    let req = ch.build_request_from_native(event).unwrap();

    match &req.input[0] {
        IncomingContentPart::Text { text } => {
            assert_eq!(text, "Approval **Deploy** prod Approve")
        }
        _ => panic!("expected Text content"),
    }
}

#[test]
fn build_request_from_native_card_action_normalizes_approval_command() {
    let event = serde_json::json!({
        "header": {
            "event_id": "ev-card-001",
            "event_type": "card.action.trigger",
            "create_time": "1700000000000",
            "app_id": "cli_test"
        },
        "event": {
            "operator": {
                "open_id": "ou_manager"
            },
            "action": {
                "value": {
                    "type": "tool_guard_approval",
                    "action": "deny",
                    "request_id": "req-123",
                    "tool_name": "Bash",
                    "severity": "high",
                    "session_ctx": {
                        "session_id": "oc_test_chat",
                        "sender_id": "ou_test_user",
                        "receive_id": "oc_test_chat",
                        "receive_id_type": "chat_id",
                        "chat_id": "oc_test_chat",
                        "chat_type": "group",
                        "is_group": true
                    }
                }
            }
        }
    });

    let ch = FeishuChannel::new(FeishuConfig::default());
    let req = ch.build_request_from_native(event).unwrap();

    assert_eq!(req.channel_id, "feishu");
    assert_eq!(req.user_id, "ou_test_user");
    assert_eq!(req.session_id, "oc_test_chat");
    assert_eq!(req.conversation_id, "oc_test_chat");
    assert_eq!(req.metadata["approval_request_id"], "req-123");
    assert_eq!(req.metadata["approval_action"], "deny");
    assert_eq!(req.metadata["from_card_action"], true);
    assert_eq!(req.metadata["feishu_receive_id"], "oc_test_chat");
    assert_eq!(req.metadata["feishu_receive_id_type"], "chat_id");
    assert_eq!(req.metadata["feishu_operator_open_id"], "ou_manager");
    assert!(matches!(
        &req.input[0],
        IncomingContentPart::Text { text } if text == "/approval deny req-123"
    ));
}

#[test]
fn build_request_from_native_card_action_rejects_duplicate_approval() {
    let event = serde_json::json!({
        "header": {
            "event_id": "ev-card-dup",
            "event_type": "card.action.trigger",
            "create_time": "1700000000000",
            "app_id": "cli_test"
        },
        "event": {
            "operator": {"open_id": "ou_manager"},
            "action": {
                "value": {
                    "type": "tool_guard_approval",
                    "action": "approve",
                    "request_id": "req-dup",
                    "tool_name": "Bash",
                    "session_ctx": {
                        "session_id": "oc_test_chat",
                        "sender_id": "ou_test_user",
                        "receive_id": "oc_test_chat",
                        "receive_id_type": "chat_id",
                        "chat_id": "oc_test_chat",
                        "chat_type": "group",
                        "is_group": true
                    }
                }
            }
        }
    });
    let ch = FeishuChannel::new(FeishuConfig::default());

    ch.build_request_from_native(event.clone())
        .expect("first card action should parse");
    let err = ch
        .build_request_from_native(event)
        .expect_err("duplicate card action should be rejected");
    assert!(
        err.to_string().contains("duplicate approval callback"),
        "unexpected duplicate error: {err}"
    );
}

#[test]
fn build_request_from_native_skips_bot() {
    let mut event = sample_feishu_event();
    event["event"]["sender"]["sender_type"] = serde_json::json!("bot");

    let ch = FeishuChannel::new(FeishuConfig::default());
    let result = ch.build_request_from_native(event);
    assert!(result.is_err());
}

#[test]
fn build_request_from_native_image_type() {
    let mut event = sample_feishu_event();
    event["event"]["message"]["message_type"] = serde_json::json!("image");
    event["event"]["message"]["content"] = serde_json::json!("{\"image_key\":\"img_xxx\"}");

    let ch = FeishuChannel::new(FeishuConfig::default());
    let req = ch.build_request_from_native(event).unwrap();

    assert_eq!(req.input.len(), 1);
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "[image: img_xxx]"),
        _ => panic!("expected Text content with type label"),
    }
}

#[test]
fn build_request_from_native_media_types_include_provider_keys_or_missing_key_hints() {
    let cases = [
        ("image", "{\"other\":\"missing\"}", "[image: missing key]"),
        (
            "file",
            "{\"file_key\":\"file_abc\",\"file_name\":\"report.pdf\"}",
            "[file: report.pdf]",
        ),
        ("file", "{\"other\":\"missing\"}", "[file: missing key]"),
        (
            "audio",
            "{\"file_key\":\"audio_abc\"}",
            "[audio: audio_abc]",
        ),
        ("audio", "{\"other\":\"missing\"}", "[audio: missing key]"),
        (
            "media",
            "{\"file_key\":\"media_abc\"}",
            "[media: media_abc]",
        ),
        ("media", "{\"other\":\"missing\"}", "[media: missing key]"),
    ];

    for (message_type, content, expected) in cases {
        let mut event = sample_feishu_event();
        event["event"]["message"]["message_type"] = serde_json::json!(message_type);
        event["event"]["message"]["content"] = serde_json::json!(content);

        let ch = FeishuChannel::new(FeishuConfig::default());
        let req = ch
            .build_request_from_native(event)
            .unwrap_or_else(|err| panic!("{message_type} should normalize: {err}"));

        match &req.input[0] {
            IncomingContentPart::Text { text } => assert_eq!(text, expected),
            other => panic!("{message_type} normalized to non-text input: {other:?}"),
        }
    }
}

#[test]
fn build_request_from_native_empty_text() {
    let mut event = sample_feishu_event();
    event["event"]["message"]["content"] = serde_json::json!("{\"text\":\"  \"}");

    let ch = FeishuChannel::new(FeishuConfig::default());
    let result = ch.build_request_from_native(event);

    assert!(
        result.is_err(),
        "empty Feishu user messages should be skipped before agent dispatch"
    );
}

#[test]
fn build_request_from_native_invalid_payload() {
    let ch = FeishuChannel::new(FeishuConfig::default());
    let result = ch.build_request_from_native(serde_json::json!("not an object"));
    assert!(result.is_err());
}

#[tokio::test]
async fn download_inbound_image_injects_local_path_for_typed_normalization() {
    let temp = tempdir().unwrap();
    let (base_url, server) = start_feishu_media_fixture().await;
    let ch = FeishuChannel::new(FeishuConfig {
        media_dir: temp.path().to_string_lossy().to_string(),
        ..Default::default()
    })
    .with_open_api_base_url(base_url);
    let mut event = sample_feishu_event();
    event["event"]["message"]["message_type"] = serde_json::json!("image");
    event["event"]["message"]["content"] = serde_json::json!("{\"image_key\":\"img_xxx\"}");

    ch.download_inbound_media_with_token(&mut event, "tenant-token")
        .await
        .expect("feishu media download");

    let content = event["event"]["message"]["content"]
        .as_str()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .expect("mutated content json");
    let local_path = content["local_path"].as_str().expect("local path");
    assert!(local_path.starts_with(temp.path().to_string_lossy().as_ref()));
    assert!(local_path.contains("om_test_001"));
    assert!(local_path.contains("img_xxx"));
    assert_eq!(content["mime"], "image/png");
    assert_eq!(std::fs::read_to_string(local_path).unwrap(), "png-bytes");

    let req = ch
        .build_request_from_native(event)
        .expect("typed feishu media request");
    match &req.input[0] {
        IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert_eq!(file_id, "img_xxx");
            assert_eq!(path.to_string_lossy(), local_path);
            assert_eq!(mime.as_deref(), Some("image/png"));
        }
        other => panic!("expected image part, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn download_inbound_file_audio_and_media_inject_local_paths_for_typed_normalization() {
    let temp = tempdir().unwrap();
    let (base_url, server) = start_feishu_media_fixture().await;
    let ch = FeishuChannel::new(FeishuConfig {
        media_dir: temp.path().to_string_lossy().to_string(),
        ..Default::default()
    })
    .with_open_api_base_url(base_url);

    let cases = [
        (
            "file",
            r#"{"file_key":"file_xxx","file_name":"report.pdf"}"#,
            "application/pdf",
            "pdf-bytes",
        ),
        (
            "audio",
            r#"{"file_key":"audio_xxx","file_name":"voice.mp3"}"#,
            "audio/mpeg",
            "audio-bytes",
        ),
        (
            "media",
            r#"{"file_key":"media_xxx","file_name":"clip.mp4"}"#,
            "video/mp4",
            "video-bytes",
        ),
    ];

    for (msg_type, content, expected_mime, expected_bytes) in cases {
        let mut event = sample_feishu_event();
        event["header"]["event_id"] = serde_json::json!(format!("ev-{msg_type}"));
        event["event"]["message"]["message_id"] = serde_json::json!(format!("om_test_{msg_type}"));
        event["event"]["message"]["message_type"] = serde_json::json!(msg_type);
        event["event"]["message"]["content"] = serde_json::json!(content);

        ch.download_inbound_media_with_token(&mut event, "tenant-token")
            .await
            .expect("feishu media download");

        let content = event["event"]["message"]["content"]
            .as_str()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .expect("mutated content json");
        let local_path = content["local_path"].as_str().expect("local path");
        assert!(local_path.starts_with(temp.path().to_string_lossy().as_ref()));
        assert_eq!(content["mime"], expected_mime);
        assert_eq!(std::fs::read_to_string(local_path).unwrap(), expected_bytes);

        let req = ch
            .build_request_from_native(event)
            .expect("typed feishu media request");
        match (msg_type, &req.input[0]) {
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
                "audio",
                IncomingContentPart::Audio {
                    path, mime, name, ..
                },
            ) => {
                assert_eq!(path.to_string_lossy(), local_path);
                assert_eq!(mime.as_deref(), Some("audio/mpeg"));
                assert_eq!(name.as_deref(), Some("voice.mp3"));
            }
            (
                "media",
                IncomingContentPart::Video {
                    path, mime, name, ..
                },
            ) => {
                assert_eq!(path.to_string_lossy(), local_path);
                assert_eq!(mime.as_deref(), Some("video/mp4"));
                assert_eq!(name.as_deref(), Some("clip.mp4"));
            }
            (_, other) => panic!("expected {msg_type} part, got {other:?}"),
        }
    }

    server.abort();
}

#[tokio::test]
async fn download_inbound_media_failure_keeps_placeholder_payload() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/open-apis/im/v1/messages/{message_id}/resources/{file_key}",
        get(|| async { StatusCode::BAD_GATEWAY }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let temp = tempdir().unwrap();
    let ch = FeishuChannel::new(FeishuConfig {
        media_dir: temp.path().to_string_lossy().to_string(),
        ..Default::default()
    })
    .with_open_api_base_url(format!("http://127.0.0.1:{}", addr.port()));
    let mut event = sample_feishu_event();
    event["event"]["message"]["message_type"] = serde_json::json!("image");
    event["event"]["message"]["content"] = serde_json::json!("{\"image_key\":\"img_xxx\"}");

    ch.download_inbound_media_with_token(&mut event, "tenant-token")
        .await
        .expect("download failure should not abort fallback parsing");

    let req = ch
        .build_request_from_native(event)
        .expect("fallback feishu media request");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "[image: img_xxx]"),
        other => panic!("expected fallback text, got {other:?}"),
    }

    server.abort();
}

async fn start_ws_server<F, Fut>(handler: F) -> (String, tokio::task::JoinHandle<()>)
where
    F: Fn(WebSocket) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let app = axum::Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let h = handler.clone();
            async move { ws.on_upgrade(h).into_response() }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = format!("ws://127.0.0.1:{}/ws", addr.port());
    (url, server)
}

#[tokio::test]
async fn integration_ws_event_to_process_handle() {
    let requests = Arc::new(Mutex::new(Vec::<AgentRequest>::new()));
    let requests_clone = requests.clone();

    let (url, server) = start_ws_server(|mut socket: WebSocket| async move {
        let event = serde_json::json!({
            "type": "event",
            "data": {
                "header": {
                    "event_type": "im.message.receive_v1",
                    "event_id": "ev-int-001"
                },
                "event": {
                    "sender": {
                        "sender_id": { "open_id": "ou_integration" },
                        "sender_type": "user"
                    },
                    "message": {
                        "message_id": "om_int_001",
                        "chat_id": "oc_int_chat",
                        "chat_type": "p2p",
                        "message_type": "text",
                        "content": "{\"text\":\"integration test\"}"
                    }
                }
            }
        });
        socket
            .send(AxumMessage::Text(
                serde_json::to_string(&event).unwrap().into(),
            ))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = socket.send(AxumMessage::Close(None)).await;
    })
    .await;

    let process = Arc::new(MockProcess {
        requests: requests_clone,
    });

    let on_message: rua_channel::feishu::ws::OnMessageFn = {
        let p = process.clone();
        Arc::new(move |native| {
            let p = p.clone();
            tokio::spawn(async move {
                let req = rua_channel::feishu::FeishuChannel::new(FeishuConfig::default())
                    .build_request_from_native(native);
                if let Ok(req) = req {
                    let mut stream = p.handle(req).await;
                    while stream.next().await.is_some() {}
                }
            });
        })
    };

    let (_stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let ws_stream = tokio_tungstenite::connect_async(&url).await.unwrap().0;
    let _disconnected =
        rua_channel::feishu::ws::handle_ws_session(ws_stream, &on_message, &mut stop_rx.clone())
            .await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let reqs = requests.lock().await;
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].channel_id, "feishu");
    assert_eq!(reqs[0].user_id, "ou_integration");
    assert_eq!(reqs[0].conversation_id, "oc_int_chat");
    match &reqs[0].input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "integration test"),
        _ => panic!("expected Text"),
    }

    server.abort();
}

#[tokio::test]
async fn integration_disconnect_reconnect_backoff() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let connect_count = Arc::new(AtomicUsize::new(0));
    let connect_count_clone = connect_count.clone();
    let requests = Arc::new(Mutex::new(Vec::<AgentRequest>::new()));
    let requests_clone = requests.clone();

    let (url, server) = start_ws_server(move |mut socket: WebSocket| {
        let count = connect_count_clone.clone();
        async move {
            let n = count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                let _ = socket.send(AxumMessage::Close(None)).await;
            } else {
                let event = serde_json::json!({
                    "type": "event",
                    "data": {
                        "header": { "event_type": "im.message.receive_v1" },
                        "event": {
                            "sender": {
                                "sender_id": { "open_id": "ou_reconnect" },
                                "sender_type": "user"
                            },
                            "message": {
                                "message_id": "om_reconnect",
                                "chat_id": "oc_reconnect",
                                "chat_type": "p2p",
                                "message_type": "text",
                                "content": "{\"text\":\"after reconnect\"}"
                            }
                        }
                    }
                });
                socket
                    .send(AxumMessage::Text(
                        serde_json::to_string(&event).unwrap().into(),
                    ))
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
                let _ = socket.send(AxumMessage::Close(None)).await;
            }
        }
    })
    .await;

    let on_message: rua_channel::feishu::ws::OnMessageFn = {
        let reqs = requests_clone.clone();
        Arc::new(move |native| {
            let reqs = reqs.clone();
            tokio::spawn(async move {
                let ch = FeishuChannel::new(FeishuConfig::default());
                if let Ok(req) = ch.build_request_from_native(native) {
                    reqs.lock().await.push(req);
                }
            });
        })
    };

    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let on_msg = on_message.clone();

    let task = tokio::spawn(async move {
        let mut backoff = Duration::from_millis(100);
        let max_backoff = Duration::from_secs(1);
        let mut connections = 0u32;
        loop {
            if *stop_rx.borrow() {
                break;
            }
            let ws = match tokio_tungstenite::connect_async(&url).await {
                Ok((stream, _)) => {
                    backoff = Duration::from_millis(100);
                    stream
                }
                Err(_) => {
                    tokio::select! {
                        _ = tokio::time::sleep(backoff) => {}
                        _ = stop_rx.changed() => { break; }
                    }
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }
            };
            connections += 1;
            let disconnected =
                rua_channel::feishu::ws::handle_ws_session(ws, &on_msg, &mut stop_rx).await;
            if !disconnected || connections >= 2 {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = stop_rx.changed() => { break; }
            }
            backoff = (backoff * 2).min(max_backoff);
        }
    });

    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("reconnect did not complete within 5s")
        .unwrap();

    assert!(connect_count.load(std::sync::atomic::Ordering::SeqCst) >= 2);

    tokio::time::sleep(Duration::from_millis(100)).await;
    let reqs = requests.lock().await;
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].user_id, "ou_reconnect");

    drop(stop_tx);
    server.abort();
}

#[tokio::test]
async fn outbound_channel_id_unchanged() {
    let ch = FeishuChannel::new(FeishuConfig::default());
    assert_eq!(ch.id(), "feishu");
}

#[tokio::test]
async fn outbound_start_stop_without_ws() {
    let ch = FeishuChannel::new(FeishuConfig::default());
    let process = Arc::new(MockProcess {
        requests: Arc::new(Mutex::new(vec![])),
    });
    ch.start(process).await.unwrap();
    ch.stop().await.unwrap();
}
