use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rua_channel::core::ChannelManager;
use rua_command::*;
use rua_security::{ApprovalAuthority, ApprovalRequestInput, ApprovalTrigger, ToolCallSnapshot};
use rua_server::control_adapters::*;
use rua_server::services::conversation_authority::{
    ConversationActiveTurnPhase, ConversationActiveTurnStart, ConversationAuthority,
    ConversationDriverKind,
};
use rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch;
use rua_server::services::foreground_operation::ForegroundOperationRegistry;
use rua_server::streaming::ActiveStreamRegistry;
use rua_workspace::{ActiveSessionTracker, SessionConcurrencyPolicy};
use tokio_util::sync::CancellationToken;

struct TestApproval;
#[async_trait]
impl rua_command::ApprovalService for TestApproval {
    async fn approve_pending(&self, _: &str) -> bool {
        false
    }
    async fn reject_pending(&self, _: &str) -> bool {
        false
    }
}

struct TestReloader;
#[async_trait]
impl rua_command::WorkspaceReloader for TestReloader {
    async fn reload_all(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

struct TestSkillsLister;
#[async_trait]
impl rua_command::SkillsLister for TestSkillsLister {
    async fn list_skills(&self, _: &str, _: &str) -> String {
        String::new()
    }
}

#[tokio::test]
async fn stream_cancel_adapter_delegates_to_active_stream_registry() {
    let registry = Arc::new(ActiveStreamRegistry::new());
    let adapter = StreamCancelAdapter::new(Arc::clone(&registry), "default");

    let cancelled = adapter.cancel_session("console", "sess-1").await;
    assert!(!cancelled);

    let count = adapter.active_session_count().await;
    assert_eq!(count, 0);
}

#[tokio::test]
async fn stream_cancel_adapter_keeps_channel_sessions_out_of_chat_stream_namespace() {
    let registry = Arc::new(ActiveStreamRegistry::new());
    let runtime_sessions = Arc::new(ActiveSessionTracker::new(
        SessionConcurrencyPolicy::CancelPrevious,
    ));
    let cancel = CancellationToken::new();
    runtime_sessions
        .try_acquire("channel-session-1", "request-1", "default", cancel.clone())
        .await
        .expect("acquire runtime session");
    let adapter = StreamCancelAdapter::new(registry, "default")
        .with_runtime_sessions(Arc::clone(&runtime_sessions));

    assert!(
        !rua_command::SessionCanceller::cancel_session(&adapter, "console", "channel-session-1",)
            .await
    );
    assert!(!cancel.is_cancelled());

    assert!(
        rua_command::SessionCanceller::cancel_session(&adapter, "dingtalk", "channel-session-1",)
            .await
    );
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn channel_manager_dispatches_stop_via_command_dispatcher() {
    let cm = ChannelManager::new();

    let called = Arc::new(AtomicBool::new(false));
    let called_clone = Arc::clone(&called);

    struct TestCanceller {
        called: Arc<AtomicBool>,
    }
    #[async_trait]
    impl rua_command::SessionCanceller for TestCanceller {
        async fn cancel_session(&self, _channel_id: &str, _session_id: &str) -> bool {
            self.called.store(true, Ordering::SeqCst);
            true
        }
        async fn active_session_count(&self) -> usize {
            0
        }
    }

    let canceller: Arc<dyn rua_command::SessionCanceller> = Arc::new(TestCanceller {
        called: called_clone,
    });

    let mut control_registry = ControlCommandRegistry::new();
    control_registry.register(Arc::new(StopCommand));

    let dispatcher = Arc::new(CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        control_registry,
        ConversationCommandRegistry::new(),
    ));

    cm.set_command_dispatcher(
        dispatcher,
        Arc::clone(&canceller),
        Arc::new(TestApproval) as Arc<dyn rua_command::ApprovalService>,
        Arc::new(NoopQueueClearer) as Arc<dyn rua_command::QueueClearer>,
        Arc::new(TestSkillsLister) as Arc<dyn rua_command::SkillsLister>,
        std::path::PathBuf::from("/tmp/test"),
        std::time::Instant::now(),
        Arc::new(TestReloader) as Arc<dyn rua_command::WorkspaceReloader>,
        None,
    )
    .await;

    let result = cm
        .handle_incoming(
            "console".into(),
            "sess-1".into(),
            serde_json::json!({"text": "/stop"}),
        )
        .await;

    assert!(result.is_ok());
    assert!(called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn control_adapters_noop_queue_clearer_returns_zero() {
    let q = NoopQueueClearer;
    assert_eq!(
        rua_command::QueueClearer::clear_queue(&q, "c", "s").await,
        0
    );
}

#[tokio::test]
async fn approval_adapter_reacquires_execution_residency_before_resolving() {
    let foreground_operations = Arc::new(ForegroundOperationRegistry::new(true));
    foreground_operations.enter_background(false);
    let conversation_authority = Arc::new(ConversationAuthority::with_foreground_operations(
        Arc::clone(&foreground_operations),
    ));
    let active_turn = conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-1".to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: "device-mobile".to_string(),
            authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    conversation_authority
        .transition_active_turn_phase(
            &rua_core::ChatAddress::agent("default", "chat-1"),
            Some(&active_turn.turn_id),
            ConversationActiveTurnPhase::WaitingApproval,
        )
        .await
        .expect("waiting approval");
    assert_eq!(foreground_operations.snapshot().active_count, 0);

    let approval_authority = Arc::new(ApprovalAuthority::new());
    let approval = approval_authority
        .create_request(ApprovalRequestInput {
            agent_id: "default".to_string(),
            owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                "default", "chat-1",
            )),
            turn_id: active_turn.turn_id.clone(),
            tool_call_id: "call-1".to_string(),
            tool_name: "Shell".to_string(),
            trigger: ApprovalTrigger::approval_mode("Shell requires approval"),
            tool_call: ToolCallSnapshot {
                display: "Shell".to_string(),
                args: serde_json::json!({}),
            },
        })
        .await;
    let adapter = ApprovalAdapter::new(Arc::clone(&approval_authority), "default")
        .with_conversation_authority(Arc::clone(&conversation_authority));

    assert!(rua_command::ApprovalService::approve_request(&adapter, &approval.id, "test",).await);
    assert_eq!(foreground_operations.snapshot().active_count, 1);
    assert_eq!(
        conversation_authority
            .active_turn(&rua_core::ChatAddress::agent("default", "chat-1"))
            .await
            .expect("active turn")
            .phase,
        ConversationActiveTurnPhase::RunningTool
    );
}

#[tokio::test]
async fn approval_adapter_resolves_runtime_session_without_fabricating_chat_address() {
    let approval_authority = Arc::new(ApprovalAuthority::new());
    let approval = approval_authority
        .create_request(ApprovalRequestInput {
            agent_id: "default".to_string(),
            owner: rua_security::ApprovalOwner::runtime_session("default", "acp-session-1"),
            turn_id: "turn-1".to_string(),
            tool_call_id: "call-1".to_string(),
            tool_name: "Shell".to_string(),
            trigger: ApprovalTrigger::approval_mode("Shell requires approval"),
            tool_call: ToolCallSnapshot {
                display: "Shell".to_string(),
                args: serde_json::json!({}),
            },
        })
        .await;
    let adapter = ApprovalAdapter::new(Arc::clone(&approval_authority), "default");

    assert!(rua_command::ApprovalService::approve_pending(&adapter, "acp-session-1").await);
    assert_eq!(
        approval_authority
            .request(&approval.id)
            .await
            .expect("approval")
            .status,
        rua_security::AuthorityStatus::Approved
    );
    assert!(approval_authority
        .snapshot_at(&rua_core::ChatAddress::agent("default", "acp-session-1"))
        .await
        .pending
        .is_empty());
}
