use rua_channel::dingtalk::mention::{process_group_mention, strip_at_prefix, MentionConfig};
use rua_channel::dingtalk::DingTalkWebhookPayload;

fn group_payload_with_at(bot_id: &str, text: &str) -> DingTalkWebhookPayload {
    let json = serde_json::json!({
        "msgType": "text",
        "msgId": "msg-g01",
        "conversationId": "conv-group-1",
        "conversationType": "2",
        "senderId": "user-001",
        "senderNick": "Alice",
        "robotCode": bot_id,
        "isInAtList": true,
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xyz",
        "text": { "content": text },
        "atUsers": [
            { "dingtalkId": "user-001" },
            { "dingtalkId": bot_id }
        ]
    });
    serde_json::from_value(json).unwrap()
}

fn group_payload_without_at(text: &str) -> DingTalkWebhookPayload {
    let json = serde_json::json!({
        "msgType": "text",
        "msgId": "msg-g02",
        "conversationId": "conv-group-2",
        "conversationType": "2",
        "senderId": "user-002",
        "senderNick": "Bob",
        "robotCode": "bot-abc",
        "isInAtList": false,
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=abc",
        "text": { "content": text },
        "atUsers": [
            { "dingtalkId": "user-003" }
        ]
    });
    serde_json::from_value(json).unwrap()
}

fn single_chat_payload(text: &str) -> DingTalkWebhookPayload {
    let json = serde_json::json!({
        "msgType": "text",
        "msgId": "msg-s01",
        "conversationId": "conv-single-1",
        "conversationType": "1",
        "senderId": "user-010",
        "senderNick": "Carol",
        "robotCode": "bot-abc",
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=s1",
        "text": { "content": text }
    });
    serde_json::from_value(json).unwrap()
}

#[test]
fn at_bot_strips_prefix_and_extracts_content() {
    let payload = group_payload_with_at("bot-abc", "@user1 @bot-abc 帮我查一下");
    let config = MentionConfig {
        bot_id: "bot-abc".into(),
        respond_to_unmentioned: false,
    };

    let result = process_group_mention(&payload, &config);

    assert!(result.is_group_chat);
    assert!(result.should_respond);
    assert_eq!(result.content, "帮我查一下");
}

#[test]
fn unmentioned_group_messages_follow_policy() {
    let payload = group_payload_without_at("some random message");

    let ignore = process_group_mention(
        &payload,
        &MentionConfig {
            bot_id: "bot-abc".into(),
            respond_to_unmentioned: false,
        },
    );
    assert!(ignore.is_group_chat);
    assert!(!ignore.should_respond);
    assert_eq!(ignore.content, "some random message");

    let accept = process_group_mention(
        &payload,
        &MentionConfig {
            bot_id: "bot-abc".into(),
            respond_to_unmentioned: true,
        },
    );
    assert!(accept.should_respond);
    assert_eq!(accept.content, "some random message");
}

#[test]
fn single_chat_always_responds() {
    let payload = single_chat_payload("hello bot");
    let config = MentionConfig {
        bot_id: "bot-abc".into(),
        respond_to_unmentioned: false,
    };

    let result = process_group_mention(&payload, &config);

    assert!(!result.is_group_chat);
    assert!(result.should_respond);
    assert_eq!(result.content, "hello bot");
}

#[test]
fn strip_at_prefix_handles_dingtalk_text_variants() {
    assert_eq!(strip_at_prefix("@bot hello"), "hello");
    assert_eq!(strip_at_prefix("@user1 @bot 帮我查一下"), "帮我查一下");
    assert_eq!(strip_at_prefix("plain text"), "plain text");
    assert_eq!(strip_at_prefix("@bot"), "@bot");
    assert_eq!(strip_at_prefix("@user1   @bot   请问一下"), "请问一下");
}

#[test]
fn empty_bot_id_falls_back_to_is_in_at_list() {
    let payload = group_payload_with_at("bot-abc", "@bot-abc hello");
    let config = MentionConfig {
        bot_id: String::new(),
        respond_to_unmentioned: false,
    };

    assert!(process_group_mention(&payload, &config).should_respond);
}

#[test]
fn staff_id_mentions_are_bot_mentions() {
    let json = serde_json::json!({
        "msgType": "text",
        "msgId": "msg-g03",
        "conversationId": "conv-group-3",
        "conversationType": "2",
        "senderId": "user-005",
        "senderNick": "Dave",
        "robotCode": "bot-xyz",
        "isInAtList": false,
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=d",
        "text": { "content": "@bot-xyz what is this" },
        "atUsers": [
            { "dingtalkId": "other", "staffId": "bot-xyz" }
        ]
    });
    let payload = serde_json::from_value(json).unwrap();
    let config = MentionConfig {
        bot_id: "bot-xyz".into(),
        respond_to_unmentioned: false,
    };

    assert!(process_group_mention(&payload, &config).should_respond);
}
