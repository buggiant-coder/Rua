use super::test_helpers;

use std::time::Duration;

use rua_workspace::{PlatformProfile, RuaRoot, SessionConcurrencyPolicy, WorkspaceRuntime};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn build_runtime(policy: SessionConcurrencyPolicy) -> (TempDir, WorkspaceRuntime) {
    let tmp = TempDir::new().expect("temp dir");
    let root = RuaRoot::new(tmp.path());
    let workspace = test_helpers::agent_workspace(&root, "test-agent");
    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let runtime = WorkspaceRuntime::with_session_policy(
        "test-agent",
        "Test Agent",
        workspace,
        PlatformProfile::Desktop,
        ctx,
        policy,
    );
    (tmp, runtime)
}

#[tokio::test]
async fn cancel_previous_fires_token_and_replaces_entry() {
    let (_tmp, runtime) = build_runtime(SessionConcurrencyPolicy::CancelPrevious);

    let cancel_a = CancellationToken::new();
    let guard_a = runtime
        .acquire_session("sess-1", "req-a", cancel_a.clone())
        .await
        .expect("first acquire succeeds");
    assert_eq!(runtime.active_session_count(), 1);
    assert!(!cancel_a.is_cancelled());

    let cancel_b = CancellationToken::new();
    let guard_b = runtime
        .acquire_session("sess-1", "req-b", cancel_b.clone())
        .await
        .expect("second acquire succeeds under CancelPrevious");

    assert!(
        cancel_a.is_cancelled(),
        "previous request's cancel token must be fired"
    );
    assert!(!cancel_b.is_cancelled());
    assert_eq!(
        runtime
            .session_tracker()
            .active_request_id("sess-1")
            .await
            .as_deref(),
        Some("req-b"),
        "tracker must now point at the second request"
    );
    assert_eq!(
        runtime.active_session_count(),
        2,
        "counter increments per acquire; release happens on drop"
    );

    drop(guard_a);
    drop(guard_b);

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(runtime.active_session_count(), 0);
    assert!(!runtime.session_tracker().is_active("sess-1").await);
}

#[tokio::test]
async fn reject_policy_returns_session_busy() {
    let (_tmp, runtime) = build_runtime(SessionConcurrencyPolicy::Reject);

    let _guard_a = runtime
        .acquire_session("sess-2", "req-a", CancellationToken::new())
        .await
        .expect("first acquire succeeds");

    let err = runtime
        .acquire_session("sess-2", "req-b", CancellationToken::new())
        .await
        .expect_err("second acquire must be rejected under Reject policy");
    let msg = err.to_string();
    assert!(
        msg.contains("session_busy") || msg.to_lowercase().contains("busy"),
        "error should mention session busy, got: {msg}"
    );
}

#[tokio::test]
async fn queue_policy_rejects_over_max() {
    let (_tmp, runtime) = build_runtime(SessionConcurrencyPolicy::Queue { max_queue: 1 });

    let _guard_a = runtime
        .acquire_session("sess-3", "req-a", CancellationToken::new())
        .await
        .expect("first acquire succeeds (Acquired)");

    let _guard_b = runtime
        .acquire_session("sess-3", "req-b", CancellationToken::new())
        .await
        .expect("queued acquire succeeds while queue has capacity");

    let err = runtime
        .acquire_session("sess-3", "req-c", CancellationToken::new())
        .await
        .expect_err("queue full must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("queue full") || msg.to_lowercase().contains("queue full"),
        "error should mention queue full, got: {msg}"
    );
}

#[tokio::test]
async fn guard_drop_releases_tracker_entry() {
    let (_tmp, runtime) = build_runtime(SessionConcurrencyPolicy::CancelPrevious);

    {
        let _guard = runtime
            .acquire_session("sess-4", "req-1", CancellationToken::new())
            .await
            .expect("acquire");
        assert!(runtime.session_tracker().is_active("sess-4").await);
        assert_eq!(runtime.active_session_count(), 1);
    }

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !runtime.session_tracker().is_active("sess-4").await,
        "tracker entry must be removed after guard drop"
    );
    assert_eq!(runtime.active_session_count(), 0);
}

#[tokio::test]
async fn default_policy_is_cancel_previous() {
    let tmp = TempDir::new().unwrap();
    let ctx = test_helpers::test_context(tmp.path().to_path_buf());
    let root = RuaRoot::new(tmp.path());
    let runtime = WorkspaceRuntime::new(
        "test-agent",
        "Test Agent",
        test_helpers::agent_workspace(&root, "test-agent"),
        PlatformProfile::Desktop,
        ctx,
    );

    let cancel_a = CancellationToken::new();
    let _guard_a = runtime
        .acquire_session("sess-5", "req-a", cancel_a.clone())
        .await
        .expect("acquire a");

    let _guard_b = runtime
        .acquire_session("sess-5", "req-b", CancellationToken::new())
        .await
        .expect("acquire b under default policy");

    assert!(
        cancel_a.is_cancelled(),
        "default policy must fire previous cancel token"
    );
}
