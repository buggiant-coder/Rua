use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use rua_workspace::context::WorkspaceContext;
use rua_workspace::error::WorkspaceError;
use rua_workspace::lifecycle::PlatformProfile;
use rua_workspace::lifecycle::{ShutdownPolicy, ShutdownReport};
use rua_workspace::registry::{AgentResolver, WorkspaceRegistry};
use rua_workspace::root::RuaRoot;
use rua_workspace::runtime::{WorkspaceRuntime, WorkspaceSummary};

pub fn test_context(data_dir: std::path::PathBuf) -> Arc<WorkspaceContext> {
    let secret_dir = data_dir.join("secret");
    let cache_dir = data_dir.join("cache");
    let _ = std::fs::create_dir_all(&secret_dir);
    let cell_store = Arc::new(cell_store::FileSystemCellStore::new());
    Arc::new(WorkspaceContext {
        agent_cells: Arc::new(rua_agent::AgentCells::new(
            Arc::clone(&cell_store),
            data_dir.join("workspaces"),
        )),
        agent_profile_store: Arc::new(rua_agent::AgentProfileStore::new(data_dir.clone())),
        provider_manager: Arc::new(rua_provider::manager::ProviderManager::new()),
        plugin_registry: Arc::new(rua_plugin_runtime::PluginRegistry::new()),
        active_plugin_registry: Arc::new(rua_plugin_runtime::ActivePluginRegistry::new()),
        security: Arc::new(rua_security::SecurityModule::new(
            Arc::new(rua_security::SecurityConfigStore::new(data_dir.clone())),
            None,
        )),
        task_runtime: Arc::new(
            rua_task::TaskRuntime::open(rua_task::TaskRuntimeConfig::new(data_dir.clone()))
                .unwrap(),
        ),
        collaboration_runtime: Arc::new(()),
        sync_transfer_runtime: Arc::new(()),
        tool_config_store: Arc::new(rua_tool::ToolConfigStore::new(Arc::new(
            rua_tool::AgentToolsCells::new(cell_store, data_dir.join("workspaces")),
        ))),
        tool_before_boundary: None,
        tool_observer_boundary: None,
        permission_request_boundary: None,
        rua_root: data_dir,
        secret_dir,
        cache_dir,
        compactor: Arc::new(rua_agent::compaction::ConversationCompactor::new(
            Default::default(),
        )),
        usage_recorder: None,
        push_store_dir: None,
        agent_resolver: Arc::new(AgentResolver::new("default")),
        command_runtime_factory: std::sync::OnceLock::new(),
        session_replication_hook: std::sync::OnceLock::new(),
        memory_publication_hook: Arc::new(std::sync::OnceLock::new()),
    })
}

pub struct StubRegistry {
    runtimes: HashMap<String, Arc<WorkspaceRuntime>>,
}

impl StubRegistry {
    pub async fn with_runtime(
        agent_id: &str,
        tmp: &std::path::Path,
        agent_runtime: Arc<dyn rua_contract::agent::AgentRuntime>,
    ) -> Arc<Self> {
        let data_dir = tmp.join("data");
        let _ = std::fs::create_dir_all(&data_dir);
        let ctx = test_context(data_dir);
        let root = RuaRoot::new(tmp);
        let workspace = root
            .agent_workspace(agent_id)
            .expect("valid Agent Workspace");
        let rt =
            WorkspaceRuntime::new(agent_id, agent_id, workspace, PlatformProfile::Desktop, ctx);
        rt.set_react_runtime(agent_runtime).await;
        let mut runtimes = HashMap::new();
        runtimes.insert(agent_id.to_string(), Arc::new(rt));
        Arc::new(Self { runtimes })
    }
}

#[async_trait]
impl WorkspaceRegistry for StubRegistry {
    async fn get_or_create(&self, agent_id: &str) -> Result<Arc<WorkspaceRuntime>, WorkspaceError> {
        self.runtimes
            .get(agent_id)
            .cloned()
            .ok_or_else(|| WorkspaceError::agent_not_found(agent_id))
    }

    async fn get(&self, agent_id: &str) -> Option<Arc<WorkspaceRuntime>> {
        self.runtimes.get(agent_id).cloned()
    }

    async fn list(&self) -> Result<Vec<WorkspaceSummary>, WorkspaceError> {
        Ok(vec![])
    }

    async fn reload_agent(&self, _agent_id: &str) -> Result<(), WorkspaceError> {
        Ok(())
    }

    async fn remove(&self, _agent_id: &str) -> Result<(), WorkspaceError> {
        Ok(())
    }

    /// The stub holds no Agent runtime, so there is nothing to stop and nothing
    /// to confirm: the honest report is "not found", not a fake clean one.
    async fn remove_with_policy(
        &self,
        agent_id: &str,
        _policy: ShutdownPolicy,
    ) -> Result<ShutdownReport, WorkspaceError> {
        Err(WorkspaceError::agent_not_found(agent_id))
    }

    async fn shutdown_all(
        &self,
        _policy: ShutdownPolicy,
    ) -> Result<ShutdownReport, WorkspaceError> {
        Ok(ShutdownReport::default())
    }
}
