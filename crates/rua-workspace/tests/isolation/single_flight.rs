use std::sync::Arc;

use rua_workspace::registry::DefaultWorkspaceRegistry;
use rua_workspace::{PlatformProfile, RuaRoot, WorkspaceRegistry};

use super::test_helpers;

struct ParallelAdmission {
    barrier: tokio::sync::Barrier,
    resumed: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl rua_workspace::AgentWorkAuthority for ParallelAdmission {
    async fn resume(&self, _id: &str) -> Result<(), rua_workspace::WorkspaceError> {
        self.resumed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.barrier.wait().await;
        Ok(())
    }
    async fn begin_shutdown(&self, _id: &str) -> Result<(), rua_workspace::WorkspaceError> {
        Ok(())
    }
    async fn outstanding_work(
        &self,
        _id: &str,
    ) -> Result<rua_workspace::AgentWork, rua_workspace::WorkspaceError> {
        Ok(rua_workspace::AgentWork::default())
    }
    async fn cancel_work(
        &self,
        _id: &str,
    ) -> Result<rua_workspace::AgentWork, rua_workspace::WorkspaceError> {
        Ok(rua_workspace::AgentWork::default())
    }
    async fn terminated_units(
        &self,
        units: &[rua_workspace::WorkUnit],
    ) -> Result<Vec<rua_workspace::WorkUnit>, rua_workspace::WorkspaceError> {
        Ok(units.to_vec())
    }
}

#[tokio::test]
async fn different_agents_initialize_without_a_global_operation_lock() {
    let temp = tempfile::tempdir().unwrap();
    let registry = DefaultWorkspaceRegistry::new(
        RuaRoot::new(temp.path()),
        PlatformProfile::Mobile,
        "left",
        test_helpers::test_context(temp.path().to_path_buf()),
    );
    let authority = Arc::new(ParallelAdmission {
        barrier: tokio::sync::Barrier::new(2),
        resumed: std::sync::atomic::AtomicUsize::new(0),
    });
    registry.set_work_authority(authority.clone()).await;
    let (left, right) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(
            registry.get_or_create("left"),
            registry.get_or_create("right")
        )
    })
    .await
    .expect("both Agents must reach admission concurrently");
    assert!(left.is_ok() && right.is_ok());
    assert_eq!(
        authority.resumed.load(std::sync::atomic::Ordering::SeqCst),
        2
    );
}

struct ControlledStart {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

struct ControlledAdmission {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    fail: std::sync::atomic::AtomicBool,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl rua_workspace::AgentWorkAuthority for ControlledAdmission {
    async fn resume(&self, _id: &str) -> Result<(), rua_workspace::WorkspaceError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(rua_workspace::WorkspaceError::internal(
                "controlled admission failure",
            ));
        }
        Ok(())
    }
    async fn begin_shutdown(&self, _id: &str) -> Result<(), rua_workspace::WorkspaceError> {
        Ok(())
    }
    async fn outstanding_work(
        &self,
        _id: &str,
    ) -> Result<rua_workspace::AgentWork, rua_workspace::WorkspaceError> {
        Ok(rua_workspace::AgentWork::default())
    }
    async fn cancel_work(
        &self,
        _id: &str,
    ) -> Result<rua_workspace::AgentWork, rua_workspace::WorkspaceError> {
        Ok(rua_workspace::AgentWork::default())
    }
    async fn terminated_units(
        &self,
        units: &[rua_workspace::WorkUnit],
    ) -> Result<Vec<rua_workspace::WorkUnit>, rua_workspace::WorkspaceError> {
        Ok(units.to_vec())
    }
}

#[tokio::test]
async fn removal_waits_for_initialization_and_cleans_the_published_instance() {
    let temp = tempfile::tempdir().unwrap();
    let registry = Arc::new(DefaultWorkspaceRegistry::new(
        RuaRoot::new(temp.path()),
        PlatformProfile::Mobile,
        "racing",
        test_helpers::test_context(temp.path().to_path_buf()),
    ));
    let authority = Arc::new(ControlledAdmission {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        fail: std::sync::atomic::AtomicBool::new(false),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    registry.set_work_authority(authority.clone()).await;
    let creating = registry.clone();
    let creation = tokio::spawn(async move { creating.get_or_create("racing").await });
    authority.entered.notified().await;
    let removing = registry.clone();
    let removal = tokio::spawn(async move { removing.remove("racing").await });
    assert!(
        registry.get("racing").await.is_none(),
        "initializing instance is not published"
    );
    authority.release.notify_one();
    let runtime = creation.await.unwrap().unwrap();
    removal.await.unwrap().unwrap();
    assert!(registry.get("racing").await.is_none());
    assert_eq!(
        runtime.lifecycle_state().await,
        rua_workspace::WorkspaceLifecycleState::Stopped
    );
    assert!(
        runtime.initialize_for_platform().await.is_err(),
        "removed instance cannot resurrect"
    );
}

#[tokio::test]
async fn cancelled_waiter_does_not_orphan_or_duplicate_initialization() {
    let temp = tempfile::tempdir().unwrap();
    let registry = Arc::new(DefaultWorkspaceRegistry::new(
        RuaRoot::new(temp.path()),
        PlatformProfile::Mobile,
        "cancelled-waiter",
        test_helpers::test_context(temp.path().to_path_buf()),
    ));
    let authority = Arc::new(ControlledAdmission {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        fail: std::sync::atomic::AtomicBool::new(false),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    registry.set_work_authority(authority.clone()).await;
    let creating = registry.clone();
    let waiter = tokio::spawn(async move { creating.get_or_create("cancelled-waiter").await });
    authority.entered.notified().await;
    waiter.abort();
    assert!(matches!(waiter.await, Err(error) if error.is_cancelled()));
    authority.release.notify_one();
    let runtime = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        registry.get_or_create("cancelled-waiter"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(authority.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    registry.remove("cancelled-waiter").await.unwrap();
    assert_eq!(
        runtime.lifecycle_state().await,
        rua_workspace::WorkspaceLifecycleState::Stopped
    );
}

#[async_trait::async_trait]
impl rua_workspace::ServiceLifecycle for ControlledStart {
    async fn start(&self) -> Result<(), rua_workspace::WorkspaceError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(())
    }
    async fn stop(&self) -> Result<(), rua_workspace::WorkspaceError> {
        Ok(())
    }
}

#[tokio::test]
async fn cancelled_rebuild_waiter_still_publishes_one_clean_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let registry = Arc::new(DefaultWorkspaceRegistry::new(
        RuaRoot::new(temp.path()),
        PlatformProfile::Mobile,
        "rebuild-cancel",
        test_helpers::test_context(temp.path().to_path_buf()),
    ));
    let authority = Arc::new(ControlledAdmission {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        fail: std::sync::atomic::AtomicBool::new(false),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    registry.set_work_authority(authority.clone()).await;
    authority.release.notify_one();
    let original = registry.get_or_create("rebuild-cancel").await.unwrap();
    authority.entered.notified().await;
    let rebuilding = registry.clone();
    let waiter = tokio::spawn(async move { rebuilding.reload_agent("rebuild-cancel").await });
    authority.entered.notified().await;
    waiter.abort();
    assert!(matches!(waiter.await, Err(error) if error.is_cancelled()));
    authority.release.notify_one();
    let replacement = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        registry.get_or_create("rebuild-cancel"),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!Arc::ptr_eq(&original, &replacement));
    assert_eq!(
        original.lifecycle_state().await,
        rua_workspace::WorkspaceLifecycleState::Stopped
    );
    registry.remove("rebuild-cancel").await.unwrap();
}

#[tokio::test]
async fn waiting_requests_share_a_failed_creation_and_later_requests_retry() {
    let temp = tempfile::tempdir().unwrap();
    let registry = Arc::new(DefaultWorkspaceRegistry::new(
        RuaRoot::new(temp.path()),
        PlatformProfile::Mobile,
        "retry",
        test_helpers::test_context(temp.path().to_path_buf()),
    ));
    let authority = Arc::new(ControlledAdmission {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        fail: std::sync::atomic::AtomicBool::new(true),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    registry.set_work_authority(authority.clone()).await;
    let creating = registry.clone();
    let first = tokio::spawn(async move { creating.get_or_create("retry").await });
    authority.entered.notified().await;
    let second = registry.get_or_create("retry");
    tokio::pin!(second);
    assert!(matches!(
        futures::poll!(second.as_mut()),
        std::task::Poll::Pending
    ));
    authority.release.notify_one();
    assert!(first.await.unwrap().is_err());
    assert!(second.await.is_err());
    assert_eq!(authority.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(registry.get("retry").await.is_none());
    authority
        .fail
        .store(false, std::sync::atomic::Ordering::SeqCst);
    authority.release.notify_one();
    assert!(registry.get_or_create("retry").await.is_ok());
    assert_eq!(authority.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn component_health_is_readable_during_slow_start() {
    let manager = Arc::new(rua_workspace::ServiceManager::with_descriptors(vec![(
        rua_workspace::ServiceDescriptor {
            name: "memory".to_string(),
            priority: 1,
            phase: rua_workspace::InitPhase::Runtime,
        },
        rua_workspace::ComponentState::NotInitialized,
    )]));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    manager
        .replace_lifecycle(
            "memory",
            Arc::new(ControlledStart {
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
        )
        .await;
    let starting = Arc::clone(&manager);
    let handle = tokio::spawn(async move {
        starting
            .start_phase(rua_workspace::InitPhase::Runtime)
            .await
    });
    entered.notified().await;
    let state = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        manager.service_state("memory"),
    )
    .await
    .expect("health must not wait for start");
    assert_eq!(state, Some(rua_workspace::ComponentState::Initializing));
    let stop = manager
        .stop_owned(tokio::time::Instant::now() + std::time::Duration::from_millis(10))
        .await;
    assert_eq!(stop.timed_out, vec!["service_operation_timeout"]);
    release.notify_one();
    handle.await.unwrap().unwrap();
    assert_eq!(
        manager.service_state("memory").await,
        Some(rua_workspace::ComponentState::Ready)
    );
}

#[tokio::test]
async fn concurrent_gets_share_one_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let root = RuaRoot::new(temp.path());
    let workspace = test_helpers::agent_workspace(&root, "single-flight");
    workspace.ensure().unwrap();
    rua_config::atomic_write_json(
        &workspace.as_path().join("agent.json"),
        &serde_json::json!({"id":"single-flight","name":"Single flight"}),
    )
    .unwrap();
    let registry = Arc::new(DefaultWorkspaceRegistry::new(
        root,
        PlatformProfile::Desktop,
        "single-flight",
        test_helpers::test_context(temp.path().to_path_buf()),
    ));
    let channel_manager = Arc::new(rua_channel::core::ChannelManager::new());
    registry.install_channel_manager("single-flight", Arc::clone(&channel_manager));
    let barrier = Arc::new(tokio::sync::Barrier::new(9));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let registry = Arc::clone(&registry);
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            registry.get_or_create("single-flight").await.unwrap()
        }));
    }
    barrier.wait().await;
    let first = handles.remove(0).await.unwrap();
    assert!(Arc::ptr_eq(
        &channel_manager,
        &first.channel_manager().await.unwrap()
    ));
    for handle in handles {
        assert!(Arc::ptr_eq(&first, &handle.await.unwrap()));
    }
    assert_eq!(registry.list().await.unwrap().len(), 1);
    assert_eq!(first.maintenance_tick_count(), 0);
    let failures = first.stop().await;
    assert_eq!(failures, vec!["work_authority"]);
}

#[tokio::test]
async fn invalid_agent_failure_does_not_poison_later_creations() {
    let temp = tempfile::tempdir().unwrap();
    let registry = DefaultWorkspaceRegistry::new(
        RuaRoot::new(temp.path()),
        PlatformProfile::Mobile,
        "valid",
        test_helpers::test_context(temp.path().to_path_buf()),
    );
    assert!(registry.get_or_create("../invalid").await.is_err());
    assert!(registry.get_or_create("../invalid").await.is_err());
    let (left, right) = tokio::join!(
        registry.get_or_create("left"),
        registry.get_or_create("right")
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert!(!Arc::ptr_eq(&left, &right));
    assert_eq!(registry.list().await.unwrap().len(), 2);
}
