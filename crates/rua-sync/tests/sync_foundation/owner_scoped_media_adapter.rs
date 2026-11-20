use std::path::Path;
use std::sync::Arc;

use rua_session::chat_media::ChatMediaManifest;
use rua_session::display_media::MediaOrigin;
use rua_sync::hlc::HlcTimestamp;
use rua_sync::media_adapter::{parse_record_id, MediaSyncAdapter};
use rua_sync::merge_store::MergeStore;
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};
use uuid::Uuid;

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}
fn media_dir(root: &Path) -> std::path::PathBuf {
    root.join("chats/media")
}
fn manifest(id: Uuid, bytes: &[u8]) -> ChatMediaManifest {
    ChatMediaManifest::new(id, "chat-a", "desktop", "image/png".into(), bytes).unwrap()
}
fn registry(root: &Path) -> SyncRegistry {
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(MediaSyncAdapter::new(root.into(), "phone")))
        .unwrap();
    registry
}

#[tokio::test]
async fn media_metadata_replication_preserves_origin_and_requires_no_media_bytes() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let id = Uuid::new_v4();
    let mut item = manifest(id, &vec![b'a'; 300 * 1024]);
    item.origin = Some(MediaOrigin {
        device_id: "desktop".into(),
        src: "./picture.png".into(),
        base_dir: Some("/pictures".into()),
        extra: Default::default(),
    });
    item.original_filename = Some("picture.png".into());
    item.extra
        .insert("future_field".into(), serde_json::json!({"preserve":true}));
    item.register(&media_dir(source.path())).unwrap();
    let adapter = MediaSyncAdapter::new(source.path().into(), "desktop");
    let changes = adapter.export_records().await.unwrap();
    assert_eq!(changes.len(), 1);
    assert!(serde_json::to_vec(&changes[0].payload).unwrap().len() < 1024);
    assert_eq!(
        serde_json::from_value::<ChatMediaManifest>(changes[0].payload.clone()).unwrap(),
        item
    );
    let store = MergeStore::open(merge_dir.path()).unwrap();
    registry(target.path())
        .apply_change(&changes[0], &store)
        .await
        .unwrap();
    assert_eq!(
        ChatMediaManifest::load(&media_dir(target.path()), id).unwrap(),
        Some(item.clone())
    );
    assert!(!media_dir(target.path()).join(id.to_string()).exists());
    assert!(!media_dir(source.path()).join(id.to_string()).exists());

    // Having a partial local file does not make the Device a verified source.
    std::fs::write(media_dir(target.path()).join(id.to_string()), b"partial").unwrap();
    assert!(item.verify_copy(&media_dir(target.path())).is_err());
    assert_eq!(
        ChatMediaManifest::load(&media_dir(target.path()), id)
            .unwrap()
            .unwrap()
            .device_ids
            .len(),
        1
    );
}

#[tokio::test]
async fn distinct_holders_and_chat_references_converge_even_with_older_announcements() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let merge = tempfile::tempdir().unwrap();
    let id = Uuid::new_v4();
    let item = manifest(id, b"abc");
    item.register(&media_dir(src.path())).unwrap();
    let origin = MediaSyncAdapter::new(src.path().into(), "desktop");
    let old = origin
        .snapshot_change_for_file(&id.to_string(), "old", "desktop", hlc(10))
        .await
        .unwrap();
    let mut newer = old.clone();
    newer.op_id = "new".into();
    newer.hlc = hlc(20);
    let mut second = item.clone();
    second.device_ids = ["phone".to_owned()].into();
    second.chat_ids = ["child".to_owned()].into();
    newer.payload = serde_json::to_value(&second).unwrap();
    let receiver = registry(dst.path());
    let state = MergeStore::open(merge.path()).unwrap();
    receiver.apply_change(&newer, &state).await.unwrap();
    receiver.apply_change(&old, &state).await.unwrap();
    let saved = ChatMediaManifest::load(&media_dir(dst.path()), id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.chat_ids, ["chat-a".into(), "child".into()].into());
    assert_eq!(saved.device_ids, ["desktop".into(), "phone".into()].into());
    let mut conflict = old.clone();
    conflict.op_id = "conflicting".into();
    conflict.payload["sha256"] = "0".repeat(64).into();
    assert!(receiver.apply_change(&conflict, &state).await.is_err());
    assert_eq!(
        ChatMediaManifest::load(&media_dir(dst.path()), id)
            .unwrap()
            .unwrap(),
        saved
    );
}

#[tokio::test]
async fn a_reclaimed_object_does_not_reappear_from_a_late_replica() {
    let root = tempfile::tempdir().unwrap();
    let merge = tempfile::tempdir().unwrap();
    let id = Uuid::new_v4();
    let dir = media_dir(root.path());
    let data = b"image";
    let m = manifest(id, data);
    m.store_copy(&dir, data).unwrap();
    let adapter = MediaSyncAdapter::new(root.path().into(), "desktop");
    let delayed = adapter
        .snapshot_change_for_file(&id.to_string(), "delayed", "laptop", hlc(80))
        .await
        .unwrap();
    let registry = registry(root.path());
    let state = MergeStore::open(merge.path()).unwrap();
    let delete = MediaSyncAdapter::delete_change("reclaim", &id.to_string(), "phone", hlc(70));
    registry.apply_change(&delete, &state).await.unwrap();
    assert_eq!(
        registry
            .apply_change(&delayed, &state)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    assert!(!dir.join(id.to_string()).exists());
    assert!(ChatMediaManifest::load(&dir, id).unwrap().is_none());
}

#[tokio::test]
async fn staged_and_unowned_files_are_excluded_from_media_inventory() {
    let root = tempfile::tempdir().unwrap();
    let dir = media_dir(root.path());
    let staged = Uuid::new_v4();
    let committed = Uuid::new_v4();
    manifest(staged, b"draft")
        .store_copy(&dir, b"draft")
        .unwrap();
    manifest(committed, b"saved").register(&dir).unwrap();
    std::fs::write(dir.join(format!(".staged-{staged}.json")), b"{}").unwrap();
    std::fs::write(dir.join(Uuid::new_v4().to_string()), b"no manifest").unwrap();
    let changes = MediaSyncAdapter::new(root.path().into(), "desktop")
        .export_records()
        .await
        .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].record_id, committed.to_string());
}

#[test]
fn inventory_identity_requires_a_canonical_media_uuid() {
    let id = Uuid::new_v4().to_string();
    assert_eq!(parse_record_id(&id).unwrap(), id);
    assert!(parse_record_id(&format!("agent/old/{id}")).is_err());
    assert!(parse_record_id("unknown-image.png").is_err());
}

#[tokio::test]
async fn receiving_only_chat_state_locates_the_original_upload_without_a_global_media_record() {
    use rua_core::{ChatAddress, ChatStore};
    use rua_session::WorkspaceSessionStore;
    use rua_sync::evergreen_chat_adapter::EvergreenChatSyncAdapter;

    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let merge = tempfile::tempdir().unwrap();
    let chat = ChatAddress::new("chat-a");
    let id = Uuid::new_v4();
    let source = ChatStore::resolve(a.path(), chat.clone());
    let target = ChatStore::resolve(b.path(), chat.clone());
    let mut item = manifest(id, b"received image");
    item.chat_ids.insert("child".into());
    item.store_copy(&media_dir(a.path()), b"received image")
        .unwrap();
    item.record_reference(
        &WorkspaceSessionStore::new(source.root()).chat_dir("chat-a"),
        "chat-a",
    )
    .unwrap();
    let changes = EvergreenChatSyncAdapter::snapshot_changes_for_chat(
        source.root(),
        &chat,
        "desktop",
        hlc(10),
    )
    .unwrap();
    let mut sync = SyncRegistry::new();
    sync.register(Arc::new(EvergreenChatSyncAdapter::new(
        b.path().into(),
        "phone",
    )))
    .unwrap();
    let merge = MergeStore::open(merge.path()).unwrap();
    for change in changes {
        sync.apply_change(&change, &merge).await.unwrap();
    }
    let received = ChatMediaManifest::load_reference(
        &WorkspaceSessionStore::new(target.root()).chat_dir("chat-a"),
        id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(received.device_ids, ["desktop".into()].into());
    assert_eq!(received.chat_ids, ["chat-a".into()].into());
    assert_eq!(received.sha256, item.sha256);
    assert!(ChatMediaManifest::load(&media_dir(b.path()), id)
        .unwrap()
        .is_none());
    assert!(!media_dir(b.path()).join(id.to_string()).exists());
}
