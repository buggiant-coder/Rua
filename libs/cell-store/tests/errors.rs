#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use cell_store::{CellStore, CellStoreError, FileSystemCellStore};
use tempfile::tempdir;

#[tokio::test]
async fn malformed_json_file_surfaces_serde_error() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("broken.json");
    tokio::fs::write(&file, b"{ this is not json")
        .await
        .unwrap();

    let store = FileSystemCellStore::new();
    let err = store
        .read::<serde_json::Value>(&file, "$.anything")
        .await
        .expect_err("malformed JSON should surface a serde error");
    assert!(
        matches!(
            err,
            CellStoreError::Serde { .. } | CellStoreError::SerdeBare(_)
        ),
        "expected serde error, got {err:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn permission_denied_surfaces_io_error() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("nope.json");
    tokio::fs::write(&file, br#"{"ok": true}"#).await.unwrap();

    let mut perms = std::fs::metadata(&file).unwrap().permissions();
    perms.set_mode(0o000);
    std::fs::set_permissions(&file, perms).unwrap();

    let store = FileSystemCellStore::new();
    let result = store.read::<serde_json::Value>(&file, "$.ok").await;

    let mut perms = std::fs::metadata(&file).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&file, perms).unwrap();

    let err = result.expect_err("unreadable file should surface an IO error");
    assert!(
        matches!(err, CellStoreError::Io { .. } | CellStoreError::IoBare(_)),
        "expected IO error, got {err:?}"
    );
}

#[tokio::test]
async fn invalid_json_path_syntax_returns_typed_error() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("file.json");
    let store = FileSystemCellStore::new();
    let cases = [
        ("", "empty path"),
        ("field", "missing leading $"),
        ("$..x", "double dot"),
        ("$.a[", "unterminated bracket"),
        ("$.a[abc]", "non-numeric index"),
        ("$.a[]", "empty index"),
        ("$.", "dangling dot"),
    ];
    for (bad, why) in cases {
        let err = store
            .read::<serde_json::Value>(&file, bad)
            .await
            .expect_err(&format!("{why} ({bad:?}) must error on read"));
        assert!(
            matches!(err, CellStoreError::InvalidJsonPath(_)),
            "{why} ({bad:?}) on read: expected InvalidJsonPath, got {err:?}"
        );
        let err = store
            .write::<serde_json::Value>(&file, bad, serde_json::Value::Null)
            .await
            .expect_err(&format!("{why} ({bad:?}) must error on write"));
        assert!(
            matches!(err, CellStoreError::InvalidJsonPath(_)),
            "{why} ({bad:?}) on write: expected InvalidJsonPath, got {err:?}"
        );
    }
}
