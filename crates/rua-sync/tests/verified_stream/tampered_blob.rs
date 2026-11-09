use rua_sync::crypto::SyncKey;
use rua_sync::opaque_replica_log::{put_encrypted_blob, read_encrypted_blob};
use rua_sync::sync_blob_store::{SyncBlobFetchConfig, SyncBlobStore};

#[tokio::test]
async fn get_blob_with_wrong_hash_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");

    let sync_key = SyncKey::from_bytes([0u8; 32]);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let api_store = store.api_store();

    let hash_ref = put_encrypted_blob(&api_store, &sync_key, b"real data")
        .await
        .unwrap();

    let mut bad_hash = hash_ref.clone();
    let byte = if bad_hash.starts_with('0') { "1" } else { "0" };
    bad_hash.replace_range(0..1, byte);

    let result = read_encrypted_blob(&api_store, &sync_key, &bad_hash)
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn invalid_hash_hex_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");

    let sync_key = SyncKey::from_bytes([0u8; 32]);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let api_store = store.api_store();

    let result = read_encrypted_blob(&api_store, &sync_key, "not_valid_hex!").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn short_hash_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");

    let sync_key = SyncKey::from_bytes([0u8; 32]);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let api_store = store.api_store();

    let result = read_encrypted_blob(&api_store, &sync_key, "abcd").await;
    assert!(result.is_err());
}
