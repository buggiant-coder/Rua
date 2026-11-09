use rua_channel::core::channel::Channel;
use rua_channel::core::content::IncomingContentPart;
use rua_channel::dingtalk::DingTalkChannel;
use rua_channel::feishu::{FeishuChannel, FeishuConfig};
use rua_channel::mattermost::{MattermostChannel, MattermostConfig};
use rua_channel::onebot::{OneBotChannel, OneBotConfig};
use rua_channel::qq::QqChannel;
use rua_channel::telegram::{TelegramChannel, TelegramConfig};

type PrivateTextCase = (
    &'static str,
    Box<dyn Channel>,
    serde_json::Value,
    &'static str,
    &'static str,
    &'static str,
);

fn telegram_private_text_native() -> serde_json::Value {
    serde_json::json!({
        "update_id": 12345,
        "message": {
            "message_id": 1,
            "chat": {
                "id": 100,
                "type": "private"
            },
            "from": {
                "id": 200,
                "is_bot": false,
                "first_name": "Test"
            },
            "text": "hello telegram"
        }
    })
}

fn dingtalk_private_text_native(text: &str) -> serde_json::Value {
    serde_json::json!({
        "msgType": "text",
        "msgId": "msg-p2p-001",
        "conversationId": "conv-p2p-1",
        "conversationType": "1",
        "senderId": "user-100",
        "senderNick": "Alice",
        "robotCode": "bot-abc",
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=p2p",
        "text": { "content": text }
    })
}

fn dingtalk_group_text_native(text: &str) -> serde_json::Value {
    serde_json::json!({
        "msgType": "text",
        "msgId": "msg-grp-001",
        "conversationId": "conv-grp-1",
        "conversationType": "2",
        "senderId": "user-200",
        "senderNick": "Bob",
        "robotCode": "bot-abc",
        "isInAtList": true,
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=grp",
        "text": { "content": text },
        "atUsers": [
            { "dingtalkId": "bot-abc" }
        ]
    })
}

fn feishu_private_text_native(text: &str) -> serde_json::Value {
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
                "content": serde_json::json!({ "text": text }).to_string()
            }
        }
    })
}

fn feishu_group_text_native(text: &str) -> serde_json::Value {
    let mut event = feishu_private_text_native(text);
    event["event"]["message"]["chat_type"] = serde_json::json!("group");
    event["event"]["message"]["chat_id"] = serde_json::json!("oc_group_123");
    event
}

fn feishu_image_native() -> serde_json::Value {
    let mut event = feishu_private_text_native("ignored");
    event["event"]["message"]["message_type"] = serde_json::json!("image");
    event["event"]["message"]["content"] = serde_json::json!("{\"image_key\":\"img_xxx\"}");
    event
}

fn feishu_media_native(message_type: &str, content: serde_json::Value) -> serde_json::Value {
    let mut event = feishu_private_text_native("ignored");
    event["event"]["message"]["message_type"] = serde_json::json!(message_type);
    event["event"]["message"]["content"] = serde_json::json!(content.to_string());
    event
}

fn mattermost_direct_text_native(text: &str) -> serde_json::Value {
    serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "D",
            "post": serde_json::json!({
                "id": "post-001",
                "channel_id": "ch-123",
                "user_id": "user-456",
                "message": text,
                "root_id": ""
            }).to_string()
        }
    })
}

fn mattermost_thread_text_native(text: &str, root_id: &str, post_id: &str) -> serde_json::Value {
    serde_json::json!({
        "event": "posted",
        "data": {
            "channel_type": "O",
            "post": serde_json::json!({
                "id": post_id,
                "channel_id": "ch-789",
                "user_id": "user-111",
                "message": text,
                "root_id": root_id
            }).to_string()
        }
    })
}

fn qq_private_text_native(text: &str) -> serde_json::Value {
    serde_json::json!({
        "sender_id": "user_abc",
        "content": text,
        "msg_id": "msg_001",
        "message_type": "c2c"
    })
}

fn qq_group_text_native(text: &str) -> serde_json::Value {
    serde_json::json!({
        "message_type": "group",
        "author": {
            "member_openid": "member_1",
            "id": "fallback_id"
        },
        "content": text,
        "id": "msg_003",
        "group_openid": "grp_300"
    })
}

fn onebot_private_text_native(text: &str) -> serde_json::Value {
    serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 12345,
        "message_id": 111,
        "raw_message": text,
        "message": [
            {"type": "text", "data": {"text": text}}
        ]
    })
}

fn onebot_group_text_native(text: &str) -> serde_json::Value {
    serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "user_id": 12345,
        "group_id": 67890,
        "message_id": 222,
        "raw_message": text,
        "message": [
            {"type": "text", "data": {"text": text}}
        ]
    })
}

#[test]
fn selected_adapters_normalize_private_text_requests() {
    let cases: Vec<PrivateTextCase> = vec![
        (
            "dingtalk",
            Box::new(DingTalkChannel::new()),
            dingtalk_private_text_native("hello dingtalk"),
            "user-100",
            "conv-p2p-1",
            "hello dingtalk",
        ),
        (
            "feishu",
            Box::new(FeishuChannel::new(FeishuConfig::default())),
            feishu_private_text_native("hello feishu"),
            "ou_test_user",
            "oc_test_chat",
            "hello feishu",
        ),
        (
            "mattermost",
            Box::new(MattermostChannel::new(MattermostConfig::default())),
            mattermost_direct_text_native("hello mattermost"),
            "user-456",
            "ch-123",
            "hello mattermost",
        ),
        (
            "telegram",
            Box::new(TelegramChannel::new(TelegramConfig::default())),
            telegram_private_text_native(),
            "200",
            "100",
            "hello telegram",
        ),
        (
            "onebot",
            Box::new(OneBotChannel::new(OneBotConfig::default())),
            onebot_private_text_native("hello onebot"),
            "12345",
            "private_12345",
            "hello onebot",
        ),
        (
            "qq",
            Box::new(QqChannel::new(String::new(), String::new(), false)),
            qq_private_text_native("hello qq"),
            "user_abc",
            "user_abc",
            "hello qq",
        ),
    ];

    for (channel_id, channel, native, expected_user, expected_conversation, expected_text) in cases
    {
        let req = channel
            .build_request_from_native(native)
            .unwrap_or_else(|err| panic!("{channel_id} failed to normalize native request: {err}"));

        assert_eq!(req.channel_id, channel_id);
        assert_eq!(req.user_id, expected_user);
        assert_eq!(req.conversation_id, expected_conversation);
        assert_eq!(req.input.len(), 1);

        match &req.input[0] {
            IncomingContentPart::Text { text } => assert_eq!(text, expected_text),
            other => panic!("{channel_id} normalized to non-text input: {other:?}"),
        }
    }
}

#[test]
fn selected_adapters_preserve_group_or_thread_routing() {
    let dingtalk = DingTalkChannel::new()
        .build_request_from_native(dingtalk_group_text_native("@bot-abc group hello"))
        .expect("dingtalk group normalization");
    assert_eq!(dingtalk.conversation_id, "conv-grp-1");
    assert_eq!(dingtalk.session_id, "conv-grp-1");
    assert_eq!(dingtalk.metadata["conversation_type"], "2");

    let feishu = FeishuChannel::new(FeishuConfig::default())
        .build_request_from_native(feishu_group_text_native("hello feishu group"))
        .expect("feishu group normalization");
    assert_eq!(feishu.conversation_id, "oc_group_123");
    assert_eq!(feishu.session_id, "oc_group_123");
    assert_eq!(feishu.metadata["feishu_chat_type"], "group");

    let telegram = TelegramChannel::new(TelegramConfig::default())
        .build_request_from_native(serde_json::json!({
            "update_id": 100,
            "message": {
                "message_id": 5,
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
        }))
        .expect("telegram group normalization");
    assert_eq!(telegram.conversation_id, "-1001234");
    assert_eq!(telegram.session_id, "-1001234");
    assert_eq!(telegram.metadata["chat_type"], "supergroup");

    let onebot = OneBotChannel::new(OneBotConfig::default())
        .build_request_from_native(onebot_group_text_native("hello onebot group"))
        .expect("onebot group normalization");
    assert_eq!(onebot.conversation_id, "group_67890");
    assert_eq!(onebot.metadata["group_id"], 67890);
    assert_eq!(onebot.metadata["message_type"], "group");

    let qq = QqChannel::new("app123".into(), "secret".into(), false)
        .build_request_from_native(qq_group_text_native("hello qq group"))
        .expect("qq group normalization");
    assert_eq!(qq.conversation_id, "group:grp_300");
    assert_eq!(qq.session_id, "group:grp_300");
    assert_eq!(qq.metadata["group_openid"], "grp_300");
    assert_eq!(qq.metadata["message_type"], "group");

    let mattermost = MattermostChannel::new(MattermostConfig::default())
        .build_request_from_native(mattermost_thread_text_native(
            "thread reply",
            "root-abc",
            "post-002",
        ))
        .expect("mattermost thread normalization");
    assert_eq!(mattermost.conversation_id, "ch-789");
    assert_eq!(mattermost.session_id, "mattermost_thread:root-abc");
    assert_eq!(mattermost.metadata["root_id"], "root-abc");

    let mattermost_new_thread = MattermostChannel::new(MattermostConfig::default())
        .build_request_from_native(mattermost_thread_text_native("@bot hello", "", "post-new"))
        .expect("mattermost new thread normalization");
    assert_eq!(
        mattermost_new_thread.session_id,
        "mattermost_thread:post-new"
    );
    assert_eq!(mattermost_new_thread.metadata["root_id"], "post-new");
}

#[test]
fn selected_adapters_normalize_empty_or_non_text_inputs_without_fake_text() {
    let dingtalk = DingTalkChannel::new()
        .build_request_from_native(dingtalk_private_text_native("  "))
        .expect("dingtalk empty text normalization");
    assert!(dingtalk.input.is_empty());

    let feishu = FeishuChannel::new(FeishuConfig::default())
        .build_request_from_native(feishu_private_text_native("  "));
    assert!(
        feishu.is_err(),
        "feishu empty user messages should be skipped before agent dispatch"
    );

    let feishu_image = FeishuChannel::new(FeishuConfig::default())
        .build_request_from_native(feishu_image_native())
        .expect("feishu image normalization");
    match &feishu_image.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "[image: img_xxx]"),
        other => panic!("feishu image normalized to non-text input: {other:?}"),
    }

    let mattermost = MattermostChannel::new(MattermostConfig::default())
        .build_request_from_native(mattermost_direct_text_native(""))
        .expect("mattermost empty text normalization");
    assert!(mattermost.input.is_empty());

    let telegram = TelegramChannel::new(TelegramConfig::default()).build_request_from_native(
        serde_json::json!({
            "update_id": 1,
            "message": {
                "message_id": 1,
                "chat": { "id": 100, "type": "private" },
                "from": { "id": 200, "is_bot": false },
                "text": ""
            }
        }),
    );
    assert!(telegram.is_err());
}

#[test]
fn feishu_inbound_media_uses_typed_parts_when_local_media_is_available() {
    let cases = vec![
        (
            "image",
            serde_json::json!({
                "image_key": "img_xxx",
                "local_path": "/tmp/rua-media/photo.png",
                "mime": "image/png"
            }),
            "img_xxx",
            "/tmp/rua-media/photo.png",
            None,
        ),
        (
            "file",
            serde_json::json!({
                "file_key": "file_xxx",
                "file_name": "report.pdf",
                "local_path": "/tmp/rua-media/report.pdf",
                "mime": "application/pdf"
            }),
            "file_xxx",
            "/tmp/rua-media/report.pdf",
            Some("report.pdf"),
        ),
        (
            "audio",
            serde_json::json!({
                "file_key": "audio_xxx",
                "file_name": "voice.ogg",
                "local_path": "/tmp/rua-media/voice.ogg",
                "mime": "audio/ogg"
            }),
            "audio_xxx",
            "/tmp/rua-media/voice.ogg",
            Some("voice.ogg"),
        ),
        (
            "media",
            serde_json::json!({
                "file_key": "video_xxx",
                "file_name": "clip.mp4",
                "local_path": "/tmp/rua-media/clip.mp4",
                "mime": "video/mp4"
            }),
            "video_xxx",
            "/tmp/rua-media/clip.mp4",
            Some("clip.mp4"),
        ),
    ];

    for (message_type, content, expected_file_id, expected_path, expected_name) in cases {
        let channel = FeishuChannel::new(FeishuConfig::default());
        let req = channel
            .build_request_from_native(feishu_media_native(message_type, content))
            .unwrap_or_else(|err| panic!("feishu {message_type} failed: {err}"));

        assert_eq!(req.input.len(), 1);
        match (&req.input[0], message_type) {
            (
                IncomingContentPart::Image {
                    file_id,
                    path,
                    mime,
                },
                "image",
            ) => {
                assert_eq!(file_id, expected_file_id);
                assert_eq!(path.to_string_lossy(), expected_path);
                assert_eq!(mime.as_deref(), Some("image/png"));
            }
            (
                IncomingContentPart::File {
                    file_id,
                    path,
                    mime,
                    name,
                },
                "file",
            ) => {
                assert_eq!(file_id, expected_file_id);
                assert_eq!(path.to_string_lossy(), expected_path);
                assert_eq!(mime.as_deref(), Some("application/pdf"));
                assert_eq!(name.as_deref(), expected_name);
            }
            (
                IncomingContentPart::Audio {
                    file_id,
                    path,
                    mime,
                    name,
                },
                "audio",
            ) => {
                assert_eq!(file_id, expected_file_id);
                assert_eq!(path.to_string_lossy(), expected_path);
                assert_eq!(mime.as_deref(), Some("audio/ogg"));
                assert_eq!(name.as_deref(), expected_name);
            }
            (
                IncomingContentPart::Video {
                    file_id,
                    path,
                    mime,
                    name,
                },
                "media",
            ) => {
                assert_eq!(file_id, expected_file_id);
                assert_eq!(path.to_string_lossy(), expected_path);
                assert_eq!(mime.as_deref(), Some("video/mp4"));
                assert_eq!(name.as_deref(), expected_name);
            }
            (other, _) => panic!("unexpected feishu media part for {message_type}: {other:?}"),
        }
    }
}

#[test]
fn selected_adapters_reject_non_message_native_payloads() {
    let non_message = serde_json::json!({
        "post_type": "notice",
        "notice_type": "group_increase"
    });
    let cases: Vec<(&str, Box<dyn Channel>, serde_json::Value)> = vec![
        (
            "telegram",
            Box::new(TelegramChannel::new(TelegramConfig::default())),
            serde_json::json!({"update_id": 1}),
        ),
        (
            "onebot",
            Box::new(OneBotChannel::new(OneBotConfig::default())),
            non_message,
        ),
    ];

    for (channel_id, channel, native) in cases {
        assert!(
            channel.build_request_from_native(native).is_err(),
            "{channel_id} must reject native payloads that are not messages"
        );
    }
}
