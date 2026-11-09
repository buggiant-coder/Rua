use rua_channel::core::channel::Channel;
use rua_channel::core::content::IncomingContentPart;
use rua_channel::yuanbao::{YuanbaoChannel, YuanbaoConfig};
use tempfile::tempdir;

fn channel() -> YuanbaoChannel {
    YuanbaoChannel::new(YuanbaoConfig {
        enabled: true,
        app_id: "app-1".into(),
        app_secret: "secret-1".into(),
        api_domain: "yuanbao.example.invalid".into(),
        media_dir: String::new(),
    })
}

#[test]
fn build_request_from_c2c_push_normalizes_sender_and_text() {
    let native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "CK8kfT4SpnXTsZg7ovLVTzWJLv8EymvNXO1BhLuAgOYwVFC1HLHzx5qq7AG0zjPq",
        "sender_nickname": "灰",
        "msg_id": "msg-1",
        "msg_body": [
            {"msg_type": "TIMTextElem", "msg_content": {"text": "  hello  "}},
            {"msg_type": "TIMTextElem", "msg_content": {"text": "yuanbao"}}
        ]
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(request.channel_id, "yuanbao");
    assert_eq!(request.user_id, "灰#zjPq");
    assert_eq!(request.session_id, "yuanbao:7AG0zjPq");
    assert_eq!(request.conversation_id, "yuanbao:7AG0zjPq");
    assert_eq!(request.message_id.as_deref(), Some("msg-1"));
    assert_eq!(request.metadata["chat_type"], "c2c");
    assert_eq!(
        request.metadata["from_account"],
        "CK8kfT4SpnXTsZg7ovLVTzWJLv8EymvNXO1BhLuAgOYwVFC1HLHzx5qq7AG0zjPq"
    );
    assert_eq!(request.metadata["user_name"], "灰#zjPq");
    assert_eq!(
        request.input,
        vec![IncomingContentPart::Text {
            text: "hello yuanbao".into()
        }]
    );
}

#[test]
fn build_request_persists_session_route_for_proactive_send() {
    let temp = tempdir().unwrap();
    let channel = YuanbaoChannel::new(YuanbaoConfig {
        media_dir: temp.path().join("media").to_string_lossy().to_string(),
        ..YuanbaoConfig::default()
    });
    let native = serde_json::json!({
        "callback_command": "Group.CallbackAfterSendMsg",
        "from_account": "raw-user-1",
        "group_code": "group-123",
        "sender_nickname": "灰",
        "msg_id": "msg-route-1",
        "msg_body": [
            {"msg_type": "TIMTextElem", "msg_content": {"text": "hello"}}
        ]
    });

    channel.build_request_from_native(native).unwrap();

    let path = temp.path().join("yuanbao_sessions.json");
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("session route file")).unwrap();
    assert_eq!(saved["group-123"]["chat_type"], "group");
    assert_eq!(saved["group-123"]["sender_id"], "raw-user-1");
    assert_eq!(saved["group-123"]["group_code"], "group-123");
    assert_eq!(saved["group-123"]["msg_id"], "msg-route-1");
}

#[test]
fn build_request_from_native_rejects_duplicate_msg_id() {
    let channel = channel();
    let native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "sender-1",
        "msg_id": "msg-duplicate",
        "msg_body": [
            {"msg_type": "TIMTextElem", "msg_content": {"text": "hello"}}
        ]
    });

    channel.build_request_from_native(native.clone()).unwrap();

    let err = channel.build_request_from_native(native).unwrap_err();
    assert!(err.to_string().contains("duplicate yuanbao event"));
}

#[test]
fn build_request_from_group_push_uses_group_session() {
    let native = serde_json::json!({
        "callback_command": "Group.CallbackAfterSendMsg",
        "from_account": "user-abc",
        "group_code": "group-123",
        "sender_nickname": "",
        "msg_body": [
            {"msg_type": "TIMTextElem", "msg_content": {"text": "hello group"}}
        ]
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(request.user_id, "unknown#-abc");
    assert_eq!(request.session_id, "yuanbao:group-123");
    assert_eq!(request.conversation_id, "yuanbao:group-123");
    assert_eq!(request.metadata["chat_type"], "group");
    assert_eq!(request.metadata["group_code"], "group-123");
    assert_eq!(request.metadata["is_group"], true);
    assert_eq!(request.metadata["mentioned"], false);
    assert_eq!(request.metadata["session_id"], "group-123");
}

#[test]
fn build_request_from_group_push_surfaces_custom_mention_metadata() {
    let native = serde_json::json!({
        "callback_command": "Group.CallbackAfterSendMsg",
        "from_account": "sender-abc",
        "group_code": "group-123",
        "msg_body": [
            {
                "msg_type": "TIMCustomElem",
                "msg_content": {
                    "data": r#"{"elem_type":1002,"user_id":"bot-123","text":"@Bot"}"#,
                    "desc": "@Bot"
                }
            },
            {"msg_type": "TIMTextElem", "msg_content": {"text": "hello"}}
        ]
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(request.metadata["chat_type"], "group");
    assert_eq!(request.metadata["is_group"], true);
    assert_eq!(request.metadata["mentioned"], true);
    assert_eq!(request.metadata["mention_user_id"], "bot-123");
    assert_eq!(
        request.input,
        vec![IncomingContentPart::Text {
            text: "hello".into()
        }]
    );
}

#[test]
fn build_request_accepts_camel_case_payload_fields() {
    let native = serde_json::json!({
        "callbackCommand": "C2C.CallbackAfterSendMsg",
        "fromAccount": "sender-12345678",
        "senderNickname": "Sender",
        "msgId": "camel-1",
        "msgBody": [
            {"msgType": "TIMTextElem", "msgContent": {"text": "camel text"}}
        ]
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(request.message_id.as_deref(), Some("camel-1"));
    assert_eq!(request.session_id, "yuanbao:12345678");
    assert_eq!(request.metadata["from_account"], "sender-12345678");
    assert_eq!(
        request.input,
        vec![IncomingContentPart::Text {
            text: "camel text".into()
        }]
    );
}

#[test]
fn build_request_surfaces_media_only_payload_as_download_placeholder() {
    let native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "sender-1",
        "msg_body": [
            {"msg_type": "TIMImageElem", "msg_content": {"url": "https://example.invalid/image.png"}},
            {"msg_type": "TIMTextElem", "msg_content": {"text": "   "}}
        ]
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(
        request.input,
        vec![IncomingContentPart::Text {
            text: "[image: download failed]".into()
        }]
    );
}

#[test]
fn build_request_accepts_local_image_and_file_media_parts() {
    let native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "sender-1",
        "msg_body": [
            {
                "msg_type": "TIMImageElem",
                "msg_content": {
                    "url": "https://cdn.example.com/image.jpg",
                    "local_path": "/tmp/image.jpg",
                    "mime": "image/jpeg"
                }
            },
            {
                "msg_type": "TIMFileElem",
                "msg_content": {
                    "url": "https://cdn.example.com/report.pdf",
                    "file_name": "report.pdf",
                    "local_path": "/tmp/report.pdf",
                    "mime": "application/pdf"
                }
            },
            {
                "msg_type": "TIMFileElem",
                "msg_content": {
                    "url": "https://cdn.example.com/voice.mp3",
                    "file_name": "voice.mp3",
                    "local_path": "/tmp/voice.mp3",
                    "mime": "audio/mpeg"
                }
            }
        ]
    });

    let request = channel().build_request_from_native(native).unwrap();

    match &request.input[0] {
        IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert_eq!(file_id, "https://cdn.example.com/image.jpg");
            assert_eq!(path.to_string_lossy(), "/tmp/image.jpg");
            assert_eq!(mime.as_deref(), Some("image/jpeg"));
        }
        other => panic!("expected image part, got {other:?}"),
    }
    match &request.input[1] {
        IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "https://cdn.example.com/report.pdf");
            assert_eq!(path.to_string_lossy(), "/tmp/report.pdf");
            assert_eq!(mime.as_deref(), Some("application/pdf"));
            assert_eq!(name.as_deref(), Some("report.pdf"));
        }
        other => panic!("expected file part, got {other:?}"),
    }
    match &request.input[2] {
        IncomingContentPart::Audio {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "https://cdn.example.com/voice.mp3");
            assert_eq!(path.to_string_lossy(), "/tmp/voice.mp3");
            assert_eq!(mime.as_deref(), Some("audio/mpeg"));
            assert_eq!(name.as_deref(), Some("voice.mp3"));
        }
        other => panic!("expected audio part, got {other:?}"),
    }
}

#[tokio::test]
async fn download_inbound_image_url_injects_local_path_for_typed_normalization() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind yuanbao media fixture");
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
    let channel = YuanbaoChannel::new(YuanbaoConfig {
        media_dir: temp.path().to_string_lossy().to_string(),
        ..YuanbaoConfig::default()
    });
    let mut native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "sender-1",
        "msg_body": [
            {
                "msg_type": "TIMImageElem",
                "msg_content": {
                    "image_info_array": [
                        {"url": format!("http://127.0.0.1:{}/image.jpg", addr.port())}
                    ]
                }
            }
        ]
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("yuanbao media download");

    let local_path = native["msg_body"][0]["msg_content"]["local_path"]
        .as_str()
        .expect("local path")
        .to_string();
    assert!(local_path.starts_with(temp.path().to_string_lossy().as_ref()));
    assert_eq!(native["msg_body"][0]["msg_content"]["mime"], "image/jpeg");
    assert_eq!(std::fs::read_to_string(&local_path).unwrap(), "jpeg-bytes");

    let request = channel.build_request_from_native(native).unwrap();
    match &request.input[0] {
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
async fn download_inbound_media_failure_keeps_placeholder_payload() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind yuanbao failure fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let app = axum::Router::new().route(
        "/image.jpg",
        axum::routing::get(|| async { axum::http::StatusCode::BAD_GATEWAY }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });
    let channel = YuanbaoChannel::new(YuanbaoConfig::default());
    let mut native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "sender-1",
        "msg_body": [
            {
                "msg_type": "TIMImageElem",
                "msg_content": {
                    "image_info_array": [
                        {"url": format!("http://127.0.0.1:{}/image.jpg", addr.port())}
                    ]
                }
            }
        ]
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("download failure should preserve fallback parsing");

    let request = channel.build_request_from_native(native).unwrap();
    match &request.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "[image: download failed]"),
        other => panic!("expected fallback text, got {other:?}"),
    }

    server.abort();
}

#[test]
fn build_request_prepends_quoted_text_prefix_to_user_text() {
    let native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "user-account",
        "sender_nickname": "灰",
        "msg_body": [
            {"msg_type": "TIMTextElem", "msg_content": {"text": "11"}}
        ],
        "cloud_custom_data": {"quote": {"type": 1, "desc": "你好呀"}}
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(
        request.input,
        vec![IncomingContentPart::Text {
            text: "[quoted message: 你好呀]\n11".into()
        }]
    );
}

#[test]
fn build_request_supports_json_string_quote_and_audio_suffix_label() {
    let native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "user-account",
        "msg_body": [
            {"msg_type": "TIMTextElem", "msg_content": {"text": "继续"}}
        ],
        "cloud_custom_data": r#"{"quote":{"type":3,"desc":"test_副本.mp3"}}"#
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(
        request.input,
        vec![IncomingContentPart::Text {
            text: "[quoted audio: test_副本.mp3]\n继续".into()
        }]
    );
}

#[test]
fn build_request_labels_non_audio_type_three_quote_as_file() {
    let native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "user-account",
        "msg_body": [
            {"msg_type": "TIMTextElem", "msg_content": {"text": "看一下"}}
        ],
        "cloud_custom_data": {"quote": {"type": 3, "desc": "陈灵威.txt"}}
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(
        request.input,
        vec![IncomingContentPart::Text {
            text: "[quoted file: 陈灵威.txt]\n看一下".into()
        }]
    );
}

#[test]
fn build_request_uses_message_label_for_unknown_quote_type() {
    let native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "user-account",
        "msg_body": [
            {"msg_type": "TIMTextElem", "msg_content": {"text": "继续"}}
        ],
        "cloud_custom_data": {"quote": {"type": 99, "desc": ""}}
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(
        request.input,
        vec![IncomingContentPart::Text {
            text: "[quoted message]\n继续".into()
        }]
    );
}

#[test]
fn build_request_ignores_malformed_cloud_custom_data_and_keeps_text() {
    let native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "user-account",
        "msg_body": [
            {"msg_type": "TIMTextElem", "msg_content": {"text": "普通消息"}}
        ],
        "cloud_custom_data": "{not json"
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(
        request.input,
        vec![IncomingContentPart::Text {
            text: "普通消息".into()
        }]
    );
}

#[test]
fn build_request_accepts_camel_case_cloud_custom_data_quote() {
    let native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "user-account",
        "msg_body": [
            {"msg_type": "TIMTextElem", "msg_content": {"text": "camel"}}
        ],
        "cloudCustomData": r#"{"quote":{"type":2,"desc":""}}"#
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(
        request.input,
        vec![IncomingContentPart::Text {
            text: "[quoted image]\ncamel".into()
        }]
    );
}

#[test]
fn build_request_can_surface_quote_even_when_media_body_is_not_supported() {
    let native = serde_json::json!({
        "callback_command": "C2C.CallbackAfterSendMsg",
        "from_account": "user-account",
        "msg_body": [
            {"msg_type": "TIMImageElem", "msg_content": {"url": "https://example.invalid/a.png"}}
        ],
        "cloud_custom_data": {"quote": {"type": 2, "desc": ""}}
    });

    let request = channel().build_request_from_native(native).unwrap();

    assert_eq!(
        request.input,
        vec![
            IncomingContentPart::Text {
                text: "[quoted image]".into()
            },
            IncomingContentPart::Text {
                text: "[image: download failed]".into()
            }
        ]
    );
}

#[test]
fn bot_messages_require_explicit_opt_in_for_both_upstream_markers() {
    for (account, extra) in [
        ("bot_123", serde_json::Value::Null),
        (
            "ordinary-user",
            serde_json::json!({"elem_type":1013}).to_string().into(),
        ),
    ] {
        let native = serde_json::json!({
            "callback_command": "C2C.CallbackAfterSendMsg",
            "from_account": account,
            "msg_body": [{"msg_type":"TIMTextElem", "msg_content":{"text":"hello","data":extra}}]
        });
        assert!(channel().build_request_from_native(native.clone()).is_err());
        let request = channel()
            .with_acl_config(serde_json::json!({"accept_bot_messages":true}))
            .build_request_from_native(native)
            .unwrap();
        assert_eq!(request.metadata["from_account"], account);
        assert!(!request.input.is_empty());
    }
}
