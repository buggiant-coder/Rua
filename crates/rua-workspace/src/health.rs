use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::lifecycle::WorkspaceLifecycleState;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ComponentState {
    Disabled,

    #[default]
    NotInitialized,

    Initializing,

    Ready,

    Degraded {
        reason: String,
    },

    Failed {
        reason: String,
    },
}

impl ComponentState {
    pub fn is_healthy(&self) -> bool {
        !matches!(self, Self::Failed { .. })
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Ready | Self::Degraded { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceHealth {
    pub agent_id: String,

    pub lifecycle: WorkspaceLifecycleState,

    pub components: HashMap<String, ComponentState>,

    pub active_sessions: usize,

    pub background_tasks: usize,
}

impl WorkspaceHealth {
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            lifecycle: WorkspaceLifecycleState::New,
            components: HashMap::new(),
            active_sessions: 0,
            background_tasks: 0,
        }
    }

    pub fn set_component(&mut self, name: impl Into<String>, state: ComponentState) {
        self.components.insert(name.into(), state);
    }

    pub fn is_healthy(&self) -> bool {
        self.lifecycle.is_healthy() && self.components.values().all(|s| s.is_healthy())
    }

    pub fn failed_components(&self) -> Vec<(&str, &str)> {
        self.components
            .iter()
            .filter_map(|(name, state)| {
                if let ComponentState::Failed { reason } = state {
                    Some((name.as_str(), reason.as_str()))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn degraded_components(&self) -> Vec<(&str, &str)> {
        self.components
            .iter()
            .filter_map(|(name, state)| {
                if let ComponentState::Degraded { reason } = state {
                    Some((name.as_str(), reason.as_str()))
                } else {
                    None
                }
            })
            .collect()
    }

    pub const COMPONENT_AGENT: &'static str = "agent";
    pub const COMPONENT_PROVIDER: &'static str = "provider";
    pub const COMPONENT_MEMORY: &'static str = "memory";
    pub const COMPONENT_MCP: &'static str = "mcp";
    pub const COMPONENT_TOOLS: &'static str = "tools";
    pub const COMPONENT_CHANNELS: &'static str = "channels";
    pub const COMPONENT_CRON: &'static str = "cron";
    pub const COMPONENT_SKILLS: &'static str = "skills";
    pub const COMPONENT_SECURITY: &'static str = "security";
}

pub struct WorkspaceHealthBuilder {
    health: WorkspaceHealth,
}

impl WorkspaceHealthBuilder {
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            health: WorkspaceHealth::new(agent_id),
        }
    }

    pub fn lifecycle(mut self, state: WorkspaceLifecycleState) -> Self {
        self.health.lifecycle = state;
        self
    }

    pub fn component(mut self, name: impl Into<String>, state: ComponentState) -> Self {
        self.health.set_component(name, state);
        self
    }

    pub fn active_sessions(mut self, count: usize) -> Self {
        self.health.active_sessions = count;
        self
    }

    pub fn background_tasks(mut self, count: usize) -> Self {
        self.health.background_tasks = count;
        self
    }

    pub fn build(self) -> WorkspaceHealth {
        self.health
    }
}
