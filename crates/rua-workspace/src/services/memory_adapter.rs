use std::sync::Arc;

use async_trait::async_trait;
use rua_agent::memory_manager::MemoryManager;

use crate::error::WorkspaceError;
use crate::service_manager::ServiceLifecycle;

/// Startup of the real per-Agent memory store.
///
/// The store itself is opened by `WorkspaceRuntime`; this lifecycle proves the
/// assembled store can answer a status probe, so a broken store is reported as
/// failed instead of being declared `Ready` by a placeholder. The resulting
/// state is recorded by [`crate::service_manager::ServiceManager`], the single
/// accounting owner. `stop` has no work because the store is opened per call.
pub struct MemoryServiceAdapter {
    manager: Arc<dyn MemoryManager>,
}

impl MemoryServiceAdapter {
    pub fn new(manager: Arc<dyn MemoryManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ServiceLifecycle for MemoryServiceAdapter {
    async fn start(&self) -> Result<(), WorkspaceError> {
        match self.manager.health_check().await {
            Ok(status) if status.healthy => Ok(()),
            Ok(status) => Err(WorkspaceError::component_init_failed(
                "memory",
                format!("memory store unhealthy: {}", status.issues.join("; ")),
            )),
            Err(e) => Err(WorkspaceError::component_init_failed(
                "memory",
                e.to_string(),
            )),
        }
    }

    async fn stop(&self) -> Result<(), WorkspaceError> {
        Ok(())
    }
}
