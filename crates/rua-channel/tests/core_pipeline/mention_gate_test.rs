use rua_channel::core::config::BaseChannelConfig;
use rua_channel::core::content::{AgentChannelRequest, IncomingContentPart};
use rua_channel::core::mention;

fn make_req(metadata: serde_json::Value) -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "agent-1".into(),
        user_id: "user-1".into(),
        session_id: "session-1".into(),
        input: vec![IncomingContentPart::Text {
            text: "hello".into(),
        }],
        channel_id: "test".into(),
        conversation_id: "conv-1".into(),
        message_id: None,
        metadata,
    }
}

fn cfg_mention(require: bool) -> BaseChannelConfig {
    BaseChannelConfig {
        require_mention: require,
        ..Default::default()
    }
}

#[test]
fn no_require_mention_always_responds() {
    let cfg = cfg_mention(false);
    let req = make_req(serde_json::json!({"is_group": true}));
    assert!(mention::should_respond(&req, &cfg));
}

#[test]
fn dm_always_responds_even_with_require_mention() {
    let cfg = cfg_mention(true);
    let req = make_req(serde_json::json!({"is_group": false}));
    assert!(mention::should_respond(&req, &cfg));
}

#[test]
fn dm_no_metadata_responds() {
    let cfg = cfg_mention(true);
    let req = make_req(serde_json::json!({}));
    assert!(mention::should_respond(&req, &cfg));
}

#[test]
fn group_require_mention_mentioned_responds() {
    let cfg = cfg_mention(true);
    let req = make_req(serde_json::json!({"is_group": true, "mentioned": true}));
    assert!(mention::should_respond(&req, &cfg));
}

#[test]
fn group_require_mention_bot_mentioned_alias_responds() {
    let cfg = cfg_mention(true);
    let req = make_req(serde_json::json!({"is_group": true, "bot_mentioned": true}));
    assert!(mention::should_respond(&req, &cfg));
}

#[test]
fn group_require_mention_bot_command_responds() {
    let cfg = cfg_mention(true);
    let req = make_req(serde_json::json!({"is_group": true, "has_bot_command": true}));
    assert!(mention::should_respond(&req, &cfg));
}

#[test]
fn group_require_mention_not_mentioned_discards() {
    let cfg = cfg_mention(true);
    let req = make_req(serde_json::json!({"is_group": true, "mentioned": false}));
    assert!(!mention::should_respond(&req, &cfg));
}

#[test]
fn group_require_mention_no_mentioned_key_discards() {
    let cfg = cfg_mention(true);
    let req = make_req(serde_json::json!({"is_group": true}));
    assert!(!mention::should_respond(&req, &cfg));
}

#[test]
fn is_group_from_is_group_flag() {
    let req = make_req(serde_json::json!({"is_group": true}));
    assert!(mention::is_group_message(&req));
}

#[test]
fn is_not_group_from_is_group_flag() {
    let req = make_req(serde_json::json!({"is_group": false}));
    assert!(!mention::is_group_message(&req));
}

#[test]
fn is_group_from_conversation_type_2() {
    let req = make_req(serde_json::json!({"conversation_type": "2"}));
    assert!(mention::is_group_message(&req));
}

#[test]
fn is_group_from_chat_type_group() {
    let req = make_req(serde_json::json!({"chat_type": "group"}));
    assert!(mention::is_group_message(&req));
}

#[test]
fn is_group_from_chat_type_group_case_insensitive() {
    let req = make_req(serde_json::json!({"chat_type": "GROUP"}));
    assert!(mention::is_group_message(&req));
}

#[test]
fn is_group_from_chat_type_supergroup() {
    let req = make_req(serde_json::json!({"chat_type": "supergroup"}));
    assert!(mention::is_group_message(&req));
}

#[test]
fn is_not_group_from_conversation_type_1() {
    let req = make_req(serde_json::json!({"conversation_type": "1"}));
    assert!(!mention::is_group_message(&req));
}

#[test]
fn is_not_group_no_metadata() {
    let req = make_req(serde_json::json!({}));
    assert!(!mention::is_group_message(&req));
}
