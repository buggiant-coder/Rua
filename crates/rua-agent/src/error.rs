use std::fmt;

use rua_core::RuaError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentErrorKind {
    Config,

    AgentNotFound,

    ProviderSelection,

    Provider,

    Session,

    Prompt,

    Command,

    Media,

    Tool,

    SecurityDenied,

    ApprovalTimeout,

    Memory,

    ContextOverflow,

    Cancelled,

    Timeout,

    Plugin,

    Internal,
}

impl fmt::Display for AgentErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Config => "config",
            Self::AgentNotFound => "agent_not_found",
            Self::ProviderSelection => "provider_selection",
            Self::Provider => "provider",
            Self::Session => "session",
            Self::Prompt => "prompt",
            Self::Command => "command",
            Self::Media => "media",
            Self::Tool => "tool",
            Self::SecurityDenied => "security_denied",
            Self::ApprovalTimeout => "approval_timeout",
            Self::Memory => "memory",
            Self::ContextOverflow => "context_overflow",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::Plugin => "plugin",
            Self::Internal => "internal",
        };
        f.write_str(s)
    }
}

#[derive(Debug)]
pub struct AgentError {
    pub kind: AgentErrorKind,

    pub message: String,

    pub detail: Option<String>,

    pub retryable: bool,

    pub trace_id: Option<String>,
}

impl AgentError {
    pub fn new(kind: AgentErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            detail: None,
            retryable: false,
            trace_id: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn with_trace(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    pub fn not_found(agent_id: impl Into<String>) -> Self {
        let id = agent_id.into();
        Self::new(
            AgentErrorKind::AgentNotFound,
            format!("Agent not found: {id}"),
        )
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Config, message)
    }

    pub fn provider_selection(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::ProviderSelection, message)
    }

    pub fn provider(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Provider, message).with_retryable(true)
    }

    pub fn session(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Session, message)
    }

    pub fn prompt(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Prompt, message)
    }

    pub fn command(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Command, message)
    }

    pub fn media(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Media, message)
    }

    pub fn tool(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Tool, message)
    }

    pub fn security_denied(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::SecurityDenied, message)
    }

    pub fn approval_timeout(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::ApprovalTimeout, message)
    }

    pub fn memory(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Memory, message)
    }

    pub fn context_overflow(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::ContextOverflow, message)
    }

    pub fn cancelled() -> Self {
        Self::new(AgentErrorKind::Cancelled, "Operation cancelled")
    }

    pub fn timeout(operation: impl Into<String>) -> Self {
        let op = operation.into();
        Self::new(AgentErrorKind::Timeout, format!("Timeout: {op}"))
    }

    pub fn plugin(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Plugin, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Internal, message)
    }

    pub fn is_cancelled(&self) -> bool {
        self.kind == AgentErrorKind::Cancelled
    }

    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    pub fn user_message(&self) -> &str {
        &self.message
    }

    pub fn status_code(&self) -> u16 {
        match self.kind {
            AgentErrorKind::AgentNotFound => 404,
            AgentErrorKind::Config => 400,
            AgentErrorKind::ProviderSelection => 400,
            AgentErrorKind::SecurityDenied => 403,
            AgentErrorKind::ApprovalTimeout => 408,
            AgentErrorKind::Cancelled => 499,
            AgentErrorKind::Timeout => 504,
            AgentErrorKind::Provider if self.retryable => 503,
            AgentErrorKind::Media => 422,
            AgentErrorKind::Command => 400,
            AgentErrorKind::Prompt => 400,
            _ => 500,
        }
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.kind, self.message)
    }
}

impl std::error::Error for AgentError {}

impl From<AgentError> for RuaError {
    fn from(e: AgentError) -> Self {
        match e.kind {
            AgentErrorKind::AgentNotFound => RuaError::NotFound {
                resource: "agent".into(),
                id: e.message,
            },
            AgentErrorKind::Config => RuaError::Config { message: e.message },
            AgentErrorKind::Provider | AgentErrorKind::ProviderSelection => RuaError::Provider {
                provider: String::new(),
                message: e.message,
                retryable: e.retryable,
            },
            AgentErrorKind::SecurityDenied => RuaError::Security { message: e.message },
            AgentErrorKind::Cancelled => RuaError::Cancelled,
            AgentErrorKind::Timeout => RuaError::Timeout {
                operation: e.message,
            },
            AgentErrorKind::Memory => RuaError::Memory { message: e.message },
            AgentErrorKind::Tool => RuaError::Tool {
                tool: String::new(),
                message: e.message,
            },
            _ => RuaError::Runtime { message: e.message },
        }
    }
}

impl From<RuaError> for AgentError {
    fn from(e: RuaError) -> Self {
        match e {
            RuaError::NotFound { resource, id } if resource == "agent" => AgentError::not_found(id),
            RuaError::Config { message } => AgentError::config(message),
            RuaError::Provider { message, .. } => AgentError::provider(message),
            RuaError::Security { message } => AgentError::security_denied(message),
            RuaError::Cancelled => AgentError::cancelled(),
            RuaError::Timeout { operation } => AgentError::timeout(operation),
            RuaError::Memory { message } => AgentError::memory(message),
            RuaError::Tool { message, .. } => AgentError::tool(message),
            other => AgentError::internal(other.to_string()),
        }
    }
}

impl From<react_agent::AgentError> for AgentError {
    fn from(e: react_agent::AgentError) -> Self {
        match e {
            react_agent::AgentError::Cancelled => AgentError::cancelled(),
            react_agent::AgentError::ContextOverflow { message } => {
                AgentError::context_overflow(message)
            }
            react_agent::AgentError::Tool { tool, message } => {
                AgentError::tool(format!("{tool}: {message}"))
            }
            react_agent::AgentError::Model(llm_err) => AgentError::provider(llm_err.to_string()),
            other => AgentError::internal(other.to_string()),
        }
    }
}
