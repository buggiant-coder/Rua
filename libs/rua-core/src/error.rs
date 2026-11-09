use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuaError {
    Config {
        message: String,
    },

    Provider {
        provider: String,
        message: String,
        retryable: bool,
    },

    Runtime {
        message: String,
    },

    Tool {
        tool: String,
        message: String,
    },

    Mcp {
        server: String,
        message: String,
    },

    Memory {
        message: String,
    },

    Cron {
        job_id: String,
        message: String,
    },

    Security {
        message: String,
    },

    Io {
        message: String,
    },

    Timeout {
        operation: String,
    },

    Cancelled,

    NotFound {
        resource: String,
        id: String,
    },

    NotImplemented {
        feature: String,
    },

    Validation {
        message: String,
    },

    Conflict {
        resource: String,
        detail: String,
    },
}

impl fmt::Display for RuaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config { message } => write!(f, "config error: {message}"),
            Self::Provider {
                provider, message, ..
            } => write!(f, "provider error ({provider}): {message}"),
            Self::Runtime { message } => write!(f, "runtime error: {message}"),
            Self::Tool { tool, message } => write!(f, "tool error ({tool}): {message}"),
            Self::Mcp { server, message } => write!(f, "MCP error ({server}): {message}"),
            Self::Memory { message } => write!(f, "memory error: {message}"),
            Self::Cron { job_id, message } => write!(f, "cron error ({job_id}): {message}"),
            Self::Security { message } => write!(f, "security: {message}"),
            Self::Io { message } => write!(f, "IO error: {message}"),
            Self::Timeout { operation } => write!(f, "timeout: {operation}"),
            Self::Cancelled => write!(f, "operation cancelled"),
            Self::NotFound { resource, id } => write!(f, "{resource} not found: {id}"),
            Self::NotImplemented { feature } => write!(f, "not implemented: {feature}"),
            Self::Validation { message } => write!(f, "validation error: {message}"),
            Self::Conflict { resource, detail } => {
                write!(f, "conflict ({resource}): {detail}")
            }
        }
    }
}

impl std::error::Error for RuaError {}

impl RuaError {
    pub fn status_code(&self) -> u16 {
        match self {
            Self::NotFound { .. } => 404,
            Self::NotImplemented { .. } => 501,
            Self::Validation { .. } => 422,
            Self::Conflict { .. } => 409,
            Self::Security { .. } => 403,
            Self::Cancelled => 499,
            Self::Timeout { .. } => 504,
            Self::Provider {
                retryable: true, ..
            } => 503,
            _ => 500,
        }
    }

    pub fn to_api_detail(&self) -> serde_json::Value {
        serde_json::json!({
            "detail": self.to_string(),
        })
    }
}

pub type Result<T> = std::result::Result<T, RuaError>;

impl From<std::io::Error> for RuaError {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            message: e.to_string(),
        }
    }
}
