pub mod acp_chat;
pub mod control;
pub mod conversation;
pub mod daemon;
pub mod dispatch;
pub mod input_window;

pub use acp_chat::{
    is_acp_reserved_command, AcpChatController, AcpChatResumeResult, AcpChatSession,
    AcpChatSessionManager, ACP_RESERVED_COMMANDS,
};
pub use control::{
    build_control_registry, AcpCommand, AcpRCommand, AcphCommand, ApprovalService, ControlCommand,
    ControlCommandRegistry, ControlContext, NoAcpCommand, QueueClearer, SessionCanceller,
    SkillsCommand, SkillsLister, StopCommand,
};
pub use conversation::{
    ContextCompactorSeam, ConversationCommand, ConversationCommandRegistry, ConversationContext,
    ConversationSessionSeam, PendingActionInfo, PendingActionSeam, PromptFileSeam, SeamError,
    SeamResult,
};
pub use daemon::{
    build_daemon_registry, ApprovalCommand, ApproveCommand, DaemonCommand, DaemonCommandRegistry,
    DaemonContext, DaemonRouterCommand, DenyCommand, LogsCommand, ReloadConfigCommand,
    RestartCommand, StatusCommand, VersionCommand, WorkspaceReloader,
};
pub use dispatch::{
    is_command, parse_command, run_command_path, CommandDispatcher, CommandOutcomeKind,
};
pub use input_window::{agent_input_indices, input_window_start};

use agent_types::AgentEvent;

#[derive(Debug, Clone)]
pub struct CommandOutcome {
    pub events: Vec<AgentEvent>,
    pub last: bool,
    pub kind: CommandOutcomeKind,
}

impl CommandOutcome {
    pub fn text(message: impl Into<String>) -> Self {
        Self {
            events: vec![AgentEvent::TextDelta {
                delta: message.into(),
                metadata: serde_json::Value::Null,
            }],
            last: true,
            kind: CommandOutcomeKind::Success,
        }
    }

    pub fn text_with_metadata(message: impl Into<String>, metadata: serde_json::Value) -> Self {
        Self {
            events: vec![AgentEvent::TextDelta {
                delta: message.into(),
                metadata,
            }],
            last: true,
            kind: CommandOutcomeKind::Success,
        }
    }

    pub fn ephemeral(message: impl Into<String>) -> Self {
        Self::text_with_metadata(message, serde_json::json!({"ephemeral": "true"}))
    }

    pub fn clear_history(message: impl Into<String>) -> Self {
        Self::text_with_metadata(
            message,
            serde_json::json!({"clear_history": true, "clear_plan": true}),
        )
    }

    pub fn empty() -> Self {
        Self {
            events: Vec::new(),
            last: true,
            kind: CommandOutcomeKind::Success,
        }
    }

    pub fn is_agent_required(&self) -> bool {
        matches!(self.kind, CommandOutcomeKind::AgentRequired { .. })
    }
}

pub struct DispatchContext {
    pub daemon: DaemonContext,
    pub control: ControlContext,
    pub conversation: ConversationContext,
}
