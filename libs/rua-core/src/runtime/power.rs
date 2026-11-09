use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct PowerScopeId(pub String);

impl PowerScopeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PowerScopeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for PowerScopeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for PowerScopeId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PowerReason {
    ChatForeground,

    CronBackground,

    McpOperation,

    FileTransfer,

    LocalInference,

    Maintenance,
}

impl fmt::Display for PowerReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::ChatForeground => "chat_foreground",
            Self::CronBackground => "cron_background",
            Self::McpOperation => "mcp_operation",
            Self::FileTransfer => "file_transfer",
            Self::LocalInference => "local_inference",
            Self::Maintenance => "maintenance",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerScope {
    pub id: PowerScopeId,
    pub reason: PowerReason,
    pub foreground_service: bool,
    pub wake_lock: bool,
    pub started_at: DateTime<Utc>,
    pub timeout_at: DateTime<Utc>,
}

impl PowerScope {
    pub fn new(reason: PowerReason, timeout_at: DateTime<Utc>) -> Self {
        let foreground_service = matches!(reason, PowerReason::CronBackground);
        let wake_lock = !matches!(reason, PowerReason::ChatForeground);
        Self {
            id: PowerScopeId::generate(),
            reason,
            foreground_service,
            wake_lock,
            started_at: Utc::now(),
            timeout_at,
        }
    }

    pub fn needs_foreground_service(&self) -> bool {
        self.foreground_service
    }

    pub fn holds_wake_lock(&self) -> bool {
        self.wake_lock
    }
}
