use std::sync::Arc;

use rua_channel::core::channel::Channel;
use rua_channel::core::content::{ChannelTarget, OutgoingContentPart, OutgoingMessage};
use rua_channel::core::ChannelManager;
use rua_channel::wecom::WecomChannel;
use rua_config::{ChannelCommonConfig, WecomConfig};
use rua_workspace::services::ChannelsServiceAdapter;

fn make_channel() -> WecomChannel {
    WecomChannel::new("bot-1".into(), "secret-1".into())
}

fn text_frame(content: &str) -> serde_json::Value {
    serde_json::json!({
        "cmd": "aibot_msg_callback",
        "headers": {"req_id": "req-1"},
        "body": {
            "msgtype": "text",
            "from": {"userid": "user-1", "name": "Alice"},
            "chatid": "",
            "chattype": "single",
            "msgid": "msg-1",
            "text": {"content": content}
        }
    })
}

fn group_frame() -> serde_json::Value {
    serde_json::json!({
        "cmd": "aibot_msg_callback",
        "headers": {"req_id": "req-2"},
        "body": {
            "msgtype": "text",
            "from": {"userid": "user-2", "name": "Bob"},
            "chatid": "chat-9",
            "chattype": "group",
            "msgid": "msg-2",
            "text": {"content": "hello group"}
        }
    })
}

fn outgoing_target() -> ChannelTarget {
    ChannelTarget {
        agent_id: "default".into(),
        conversation_id: "wecom:user-1".into(),
        sender_id: Some("user-1".into()),
        reply_to: None,
        metadata: serde_json::Value::Null,
    }
}

fn outgoing_message(parts: Vec<OutgoingContentPart>) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "wecom".into(),
        conversation_id: "wecom:user-1".into(),
        receiver_id: Some("user-1".into()),
        content: parts,
        reply_to: None,
        metadata: serde_json::Value::Null,
    }
}

#[test]
fn aibot_text_callback_trims_content_and_preserves_metadata() {
    let request = make_channel()
        .build_request_from_native(text_frame("  hello wecom  "))
        .unwrap();

    assert_eq!(request.channel_id, "wecom");
    assert_eq!(request.user_id, "user-1");
    assert_eq!(request.session_id, "wecom:user-1");
    assert_eq!(request.conversation_id, "");
    assert_eq!(request.message_id.as_deref(), Some("msg-1"));
    assert_eq!(request.input.len(), 1);
    assert_eq!(
        request.input[0],
        rua_channel::core::content::IncomingContentPart::Text {
            text: "hello wecom".into()
        }
    );
    assert_eq!(request.metadata["wecom_sender_id"], "user-1");
    assert_eq!(request.metadata["wecom_req_id"], "req-1");
    assert_eq!(request.metadata["user_name"], "Alice");
    assert_eq!(request.metadata["is_group"], false);
}

#[test]
fn aibot_group_callback_uses_group_session_and_empty_user_id() {
    let request = make_channel()
        .build_request_from_native(group_frame())
        .unwrap();

    assert_eq!(request.channel_id, "wecom");
    assert_eq!(request.user_id, "");
    assert_eq!(request.session_id, "wecom:group:chat-9");
    assert_eq!(request.conversation_id, "chat-9");
    assert_eq!(request.metadata["wecom_chat_type"], "group");
    assert_eq!(request.metadata["is_group"], true);
}

#[test]
fn aibot_empty_text_callback_is_rejected() {
    let err = make_channel()
        .build_request_from_native(text_frame("   "))
        .unwrap_err();

    let message = err.to_string();
    assert!(
        message.contains("no parseable content"),
        "unexpected error: {message}"
    );
}

#[tokio::test]
async fn proactive_send_requires_active_aibot_ws_sender() {
    let channel = make_channel();
    let message = outgoing_message(vec![OutgoingContentPart::Text {
        text: "hello".into(),
    }]);

    let err = channel.send(outgoing_target(), message).await.unwrap_err();
    assert!(
        err.to_string().contains("active WebSocket sender"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn incomplete_wecom_config_does_not_register_channel() {
    let manager = Arc::new(ChannelManager::new());
    let config = WecomConfig {
        enabled: true,
        bot_id: "bot-1".into(),
        secret: String::new(),
        media_dir: String::new(),
        common: ChannelCommonConfig::default(),
    };

    let adapter = ChannelsServiceAdapter::new(Arc::clone(&manager)).with_wecom_config(config);
    assert!(adapter.register_configured_channels().await.is_err());

    let channels = manager.channel_ids().await;
    assert!(
        !channels.iter().any(|id| id == "wecom"),
        "incomplete WeCom config should not register; got {channels:?}"
    );
}

#[tokio::test]
async fn valid_wecom_config_registers_aibot_channel() {
    let manager = Arc::new(ChannelManager::new());
    let config = WecomConfig {
        enabled: true,
        bot_id: "bot-1".into(),
        secret: "secret-1".into(),
        media_dir: String::new(),
        common: ChannelCommonConfig::default(),
    };

    let adapter = ChannelsServiceAdapter::new(Arc::clone(&manager)).with_wecom_config(config);
    adapter.register_configured_channels().await.unwrap();

    let channels = manager.channel_ids().await;
    assert!(
        channels.iter().any(|id| id == "wecom"),
        "WeCom should be registered; got {channels:?}"
    );
}
