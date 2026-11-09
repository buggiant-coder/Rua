use std::path::PathBuf;

use rua_core::RuaError;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool not found: {name}")]
    NotFound { name: String },

    #[error("duplicate tool name: {name}")]
    Duplicate { name: String },

    #[error("schema validation failed for tool `{tool}`: {message}")]
    SchemaValidation { tool: String, message: String },

    #[error("json repair failed for tool `{tool}`: {message}")]
    JsonRepairFailed { tool: String, message: String },

    #[error("path security violation: {message}")]
    PathSecurity { message: String },

    #[error("file not found: {}", path.display())]
    FileNotFound { path: PathBuf },

    #[error("binary file detected: {}", path.display())]
    BinaryFile { path: PathBuf },

    #[error("file too large: {} ({size} bytes, max {max} bytes)", path.display())]
    FileTooLarge { path: PathBuf, size: u64, max: u64 },

    #[error("edit error in {}: {message}", path.display())]
    EditError { path: PathBuf, message: String },

    #[error("command execution failed: {message}")]
    CommandFailed { message: String },

    #[error("command timed out after {timeout_ms}ms")]
    CommandTimeout { timeout_ms: u64 },

    #[error("command was cancelled")]
    CommandCancelled,

    #[error("shell execution is disabled: {reason}")]
    ExecutorDisabled { reason: String },

    #[error("tool `{tool}` denied: {reason}")]
    Denied { tool: String, reason: String },

    #[error("approval {status} for tool `{tool}`")]
    ApprovalFailed { tool: String, status: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("access to secret directory denied: {}", path.display())]
    SecretDirDenied { path: PathBuf },

    #[error("atomic write failed for {}: {message}", path.display())]
    AtomicWriteFailed { path: PathBuf, message: String },

    #[error("result store error: {message}")]
    ResultStore { message: String },

    #[error("tool `{tool}` is not supported on platform `{platform}`: {reason}")]
    UnsupportedOnPlatform {
        tool: String,
        platform: String,
        reason: String,
    },

    #[error("SSRF protection: access to {address} is forbidden")]
    SsrfBlocked { address: String },

    #[error("web fetch error: {message}")]
    WebFetch { message: String },

    #[error("web search error: {message}")]
    WebSearch { message: String },

    #[error("search error: {message}")]
    Search { message: String },

    #[error("regex rejected: pattern too complex or dangerous")]
    RegexTooComplex,

    #[error("user interaction unavailable: {reason}")]
    InteractionUnavailable { reason: String },

    #[error("agent collaboration error: {message}")]
    AgentCollab { message: String },

    #[error("recursion limit exceeded: depth {depth} exceeds max {max}")]
    RecursionLimit { depth: u32, max: u32 },

    #[error("{0}")]
    Internal(String),
}

impl From<ToolError> for RuaError {
    fn from(e: ToolError) -> Self {
        match &e {
            ToolError::NotFound { name } => RuaError::NotFound {
                resource: "tool".into(),
                id: name.clone(),
            },
            ToolError::Denied { .. } | ToolError::SecretDirDenied { .. } => RuaError::Security {
                message: e.to_string(),
            },
            ToolError::CommandCancelled => RuaError::Cancelled,
            ToolError::CommandTimeout { .. } => RuaError::Timeout {
                operation: e.to_string(),
            },
            _ => RuaError::Tool {
                tool: "rua-tool".into(),
                message: e.to_string(),
            },
        }
    }
}
