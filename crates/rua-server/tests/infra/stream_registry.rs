use std::sync::Arc;
use std::time::Duration;

use rua_core::{runtime::RunControl, ChatAddress};
use rua_server::{ActiveStreamRegistry, Priority, StreamKey};

fn key(chat_id: &str) -> StreamKey {
    StreamKey::new(
        "console",
        ChatAddress::agent("default", chat_id),
        Priority::Normal,
    )
}

#[tokio::test]
async fn registered_execution_replays_and_fans_out_events() {
    let registry = ActiveStreamRegistry::new();
    let key = key("chat-1");
    let (_producer_id, _cancel_rx) = registry.register_active_keyed(&key).await;
    registry.push_event_keyed(&key, "first".to_string()).await;

    let (replay, mut rx) = registry
        .subscribe_keyed(&key)
        .await
        .expect("active execution subscription");
    assert_eq!(replay, vec!["first"]);

    registry.push_event_keyed(&key, "second".to_string()).await;
    assert_eq!(rx.recv().await.as_deref(), Some("second"));
}

#[tokio::test]
async fn execution_owner_waits_for_previous_owner_to_finish() {
    let registry = Arc::new(ActiveStreamRegistry::new());
    let key = key("chat-1");
    let (producer_id, _cancel_rx) = registry.register_active_keyed(&key).await;

    let mut waiting = {
        let registry = Arc::clone(&registry);
        let key = key.clone();
        tokio::spawn(async move { registry.register_active_keyed(&key).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut waiting)
            .await
            .is_err(),
        "a second execution owner must wait for the first owner"
    );

    assert!(registry.mark_completed(&key, producer_id).await);
    let (_next_producer_id, _next_cancel_rx) =
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("next owner registers after completion")
            .expect("owner task");
}

#[tokio::test]
async fn stop_and_interjection_are_execution_signals_only() {
    let registry = ActiveStreamRegistry::new();
    let key = key("chat-1");
    let (_producer_id, cancel_rx) = registry.register_active_keyed(&key).await;

    assert!(registry.interject_key_at_boundary(&key).await);
    assert_eq!(*cancel_rx.borrow(), RunControl::InterjectAtBoundary);
    registry.clear_pending_control(&key).await;
    assert_eq!(*cancel_rx.borrow(), RunControl::Running);

    assert!(registry.continue_key_at_boundary(&key).await);
    assert_eq!(*cancel_rx.borrow(), RunControl::ContinueAtBoundary);
    registry.clear_pending_control(&key).await;
    assert_eq!(*cancel_rx.borrow(), RunControl::Running);

    assert!(registry.interject_key(&key).await);
    assert_eq!(*cancel_rx.borrow(), RunControl::Interject);
    registry.clear_pending_control(&key).await;
    assert!(registry.cancel_key(&key).await);
    assert_eq!(*cancel_rx.borrow(), RunControl::Stop);
    assert!(!registry.interject_key(&key).await);
    assert!(!registry.interject_key_at_boundary(&key).await);
    assert_eq!(
        *cancel_rx.borrow(),
        RunControl::Stop,
        "later interjection requests must not replace an explicit Stop"
    );
}

#[tokio::test]
async fn active_turn_binding_is_execution_metadata_not_queue_state() {
    let registry = ActiveStreamRegistry::new();
    let key = key("chat-1");
    let (_producer_id, _cancel_rx) = registry.register_active_keyed(&key).await;

    assert!(
        registry
            .set_active_turn_id(
                "console",
                &ChatAddress::agent("default", "chat-1"),
                Priority::Normal,
                "turn-1"
            )
            .await
    );
    assert_eq!(
        registry
            .active_turn_id_for_chat("console", &ChatAddress::agent("default", "chat-1"))
            .await
            .as_deref(),
        Some("turn-1")
    );
}

#[tokio::test]
async fn idle_watcher_wakes_when_execution_registers() {
    let registry = Arc::new(ActiveStreamRegistry::new());
    let key = key("chat-1");
    let watcher = {
        let registry = Arc::clone(&registry);
        let key = key.clone();
        tokio::spawn(async move { registry.subscribe_keyed_or_wait(&key).await })
    };

    let (_producer_id, _cancel_rx) = registry.register_active_keyed(&key).await;
    let (replay, _rx) = tokio::time::timeout(Duration::from_secs(1), watcher)
        .await
        .expect("watcher wakes")
        .expect("watcher task");
    assert!(replay.is_empty());
}

#[tokio::test]
async fn stale_execution_cleanup_removes_inactive_entries() {
    let registry = ActiveStreamRegistry::new();
    let key = key("chat-1");
    let (_producer_id, _cancel_rx) = registry.register_active_keyed(&key).await;
    tokio::time::sleep(Duration::from_millis(2)).await;

    assert_eq!(registry.cleanup_stale(Duration::from_millis(1)).await, 1);
    assert!(
        !registry
            .is_active_for_chat("console", &ChatAddress::agent("default", "chat-1"))
            .await
    );
}

#[tokio::test]
async fn equal_chat_ids_in_agent_and_project_scopes_isolate_streams_and_stop() {
    let registry = ActiveStreamRegistry::new();
    let agent_address = ChatAddress::agent("same-owner", "shared-chat");
    let project_address = ChatAddress::project("same-owner", "shared-chat");
    let agent_key = StreamKey::new("console", agent_address.clone(), Priority::Normal);
    let project_key = StreamKey::new("console", project_address.clone(), Priority::Normal);
    let (_agent_producer, agent_cancel) = registry.register_active_keyed(&agent_key).await;
    let (_project_producer, project_cancel) = registry.register_active_keyed(&project_key).await;
    let (_, mut agent_rx) = registry
        .subscribe_keyed(&agent_key)
        .await
        .expect("Agent stream");
    let (_, mut project_rx) = registry
        .subscribe_keyed(&project_key)
        .await
        .expect("Project stream");

    registry
        .push_event_keyed(&agent_key, "agent-only".to_string())
        .await;
    assert_eq!(agent_rx.recv().await.as_deref(), Some("agent-only"));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), project_rx.recv())
            .await
            .is_err(),
        "Agent replay must not reach the equal-UUID Project stream"
    );

    assert_eq!(
        registry
            .cancel_all_priorities("console", &agent_address)
            .await,
        1
    );
    assert_eq!(*agent_cancel.borrow(), RunControl::Stop);
    assert_eq!(*project_cancel.borrow(), RunControl::Running);
    assert!(
        registry
            .is_active_for_chat("console", &project_address)
            .await
    );
}
