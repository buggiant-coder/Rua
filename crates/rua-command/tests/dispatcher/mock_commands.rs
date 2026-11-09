use std::sync::Arc;

use async_trait::async_trait;
use rua_command::*;

pub struct MockConversationCmd {
    cmd_name: &'static str,
    response: &'static str,
    requires_agent: bool,
}

impl MockConversationCmd {
    pub fn new(name: &'static str, response: &'static str) -> Arc<Self> {
        Arc::new(Self {
            cmd_name: name,
            response,
            requires_agent: false,
        })
    }

    pub fn agent_required(name: &'static str, response: &'static str) -> Arc<Self> {
        Arc::new(Self {
            cmd_name: name,
            response,
            requires_agent: true,
        })
    }
}

#[async_trait]
impl ConversationCommand for MockConversationCmd {
    fn name(&self) -> &str {
        self.cmd_name
    }
    fn help(&self) -> &str {
        "mock conversation command"
    }
    fn agent_required(&self) -> bool {
        self.requires_agent
    }
    async fn execute(&self, _ctx: &ConversationContext, _args: &str) -> CommandOutcome {
        CommandOutcome::text(self.response)
    }
}

pub struct MockControlCmd {
    cmd_name: &'static str,
    response: &'static str,
}

impl MockControlCmd {
    pub fn new(name: &'static str, response: &'static str) -> Arc<Self> {
        Arc::new(Self {
            cmd_name: name,
            response,
        })
    }
}

#[async_trait]
impl ControlCommand for MockControlCmd {
    fn name(&self) -> &str {
        self.cmd_name
    }
    fn help(&self) -> &str {
        "mock control command"
    }
    async fn execute(&self, _ctx: &ControlContext, _args: &str) -> CommandOutcome {
        CommandOutcome::text(self.response)
    }
}

pub struct MockDaemonCmd {
    cmd_name: &'static str,
    response: &'static str,
}

impl MockDaemonCmd {
    pub fn new(name: &'static str, response: &'static str) -> Arc<Self> {
        Arc::new(Self {
            cmd_name: name,
            response,
        })
    }
}

#[async_trait]
impl DaemonCommand for MockDaemonCmd {
    fn name(&self) -> &str {
        self.cmd_name
    }
    fn help(&self) -> &str {
        "mock daemon command"
    }
    async fn execute(&self, _ctx: &DaemonContext, _args: &str) -> CommandOutcome {
        CommandOutcome::text(self.response)
    }
}

pub struct MockSessionCanceller;

#[async_trait]
impl SessionCanceller for MockSessionCanceller {
    async fn cancel_session(&self, _channel_id: &str, _session_id: &str) -> bool {
        false
    }
    async fn active_session_count(&self) -> usize {
        0
    }
}

pub struct MockQueueClearer;

#[async_trait]
impl QueueClearer for MockQueueClearer {
    async fn clear_queue(&self, _channel_id: &str, _session_id: &str) -> usize {
        0
    }
}

pub struct MockApprovalService;

#[async_trait]
impl ApprovalService for MockApprovalService {
    async fn approve_pending(&self, _session_id: &str) -> bool {
        false
    }
    async fn reject_pending(&self, _session_id: &str) -> bool {
        false
    }
}

pub struct MockWorkspaceReloader;

#[async_trait]
impl WorkspaceReloader for MockWorkspaceReloader {
    async fn reload_all(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

pub struct MockSkillsLister;

#[async_trait]
impl SkillsLister for MockSkillsLister {
    async fn list_skills(&self, _channel_id: &str, _locale: &str) -> String {
        "No skills are currently enabled for this channel.".to_string()
    }
}

pub struct MockAcpChatController;

#[async_trait]
impl AcpChatController for MockAcpChatController {
    async fn enter(&self, _chat_id: &str, _runner: &str, _cwd: Option<&str>) -> String {
        "mock acp enter".to_string()
    }
    async fn exit(&self, _chat_id: &str) -> String {
        "mock acp exit".to_string()
    }
    async fn list_sessions(&self, _chat_id: &str, _runner: &str, _cwd: Option<&str>) -> String {
        "mock acp list".to_string()
    }
    async fn resume(
        &self,
        _chat_id: &str,
        _runner: &str,
        _acp_session_id: &str,
        _cwd: Option<&str>,
    ) -> String {
        "mock acp resume".to_string()
    }
}

pub fn make_dispatch_context() -> DispatchContext {
    DispatchContext {
        daemon: DaemonContext {
            data_dir: std::path::PathBuf::from("/tmp/test"),
            started_at: std::time::Instant::now(),
            workspace_reloader: Arc::new(MockWorkspaceReloader),
            exit_fn: None,
            approval_service: Arc::new(MockApprovalService),
            locale: "zh".into(),
        },
        control: ControlContext {
            session_id: "test-session".to_string(),
            user_id: "test-user".to_string(),
            channel: "test-channel".to_string(),
            session_canceller: Arc::new(MockSessionCanceller),
            approval_service: Arc::new(MockApprovalService),
            queue_clearer: Arc::new(MockQueueClearer),
            skills_lister: Arc::new(MockSkillsLister),
            acp_chat_manager: Arc::new(AcpChatSessionManager::new(std::time::Duration::from_secs(
                300,
            ))),
            acp_chat_controller: Arc::new(MockAcpChatController),
            locale: "zh".into(),
        },
        conversation: ConversationContext::stub(
            "test-agent",
            "test-session",
            "test-user",
            "test-channel",
        ),
    }
}
