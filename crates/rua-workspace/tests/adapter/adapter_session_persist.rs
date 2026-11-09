use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use futures::Stream;
use rua_channel::core::channel::Process;
use rua_channel::core::content::{AgentChannelRequest, IncomingContentPart};
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::tool::{ToolCall, ToolResult};
use rua_core::{Result, RuaError};
use rua_workspace::channel_process_adapter::ChannelProcessAdapter;
use tokio::sync::Mutex;

struct MockRuntime {
    events: Vec<RuntimeEvent>,
}

#[async_trait]
impl AgentRuntime for MockRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        let events: Vec<std::result::Result<RuntimeEvent, RuaError>> =
            self.events.iter().cloned().map(Ok).collect();
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

struct CapturingRuntime {
    captured_messages: Arc<Mutex<Option<Vec<AgentInputMessage>>>>,
}

#[async_trait]
impl AgentRuntime for CapturingRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        *self.captured_messages.lock().await = Some(req.messages.clone());
        let events: Vec<std::result::Result<RuntimeEvent, RuaError>> = vec![
            Ok(RuntimeEvent::Start {
                session_id: req.session_id,
                agent_id: Some(req.agent_id),
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
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

fn make_request(session_id: &str, agent_id: &str, text: &str) -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: agent_id.to_string(),
        user_id: "test-user".to_string(),
        session_id: session_id.to_string(),
        input: vec![IncomingContentPart::Text {
            text: text.to_string(),
        }],
        channel_id: "test-channel".to_string(),
        conversation_id: "test-conv".to_string(),
        message_id: None,
        metadata: serde_json::json!({}),
    }
}

fn message_texts(messages: &[AgentInputMessage]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| {
            message.content.iter().filter_map(|part| match part {
                rua_core::message::ContentPart::Text { text } => Some(text.clone()),
                _ => None,
            })
        })
        .collect()
}

#[tokio::test]
async fn session_persist_saves_thinking_and_tool_events() {
    let tmp = tempfile::tempdir().unwrap();
    let ws_dir = tmp.path().to_path_buf();

    let events = vec![
        RuntimeEvent::Start {
            session_id: SessionId::new("sess-1"),
            agent_id: None,
        },
        RuntimeEvent::ThinkingDelta {
            delta: "Let me think about ".to_string(),
            signature: None,
            message_id: None,
        },
        RuntimeEvent::ThinkingDelta {
            delta: "this problem.".to_string(),
            signature: None,
            message_id: None,
        },
        RuntimeEvent::ToolCall {
            call: ToolCall::new(
                "call-1",
                "read_file",
                serde_json::json!({"path": "/tmp/test.txt"}),
            ),
        },
        RuntimeEvent::ToolResult {
            result: ToolResult::text("call-1", "file contents here"),
        },
        RuntimeEvent::TextDelta {
            delta: "Here is ".to_string(),
            message_id: None,
        },
        RuntimeEvent::TextDelta {
            delta: "the answer.".to_string(),
            message_id: None,
        },
        RuntimeEvent::End {
            finish_reason: Some("stop".to_string()),
        },
    ];

    let runtime = Arc::new(MockRuntime { events });
    let adapter = ChannelProcessAdapter::new(runtime).with_workspace_dir(ws_dir.clone());

    let req = make_request("sess-1", "test-agent", "Hello");
    let stream = adapter.handle(req).await;
    let _collected: Vec<_> = stream.collect().await;

    let store = rua_session::WorkspaceSessionStore::new(&ws_dir);
    let header = store
        .load_chat_header("sess-1")
        .await
        .unwrap()
        .expect("channel persist should create Evergreen header");
    assert_eq!(header.chat_id, "sess-1");
    let messages = store.read_message_range("sess-1", 1, 2).await.unwrap();

    assert_eq!(messages.len(), 2, "should have user + assistant messages");

    let assistant_content = messages[1].content.as_array().expect("content array");

    assert!(
        assistant_content.len() >= 3,
        "should have thinking + tool_use + tool_result + text"
    );

    let thinking_part = &assistant_content[0];
    assert_eq!(thinking_part["type"], "thinking");
    assert_eq!(
        thinking_part["thinking"],
        "Let me think about this problem."
    );

    let tool_use_part = &assistant_content[1];
    assert_eq!(tool_use_part["type"], "tool_use");
    assert_eq!(tool_use_part["id"], "call-1");
    assert_eq!(tool_use_part["name"], "read_file");
    assert_eq!(
        tool_use_part["input"],
        serde_json::json!({"path": "/tmp/test.txt"})
    );

    let tool_result_part = &assistant_content[2];
    assert_eq!(tool_result_part["type"], "tool_result");
    assert_eq!(tool_result_part["id"], "call-1");
    assert_eq!(tool_result_part["name"], "");
    assert_eq!(
        tool_result_part["output"],
        serde_json::json!([{"type": "text", "text": "file contents here"}])
    );

    let text_part = &assistant_content[3];
    assert_eq!(text_part["type"], "text");
    assert_eq!(text_part["text"], "Here is the answer.");
}

#[tokio::test]
async fn channel_process_persists_memory_signal_as_typed_runtime_note() {
    let tmp = tempfile::tempdir().unwrap();
    let ws_dir = tmp.path().to_path_buf();
    let captured_messages = Arc::new(Mutex::new(None));
    let runtime = Arc::new(CapturingRuntime {
        captured_messages: Arc::clone(&captured_messages),
    });
    let adapter = ChannelProcessAdapter::new(runtime)
        .with_workspace_dir(ws_dir.clone())
        .with_memory_write_available(true);

    let req = make_request(
        "sess-memory-signal",
        "test-agent",
        "记住一下：以后回答直接给工程结论。",
    );
    let stream = adapter.handle(req).await;
    let _collected: Vec<_> = stream.collect().await;

    let captured = captured_messages
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    let joined = message_texts(&captured).join("\n");
    assert!(joined.contains("<memory_signal>"), "messages: {joined}");
    assert!(joined.contains("explicit_memory"), "messages: {joined}");
    assert!(joined.contains("memory_write"), "messages: {joined}");
    assert!(
        joined.contains("以后回答直接给工程结论"),
        "messages: {joined}"
    );

    let store = rua_session::WorkspaceSessionStore::new(&ws_dir);
    let messages = store
        .read_message_range("sess-memory-signal", 1, 10)
        .await
        .unwrap();
    let note_entry = messages
        .iter()
        .find(|message| rua_session::RuntimeNote::from_message_log_entry(message).is_some())
        .expect("Memory Signal Runtime Note");
    assert_eq!(note_entry.role, "system");
    let note = rua_session::RuntimeNote::from_message_log_entry(note_entry).unwrap();
    let rua_session::RuntimeNotePayload::MemorySignal(signal) = note.payload else {
        panic!("expected Memory Signal Runtime Note");
    };
    assert_eq!(signal.signal_kind, "explicit_memory");
    assert_eq!(signal.confidence, "high");
    assert_eq!(signal.suggested_memory_kind, "preference");
    assert_eq!(signal.suggested_scope, "user");
}

#[tokio::test]
async fn session_persist_text_only_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    let ws_dir = tmp.path().to_path_buf();

    let events = vec![
        RuntimeEvent::Start {
            session_id: SessionId::new("sess-2"),
            agent_id: None,
        },
        RuntimeEvent::TextDelta {
            delta: "Simple reply.".to_string(),
            message_id: None,
        },
        RuntimeEvent::End {
            finish_reason: Some("stop".to_string()),
        },
    ];

    let runtime = Arc::new(MockRuntime { events });
    let adapter = ChannelProcessAdapter::new(runtime).with_workspace_dir(ws_dir.clone());

    let req = make_request("sess-2", "test-agent", "Hi");
    let stream = adapter.handle(req).await;
    let _collected: Vec<_> = stream.collect().await;

    let store = rua_session::WorkspaceSessionStore::new(&ws_dir);
    let header = store
        .load_chat_header("sess-2")
        .await
        .unwrap()
        .expect("channel persist should create Evergreen header");
    assert_eq!(header.chat_id, "sess-2");
    let messages = store.read_message_range("sess-2", 1, 2).await.unwrap();

    assert_eq!(messages.len(), 2);

    let assistant_content = messages[1].content.as_array().expect("content array");
    assert_eq!(assistant_content.len(), 1);
    assert_eq!(assistant_content[0]["type"], "text");
    assert_eq!(assistant_content[0]["text"], "Simple reply.");
}
