use crate::runtime_observation_receipts::TaskWatchWakeReceipts;
use rua_core::ChatAddress;
use std::time::Duration;

#[tokio::test]
async fn an_overlapping_delivery_is_rejected_while_the_first_is_in_flight() {
    let receipts = TaskWatchWakeReceipts::default();
    let address = ChatAddress::new("photo-chat");
    let mut first = receipts
        .register("terminal-note".into(), address.clone())
        .expect("first claim");
    first.enqueued = true;
    let id = first.receipt_id.clone();
    let overlapping = match receipts.register("terminal-note".into(), address.clone()) {
        Ok(_) => panic!("an in-flight observation must not be claimed twice"),
        Err(error) => error,
    };
    assert!(
        overlapping.contains("already registered"),
        "unexpected rejection: {overlapping}"
    );
    // Releasing the observation lets a genuinely new delivery claim it again.
    let waiter = tokio::spawn(first.wait());
    receipts.complete(&id);
    waiter.await.unwrap().unwrap();
    let mut retry = receipts
        .register("terminal-note".into(), address)
        .expect("a released observation may be delivered again");
    retry.enqueued = true;
    assert_ne!(retry.receipt_id, id);
}

#[tokio::test]
async fn failed_receipt_can_retry_and_old_generation_cannot_finish_new_generation() {
    let receipts = TaskWatchWakeReceipts::default();
    let address = ChatAddress::new("photo-chat");
    let mut old = receipts
        .register("terminal-note".into(), address.clone())
        .expect("first claim");
    old.enqueued = true;
    let old_id = old.receipt_id.clone();
    receipts.fail(&old_id, "disk write failed");
    let mut next = receipts
        .register("terminal-note".into(), address)
        .expect("a failed observation may be retried");
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
    let fresh = receipts
        .register("terminal-note".into(), address.clone())
        .expect("first claim");
    drop(fresh);
    // Abandoning a claim before it is enqueued must release it, so the sender is
    // told the observation failed rather than left waiting forever.
    let mut retry = receipts
        .register("terminal-note".into(), address.clone())
        .expect("an abandoned claim must not block the retry");
    retry.enqueued = true;
    receipts.fail_all_for_address(&address, "producer panic");
    assert_eq!(retry.wait().await.unwrap_err(), "producer panic");
}
