use bytes::Bytes;
use iroh_blobs::api::blobs::BlobStatus;
use iroh_blobs::api::Store as BlobsApiStore;
use iroh_blobs::store::fs::options::Options as BlobsFsOptions;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::store::GcConfig;
use iroh_blobs::HashAndFormat;
use rua_sync::crypto::SyncKey;
use rua_sync::endpoint::SyncNode;
use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use rua_sync::opaque_replica_log::{put_encrypted_blob, read_encrypted_blob};
use rua_sync::sync_blob_store::{SyncBlobFetchConfig, SyncBlobStore};
use std::time::Duration;

#[tokio::test]
async fn put_get_roundtrip_1mb() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");

    let sync_key = SyncKey::from_bytes([0u8; 32]);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let api_store = store.api_store();

    let plaintext: Vec<u8> = (0..1_048_576).map(|i| (i % 256) as u8).collect();

    let hash_ref = put_encrypted_blob(&api_store, &sync_key, &plaintext)
        .await
        .unwrap();

    assert!(!hash_ref.is_empty());

    let recovered = read_encrypted_blob(&api_store, &sync_key, &hash_ref)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered, plaintext);
}

#[tokio::test]
async fn put_stores_only_ciphertext() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");

    let sync_key = SyncKey::from_bytes([0u8; 32]);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let api_store = store.api_store();

    let plaintext = b"secret sync content";
    let hash_ref = put_encrypted_blob(&api_store, &sync_key, plaintext)
        .await
        .unwrap();

    let plaintext_hash = blake3::hash(plaintext).to_hex().to_string();
    assert_ne!(
        hash_ref, plaintext_hash,
        "hash should be of ciphertext, not plaintext"
    );
}

#[tokio::test]
async fn has_blob_returns_true_after_put() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");

    let sync_key = SyncKey::from_bytes([0u8; 32]);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let api_store = store.api_store();

    let hash_ref = put_encrypted_blob(&api_store, &sync_key, b"hello")
        .await
        .unwrap();

    assert!(store.has_blob(&hash_ref).await.unwrap());
}

#[tokio::test]
async fn put_encrypted_blob_uses_temporary_tag_reclaimed_by_gc() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");
    std::fs::create_dir_all(&blobs_dir).unwrap();

    let mut opts = BlobsFsOptions::new(&blobs_dir);
    opts.gc = Some(GcConfig {
        interval: Duration::from_secs(1),
        add_protected: None,
    });
    let store: BlobsApiStore = FsStore::load_with_opts(blobs_dir.join("blobs.db"), opts)
        .await
        .unwrap()
        .into();
    let sync_key = SyncKey::from_bytes([9u8; 32]);

    std::env::set_var("RUA_SYNC_TEMP_BLOB_RETENTION_SECS", "1");
    let hash_ref = put_encrypted_blob(&store, &sync_key, &vec![0x5Au8; 1024 * 1024])
        .await
        .unwrap();
    std::env::remove_var("RUA_SYNC_TEMP_BLOB_RETENTION_SECS");
    let hash_bytes: [u8; 32] = hex::decode(&hash_ref).unwrap().try_into().unwrap();
    let hash = iroh_blobs::Hash::from_bytes(hash_bytes);

    assert!(
        matches!(
            store.status(hash).await.unwrap(),
            BlobStatus::Complete { .. }
        ),
        "blob must be present while the retained TempTag is alive"
    );

    tokio::time::sleep(Duration::from_secs(5)).await;

    let status = store.status(hash).await.unwrap();
    assert!(
        matches!(status, BlobStatus::NotFound),
        "temporary encrypted blob should be reclaimed after TTL + GC, got {status:?}"
    );
}

#[tokio::test]
async fn production_external_store_starts_background_gc() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");
    let node_dir = dir.path().join("node");
    let device = DeviceIdentity::generate();
    let network = NetworkSecret::generate();

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let old_node = SyncNode::bind_with_gc_interval(
        &node_dir,
        network.namespace_secret(),
        &device,
        Some(store.api_store()),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    let current = store
        .api_store()
        .blobs()
        .add_slice("current conversation state")
        .temp_tag()
        .await
        .unwrap();
    let current_hash = current.hash();
    old_node
        .doc()
        .set_hash(
            device.to_iroh_author().id(),
            Bytes::from_static(b"conversations/current.json"),
            current_hash,
            "current conversation state".len() as u64,
        )
        .await
        .unwrap();
    drop(current);
    let orphan = store
        .api_store()
        .blobs()
        .add_slice("orphaned sync blob")
        .temp_tag()
        .await
        .unwrap();
    let orphan_hash = orphan.hash();
    drop(orphan);
    let current_auto_tag = store
        .api_store()
        .tags()
        .create(HashAndFormat::raw(current_hash))
        .await
        .unwrap();
    let orphan_auto_tag = store
        .api_store()
        .tags()
        .create(HashAndFormat::raw(orphan_hash))
        .await
        .unwrap();
    store
        .api_store()
        .tags()
        .set("explicit-sync-tag", HashAndFormat::raw(orphan_hash))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(3), old_node.shutdown())
        .await
        .expect("old SyncNode should shut down before reopening its stores");
    drop(old_node);
    drop(store);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let node = SyncNode::bind_with_gc_interval(
        &node_dir,
        network.namespace_secret(),
        &device,
        Some(store.api_store()),
        Duration::from_millis(20),
    )
    .await
    .unwrap();

    assert!(store
        .api_store()
        .tags()
        .get(current_auto_tag)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .api_store()
        .tags()
        .get(orphan_auto_tag)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .api_store()
        .tags()
        .get("explicit-sync-tag")
        .await
        .unwrap()
        .is_some());
    store
        .api_store()
        .tags()
        .delete("explicit-sync-tag")
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if matches!(
                store.api_store().status(orphan_hash).await.unwrap(),
                BlobStatus::NotFound
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("external SyncBlobStore should run background GC");

    assert!(matches!(
        store.api_store().status(current_hash).await.unwrap(),
        BlobStatus::Complete { .. }
    ));

    tokio::time::timeout(Duration::from_secs(3), node.shutdown())
        .await
        .expect("SyncNode shutdown should stop background GC");
}

#[tokio::test]
async fn read_encrypted_blob_returns_none_for_missing() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");

    let sync_key = SyncKey::from_bytes([0u8; 32]);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let api_store = store.api_store();

    let result = read_encrypted_blob(&api_store, &sync_key, &"00".repeat(32))
        .await
        .unwrap();
    assert!(result.is_none());
}
