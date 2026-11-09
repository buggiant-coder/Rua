use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rua_command::*;

struct RecordingCanceller {
    called: AtomicBool,
    last_channel: tokio::sync::Mutex<String>,
    last_session: tokio::sync::Mutex<String>,
}

impl RecordingCanceller {
    fn new() -> Self {
        Self {
            called: AtomicBool::new(false),
            last_channel: tokio::sync::Mutex::new(String::new()),
            last_session: tokio::sync::Mutex::new(String::new()),
        }
    }
}

#[async_trait]
impl SessionCanceller for RecordingCanceller {
    async fn cancel_session(&self, channel_id: &str, session_id: &str) -> bool {
        self.called.store(true, Ordering::SeqCst);
        *self.last_channel.lock().await = channel_id.to_string();
        *self.last_session.lock().await = session_id.to_string();
        true
    }

    async fn active_session_count(&self) -> usize {
        1
    }
}

struct RecordingQueueClearer {
    cleared_count: AtomicUsize,
}

impl RecordingQueueClearer {
    fn new() -> Self {
        Self {
            cleared_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl QueueClearer for RecordingQueueClearer {
    async fn clear_queue(&self, _channel_id: &str, _session_id: &str) -> usize {
        self.cleared_count.fetch_add(1, Ordering::SeqCst);
        0
    }
}

struct NoopApproval;
#[async_trait]
impl ApprovalService for NoopApproval {
    async fn approve_pending(&self, _: &str) -> bool {
        false
    }
    async fn reject_pending(&self, _: &str) -> bool {
        false
    }
}

struct NoopSkillsLister;
#[async_trait]
impl SkillsLister for NoopSkillsLister {
    async fn list_skills(&self, _: &str, _: &str) -> String {
        String::new()
    }
}

struct MockAcpChatController;
#[async_trait]
impl AcpChatController for MockAcpChatController {
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

#[tokio::test]
async fn stop_command_calls_cancel_with_channel_and_session() {
    let canceller = Arc::new(RecordingCanceller::new());
    let queue = Arc::new(RecordingQueueClearer::new());

    let ctx = ControlContext {
        session_id: "sess-1".to_string(),
        user_id: "user-1".to_string(),
        channel: "console".to_string(),
        session_canceller: Arc::clone(&canceller) as Arc<dyn SessionCanceller>,
        approval_service: Arc::new(NoopApproval) as Arc<dyn ApprovalService>,
        queue_clearer: Arc::clone(&queue) as Arc<dyn QueueClearer>,
        skills_lister: Arc::new(NoopSkillsLister) as Arc<dyn SkillsLister>,
        acp_chat_manager: Arc::new(rua_command::AcpChatSessionManager::new(
            std::time::Duration::from_secs(300),
        )),
        acp_chat_controller: Arc::new(MockAcpChatController),
        locale: "zh".into(),
    };

    let cmd = StopCommand;
    let outcome = cmd.execute(&ctx, "").await;

    assert!(canceller.called.load(Ordering::SeqCst));
    assert_eq!(*canceller.last_channel.lock().await, "console");
    assert_eq!(*canceller.last_session.lock().await, "sess-1");

    let text: String = outcome
        .events
        .iter()
        .filter_map(|e| match e {
            agent_types::AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "任务已停止");
}

#[tokio::test]
async fn stop_command_with_session_arg_uses_specified_session() {
    let canceller = Arc::new(RecordingCanceller::new());

    let ctx = ControlContext {
        session_id: "default-sess".to_string(),
        user_id: "user-1".to_string(),
        channel: "console".to_string(),
        session_canceller: Arc::clone(&canceller) as Arc<dyn SessionCanceller>,
        approval_service: Arc::new(NoopApproval) as Arc<dyn ApprovalService>,
        queue_clearer: Arc::new(RecordingQueueClearer::new()) as Arc<dyn QueueClearer>,
        skills_lister: Arc::new(NoopSkillsLister) as Arc<dyn SkillsLister>,
        acp_chat_manager: Arc::new(rua_command::AcpChatSessionManager::new(
            std::time::Duration::from_secs(300),
        )),
        acp_chat_controller: Arc::new(MockAcpChatController),
        locale: "zh".into(),
    };

    let cmd = StopCommand;
    cmd.execute(&ctx, "session=other-sess").await;

    assert_eq!(*canceller.last_session.lock().await, "other-sess");
}

#[tokio::test]
async fn stop_command_no_active_task_returns_no_active_message() {
    struct NeverCancels;
    #[async_trait]
    impl SessionCanceller for NeverCancels {
        async fn cancel_session(&self, _: &str, _: &str) -> bool {
            false
        }
        async fn active_session_count(&self) -> usize {
            0
        }
    }

    let ctx = ControlContext {
        session_id: "s".to_string(),
        user_id: "u".to_string(),
        channel: "c".to_string(),
        session_canceller: Arc::new(NeverCancels),
        approval_service: Arc::new(NoopApproval),
        queue_clearer: Arc::new(RecordingQueueClearer::new()),
        skills_lister: Arc::new(NoopSkillsLister),
        acp_chat_manager: Arc::new(rua_command::AcpChatSessionManager::new(
            std::time::Duration::from_secs(300),
        )),
        acp_chat_controller: Arc::new(MockAcpChatController),
        locale: "zh".into(),
    };

    let outcome = StopCommand.execute(&ctx, "").await;
    let text: String = outcome
        .events
        .iter()
        .filter_map(|e| match e {
            agent_types::AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "没有正在进行的生成可停止。");
}
