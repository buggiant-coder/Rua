use std::sync::Arc;

use rua_sync::crypto::SyncKey;
use rua_sync::hlc::HlcTimestamp;
use rua_sync::media_adapter::{
    parse_record_id, MediaFilePayload, MediaOwner, MediaSyncAdapter, MEDIA_KIND_DELETE,
};
use rua_sync::merge_store::MergeStore;
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

const MEDIA_A: &str = "00000000-0000-4000-8000-000000000001";
const MEDIA_B: &str = "00000000-0000-4000-8000-000000000002";

#[tokio::test]
async fn media_origin_and_unknown_metadata_survive_device_replication() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let (store, key) = blob_context();
    let owner = MediaOwner::Project("project-origin".into());
    let path = owner_path(source.path(), &owner, MEDIA_A);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"image bytes").unwrap();
    let metadata = serde_json::json!({
        "mime_type": "image/png",
        "byte_size": 11,
        "origin": {"device_id": "desktop", "src": "./preview.png", "base_dir": "/tmp/render"},
        "future_metadata": {"keep": true}
    });
    std::fs::write(
        path.with_file_name(format!(".{MEDIA_A}.media.json")),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    let change = MediaSyncAdapter::new(source.path().to_path_buf(), "desktop")
        .with_blob_store(store.clone(), key.clone())
        .snapshot_change_for_file(&owner, MEDIA_A, "origin", "desktop", hlc(10))
        .await
        .unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(
            MediaSyncAdapter::new(target.path().to_path_buf(), "phone").with_blob_store(store, key),
        ))
        .unwrap();
    let merge = tempfile::tempdir().unwrap();
    registry
        .apply_change(&change, &MergeStore::open(merge.path()).unwrap())
        .await
        .unwrap();
    let replicated: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            owner_path(target.path(), &owner, MEDIA_A)
                .with_file_name(format!(".{MEDIA_A}.media.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(replicated, metadata);
}

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn blob_context() -> (rua_sync::endpoint::BlobsApiStore, SyncKey) {
    (
        iroh_blobs::store::mem::MemStore::default().into(),
        SyncKey::from_bytes([7; 32]),
    )
}

fn owner_path(root: &std::path::Path, owner: &MediaOwner, media_id: &str) -> std::path::PathBuf {
    match owner {
        MediaOwner::Agent(id) => root.join("workspaces").join(id),
        MediaOwner::Project(id) => root.join("projects").join(id),
    }
    .join("media")
    .join(media_id)
}

fn write_mime(path: &std::path::Path, mime: &str) {
    let media_id = path.file_name().unwrap().to_str().unwrap();
    std::fs::write(
        path.with_file_name(format!(".{media_id}.media.json")),
        serde_json::to_vec(&serde_json::json!({"mime_type": mime})).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn snapshot_uses_owner_scoped_id_and_inlines_mime_metadata() {
    let data = tempfile::tempdir().unwrap();
    let (store, key) = blob_context();
    let adapter =
        MediaSyncAdapter::new(data.path().to_path_buf(), "device-a").with_blob_store(store, key);
    let owner = MediaOwner::Agent("same".to_string());
    let path = owner_path(data.path(), &owner, MEDIA_A);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"secret image bytes").unwrap();
    write_mime(&path, "image/png");

    let change = adapter
        .snapshot_change_for_file(&owner, MEDIA_A, "op-media", "device-a", hlc(10))
        .await
        .unwrap();
    assert_eq!(change.record_id, format!("agent/same/{MEDIA_A}"));
    let payload: MediaFilePayload = serde_json::from_value(change.payload).unwrap();
    assert_eq!(payload.media_id, MEDIA_A);
    assert_eq!(payload.mime.as_deref(), Some("image/png"));
    assert!(payload.inline_ciphertext_b64.is_some());
    assert!(payload.hash_ref.is_empty());
}

#[tokio::test]
async fn snapshot_large_project_media_uses_blob_store() {
    let data = tempfile::tempdir().unwrap();
    let (store, key) = blob_context();
    let adapter =
        MediaSyncAdapter::new(data.path().to_path_buf(), "device-a").with_blob_store(store, key);
    let owner = MediaOwner::Project("same".to_string());
    let path = owner_path(data.path(), &owner, MEDIA_A);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, vec![7u8; 300 * 1024]).unwrap();
    let change = adapter
        .snapshot_change_for_file(&owner, MEDIA_A, "op-large", "device-a", hlc(10))
        .await
        .unwrap();
    let payload: MediaFilePayload = serde_json::from_value(change.payload).unwrap();
    assert!(!payload.hash_ref.is_empty());
    assert!(payload.inline_ciphertext_b64.is_none());
}

#[tokio::test]
async fn apply_recreates_project_media_and_mime_sidecar() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let merge = tempfile::tempdir().unwrap();
    let (store, key) = blob_context();
    let source_adapter = MediaSyncAdapter::new(source.path().to_path_buf(), "device-a")
        .with_blob_store(store.clone(), key.clone());
    let target_adapter = Arc::new(
        MediaSyncAdapter::new(target.path().to_path_buf(), "device-b").with_blob_store(store, key),
    );
    let mut registry = SyncRegistry::new();
    registry.register(target_adapter).unwrap();
    let owner = MediaOwner::Project("project1".to_string());
    let source_path = owner_path(source.path(), &owner, MEDIA_A);
    std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
    std::fs::write(&source_path, b"portable media").unwrap();
    write_mime(&source_path, "image/png");
    let change = source_adapter
        .snapshot_change_for_file(&owner, MEDIA_A, "op-apply", "device-a", hlc(20))
        .await
        .unwrap();
    let result = registry
        .apply_change(&change, &MergeStore::open(merge.path()).unwrap())
        .await
        .unwrap();
    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let target_path = owner_path(target.path(), &owner, MEDIA_A);
    assert_eq!(std::fs::read(&target_path).unwrap(), b"portable media");
    assert!(target_path
        .with_file_name(format!(".{MEDIA_A}.media.json"))
        .is_file());
}

#[tokio::test]
async fn delete_removes_media_and_mime_sidecar() {
    let data = tempfile::tempdir().unwrap();
    let merge = tempfile::tempdir().unwrap();
    let (store, key) = blob_context();
    let adapter = Arc::new(
        MediaSyncAdapter::new(data.path().to_path_buf(), "device-b").with_blob_store(store, key),
    );
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let owner = MediaOwner::Agent("agent1".to_string());
    let path = owner_path(data.path(), &owner, MEDIA_A);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"old").unwrap();
    write_mime(&path, "image/png");
    let change = MediaSyncAdapter::delete_change("delete", &owner, MEDIA_A, "device-a", hlc(30));
    assert_eq!(change.kind, MEDIA_KIND_DELETE);
    registry
        .apply_change(&change, &MergeStore::open(merge.path()).unwrap())
        .await
        .unwrap();
    assert!(!path.exists());
    assert!(!path
        .with_file_name(format!(".{MEDIA_A}.media.json"))
        .exists());
}

#[tokio::test]
async fn stale_upsert_does_not_overwrite_newer_media() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let merge = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge.path()).unwrap();
    let (store, key) = blob_context();
    let source_adapter = MediaSyncAdapter::new(source.path().to_path_buf(), "device-a")
        .with_blob_store(store.clone(), key.clone());
    let target_adapter = Arc::new(
        MediaSyncAdapter::new(target.path().to_path_buf(), "device-b").with_blob_store(store, key),
    );
    let mut registry = SyncRegistry::new();
    registry.register(target_adapter).unwrap();
    let owner = MediaOwner::Agent("agent1".to_string());
    let source_path = owner_path(source.path(), &owner, MEDIA_A);
    std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
    std::fs::write(&source_path, b"new").unwrap();
    let newer = source_adapter
        .snapshot_change_for_file(&owner, MEDIA_A, "new", "device-a", hlc(200))
        .await
        .unwrap();
    registry.apply_change(&newer, &merge_store).await.unwrap();
    std::fs::write(&source_path, b"old").unwrap();
    let stale = source_adapter
        .snapshot_change_for_file(&owner, MEDIA_A, "old", "device-a", hlc(100))
        .await
        .unwrap();
    let result = registry.apply_change(&stale, &merge_store).await.unwrap();
    assert_eq!(result.status, RegistryApplyStatus::Skipped);
    assert_eq!(
        std::fs::read(owner_path(target.path(), &owner, MEDIA_A)).unwrap(),
        b"new"
    );
}

#[tokio::test]
async fn export_scans_agent_and_project_media_without_sidecars() {
    let data = tempfile::tempdir().unwrap();
    let (store, key) = blob_context();
    let adapter =
        MediaSyncAdapter::new(data.path().to_path_buf(), "device-a").with_blob_store(store, key);
    for (owner, media_id) in [
        (MediaOwner::Agent("same".to_string()), MEDIA_A),
        (MediaOwner::Project("same".to_string()), MEDIA_B),
    ] {
        let path = owner_path(data.path(), &owner, media_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"content").unwrap();
        write_mime(&path, "text/plain");
    }
    let changes = adapter.export_records().await.unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].record_id, format!("agent/same/{MEDIA_A}"));
    assert_eq!(changes[1].record_id, format!("project/same/{MEDIA_B}"));
}

#[test]
fn parser_rejects_old_ids_and_invalid_owner_or_media() {
    assert!(parse_record_id(&format!("agent/same/{MEDIA_A}")).is_ok());
    assert!(parse_record_id(&format!("project/same/{MEDIA_A}")).is_ok());
    assert!(parse_record_id("same/legacy.png").is_err());
    assert!(parse_record_id(&format!("unknown/same/{MEDIA_A}")).is_err());
    assert!(parse_record_id("agent/same/not-a-uuid").is_err());
}
