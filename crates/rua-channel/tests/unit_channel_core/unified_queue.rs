use std::sync::Arc;

use async_trait::async_trait;
use rua_channel::core::queue::{QueueConsumer, UnifiedQueueManager};
use tokio::sync::mpsc;

struct NoopConsumer;

#[async_trait]
impl QueueConsumer for NoopConsumer {
    async fn consume(
        &self,
        mut rx: mpsc::Receiver<serde_json::Value>,
        _channel_id: String,
        _session_id: String,
        _priority: u32,
    ) {
        while rx.recv().await.is_some() {}
    }
}

#[tokio::test]
async fn enqueue_reuses_same_session_priority_queue() {
    let consumer = Arc::new(NoopConsumer);
    let mgr = UnifiedQueueManager::new(consumer);

    assert_eq!(mgr.queue_count().await, 0);

    mgr.enqueue(
        "console".into(),
        "session1".into(),
        0,
        serde_json::json!({"text": "hello"}),
    )
    .await
    .unwrap();
    assert_eq!(mgr.queue_count().await, 1);

    mgr.enqueue(
        "console".into(),
        "session1".into(),
        0,
        serde_json::json!({"text": "world"}),
    )
    .await
    .unwrap();
    assert_eq!(mgr.queue_count().await, 1);

    mgr.stop_all().await;
}

#[tokio::test]
async fn different_sessions_or_priorities_create_separate_queues() {
    let consumer = Arc::new(NoopConsumer);
    let mgr = UnifiedQueueManager::new(consumer);

    mgr.enqueue("console".into(), "s1".into(), 0, serde_json::json!({}))
        .await
        .unwrap();
    mgr.enqueue("console".into(), "s2".into(), 0, serde_json::json!({}))
        .await
        .unwrap();
    mgr.enqueue("console".into(), "s1".into(), 20, serde_json::json!({}))
        .await
        .unwrap();

    assert_eq!(mgr.queue_count().await, 3);

    mgr.stop_all().await;
}

#[tokio::test]
async fn stop_all_clears_all_queues() {
    let consumer = Arc::new(NoopConsumer);
    let mgr = UnifiedQueueManager::new(consumer);

    mgr.enqueue("console".into(), "s1".into(), 0, serde_json::json!({}))
        .await
        .unwrap();
    mgr.enqueue("dingtalk".into(), "s2".into(), 20, serde_json::json!({}))
        .await
        .unwrap();

    assert_eq!(mgr.queue_count().await, 2);

    mgr.stop_all().await;

    assert_eq!(mgr.queue_count().await, 0);
}

#[tokio::test]
async fn idle_eviction_defaults_to_ten_minutes() {
    let consumer = Arc::new(NoopConsumer);
    let mgr = UnifiedQueueManager::new(consumer);

    assert_eq!(mgr.idle_eviction_secs, 600);
}

#[tokio::test]
async fn idle_eviction_gc_reclaims_idle_queues() {
    let consumer = Arc::new(NoopConsumer);
    let mgr = UnifiedQueueManager::new(consumer)
        .with_idle_eviction_secs(1)
        .with_cleanup_interval_secs(1);

    mgr.enqueue(
        "console".into(),
        "s1".into(),
        0,
        serde_json::json!({"x": 1}),
    )
    .await
    .unwrap();
    assert_eq!(mgr.queue_count().await, 1);

    mgr.start_cleanup_loop();
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

    assert_eq!(mgr.queue_count().await, 0);

    mgr.stop_all().await;
}
