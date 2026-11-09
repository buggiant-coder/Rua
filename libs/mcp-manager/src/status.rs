use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionState {
    Disabled,

    NotConnected,

    Connecting,

    Ready,

    Degraded,

    Failed,

    Stopping,
}

impl McpConnectionState {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Ready | Self::Degraded)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Disabled | Self::NotConnected | Self::Failed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub key: String,

    pub name: String,

    pub enabled: bool,

    pub state: McpConnectionState,

    pub transport_type: String,

    pub tools_count: usize,

    pub last_error: Option<String>,

    pub connected_at: Option<DateTime<Utc>>,

    pub mobile_unavailable_reason: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_errors: Vec<String>,
}

impl McpServerStatus {
    pub fn disabled(
        key: impl Into<String>,
        name: impl Into<String>,
        transport_type: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            name: name.into(),
            enabled: false,
            state: McpConnectionState::Disabled,
            transport_type: transport_type.into(),
            tools_count: 0,
            last_error: None,
            connected_at: None,
            mobile_unavailable_reason: None,
            config_errors: Vec::new(),
        }
    }

    pub fn not_connected(
        key: impl Into<String>,
        name: impl Into<String>,
        transport_type: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            name: name.into(),
            enabled: true,
            state: McpConnectionState::NotConnected,
            transport_type: transport_type.into(),
            tools_count: 0,
            last_error: None,
            connected_at: None,
            mobile_unavailable_reason: None,
            config_errors: Vec::new(),
        }
    }
}
