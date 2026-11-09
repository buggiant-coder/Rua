use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use rua_agent::context_manager::{
    CompactContextRequest, CompactContextResult, ContextManager, ContextSummaryRequest,
    ContextSummaryResult,
};
use rua_agent::runtime::ReactAgentRuntime;
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime};
use rua_core::event::RuntimeEvent;
use rua_core::id::{AgentId, SessionId};
use rua_provider::manager::ProviderManager;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::json;
use tokio::sync::Notify;

fn make_runtime() -> ReactAgentRuntime {
    let pm = Arc::new(ProviderManager::new());
    ReactAgentRuntime::new(pm)
}

fn make_request(session_id: &str, query: &str) -> AgentRequest {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("locale".into(), serde_json::json!("en"));
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
        metadata,
        cancel: None,
        run_control: None,
        workspace_dir: Some(std::path::PathBuf::from("/tmp/test-workspace")),
        conversation_store_dir: None,
        working_dir: None,
    }
}

fn make_request_with_workspace(
    session_id: &str,
    query: &str,
    workspace_dir: std::path::PathBuf,
) -> AgentRequest {
    let mut req = make_request(session_id, query);
    req.workspace_dir = Some(workspace_dir);
    req
}

struct WaitingContextManager {
    started: Arc<Notify>,
    release: Arc<Notify>,
    model: Mutex<Option<rua_session::CheckpointModelIdentity>>,
}

#[async_trait]
impl ContextManager for WaitingContextManager {
    async fn compact_context(
        &self,
        req: CompactContextRequest,
    ) -> Result<CompactContextResult, rua_agent::AgentError> {
        Ok(CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        req: ContextSummaryRequest,
    ) -> Result<ContextSummaryResult, rua_agent::AgentError> {
        *self.model.lock().unwrap() = req.model;
        self.started.notify_waiters();
        self.release.notified().await;
        Ok(ContextSummaryResult {
            summary: "delayed compact summary".to_string(),
        })
    }
}

async fn create_chat_with_one_turn(workspace_dir: &std::path::Path, chat_id: &str) {
    let store = WorkspaceSessionStore::new(workspace_dir);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "test-agent".to_string(),
            title: "Test".to_string(),
            created_at: "2026-07-01T00:00:00Z".to_string(),
            last_activity_at: "2026-07-01T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-01T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-01T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-01T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "user-1".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-07-01T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "hello"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "assistant-1".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-07-01T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "hi"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .unwrap();
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
async fn clear_command_clears_session() {
    let sid = "test-session-clear";

    let runtime = make_runtime();
    let req = make_request(sid, "/clear");
    let deltas = collect_text_deltas(&runtime, req).await;

    assert!(!deltas.is_empty(), "should return text");
    assert!(
        deltas.iter().any(|d| d.contains("History Cleared")),
        "should mention cleared: {deltas:?}"
    );
}

#[tokio::test]
async fn clear_command_uses_conversation_store_instead_of_agent_workspace() {
    let agent_workspace = tempfile::tempdir().unwrap();
    let conversation_store = tempfile::tempdir().unwrap();
    let sid = "test-session-clear-project";
    create_chat_with_one_turn(conversation_store.path(), sid).await;

    let runtime = make_runtime();
    let mut req = make_request_with_workspace(sid, "/clear", agent_workspace.path().to_path_buf());
    req.conversation_store_dir = Some(conversation_store.path().to_path_buf());
    let deltas = collect_text_deltas(&runtime, req).await;

    assert!(
        deltas.iter().any(|delta| delta.contains("History Cleared")),
        "clear result should be returned: {deltas:?}"
    );
    let header = WorkspaceSessionStore::new(conversation_store.path())
        .load_chat_header(sid)
        .await
        .expect("conversation store header")
        .expect("conversation store chat");
    assert_eq!(
        header
            .projection_boundary
            .as_ref()
            .map(|boundary| boundary.kind.as_str()),
        Some("clear")
    );
    assert!(
        WorkspaceSessionStore::new(agent_workspace.path())
            .load_chat_header(sid)
            .await
            .expect("agent workspace header")
            .is_none(),
        "conversation command must not write into the Agent workspace"
    );
}

#[tokio::test]
async fn new_command_clears_and_queues_summary() {
    let sid = "test-session-new";

    let runtime = make_runtime();
    let req = make_request(sid, "/new");
    let deltas = collect_text_deltas(&runtime, req).await;

    assert!(!deltas.is_empty());
    assert!(
        deltas.iter().any(|d| d.contains("New Conversation")
            || d.contains("Context Manager Disabled")
            || d.contains("No messages")),
        "should handle /new: {deltas:?}"
    );
}

#[tokio::test]
async fn prompts_command_is_not_registered() {
    let sid = "test-session-prompts";

    let runtime = make_runtime();
    let req = make_request(sid, "/prompts");
    let result = runtime.stream(req).await;
    assert!(
        result.is_err(),
        "/prompts should fall through as ordinary model input after removal"
    );
}

#[tokio::test]
async fn history_command_is_not_registered() {
    let sid = "test-session-history";

    let runtime = make_runtime();
    let req = make_request(sid, "/history");
    let result = runtime.stream(req).await;
    assert!(
        result.is_err(),
        "/history should fall through as ordinary model input after removal"
    );
}

#[tokio::test]
async fn compact_command_streams_pending_bubble_before_summary_finishes() {
    let tmp = tempfile::tempdir().unwrap();
    let sid = "test-session-compact-pending";
    create_chat_with_one_turn(tmp.path(), sid).await;

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let context_manager = Arc::new(WaitingContextManager {
        started: Arc::clone(&started),
        release: Arc::clone(&release),
        model: Mutex::new(None),
    });
    let runtime =
        make_runtime().with_context_manager(context_manager.clone() as Arc<dyn ContextManager>);
    let mut req = make_request_with_workspace(sid, "/compact", tmp.path().to_path_buf());
    req.metadata.insert(
        "chat_model_slot".into(),
        json!({
            "provider_id": "chat-provider",
            "model": "chat-model"
        }),
    );

    let stream = tokio::time::timeout(std::time::Duration::from_millis(200), runtime.stream(req))
        .await
        .expect("/compact stream should be returned before summary finishes")
        .expect("stream should succeed");
    let mut stream = Box::pin(stream);

    let first = tokio::time::timeout(std::time::Duration::from_millis(200), stream.next())
        .await
        .expect("start event should arrive")
        .expect("stream should not end")
        .expect("start event should be ok");
    assert!(matches!(first, RuntimeEvent::Start { .. }));

    let second = tokio::time::timeout(std::time::Duration::from_millis(200), stream.next())
        .await
        .expect("pending bubble event should arrive")
        .expect("stream should not end")
        .expect("pending bubble event should be ok");
    assert!(matches!(
        second,
        RuntimeEvent::TextDelta { ref delta, .. } if delta.is_empty()
    ));

    tokio::time::timeout(std::time::Duration::from_millis(200), started.notified())
        .await
        .expect("summary should have started after pending bubble");
    assert_eq!(
        context_manager.model.lock().unwrap().as_ref(),
        Some(&rua_session::CheckpointModelIdentity {
            provider: "chat-provider".to_string(),
            model: "chat-model".to_string(),
        })
    );
    release.notify_waiters();

    let mut deltas = Vec::new();
    while let Some(event) = stream.next().await {
        if let RuntimeEvent::TextDelta { delta, .. } = event.expect("event should be ok") {
            deltas.push(delta);
        }
    }
    assert!(
        deltas
            .iter()
            .any(|delta| delta.contains("delayed compact summary")),
        "compact result should stream after summary release: {deltas:?}"
    );
}

#[tokio::test]
async fn stream_returns_start_and_end_framing() {
    let sid = "test-session-framing";

    let runtime = make_runtime();
    let req = make_request(sid, "/clear");
    let stream = runtime.stream(req).await.expect("stream should succeed");
    let events: Vec<_> = stream.collect().await;

    assert!(
        matches!(&events[0], Ok(RuntimeEvent::Start { .. })),
        "first event should be Start"
    );
    assert!(
        matches!(&events[events.len() - 1], Ok(RuntimeEvent::End { .. })),
        "last event should be End"
    );
}

#[tokio::test]
async fn non_command_falls_through() {
    let sid = "test-session-fallthrough";

    let runtime = make_runtime();
    let req = make_request(sid, "hello world");

    let result = runtime.stream(req).await;
    assert!(
        result.is_err(),
        "non-command should fall through to agent loop which fails without provider"
    );
}
