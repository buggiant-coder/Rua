#[path = "../voice_test_helpers.rs"]
mod voice_test_helpers;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use rua_channel::core::channel::{Channel, Process};
use rua_channel::core::content::{
    ChannelTarget, IncomingContentPart, OutgoingContentPart, OutgoingMessage,
};
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::{Result, RuaError};
use rua_voice::{VoiceChannel, VoiceManager};
use std::pin::Pin;
use std::sync::Arc;
use tempfile::TempDir;

struct StubRuntime;

#[async_trait]
impl AgentRuntime for StubRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        Ok(Box::pin(stream::empty()))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> Result<()> {
        Ok(())
    }

    async fn warmup(&self, _ctx: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

struct NoopProcess;

#[async_trait]
impl Process for NoopProcess {
    async fn handle(
        &self,
        _req: rua_channel::core::channel::AgentRequest,
    ) -> BoxStream<'static, RuntimeEvent> {
        Box::pin(stream::empty())
    }
}

async fn channel(enabled: bool) -> (VoiceChannel, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let secret_dir = tmp.path().join("secret");
    std::fs::create_dir_all(&data_dir).expect("data dir");
    rua_config::save_config(
        &rua_config::RootConfig::default(),
        &data_dir.join("config.json"),
    )
    .unwrap();
    std::fs::create_dir_all(&secret_dir).expect("secret dir");
    let runtime: Arc<dyn AgentRuntime> = Arc::new(StubRuntime);
    let registry =
        voice_test_helpers::StubRegistry::with_runtime("default", tmp.path(), runtime).await;
    let manager = Arc::new(
        VoiceManager::with_dirs(
            "test-secret".into(),
            data_dir,
            secret_dir,
            registry,
            "default".into(),
        )
        .expect("valid test configuration"),
    );
    manager.set_enabled(enabled);
    (VoiceChannel::new(manager), tmp)
}

fn target() -> ChannelTarget {
    ChannelTarget {
        agent_id: "default".into(),
        conversation_id: "CA-voice-001".into(),
        sender_id: Some("+15550000001".into()),
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

fn outgoing(parts: Vec<OutgoingContentPart>) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "voice".into(),
        conversation_id: "CA-voice-001".into(),
        receiver_id: Some("+15550000001".into()),
        content: parts,
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

#[tokio::test]
async fn voice_channel_exposes_stable_id_and_restartable_lifecycle() {
    let (channel, _tmp) = channel(false).await;

    assert_eq!(channel.id(), "voice");
    channel.start(Arc::new(NoopProcess)).await.expect("start");
    channel.stop().await.expect("stop");
}

#[tokio::test]
async fn voice_channel_send_is_best_effort_for_disabled_or_non_text_messages() {
    let (disabled, _tmp) = channel(false).await;
    disabled
        .send(
            target(),
            outgoing(vec![OutgoingContentPart::Text {
                text: "hello caller".into(),
            }]),
        )
        .await
        .expect("disabled voice channel skips outbound instead of failing");

    let (enabled, _tmp) = channel(true).await;
    enabled
        .send(
            target(),
            outgoing(vec![OutgoingContentPart::Image {
                url: Some("file:///tmp/image.png".into()),
                path: None,
                alt: None,
            }]),
        )
        .await
        .expect("media-only voice output is ignored instead of failing");
    enabled
        .send(
            target(),
            outgoing(vec![
                OutgoingContentPart::Markdown {
                    text: "**hello**".into(),
                },
                OutgoingContentPart::Refusal {
                    text: "cannot do that".into(),
                },
            ]),
        )
        .await
        .expect_err("an absent call cannot receive text");
}

#[tokio::test]
async fn twilio_native_payload_maps_to_voice_agent_request() {
    let (channel, _tmp) = channel(false).await;
    let req = channel
        .build_request_from_native(serde_json::json!({
            "CallSid": "CA-native-001",
            "From": "+15550000001",
            "SpeechResult": "Hello from Twilio"
        }))
        .expect("voice native payload");

    assert_eq!(req.channel_id, "voice");
    assert_eq!(req.session_id, "CA-native-001");
    assert_eq!(req.conversation_id, "CA-native-001");
    assert_eq!(req.user_id, "+15550000001");
    assert_eq!(req.metadata["call_sid"], "CA-native-001");
    assert_eq!(
        req.input,
        vec![IncomingContentPart::Text {
            text: "Hello from Twilio".into()
        }]
    );
}

#[tokio::test]
async fn twilio_native_payload_without_call_sid_uses_sender_conversation_and_empty_input() {
    let (channel, _tmp) = channel(false).await;
    let req = channel
        .build_request_from_native(serde_json::json!({
            "From": "+15550000002",
            "SpeechResult": ""
        }))
        .expect("voice native payload");

    assert_eq!(req.channel_id, "voice");
    assert_eq!(req.session_id, "+15550000002");
    assert_eq!(req.conversation_id, "+15550000002");
    assert_eq!(req.user_id, "+15550000002");
    assert_eq!(req.input, Vec::<IncomingContentPart>::new());
    assert_eq!(req.metadata["call_sid"], "");
}

#[tokio::test]
async fn qwenpaw_style_voice_payload_aliases_map_to_rust_request() {
    let (channel, _tmp) = channel(false).await;
    let req = channel
        .build_request_from_native(serde_json::json!({
            "session_id": "session_123",
            "from_number": "+1234567890",
            "transcript": ""
        }))
        .expect("voice native payload");

    assert_eq!(req.channel_id, "voice");
    assert_eq!(req.session_id, "session_123");
    assert_eq!(req.conversation_id, "session_123");
    assert_eq!(req.user_id, "+1234567890");
    assert_eq!(req.metadata["call_sid"], "session_123");
    assert_eq!(
        req.input,
        vec![IncomingContentPart::Text {
            text: String::new()
        }],
        "explicit QwenPaw-style transcript payloads preserve empty transcript text"
    );
}
