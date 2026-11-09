use rua_channel::core::content::IncomingContentPart;
use rua_channel::dingtalk::parser::{parse_native, parse_with_routing};

fn sample_payload() -> serde_json::Value {
    serde_json::json!({
        "msgType": "text",
        "msgId": "msg-001",
        "conversationId": "conv-123",
        "conversationType": "1",
        "senderId": "user-456",
        "senderNick": "Alice",
        "senderStaffId": "staff-789",
        "robotCode": "bot-abc",
        "isInAtList": false,
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xyz",
        "sessionWebhookExpiredTime": 1700000000000i64,
        "text": { "content": " hello world " }
    })
}

#[test]
fn parse_text_message_into_agent_channel_request() {
    let req = parse_native(sample_payload()).unwrap();

    assert_eq!(req.channel_id, "dingtalk");
    assert_eq!(req.user_id, "user-456");
    assert_eq!(req.conversation_id, "conv-123");
    assert_eq!(req.session_id, "conv-123");
    assert_eq!(req.message_id, Some("msg-001".into()));
    assert_eq!(req.input.len(), 1);
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "hello world"),
        other => panic!("expected Text content, got {other:?}"),
    }
    assert_eq!(
        req.metadata["session_webhook"],
        "https://oapi.dingtalk.com/robot/sendBySession?session=xyz"
    );
    assert_eq!(req.metadata["sender_nick"], "Alice");
}

#[test]
fn parse_empty_or_missing_text_as_empty_input() {
    let mut blank_text = sample_payload();
    blank_text["text"]["content"] = serde_json::json!("  ");
    assert!(parse_native(blank_text).unwrap().input.is_empty());

    let missing_text = serde_json::json!({
        "msgType": "image",
        "msgId": "msg-002",
        "conversationId": "conv-456",
        "conversationType": "2",
        "senderId": "user-789",
        "senderNick": "Bob",
        "robotCode": "bot-def",
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=abc"
    });
    let req = parse_native(missing_text).unwrap();
    assert!(req.input.is_empty());
    assert_eq!(req.metadata["msg_type"], "image");
}

#[test]
fn parse_rich_text_content_when_top_level_text_is_absent() {
    let mut payload = sample_payload();
    payload["text"] = serde_json::Value::Null;
    payload["content"] = serde_json::json!({
        "richText": [
            { "text": "Hello" },
            { "content": "world" }
        ]
    });

    let req = parse_native(payload).unwrap();

    assert_eq!(
        req.input,
        vec![
            IncomingContentPart::Text {
                text: "Hello".into()
            },
            IncomingContentPart::Text {
                text: "world".into()
            },
        ]
    );
}

#[test]
fn parse_rich_media_download_codes_as_stable_placeholders() {
    let mut payload = sample_payload();
    payload["text"]["content"] = serde_json::json!("see attachments");
    payload["content"] = serde_json::json!({
        "richText": [
            { "type": "picture", "downloadCode": "img_dl_code" },
            { "type": "file", "download_code": "file_dl_code", "fileName": "report.pdf" }
        ]
    });

    let req = parse_native(payload).unwrap();

    assert_eq!(
        req.input,
        vec![
            IncomingContentPart::Text {
                text: "see attachments".into()
            },
            IncomingContentPart::Text {
                text: "[image: img_dl_code]".into()
            },
            IncomingContentPart::Text {
                text: "[file: report.pdf]".into()
            },
        ]
    );
}

#[test]
fn parse_single_download_code_and_voice_recognition_from_content() {
    let mut image = sample_payload();
    image["msgType"] = serde_json::json!("picture");
    image["text"]["content"] = serde_json::json!("//n");
    image["content"] = serde_json::json!({
        "downloadCode": "single_img_code"
    });
    let req = parse_native(image).unwrap();
    assert_eq!(
        req.input,
        vec![IncomingContentPart::Text {
            text: "[image: single_img_code]".into()
        }]
    );

    let mut voice = sample_payload();
    voice["msgType"] = serde_json::json!("voice");
    voice["text"] = serde_json::Value::Null;
    voice["content"] = serde_json::json!({
        "downloadCode": "voice_dl_code",
        "recognition": "转写文本"
    });
    let req = parse_native(voice).unwrap();
    assert_eq!(
        req.input,
        vec![IncomingContentPart::Text {
            text: "转写文本".into()
        }]
    );
}

#[test]
fn parse_quoted_text_context_before_current_text() {
    let mut payload = sample_payload();
    payload["text"] = serde_json::json!({
        "content": "current reply",
        "isReplyMsg": true,
        "repliedMsg": {
            "msgType": "text",
            "content": { "text": "original question" }
        }
    });

    let req = parse_native(payload).unwrap();

    assert_eq!(
        req.input,
        vec![
            IncomingContentPart::Text {
                text: "[quoted message: original question]".into()
            },
            IncomingContentPart::Text {
                text: "current reply".into()
            },
        ]
    );
}

#[test]
fn parse_quoted_rich_text_and_media_context() {
    let mut payload = sample_payload();
    payload["text"] = serde_json::json!({
        "content": "current reply",
        "isReplyMsg": true,
        "repliedMsg": {
            "msgType": "richText",
            "content": {
                "richText": [
                    { "msgType": "text", "content": "quoted note" },
                    { "msgType": "picture", "downloadCode": "quoted_img_code" }
                ]
            }
        }
    });

    let req = parse_native(payload).unwrap();

    assert_eq!(
        req.input,
        vec![
            IncomingContentPart::Text {
                text: "[quoted message: quoted note]".into()
            },
            IncomingContentPart::Text {
                text: "[image: quoted_img_code]".into()
            },
            IncomingContentPart::Text {
                text: "current reply".into()
            },
        ]
    );
}

#[test]
fn invalid_native_payload_returns_validation_error() {
    let err = parse_native(serde_json::json!("not an object")).unwrap_err();
    assert!(matches!(err, rua_core::RuaError::Validation { .. }));
}

#[test]
fn routing_p2p_messages_are_always_accepted() {
    let result = parse_with_routing(sample_payload(), "bot-abc", "MyBot").unwrap();

    let req = result.expect("p2p DingTalk messages should be routed");
    assert_eq!(req.user_id, "user-456");
}

#[test]
fn routing_group_messages_accept_at_users_mentions() {
    let payload = serde_json::json!({
        "msgType": "text",
        "msgId": "msg-g01",
        "conversationId": "conv-group-1",
        "conversationType": "2",
        "senderId": "user-001",
        "senderNick": "Alice",
        "robotCode": "bot-abc",
        "sessionWebhook": "https://example.com/webhook",
        "text": { "content": "@bot-abc 帮我查一下" },
        "atUsers": [
            { "dingtalkId": "bot-abc" }
        ]
    });

    let req = parse_with_routing(payload, "bot-abc", "MyBot")
        .unwrap()
        .expect("explicit atUsers bot mention should route");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "帮我查一下"),
        other => panic!("expected Text content, got {other:?}"),
    }
}

#[test]
fn routing_group_messages_accept_text_fallback_mentions() {
    let payload = serde_json::json!({
        "msgType": "text",
        "msgId": "msg-g02",
        "conversationId": "conv-group-2",
        "conversationType": "2",
        "senderId": "user-002",
        "senderNick": "Bob",
        "robotCode": "bot-abc",
        "sessionWebhook": "https://example.com/webhook",
        "text": { "content": "@MyBot 你好" },
        "atUsers": [
            { "dingtalkId": "user-003" }
        ]
    });

    let req = parse_with_routing(payload, "bot-abc", "MyBot")
        .unwrap()
        .expect("text @bot-name fallback should route");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "你好"),
        other => panic!("expected Text content, got {other:?}"),
    }
}

#[test]
fn routing_group_messages_without_mentions_are_skipped() {
    let payload = serde_json::json!({
        "msgType": "text",
        "msgId": "msg-g03",
        "conversationId": "conv-group-3",
        "conversationType": "2",
        "senderId": "user-003",
        "senderNick": "Carol",
        "robotCode": "bot-abc",
        "sessionWebhook": "https://example.com/webhook",
        "text": { "content": "random chat message" },
        "atUsers": [
            { "dingtalkId": "user-004" }
        ]
    });

    assert!(parse_with_routing(payload, "bot-abc", "MyBot")
        .unwrap()
        .is_none());
}
