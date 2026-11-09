use super::test_helpers;

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use mcp_manager::error::McpError;
use mcp_manager::manager::McpManager;
use mcp_manager::status::McpServerStatus;
use mcp_manager::types::{
    McpCallContext, McpCallResult, McpConfig, McpReloadReport, McpServerConfig, McpServerId,
    McpShutdownReport, McpToolInfo, ToolFilter,
};
use rua_agent::memory_manager::{
    DreamMemoryRequest, DreamMemoryResult, LoadContextRequest, MemoryContext, MemoryManager,
    MemorySearchHit, MemorySearchRequest, MemoryStatus,
};
use rua_agent::AgentError;
use rua_channel::core::ChannelManager;
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::{Result, RuaError};
use rua_skills::SkillService;
use rua_workspace::service_manager::ImPlatformConfigs;
use rua_workspace::services::SkillsServiceAdapter;
use rua_workspace::{ComponentState, PlatformProfile, RuaRoot, WorkspaceRuntime};
use tempfile::TempDir;

#[derive(Default)]
struct StubAgentRuntime;

#[async_trait]
impl AgentRuntime for StubAgentRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>
    {
        Err(RuaError::NotImplemented {
            feature: "stub stream".into(),
        })
    }

    async fn cancel(&self, _session_id: &SessionId) -> Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct StubMcpManager;

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
    ) -> std::result::Result<McpReloadReport, McpError> {
        Ok(McpReloadReport::default())
    }

    async fn list_servers(&self) -> std::result::Result<Vec<McpServerStatus>, McpError> {
        Ok(Vec::new())
    }

    async fn list_tools(
        &self,
        _filter: ToolFilter,
    ) -> std::result::Result<Vec<McpToolInfo>, McpError> {
        Ok(Vec::new())
    }

    async fn call_tool(
        &self,
        _server: &McpServerId,
        _tool: &str,
        _args: serde_json::Value,
        _ctx: McpCallContext,
    ) -> std::result::Result<McpCallResult, McpError> {
        Ok(McpCallResult {
            structured_content: None,
            content: Vec::new(),
            is_error: false,
        })
    }

    async fn reload(
        &self,
        _new_config: McpConfig,
    ) -> std::result::Result<McpReloadReport, McpError> {
        Ok(McpReloadReport::default())
    }

    async fn invalidate_tool_cache(
        &self,
        _server: &McpServerId,
    ) -> std::result::Result<(), McpError> {
        Ok(())
    }

    async fn connect_server(&self, _server: &McpServerId) -> std::result::Result<(), McpError> {
        Ok(())
    }

    async fn disconnect_server(&self, _server: &McpServerId) -> std::result::Result<(), McpError> {
        Ok(())
    }

    async fn replace_client(
        &self,
        _server_id: &McpServerId,
        _config: McpServerConfig,
    ) -> std::result::Result<(), McpError> {
        Ok(())
    }

    async fn shutdown_all(&self) -> std::result::Result<McpShutdownReport, McpError> {
        Ok(McpShutdownReport::default())
    }
}

#[derive(Default)]
struct StubMemoryManager;

#[async_trait]
impl MemoryManager for StubMemoryManager {
    async fn dream_memory(
        &self,
        _req: DreamMemoryRequest,
    ) -> std::result::Result<DreamMemoryResult, AgentError> {
        Ok(DreamMemoryResult {
            items_consolidated: 0,
            new_insights: vec![],
        })
    }

    async fn memory_search(
        &self,
        _req: MemorySearchRequest,
    ) -> std::result::Result<Vec<MemorySearchHit>, AgentError> {
        Ok(Vec::new())
    }

    async fn load_context(
        &self,
        _req: LoadContextRequest,
    ) -> std::result::Result<MemoryContext, AgentError> {
        Ok(MemoryContext {
            recent_items: Vec::new(),
        })
    }

    async fn delete_memory(&self, _memory_id: &str) -> std::result::Result<bool, AgentError> {
        Ok(false)
    }

    async fn health_check(&self) -> std::result::Result<MemoryStatus, AgentError> {
        Ok(MemoryStatus {
            healthy: true,
            document_count: 0,
            index_size_bytes: 0,
            issues: Vec::new(),
        })
    }

    async fn reindex(&self, _workspace_dir: &Path) -> std::result::Result<usize, AgentError> {
        Ok(0)
    }
}

fn make_runtime(tmp: &TempDir, profile: PlatformProfile) -> WorkspaceRuntime {
    let working = tmp.path().join("work");
    let secret = tmp.path().join("secret");
    std::fs::create_dir_all(&working).unwrap();
    std::fs::create_dir_all(&secret).unwrap();
    let ctx = test_helpers::test_context(working.clone());
    let root = RuaRoot::from_paths(working, secret);
    let workspace = test_helpers::agent_workspace(&root, "agent_a");
    WorkspaceRuntime::new("agent_a", "Agent A", workspace, profile, ctx)
}

/// Registers the real per-Agent lifecycle implementations backed by stubs.
async fn wire_agent_and_runtime(rt: &WorkspaceRuntime) {
    let sm = rt.service_manager();
    sm.register_agent_service(
        rua_core::id::AgentId::new("agent_a"),
        Arc::new(StubAgentRuntime),
    )
    .await;

    let mcp: Arc<dyn McpManager> = Arc::new(StubMcpManager);
    let memory: Arc<dyn MemoryManager> = Arc::new(StubMemoryManager);
    sm.register_runtime_services(mcp, None, memory).await;
}

fn make_skill_service() -> Arc<SkillService> {
    let dir = tempfile::tempdir().unwrap();
    let workspace_dir = dir.path().join("ws");
    std::fs::create_dir_all(&workspace_dir).unwrap();
    let svc = SkillService::new(&workspace_dir).unwrap();

    std::mem::forget(dir);
    Arc::new(svc)
}

async fn assert_state(rt: &WorkspaceRuntime, name: &str, expected: ComponentState) {
    let st = rt.service_manager().service_state(name).await;
    assert!(
        matches!(&st, Some(s) if std::mem::discriminant(s) == std::mem::discriminant(&expected)),
        "expected {name} to be {expected:?}, got {st:?}"
    );
}

async fn initialize_all_phases(rt: &WorkspaceRuntime) {
    rt.initialize_core().await.unwrap();
    rt.initialize_metadata().await.unwrap();
    rt.initialize_runtime().await.unwrap();
}

fn isolated_skills_adapter(
    service: Arc<SkillService>,
    pool_dir: PathBuf,
) -> Arc<dyn rua_workspace::ServiceLifecycle> {
    Arc::new(SkillsServiceAdapter::new(service).with_pool_dir(pool_dir))
}

use std::path::PathBuf;

#[tokio::test]
async fn desktop_profile_services_ready_after_register_and_initialize() {
    let tmp = TempDir::new().unwrap();
    let pool_tmp = TempDir::new().unwrap();
    let rt = make_runtime(&tmp, PlatformProfile::Desktop);
    wire_agent_and_runtime(&rt).await;

    let skills = make_skill_service();
    let channel_manager = Arc::new(ChannelManager::new());

    let sm = rt.service_manager();
    sm.register_platform_services(
        PlatformProfile::Desktop,
        Arc::clone(&skills),
        Arc::clone(&channel_manager),
        ImPlatformConfigs::default(),
        None,
    )
    .await;

    sm.replace_lifecycle(
        "skills",
        isolated_skills_adapter(Arc::clone(&skills), pool_tmp.path().to_path_buf()),
    )
    .await;

    initialize_all_phases(&rt).await;

    for name in ["agent", "skills", "mcp", "channels", "memory"] {
        assert_state(&rt, name, ComponentState::Ready).await;
    }

    let states = sm.component_states().await;
    // Only capabilities with a real start/stop lifecycle are accounted for.
    for absent in ["provider", "tools", "cron", "session_store"] {
        assert!(
            !states.contains_key(absent),
            "{absent} has no real lifecycle and must not be accounted for: {states:?}"
        );
    }
    let ready = states
        .values()
        .filter(|s| matches!(s, ComponentState::Ready))
        .count();
    assert_eq!(
        ready, 5,
        "expected 5 Ready services on Desktop, got {ready} (states: {states:?})"
    );
}

#[tokio::test]
async fn mobile_profile_disables_three_platform_slots_without_panic() {
    let tmp = TempDir::new().unwrap();
    let rt = make_runtime(&tmp, PlatformProfile::Mobile);

    initialize_all_phases(&rt).await;

    for name in ["mcp", "skills", "channels"] {
        assert_state(&rt, name, ComponentState::Disabled).await;
    }

    for name in ["agent", "memory"] {
        assert_state(&rt, name, ComponentState::Ready).await;
    }

    let states = rt.service_manager().component_states().await;
    let ready = states
        .values()
        .filter(|s| matches!(s, ComponentState::Ready))
        .count();
    let disabled = states
        .values()
        .filter(|s| matches!(s, ComponentState::Disabled))
        .count();
    assert_eq!(
        ready, 2,
        "expected 2 Ready on Mobile, got {ready} (states: {states:?})"
    );
    assert_eq!(
        disabled, 3,
        "expected 3 Disabled on Mobile, got {disabled} (states: {states:?})"
    );
}

#[tokio::test]
async fn mobile_register_platform_is_noop() {
    let tmp = TempDir::new().unwrap();
    let rt = make_runtime(&tmp, PlatformProfile::Mobile);

    let skills = make_skill_service();
    let channel_manager = Arc::new(ChannelManager::new());

    let sm = rt.service_manager();
    sm.register_platform_services(
        PlatformProfile::Mobile,
        skills,
        channel_manager,
        ImPlatformConfigs::default(),
        None,
    )
    .await;

    for name in ["skills", "channels"] {
        let st = sm.service_state(name).await;
        assert!(
            matches!(st, Some(ComponentState::Disabled)),
            "expected {name} to remain Disabled on Mobile, got {st:?}"
        );
    }

    let cron_state = sm.service_state("cron").await;
    assert!(
        cron_state.is_none(),
        "cron is owned by the process composition root and must not be a per-Agent service: {cron_state:?}"
    );
}

#[cfg(all(feature = "imessage", target_os = "macos"))]
#[tokio::test]
async fn imessage_config_is_composed_into_workspace_channel_manager() {
    use rua_workspace::service_manager::ServiceLifecycle;
    use rua_workspace::services::ChannelsServiceAdapter;

    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("chat.db");
    // No Process is injected: test registration without touching Messages or
    // requiring the host's imsg executable. Poller lifecycle is covered in rua-channel.
    let manager = Arc::new(ChannelManager::new());
    let adapter = ChannelsServiceAdapter::new(manager.clone()).with_imessage_config(
        rua_config::IMessageConfig {
            enabled: true,
            db_path: db.to_string_lossy().into_owned(),
            poll_sec: Some(60.0),
            ..Default::default()
        },
    );
    assert!(adapter
        .start()
        .await
        .unwrap_err()
        .to_string()
        .contains("Process"));
    assert!(manager
        .channel_ids()
        .await
        .iter()
        .any(|id| id == "imessage"));
    adapter.stop().await.unwrap();
}
