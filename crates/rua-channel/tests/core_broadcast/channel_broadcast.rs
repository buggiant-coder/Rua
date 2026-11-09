use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rua_channel::core::{
    AgentRequest, Channel, ChannelManager, OutMessage, OutgoingContentPart, Process, ToHandle,
};

struct RecordingChannel {
    id: String,
    messages: Mutex<Vec<OutMessage>>,
    fail: bool,
}

impl RecordingChannel {
    fn new(id: &str, fail: bool) -> Arc<Self> {
        Arc::new(Self {
            id: id.into(),
            messages: Mutex::new(Vec::new()),
            fail,
        })
    }
}

#[async_trait]
impl Channel for RecordingChannel {
    fn id(&self) -> &String {
        &self.id
    }
    async fn start(&self, _process: Arc<dyn Process>) -> rua_core::Result<()> {
        Ok(())
    }
    async fn stop(&self) -> rua_core::Result<()> {
        Ok(())
    }
    async fn send(&self, _target: ToHandle, message: OutMessage) -> rua_core::Result<()> {
        self.messages.lock().unwrap().push(message);
        if self.fail {
            return Err(rua_core::RuaError::Validation {
                message: "send failed".into(),
            });
        }
        Ok(())
    }
    fn build_request_from_native(
        &self,
        _native: serde_json::Value,
    ) -> rua_core::Result<AgentRequest> {
        Err(rua_core::RuaError::Validation {
            message: "outbound test channel".into(),
        })
    }
}

#[tokio::test]
async fn broadcast_text_reaches_every_registered_channel_despite_send_failure() {
    let manager = ChannelManager::new();
    let good = RecordingChannel::new("telegram", false);
    let bad = RecordingChannel::new("dingtalk", true);
    manager.register(good.clone()).await.unwrap();
    manager.register(bad.clone()).await.unwrap();

    let results = manager.broadcast_text("notification").await;
    assert_eq!(results.len(), 2);
    assert_eq!(
        results.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    assert!(results
        .iter()
        .any(|(id, result)| id == "dingtalk" && result.is_err()));
    for channel in [good, bad] {
        let messages = channel.messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert!(
            matches!(&messages[0].content[0], OutgoingContentPart::Text { text } if text == "notification")
        );
    }
}

#[tokio::test]
async fn broadcast_without_channels_is_empty() {
    assert!(ChannelManager::new()
        .broadcast_text("notification")
        .await
        .is_empty());
}

#[tokio::test]
async fn visible_events_are_sent_and_delivery_errors_are_not_swallowed() {
    let manager = ChannelManager::new();
    let channel = RecordingChannel::new("telegram", false);
    manager.register(channel.clone()).await.unwrap();
    let target = ToHandle {
        agent_id: "agent-1".into(),
        conversation_id: "chat-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({}),
    };
    manager
        .send_stream_event(
            "telegram",
            target.clone(),
            rua_core::RuntimeEvent::TextDelta {
                delta: "visible".into(),
                message_id: None,
            },
        )
        .await
        .unwrap();
    manager
        .send_stream_event(
            "telegram",
            target.clone(),
            rua_core::RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(channel.messages.lock().unwrap().len(), 1);
    assert!(manager
        .send_stream_event(
            "telegram",
            target.clone(),
            rua_core::RuntimeEvent::ToolApprovalRequest {
                id: "request".into(),
                name: "tool".into(),
                arguments: serde_json::json!({}),
            }
        )
        .await
        .is_err());
    let failed = RecordingChannel::new("failed", true);
    manager.register(failed).await.unwrap();
    for event in [
        rua_core::RuntimeEvent::TextDelta {
            delta: "visible".into(),
            message_id: None,
        },
        rua_core::RuntimeEvent::ToolApprovalPending {
            call_id: "call".into(),
            request_id: Some("request".into()),
            tool_name: "tool".into(),
            prompt: None,
        },
    ] {
        assert!(manager
            .send_stream_event("failed", target.clone(), event)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn targeted_text_and_approval_preserve_routing_and_metadata() {
    let manager = ChannelManager::new();
    let channel = RecordingChannel::new("telegram", false);
    manager.register(channel.clone()).await.unwrap();
    let target = ToHandle {
        agent_id: "agent-1".into(),
        conversation_id: "chat-1".into(),
        sender_id: Some("user-1".into()),
        reply_to: None,
        metadata: serde_json::json!({}),
    };
    manager
        .send_text("telegram", target.clone(), "hello".into())
        .await
        .unwrap();
    manager
        .send_stream_event(
            "telegram",
            target,
            rua_core::RuntimeEvent::ToolApprovalPending {
                call_id: "call-1".into(),
                tool_name: "example_tool".into(),
                request_id: Some("approval-1".into()),
                prompt: None,
            },
        )
        .await
        .unwrap();
    let messages = channel.messages.lock().unwrap();
    assert_eq!(messages.len(), 2);
    for message in messages.iter() {
        assert_eq!(message.channel_id, "telegram");
        assert_eq!(message.conversation_id, "chat-1");
        assert_eq!(message.receiver_id.as_deref(), Some("user-1"));
    }
    assert_eq!(messages[1].metadata["message_type"], "tool_guard_approval");
    assert_eq!(messages[1].metadata["approval_request_id"], "approval-1");
    assert_eq!(messages[1].metadata["tool_name"], "example_tool");
}
