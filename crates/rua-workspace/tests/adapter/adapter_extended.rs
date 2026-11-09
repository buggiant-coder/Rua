use super::test_helpers;

use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::{AgentId, SessionId};
use rua_core::{Result, RuaError};
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
struct StubMcpManager {
    load_config_count: AtomicUsize,
    list_servers_count: AtomicUsize,
    shutdown_count: AtomicUsize,
    fail_load: AtomicBool,
    /// Simulates `Ok(report)` with per-server failures, which is a real partial
    /// failure shape: the call succeeded but servers did not load.
    fail_load_with_report_errors: AtomicBool,
    /// Simulates `Ok(shutdown_report)` where some servers failed to shut down.
    fail_shutdown_with_report_errors: AtomicBool,
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
    ) -> std::result::Result<McpReloadReport, McpError> {
        self.load_config_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_load.load(Ordering::SeqCst) {
            return Err(McpError::ConfigInvalid {
                message: "stub load_config failure".into(),
            });
        }
        if self.fail_load_with_report_errors.load(Ordering::SeqCst) {
            let mut report = McpReloadReport::default();
            report.errors.insert(
                McpServerId::new("broken-server"),
                "stub server error".into(),
            );
            return Ok(report);
        }
        Ok(McpReloadReport::default())
    }

    async fn list_servers(&self) -> std::result::Result<Vec<McpServerStatus>, McpError> {
        self.list_servers_count.fetch_add(1, Ordering::SeqCst);
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
        self.shutdown_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_shutdown_with_report_errors.load(Ordering::SeqCst) {
            let mut report = McpShutdownReport::default();
            report.errors.insert(
                McpServerId::new("stuck-server"),
                "stub shutdown error".into(),
            );
            return Ok(report);
        }
        Ok(McpShutdownReport::default())
    }
}

#[derive(Default)]
struct StubMemoryManager {
    health_check_count: AtomicUsize,
}

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
        self.health_check_count.fetch_add(1, Ordering::SeqCst);
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

fn make_runtime(tmp: &TempDir) -> WorkspaceRuntime {
    let working = tmp.path().join("work");
    let secret = tmp.path().join("secret");
    std::fs::create_dir_all(&working).unwrap();
    std::fs::create_dir_all(&secret).unwrap();
    let ctx = test_helpers::test_context(working.clone());
    let root = RuaRoot::from_paths(working, secret);
    let workspace = test_helpers::agent_workspace(&root, "agent_a");
    WorkspaceRuntime::new(
        "agent_a",
        "Agent A",
        workspace,
        PlatformProfile::Desktop,
        ctx,
    )
}

#[derive(Default)]
struct WireOptions {
    fail_mcp_load: bool,

    with_mcp_initial_config: bool,
}

struct Wired {
    mcp: Arc<StubMcpManager>,
}

async fn wire_all(rt: &WorkspaceRuntime, opts: WireOptions) -> Wired {
    let agent_runtime: Arc<dyn AgentRuntime> = Arc::new(StubAgentRuntime);

    let sm = rt.service_manager();
    sm.register_agent_service(AgentId::new("agent_a"), agent_runtime)
        .await;

    let mcp = Arc::new(StubMcpManager::default());
    if opts.fail_mcp_load {
        mcp.fail_load.store(true, Ordering::SeqCst);
    }
    let memory = Arc::new(StubMemoryManager::default());

    let mcp_dyn: Arc<dyn McpManager> = mcp.clone();
    rt.set_mcp_manager(mcp_dyn.clone()).await;
    let memory_dyn: Arc<dyn MemoryManager> = memory.clone();
    let initial_config = if opts.with_mcp_initial_config {
        Some(McpConfig::default())
    } else {
        None
    };

    sm.register_runtime_services(mcp_dyn, initial_config, memory_dyn)
        .await;

    Wired { mcp }
}

async fn assert_ready(rt: &WorkspaceRuntime, name: &str) {
    let st = rt.service_manager().service_state(name).await;
    assert!(
        matches!(st, Some(ComponentState::Ready)),
        "expected {name} to be Ready, got {st:?}"
    );
}

async fn initialize_all_phases(rt: &WorkspaceRuntime) {
    rt.initialize_core().await.unwrap();
    rt.initialize_metadata().await.unwrap();
    rt.initialize_runtime().await.unwrap();
}

async fn try_initialize_all_phases(rt: &WorkspaceRuntime) {
    let _ = rt.initialize_core().await;
    let _ = rt.initialize_metadata().await;
    let _ = rt.initialize_runtime().await;
}

#[tokio::test]
async fn assembled_services_ready_after_initialize() {
    let tmp = TempDir::new().unwrap();
    let rt = make_runtime(&tmp);
    let _wired = wire_all(
        &rt,
        WireOptions {
            with_mcp_initial_config: true,
            ..Default::default()
        },
    )
    .await;

    initialize_all_phases(&rt).await;

    for name in ["agent", "mcp", "memory"] {
        assert_ready(&rt, name).await;
    }

    // `provider` is process-shared and `tools` is constructed inside the React
    // runtime: neither has a per-Agent lifecycle to account for.
    let sm = rt.service_manager();
    for absent in ["provider", "tools"] {
        assert!(
            sm.service_state(absent).await.is_none(),
            "{absent} must not be accounted for as an Agent service"
        );
    }
}

#[tokio::test]
async fn best_effort_when_mcp_load_fails() {
    let tmp = TempDir::new().unwrap();
    let rt = make_runtime(&tmp);
    let Wired { mcp, .. } = wire_all(
        &rt,
        WireOptions {
            fail_mcp_load: true,
            with_mcp_initial_config: true,
        },
    )
    .await;

    try_initialize_all_phases(&rt).await;

    assert_eq!(
        mcp.load_config_count.load(Ordering::SeqCst),
        1,
        "load_config should still have been attempted"
    );

    let sm = rt.service_manager();
    let mcp_state = sm
        .service_state("mcp")
        .await
        .expect("mcp service should exist");
    assert!(
        matches!(mcp_state, ComponentState::Failed { .. }),
        "mcp should be Failed after load_config error, got {mcp_state:?}"
    );

    for name in ["memory", "agent"] {
        assert_ready(&rt, name).await;
    }
    assert!(
        sm.failed_count().await >= 1,
        "manager should report at least one failed service"
    );
}

#[tokio::test]
async fn mcp_ok_with_report_errors_is_reported_as_failed() {
    let tmp = TempDir::new().unwrap();
    let rt = make_runtime(&tmp);
    let Wired { mcp, .. } = wire_all(
        &rt,
        WireOptions {
            with_mcp_initial_config: true,
            ..Default::default()
        },
    )
    .await;
    // `load_config` returns `Ok`, but the report contains per-server failures.
    mcp.fail_load_with_report_errors
        .store(true, Ordering::SeqCst);

    try_initialize_all_phases(&rt).await;

    let sm = rt.service_manager();
    let mcp_state = sm
        .service_state("mcp")
        .await
        .expect("mcp service should exist");
    assert!(
        matches!(mcp_state, ComponentState::Failed { .. }),
        "Ok(report) with report errors must not be recorded as Ready, got {mcp_state:?}"
    );
    assert!(
        sm.failed_count().await >= 1,
        "the partial MCP failure must be counted"
    );

    // The independent resources still start.
    for name in ["memory", "agent"] {
        assert_ready(&rt, name).await;
    }
}

#[tokio::test]
async fn mcp_shutdown_report_errors_are_reported() {
    let tmp = TempDir::new().unwrap();
    let rt = make_runtime(&tmp);
    let Wired { mcp, .. } = wire_all(
        &rt,
        WireOptions {
            with_mcp_initial_config: true,
            ..Default::default()
        },
    )
    .await;

    initialize_all_phases(&rt).await;
    assert_ready(&rt, "mcp").await;

    // `shutdown_all` returns `Ok`, but some servers did not stop.
    mcp.fail_shutdown_with_report_errors
        .store(true, Ordering::SeqCst);

    let failed = rt.stop().await;
    assert!(
        failed.iter().any(|name| name == "mcp"),
        "a shutdown report with errors must be reported as a failed stop, got {failed:?}"
    );
    let state = rt.service_manager().service_state("mcp").await;
    assert!(
        matches!(state, Some(ComponentState::Failed { .. })),
        "a partial shutdown must not be recorded as a clean stop, got {state:?}"
    );
}
