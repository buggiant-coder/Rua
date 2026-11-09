use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use rua_agent::runtime::{CommandRuntimeDispatcherAdapter, ReactAgentRuntime};
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime, CommandRuntimeConfig};
use rua_core::event::RuntimeEvent;
use rua_core::id::{AgentId, SessionId};
use rua_provider::manager::ProviderManager;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};

struct MockCanceller {
    called: AtomicBool,
}

impl MockCanceller {
    fn new() -> Self {
        Self {
            called: AtomicBool::new(false),
        }
    }

    fn was_called(&self) -> bool {
        self.called.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl rua_command::SessionCanceller for MockCanceller {
    async fn cancel_session(&self, _channel_id: &str, _session_id: &str) -> bool {
        self.called.store(true, Ordering::Relaxed);
        true
    }

    async fn active_session_count(&self) -> usize {
        1
    }
}

struct NoopApproval;

#[async_trait]
impl rua_command::ApprovalService for NoopApproval {
    async fn approve_pending(&self, _session_id: &str) -> bool {
        false
    }
    async fn reject_pending(&self, _session_id: &str) -> bool {
        false
    }
}

struct NoopQueueClearer;

#[async_trait]
impl rua_command::QueueClearer for NoopQueueClearer {
    async fn clear_queue(&self, _channel_id: &str, _session_id: &str) -> usize {
        0
    }
}

struct NoopReloader;

#[async_trait]
impl rua_command::WorkspaceReloader for NoopReloader {
    async fn reload_all(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

struct NoopSkillsLister;

#[async_trait]
impl rua_command::SkillsLister for NoopSkillsLister {
    async fn list_skills(&self, _channel_id: &str, _locale: &str) -> String {
        "No skills are currently enabled for this channel.".to_string()
    }
}

struct NoopAcpChatController;

#[async_trait]
impl rua_command::AcpChatController for NoopAcpChatController {
    async fn enter(&self, _chat_id: &str, _runner: &str, _cwd: Option<&str>) -> String {
        String::new()
    }
    async fn exit(&self, _chat_id: &str) -> String {
        String::new()
    }
    async fn list_sessions(&self, _chat_id: &str, _runner: &str, _cwd: Option<&str>) -> String {
        String::new()
    }
    async fn resume(
        &self,
        _chat_id: &str,
        _runner: &str,
        _acp_session_id: &str,
        _cwd: Option<&str>,
    ) -> String {
        String::new()
    }
}

fn make_request(session_id: &str, query: &str) -> AgentRequest {
    AgentRequest {
        session_id: SessionId::new(session_id),
        user_id: "test-user".into(),
        agent_id: AgentId::new("test-agent"),
        messages: vec![AgentInputMessage {
            role: "user".into(),
            content: vec![rua_core::message::ContentPart::Text { text: query.into() }],
        }],
        model_override: None,
        tool_mode: "auto".into(),
        attachments: vec![],
        display_preferences: None,
        disabled_tools: vec![],
        metadata: std::collections::HashMap::new(),
        cancel: None,
        run_control: None,
        workspace_dir: Some(std::path::PathBuf::from("/tmp/test-workspace")),
        conversation_store_dir: None,
        working_dir: None,
    }
}

fn make_runtime_with_commands(
    canceller: Arc<dyn rua_command::SessionCanceller>,
) -> ReactAgentRuntime {
    let pm = Arc::new(ProviderManager::new());
    let rt = ReactAgentRuntime::new(pm);

    let mut control_registry = rua_command::ControlCommandRegistry::new();
    control_registry.register(Arc::new(rua_command::StopCommand));

    let daemon_registry = rua_command::build_daemon_registry();

    let dispatcher = CommandRuntimeDispatcherAdapter::new(
        daemon_registry,
        control_registry,
        canceller,
        Arc::new(NoopApproval) as Arc<dyn rua_command::ApprovalService>,
        Arc::new(NoopQueueClearer) as Arc<dyn rua_command::QueueClearer>,
        Arc::new(NoopSkillsLister) as Arc<dyn rua_command::SkillsLister>,
        Arc::new(NoopReloader) as Arc<dyn rua_command::WorkspaceReloader>,
        Arc::new(rua_command::AcpChatSessionManager::new(
            std::time::Duration::from_secs(300),
        )),
        Arc::new(NoopAcpChatController) as Arc<dyn rua_command::AcpChatController>,
        Arc::new(rua_acp::service::AcpService::new(
            Arc::new(rua_config::types::AcpConfig::default()),
            "/tmp".to_string(),
        )),
        std::path::PathBuf::from("/tmp/test-data"),
        std::path::PathBuf::from("/tmp/test-workspace"),
        std::time::Instant::now(),
        None,
    );

    rt.set_command_runtime(CommandRuntimeConfig {
        dispatcher: Arc::new(dispatcher),
    });

    rt
}

async fn collect_text_deltas(runtime: &ReactAgentRuntime, req: AgentRequest) -> Vec<String> {
    let stream = runtime.stream(req).await.expect("stream should succeed");
    let events: Vec<_> = stream.collect().await;
    events
        .into_iter()
        .filter_map(|r| match r {
            Ok(RuntimeEvent::TextDelta { delta, .. }) => Some(delta),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn stop_command_calls_canceller() {
    let canceller = Arc::new(MockCanceller::new());
    let runtime = make_runtime_with_commands(canceller.clone());

    let mut req = make_request("sess-1", "/stop");
    req.metadata
        .insert("locale".into(), serde_json::json!("en"));
    let deltas = collect_text_deltas(&runtime, req).await;

    assert!(canceller.was_called(), "canceller should have been called");
    assert!(
        deltas.iter().any(|d| d.contains("Task Stopped")),
        "should return stop confirmation: {deltas:?}"
    );
}

#[tokio::test]
async fn status_command_returns_info() {
    let canceller = Arc::new(MockCanceller::new());
    let runtime = make_runtime_with_commands(canceller);

    let req = make_request("sess-2", "/status");
    let deltas = collect_text_deltas(&runtime, req).await;

    assert!(!deltas.is_empty(), "should return status info");
    assert!(
        deltas.iter().any(|d| d.contains("pid")
            || d.contains("PID")
            || d.contains("uptime")
            || d.contains("Status")),
        "should contain runtime info: {deltas:?}"
    );
}

#[tokio::test]
async fn command_output_is_persisted_by_runtime_before_stream_ends() {
    let workspace = tempfile::tempdir().expect("workspace");
    let chat_id = "command-persistence";
    let turn_id = "turn-command-persistence";
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "test-agent".to_string(),
            title: "Command persistence".to_string(),
            created_at: "2026-08-04T00:00:00Z".to_string(),
            last_activity_at: "2026-08-04T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("chat header");
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-04T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "user-command".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-08-04T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{"type": "text", "text": "/status"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .expect("submitted turn");

    let runtime = make_runtime_with_commands(Arc::new(MockCanceller::new()));
    let mut req = make_request(chat_id, "/status");
    req.workspace_dir = Some(workspace.path().to_path_buf());
    req.conversation_store_dir = Some(workspace.path().to_path_buf());
    req.metadata
        .insert("turn_id".into(), serde_json::json!(turn_id));
    req.metadata.insert(
        rua_agent::turn_persistence::LOADED_MESSAGE_COUNT_METADATA_KEY.into(),
        serde_json::json!(0),
    );

    let stream = runtime.stream(req).await.expect("command stream");
    let events = stream.collect::<Vec<_>>().await;
    assert!(events.iter().all(Result::is_ok));

    let messages = store
        .read_turn_messages(chat_id, turn_id)
        .await
        .expect("turn messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].role, "assistant");
    assert!(
        messages[1].content.to_string().contains("pid"),
        "command output must be durable before the terminal event: {messages:?}"
    );
}

#[tokio::test]
async fn clear_command_still_works_with_workspace_store() {
    let canceller = Arc::new(MockCanceller::new());
    let pm = Arc::new(ProviderManager::new());
    let rt = ReactAgentRuntime::new(pm);

    let mut control_registry = rua_command::ControlCommandRegistry::new();
    control_registry.register(Arc::new(rua_command::StopCommand));
    let daemon_registry = rua_command::build_daemon_registry();

    let dispatcher = CommandRuntimeDispatcherAdapter::new(
        daemon_registry,
        control_registry,
        canceller as Arc<dyn rua_command::SessionCanceller>,
        Arc::new(NoopApproval) as Arc<dyn rua_command::ApprovalService>,
        Arc::new(NoopQueueClearer) as Arc<dyn rua_command::QueueClearer>,
        Arc::new(NoopSkillsLister) as Arc<dyn rua_command::SkillsLister>,
        Arc::new(NoopReloader) as Arc<dyn rua_command::WorkspaceReloader>,
        Arc::new(rua_command::AcpChatSessionManager::new(
            std::time::Duration::from_secs(300),
        )),
        Arc::new(NoopAcpChatController) as Arc<dyn rua_command::AcpChatController>,
        Arc::new(rua_acp::service::AcpService::new(
            Arc::new(rua_config::types::AcpConfig::default()),
            "/tmp".to_string(),
        )),
        std::path::PathBuf::from("/tmp/test-data"),
        std::path::PathBuf::from("/tmp/test-workspace"),
        std::time::Instant::now(),
        None,
    );

    rt.set_command_runtime(CommandRuntimeConfig {
        dispatcher: Arc::new(dispatcher),
    });

    let mut req = make_request("sess-3", "/clear");
    req.metadata
        .insert("locale".into(), serde_json::json!("en"));
    let deltas = collect_text_deltas(&rt, req).await;

    assert!(!deltas.is_empty(), "should return clear response");
    assert!(
        deltas.iter().any(|d| d.contains("History Cleared")),
        "should handle /clear with en locale: {deltas:?}"
    );
}

#[tokio::test]
async fn no_injection_control_commands_fall_through() {
    let pm = Arc::new(ProviderManager::new());
    let runtime = ReactAgentRuntime::new(pm);

    let req = make_request("sess-4", "/stop");
    let result = runtime.stream(req).await;

    assert!(
        result.is_err(),
        "/stop without injection should fall through to agent loop and fail"
    );
}
