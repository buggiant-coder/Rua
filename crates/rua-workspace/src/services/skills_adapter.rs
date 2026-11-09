use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rua_skills::{
    ensure_skill_pool_initialized, ensure_skill_pool_initialized_in, reconcile_workspace_manifest,
    SkillService,
};

use crate::error::WorkspaceError;
use crate::service_manager::ServiceLifecycle;

/// Startup of the skill pool and the per-Agent skill manifest.
///
/// `start` performs the real pool/reconcile work; the observed state is owned by
/// [`crate::service_manager::ServiceManager`]. `stop` has no work because the
/// skill pool is shared and outlives a single workspace.
pub struct SkillsServiceAdapter {
    service: Arc<SkillService>,

    pool_dir_override: Option<PathBuf>,
}

impl SkillsServiceAdapter {
    pub fn new(service: Arc<SkillService>) -> Self {
        Self {
            service,
            pool_dir_override: None,
        }
    }

    pub fn with_pool_dir(mut self, pool_dir: PathBuf) -> Self {
        self.pool_dir_override = Some(pool_dir);
        self
    }

    pub fn service(&self) -> Arc<SkillService> {
        Arc::clone(&self.service)
    }
}

#[async_trait]
impl ServiceLifecycle for SkillsServiceAdapter {
    async fn start(&self) -> Result<(), WorkspaceError> {
        let pool_init = match &self.pool_dir_override {
            Some(dir) => ensure_skill_pool_initialized_in(dir),
            None => ensure_skill_pool_initialized(),
        };
        if let Err(e) = pool_init {
            let reason = format!("ensure_skill_pool_initialized failed: {e}");
            return Err(WorkspaceError::component_init_failed("skills", reason));
        }

        if let Err(e) = reconcile_workspace_manifest(self.service.workspace_dir()) {
            let reason = format!("reconcile_workspace_manifest failed: {e}");
            return Err(WorkspaceError::component_init_failed("skills", reason));
        }

        Ok(())
    }

    async fn stop(&self) -> Result<(), WorkspaceError> {
        Ok(())
    }
}
