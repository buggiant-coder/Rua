use llm_multiplex::LlmError;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("model error: {0}")]
    Model(#[from] LlmError),

    #[error("tool error for `{tool}`: {message}")]
    Tool { tool: String, message: String },

    #[error("fatal tool error for `{tool}`: {message}")]
    ToolFatal { tool: String, message: String },

    #[error("tool not found: {name}")]
    ToolNotFound { name: String },

    #[error("invalid tool arguments for `{tool}`: {message}")]
    InvalidToolArgs { tool: String, message: String },

    #[error("tool `{tool}` denied: {reason}")]
    ToolDenied { tool: String, reason: String },

    #[error("tool `{tool}` requires approval: {prompt}")]
    ApprovalRequired { tool: String, prompt: String },

    #[error("max iterations ({max}) exceeded")]
    MaxIterations { max: u32 },

    #[error("agent execution cancelled")]
    Cancelled,

    #[error("agent execution timed out after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },

    #[error("context overflow: {message}")]
    ContextOverflow { message: String },

    #[error("config error: {0}")]
    Config(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl AgentError {
    pub fn is_retryable(&self) -> bool {
        match self {
            AgentError::Model(e) => e.is_retryable(),
            AgentError::ContextOverflow { .. } => true,
            _ => false,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        match self {
            AgentError::Cancelled => true,
            AgentError::Model(llm_err) => matches!(llm_err, LlmError::Cancelled),
            _ => false,
        }
    }

    pub fn is_fatal_tool_error(&self) -> bool {
        matches!(self, AgentError::ToolFatal { .. })
    }
}
