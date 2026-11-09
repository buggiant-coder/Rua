use std::sync::Arc;

use agent_types::Role;
use rua_command::control::{ControlCommandRegistry, ControlContext};
use rua_command::conversation::*;
use rua_command::daemon::{DaemonCommandRegistry, DaemonContext};
use rua_command::{CommandDispatcher, DispatchContext};

use crate::mock_seams::*;

fn make_dispatch_ctx(conv_ctx: ConversationContext) -> DispatchContext {
    DispatchContext {
        daemon: DaemonContext {
            data_dir: std::path::PathBuf::from("/tmp/test"),
            started_at: std::time::Instant::now(),
            workspace_reloader: Arc::new(StubWorkspaceReloader),
            exit_fn: None,
            approval_service: Arc::new(StubApprovalService),
            locale: "zh".into(),
        },
        control: ControlContext {
            session_id: "s".to_string(),
            user_id: "u".to_string(),
            channel: "c".to_string(),
            session_canceller: Arc::new(StubSessionCanceller),
            approval_service: Arc::new(StubApprovalService),
            queue_clearer: Arc::new(StubQueueClearer),
            skills_lister: Arc::new(StubSkillsLister),
            acp_chat_manager: Arc::new(rua_command::AcpChatSessionManager::new(
                std::time::Duration::from_secs(300),
            )),
            acp_chat_controller: Arc::new(StubAcpChatController),
            locale: "zh".into(),
        },
        conversation: conv_ctx,
    }
}

struct StubWorkspaceReloader;
#[async_trait::async_trait]
impl rua_command::daemon::WorkspaceReloader for StubWorkspaceReloader {
    async fn reload_all(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

struct StubSessionCanceller;
#[async_trait::async_trait]
impl rua_command::control::SessionCanceller for StubSessionCanceller {
    async fn cancel_session(&self, _: &str, _: &str) -> bool {
        false
    }
    async fn active_session_count(&self) -> usize {
        0
    }
}

struct StubQueueClearer;
#[async_trait::async_trait]
impl rua_command::control::QueueClearer for StubQueueClearer {
    async fn clear_queue(&self, _: &str, _: &str) -> usize {
        0
    }
}

struct StubApprovalService;
#[async_trait::async_trait]
impl rua_command::control::ApprovalService for StubApprovalService {
    async fn approve_pending(&self, _: &str) -> bool {
        false
    }
    async fn reject_pending(&self, _: &str) -> bool {
        false
    }
}

struct StubSkillsLister;
#[async_trait::async_trait]
impl rua_command::control::SkillsLister for StubSkillsLister {
    async fn list_skills(&self, _: &str, _: &str) -> String {
        String::new()
    }
}

struct StubAcpChatController;
#[async_trait::async_trait]
impl rua_command::AcpChatController for StubAcpChatController {
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

#[test]
fn registry_has_exactly_3_commands() {
    let registry = rua_agent::command_adapters::build_conversation_registry();
    let list = registry.list();
    assert_eq!(
        list.len(),
        3,
        "expected 3 commands, got {}: {:?}",
        list.len(),
        list
    );
}

#[test]
fn registry_command_names() {
    let registry = rua_agent::command_adapters::build_conversation_registry();
    let names: Vec<&str> = registry.list().iter().map(|(n, _)| *n).collect();
    let expected = vec!["clear", "compact", "new"];
    assert_eq!(names, expected);
}

#[test]
fn registry_excludes_ui_replaced_commands() {
    let registry = rua_agent::command_adapters::build_conversation_registry();
    let names: Vec<&str> = registry.list().iter().map(|(n, _)| *n).collect();
    for removed in [
        "compact_str",
        "dump_history",
        "history",
        "message",
        "no",
        "prompt",
        "prompts",
        "yes",
    ] {
        assert!(
            !names.contains(&removed),
            "/{removed} should not be registered as a slash command"
        );
    }
}

#[test]
fn registry_agent_required_flags() {
    let registry = rua_agent::command_adapters::build_conversation_registry();
    let agent_required: Vec<&str> = registry
        .list()
        .iter()
        .filter(|(name, _)| registry.is_agent_required(name))
        .map(|(n, _)| *n)
        .collect();
    let mut ar = agent_required.clone();
    ar.sort();
    assert_eq!(ar, Vec::<&str>::new());
}

#[tokio::test]
async fn dispatch_clear_through_dispatcher() {
    let session = Arc::new(
        MockSession::new()
            .with_messages(vec![make_message(Role::User, "hello")])
            .with_summary("old summary"),
    );
    let mut ctx = make_ctx_with_session(session.clone());
    ctx.session = Some(session.clone());

    let registry = rua_agent::command_adapters::build_conversation_registry();
    let dispatcher = CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        ControlCommandRegistry::new(),
        registry,
    );

    let dispatch_ctx = make_dispatch_ctx(ctx);
    let outcome = dispatcher.dispatch("/clear", &dispatch_ctx).await;
    let text = outcome_text(&outcome);
    assert!(text.contains("History Cleared"));
    let meta = outcome_metadata(&outcome);
    assert_eq!(meta["clear_history"], true);
    assert_eq!(meta["clear_plan"], true);
    assert!(session
        .clear_messages_called
        .load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn dispatch_yes_is_not_registered() {
    let ctx = ConversationContext::stub("a", "s", "u", "c");
    let registry = rua_agent::command_adapters::build_conversation_registry();
    let dispatcher = CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        ControlCommandRegistry::new(),
        registry,
    );

    let dispatch_ctx = make_dispatch_ctx(ctx);
    let outcome = dispatcher.dispatch("/yes", &dispatch_ctx).await;
    assert!(outcome_text(&outcome).contains("Unknown command"));
}

#[tokio::test]
async fn dispatch_compact_calls_compactor() {
    let session = Arc::new(MockSession::new().with_messages(vec![make_message(Role::User, "hi")]));
    let compactor = Arc::new(MockCompactor::returning("summary result"));
    let mut ctx = make_ctx_with_session(session);
    ctx.context_compactor = Some(compactor.clone());

    let registry = rua_agent::command_adapters::build_conversation_registry();
    let dispatcher = CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        ControlCommandRegistry::new(),
        registry,
    );

    let dispatch_ctx = make_dispatch_ctx(ctx);
    let outcome = dispatcher.dispatch("/compact", &dispatch_ctx).await;
    let text = outcome_text(&outcome);
    assert!(text.contains("Compact Complete"));
    assert!(compactor.called.load(std::sync::atomic::Ordering::SeqCst));
}
