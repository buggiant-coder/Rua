use rua_channel::core::channel::Channel;
use rua_channel::onebot::{OneBotChannel, OneBotConfig};

#[test]
fn build_request_from_native_private_message() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 12345,
        "message_id": 111,
        "raw_message": "hello world",
        "message": [
            {"type": "text", "data": {"text": "hello world"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.channel_id, "onebot");
    assert_eq!(req.user_id, "12345");
    assert_eq!(req.conversation_id, "private_12345");
    assert_eq!(req.message_id, Some("111".to_string()));
    assert_eq!(req.input.len(), 1);
    assert_eq!(req.metadata["sender_id"], "12345");
    assert_eq!(req.metadata["is_group"], false);

    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "hello world");
        }
        _ => panic!("expected Text part"),
    }
}

#[test]
fn build_request_from_native_rejects_duplicate_message_id() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 12345,
        "message_id": 111,
        "raw_message": "hello world",
        "message": [
            {"type": "text", "data": {"text": "hello world"}}
        ]
    });

    channel
        .build_request_from_native(native.clone())
        .expect("first message should parse");
    let err = channel
        .build_request_from_native(native)
        .expect_err("duplicate message should be rejected");

    assert!(
        err.to_string().contains("duplicate onebot event"),
        "unexpected duplicate error: {err}"
    );
}

#[test]
fn build_request_from_native_group_message() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "user_id": 12345,
        "group_id": 67890,
        "message_id": 222,
        "raw_message": "group msg",
        "message": [
            {"type": "text", "data": {"text": "group msg"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.conversation_id, "group_67890");
    assert_eq!(req.metadata["group_id"], 67890);
    assert_eq!(req.metadata["message_type"], "group");
    assert_eq!(req.metadata["sender_id"], "12345");
    assert_eq!(req.metadata["is_group"], true);
}

#[test]
fn build_request_from_native_non_message_returns_none_error() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "notice",
        "notice_type": "group_increase",
    });

    let result = channel.build_request_from_native(native);
    assert!(result.is_err());
}

#[test]
fn build_request_from_native_extract_from_message_array() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 99,
        "raw_message": "",
        "message": [
            {"type": "text", "data": {"text": "part1"}},
            {"type": "face", "data": {"id": "5"}},
            {"type": "text", "data": {"text": "part2"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Text { text } => {
            assert_eq!(text, "part1part2");
        }
        _ => panic!("expected Text"),
    }
}

#[test]
fn build_request_from_native_skips_empty_or_whitespace_only_messages() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    for message in [
        serde_json::json!("   "),
        serde_json::json!([]),
        serde_json::json!([
            {"type": "text", "data": {"text": "  "}},
            {"type": "face", "data": {"id": "178"}}
        ]),
    ] {
        let native = serde_json::json!({
            "post_type": "message",
            "message_type": "private",
            "user_id": 99,
            "message_id": 333,
            "message": message
        });

        let result = channel.build_request_from_native(native);
        assert!(
            result.is_err(),
            "empty OneBot user messages should be skipped before agent dispatch"
        );
    }
}

#[test]
fn build_request_from_native_marks_group_bot_mentions_from_at_segments() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "group",
        "self_id": 99999,
        "user_id": 12345,
        "group_id": 67890,
        "message_id": 444,
        "message": [
            {"type": "at", "data": {"qq": "99999"}},
            {"type": "text", "data": {"text": "hello bot"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(req.metadata["mentioned"], true);
    assert_eq!(req.metadata["bot_mentioned"], true);
}

#[test]
fn channel_id_is_onebot() {
    let channel = OneBotChannel::new(OneBotConfig::default());
    assert_eq!(channel.id(), "onebot");
}
