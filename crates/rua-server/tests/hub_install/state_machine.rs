use std::sync::Arc;
use std::time::Duration;

use rua_server::services::hub_install_task::{
    run_install_task, HubInstallRegistry, HubInstallStatusKind,
};
use serde_json::json;

#[test]
fn task_owner_controls_cancellation_and_publication() {
    let registry = HubInstallRegistry::new();
    let (id, token) = registry.create("agent-a".into(), "source".into(), String::new(), false);
    assert_eq!(registry.snapshot(&id).unwrap().agent_id, "agent-a");
    assert!(registry.cancel("agent-b", &id).is_none());
    assert!(!token.is_cancelled());
    assert!(!registry.mark_sync_published("agent-b", &id));
    assert!(!registry.snapshot(&id).unwrap().sync_published);
    assert_eq!(
        registry.cancel("agent-a", &id),
        Some(HubInstallStatusKind::Cancelled)
    );
    assert!(token.is_cancelled());
    registry.set_status(
        &id,
        HubInstallStatusKind::Completed,
        None,
        Some(json!({"name":"skill"})),
    );
    assert!(registry.mark_sync_published("agent-a", &id));
    assert!(!registry.mark_sync_published("agent-a", &id));
}

#[tokio::test]
async fn start_to_completed() {
    let registry = Arc::new(HubInstallRegistry::new());
    let (task_id, cancel) = registry.create(
        "agent-a".into(),
        "https://hub/x".to_string(),
        String::new(),
        true,
    );

    assert_eq!(
        registry.snapshot(&task_id).unwrap().status,
        HubInstallStatusKind::Pending
    );

    run_install_task(
        registry.clone(),
        task_id.clone(),
        cancel,
        Duration::from_secs(5),
        |_token| async { Ok(json!({ "installed": true, "name": "x" })) },
    )
    .await;

    let task = registry.snapshot(&task_id).unwrap();
    assert_eq!(task.status, HubInstallStatusKind::Completed);
    assert!(task.error.is_none());
    assert_eq!(
        task.result.as_ref().and_then(|r| r.get("name")),
        Some(&json!("x"))
    );
}

#[tokio::test]
async fn start_then_cancel_is_cancelled() {
    let registry = Arc::new(HubInstallRegistry::new());
    let (task_id, cancel) = registry.create(
        "agent-a".into(),
        "https://hub/x".to_string(),
        String::new(),
        true,
    );

    let reg = registry.clone();
    let id = task_id.clone();
    let handle = tokio::spawn(async move {
        run_install_task(
            reg,
            id,
            cancel,
            Duration::from_secs(5),
            |token| async move {
                token.cancelled().await;
                Err("cancelled by token".to_string())
            },
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    let prev = registry.cancel("agent-a", &task_id);
    assert_eq!(prev, Some(HubInstallStatusKind::Cancelled));

    handle.await.unwrap();
    assert_eq!(
        registry.snapshot(&task_id).unwrap().status,
        HubInstallStatusKind::Cancelled
    );
}

#[tokio::test]
async fn timeout_marks_failed() {
    let registry = Arc::new(HubInstallRegistry::new());
    let (task_id, cancel) = registry.create(
        "agent-a".into(),
        "https://hub/x".to_string(),
        String::new(),
        true,
    );

    run_install_task(
        registry.clone(),
        task_id.clone(),
        cancel,
        Duration::from_millis(50),
        |_token| async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(json!({}))
        },
    )
    .await;

    let task = registry.snapshot(&task_id).unwrap();
    assert_eq!(task.status, HubInstallStatusKind::Failed);
    assert!(task.error.unwrap().contains("timed out"));
}

#[tokio::test]
async fn cancel_unknown_task_returns_none() {
    let registry = HubInstallRegistry::new();
    assert!(registry.cancel("agent-a", "no-such-task").is_none());
    assert!(registry.snapshot("no-such-task").is_none());
}
