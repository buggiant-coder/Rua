use std::fmt;

use serde::{Deserialize, Serialize};

pub type McpResult<T> = Result<T, McpError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpError {
    ConfigInvalid {
        message: String,
    },

    CommandNotFound {
        command: String,
    },

    PermissionDenied {
        message: String,
    },

    StartupTimeout {
        server: String,
        timeout_secs: u64,
    },

    HandshakeFailed {
        server: String,
        message: String,
    },

    ProtocolError {
        server: String,
        message: String,
    },

    ToolNotFound {
        server: String,
        tool: String,
    },

    SchemaValidationFailed {
        server: String,
        tool: String,
        message: String,
    },

    CallTimeout {
        server: String,
        tool: String,
        timeout_secs: u64,
    },

    TransportClosed {
        server: String,
        message: String,
    },

    ServerCrashed {
        server: String,
        message: String,
    },

    AuthFailed {
        server: String,
        status_code: u16,
        message: String,
    },

    SsrfBlocked {
        server: String,
        url: String,
    },

    MobilePolicyDenied {
        server: String,
        reason: String,
    },

    Cancelled {
        server: String,
    },

    ServerNotFound {
        server: String,
    },

    Internal {
        message: String,
    },
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigInvalid { message } => write!(f, "MCP config invalid: {message}"),
            Self::CommandNotFound { command } => {
                write!(f, "MCP command not found: {command}")
            }
            Self::PermissionDenied { message } => {
                write!(f, "MCP permission denied: {message}")
            }
            Self::StartupTimeout {
                server,
                timeout_secs,
            } => write!(
                f,
                "MCP server {server} startup timeout after {timeout_secs}s"
            ),
            Self::HandshakeFailed { server, message } => {
                write!(f, "MCP handshake failed ({server}): {message}")
            }
            Self::ProtocolError { server, message } => {
                write!(f, "MCP protocol error ({server}): {message}")
            }
            Self::ToolNotFound { server, tool } => {
                write!(f, "MCP tool not found ({server}): {tool}")
            }
            Self::SchemaValidationFailed {
                server,
                tool,
                message,
            } => write!(
                f,
                "MCP schema validation failed ({server}/{tool}): {message}"
            ),
            Self::CallTimeout {
                server,
                tool,
                timeout_secs,
            } => write!(
                f,
                "MCP tool call timeout ({server}/{tool}) after {timeout_secs}s"
            ),
            Self::TransportClosed { server, message } => {
                write!(f, "MCP transport closed ({server}): {message}")
            }
            Self::ServerCrashed { server, message } => {
                write!(f, "MCP server crashed ({server}): {message}")
            }
            Self::AuthFailed {
                server,
                status_code,
                message,
            } => {
                write!(
                    f,
                    "MCP auth failed ({server}): HTTP {status_code}: {message}"
                )
            }
            Self::SsrfBlocked { server, url } => {
                write!(f, "MCP SSRF blocked ({server}): {url}")
            }
            Self::MobilePolicyDenied { server, reason } => {
                write!(f, "MCP mobile policy denied ({server}): {reason}")
            }
            Self::Cancelled { server } => write!(f, "MCP operation cancelled ({server})"),
            Self::ServerNotFound { server } => write!(f, "MCP server not found: {server}"),
            Self::Internal { message } => write!(f, "MCP internal error: {message}"),
        }
    }
}

impl std::error::Error for McpError {}

impl McpError {
    pub fn server_id(&self) -> Option<&str> {
        match self {
            Self::ConfigInvalid { .. }
            | Self::CommandNotFound { .. }
            | Self::PermissionDenied { .. }
            | Self::Internal { .. } => None,
            Self::StartupTimeout { server, .. }
            | Self::HandshakeFailed { server, .. }
            | Self::ProtocolError { server, .. }
            | Self::ToolNotFound { server, .. }
            | Self::SchemaValidationFailed { server, .. }
            | Self::CallTimeout { server, .. }
            | Self::TransportClosed { server, .. }
            | Self::ServerCrashed { server, .. }
            | Self::AuthFailed { server, .. }
            | Self::SsrfBlocked { server, .. }
            | Self::MobilePolicyDenied { server, .. }
            | Self::Cancelled { server }
            | Self::ServerNotFound { server } => Some(server),
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::StartupTimeout { .. }
                | Self::TransportClosed { .. }
                | Self::CallTimeout { .. }
                | Self::ServerCrashed { .. }
        )
    }

    pub fn is_config_error(&self) -> bool {
        matches!(
            self,
            Self::ConfigInvalid { .. }
                | Self::CommandNotFound { .. }
                | Self::PermissionDenied { .. }
                | Self::AuthFailed { .. }
                | Self::SsrfBlocked { .. }
                | Self::MobilePolicyDenied { .. }
        )
    }
}
