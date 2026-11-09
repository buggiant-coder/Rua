use std::sync::Arc;

use async_trait::async_trait;

use rua_channel::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use rua_channel::core::content::IncomingContentPart;
use rua_channel::core::ChannelId;
use rua_core::Result;

use crate::VoiceManager;

pub struct VoiceChannel {
    channel_id: ChannelId,
    manager: Arc<VoiceManager>,
}

impl VoiceChannel {
    pub fn new(manager: Arc<VoiceManager>) -> Self {
        Self {
            channel_id: "voice".into(),
            manager,
        }
    }
}

#[async_trait]
impl channel::Channel for VoiceChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, _process: Arc<dyn Process>) -> Result<()> {
        tracing::info!("voice channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.manager.stop_calls().await;
        tracing::info!("voice channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        let text = render_voice_content(&msg.content);
        if text.is_empty() {
            return Ok(());
        }
        if !self.manager.is_enabled() {
            tracing::debug!("voice channel disabled, skipping outbound");
            return Ok(());
        }
        self.manager.send_to_call(&to.conversation_id, text).await
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        let call_sid = native
            .get("CallSid")
            .or_else(|| native.get("session_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let from = native
            .get("From")
            .or_else(|| native.get("from_number"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let has_compat_transcript = native.get("transcript").is_some();
        let transcript = native
            .get("SpeechResult")
            .or_else(|| native.get("transcript"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let input = if transcript.is_empty() && !has_compat_transcript {
            vec![]
        } else {
            vec![IncomingContentPart::Text { text: transcript }]
        };

        let conversation_id = if call_sid.is_empty() {
            from.clone()
        } else {
            call_sid.clone()
        };

        Ok(AgentRequest {
            agent_id: String::new(),
            user_id: from,
            session_id: conversation_id.clone(),
            input,
            channel_id: "voice".into(),
            conversation_id,
            message_id: None,
            metadata: serde_json::json!({
                "call_sid": call_sid,
            }),
        })
    }
}

fn render_voice_content(parts: &[rua_channel::core::content::OutgoingContentPart]) -> String {
    let mut segments = Vec::new();
    for part in parts {
        match part {
            rua_channel::core::content::OutgoingContentPart::Text { text } => {
                segments.push(text.clone())
            }
            rua_channel::core::content::OutgoingContentPart::Markdown { text } => {
                segments.push(text.clone())
            }
            rua_channel::core::content::OutgoingContentPart::Refusal { text } => {
                segments.push(text.clone())
            }
            _ => {}
        }
    }
    segments.join("\n")
}
