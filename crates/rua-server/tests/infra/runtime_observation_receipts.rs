use crate::runtime_observation_receipts::TaskWatchWakeReceipts;
use rua_core::ChatAddress;
use std::time::Duration;

#[tokio::test]
async fn enqueued_observation_keeps_one_receipt_after_waiter_drop_and_retry() {
    let receipts = TaskWatchWakeReceipts::default();
    let address = ChatAddress::new("photo-chat");
    let mut first = receipts.register("terminal-note".into(), address.clone());
    assert!(first.fresh);
    first.enqueued = true;
    let id = first.receipt_id.clone();
    drop(first);
    let second = receipts.register("terminal-note".into(), address.clone());
    let third = receipts.register("terminal-note".into(), address);
    assert!(!second.fresh);
    assert!(!third.fresh);
    assert_eq!(second.receipt_id, id);
    let second = tokio::spawn(second.wait());
    let third = tokio::spawn(third.wait());
    receipts.complete(&id);
    second.await.unwrap().unwrap();
    third.await.unwrap().unwrap();
}

#[tokio::test]
async fn failed_receipt_can_retry_and_old_generation_cannot_finish_new_generation() {
    let receipts = TaskWatchWakeReceipts::default();
    let address = ChatAddress::new("photo-chat");
    let mut old = receipts.register("terminal-note".into(), address.clone());
    old.enqueued = true;
    let old_id = old.receipt_id.clone();
    receipts.fail(&old_id, "disk write failed");
    let mut next = receipts.register("terminal-note".into(), address);
    assert!(next.fresh);
    next.enqueued = true;
    let next_id = next.receipt_id.clone();
    assert_ne!(next_id, old_id);
    assert_eq!(old.wait().await.unwrap_err(), "disk write failed");
    let mut waiting = Box::pin(next.wait());
    receipts.complete(&old_id);
    receipts.fail(&old_id, "late old producer error");
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut waiting)
            .await
            .is_err()
    );
    receipts.complete(&next_id);
    waiting.await.unwrap();
}

#[tokio::test]
async fn producer_panic_and_pre_enqueue_drop_fail_instead_of_acknowledging() {
    let receipts = TaskWatchWakeReceipts::default();
    let address = ChatAddress::new("photo-chat");
    let fresh = receipts.register("terminal-note".into(), address.clone());
    let subscriber = receipts.register("terminal-note".into(), address.clone());
    drop(fresh);
    assert!(subscriber.wait().await.is_err());
    let mut retry = receipts.register("terminal-note".into(), address.clone());
    retry.enqueued = true;
    receipts.fail_all_for_address(&address, "producer panic");
    assert_eq!(retry.wait().await.unwrap_err(), "producer panic");
}
