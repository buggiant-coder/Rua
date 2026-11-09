use super::test_helpers;

use std::sync::Arc;

use rua_workspace::{PlatformProfile, RuaRoot, WorkspaceRuntime};
use tempfile::TempDir;

fn runtime(
    agent_id: &str,
    agent_name: &str,
    root: RuaRoot,
    profile: PlatformProfile,
    ctx: Arc<rua_workspace::WorkspaceContext>,
) -> WorkspaceRuntime {
    let workspace = test_helpers::agent_workspace(&root, agent_id);
    WorkspaceRuntime::new(agent_id, agent_name, workspace, profile, ctx)
}

use async_trait::async_trait;
use mcp_manager::manager::McpManager;
use mcp_manager::types::{
    McpCallContext, McpCallResult, McpConfig, McpReloadReport, McpServerConfig, McpServerId,
    McpShutdownReport, McpToolInfo, ToolFilter,
};
use mcp_manager::McpServerStatus;

struct StubMcpManager {
    label: String,
    reload_count: std::sync::atomic::AtomicU32,
}

impl StubMcpManager {
    fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            reload_count: std::sync::atomic::AtomicU32::new(0),
        }
    }

    fn reload_count(&self) -> u32 {
        self.reload_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl McpManager for StubMcpManager {
    async fn suspend_new_calls(&self) -> mcp_manager::error::McpResult<()> {
        Ok(())
    }
    async fn configuration(&self) -> mcp_manager::error::McpResult<McpConfig> {
        Ok(McpConfig::default())
    }

    async fn load_config(
        &self,
        _config: McpConfig,
    ) -> mcp_manager::error::McpResult<McpReloadReport> {
        Ok(McpReloadReport::default())
    }

    async fn list_servers(&self) -> mcp_manager::error::McpResult<Vec<McpServerStatus>> {
        Ok(vec![])
    }

    async fn list_tools(
        &self,
        _filter: ToolFilter,
    ) -> mcp_manager::error::McpResult<Vec<McpToolInfo>> {
        Ok(vec![McpToolInfo {
            server_id: McpServerId::new(self.label.clone()),
            name: format!("tool_from_{}", self.label),
            description: format!("Tool belonging to {}", self.label),
            input_schema: serde_json::Value::Null,
            raw: serde_json::Value::Null,
        }])
    }

    async fn call_tool(
        &self,
        _server: &McpServerId,
        _tool: &str,
        _args: serde_json::Value,
        _ctx: McpCallContext,
    ) -> mcp_manager::error::McpResult<McpCallResult> {
        unimplemented!()
    }

    async fn reload(
        &self,
        _new_config: McpConfig,
    ) -> mcp_manager::error::McpResult<McpReloadReport> {
        self.reload_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(McpReloadReport::default())
    }

    async fn invalidate_tool_cache(
        &self,
        _server: &McpServerId,
    ) -> mcp_manager::error::McpResult<()> {
        Ok(())
    }

    async fn connect_server(&self, _server: &McpServerId) -> mcp_manager::error::McpResult<()> {
        Ok(())
    }

    async fn disconnect_server(&self, _server: &McpServerId) -> mcp_manager::error::McpResult<()> {
        Ok(())
    }

    async fn replace_client(
        &self,
        _server_id: &McpServerId,
        _config: McpServerConfig,
    ) -> mcp_manager::error::McpResult<()> {
        Ok(())
    }

    async fn shutdown_all(&self) -> mcp_manager::error::McpResult<McpShutdownReport> {
        Ok(McpShutdownReport::default())
    }
}

#[tokio::test]
async fn mcp_manager_per_agent_isolation() {
    let tmp = TempDir::new().unwrap();
    let root_a = RuaRoot::from_paths(tmp.path().join("a_data"), tmp.path().join("a_secret"));
    let root_b = RuaRoot::from_paths(tmp.path().join("b_data"), tmp.path().join("b_secret"));

    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let rt_a = runtime(
        "agent_a",
        "Agent A",
        root_a,
        PlatformProfile::Desktop,
        Arc::clone(&ctx),
    );
    let rt_b = runtime("agent_b", "Agent B", root_b, PlatformProfile::Desktop, ctx);

    let mgr_a: Arc<dyn McpManager> = Arc::new(StubMcpManager::new("agent_a"));
    let mgr_b: Arc<dyn McpManager> = Arc::new(StubMcpManager::new("agent_b"));

    rt_a.set_mcp_manager(Arc::clone(&mgr_a)).await;
    rt_b.set_mcp_manager(Arc::clone(&mgr_b)).await;

    let tools_a = rt_a
        .mcp_manager()
        .await
        .unwrap()
        .list_tools(ToolFilter::default())
        .await
        .unwrap();
    assert_eq!(tools_a.len(), 1);
    assert_eq!(tools_a[0].name, "tool_from_agent_a");

    let tools_b = rt_b
        .mcp_manager()
        .await
        .unwrap()
        .list_tools(ToolFilter::default())
        .await
        .unwrap();
    assert_eq!(tools_b.len(), 1);
    assert_eq!(tools_b[0].name, "tool_from_agent_b");

    assert_ne!(tools_a[0].name, tools_b[0].name);
}

#[tokio::test]
async fn memory_manager_workspace_dir_per_agent() {
    let tmp = TempDir::new().unwrap();

    let ws_a = tmp.path().join("workspaces").join("agent_a");
    let ws_b = tmp.path().join("workspaces").join("agent_b");
    std::fs::create_dir_all(&ws_a).unwrap();
    std::fs::create_dir_all(&ws_b).unwrap();

    let root_a = RuaRoot::from_paths(tmp.path().to_path_buf(), tmp.path().join("secret"));
    let root_b = RuaRoot::from_paths(tmp.path().to_path_buf(), tmp.path().join("secret"));

    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let rt_a = runtime(
        "agent_a",
        "Agent A",
        root_a,
        PlatformProfile::Desktop,
        Arc::clone(&ctx),
    );
    let rt_b = runtime("agent_b", "Agent B", root_b, PlatformProfile::Desktop, ctx);

    let dir_a = rt_a.workspace_dir();
    let dir_b = rt_b.workspace_dir();

    assert!(
        dir_a.ends_with("agent_a"),
        "agent_a workspace_dir should end with agent_a, got: {}",
        dir_a.display()
    );
    assert!(
        dir_b.ends_with("agent_b"),
        "agent_b workspace_dir should end with agent_b, got: {}",
        dir_b.display()
    );
    assert_ne!(dir_a, dir_b);
}

#[tokio::test]
async fn mcp_reload_per_agent_isolation() {
    let tmp = TempDir::new().unwrap();
    let root_a = RuaRoot::from_paths(tmp.path().join("a_data"), tmp.path().join("a_secret"));
    let root_b = RuaRoot::from_paths(tmp.path().join("b_data"), tmp.path().join("b_secret"));

    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let rt_a = runtime(
        "agent_a",
        "Agent A",
        root_a,
        PlatformProfile::Desktop,
        Arc::clone(&ctx),
    );
    let rt_b = runtime("agent_b", "Agent B", root_b, PlatformProfile::Desktop, ctx);

    let mgr_a = Arc::new(StubMcpManager::new("agent_a"));
    let mgr_b = Arc::new(StubMcpManager::new("agent_b"));

    rt_a.set_mcp_manager(Arc::clone(&mgr_a) as Arc<dyn McpManager>)
        .await;
    rt_b.set_mcp_manager(Arc::clone(&mgr_b) as Arc<dyn McpManager>)
        .await;

    let mgr_a_ref = rt_a.mcp_manager().await.unwrap();
    mgr_a_ref.reload(McpConfig::default()).await.unwrap();

    assert_eq!(mgr_a.reload_count(), 1);

    assert_eq!(mgr_b.reload_count(), 0);

    mgr_a_ref.reload(McpConfig::default()).await.unwrap();
    assert_eq!(mgr_a.reload_count(), 2);
    assert_eq!(mgr_b.reload_count(), 0);
}

#[tokio::test]
async fn memory_manager_lazy_init_vec_db_per_agent() {
    let tmp = TempDir::new().unwrap();
    let root = RuaRoot::from_paths(tmp.path().to_path_buf(), tmp.path().join("secret"));

    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let rt_a = runtime(
        "agent_a",
        "Agent A",
        root.clone(),
        PlatformProfile::Desktop,
        Arc::clone(&ctx),
    );
    let rt_b = runtime(
        "agent_b",
        "Agent B",
        root.clone(),
        PlatformProfile::Desktop,
        ctx,
    );

    rt_a.initialize_core().await.unwrap();
    rt_a.initialize_metadata().await.unwrap();
    rt_a.initialize_runtime().await.unwrap();

    rt_b.initialize_core().await.unwrap();
    rt_b.initialize_metadata().await.unwrap();
    rt_b.initialize_runtime().await.unwrap();

    assert!(
        rt_a.memory_manager().await.is_some(),
        "agent_a memory_manager not set"
    );
    assert!(
        rt_b.memory_manager().await.is_some(),
        "agent_b memory_manager not set"
    );

    let vec_db_a = rt_a.workspace_dir().join("memory_vec.db");
    let vec_db_b = rt_b.workspace_dir().join("memory_vec.db");

    assert!(vec_db_a.exists(), "agent_a memory_vec.db not created");
    assert!(vec_db_b.exists(), "agent_b memory_vec.db not created");
    assert_ne!(
        vec_db_a, vec_db_b,
        "vec.db paths should differ between agents"
    );
    assert!(vec_db_a.to_string_lossy().contains("agent_a"));
    assert!(vec_db_b.to_string_lossy().contains("agent_b"));
}

#[tokio::test]
async fn memory_manager_per_agent_embedding_config() {
    let tmp = TempDir::new().unwrap();
    let root = RuaRoot::from_paths(tmp.path().to_path_buf(), tmp.path().join("secret"));

    let ws_a = tmp.path().join("workspaces").join("agent_a");
    let ws_b = tmp.path().join("workspaces").join("agent_b");
    std::fs::create_dir_all(&ws_a).unwrap();
    std::fs::create_dir_all(&ws_b).unwrap();

    let agent_json_a = serde_json::json!({
        "id": "agent_a",
        "name": "Agent A",
        "running": {
            "embedding_config": {
                "base_url": "http://embed-a.local:8080",
                "model_name": "model-a",
                "dimensions": 256
            }
        }
    });
    let agent_json_b = serde_json::json!({
        "id": "agent_b",
        "name": "Agent B",
        "running": {
            "embedding_config": {
                "base_url": "http://embed-b.local:9090",
                "model_name": "model-b",
                "dimensions": 512
            }
        }
    });
    std::fs::write(
        ws_a.join("agent.json"),
        serde_json::to_string_pretty(&agent_json_a).unwrap(),
    )
    .unwrap();
    std::fs::write(
        ws_b.join("agent.json"),
        serde_json::to_string_pretty(&agent_json_b).unwrap(),
    )
    .unwrap();

    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let rt_a = runtime(
        "agent_a",
        "Agent A",
        root.clone(),
        PlatformProfile::Desktop,
        Arc::clone(&ctx),
    );
    let rt_b = runtime(
        "agent_b",
        "Agent B",
        root.clone(),
        PlatformProfile::Desktop,
        ctx,
    );

    rt_a.initialize_core().await.unwrap();
    rt_a.initialize_metadata().await.unwrap();
    rt_a.initialize_runtime().await.unwrap();

    rt_b.initialize_core().await.unwrap();
    rt_b.initialize_metadata().await.unwrap();
    rt_b.initialize_runtime().await.unwrap();

    assert!(
        rt_a.memory_manager().await.is_some(),
        "agent_a memory_manager not set"
    );
    assert!(
        rt_b.memory_manager().await.is_some(),
        "agent_b memory_manager not set"
    );

    let cfg_a: rua_config::AgentProfileConfig =
        serde_json::from_slice(&std::fs::read(ws_a.join("agent.json")).unwrap()).unwrap();
    let cfg_b: rua_config::AgentProfileConfig =
        serde_json::from_slice(&std::fs::read(ws_b.join("agent.json")).unwrap()).unwrap();
    assert_ne!(
        cfg_a.running.embedding_config.base_url, cfg_b.running.embedding_config.base_url,
        "embedding base_urls should differ"
    );
    assert_eq!(
        cfg_a.running.embedding_config.base_url,
        "http://embed-a.local:8080"
    );
    assert_eq!(
        cfg_b.running.embedding_config.base_url,
        "http://embed-b.local:9090"
    );

    let vec_db_a = ws_a.join("memory_vec.db");
    let vec_db_b = ws_b.join("memory_vec.db");
    assert!(vec_db_a.exists(), "agent_a vec.db not created");
    assert!(vec_db_b.exists(), "agent_b vec.db not created");
    assert_ne!(vec_db_a, vec_db_b);
}

#[tokio::test]
async fn channel_manager_lazy_init_per_agent_isolation() {
    let tmp = TempDir::new().unwrap();
    let root_a = RuaRoot::from_paths(tmp.path().join("a_data"), tmp.path().join("a_secret"));
    let root_b = RuaRoot::from_paths(tmp.path().join("b_data"), tmp.path().join("b_secret"));

    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let rt_a = runtime(
        "agent_a",
        "Agent A",
        root_a,
        PlatformProfile::Desktop,
        Arc::clone(&ctx),
    );
    let rt_b = runtime("agent_b", "Agent B", root_b, PlatformProfile::Desktop, ctx);

    let cm_a = rt_a
        .channel_manager()
        .await
        .expect("lazy-init should produce Some");
    let cm_b = rt_b
        .channel_manager()
        .await
        .expect("lazy-init should produce Some");

    assert!(
        !Arc::ptr_eq(&cm_a, &cm_b),
        "two agents must have independent ChannelManager instances"
    );

    let cm_a2 = rt_a.channel_manager().await.unwrap();
    assert!(Arc::ptr_eq(&cm_a, &cm_a2));
}

#[tokio::test]
async fn security_module_is_shared_by_workspace_runtimes() {
    let tmp = TempDir::new().unwrap();
    let root_a = RuaRoot::from_paths(tmp.path().join("a_data"), tmp.path().join("a_secret"));
    let root_b = RuaRoot::from_paths(tmp.path().join("b_data"), tmp.path().join("b_secret"));

    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let rt_a = runtime(
        "agent_a",
        "Agent A",
        root_a,
        PlatformProfile::Desktop,
        Arc::clone(&ctx),
    );
    let rt_b = runtime("agent_b", "Agent B", root_b, PlatformProfile::Desktop, ctx);

    let security_a = rt_a.security_module().await.expect("should produce Some");
    let security_b = rt_b.security_module().await.expect("should produce Some");

    assert!(
        Arc::ptr_eq(&security_a, &security_b),
        "workspace runtimes must delegate to the shared SecurityModule instance"
    );

    let security_a2 = rt_a.security_module().await.unwrap();
    assert!(Arc::ptr_eq(&security_a, &security_a2));
}
