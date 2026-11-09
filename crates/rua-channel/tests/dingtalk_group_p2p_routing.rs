use rua_channel::core::content::IncomingContentPart;
use rua_channel::dingtalk::parser::parse_with_routing;

fn p2p_payload(text: &str) -> serde_json::Value {
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

fn group_payload_with_at(bot_id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "msgType": "text",
        "msgId": "msg-grp-001",
        "conversationId": "conv-grp-1",
        "conversationType": "2",
        "senderId": "user-200",
        "senderNick": "Bob",
        "robotCode": bot_id,
        "isInAtList": true,
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=grp",
        "text": { "content": text },
        "atUsers": [
            { "dingtalkId": "user-200" },
            { "dingtalkId": bot_id }
        ]
    })
}

fn group_payload_no_at(text: &str) -> serde_json::Value {
    serde_json::json!({
        "msgType": "text",
        "msgId": "msg-grp-002",
        "conversationId": "conv-grp-2",
        "conversationType": "2",
        "senderId": "user-300",
        "senderNick": "Carol",
        "robotCode": "bot-abc",
        "isInAtList": false,
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=grp2",
        "text": { "content": text },
        "atUsers": [
            { "dingtalkId": "user-300" }
        ]
    })
}

fn group_payload_text_only_mention(bot_name: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "msgType": "text",
        "msgId": "msg-grp-003",
        "conversationId": "conv-grp-3",
        "conversationType": "2",
        "senderId": "user-400",
        "senderNick": "Dave",
        "robotCode": "bot-abc",
        "isInAtList": false,
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=grp3",
        "text": { "content": format!("@{bot_name} {text}") },
        "atUsers": [
            { "dingtalkId": "user-999" }
        ]
    })
}

#[test]
fn p2p_message_always_enqueued() {
    let result = parse_with_routing(p2p_payload("hello bot"), "bot-abc", "MyBot").unwrap();
    let req = result.expect("P2P should always produce a request");
    assert_eq!(req.conversation_id, "conv-p2p-1");
    assert_eq!(req.user_id, "user-100");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "hello bot"),
        _ => panic!("expected Text"),
    }
}

#[test]
fn p2p_empty_text_still_enqueued() {
    let result = parse_with_routing(p2p_payload("  "), "bot-abc", "MyBot").unwrap();
    let req = result.expect("P2P should enqueue even with empty text");
    assert!(req.input.is_empty());
}

#[test]
fn group_at_bot_enqueued_with_stripped_prefix() {
    let payload = group_payload_with_at("bot-abc", "@user @bot-abc 帮我查一下");
    let result = parse_with_routing(payload, "bot-abc", "MyBot").unwrap();
    let req = result.expect("group @bot should enqueue");
    assert_eq!(req.conversation_id, "conv-grp-1");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "帮我查一下"),
        _ => panic!("expected Text"),
    }
}

#[test]
fn group_no_mention_not_enqueued() {
    let payload = group_payload_no_at("random chat in group");
    let result = parse_with_routing(payload, "bot-abc", "MyBot").unwrap();
    assert!(result.is_none(), "group without @bot should skip");
}

#[test]
fn group_text_fallback_mention_enqueued() {
    let payload = group_payload_text_only_mention("MyBot", "请问一下天气");
    let result = parse_with_routing(payload, "bot-abc", "MyBot").unwrap();
    let req = result.expect("text fallback @bot_name should enqueue");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "请问一下天气"),
        _ => panic!("expected Text"),
    }
}

#[test]
fn group_at_bot_preserves_metadata() {
    let payload = group_payload_with_at("bot-abc", "@bot-abc check status");
    let result = parse_with_routing(payload, "bot-abc", "MyBot").unwrap();
    let req = result.unwrap();
    assert_eq!(req.metadata["conversation_type"], "2");
    assert_eq!(
        req.metadata["session_webhook"],
        "https://oapi.dingtalk.com/robot/sendBySession?session=grp"
    );
    assert_eq!(req.metadata["sender_nick"], "Bob");
}
