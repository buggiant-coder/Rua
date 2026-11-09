use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::{stream, Stream, StreamExt};
use rua_channel::core::console::{ConsoleChannelService, ProcessRequest, SessionPersistence};
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::id::SessionId;
use rua_core::message::ContentPart;
use rua_core::{Result, RuaError, RunControl, RuntimeEvent};
use tokio::sync::{Mutex, Notify};

struct EventRuntime {
    events: Vec<RuntimeEvent>,
}

struct CapturingRuntime {
    request: Arc<Mutex<Option<AgentRequest>>>,
}

#[async_trait]
impl AgentRuntime for EventRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        Ok(Box::pin(stream::iter(
            self.events.clone().into_iter().map(Ok),
        )))
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

#[async_trait]
impl AgentRuntime for CapturingRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        *self.request.lock().await = Some(req);
        Ok(Box::pin(stream::iter([Ok(RuntimeEvent::End {
            finish_reason: Some("stop".to_string()),
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

struct FailingRuntime;

#[async_trait]
impl AgentRuntime for FailingRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        Err(RuaError::Runtime {
            message: "provider unavailable".to_string(),
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

struct PanickingRuntime;

#[async_trait]
impl AgentRuntime for PanickingRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        panic!("simulated provider panic")
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

#[derive(Default)]
struct RecordingPersistence {
    save_calls: AtomicUsize,
    finalize_calls: AtomicUsize,
    messages: Mutex<Vec<AgentInputMessage>>,
    assistant_started_at: Mutex<Option<String>>,
    token_usage: Mutex<Option<serde_json::Value>>,
    last_call_token_usage: Mutex<Option<serde_json::Value>>,
}

#[async_trait]
impl SessionPersistence for RecordingPersistence {
    async fn save_messages(
        &self,
        _agent_id: &str,
        _chat_id: &str,
        messages: &[AgentInputMessage],
    ) -> Result<()> {
        self.save_calls.fetch_add(1, Ordering::SeqCst);
        *self.messages.lock().await = messages.to_vec();
        Ok(())
    }

    async fn save_messages_with_assistant_started_at(
        &self,
        agent_id: &str,
        chat_id: &str,
        messages: &[AgentInputMessage],
        assistant_started_at: Option<&str>,
    ) -> Result<()> {
        *self.assistant_started_at.lock().await = assistant_started_at.map(str::to_string);
        self.save_messages(agent_id, chat_id, messages).await
    }

    async fn save_messages_with_token_usage(
        &self,
        agent_id: &str,
        chat_id: &str,
        messages: &[AgentInputMessage],
        assistant_started_at: Option<&str>,
        usage: Option<serde_json::Value>,
        last_call_usage: Option<serde_json::Value>,
    ) -> Result<()> {
        *self.token_usage.lock().await = usage;
        *self.last_call_token_usage.lock().await = last_call_usage;
        self.save_messages_with_assistant_started_at(
            agent_id,
            chat_id,
            messages,
            assistant_started_at,
        )
        .await
    }

    async fn finalize_turn(
        &self,
        _agent_id: &str,
        _chat_id: &str,
        assistant_started_at: Option<&str>,
        usage: Option<serde_json::Value>,
        last_call_usage: Option<serde_json::Value>,
    ) -> Result<()> {
        self.finalize_calls.fetch_add(1, Ordering::SeqCst);
        *self.assistant_started_at.lock().await = assistant_started_at.map(str::to_string);
        *self.token_usage.lock().await = usage;
        *self.last_call_token_usage.lock().await = last_call_usage;
        Ok(())
    }

    async fn save_chat_entry(
        &self,
        _agent_id: &str,
        _chat_id: &str,
        _chat_name: &str,
    ) -> Result<()> {
        Ok(())
    }
}

struct BlockingPersistence {
    entered: Notify,
    release: Notify,
}

impl BlockingPersistence {
    fn new() -> Self {
        Self {
            entered: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[async_trait]
impl SessionPersistence for BlockingPersistence {
    async fn save_messages(
        &self,
        _agent_id: &str,
        _chat_id: &str,
        _messages: &[AgentInputMessage],
    ) -> Result<()> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(())
    }

    async fn finalize_turn(
        &self,
        _agent_id: &str,
        _chat_id: &str,
        _assistant_started_at: Option<&str>,
        _usage: Option<serde_json::Value>,
        _last_call_usage: Option<serde_json::Value>,
    ) -> Result<()> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(())
    }

    async fn save_chat_entry(
        &self,
        _agent_id: &str,
        _chat_id: &str,
        _chat_name: &str,
    ) -> Result<()> {
        Ok(())
    }
}

async fn drain_process(runtime: Arc<dyn AgentRuntime>, persistence: Arc<RecordingPersistence>) {
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let service = ConsoleChannelService::new(runtime);
    let mut stream = service.process(ProcessRequest {
        agent_id: "agent-1".into(),
        chat_id: "chat-1".into(),
        messages: vec![user_message("go")],
        model_override: None,
        workspace_dir: None,
        conversation_store_dir: None,
        working_dir: None,
        cancel_rx,
        user_message: "go".into(),
        persistence,
        disabled_tools: Vec::new(),
        metadata: HashMap::new(),
    });

    while stream.next().await.is_some() {}
}

#[tokio::test]
async fn console_only_finalizes_turn_without_replaying_runtime_output() {
    let runtime = Arc::new(EventRuntime {
        events: vec![
            RuntimeEvent::TextDelta {
                delta: "done".to_string(),
                message_id: None,
            },
            RuntimeEvent::Usage {
                input_tokens: Some(10),
                output_tokens: Some(2),
                total_tokens: Some(12),
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
            },
        ],
    });
    let persistence = Arc::new(RecordingPersistence::default());

    drain_process(runtime, Arc::clone(&persistence)).await;

    assert_eq!(persistence.save_calls.load(Ordering::SeqCst), 0);
    assert_eq!(persistence.finalize_calls.load(Ordering::SeqCst), 1);
    assert!(persistence.messages.lock().await.is_empty());
    assert_eq!(
        persistence.token_usage.lock().await.as_ref().unwrap()["total_tokens"],
        12
    );
}

fn user_message(text: &str) -> AgentInputMessage {
    AgentInputMessage {
        role: "user".into(),
        content: vec![ContentPart::Text { text: text.into() }],
    }
}

#[tokio::test]
async fn process_forwards_separate_runtime_paths() {
    let captured = Arc::new(Mutex::new(None));
    let runtime = Arc::new(CapturingRuntime {
        request: Arc::clone(&captured),
    });
    let persistence = Arc::new(RecordingPersistence::default());
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let service = ConsoleChannelService::new(runtime);
    let mut stream = service.process(ProcessRequest {
        agent_id: "agent-1".into(),
        chat_id: "chat-1".into(),
        messages: vec![user_message("go")],
        model_override: None,
        workspace_dir: Some("/agent-workspace".into()),
        conversation_store_dir: Some("/conversation-store".into()),
        working_dir: Some("/project-root".into()),
        cancel_rx,
        user_message: "go".into(),
        persistence,
        disabled_tools: Vec::new(),
        metadata: HashMap::new(),
    });

    while stream.next().await.is_some() {}

    let request = captured.lock().await.take().expect("captured request");
    assert_eq!(
        request.workspace_dir.as_deref(),
        Some(std::path::Path::new("/agent-workspace"))
    );
    assert_eq!(
        request.conversation_store_dir.as_deref(),
        Some(std::path::Path::new("/conversation-store"))
    );
    assert_eq!(
        request.working_dir.as_deref(),
        Some(std::path::Path::new("/project-root"))
    );
}

#[tokio::test]
async fn process_drains_runtime_after_control_sender_closes() {
    let runtime = Arc::new(EventRuntime {
        events: vec![
            RuntimeEvent::Start {
                session_id: SessionId::new("chat-1"),
                agent_id: None,
            },
            RuntimeEvent::TextDelta {
                delta: "answer".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });
    let persistence = Arc::new(RecordingPersistence::default());
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    drop(cancel_tx);
    let service = ConsoleChannelService::new(runtime);
    let stream = service.process(ProcessRequest {
        agent_id: "agent-1".into(),
        chat_id: "chat-1".into(),
        messages: vec![user_message("go")],
        model_override: None,
        workspace_dir: None,
        conversation_store_dir: None,
        working_dir: None,
        cancel_rx,
        user_message: "go".into(),
        persistence,
        disabled_tools: Vec::new(),
        metadata: HashMap::new(),
    });

    let events = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        stream.collect::<Vec<_>>(),
    )
    .await
    .expect("closed control channel must not starve runtime events");
    assert!(events
        .iter()
        .any(|event| matches!(event, Ok(RuntimeEvent::End { .. }))));
}

#[tokio::test]
async fn process_holds_terminal_event_until_turn_persistence_finishes() {
    let runtime = Arc::new(EventRuntime {
        events: vec![
            RuntimeEvent::Start {
                session_id: SessionId::new("chat-1"),
                agent_id: None,
            },
            RuntimeEvent::TextDelta {
                delta: "answer".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });
    let persistence = Arc::new(BlockingPersistence::new());
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let service = ConsoleChannelService::new(runtime);
    let mut stream = service.process(ProcessRequest {
        agent_id: "agent-1".into(),
        chat_id: "chat-1".into(),
        messages: vec![user_message("go")],
        model_override: None,
        workspace_dir: None,
        conversation_store_dir: None,
        working_dir: None,
        cancel_rx,
        user_message: "go".into(),
        persistence: persistence.clone(),
        disabled_tools: Vec::new(),
        metadata: HashMap::new(),
    });

    assert!(matches!(
        stream.next().await,
        Some(Ok(RuntimeEvent::Start { .. }))
    ));
    assert!(matches!(
        stream.next().await,
        Some(Ok(RuntimeEvent::TextDelta { .. }))
    ));
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        persistence.entered.notified(),
    )
    .await
    .expect("terminal candidate enters Turn persistence");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), stream.next())
            .await
            .is_err(),
        "terminal event must remain internal while Turn persistence is blocked"
    );

    persistence.release.notify_one();
    assert!(matches!(
        stream.next().await,
        Some(Ok(RuntimeEvent::End { .. }))
    ));
}

#[tokio::test]
async fn process_holds_startup_failure_until_turn_persistence_finishes() {
    let persistence = Arc::new(BlockingPersistence::new());
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let service = ConsoleChannelService::new(Arc::new(FailingRuntime));
    let mut stream = service.process(ProcessRequest {
        agent_id: "agent-1".into(),
        chat_id: "chat-1".into(),
        messages: vec![user_message("go")],
        model_override: None,
        workspace_dir: None,
        conversation_store_dir: None,
        working_dir: None,
        cancel_rx,
        user_message: "go".into(),
        persistence: persistence.clone(),
        disabled_tools: Vec::new(),
        metadata: HashMap::new(),
    });

    assert!(matches!(
        stream.next().await,
        Some(Ok(RuntimeEvent::Start { .. }))
    ));
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        persistence.entered.notified(),
    )
    .await
    .expect("startup failure enters Turn persistence");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), stream.next())
            .await
            .is_err(),
        "startup failure must remain internal while Turn persistence is blocked"
    );

    persistence.release.notify_one();
    assert!(matches!(
        stream.next().await,
        Some(Ok(RuntimeEvent::Error { .. }))
    ));
}

#[tokio::test]
async fn process_converts_runtime_panic_into_visible_error_terminal() {
    let persistence = Arc::new(RecordingPersistence::default());
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let service = ConsoleChannelService::new(Arc::new(PanickingRuntime));
    let events = service
        .process(ProcessRequest {
            agent_id: "agent-1".into(),
            chat_id: "chat-1".into(),
            messages: vec![user_message("go")],
            model_override: None,
            workspace_dir: None,
            conversation_store_dir: None,
            working_dir: None,
            cancel_rx,
            user_message: "go".into(),
            persistence,
            disabled_tools: Vec::new(),
            metadata: HashMap::new(),
        })
        .collect::<Vec<_>>()
        .await;

    assert!(
        events.iter().any(|event| {
            matches!(
                event,
                Ok(RuntimeEvent::Error { error })
                    if error.to_string().contains("simulated provider panic")
            )
        }),
        "runtime panic must become a visible error event, got {events:?}"
    );
    assert!(
        events.iter().any(|event| {
            matches!(
                event,
                Ok(RuntimeEvent::End { finish_reason })
                    if finish_reason.as_deref() == Some("error")
            )
        }),
        "runtime panic must terminate the stream as an error, got {events:?}"
    );
}

#[tokio::test]
async fn process_records_assistant_start_only_after_assistant_output() {
    let no_output_runtime = Arc::new(EventRuntime {
        events: vec![
            RuntimeEvent::Start {
                session_id: SessionId::new("chat-1"),
                agent_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });
    let no_output_persistence = Arc::new(RecordingPersistence::default());
    drain_process(no_output_runtime, Arc::clone(&no_output_persistence)).await;
    assert_eq!(
        *no_output_persistence.assistant_started_at.lock().await,
        None
    );

    let output_runtime = Arc::new(EventRuntime {
        events: vec![
            RuntimeEvent::Start {
                session_id: SessionId::new("chat-1"),
                agent_id: None,
            },
            RuntimeEvent::TextDelta {
                delta: "first output".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });
    let output_persistence = Arc::new(RecordingPersistence::default());
    drain_process(output_runtime, Arc::clone(&output_persistence)).await;
    assert!(
        output_persistence
            .assistant_started_at
            .lock()
            .await
            .is_some(),
        "first assistant output should define assistant start"
    );
}

#[tokio::test]
async fn process_passes_final_runtime_usage_to_persistence() {
    let runtime = Arc::new(EventRuntime {
        events: vec![
            RuntimeEvent::Start {
                session_id: SessionId::new("chat-1"),
                agent_id: None,
            },
            RuntimeEvent::TextDelta {
                delta: "answer".into(),
                message_id: None,
            },
            RuntimeEvent::Usage {
                input_tokens: Some(10500),
                output_tokens: Some(770),
                total_tokens: Some(11270),
                cache_read_tokens: Some(120),
                cache_write_tokens: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });
    let persistence = Arc::new(RecordingPersistence::default());

    drain_process(runtime, Arc::clone(&persistence)).await;

    let usage = persistence.token_usage.lock().await.clone().unwrap();
    assert_eq!(usage["prompt_tokens"], 10500);
    assert_eq!(usage["completion_tokens"], 770);
    assert_eq!(usage["total_tokens"], 11270);
    assert_eq!(usage["cache_read_tokens"], 120);
}

#[tokio::test]
async fn process_separates_turn_total_from_last_model_call_usage() {
    let runtime = Arc::new(EventRuntime {
        events: vec![
            RuntimeEvent::Start {
                session_id: SessionId::new("chat-1"),
                agent_id: None,
            },
            RuntimeEvent::Usage {
                input_tokens: Some(100),
                output_tokens: Some(10),
                total_tokens: Some(110),
                cache_read_tokens: Some(40),
                cache_write_tokens: None,
            },
            RuntimeEvent::IterationEnd {
                iteration: 0,
                has_tool_calls: true,
            },
            RuntimeEvent::Usage {
                input_tokens: Some(140),
                output_tokens: Some(20),
                total_tokens: Some(160),
                cache_read_tokens: Some(80),
                cache_write_tokens: None,
            },
            RuntimeEvent::IterationEnd {
                iteration: 1,
                has_tool_calls: false,
            },
            RuntimeEvent::Usage {
                input_tokens: Some(240),
                output_tokens: Some(30),
                total_tokens: Some(270),
                cache_read_tokens: Some(120),
                cache_write_tokens: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });
    let persistence = Arc::new(RecordingPersistence::default());

    drain_process(runtime, Arc::clone(&persistence)).await;

    let total = persistence.token_usage.lock().await.clone().unwrap();
    assert_eq!(total["prompt_tokens"], 240);
    assert_eq!(total["completion_tokens"], 30);
    assert_eq!(total["cache_read_tokens"], 120);

    let last_call = persistence
        .last_call_token_usage
        .lock()
        .await
        .clone()
        .unwrap();
    assert_eq!(last_call["prompt_tokens"], 140);
    assert_eq!(last_call["completion_tokens"], 20);
    assert_eq!(last_call["cache_read_tokens"], 80);
}
