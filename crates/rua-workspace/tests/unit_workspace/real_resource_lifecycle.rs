//! Real-resource lifecycle accounting.
//!
//! ADR-0001 §2.1: the component table records capabilities that have a real
//! initialization, stop, or cleanup behaviour. Nothing may be reported `Ready`
//! without a lifecycle that actually started, and a capability that the
//! runtime never assembled must not be accounted for at all.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rua_workspace::service_manager::ServiceLifecycle;
use rua_workspace::{ComponentState, PlatformProfile, RuaRoot, WorkspaceRuntime};

use crate::test_helpers::{agent_workspace, test_context};

#[derive(Default)]
struct CountingLifecycle {
    start_count: AtomicUsize,
    stop_count: AtomicUsize,
    fail_start: AtomicBool,
    fail_stop: AtomicBool,
}

#[async_trait]
impl ServiceLifecycle for CountingLifecycle {
    async fn start(&self) -> Result<(), rua_workspace::WorkspaceError> {
        self.start_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_start.load(Ordering::SeqCst) {
            return Err(rua_workspace::WorkspaceError::component_init_failed(
                "counting",
                "stub start failed",
            ));
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), rua_workspace::WorkspaceError> {
        self.stop_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_stop.load(Ordering::SeqCst) {
            return Err(rua_workspace::WorkspaceError::component_shutdown_failed(
                "counting",
                "stub stop failed",
            ));
        }
        Ok(())
    }
}

fn make_runtime(tmp: &tempfile::TempDir, profile: PlatformProfile) -> WorkspaceRuntime {
    let working = tmp.path().join("work");
    let secret = tmp.path().join("secret");
    std::fs::create_dir_all(&working).unwrap();
    std::fs::create_dir_all(&secret).unwrap();
    let ctx = test_context(working.clone());
    let root = RuaRoot::from_paths(working, secret);
    let workspace = agent_workspace(&root, "agent_a");
    WorkspaceRuntime::new("agent_a", "Agent A", workspace, profile, ctx)
}

#[tokio::test]
async fn capabilities_without_a_lifecycle_are_not_accounted_for() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = make_runtime(&tmp, PlatformProfile::Desktop);

    // `provider`, `tools`, and `cron` have no per-Agent lifecycle: the provider
    // and the cron scheduler are process-owned, and the toolkit is constructed
    // inside the React runtime. Reporting them would be a placeholder entry.
    for absent in ["provider", "tools", "cron", "session_store"] {
        assert!(
            rt.service_manager().service_state(absent).await.is_none(),
            "{absent} must not be registered as a per-Agent service"
        );
    }

    rt.initialize_for_platform().await.unwrap();

    let states = rt.service_manager().component_states().await;
    assert!(
        !states.is_empty(),
        "the runtime assembles real resources and must account for them"
    );
    assert!(
        !states
            .values()
            .any(|state| matches!(state, ComponentState::Initializing)),
        "no capability may be left Initializing after initialization: {states:?}"
    );
}

#[tokio::test]
async fn unassembled_capability_stays_not_initialized_on_desktop() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = make_runtime(&tmp, PlatformProfile::Desktop);

    // Desktop declares the platform capabilities but does not assemble them:
    // no `register_platform_services` call happened, so they stay
    // `NotInitialized` even after the Runtime phase runs.
    assert!(matches!(
        rt.service_manager().service_state("channels").await,
        Some(ComponentState::NotInitialized)
    ));

    rt.initialize_for_platform().await.unwrap();

    assert!(
        matches!(
            rt.service_manager().service_state("channels").await,
            Some(ComponentState::NotInitialized)
        ),
        "an unassembled capability must not become Ready"
    );
    assert!(
        matches!(
            rt.service_manager().service_state("skills").await,
            Some(ComponentState::NotInitialized)
        ),
        "an unassembled capability must not become Ready"
    );
}

#[tokio::test]
async fn mobile_disables_platform_capabilities() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = make_runtime(&tmp, PlatformProfile::Mobile);

    rt.initialize_for_platform().await.unwrap();

    for name in ["mcp", "skills", "channels"] {
        assert!(
            matches!(
                rt.service_manager().service_state(name).await,
                Some(ComponentState::Disabled)
            ),
            "{name} must be Disabled on the Mobile profile"
        );
    }
    for absent in ["provider", "tools", "cron"] {
        assert!(rt.service_manager().service_state(absent).await.is_none());
    }
}

#[tokio::test]
async fn real_lifecycle_starts_once_and_stops_once() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = make_runtime(&tmp, PlatformProfile::Desktop);
    let counting = Arc::new(CountingLifecycle::default());
    let lifecycle: Arc<dyn ServiceLifecycle> = counting.clone();

    rt.service_manager()
        .replace_lifecycle("memory", Arc::clone(&lifecycle))
        .await;

    rt.initialize_for_platform().await.unwrap();
    assert_eq!(
        counting.start_count.load(Ordering::SeqCst),
        1,
        "the assembled lifecycle must be started exactly once"
    );
    assert!(
        matches!(
            rt.service_manager().service_state("memory").await,
            Some(ComponentState::Ready)
        ),
        "a successful real start must be recorded as Ready"
    );

    let failed = rt.stop().await;
    assert_eq!(
        failed,
        vec!["work_authority"],
        "only the absent work authority is unconfirmed"
    );
    assert_eq!(
        counting.stop_count.load(Ordering::SeqCst),
        1,
        "stop must drive the real lifecycle exactly once"
    );
    assert!(
        matches!(
            rt.service_manager().service_state("memory").await,
            Some(ComponentState::NotInitialized)
        ),
        "a stopped capability is no longer started"
    );
}

#[tokio::test]
async fn failed_start_is_reported_and_other_resources_keep_running() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = make_runtime(&tmp, PlatformProfile::Desktop);
    let failing = Arc::new(CountingLifecycle::default());
    failing.fail_start.store(true, Ordering::SeqCst);
    let failing_lifecycle: Arc<dyn ServiceLifecycle> = failing.clone();

    rt.service_manager()
        .replace_lifecycle("memory", failing_lifecycle)
        .await;

    // A failed capability does not abort the workspace: the other independent
    // resources still start.
    rt.initialize_for_platform().await.unwrap();

    let memory_state = rt
        .service_manager()
        .service_state("memory")
        .await
        .expect("memory service is declared");
    assert!(
        matches!(memory_state, ComponentState::Failed { .. }),
        "a failing real start must be reported as Failed, got {memory_state:?}"
    );
    assert_eq!(failing.start_count.load(Ordering::SeqCst), 1);
    assert!(
        rt.service_manager().failed_count().await >= 1,
        "the manager must report the failed capability"
    );
}

#[tokio::test]
async fn failing_stop_is_reported_instead_of_silently_succeeding() {
    let tmp = tempfile::tempdir().unwrap();
    let rt = make_runtime(&tmp, PlatformProfile::Desktop);
    let counting = Arc::new(CountingLifecycle::default());
    counting.fail_stop.store(true, Ordering::SeqCst);
    let lifecycle: Arc<dyn ServiceLifecycle> = counting.clone();

    rt.service_manager()
        .replace_lifecycle("memory", lifecycle)
        .await;
    rt.initialize_for_platform().await.unwrap();

    let failed = rt.stop().await;
    assert!(
        failed.iter().any(|name| name == "memory"),
        "a failed real stop must be reported, got {failed:?}"
    );
    let state = rt.service_manager().service_state("memory").await;
    assert!(
        matches!(state, Some(ComponentState::Failed { .. })),
        "a failed stop must not be recorded as success, got {state:?}"
    );
}
