use super::test_helpers;

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::event::RuntimeEvent;
use rua_core::id::{AgentId, SessionId};
use rua_core::{Result, RuaError};
use rua_workspace::{ComponentState, PlatformProfile, RuaRoot, WorkspaceRuntime};
use tempfile::TempDir;

#[derive(Default)]
struct StubAgentRuntime {
    warmup_count: AtomicUsize,
    shutdown_count: AtomicUsize,
    fail_warmup: AtomicBool,
}

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
        self.warmup_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_warmup.load(Ordering::SeqCst) {
            return Err(RuaError::Runtime {
                message: "stub warmup failed".into(),
            });
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        self.shutdown_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
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

fn wire(rt: &WorkspaceRuntime, runtime: Arc<StubAgentRuntime>) -> Arc<StubAgentRuntime> {
    let runtime_dyn: Arc<dyn AgentRuntime> = runtime.clone();
    let sm = rt.service_manager();
    futures::executor::block_on(sm.register_agent_service(AgentId::new("agent_a"), runtime_dyn));
    runtime
}

#[tokio::test]
async fn core_services_ready_after_initialize() {
    let tmp = TempDir::new().unwrap();
    let rt = make_runtime(&tmp);
    let stub = wire(&rt, Arc::new(StubAgentRuntime::default()));

    rt.initialize_for_platform()
        .await
        .expect("initialize_for_platform should succeed when all services start");

    let sm = rt.service_manager();
    assert!(
        matches!(sm.service_state("agent").await, Some(ComponentState::Ready)),
        "agent should be Ready"
    );
    // The provider is process-shared: it has no per-Agent lifecycle and must
    // not be accounted for as an Agent service at all.
    assert!(
        sm.service_state("provider").await.is_none(),
        "provider is process-owned and must not appear as an Agent service"
    );
    assert_eq!(
        stub.warmup_count.load(Ordering::SeqCst),
        1,
        "agent runtime warmup should be called exactly once"
    );
    assert_eq!(
        stub.shutdown_count.load(Ordering::SeqCst),
        0,
        "agent runtime shutdown should not be called during init"
    );
}

#[tokio::test]
async fn best_effort_when_agent_fails_to_start() {
    let tmp = TempDir::new().unwrap();
    let rt = make_runtime(&tmp);
    let stub = Arc::new(StubAgentRuntime::default());
    stub.fail_warmup.store(true, Ordering::SeqCst);
    let _stub = wire(&rt, stub.clone());

    let _ = rt.initialize_for_platform().await;

    let sm = rt.service_manager();
    let agent_state = sm
        .service_state("agent")
        .await
        .expect("agent service should exist");
    assert!(
        matches!(agent_state, ComponentState::Failed { .. }),
        "agent should be Failed after warmup error, got {agent_state:?}"
    );
    // Independent resources are unaffected by the agent failure; the shared
    // provider is untouched because it is not an Agent-level resource.
    assert!(sm.service_state("provider").await.is_none());
    assert!(
        sm.failed_count().await >= 1,
        "manager should report at least one failed service"
    );
}
