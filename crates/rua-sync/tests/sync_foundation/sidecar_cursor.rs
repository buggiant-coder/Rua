use rua_sync::hlc::HlcTimestamp;
use rua_sync::sidecar::{RecordState, SidecarEntry, SidecarStore};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn temp_store() -> (tempfile::TempDir, SidecarStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = SidecarStore::open(dir.path()).unwrap();
    (dir, store)
}

#[test]
fn get_nonexistent_returns_none() {
    let (_dir, store) = temp_store();
    assert!(store.get_cursor("missing").unwrap().is_none());
}

#[test]
fn set_and_get_roundtrip() {
    let (_dir, store) = temp_store();
    let entry = SidecarEntry {
        hlc: hlc(100),
        state: RecordState::Live,
        base_blob_hash: Some("abc123".to_string()),
    };
    store.set_cursor("rec-1", &entry).unwrap();
    let got = store.get_cursor("rec-1").unwrap().unwrap();
    assert_eq!(got.hlc.wall_ms, 100);
    assert_eq!(got.state, RecordState::Live);
    assert_eq!(got.base_blob_hash.as_deref(), Some("abc123"));
}

#[test]
fn set_overwrites_existing() {
    let (_dir, store) = temp_store();
    let e1 = SidecarEntry {
        hlc: hlc(1),
        state: RecordState::Live,
        base_blob_hash: None,
    };
    store.set_cursor("r", &e1).unwrap();

    let e2 = SidecarEntry {
        hlc: hlc(99),
        state: RecordState::Live,
        base_blob_hash: Some("new".to_string()),
    };
    store.set_cursor("r", &e2).unwrap();

    let got = store.get_cursor("r").unwrap().unwrap();
    assert_eq!(got.hlc.wall_ms, 99);
    assert_eq!(got.base_blob_hash.as_deref(), Some("new"));
}

#[test]
fn mark_tombstone_live_to_tombstoned() {
    let (_dir, store) = temp_store();
    let entry = SidecarEntry {
        hlc: hlc(10),
        state: RecordState::Live,
        base_blob_hash: None,
    };
    store.set_cursor("r", &entry).unwrap();

    let changed = store.mark_tombstone("r", hlc(20)).unwrap();
    assert!(changed);

    let got = store.get_cursor("r").unwrap().unwrap();
    assert_eq!(got.state, RecordState::Tombstoned);
    assert_eq!(got.hlc.wall_ms, 20);
}

#[test]
fn mark_tombstone_already_tombstoned_is_noop() {
    let (_dir, store) = temp_store();
    let entry = SidecarEntry {
        hlc: hlc(10),
        state: RecordState::Tombstoned,
        base_blob_hash: None,
    };
    store.set_cursor("r", &entry).unwrap();

    let changed = store.mark_tombstone("r", hlc(20)).unwrap();
    assert!(!changed);
    let got = store.get_cursor("r").unwrap().unwrap();
    assert_eq!(got.hlc.wall_ms, 10);
}

#[test]
fn mark_tombstone_nonexistent_is_noop() {
    let (_dir, store) = temp_store();
    let changed = store.mark_tombstone("missing", hlc(1)).unwrap();
    assert!(!changed);
}

#[test]
fn multiple_records_independent() {
    let (_dir, store) = temp_store();
    for i in 0..5 {
        let entry = SidecarEntry {
            hlc: hlc(i * 10),
            state: RecordState::Live,
            base_blob_hash: None,
        };
        store.set_cursor(&format!("r-{i}"), &entry).unwrap();
    }
    for i in 0..5 {
        let got = store.get_cursor(&format!("r-{i}")).unwrap().unwrap();
        assert_eq!(got.hlc.wall_ms, i * 10);
    }
}
