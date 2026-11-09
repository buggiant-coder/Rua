//! Agent shutdown must stop real resources and report only what it observed.
//!
//! These fixtures use isolated temporary roots. Nothing here touches the service
//! that carries the current session: every registry and runtime is local.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rua_core::ChatAddress;
use rua_workspace::health::ComponentState;
use rua_workspace::registry::DefaultWorkspaceRegistry;
use rua_workspace::{
    AgentWork, AgentWorkAuthority, PlatformProfile, RuaRoot, ServiceLifecycle, ShutdownPolicy,
    WorkKind, WorkUnit, WorkspaceError, WorkspaceRegistry, WorkspaceRuntime,
};
use tempfile::TempDir;

use super::test_helpers;

/// A real owned resource whose start/stop is observable.
#[derive(Default)]
struct CountingLifecycle {
    starts: AtomicUsize,
    stops: AtomicUsize,
    fail_start: AtomicBool,
    fail_stop: AtomicBool,
}

#[async_trait]
impl ServiceLifecycle for CountingLifecycle {
    async fn start(&self) -> Result<(), WorkspaceError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        if self.fail_start.load(Ordering::SeqCst) {
            return Err(WorkspaceError::component_init_failed(
                "memory",
                "injected start failure",
            ));
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), WorkspaceError> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        if self.fail_stop.load(Ordering::SeqCst) {
            return Err(WorkspaceError::component_shutdown_failed(
                "memory",
                "injected stop failure",
            ));
        }
        Ok(())
    }
}

/// An authority that reports whatever work the test declares.
///
/// It models the two facts a shutdown must tell apart: work that is gone *and*
/// confirmed terminal (a real termination) and work that merely disappeared (not
/// a termination at all).
struct StubAuthority {
    turns: AtomicUsize,
    tasks: AtomicUsize,
    cancel_calls: AtomicUsize,
    /// Agent IDs whose work this authority was asked to cancel.
    cancelled_agents: std::sync::Mutex<Vec<String>>,
    /// Work still held after cancellation, used to prove an unconfirmed stop is
    /// reported instead of being relabelled as success.
    surviving_turns: AtomicUsize,
    /// Turns cancelled into a terminal state, so the termination is evidenced.
    terminate_on_cancel: AtomicBool,
    /// Turns that vanish on cancel without any terminal fact.
    vanish_on_cancel: AtomicBool,
    fail: AtomicBool,
}

impl StubAuthority {
    fn new(turns: usize, tasks: usize) -> Self {
        Self {
            turns: AtomicUsize::new(turns),
            tasks: AtomicUsize::new(tasks),
            cancel_calls: AtomicUsize::new(0),
            cancelled_agents: std::sync::Mutex::new(Vec::new()),
            surviving_turns: AtomicUsize::new(0),
            terminate_on_cancel: AtomicBool::new(false),
            vanish_on_cancel: AtomicBool::new(false),
            fail: AtomicBool::new(false),
        }
    }

    fn units(turns: usize, tasks: usize) -> Vec<WorkUnit> {
        let chat = ChatAddress::agent("agent-under-test", "chat-1");
        let mut units: Vec<WorkUnit> = (0..turns)
            .map(|index| WorkUnit {
                attempt_no: None,
                kind: WorkKind::Turn,
                chat: chat.clone(),
                id: format!("turn-{index}"),
            })
            .collect();
        units.extend((0..tasks).map(|index| WorkUnit {
            attempt_no: Some(1),
            kind: WorkKind::Task,
            chat: chat.clone(),
            id: format!("task-{index}"),
        }));
        units
    }
}

#[async_trait]
impl AgentWorkAuthority for StubAuthority {
    async fn resume(&self, _agent_id: &str) -> Result<(), WorkspaceError> {
        Ok(())
    }
    async fn begin_shutdown(&self, _agent_id: &str) -> Result<(), WorkspaceError> {
        Ok(())
    }

    async fn outstanding_work(&self, _agent_id: &str) -> Result<AgentWork, WorkspaceError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(WorkspaceError::internal("authority unavailable"));
        }
        Ok(AgentWork {
            units: Self::units(
                self.turns.load(Ordering::SeqCst),
                self.tasks.load(Ordering::SeqCst),
            ),
            queued_inputs: 0,
        })
    }

    async fn cancel_work(&self, agent_id: &str) -> Result<AgentWork, WorkspaceError> {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut agents) = self.cancelled_agents.lock() {
            agents.push(agent_id.to_string());
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(WorkspaceError::internal("authority unavailable"));
        }
        if self.terminate_on_cancel.load(Ordering::SeqCst)
            || self.vanish_on_cancel.load(Ordering::SeqCst)
        {
            self.turns.store(0, Ordering::SeqCst);
            self.tasks.store(0, Ordering::SeqCst);
        } else {
            self.turns.store(
                self.surviving_turns.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
            self.tasks.store(0, Ordering::SeqCst);
        }
        Ok(AgentWork {
            units: Self::units(
                self.turns.load(Ordering::SeqCst),
                self.tasks.load(Ordering::SeqCst),
            ),
            queued_inputs: 0,
        })
    }

    async fn terminated_units(&self, units: &[WorkUnit]) -> Result<Vec<WorkUnit>, WorkspaceError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(WorkspaceError::internal("authority unavailable"));
        }
        // Only a cancel that recorded a terminal fact may confirm termination.
        if self.terminate_on_cancel.load(Ordering::SeqCst) {
            Ok(units.to_vec())
        } else {
            Ok(Vec::new())
        }
    }
}

fn build_runtime(agent_id: &str) -> (TempDir, WorkspaceRuntime) {
    let tmp = TempDir::new().expect("temp dir");
    let root = RuaRoot::new(tmp.path());
    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let workspace = test_helpers::agent_workspace(&root, agent_id);
    let runtime =
        WorkspaceRuntime::new(agent_id, agent_id, workspace, PlatformProfile::Desktop, ctx);
    (tmp, runtime)
}

fn build_registry(tmp: &TempDir) -> DefaultWorkspaceRegistry {
    let root = RuaRoot::new(tmp.path());
    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    DefaultWorkspaceRegistry::new(root, PlatformProfile::Desktop, "agent_a", ctx)
}

fn seed_agent(root: &RuaRoot, id: &str) {
    let dir = test_helpers::agent_workspace(root, id)
        .as_path()
        .to_path_buf();
    std::fs::create_dir_all(&dir).expect("workspace dir");
    std::fs::write(
        dir.join("agent.json"),
        format!(r#"{{"id":"{id}","name":"{id}"}}"#),
    )
    .expect("agent json");
}

#[tokio::test]
async fn graceful_shutdown_reports_drain_and_stops_real_resources() {
    let (_tmp, rt) = build_runtime("agent-drain");
    let counting = Arc::new(CountingLifecycle::default());
    let lifecycle: Arc<dyn ServiceLifecycle> = counting.clone();
    rt.service_manager()
        .replace_lifecycle("memory", lifecycle)
        .await;
    rt.initialize_for_platform().await.expect("init");
    assert_eq!(counting.starts.load(Ordering::SeqCst), 1);

    // No work, so a Graceful policy has nothing to cancel.
    let authority = Arc::new(StubAuthority::new(0, 0));
    rt.set_work_authority(authority.clone());

    let report = rt
        .shutdown(ShutdownPolicy::Graceful {
            timeout: Duration::from_secs(5),
        })
        .await;

    assert_eq!(
        counting.stops.load(Ordering::SeqCst),
        1,
        "shutdown must drive the real stop path"
    );
    assert_eq!(
        authority.cancel_calls.load(Ordering::SeqCst),
        0,
        "a drain with nothing outstanding must not cancel work"
    );
    assert!(
        report.authority_confirmed,
        "authority confirmed no work left"
    );
    assert!(
        report.failed_components.is_empty(),
        "no component failed: {:?}",
        report.failed_components
    );
    assert!(
        report.finished_within_timeout,
        "a clean drain is within its bound"
    );
    assert!(
        report.is_clean(),
        "a fully observed shutdown is clean: {report:?}"
    );
}

#[tokio::test]
async fn graceful_shutdown_that_must_cancel_reports_cancelled_not_drained() {
    let (_tmp, rt) = build_runtime("agent-cancel");
    let authority = Arc::new(StubAuthority::new(1, 0));
    authority.terminate_on_cancel.store(true, Ordering::SeqCst);
    rt.set_work_authority(authority.clone());

    let report = rt
        .shutdown(ShutdownPolicy::Graceful {
            timeout: Duration::from_millis(120),
        })
        .await;

    assert_eq!(
        report.turns_drained, 0,
        "a Turn that had to be cancelled is not a drained Turn"
    );
    assert_eq!(
        report.turns_cancelled, 1,
        "the confirmed terminal Turn must be reported as cancelled, got {report:?}"
    );
    assert_eq!(
        report.turns_unresolved, 0,
        "a confirmed termination leaves nothing unresolved"
    );
    assert!(
        authority.cancel_calls.load(Ordering::SeqCst) >= 1,
        "an expired drain must escalate to cancellation"
    );
}

#[tokio::test]
async fn immediate_policy_terminates_without_claiming_a_graceful_completion() {
    let (_tmp, rt) = build_runtime("agent-immediate");
    let authority = Arc::new(StubAuthority::new(1, 0));
    authority.terminate_on_cancel.store(true, Ordering::SeqCst);
    rt.set_work_authority(authority.clone());

    let report = rt.shutdown(ShutdownPolicy::Immediate).await;

    assert_eq!(
        report.turns_drained, 0,
        "an immediate stop never drains a Turn"
    );
    assert!(
        authority.cancel_calls.load(Ordering::SeqCst) >= 1,
        "an immediate stop must cancel rather than drain"
    );
    assert!(
        report.authority_confirmed,
        "the owner did confirm the cancelled Turn ended, got {report:?}"
    );
}

#[tokio::test]
async fn missing_work_authority_withholds_confirmation_instead_of_faking_success() {
    let (_tmp, rt) = build_runtime("agent-no-authority");

    let report = rt
        .shutdown(ShutdownPolicy::Graceful {
            timeout: Duration::from_secs(1),
        })
        .await;

    assert!(
        !report.authority_confirmed,
        "without an authority the termination of work is unconfirmed"
    );
    assert!(
        report
            .failed_components
            .iter()
            .any(|c| c == "work_authority"),
        "the unconfirmed owner must be reported, got {:?}",
        report.failed_components
    );
    assert!(
        !report.is_clean(),
        "an unconfirmed shutdown is never reported as clean"
    );
}

#[tokio::test]
async fn work_that_survives_cancellation_is_reported_as_unresolved() {
    let (_tmp, rt) = build_runtime("agent-stuck-work");
    let authority = Arc::new(StubAuthority::new(1, 0));
    authority.surviving_turns.store(1, Ordering::SeqCst);
    rt.set_work_authority(authority);

    let report = rt
        .shutdown(ShutdownPolicy::Graceful {
            timeout: Duration::from_millis(200),
        })
        .await;

    assert!(
        !report.authority_confirmed,
        "a Turn that ignored cancellation must not be confirmed as ended"
    );
    assert_eq!(
        report.turns_unresolved, 1,
        "the surviving Turn must stay visible as unresolved, got {report:?}"
    );
    assert_eq!(
        report.turns_cancelled, 0,
        "a cancelled request is not a termination"
    );
}

#[tokio::test]
async fn work_that_vanishes_without_a_terminal_fact_is_not_counted_as_terminated() {
    let (_tmp, rt) = build_runtime("agent-vanished-work");
    let authority = Arc::new(StubAuthority::new(1, 0));
    authority.vanish_on_cancel.store(true, Ordering::SeqCst);
    rt.set_work_authority(authority);

    let report = rt
        .shutdown(ShutdownPolicy::Graceful {
            timeout: Duration::from_millis(120),
        })
        .await;

    assert_eq!(
        report.turns_cancelled, 0,
        "a Turn that disappeared without a terminal fact is not a termination"
    );
    assert_eq!(
        report.turns_unresolved, 1,
        "the unconfirmed disappearance must stay unresolved, got {report:?}"
    );
}

#[tokio::test]
async fn authority_failure_is_reported_and_cleanup_still_runs() {
    let (_tmp, rt) = build_runtime("agent-authority-error");
    let authority = Arc::new(StubAuthority::new(0, 0));
    authority.fail.store(true, Ordering::SeqCst);
    rt.set_work_authority(authority);

    let counting = Arc::new(CountingLifecycle::default());
    let lifecycle: Arc<dyn ServiceLifecycle> = counting.clone();
    rt.service_manager()
        .replace_lifecycle("memory", lifecycle)
        .await;
    rt.initialize_for_platform().await.expect("init");

    let report = rt.shutdown(ShutdownPolicy::default()).await;

    assert!(
        report
            .failed_components
            .iter()
            .any(|name| name == "work_authority"),
        "the authority failure must be reported, got {:?}",
        report.failed_components
    );
    assert_eq!(
        counting.stops.load(Ordering::SeqCst),
        1,
        "a failed owner must not prevent owned resources from stopping"
    );
}

#[tokio::test]
async fn one_failing_component_does_not_block_other_stops() {
    let (_tmp, rt) = build_runtime("agent-partial-stop");
    let failing = Arc::new(CountingLifecycle::default());
    failing.fail_stop.store(true, Ordering::SeqCst);
    let failing_lifecycle: Arc<dyn ServiceLifecycle> = failing.clone();
    rt.service_manager()
        .replace_lifecycle("memory", failing_lifecycle)
        .await;

    let healthy = Arc::new(CountingLifecycle::default());
    let healthy_lifecycle: Arc<dyn ServiceLifecycle> = healthy.clone();
    rt.service_manager()
        .replace_lifecycle("skills", healthy_lifecycle)
        .await;

    let authority = Arc::new(StubAuthority::new(0, 0));
    rt.set_work_authority(authority);
    rt.initialize_for_platform().await.expect("init");

    let report = rt.shutdown(ShutdownPolicy::default()).await;

    assert!(
        report.failed_components.iter().any(|name| name == "memory"),
        "the failing component must be reported, got {:?}",
        report.failed_components
    );
    assert_eq!(
        healthy.stops.load(Ordering::SeqCst),
        1,
        "the healthy component must still be stopped"
    );
    assert!(
        !report.is_clean(),
        "a partial stop is never reported as a clean shutdown"
    );
}

#[tokio::test]
async fn maintenance_task_is_cancelled_and_confirmed_exited() {
    let (_tmp, rt) = build_runtime("agent-maintenance");
    rt.spawn_maintenance_task(Duration::from_millis(30));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let authority = Arc::new(StubAuthority::new(0, 0));
    rt.set_work_authority(authority);

    let report = rt.shutdown(ShutdownPolicy::default()).await;
    assert!(
        !report.failed_components.iter().any(|c| c == "maintenance"),
        "maintenance must be confirmed stopped, got {:?}",
        report.failed_components
    );

    let ticks_at_stop = rt.maintenance_tick_count();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        rt.maintenance_tick_count(),
        ticks_at_stop,
        "maintenance kept running after shutdown"
    );
}

#[tokio::test]
async fn repeated_shutdown_is_idempotent() {
    let (_tmp, rt) = build_runtime("agent-idempotent");
    let counting = Arc::new(CountingLifecycle::default());
    let lifecycle: Arc<dyn ServiceLifecycle> = counting.clone();
    rt.service_manager()
        .replace_lifecycle("memory", lifecycle)
        .await;
    let authority = Arc::new(StubAuthority::new(0, 0));
    rt.set_work_authority(authority);
    rt.initialize_for_platform().await.expect("init");

    let first = rt.shutdown(ShutdownPolicy::default()).await;
    let second = rt.shutdown(ShutdownPolicy::default()).await;

    assert_eq!(
        counting.stops.load(Ordering::SeqCst),
        1,
        "a stopped resource is not stopped twice"
    );
    assert!(
        second.failed_components.is_empty(),
        "a repeat shutdown reports no new failures, got {:?}",
        second.failed_components
    );
    assert!(
        first.authority_confirmed == second.authority_confirmed,
        "the authority outcome is stable across repeats"
    );
}

#[tokio::test]
async fn partially_initialized_agent_is_still_cleaned_up() {
    let (_tmp, rt) = build_runtime("agent-partial-init");
    let counting = Arc::new(CountingLifecycle::default());
    // A capability that fails to start may still hold a half-open resource, so
    // it must be stopped even though the Agent never became RuntimeReady.
    counting.fail_start.store(true, Ordering::SeqCst);
    let lifecycle: Arc<dyn ServiceLifecycle> = counting.clone();
    rt.service_manager()
        .replace_lifecycle("memory", lifecycle)
        .await;

    rt.initialize_for_platform().await.expect("init");
    assert_eq!(counting.starts.load(Ordering::SeqCst), 1);
    // The failing capability is reported in its own state: the workspace does not
    // pretend the whole Agent failed, and it does not claim the resource is Ready.
    let memory_state = rt.service_manager().service_state("memory").await;
    assert!(
        matches!(memory_state, Some(ComponentState::Failed { .. })),
        "a failed start must be visible as that capability's state, got {memory_state:?}"
    );

    let authority = Arc::new(StubAuthority::new(0, 0));
    rt.set_work_authority(authority);

    rt.shutdown(ShutdownPolicy::default()).await;
    assert_eq!(
        counting.stops.load(Ordering::SeqCst),
        1,
        "a half-open resource from a partial init must still be stopped"
    );
}

#[tokio::test]
async fn registry_remove_stops_only_the_removed_agent_and_its_work() {
    let tmp = TempDir::new().expect("temp dir");
    let root = RuaRoot::new(tmp.path());
    for id in ["agent_a", "agent_b"] {
        seed_agent(&root, id);
    }

    let registry = build_registry(&tmp);
    registry.register_agent("agent_a", "Agent A").await;
    registry.register_agent("agent_b", "Agent B").await;

    // Real work for agent_a only: a drain with nothing outstanding never calls
    // the owner, so termination has to be provoked to be observable.
    let authority = Arc::new(StubAuthority::new(1, 0));
    authority.terminate_on_cancel.store(true, Ordering::SeqCst);
    registry.set_work_authority(authority.clone()).await;

    let counting_a = Arc::new(CountingLifecycle::default());
    let counting_b = Arc::new(CountingLifecycle::default());

    let rt_a = registry.get_or_create("agent_a").await.expect("agent_a");
    let lifecycle_a: Arc<dyn ServiceLifecycle> = counting_a.clone();
    rt_a.service_manager()
        .replace_lifecycle("memory", lifecycle_a)
        .await;
    rt_a.service_manager()
        .start_phase(rua_workspace::InitPhase::Runtime)
        .await
        .expect("start replacement a");

    let rt_b = registry.get_or_create("agent_b").await.expect("agent_b");
    let lifecycle_b: Arc<dyn ServiceLifecycle> = counting_b.clone();
    rt_b.service_manager()
        .replace_lifecycle("memory", lifecycle_b)
        .await;
    rt_b.service_manager()
        .start_phase(rua_workspace::InitPhase::Runtime)
        .await
        .expect("start replacement b");

    // The registry must have given both runtimes the real work owner.
    assert!(rt_a.has_work_authority());
    assert!(rt_b.has_work_authority());

    registry
        .remove_with_policy(
            "agent_a",
            ShutdownPolicy::CancelActive {
                timeout: Duration::from_secs(1),
            },
        )
        .await
        .expect("remove a");

    assert_eq!(
        counting_a.stops.load(Ordering::SeqCst),
        1,
        "the removed Agent's resources must stop"
    );
    assert_eq!(
        *authority.cancelled_agents.lock().unwrap(),
        vec!["agent_a".to_string()],
        "only the removed Agent's work may be terminated"
    );
    assert_eq!(
        counting_b.stops.load(Ordering::SeqCst),
        0,
        "the other Agent must keep running"
    );
    assert!(
        registry.get("agent_b").await.is_some(),
        "the other Agent stays in the registry"
    );
}

#[tokio::test]
async fn remove_with_policy_reports_the_policy_outcome_for_the_removed_agent() {
    let tmp = TempDir::new().expect("temp dir");
    let root = RuaRoot::new(tmp.path());
    seed_agent(&root, "agent_a");

    let registry = build_registry(&tmp);
    registry.register_agent("agent_a", "Agent A").await;

    let authority = Arc::new(StubAuthority::new(1, 0));
    authority.terminate_on_cancel.store(true, Ordering::SeqCst);
    registry.set_work_authority(authority.clone()).await;

    let rt = registry.get_or_create("agent_a").await.expect("agent_a");
    rt.initialize_for_platform().await.expect("init");

    let report = registry
        .remove_with_policy(
            "agent_a",
            ShutdownPolicy::CancelActive {
                timeout: Duration::from_secs(1),
            },
        )
        .await
        .expect("remove_with_policy");

    assert_eq!(
        report.turns_cancelled, 1,
        "CancelActive must report the confirmed cancelled Turn, got {report:?}"
    );
    assert_eq!(report.turns_drained, 0, "CancelActive never drains");
    assert!(
        report.authority_confirmed,
        "the owner confirmed termination"
    );
    assert!(
        report.is_clean(),
        "a confirmed cancellation of all work is clean: {report:?}"
    );
    assert!(
        registry.get("agent_a").await.is_none(),
        "the removed Agent leaves the index"
    );
}

#[tokio::test]
async fn shutdown_all_applies_the_policy_and_clears_every_agent() {
    let tmp = TempDir::new().expect("temp dir");
    let root = RuaRoot::new(tmp.path());
    for id in ["agent_a", "agent_b"] {
        seed_agent(&root, id);
    }

    let registry = build_registry(&tmp);
    registry.register_agent("agent_a", "Agent A").await;
    registry.register_agent("agent_b", "Agent B").await;

    let authority = Arc::new(StubAuthority::new(0, 0));
    registry.set_work_authority(authority.clone()).await;

    let rt_a = registry.get_or_create("agent_a").await.expect("agent_a");
    let rt_b = registry.get_or_create("agent_b").await.expect("agent_b");
    for rt in [&rt_a, &rt_b] {
        let counting = Arc::new(CountingLifecycle::default());
        let lifecycle: Arc<dyn ServiceLifecycle> = counting.clone();
        rt.service_manager()
            .replace_lifecycle("memory", lifecycle)
            .await;
        rt.initialize_for_platform().await.expect("init");
    }

    let report = registry
        .shutdown_all(ShutdownPolicy::CancelActive {
            timeout: Duration::from_secs(1),
        })
        .await
        .expect("shutdown_all");

    assert_eq!(
        authority.cancel_calls.load(Ordering::SeqCst),
        2,
        "every Agent's work is terminated through the owner"
    );
    assert!(
        report.failed_components.is_empty(),
        "no component failed: {:?}",
        report.failed_components
    );
    assert!(report.authority_confirmed);
    assert!(
        registry.list().await.expect("list").is_empty(),
        "shutdown_all removes every runtime from the index"
    );
}

#[tokio::test]
async fn shutdown_all_keeps_going_after_one_agent_fails_to_stop() {
    let tmp = TempDir::new().expect("temp dir");
    let root = RuaRoot::new(tmp.path());
    for id in ["agent_a", "agent_b"] {
        seed_agent(&root, id);
    }

    let registry = build_registry(&tmp);
    registry.register_agent("agent_a", "Agent A").await;
    registry.register_agent("agent_b", "Agent B").await;

    let authority = Arc::new(StubAuthority::new(0, 0));
    registry.set_work_authority(authority).await;

    let failing = Arc::new(CountingLifecycle::default());
    failing.fail_stop.store(true, Ordering::SeqCst);
    let healthy = Arc::new(CountingLifecycle::default());

    let rt_a = registry.get_or_create("agent_a").await.expect("agent_a");
    let failing_lifecycle: Arc<dyn ServiceLifecycle> = failing.clone();
    rt_a.service_manager()
        .replace_lifecycle("memory", failing_lifecycle)
        .await;
    rt_a.service_manager()
        .start_phase(rua_workspace::InitPhase::Runtime)
        .await
        .expect("start replacement a");

    let rt_b = registry.get_or_create("agent_b").await.expect("agent_b");
    let healthy_lifecycle: Arc<dyn ServiceLifecycle> = healthy.clone();
    rt_b.service_manager()
        .replace_lifecycle("memory", healthy_lifecycle)
        .await;
    rt_b.service_manager()
        .start_phase(rua_workspace::InitPhase::Runtime)
        .await
        .expect("start replacement b");

    let report = registry
        .shutdown_all(ShutdownPolicy::CancelActive {
            timeout: Duration::from_secs(1),
        })
        .await
        .expect("shutdown_all");

    assert!(
        report
            .failed_components
            .iter()
            .any(|component| component == "agent_a:memory"),
        "the failing Agent's component must be attributed, got {:?}",
        report.failed_components
    );
    assert_eq!(
        healthy.stops.load(Ordering::SeqCst),
        1,
        "the healthy Agent must still be stopped"
    );
    assert!(
        !report.is_clean(),
        "one Agent's failure keeps the aggregate from reading as clean"
    );
    assert!(
        registry.get("agent_a").await.is_some(),
        "failed cleanup retains its owner for retry"
    );
    assert!(
        registry.get("agent_b").await.is_none(),
        "successful cleanup removes the healthy Agent"
    );
    failing.fail_stop.store(false, Ordering::SeqCst);
    let retry = registry
        .remove_with_policy(
            "agent_a",
            ShutdownPolicy::CancelActive {
                timeout: Duration::from_secs(1),
            },
        )
        .await
        .expect("retry shutdown");
    assert!(retry.is_clean(), "retry must complete cleanup: {retry:?}");
    assert!(registry.get("agent_a").await.is_none());
}

#[tokio::test]
async fn shutdown_all_uses_one_deadline_for_multiple_agents() {
    let tmp = TempDir::new().unwrap();
    let root = RuaRoot::new(tmp.path());
    for id in ["deadline-a", "deadline-b"] {
        seed_agent(&root, id);
    }
    let registry = build_registry(&tmp);
    let authority = Arc::new(StubAuthority::new(1, 0));
    authority.surviving_turns.store(1, Ordering::SeqCst);
    registry.set_work_authority(authority).await;
    registry.get_or_create("deadline-a").await.unwrap();
    registry.get_or_create("deadline-b").await.unwrap();
    let started = tokio::time::Instant::now();
    let report = tokio::time::timeout(
        Duration::from_millis(500),
        registry.shutdown_all(ShutdownPolicy::Graceful {
            timeout: Duration::from_millis(50),
        }),
    )
    .await
    .expect("aggregate shutdown must have one finite budget")
    .unwrap();
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(!report.is_clean());
    assert!(registry.get("deadline-a").await.is_some());
    assert!(registry.get("deadline-b").await.is_some());
}
