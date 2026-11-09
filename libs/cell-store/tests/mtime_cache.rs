use std::time::Duration;

use cell_store::{CellStore, FileSystemCellStore};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn cache_populated_on_first_read() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("seed.json");

    tokio::fs::write(&file, br#"{"answer": 42}"#).await.unwrap();

    let store = FileSystemCellStore::new();
    assert_eq!(store.cache_len(), 0, "no cache before first read");

    let v: Option<u32> = store.read(&file, "$.answer").await.unwrap();
    assert_eq!(v, Some(42));
    assert_eq!(store.cache_len(), 1, "cache populated after first read");
}

#[tokio::test]
async fn cache_hit_when_mtime_unchanged_avoids_disk_read() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("steady.json");
    let store = FileSystemCellStore::new();

    store
        .write(&file, "$", json!({"hits": "served-from-cache"}))
        .await
        .unwrap();

    for _ in 0..5 {
        let v: Option<String> = store.read(&file, "$.hits").await.unwrap();
        assert_eq!(v.as_deref(), Some("served-from-cache"));
    }
    assert_eq!(
        store.cache_len(),
        1,
        "no new cache entries on repeated reads"
    );
}

#[tokio::test]
async fn cache_invalidated_when_external_process_modifies_file() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("ext.json");
    let store = FileSystemCellStore::new();

    store
        .write(&file, "$", json!({"version": "v1"}))
        .await
        .unwrap();
    let v1: Option<String> = store.read(&file, "$.version").await.unwrap();
    assert_eq!(v1.as_deref(), Some("v1"));

    tokio::time::sleep(Duration::from_millis(1100)).await;
    tokio::fs::write(&file, br#"{"version": "v2-from-outside"}"#)
        .await
        .unwrap();

    let v2: Option<String> = store.read(&file, "$.version").await.unwrap();
    assert_eq!(
        v2.as_deref(),
        Some("v2-from-outside"),
        "store must re-read when file mtime changes"
    );
}

#[tokio::test]
async fn cache_invalidated_when_mtime_increases() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("mtime.json");
    let store = FileSystemCellStore::new();

    store.write(&file, "$.n", json!(1)).await.unwrap();
    let first: Option<i32> = store.read(&file, "$.n").await.unwrap();
    assert_eq!(first, Some(1));

    tokio::time::sleep(Duration::from_millis(1100)).await;

    store.write(&file, "$.n", json!(2)).await.unwrap();

    let second: Option<i32> = store.read(&file, "$.n").await.unwrap();
    assert_eq!(second, Some(2), "cache must refresh after mtime bump");
}
