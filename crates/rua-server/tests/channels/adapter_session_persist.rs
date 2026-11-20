use std::pin::Pin;
use std::sync::Arc;

use super::channel_process_adapter::{collect_turn, mounted_adapter};
use async_trait::async_trait;
use futures::stream;
use futures::Stream;
use rua_channel::core::content::{AgentChannelRequest, IncomingContentPart};
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::tool::{ToolCall, ToolResult};
use rua_core::{Result, RuaError};
use tokio::sync::Mutex;

struct MockRuntime {
    events: Vec<RuntimeEvent>,
}

#[async_trait]
impl AgentRuntime for MockRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        let events: Vec<std::result::Result<RuntimeEvent, RuaError>> =
            self.events.iter().cloned().map(Ok).collect();
        Ok(rua_agent::turn_persistence::persist_runtime_stream(
            &req,
            Box::pin(stream::iter(events)),
        ))
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
        let store =
            rua_session::WorkspaceSessionStore::new(req.conversation_store_dir.as_ref().unwrap());
        let messages = store
            .read_message_range(req.session_id.as_str(), 1, 10)
            .await
            .unwrap();
        assert!(
            messages
                .iter()
                .filter_map(rua_session::RuntimeNote::from_message_log_entry)
                .any(|note| note.render_for_model().contains("<memory_signal>")),
            "the model projection needs the durable note before its first call"
        );
        *self.captured_messages.lock().await = Some(req.messages.clone());
        let events: Vec<std::result::Result<RuntimeEvent, RuaError>> = vec![
            Ok(RuntimeEvent::Start {
                session_id: req.session_id.clone(),
                agent_id: Some(req.agent_id.clone()),
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
            }),
        ];
        Ok(rua_agent::turn_persistence::persist_runtime_stream(
            &req,
            Box::pin(stream::iter(events)),
        ))
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
            message_id: Some("reasoning".into()),
        },
        RuntimeEvent::ThinkingDelta {
            delta: "this problem.".to_string(),
            signature: None,
            message_id: Some("reasoning".into()),
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
            message_id: Some("answer".into()),
        },
        RuntimeEvent::TextDelta {
            delta: "the answer.".to_string(),
            message_id: Some("answer".into()),
        },
        RuntimeEvent::End {
            finish_reason: Some("stop".to_string()),
        },
    ];

    let runtime = Arc::new(MockRuntime { events });
    let (_state, adapter) = mounted_adapter(&ws_dir, "test-agent", runtime).await;

    let req = make_request("sess-1", "test-agent", "Hello");
    let events = collect_turn(adapter.as_ref(), req.clone()).await;
    assert!(
        matches!(events.last(), Some(RuntimeEvent::End { .. })),
        "{events:?}"
    );

    let mapped = adapter.control_chat_id(&req).await.unwrap().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(ws_dir.join("chats"));
    let header = store
        .load_chat_header(&mapped)
        .await
        .unwrap()
        .expect("channel persist should create Evergreen header");
    assert_eq!(header.chat_id, mapped);
    let messages = store.read_message_range(&mapped, 1, 20).await.unwrap();
    let parts: Vec<_> = messages
        .iter()
        .filter(|message| message.role != "user")
        .flat_map(|message| message.content.as_array().into_iter().flatten())
        .collect();
    assert_eq!(
        parts.iter().map(|part| &part["type"]).collect::<Vec<_>>(),
        ["thinking", "tool_use", "tool_result", "text"],
        "{messages:?}"
    );
    assert_eq!(parts[0]["thinking"], "Let me think about this problem.");
    assert_eq!(parts[1]["id"], "call-1");
    assert_eq!(parts[1]["name"], "read_file");
    assert_eq!(
        parts[1]["input"],
        serde_json::json!({"path":"/tmp/test.txt"})
    );
    assert_eq!(parts[2]["id"], "call-1");
    assert!(parts[2].to_string().contains("file contents here"));
    assert_eq!(parts[3]["text"], "Here is the answer.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_process_persists_memory_signal_as_typed_runtime_note() {
    let tmp = tempfile::tempdir().unwrap();
    let ws_dir = tmp.path().to_path_buf();
    let captured_messages = Arc::new(Mutex::new(None));
    let runtime = Arc::new(CapturingRuntime {
        captured_messages: Arc::clone(&captured_messages),
    });
    let (_state, adapter) = mounted_adapter(&ws_dir, "test-agent", runtime).await;

    let req = make_request(
        "sess-memory-signal",
        "test-agent",
        "记住一下：以后回答直接给工程结论。",
    );
    let events = collect_turn(adapter.as_ref(), req.clone()).await;
    assert!(
        matches!(events.last(), Some(RuntimeEvent::End { .. })),
        "{events:?}"
    );

    let captured = captured_messages
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    let joined = message_texts(&captured).join("\n");
    assert!(
        joined.contains("以后回答直接给工程结论"),
        "messages: {joined}"
    );
    assert!(
        !joined.contains("<memory_signal>"),
        "duplicate transient note: {joined}"
    );

    let mapped = adapter.control_chat_id(&req).await.unwrap().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(ws_dir.join("chats"));
    let messages = store.read_message_range(&mapped, 1, 10).await.unwrap();
    let note_entry = messages
        .iter()
        .find(|message| rua_session::RuntimeNote::from_message_log_entry(message).is_some())
        .expect("Memory Signal Runtime Note");
    assert_eq!(note_entry.role, "system");
    let note = rua_session::RuntimeNote::from_message_log_entry(note_entry).unwrap();
    let model_note = note.render_for_model();
    assert!(
        model_note.contains("<memory_signal>") && model_note.contains("memory_write"),
        "{model_note}"
    );
    let rua_session::RuntimeNotePayload::MemorySignal(signal) = note.payload else {
        panic!("expected Memory Signal Runtime Note");
    };
    assert_eq!(signal.signal_kind, "explicit_memory");
    assert_eq!(signal.confidence, "high");
    assert_eq!(signal.suggested_memory_kind, "preference");
    assert_eq!(signal.suggested_scope, "user");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
            message_id: Some("simple-answer".into()),
        },
        RuntimeEvent::End {
            finish_reason: Some("stop".to_string()),
        },
    ];

    let runtime = Arc::new(MockRuntime { events });
    let (_state, adapter) = mounted_adapter(&ws_dir, "test-agent", runtime).await;

    let req = make_request("sess-2", "test-agent", "Hi");
    let events = collect_turn(adapter.as_ref(), req.clone()).await;
    assert!(
        matches!(events.last(), Some(RuntimeEvent::End { .. })),
        "{events:?}"
    );

    let mapped = adapter.control_chat_id(&req).await.unwrap().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(ws_dir.join("chats"));
    let header = store
        .load_chat_header(&mapped)
        .await
        .unwrap()
        .expect("channel persist should create Evergreen header");
    assert_eq!(header.chat_id, mapped);
    let messages = store.read_message_range(&mapped, 1, 2).await.unwrap();

    assert_eq!(messages.len(), 2);

    let assistant_content = messages[1].content.as_array().expect("content array");
    assert_eq!(assistant_content.len(), 1);
    assert_eq!(assistant_content[0]["type"], "text");
    assert_eq!(assistant_content[0]["text"], "Simple reply.");
}
