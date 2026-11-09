use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use futures::Stream;

use rua_channel::core::channel::Process;
use rua_channel::core::content::{AgentChannelRequest, IncomingContentPart};
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::{Result, RuaError};
use rua_workspace::channel_process_adapter::ChannelProcessAdapter;

struct MockRuntime {
    response_text: String,
}

#[async_trait]
impl AgentRuntime for MockRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        let text = self.response_text.clone();
        let events: Vec<std::result::Result<RuntimeEvent, RuaError>> = vec![
            Ok(RuntimeEvent::Start {
                session_id: "s1".into(),
                agent_id: Some("a1".into()),
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: text,
                message_id: None,
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            }),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn cancel(&self, _session_id: &SessionId) -> Result<()> {
        Ok(())
    }
    async fn warmup(&self, _context: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

fn make_text_request(text: &str) -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "test-agent".into(),
        user_id: "user-1".to_string(),
        session_id: "session-1".to_string(),
        input: vec![IncomingContentPart::Text {
            text: text.to_string(),
        }],
        channel_id: "dingtalk".into(),
        conversation_id: "conv-1".into(),
        message_id: None,
        metadata: serde_json::Value::Null,
    }
}

#[tokio::test]
async fn adapter_forwards_text_to_runtime_and_streams_events() {
    let runtime = Arc::new(MockRuntime {
        response_text: "hello from agent".to_string(),
    });
    let adapter = ChannelProcessAdapter::new(runtime);

    let req = make_text_request("hi");
    let events: Vec<RuntimeEvent> = adapter.handle(req).await.collect().await;

    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0], RuntimeEvent::Start { .. }));
    assert!(matches!(
        &events[1],
        RuntimeEvent::TextDelta { delta, .. } if delta == "hello from agent"
    ));
    assert!(matches!(&events[2], RuntimeEvent::End { .. }));
}

struct CaptureRuntime {
    seen: Mutex<Option<AgentRequest>>,
}

#[async_trait]
impl AgentRuntime for CaptureRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        *self.seen.lock().expect("capture mutex poisoned") = Some(req);
        Ok(Box::pin(stream::iter(vec![Ok(RuntimeEvent::End {
            finish_reason: Some("stop".into()),
        })])))
    }

    async fn cancel(&self, _session_id: &SessionId) -> Result<()> {
        Ok(())
    }
    async fn warmup(&self, _context: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn adapter_preserves_audio_and_video_channel_inputs() {
    let runtime = Arc::new(CaptureRuntime {
        seen: Mutex::new(None),
    });
    let adapter = ChannelProcessAdapter::new(runtime.clone());
    let req = AgentChannelRequest {
        agent_id: "test-agent".into(),
        user_id: "user-1".to_string(),
        session_id: "session-1".to_string(),
        input: vec![
            IncomingContentPart::Audio {
                file_id: "aud-1".into(),
                path: PathBuf::from("/tmp/voice.ogg"),
                mime: Some("audio/ogg".into()),
                name: Some("voice.ogg".into()),
            },
            IncomingContentPart::Video {
                file_id: "vid-1".into(),
                path: PathBuf::from("/tmp/clip.mp4"),
                mime: Some("video/mp4".into()),
                name: Some("clip.mp4".into()),
            },
        ],
        channel_id: "telegram".into(),
        conversation_id: "conv-1".into(),
        message_id: None,
        metadata: serde_json::Value::Null,
    };

    let _events: Vec<RuntimeEvent> = adapter.handle(req).await.collect().await;
    let captured = runtime
        .seen
        .lock()
        .expect("capture mutex poisoned")
        .take()
        .expect("runtime request captured");
    let content = &captured.messages[0].content;

    assert!(matches!(
        &content[0],
        agent_types::ContentPart::Audio { url, media_type }
            if url == "file:///tmp/voice.ogg" && media_type.as_deref() == Some("audio/ogg")
    ));
    assert!(matches!(
        &content[1],
        agent_types::ContentPart::Video { url, media_type }
            if url == "file:///tmp/clip.mp4" && media_type.as_deref() == Some("video/mp4")
    ));
}

struct ErrorRuntime;

#[async_trait]
impl AgentRuntime for ErrorRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        Err(RuaError::Runtime {
            message: "runtime exploded".into(),
        })
    }

    async fn cancel(&self, _session_id: &SessionId) -> Result<()> {
        Ok(())
    }
    async fn warmup(&self, _context: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn adapter_converts_runtime_error_to_error_event() {
    let adapter = ChannelProcessAdapter::new(Arc::new(ErrorRuntime));

    let req = make_text_request("hi");
    let events: Vec<RuntimeEvent> = adapter.handle(req).await.collect().await;

    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], RuntimeEvent::Error { .. }));
}

struct StreamErrorRuntime;

#[async_trait]
impl AgentRuntime for StreamErrorRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        let events: Vec<std::result::Result<RuntimeEvent, RuaError>> = vec![
            Ok(RuntimeEvent::Start {
                session_id: "s1".into(),
                agent_id: None,
            }),
            Err(RuaError::Runtime {
                message: "mid-stream failure".into(),
            }),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn cancel(&self, _session_id: &SessionId) -> Result<()> {
        Ok(())
    }
    async fn warmup(&self, _context: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn adapter_maps_stream_item_errors_to_error_events() {
    let adapter = ChannelProcessAdapter::new(Arc::new(StreamErrorRuntime));

    let req = make_text_request("hi");
    let events: Vec<RuntimeEvent> = adapter.handle(req).await.collect().await;

    assert_eq!(events.len(), 2);
    assert!(matches!(&events[0], RuntimeEvent::Start { .. }));
    assert!(matches!(&events[1], RuntimeEvent::Error { .. }));
}
