use cell_store::{CellStore, CellStoreError, FileSystemCellStore};
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn read_returns_none_for_missing_file() {
    let dir = tempdir().unwrap();
    let store = FileSystemCellStore::new();
    let value: Option<String> = store
        .read(&dir.path().join("nope.json"), "$.field")
        .await
        .unwrap();
    assert!(value.is_none());
}

#[tokio::test]
async fn read_returns_none_for_missing_json_path() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("only-other-field.json");
    let store = FileSystemCellStore::new();
    store
        .write(&file, "$.other", json!("present"))
        .await
        .unwrap();
    let missing: Option<String> = store.read(&file, "$.absent").await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn write_then_read_roundtrips_value() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("round.json");
    let store = FileSystemCellStore::new();
    store
        .write(&file, "$.name", "openai".to_string())
        .await
        .unwrap();
    let value: Option<String> = store.read(&file, "$.name").await.unwrap();
    assert_eq!(value.as_deref(), Some("openai"));
}

#[tokio::test]
async fn write_creates_parent_dirs_if_missing() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("nested/dirs/that/do/not/exist.json");
    let store = FileSystemCellStore::new();
    store.write(&file, "$.k", 42_u32).await.unwrap();
    assert!(file.exists(), "parent dirs should be created on write");
    let value: Option<u32> = store.read(&file, "$.k").await.unwrap();
    assert_eq!(value, Some(42));
}

#[tokio::test]
async fn delete_removes_field() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("with-field.json");
    let store = FileSystemCellStore::new();
    store.write(&file, "$.keep", json!(1)).await.unwrap();
    store.write(&file, "$.drop", json!(2)).await.unwrap();
    store.delete(&file, "$.drop").await.unwrap();

    let keep: Option<i32> = store.read(&file, "$.keep").await.unwrap();
    let dropped: Option<i32> = store.read(&file, "$.drop").await.unwrap();
    assert_eq!(keep, Some(1));
    assert_eq!(dropped, None);
    assert!(file.exists(), "file itself should remain");
}

#[tokio::test]
async fn delete_with_root_path_removes_entire_file() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("doomed.json");
    let store = FileSystemCellStore::new();
    store.write(&file, "$", json!({"x": 1})).await.unwrap();
    assert!(file.exists());
    store.delete(&file, "$").await.unwrap();
    assert!(!file.exists(), "root delete should remove the file");

    store.delete(&file, "$").await.unwrap();
}

#[tokio::test]
async fn read_supports_nested_json_path() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("nested.json");
    let store = FileSystemCellStore::new();
    store
        .write(&file, "$", json!({"a": {"b": {"c": "deep"}}}))
        .await
        .unwrap();
    let v: Option<String> = store.read(&file, "$.a.b.c").await.unwrap();
    assert_eq!(v.as_deref(), Some("deep"));
}

#[tokio::test]
async fn read_supports_array_index_json_path() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("array.json");
    let store = FileSystemCellStore::new();
    store
        .write(&file, "$.arr", json!(["zero", "one", "two"]))
        .await
        .unwrap();
    let v: Option<String> = store.read(&file, "$.arr[1]").await.unwrap();
    assert_eq!(v.as_deref(), Some("one"));
    let missing: Option<String> = store.read(&file, "$.arr[99]").await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn invalid_json_path_returns_error() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("any.json");
    let store = FileSystemCellStore::new();
    for bad in &["", "field", "$..x", "$.[0]", "$.a[", "$.a[abc]"] {
        let err = store
            .read::<serde_json::Value>(&file, bad)
            .await
            .expect_err(&format!("expected InvalidJsonPath for {bad:?}"));
        assert!(
            matches!(err, CellStoreError::InvalidJsonPath(_)),
            "unexpected error variant for {bad:?}: {err:?}"
        );
    }
}

#[tokio::test]
async fn write_with_root_path_replaces_entire_value() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("root.json");
    let store = FileSystemCellStore::new();
    store.write(&file, "$", json!({"first": 1})).await.unwrap();
    store.write(&file, "$", json!({"second": 2})).await.unwrap();
    let prev: Option<i32> = store.read(&file, "$.first").await.unwrap();
    assert!(prev.is_none(), "root write should replace, not merge");
    let now: Option<i32> = store.read(&file, "$.second").await.unwrap();
    assert_eq!(now, Some(2));
}

#[tokio::test]
async fn root_write_replaces_malformed_json_without_reading_old_file() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("corrupt.json");
    std::fs::write(&file, "not json").unwrap();
    let store = FileSystemCellStore::new();

    store.write(&file, "$", json!({"ok": true})).await.unwrap();

    let v: Option<serde_json::Value> = store.read(&file, "$").await.unwrap();
    assert_eq!(v, Some(json!({"ok": true})));
}
