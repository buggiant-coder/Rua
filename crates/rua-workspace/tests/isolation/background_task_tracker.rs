use std::time::Duration;

use super::test_helpers;

use rua_workspace::registry::DefaultWorkspaceRegistry;
use rua_workspace::{
    ActiveTaskTracker, PlatformProfile, RuaRoot, WorkspaceRegistry, WorkspaceRuntime,
};
use tempfile::TempDir;

fn build_runtime(agent_id: &str) -> (TempDir, WorkspaceRuntime) {
    let tmp = TempDir::new().expect("temp dir");
    let root = RuaRoot::new(tmp.path());
    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let workspace = test_helpers::agent_workspace(&root, agent_id);
    let runtime =
        WorkspaceRuntime::new(agent_id, agent_id, workspace, PlatformProfile::Desktop, ctx);
    (tmp, runtime)
}

#[tokio::test]
async fn active_task_tracker_register_complete_and_names_match_rust_reload_boundary() {
    let tracker = ActiveTaskTracker::new();

    assert_eq!(tracker.active_count(), 0);
    assert!(tracker.active_task_names().await.is_empty());

    let cancel = tracker
        .register(
            "task-1".to_string(),
            "background title generation".to_string(),
        )
        .await;

    assert_eq!(tracker.active_count(), 1);
    assert_eq!(
        tracker.active_task_names().await,
        vec!["background title generation".to_string()]
    );
    assert!(!cancel.is_cancelled());

    tracker.complete("task-1").await;

    assert_eq!(tracker.active_count(), 0);
    assert!(tracker.active_task_names().await.is_empty());
    assert!(tracker.drain(Duration::from_millis(1)).await);
}

#[tokio::test]
async fn active_task_tracker_lists_task_names_in_stable_order() {
    let tracker = ActiveTaskTracker::new();
    tracker
        .register("task-z".to_string(), "zeta sync".to_string())
        .await;
    tracker
        .register("task-a".to_string(), "alpha sync".to_string())
        .await;
    tracker
        .register("task-m".to_string(), "middle sync".to_string())
        .await;

    assert_eq!(
        tracker.active_task_names().await,
        vec![
            "alpha sync".to_string(),
            "middle sync".to_string(),
            "zeta sync".to_string(),
        ],
        "status/list surfaces should not inherit HashMap iteration order"
    );
}

#[tokio::test]
async fn active_task_tracker_register_is_idempotent_for_live_task_id() {
    let tracker = ActiveTaskTracker::new();
    let first = tracker
        .register("task-1".to_string(), "first name".to_string())
        .await;
    let second = tracker
        .register(
            "task-1".to_string(),
            "second name should not replace".to_string(),
        )
        .await;

    assert_eq!(tracker.active_count(), 1);
    assert_eq!(tracker.active_task_names().await, vec!["first name"]);

    second.cancel();
    assert!(
        first.is_cancelled(),
        "duplicate registration should return the existing cancellation token"
    );

    tracker.complete("task-1").await;
    assert_eq!(tracker.active_count(), 0);
}

#[tokio::test]
async fn active_task_tracker_completed_task_id_can_start_new_lifecycle() {
    let tracker = ActiveTaskTracker::new();
    let first = tracker
        .register("task-reuse".to_string(), "first run".to_string())
        .await;

    tracker.complete("task-reuse").await;

    let second = tracker
        .register("task-reuse".to_string(), "second run".to_string())
        .await;

    assert!(!first.is_cancelled());
    assert!(!second.is_cancelled());
    assert_eq!(tracker.active_count(), 1);
    assert_eq!(tracker.active_task_names().await, vec!["second run"]);

    second.cancel();
    assert!(
        !first.is_cancelled(),
        "a completed task id reused later must not share the old cancellation token"
    );
    assert!(second.is_cancelled());

    tracker.complete("task-reuse").await;
    assert_eq!(tracker.active_count(), 0);
}

#[tokio::test]
async fn active_task_tracker_cancel_and_cancel_all_signal_tokens_without_dropping_tasks() {
    let tracker = ActiveTaskTracker::new();
    let first = tracker
        .register("task-1".to_string(), "first".to_string())
        .await;
    let second = tracker
        .register("task-2".to_string(), "second".to_string())
        .await;

    assert!(tracker.cancel("task-1").await);
    assert!(first.is_cancelled());
    assert!(!second.is_cancelled());
    assert_eq!(tracker.active_count(), 2);

    assert!(!tracker.cancel("missing").await);
    assert_eq!(tracker.cancel_all().await, 2);
    assert!(second.is_cancelled());
    assert_eq!(
        tracker.active_count(),
        2,
        "cancellation requests do not mark tasks complete; task owners must call complete"
    );

    tracker.complete("task-1").await;
    tracker.complete("task-2").await;
    assert_eq!(tracker.active_count(), 0);
}

#[tokio::test]
async fn active_task_tracker_unknown_and_empty_lifecycle_ops_are_safe() {
    let tracker = ActiveTaskTracker::new();

    tracker.complete("never-registered").await;
    assert!(!tracker.cancel("never-registered").await);
    assert_eq!(tracker.cancel_all().await, 0);
    assert!(tracker.drain(Duration::from_millis(1)).await);

    assert_eq!(tracker.active_count(), 0);
    assert!(tracker.active_task_names().await.is_empty());
}

#[tokio::test]
async fn active_task_tracker_drain_waits_until_complete_or_times_out() {
    let tracker = std::sync::Arc::new(ActiveTaskTracker::new());
    tracker
        .register("task-1".to_string(), "slow task".to_string())
        .await;

    assert!(!tracker.drain(Duration::from_millis(1)).await);

    let worker_tracker = std::sync::Arc::clone(&tracker);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        worker_tracker.complete("task-1").await;
    });

    assert!(tracker.drain(Duration::from_secs(1)).await);
    assert_eq!(tracker.active_count(), 0);
}

#[tokio::test]
async fn workspace_runtime_has_active_work_includes_background_tasks() {
    let (_tmp, runtime) = build_runtime("agent-bg");

    assert!(!runtime.has_active_work());
    let cancel = runtime
        .task_tracker()
        .register(
            "task-1".to_string(),
            "external background request".to_string(),
        )
        .await;

    assert!(runtime.has_active_work());
    assert_eq!(runtime.background_task_count(), 1);
    assert!(!cancel.is_cancelled());

    runtime.begin_drain().await;
    assert!(!runtime.wait_drain(Duration::from_millis(1)).await);

    runtime.task_tracker().complete("task-1").await;
    assert!(runtime.wait_drain(Duration::from_millis(1)).await);
    assert!(!runtime.has_active_work());
}

#[tokio::test]
async fn workspace_runtime_stop_cancels_tracked_background_tasks() {
    let (_tmp, runtime) = build_runtime("agent-bg-stop");
    let cancel = runtime
        .task_tracker()
        .register("task-1".to_string(), "long task".to_string())
        .await;

    let _failed_services = runtime.stop().await;

    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn reload_agent_without_work_authority_reports_failure_and_retains_old_runtime() {
    let tmp = TempDir::new().expect("temp dir");
    let root = RuaRoot::new(tmp.path());
    let agent_dir = test_helpers::agent_workspace(&root, "agent-bg-reload")
        .as_path()
        .to_path_buf();
    std::fs::create_dir_all(&agent_dir).expect("agent workspace dir");
    std::fs::write(
        agent_dir.join("agent.json"),
        r#"{"id":"agent-bg-reload","name":"Agent BG Reload"}"#,
    )
    .expect("agent json");

    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let mut registry =
        DefaultWorkspaceRegistry::new(root, PlatformProfile::Desktop, "agent-bg-reload", ctx);
    registry.set_drain_timeout(Duration::from_secs(1));
    registry
        .register_agent("agent-bg-reload", "Agent BG Reload")
        .await;

    let old_runtime = registry.get_or_create("agent-bg-reload").await.unwrap();
    let cancel = old_runtime
        .task_tracker()
        .register(
            "background-1".to_string(),
            "external background request".to_string(),
        )
        .await;

    assert!(registry.reload_agent("agent-bg-reload").await.is_err());

    let new_runtime = registry.get("agent-bg-reload").await.unwrap();
    assert!(
        std::sync::Arc::ptr_eq(&old_runtime, &new_runtime),
        "failed shutdown must retain the owner for cleanup retry"
    );
    assert!(
        cancel.is_cancelled(),
        "owned background task receives cancellation during cleanup"
    );

    old_runtime.task_tracker().complete("background-1").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(old_runtime.background_task_count(), 0);
    assert!(cancel.is_cancelled());
}
