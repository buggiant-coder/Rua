use iroh_docs::AuthorId;
use rua_sync::endpoint::SyncNode;
use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use rua_sync::inbound_inbox::{
    next_retry_after_ms, InboundInbox, InboxState, DAILY_REPAIR_SCAN_INTERVAL_MS,
};

fn author(byte: u8) -> AuthorId {
    AuthorId::from(&[byte; 32])
}

fn temp_inbox() -> (tempfile::TempDir, InboundInbox) {
    let dir = tempfile::tempdir().unwrap();
    let inbox = InboundInbox::open(dir.path()).unwrap();
    (dir, inbox)
}

#[test]
fn record_remote_insert_is_pending() {
    let (_dir, inbox) = temp_inbox();
    let key = b"opaque-key";
    let hash = iroh_blobs::Hash::new(b"ciphertext");

    inbox.record_remote_insert(author(1), key, hash).unwrap();

    assert_eq!(inbox.count_by_state(InboxState::Pending).unwrap(), 1);
    let pending = inbox.pending_ready(u64::MAX).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].author, author(1));
    assert_eq!(pending[0].key, key);
    assert_eq!(pending[0].state, InboxState::Pending);
}

#[test]
fn replica_deletion_markers_do_not_wait_for_content_and_repair_old_dormant_rows() {
    let (dir, inbox) = temp_inbox();
    inbox
        .record_remote_insert(author(1), b"deleted", iroh_blobs::Hash::EMPTY)
        .unwrap();
    assert_eq!(inbox.count_by_state(InboxState::Discarded).unwrap(), 1);
    assert!(inbox.pending_ready(u64::MAX).unwrap().is_empty());
    let conn = rusqlite::Connection::open(dir.path().join("sync-state.sqlite")).unwrap();
    conn.execute(
        "UPDATE inbound_inbox SET state='dormant', attempts=3, last_error='content unavailable'",
        [],
    )
    .unwrap();
    assert!(inbox
        .record_remote_insert_for_repair(author(1), b"deleted", iroh_blobs::Hash::EMPTY)
        .unwrap());
    assert_eq!(inbox.count_by_state(InboxState::Dormant).unwrap(), 0);
    assert!(!inbox
        .record_remote_insert_for_repair(author(1), b"deleted", iroh_blobs::Hash::EMPTY)
        .unwrap());
    inbox
        .record_remote_insert(author(1), b"deleted", iroh_blobs::Hash::new(b"replacement"))
        .unwrap();
    assert_eq!(inbox.count_by_state(InboxState::Pending).unwrap(), 1);
}

#[test]
fn same_content_hash_is_an_idempotent_noop() {
    let (_dir, inbox) = temp_inbox();
    let key = b"same-version";
    let hash = iroh_blobs::Hash::new(b"body");
    inbox.record_remote_insert(author(1), key, hash).unwrap();
    inbox
        .mark_retryable(author(1), key, "blob missing", 1_000)
        .unwrap();

    assert!(!inbox
        .record_remote_insert_for_repair(author(1), key, hash)
        .unwrap());

    let ready = inbox.pending_ready(1_000).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].state, InboxState::Retryable);
    assert_eq!(ready[0].attempts, 1);
    assert_eq!(ready[0].last_error.as_deref(), Some("blob missing"));
}

#[test]
fn changed_content_hash_resets_processing_state() {
    let (_dir, inbox) = temp_inbox();
    let key = b"changed-version";
    inbox
        .record_remote_insert(author(1), key, iroh_blobs::Hash::new(b"old"))
        .unwrap();
    inbox.mark_failed(author(1), key, "bad envelope").unwrap();
    let replacement = iroh_blobs::Hash::new(b"new");

    assert!(inbox
        .record_remote_insert_for_repair(author(1), key, replacement)
        .unwrap());

    let ready = inbox.pending_ready(u64::MAX).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].content_hash, replacement.to_string());
    assert_eq!(ready[0].state, InboxState::Pending);
    assert_eq!(ready[0].attempts, 0);
    assert_eq!(ready[0].last_error, None);
}

#[test]
fn sqlite_inbox_reopens_with_durable_state() {
    let (dir, inbox) = temp_inbox();
    let key = b"durable";
    inbox
        .record_remote_insert(author(5), key, iroh_blobs::Hash::new(b"body"))
        .unwrap();
    inbox
        .mark_retryable(author(5), key, "wait for content", 123)
        .unwrap();
    drop(inbox);

    let reopened = InboundInbox::open(dir.path()).unwrap();
    let ready = reopened.pending_ready(123).unwrap();

    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].author, author(5));
    assert_eq!(ready[0].key, key);
    assert_eq!(ready[0].state, InboxState::Retryable);
    assert_eq!(ready[0].attempts, 1);
    assert!(dir.path().join("sync-state.sqlite").is_file());
    assert!(!dir.path().join("inbound_inbox.json").exists());
}

#[test]
fn retryable_entry_waits_until_retry_after() {
    let (_dir, inbox) = temp_inbox();
    let key = b"k";
    inbox
        .record_remote_insert(author(2), key, iroh_blobs::Hash::new(b"body"))
        .unwrap();
    inbox
        .mark_retryable(author(2), key, "blob missing", 1_000)
        .unwrap();

    assert!(inbox.pending_ready(999).unwrap().is_empty());
    let ready = inbox.pending_ready(1_000).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].state, InboxState::Retryable);
    assert_eq!(ready[0].attempts, 1);
    assert_eq!(ready[0].last_error.as_deref(), Some("blob missing"));
}

#[test]
fn obsolete_source_entry_is_removed_instead_of_remaining_pending() {
    let (_dir, inbox) = temp_inbox();
    let key = b"obsolete";
    inbox
        .record_remote_insert(author(2), key, iroh_blobs::Hash::new(b"body"))
        .unwrap();

    assert!(inbox.discard(author(2), key).unwrap());
    assert!(!inbox.discard(author(2), key).unwrap());
    assert!(inbox.pending_ready(u64::MAX).unwrap().is_empty());
    assert_eq!(inbox.diagnostics(10).unwrap().counts, Default::default());
}

#[test]
fn discarded_source_content_stays_terminal_until_the_content_hash_changes() {
    let (_dir, inbox) = temp_inbox();
    let key = b"discarded";
    let missing_hash = iroh_blobs::Hash::new(b"missing");
    inbox
        .record_remote_insert(author(3), key, missing_hash)
        .unwrap();
    inbox
        .mark_discarded(author(3), key, "all known sources lack the blob")
        .unwrap();

    assert!(!inbox
        .record_remote_insert_for_repair(author(3), key, missing_hash)
        .unwrap());
    let diagnostics = inbox.diagnostics(10).unwrap();
    assert_eq!(diagnostics.counts.pending, 0);
    assert_eq!(diagnostics.counts.retryable, 0);
    assert_eq!(diagnostics.counts.discarded, 1);

    assert!(inbox
        .record_remote_insert_for_repair(author(3), key, iroh_blobs::Hash::new(b"replacement"),)
        .unwrap());
    assert_eq!(inbox.count_by_state(InboxState::Discarded).unwrap(), 0);
    assert_eq!(inbox.count_by_state(InboxState::Pending).unwrap(), 1);
}

#[test]
fn retry_after_uses_bounded_exponential_backoff() {
    assert_eq!(next_retry_after_ms(0, 10_000), 11_000);
    assert_eq!(next_retry_after_ms(1, 10_000), 12_000);
    assert_eq!(next_retry_after_ms(5, 10_000), 42_000);
    assert_eq!(next_retry_after_ms(30, 10_000), 3_610_000);
}

#[test]
fn limited_ready_entries_prioritize_new_pending_over_retryable_backlog() {
    let (_dir, inbox) = temp_inbox();
    inbox
        .record_remote_insert(author(1), b"retry", iroh_blobs::Hash::new(b"retry"))
        .unwrap();
    inbox
        .mark_retryable(author(1), b"retry", "old missing blob", 0)
        .unwrap();
    inbox
        .record_remote_insert(author(2), b"pending", iroh_blobs::Hash::new(b"pending"))
        .unwrap();

    let ready = inbox.pending_ready_limited(u64::MAX, 1).unwrap();

    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].state, InboxState::Pending);
    assert_eq!(ready[0].key, b"pending");
}

#[test]
fn stale_retryable_backlog_can_be_snoozed_without_blocking_new_pending() {
    let (_dir, inbox) = temp_inbox();
    inbox
        .record_remote_insert(author(1), b"retry", iroh_blobs::Hash::new(b"retry"))
        .unwrap();
    for _ in 0..12 {
        inbox
            .mark_retryable(author(1), b"retry", "old missing blob", 0)
            .unwrap();
    }
    inbox
        .record_remote_insert(author(2), b"pending", iroh_blobs::Hash::new(b"pending"))
        .unwrap();

    let snoozed = inbox
        .snooze_retryable_backlog(10_000, 12, 3_600_000, 10)
        .unwrap();
    let ready = inbox.pending_ready_limited(10_000, 10).unwrap();

    assert_eq!(snoozed, 1);
    assert_eq!(inbox.count_by_state(InboxState::Retryable).unwrap(), 1);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].state, InboxState::Pending);
    assert_eq!(ready[0].key, b"pending");
}

#[test]
fn stale_unreadable_retryable_backlog_can_be_marked_dormant() {
    let (_dir, inbox) = temp_inbox();
    inbox
        .record_remote_insert(author(1), b"retry", iroh_blobs::Hash::new(b"retry"))
        .unwrap();
    for _ in 0..24 {
        inbox
            .mark_retryable(author(1), b"retry", "entry content not readable yet", 0)
            .unwrap();
    }

    let dormant = inbox
        .mark_dormant_retryable_backlog(10_000, 24, 10)
        .unwrap();
    let diagnostics = inbox.diagnostics(10).unwrap();

    assert_eq!(dormant, 1);
    assert_eq!(inbox.count_by_state(InboxState::Retryable).unwrap(), 0);
    assert_eq!(inbox.count_by_state(InboxState::Dormant).unwrap(), 1);
    assert_eq!(diagnostics.counts.dormant, 1);
    assert!(inbox.pending_ready(u64::MAX).unwrap().is_empty());
}

#[test]
fn stale_missing_encrypted_blob_backlog_can_be_marked_dormant() {
    let (_dir, inbox) = temp_inbox();
    inbox
        .record_remote_insert(author(1), b"retry", iroh_blobs::Hash::new(b"retry"))
        .unwrap();
    let error = "v4 apply failed: store: download encrypted blob \
                 68deaaef5a1b9c5f90266ecc9c554915d1f998bf9b5438a1dc96345c34e9dcad: \
                 Unable to download; local status not_found -> not_found";
    for _ in 0..24 {
        inbox.mark_retryable(author(1), b"retry", error, 0).unwrap();
    }

    let dormant = inbox
        .mark_dormant_retryable_backlog(10_000, 24, 10)
        .unwrap();

    assert_eq!(dormant, 1);
    assert_eq!(inbox.count_by_state(InboxState::Retryable).unwrap(), 0);
    assert_eq!(inbox.count_by_state(InboxState::Dormant).unwrap(), 1);
}

#[test]
fn self_authored_entries_can_be_marked_applied_without_reading_content() {
    let (_dir, inbox) = temp_inbox();
    let self_author = author(1);
    inbox
        .record_remote_insert(self_author, b"pending", iroh_blobs::Hash::new(b"pending"))
        .unwrap();
    inbox
        .record_remote_insert(self_author, b"retry", iroh_blobs::Hash::new(b"retry"))
        .unwrap();
    inbox
        .mark_retryable(self_author, b"retry", "entry content not readable yet", 0)
        .unwrap();
    inbox
        .record_remote_insert(self_author, b"dormant", iroh_blobs::Hash::new(b"dormant"))
        .unwrap();
    for _ in 0..24 {
        inbox
            .mark_retryable(self_author, b"dormant", "entry content not readable yet", 0)
            .unwrap();
    }
    inbox
        .mark_dormant_retryable_backlog(10_000, 24, 10)
        .unwrap();
    inbox
        .record_remote_insert(author(2), b"remote", iroh_blobs::Hash::new(b"remote"))
        .unwrap();

    let applied = inbox.mark_author_applied(self_author).unwrap();

    assert_eq!(applied, 3);
    assert_eq!(inbox.count_by_state(InboxState::Applied).unwrap(), 3);
    assert_eq!(inbox.count_by_state(InboxState::Dormant).unwrap(), 0);
    let ready = inbox.pending_ready(u64::MAX).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].author, author(2));
    assert_eq!(ready[0].key, b"remote");
}

#[test]
fn apply_failures_are_not_marked_dormant() {
    let (_dir, inbox) = temp_inbox();
    inbox
        .record_remote_insert(author(1), b"retry", iroh_blobs::Hash::new(b"retry"))
        .unwrap();
    for _ in 0..24 {
        inbox
            .mark_retryable(author(1), b"retry", "v4 apply failed: bad domain", 0)
            .unwrap();
    }

    let dormant = inbox
        .mark_dormant_retryable_backlog(10_000, 24, 10)
        .unwrap();

    assert_eq!(dormant, 0);
    assert_eq!(inbox.count_by_state(InboxState::Retryable).unwrap(), 1);
    assert_eq!(inbox.count_by_state(InboxState::Dormant).unwrap(), 0);
}

#[test]
fn applied_entries_are_not_returned_as_pending() {
    let (_dir, inbox) = temp_inbox();
    let key = b"k";
    inbox
        .record_remote_insert(author(3), key, iroh_blobs::Hash::new(b"body"))
        .unwrap();
    inbox.mark_applied(author(3), key).unwrap();

    assert_eq!(inbox.count_by_state(InboxState::Applied).unwrap(), 1);
    assert!(inbox.pending_ready(u64::MAX).unwrap().is_empty());
}

#[test]
fn failed_entries_are_not_retried() {
    let (_dir, inbox) = temp_inbox();
    let key = b"k";
    inbox
        .record_remote_insert(author(4), key, iroh_blobs::Hash::new(b"body"))
        .unwrap();
    inbox.mark_failed(author(4), key, "bad envelope").unwrap();

    assert_eq!(inbox.count_by_state(InboxState::Failed).unwrap(), 1);
    assert!(inbox.pending_ready(u64::MAX).unwrap().is_empty());
}

#[test]
fn retryable_entries_stop_after_three_total_attempts() {
    use rua_sync::inbound_inbox::RetryExhaustion;

    let (_dir, inbox) = temp_inbox();
    let key = b"limited";
    let author = author(6);
    inbox
        .record_remote_insert(author, key, iroh_blobs::Hash::new(b"body"))
        .unwrap();

    for expected in [InboxState::Retryable, InboxState::Retryable] {
        assert_eq!(
            inbox
                .mark_retryable_limited(
                    author,
                    key,
                    "temporary failure",
                    0,
                    3,
                    RetryExhaustion::Failed,
                )
                .unwrap(),
            expected
        );
    }
    assert_eq!(
        inbox
            .mark_retryable_limited(author, key, "still failing", 0, 3, RetryExhaustion::Failed,)
            .unwrap(),
        InboxState::Failed
    );
    assert!(inbox.pending_ready(u64::MAX).unwrap().is_empty());
}

#[test]
fn duplicate_remote_insert_does_not_revive_dormant_content() {
    use rua_sync::inbound_inbox::RetryExhaustion;

    let (_dir, inbox) = temp_inbox();
    let author = author(8);
    let key = b"missing-content";
    let hash = iroh_blobs::Hash::new(b"missing-content");
    inbox.record_remote_insert(author, key, hash).unwrap();
    for _ in 0..3 {
        inbox
            .mark_retryable_limited(
                author,
                key,
                "content unavailable",
                0,
                3,
                RetryExhaustion::Dormant,
            )
            .unwrap();
    }

    inbox.record_remote_insert(author, key, hash).unwrap();

    assert_eq!(inbox.count_by_state(InboxState::Dormant).unwrap(), 1);
    assert!(inbox.pending_ready(u64::MAX).unwrap().is_empty());
    let diagnostics = inbox.diagnostics(10).unwrap();
    assert_eq!(diagnostics.recent_errors[0].attempts, 3);
}

#[test]
fn exhausted_content_retry_becomes_dormant_and_manual_retry_only_revives_failed() {
    use rua_sync::inbound_inbox::RetryExhaustion;

    let (_dir, inbox) = temp_inbox();
    let author = author(7);
    for key in [b"content".as_slice(), b"failed"] {
        inbox
            .record_remote_insert(author, key, iroh_blobs::Hash::new(key))
            .unwrap();
    }
    for _ in 0..3 {
        inbox
            .mark_retryable_limited(
                author,
                b"content",
                "content unavailable",
                0,
                3,
                RetryExhaustion::Dormant,
            )
            .unwrap();
        inbox
            .mark_retryable_limited(
                author,
                b"failed",
                "apply failed",
                0,
                3,
                RetryExhaustion::Failed,
            )
            .unwrap();
    }

    assert_eq!(inbox.retry_failed().unwrap(), 1);
    assert_eq!(inbox.count_by_state(InboxState::Pending).unwrap(), 1);
    assert_eq!(inbox.count_by_state(InboxState::Dormant).unwrap(), 1);
    let pending = inbox.pending_ready(u64::MAX).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].key, b"failed");
    assert_eq!(pending[0].attempts, 0);
}

#[test]
fn terminal_entries_ignore_late_worker_updates() {
    let (_dir, inbox) = temp_inbox();
    let applied_key = b"applied";
    let failed_key = b"failed";
    let discarded_key = b"discarded";

    for key in [applied_key.as_slice(), failed_key, discarded_key] {
        inbox
            .record_remote_insert(author(5), key, iroh_blobs::Hash::new(key))
            .unwrap();
    }

    inbox.mark_applied(author(5), applied_key).unwrap();
    inbox
        .mark_failed(author(5), failed_key, "invalid envelope")
        .unwrap();
    inbox
        .mark_discarded(author(5), discarded_key, "obsolete entry")
        .unwrap();

    inbox
        .mark_retryable(author(5), applied_key, "late retry", 0)
        .unwrap();
    inbox
        .mark_discarded(author(5), failed_key, "late discard")
        .unwrap();
    inbox.mark_applied(author(5), discarded_key).unwrap();

    assert_eq!(inbox.count_by_state(InboxState::Applied).unwrap(), 1);
    assert_eq!(inbox.count_by_state(InboxState::Failed).unwrap(), 1);
    assert_eq!(inbox.count_by_state(InboxState::Discarded).unwrap(), 1);
    assert_eq!(inbox.count_by_state(InboxState::Retryable).unwrap(), 0);
}

#[test]
fn diagnostics_counts_states_and_recent_errors() {
    let (_dir, inbox) = temp_inbox();
    inbox
        .record_remote_insert(author(1), b"pending", iroh_blobs::Hash::new(b"pending"))
        .unwrap();
    inbox
        .record_remote_insert(author(2), b"retry", iroh_blobs::Hash::new(b"retry"))
        .unwrap();
    inbox
        .mark_retryable(author(2), b"retry", "blob missing", 1_000)
        .unwrap();
    inbox
        .record_remote_insert(author(3), b"failed", iroh_blobs::Hash::new(b"failed"))
        .unwrap();
    inbox
        .mark_failed(author(3), b"failed", "bad envelope")
        .unwrap();

    let diagnostics = inbox.diagnostics(1).unwrap();

    assert_eq!(diagnostics.counts.pending, 1);
    assert_eq!(diagnostics.counts.retryable, 1);
    assert_eq!(diagnostics.counts.dormant, 0);
    assert_eq!(diagnostics.counts.failed, 1);
    assert_eq!(diagnostics.recent_errors.len(), 1);
    assert!(
        diagnostics.recent_errors[0].last_error == "blob missing"
            || diagnostics.recent_errors[0].last_error == "bad envelope"
    );
}

#[tokio::test]
async fn repair_scan_enqueues_doc_entries_without_resetting_applied() {
    let (inbox_dir, inbox) = temp_inbox();
    let node_dir = tempfile::tempdir().unwrap();
    let device = DeviceIdentity::generate();
    let network = NetworkSecret::generate();
    let node = SyncNode::bind(node_dir.path(), network.namespace_secret(), &device, None)
        .await
        .unwrap();
    let author = device.to_iroh_author();
    let key = b"opaque-repair-key";
    let hash = node
        .doc()
        .set_bytes(author.id(), key.to_vec(), b"ciphertext".to_vec())
        .await
        .unwrap();

    assert!(inbox.needs_initial_repair_scan().unwrap());
    let first = inbox.repair_scan_doc_entries(node.doc()).await.unwrap();
    assert_eq!(first.scanned, 1);
    assert_eq!(first.recorded, 1);
    assert_eq!(first.already_applied, 0);
    assert_eq!(inbox.count_by_state(InboxState::Pending).unwrap(), 1);
    assert!(!inbox.needs_initial_repair_scan().unwrap());
    let last_successful = inbox.last_successful_repair_scan_ms().unwrap().unwrap();
    assert!(!inbox
        .repair_scan_due(last_successful + DAILY_REPAIR_SCAN_INTERVAL_MS - 1)
        .unwrap());
    assert!(inbox
        .repair_scan_due(last_successful + DAILY_REPAIR_SCAN_INTERVAL_MS)
        .unwrap());
    let diagnostics = inbox.diagnostics(10).unwrap().repair_scan;
    assert_eq!(diagnostics.last_successful_at_ms, Some(last_successful));
    assert_eq!(diagnostics.last_scanned, 1);
    assert_eq!(diagnostics.last_recorded, 1);
    assert_eq!(diagnostics.last_errors, 0);
    assert!(diagnostics.last_duration_ms.is_some());

    inbox.mark_applied(author.id(), key).unwrap();
    let second = inbox.repair_scan_doc_entries(node.doc()).await.unwrap();
    assert_eq!(second.scanned, 1);
    assert_eq!(second.recorded, 0);
    assert_eq!(second.already_applied, 1);
    assert_eq!(hash, iroh_blobs::Hash::new(b"ciphertext"));

    drop(inbox);
    let reopened = InboundInbox::open(inbox_dir.path()).unwrap();
    assert!(!reopened.needs_initial_repair_scan().unwrap());
}

#[tokio::test]
async fn repair_scan_observes_real_doc_deletion_markers() {
    let (_dir, inbox) = temp_inbox();
    let node_dir = tempfile::tempdir().unwrap();
    let device = DeviceIdentity::generate();
    let network = NetworkSecret::generate();
    let node = SyncNode::bind(node_dir.path(), network.namespace_secret(), &device, None)
        .await
        .unwrap();
    let author = device.to_iroh_author();
    let key = b"deleted-replica-entry";
    let hash = node
        .doc()
        .set_bytes(author.id(), key.to_vec(), b"ciphertext".to_vec())
        .await
        .unwrap();
    inbox.record_remote_insert(author.id(), key, hash).unwrap();
    node.doc().del(author.id(), key.to_vec()).await.unwrap();
    let report = inbox.repair_scan_doc_entries(node.doc()).await.unwrap();
    assert_eq!(report.scanned, 1);
    assert_eq!(inbox.count_by_state(InboxState::Discarded).unwrap(), 1);
    assert!(inbox.pending_ready(u64::MAX).unwrap().is_empty());
    node.shutdown().await;
}

#[tokio::test]
async fn repair_scan_preserves_retryable_entries() {
    let (_inbox_dir, inbox) = temp_inbox();
    let node_dir = tempfile::tempdir().unwrap();
    let device = DeviceIdentity::generate();
    let network = NetworkSecret::generate();
    let node = SyncNode::bind(node_dir.path(), network.namespace_secret(), &device, None)
        .await
        .unwrap();
    let author = device.to_iroh_author();
    let key = b"opaque-retry-key";
    node.doc()
        .set_bytes(author.id(), key.to_vec(), b"ciphertext".to_vec())
        .await
        .unwrap();

    let first = inbox.repair_scan_doc_entries(node.doc()).await.unwrap();
    assert_eq!(first.recorded, 1);
    inbox
        .mark_retryable(author.id(), key, "blob not ready", 10_000)
        .unwrap();

    let second = inbox.repair_scan_doc_entries(node.doc()).await.unwrap();
    assert_eq!(second.scanned, 1);
    assert_eq!(second.recorded, 0);
    assert_eq!(second.already_applied, 1);
    assert_eq!(inbox.count_by_state(InboxState::Retryable).unwrap(), 1);
    assert!(
        inbox.pending_ready(9_999).unwrap().is_empty(),
        "repair scan must not reset retry_after_ms"
    );
    let ready = inbox.pending_ready(10_000).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].last_error.as_deref(), Some("blob not ready"));
}

#[tokio::test]
async fn repair_scan_requeues_dormant_entries_for_another_read_attempt() {
    let (_inbox_dir, inbox) = temp_inbox();
    let node_dir = tempfile::tempdir().unwrap();
    let device = DeviceIdentity::generate();
    let network = NetworkSecret::generate();
    let node = SyncNode::bind(node_dir.path(), network.namespace_secret(), &device, None)
        .await
        .unwrap();
    let author = device.to_iroh_author();
    let key = b"opaque-dormant-key";
    node.doc()
        .set_bytes(author.id(), key.to_vec(), b"ciphertext".to_vec())
        .await
        .unwrap();

    let first = inbox.repair_scan_doc_entries(node.doc()).await.unwrap();
    assert_eq!(first.recorded, 1);
    for _ in 0..24 {
        inbox
            .mark_retryable(author.id(), key, "entry content not readable yet", 0)
            .unwrap();
    }
    inbox
        .mark_dormant_retryable_backlog(10_000, 24, 10)
        .unwrap();
    assert_eq!(inbox.count_by_state(InboxState::Dormant).unwrap(), 1);

    let second = inbox.repair_scan_doc_entries(node.doc()).await.unwrap();

    assert_eq!(second.scanned, 1);
    assert_eq!(second.recorded, 1);
    assert_eq!(second.already_applied, 0);
    assert_eq!(inbox.count_by_state(InboxState::Dormant).unwrap(), 0);
    let ready = inbox.pending_ready(u64::MAX).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].state, InboxState::Pending);
    assert_eq!(ready[0].attempts, 0);
    assert_eq!(ready[0].last_error.as_deref(), None);
}

#[test]
fn content_ready_reactivates_only_matching_dormant_blob() {
    use rua_sync::inbound_inbox::RetryExhaustion;

    let (_dir, inbox) = temp_inbox();
    let author = author(10);
    let ready_hash = iroh_blobs::Hash::new(b"ready");
    let missing_hash = iroh_blobs::Hash::new(b"missing");
    for (key, hash) in [
        (b"ready".as_slice(), ready_hash),
        (b"missing".as_slice(), missing_hash),
    ] {
        inbox.record_remote_insert(author, key, hash).unwrap();
        for _ in 0..3 {
            inbox
                .mark_retryable_limited(
                    author,
                    key,
                    "content unavailable",
                    0,
                    3,
                    RetryExhaustion::Dormant,
                )
                .unwrap();
        }
    }

    assert_eq!(
        inbox
            .reactivate_dormant_content(ready_hash, 10_000)
            .unwrap(),
        1
    );

    let ready = inbox.pending_ready(10_000).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].key, b"ready");
    assert_eq!(ready[0].attempts, 0);
    assert_eq!(inbox.count_by_state(InboxState::Dormant).unwrap(), 1);
    assert_eq!(
        inbox
            .reactivate_dormant_content(ready_hash, 10_001)
            .unwrap(),
        0
    );
}
