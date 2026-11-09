use iroh_docs::AuthorId;
use rua_sync::hlc::HlcTimestamp;
use rua_sync::inbound_inbox::{InboundInbox, InboxState};
use rua_sync::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use rua_sync::sidecar::{ApplyCommit, ConflictEntry, RecordState};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn temp_store() -> (tempfile::TempDir, MergeStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = MergeStore::open(dir.path()).unwrap();
    (dir, store)
}

fn author(byte: u8) -> AuthorId {
    AuthorId::from(&[byte; 32])
}

#[test]
fn applied_ops_are_idempotent() {
    let (_dir, store) = temp_store();

    assert!(!store.has_applied_op("op-1").unwrap());
    store.mark_applied_op("op-1").unwrap();
    store.mark_applied_op("op-1").unwrap();

    assert!(store.has_applied_op("op-1").unwrap());
    assert!(!store.has_applied_op("op-2").unwrap());
}

#[test]
fn commit_apply_marks_op_and_advances_cursor_atomically() {
    let (_dir, store) = temp_store();
    let entry = live_cursor(hlc(100), Some("base-1".to_string()));

    let committed = store.commit_apply("op-1", "record-1", &entry).unwrap();

    assert!(committed);
    assert!(store.has_applied_op("op-1").unwrap());
    let cursor = store.get_record_cursor("record-1").unwrap().unwrap();
    assert_eq!(cursor.hlc.wall_ms, 100);
    assert_eq!(cursor.state, RecordState::Live);
    assert_eq!(cursor.base_blob_hash.as_deref(), Some("base-1"));
}

#[test]
fn sidecar_reopens_with_durable_sqlite_state() {
    let (dir, store) = temp_store();
    let entry = live_cursor(hlc(100), Some("base-1".to_string()));
    assert!(store.commit_apply("op-1", "record-1", &entry).unwrap());
    drop(store);

    let reopened = MergeStore::open(dir.path()).unwrap();

    assert!(reopened.has_applied_op("op-1").unwrap());
    assert_eq!(reopened.get_record_cursor("record-1").unwrap(), Some(entry));
    assert!(dir.path().join("sync-state.sqlite").is_file());
    assert!(!dir.path().join("applied_ops.json").exists());
    assert!(!dir.path().join("cursors.json").exists());
}

#[test]
fn inbound_apply_commit_updates_sidecar_and_inbox_together() {
    let dir = tempfile::tempdir().unwrap();
    let inbox = InboundInbox::open(dir.path()).unwrap();
    let store = MergeStore::open(dir.path()).unwrap();
    let author = author(7);
    let key = b"inbound-entry";
    inbox
        .record_remote_insert(author, key, iroh_blobs::Hash::new(b"ciphertext"))
        .unwrap();
    let cursor = live_cursor(hlc(200), Some("base-2".to_string()));
    let commit = ApplyCommit::with_cursor("op-2", "prompts:record-2", cursor.clone());

    let committed = store
        .commit_inbound_apply(author, key, 123, &[commit])
        .unwrap();

    assert_eq!(committed, 1);
    assert!(store.has_applied_op("op-2").unwrap());
    assert_eq!(
        store.get_record_cursor("prompts:record-2").unwrap(),
        Some(cursor)
    );
    assert_eq!(inbox.count_by_state(InboxState::Applied).unwrap(), 1);
    assert!(inbox.pending_ready(u64::MAX).unwrap().is_empty());
}

#[test]
fn missing_inbox_row_rolls_back_sidecar_commit() {
    let dir = tempfile::tempdir().unwrap();
    let store = MergeStore::open(dir.path()).unwrap();
    let cursor = live_cursor(hlc(300), None);
    let commit = ApplyCommit::with_cursor("op-3", "prompts:record-3", cursor);

    let error = store
        .commit_inbound_apply(author(8), b"missing", 456, &[commit])
        .unwrap_err();

    assert!(error.to_string().contains("inbound inbox entry missing"));
    assert!(!store.has_applied_op("op-3").unwrap());
    assert!(store
        .get_record_cursor("prompts:record-3")
        .unwrap()
        .is_none());
}

#[test]
fn duplicate_commit_does_not_move_cursor() {
    let (_dir, store) = temp_store();
    let first = live_cursor(hlc(100), None);
    let second = live_cursor(hlc(200), None);

    assert!(store.commit_apply("op-1", "record-1", &first).unwrap());
    assert!(!store.commit_apply("op-1", "record-1", &second).unwrap());

    let cursor = store.get_record_cursor("record-1").unwrap().unwrap();
    assert_eq!(cursor.hlc.wall_ms, 100);
}

#[test]
fn tombstone_cursor_roundtrips() {
    let (_dir, store) = temp_store();
    let entry = tombstone_cursor(hlc(300), None);

    assert!(store.commit_apply("delete-op", "record-1", &entry).unwrap());

    let cursor = store.get_record_cursor("record-1").unwrap().unwrap();
    assert_eq!(cursor.state, RecordState::Tombstoned);
    assert_eq!(cursor.hlc.wall_ms, 300);
}

#[test]
fn diagnostics_counts_applied_ops_and_cursors_by_domain() {
    let (_dir, store) = temp_store();
    let live = live_cursor(hlc(100), None);
    let tombstone = tombstone_cursor(hlc(200), None);

    assert!(store
        .commit_apply("op-prompt", "prompts:a.md", &live)
        .unwrap());
    assert!(store
        .commit_apply("op-chat", "chat:agent/chats/chat-a", &tombstone)
        .unwrap());
    store.mark_applied_op("op-skipped").unwrap();

    let diagnostics = store.diagnostics().unwrap();

    assert_eq!(diagnostics.applied_ops_total, 3);
    assert_eq!(diagnostics.applied_ops_by_domain["prompts"], 1);
    assert_eq!(diagnostics.applied_ops_by_domain["chat"], 1);
    assert_eq!(diagnostics.applied_ops_by_domain["unknown"], 1);
    assert_eq!(diagnostics.cursors_by_domain["prompts"].live, 1);
    assert_eq!(diagnostics.cursors_by_domain["chat"].tombstoned, 1);
}

#[test]
fn base_snapshot_roundtrips() {
    let (_dir, store) = temp_store();

    let hash = store.put_base_snapshot(b"base content").unwrap();
    let cursor = live_cursor(hlc(10), Some(hash.clone()));
    assert!(store.commit_apply("op-1", "record-1", &cursor).unwrap());

    let got = store.get_base_snapshot(&hash).unwrap().unwrap();
    assert_eq!(got, b"base content");

    let got_cursor = store.get_record_cursor("record-1").unwrap().unwrap();
    assert_eq!(got_cursor.base_blob_hash.as_deref(), Some(hash.as_str()));
}

#[test]
fn pending_conflict_does_not_write_live_file() {
    let (dir, store) = temp_store();
    let live_path = dir.path().join("live.json");
    let live_path_str = live_path.to_string_lossy().into_owned();

    let id = store
        .record_conflict(ConflictEntry {
            id: "conflict-1".to_string(),
            live_path: live_path_str,
            record_id: "record-1".to_string(),
            domain: "settings".to_string(),
            winner_text: "{\"value\":1}".to_string(),
            loser_text: "{\"value\":2}".to_string(),
            winner_hlc: hlc(20),
            created_at_ms: 123,
        })
        .unwrap();

    assert_eq!(id, "conflict-1");
    assert!(!live_path.exists());

    let conflicts = store.list_pending_conflicts().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].record_id, "record-1");
}
