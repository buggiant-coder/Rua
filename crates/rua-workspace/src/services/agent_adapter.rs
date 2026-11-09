use std::sync::Arc;

use async_trait::async_trait;
use rua_contract::agent::{AgentRuntime, RuntimeWarmupContext};
use rua_core::id::AgentId;

use crate::error::WorkspaceError;
use crate::service_manager::ServiceLifecycle;

/// Startup and shutdown of the per-Agent runtime.
///
/// The observed state is owned by [`crate::service_manager::ServiceManager`];
/// this lifecycle only performs the real warmup / shutdown calls.
pub struct AgentServiceAdapter {
    runtime: Arc<dyn AgentRuntime>,
    agent_id: AgentId,
}

impl AgentServiceAdapter {
    pub fn new(runtime: Arc<dyn AgentRuntime>, agent_id: AgentId) -> Self {
        Self { runtime, agent_id }
    }
}

#[async_trait]
impl ServiceLifecycle for AgentServiceAdapter {
    async fn start(&self) -> Result<(), WorkspaceError> {
        let context = RuntimeWarmupContext {
            agent_id: self.agent_id.clone(),
        };
        self.runtime
            .warmup(context)
            .await
            .map_err(|error| WorkspaceError::component_init_failed("agent", error.to_string()))
    }

    async fn stop(&self) -> Result<(), WorkspaceError> {
        self.runtime
            .shutdown()
            .await
            .map_err(|error| WorkspaceError::component_shutdown_failed("agent", error.to_string()))
    }
}
