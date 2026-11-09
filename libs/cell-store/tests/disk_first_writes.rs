#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cell_store::{CellStore, CellStoreError, FileSystemCellStore};
use tempfile::tempdir;

fn is_io_error(err: &CellStoreError) -> bool {
    matches!(err, CellStoreError::Io { .. } | CellStoreError::IoBare(_))
}

fn lock_dir(path: &std::path::Path) {
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o555);
    std::fs::set_permissions(path, perms).unwrap();
}

fn unlock_dir(path: &std::path::Path) {
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[tokio::test]
async fn write_failure_leaves_cache_unchanged() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("guarded.json");
    let store = FileSystemCellStore::new();
    store.write(&file, "$.k", "v1".to_string()).await.unwrap();
    assert_eq!(
        store.read::<String>(&file, "$.k").await.unwrap().as_deref(),
        Some("v1")
    );

    lock_dir(dir.path());
    let result = store.write(&file, "$.k", "v2-must-fail".to_string()).await;
    unlock_dir(dir.path());

    let err = result.expect_err("write into read-only parent must fail");
    assert!(is_io_error(&err), "expected IO error, got {err:?}");

    let after: Option<String> = store.read(&file, "$.k").await.unwrap();
    assert_eq!(
        after.as_deref(),
        Some("v1"),
        "cache must not be updated on disk failure"
    );
}

#[tokio::test]
async fn update_failure_does_not_apply_mutator() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("guarded.json");
    let store = FileSystemCellStore::new();
    store.write(&file, "$.n", 1_u32).await.unwrap();

    lock_dir(dir.path());

    let mutator_ran = Arc::new(AtomicBool::new(false));
    let mutator_ran_clone = Arc::clone(&mutator_ran);
    let result = store
        .update::<u32, _>(&file, "$.n", move |current| {
            mutator_ran_clone.store(true, Ordering::SeqCst);
            current.unwrap_or(0) + 99
        })
        .await;

    unlock_dir(dir.path());

    let err = result.expect_err("update into read-only parent must fail");
    assert!(is_io_error(&err), "expected IO error, got {err:?}");
    assert!(
        mutator_ran.load(Ordering::SeqCst),
        "mutator runs before the disk write attempt, so it must have executed"
    );

    let after: Option<u32> = store.read(&file, "$.n").await.unwrap();
    assert_eq!(
        after,
        Some(1),
        "cache must reflect the original value, not the would-be mutation"
    );
}
