use super::test_helpers;

use std::sync::Arc;

use rua_workspace::context::WorkspaceContext;
use rua_workspace::registry::AgentResolver;

#[test]
fn workspace_context_has_all_fields() {
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret");
    let cache = dir.path().join("cache");
    std::fs::create_dir_all(&secret).unwrap();
    let cell_store = Arc::new(cell_store::FileSystemCellStore::new());

    let ctx = WorkspaceContext {
        agent_cells: Arc::new(rua_agent::AgentCells::new(
            Arc::clone(&cell_store),
            dir.path().join("workspaces"),
        )),
        agent_profile_store: Arc::new(rua_agent::AgentProfileStore::new(dir.path().to_path_buf())),
        provider_manager: Arc::new(rua_provider::manager::ProviderManager::new()),
        plugin_registry: Arc::new(rua_plugin_runtime::PluginRegistry::new()),
        active_plugin_registry: Arc::new(rua_plugin_runtime::ActivePluginRegistry::new()),
        security: Arc::new(rua_security::SecurityModule::new(
            Arc::new(rua_security::SecurityConfigStore::new(
                dir.path().to_path_buf(),
            )),
            None,
        )),
        task_runtime: Arc::new(
            rua_task::TaskRuntime::open(rua_task::TaskRuntimeConfig::new(dir.path())).unwrap(),
        ),
        collaboration_runtime: Arc::new(()),
        sync_transfer_runtime: Arc::new(()),
        tool_config_store: Arc::new(rua_tool::ToolConfigStore::new(Arc::new(
            rua_tool::AgentToolsCells::new(cell_store, dir.path().join("workspaces")),
        ))),
        tool_before_boundary: None,
        tool_observer_boundary: None,
        permission_request_boundary: None,
        rua_root: dir.path().to_path_buf(),
        secret_dir: secret.clone(),
        cache_dir: cache.clone(),
        compactor: Arc::new(rua_agent::compaction::ConversationCompactor::new(
            Default::default(),
        )),
        usage_recorder: None,
        push_store_dir: Some(dir.path().to_path_buf()),
        agent_resolver: Arc::new(AgentResolver::new("default")),
        command_runtime_factory: std::sync::OnceLock::new(),
        session_replication_hook: std::sync::OnceLock::new(),
        memory_publication_hook: Arc::new(std::sync::OnceLock::new()),
    };

    assert_eq!(ctx.rua_root, dir.path());
    assert_eq!(ctx.secret_dir, secret);
    assert_eq!(ctx.cache_dir, cache);
    assert!(ctx.push_store_dir.is_some());
}

#[test]
fn workspace_context_arc_clone() {
    let ctx = test_helpers::test_context(tempfile::tempdir().unwrap().path().to_path_buf());
    let ctx2 = Arc::clone(&ctx);
    assert!(Arc::ptr_eq(&ctx, &ctx2));
}

#[tokio::test]
async fn workspace_context_agent_resolver_works() {
    let ctx = test_helpers::test_context(tempfile::tempdir().unwrap().path().to_path_buf());
    let active = ctx.agent_resolver.active_agent_id().await;
    assert_eq!(active, "default");
}
