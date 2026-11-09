use std::fmt;

use rua_core::RuaError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceErrorKind {
    AgentNotFound,

    AgentAlreadyExists,

    ConfigInvalid,

    WorkspacePathInvalid,

    ProviderUnavailable,

    SessionBusy,

    CollaborationDenied,

    CollaborationLoop,

    BackgroundTaskNotFound,

    ComponentInitFailed,

    ComponentShutdownFailed,

    MobilePolicyDenied,

    Cancelled,

    Timeout,

    Internal,
}

impl fmt::Display for WorkspaceErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::AgentNotFound => "agent_not_found",
            Self::AgentAlreadyExists => "agent_already_exists",
            Self::ConfigInvalid => "config_invalid",
            Self::WorkspacePathInvalid => "workspace_path_invalid",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::SessionBusy => "session_busy",
            Self::CollaborationDenied => "collaboration_denied",
            Self::CollaborationLoop => "collaboration_loop",
            Self::BackgroundTaskNotFound => "background_task_not_found",
            Self::ComponentInitFailed => "component_init_failed",
            Self::ComponentShutdownFailed => "component_shutdown_failed",
            Self::MobilePolicyDenied => "mobile_policy_denied",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::Internal => "internal",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceError {
    pub kind: WorkspaceErrorKind,

    pub message: String,

    pub detail: Option<String>,
}

impl WorkspaceError {
    pub fn new(kind: WorkspaceErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn agent_not_found(agent_id: impl Into<String>) -> Self {
        let id = agent_id.into();
        Self::new(
            WorkspaceErrorKind::AgentNotFound,
            format!("Agent not found: {id}"),
        )
    }

    pub fn agent_already_exists(agent_id: impl Into<String>) -> Self {
        let id = agent_id.into();
        Self::new(
            WorkspaceErrorKind::AgentAlreadyExists,
            format!("Agent already exists: {id}"),
        )
    }

    pub fn config_invalid(message: impl Into<String>) -> Self {
        Self::new(WorkspaceErrorKind::ConfigInvalid, message)
    }

    pub fn path_invalid(message: impl Into<String>) -> Self {
        Self::new(WorkspaceErrorKind::WorkspacePathInvalid, message)
    }

    pub fn provider_unavailable(message: impl Into<String>) -> Self {
        Self::new(WorkspaceErrorKind::ProviderUnavailable, message)
    }

    pub fn session_busy(message: impl Into<String>) -> Self {
        Self::new(WorkspaceErrorKind::SessionBusy, message)
    }

    pub fn collaboration_denied(message: impl Into<String>) -> Self {
        Self::new(WorkspaceErrorKind::CollaborationDenied, message)
    }

    pub fn collaboration_loop(message: impl Into<String>) -> Self {
        Self::new(WorkspaceErrorKind::CollaborationLoop, message)
    }

    pub fn task_not_found(task_id: impl Into<String>) -> Self {
        let id = task_id.into();
        Self::new(
            WorkspaceErrorKind::BackgroundTaskNotFound,
            format!("Background task not found: {id}"),
        )
    }

    pub fn component_init_failed(component: impl Into<String>, reason: impl Into<String>) -> Self {
        let c = component.into();
        let r = reason.into();
        Self::new(
            WorkspaceErrorKind::ComponentInitFailed,
            format!("Component {c} init failed: {r}"),
        )
    }

    pub fn component_shutdown_failed(
        component: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        let c = component.into();
        let r = reason.into();
        Self::new(
            WorkspaceErrorKind::ComponentShutdownFailed,
            format!("Component {c} shutdown failed: {r}"),
        )
    }

    pub fn mobile_policy_denied(message: impl Into<String>) -> Self {
        Self::new(WorkspaceErrorKind::MobilePolicyDenied, message)
    }

    pub fn cancelled() -> Self {
        Self::new(WorkspaceErrorKind::Cancelled, "Operation cancelled")
    }

    pub fn timeout(operation: impl Into<String>) -> Self {
        let op = operation.into();
        Self::new(WorkspaceErrorKind::Timeout, format!("Timeout: {op}"))
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(WorkspaceErrorKind::Internal, message)
    }

    pub fn status_code(&self) -> u16 {
        match self.kind {
            WorkspaceErrorKind::AgentNotFound | WorkspaceErrorKind::BackgroundTaskNotFound => 404,
            WorkspaceErrorKind::AgentAlreadyExists => 409,
            WorkspaceErrorKind::ConfigInvalid | WorkspaceErrorKind::WorkspacePathInvalid => 400,
            WorkspaceErrorKind::ProviderUnavailable => 503,
            WorkspaceErrorKind::SessionBusy => 429,
            WorkspaceErrorKind::CollaborationDenied | WorkspaceErrorKind::MobilePolicyDenied => 403,
            WorkspaceErrorKind::CollaborationLoop => 422,
            WorkspaceErrorKind::Cancelled => 499,
            WorkspaceErrorKind::Timeout => 504,
            _ => 500,
        }
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.kind, self.message)
    }
}

impl std::error::Error for WorkspaceError {}

impl From<WorkspaceError> for RuaError {
    fn from(e: WorkspaceError) -> Self {
        match e.kind {
            WorkspaceErrorKind::AgentNotFound | WorkspaceErrorKind::BackgroundTaskNotFound => {
                RuaError::NotFound {
                    resource: "workspace".into(),
                    id: e.message,
                }
            }
            WorkspaceErrorKind::ConfigInvalid | WorkspaceErrorKind::WorkspacePathInvalid => {
                RuaError::Config { message: e.message }
            }
            WorkspaceErrorKind::CollaborationDenied | WorkspaceErrorKind::MobilePolicyDenied => {
                RuaError::Security { message: e.message }
            }
            WorkspaceErrorKind::Cancelled => RuaError::Cancelled,
            WorkspaceErrorKind::Timeout => RuaError::Timeout {
                operation: e.message,
            },
            _ => RuaError::Runtime { message: e.message },
        }
    }
}

impl From<rua_agent::AgentError> for WorkspaceError {
    fn from(e: rua_agent::AgentError) -> Self {
        match e.kind {
            rua_agent::AgentErrorKind::AgentNotFound => WorkspaceError::agent_not_found(e.message),
            rua_agent::AgentErrorKind::Config => WorkspaceError::config_invalid(e.message),
            rua_agent::AgentErrorKind::Cancelled => WorkspaceError::cancelled(),
            rua_agent::AgentErrorKind::Timeout => WorkspaceError::timeout(e.message),
            _ => WorkspaceError::internal(e.message),
        }
    }
}
