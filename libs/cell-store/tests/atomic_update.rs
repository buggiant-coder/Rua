use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cell_store::{CellStore, FileSystemCellStore};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn update_applies_mutator_and_persists() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("u.json");
    let store = FileSystemCellStore::new();
    store.write(&file, "$.n", 10_u32).await.unwrap();

    let after = store
        .update(&file, "$.n", |current: Option<u32>| {
            current.unwrap_or(0) + 5
        })
        .await
        .unwrap();
    assert_eq!(after, 15);

    let on_disk: Option<u32> = store.read(&file, "$.n").await.unwrap();
    assert_eq!(on_disk, Some(15));
}

#[tokio::test]
async fn update_with_initial_none_creates_new_value() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("fresh.json");
    let store = FileSystemCellStore::new();
    let value = store
        .update(&file, "$.greeting", |current: Option<String>| {
            assert!(current.is_none(), "expected no prior value");
            "hello".to_string()
        })
        .await
        .unwrap();
    assert_eq!(value, "hello");

    let echoed: Option<String> = store.read(&file, "$.greeting").await.unwrap();
    assert_eq!(echoed.as_deref(), Some("hello"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_updates_on_same_cell_serialize_correctly() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("counter.json");
    let store = Arc::new(FileSystemCellStore::new());
    store.write(&file, "$.count", 0_u64).await.unwrap();

    const N: u64 = 32;
    let mut handles = Vec::new();
    for _ in 0..N {
        let store = Arc::clone(&store);
        let file = file.clone();
        handles.push(tokio::spawn(async move {
            store
                .update(&file, "$.count", |current: Option<u64>| {
                    current.unwrap_or(0) + 1
                })
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let final_count: Option<u64> = store.read(&file, "$.count").await.unwrap();
    assert_eq!(final_count, Some(N), "race-y update lost increments");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_updates_on_different_cells_in_same_file_serialize() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("shared.json");
    let store = Arc::new(FileSystemCellStore::new());
    store
        .write(&file, "$", json!({"a": 0_u64, "b": 0_u64}))
        .await
        .unwrap();

    const N: u64 = 20;
    let mut handles = Vec::new();
    for i in 0..(2 * N) {
        let store = Arc::clone(&store);
        let file = file.clone();
        let path = if i % 2 == 0 { "$.a" } else { "$.b" };
        handles.push(tokio::spawn(async move {
            store
                .update(&file, path, |current: Option<u64>| current.unwrap_or(0) + 1)
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let a: Option<u64> = store.read(&file, "$.a").await.unwrap();
    let b: Option<u64> = store.read(&file, "$.b").await.unwrap();
    assert_eq!(a, Some(N), "$.a lost increments");
    assert_eq!(b, Some(N), "$.b lost increments");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_updates_on_different_files_run_parallel() {
    let dir = tempdir().unwrap();
    let store = Arc::new(FileSystemCellStore::new());
    let concurrent = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    const N: usize = 4;
    let mut handles = Vec::new();
    for i in 0..N {
        let store = Arc::clone(&store);
        let file = dir.path().join(format!("file-{i}.json"));
        let concurrent = Arc::clone(&concurrent);
        let peak = Arc::clone(&peak);
        handles.push(tokio::spawn(async move {
            store
                .update(&file, "$.k", move |_: Option<u32>| {
                    let now = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(80));
                    concurrent.fetch_sub(1, Ordering::SeqCst);
                    1_u32
                })
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let observed_peak = peak.load(Ordering::SeqCst);
    assert!(
        observed_peak >= 2,
        "expected concurrent execution across different files (peak={observed_peak})"
    );
}

#[tokio::test]
async fn update_returns_new_value_to_caller() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("ret.json");
    let store = FileSystemCellStore::new();
    let returned = store
        .update(&file, "$.tag", |_: Option<String>| {
            "minted-by-mutator".to_string()
        })
        .await
        .unwrap();
    assert_eq!(returned, "minted-by-mutator");
}

#[tokio::test]
async fn compare_exchange_rejects_stale_values_without_writing() {
    use cell_store::{CellStore, FileSystemCellStore};
    use serde_json::json;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("agent.json");
    let store = FileSystemCellStore::new();
    store
        .write(&path, "$.tools", json!({"revision":1}))
        .await
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(!store
        .compare_exchange(
            &path,
            "$.tools",
            Some(json!({"revision":0})),
            json!({"revision":2})
        )
        .await
        .unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(store
        .compare_exchange(
            &path,
            "$.tools",
            Some(json!({"revision":1})),
            json!({"revision":2})
        )
        .await
        .unwrap());
}

#[tokio::test]
async fn compare_exchange_has_one_winner_for_concurrent_writers() {
    use cell_store::{CellStore, FileSystemCellStore};
    use serde_json::json;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("agent.json");
    let store = FileSystemCellStore::new();
    store
        .write(&path, "$.tools", json!({"revision":1}))
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        store.compare_exchange(
            &path,
            "$.tools",
            Some(json!({"revision":1})),
            json!({"revision":2})
        ),
        store.compare_exchange(
            &path,
            "$.tools",
            Some(json!({"revision":1})),
            json!({"revision":3})
        )
    );
    assert_ne!(first.unwrap(), second.unwrap());
}
