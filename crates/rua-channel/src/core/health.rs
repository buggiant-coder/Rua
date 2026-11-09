use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelHealth {
    Healthy,
    Degraded { reason: String },
    Unhealthy { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChannelLifecycle {
    Registered,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelStatus {
    pub lifecycle: ChannelLifecycle,
    pub refresh_error: Option<String>,
}

impl ChannelStatus {
    pub fn is_running(&self) -> bool {
        self.lifecycle == ChannelLifecycle::Running
    }

    pub fn health(&self) -> ChannelHealth {
        match &self.lifecycle {
            ChannelLifecycle::Running => match &self.refresh_error {
                Some(reason) => ChannelHealth::Degraded {
                    reason: reason.clone(),
                },
                None => ChannelHealth::Healthy,
            },
            ChannelLifecycle::Failed { reason } => ChannelHealth::Unhealthy {
                reason: reason.clone(),
            },
            _ => ChannelHealth::Unhealthy {
                reason: format!("Channel runtime is {}.", self.runtime_label()),
            },
        }
    }

    pub fn runtime_label(&self) -> &'static str {
        match self.lifecycle {
            ChannelLifecycle::Registered => "registered",
            ChannelLifecycle::Starting => "starting",
            ChannelLifecycle::Running => "running",
            ChannelLifecycle::Stopping => "stopping",
            ChannelLifecycle::Stopped => "stopped",
            ChannelLifecycle::Failed { .. } => "failed",
        }
    }
}
