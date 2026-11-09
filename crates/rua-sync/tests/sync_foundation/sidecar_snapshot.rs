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
fn put_and_get_roundtrip() {
    let (_dir, store) = temp_store();
    let content = b"hello snapshot world";
    let hash = store.put_base_snapshot(content).unwrap();
    assert!(!hash.is_empty());

    let got = store.get_base_snapshot(&hash).unwrap().unwrap();
    assert_eq!(got, content);
}

#[test]
fn get_missing_returns_none() {
    let (_dir, store) = temp_store();
    let got = store
        .get_base_snapshot("0000000000000000000000000000000000000000000000000000000000000000")
        .unwrap();
    assert!(got.is_none());
}

#[test]
fn same_content_same_hash() {
    let (_dir, store) = temp_store();
    let h1 = store.put_base_snapshot(b"identical").unwrap();
    let h2 = store.put_base_snapshot(b"identical").unwrap();
    assert_eq!(h1, h2);
}

#[test]
fn different_content_different_hash() {
    let (_dir, store) = temp_store();
    let h1 = store.put_base_snapshot(b"aaa").unwrap();
    let h2 = store.put_base_snapshot(b"bbb").unwrap();
    assert_ne!(h1, h2);
}

#[test]
fn cursor_with_base_blob_hash_roundtrip() {
    let (_dir, store) = temp_store();

    let hash = store.put_base_snapshot(b"base content").unwrap();

    let entry = SidecarEntry {
        hlc: hlc(42),
        state: RecordState::Live,
        base_blob_hash: Some(hash.clone()),
    };
    store.set_cursor("prompt-1", &entry).unwrap();

    let got = store.get_cursor("prompt-1").unwrap().unwrap();
    assert_eq!(got.base_blob_hash.as_deref(), Some(hash.as_str()));

    let snap = store.get_base_snapshot(&hash).unwrap().unwrap();
    assert_eq!(snap, b"base content");
}

#[test]
fn empty_content_snapshot() {
    let (_dir, store) = temp_store();
    let hash = store.put_base_snapshot(b"").unwrap();
    let got = store.get_base_snapshot(&hash).unwrap().unwrap();
    assert!(got.is_empty());
}

#[test]
fn large_snapshot() {
    let (_dir, store) = temp_store();
    let content = vec![0xABu8; 1024 * 1024];
    let hash = store.put_base_snapshot(&content).unwrap();
    let got = store.get_base_snapshot(&hash).unwrap().unwrap();
    assert_eq!(got.len(), 1024 * 1024);
    assert_eq!(got, content);
}

#[test]
fn put_recreates_missing_snapshots_dir() {
    let (dir, store) = temp_store();
    std::fs::remove_dir_all(dir.path().join("snapshots")).unwrap();

    let hash = store.put_base_snapshot(b"after directory removal").unwrap();
    let got = store.get_base_snapshot(&hash).unwrap().unwrap();

    assert_eq!(got, b"after directory removal");
}
