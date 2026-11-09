use std::path::PathBuf;
use std::sync::Arc;

use rua_workspace::context::WorkspaceContext;
use rua_workspace::registry::AgentResolver;
use rua_workspace::{RuaRoot, WorkspaceRoot};

#[allow(dead_code)]
pub fn agent_workspace(root: &RuaRoot, agent_id: &str) -> WorkspaceRoot {
    root.agent_workspace(agent_id)
        .expect("test Agent ID must be a valid Workspace owner")
}

pub fn test_context(working_dir: PathBuf) -> Arc<WorkspaceContext> {
    let secret_dir = working_dir.join("secret");
    let cache_dir = working_dir.with_extension("cache");
    let _ = std::fs::create_dir_all(&secret_dir);
    let cell_store = Arc::new(cell_store::FileSystemCellStore::new());
    Arc::new(WorkspaceContext {
        agent_cells: Arc::new(rua_agent::AgentCells::new(
            Arc::clone(&cell_store),
            working_dir.join("workspaces"),
        )),
        agent_profile_store: Arc::new(rua_agent::AgentProfileStore::new(working_dir.clone())),
        provider_manager: Arc::new(rua_provider::manager::ProviderManager::new()),
        plugin_registry: Arc::new(rua_plugin_runtime::PluginRegistry::new()),
        active_plugin_registry: Arc::new(rua_plugin_runtime::ActivePluginRegistry::new()),
        security: Arc::new(rua_security::SecurityModule::new(
            Arc::new(rua_security::SecurityConfigStore::new(working_dir.clone())),
            None,
        )),
        task_runtime: Arc::new(
            rua_task::TaskRuntime::open(rua_task::TaskRuntimeConfig::new(working_dir.clone()))
                .unwrap(),
        ),
        collaboration_runtime: Arc::new(()),
        sync_transfer_runtime: Arc::new(()),
        tool_config_store: Arc::new(rua_tool::ToolConfigStore::new(Arc::new(
            rua_tool::AgentToolsCells::new(cell_store, working_dir.join("workspaces")),
        ))),
        tool_before_boundary: None,
        tool_observer_boundary: None,
        permission_request_boundary: None,
        rua_root: working_dir,
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
