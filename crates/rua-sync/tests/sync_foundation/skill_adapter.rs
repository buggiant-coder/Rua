use std::sync::Arc;

use base64::Engine;
use iroh::endpoint::presets;
use rua_sync::crypto::SyncKey;
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::opaque_replica_log::{put_encrypted_blob, SyncChange};
use rua_sync::skill_adapter::{
    workspace_record_id, SkillBlobFileRef, SkillBlobManifest, SkillBlobManifestSyncAdapter,
    SKILL_BLOB_DOMAIN, SKILL_BLOB_KIND_DELETE, SKILL_BLOB_KIND_UPSERT,
};
use rua_sync::sync_blob_store::StatefulBlobDownloader;
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

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
        SyncKey::from_bytes([9; 32]),
    )
}

fn skill_dir(data_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    data_dir.join("skill_pool").join(name)
}

fn workspace_skill_dir(
    data_dir: &std::path::Path,
    agent_id: &str,
    name: &str,
) -> std::path::PathBuf {
    data_dir
        .join("workspaces")
        .join(agent_id)
        .join("skills")
        .join(name)
}

async fn adapter(data_dir: &std::path::Path) -> Arc<SkillBlobManifestSyncAdapter> {
    let (store, key) = blob_context();
    Arc::new(
        SkillBlobManifestSyncAdapter::new(data_dir.to_path_buf(), "device-b")
            .with_blob_store(store, key),
    )
}

#[tokio::test]
async fn snapshot_manifest_inlines_small_file_content_without_blob_ref() {
    let data = tempfile::tempdir().unwrap();
    let adapter = adapter(data.path()).await;
    let dir = skill_dir(data.path(), "writer");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "secret instructions").unwrap();

    let change = adapter
        .snapshot_change_for_skill("writer", "op-skill", "device-a", hlc(10))
        .await
        .unwrap();

    assert_eq!(change.domain, SKILL_BLOB_DOMAIN);
    assert_eq!(change.kind, SKILL_BLOB_KIND_UPSERT);
    let manifest: SkillBlobManifest = serde_json::from_value(change.payload).unwrap();
    assert_eq!(manifest.scope, "pool");
    assert_eq!(manifest.agent_id, None);
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(manifest.files[0].rel_path, "SKILL.md");
    assert_eq!(manifest.files[0].size, "secret instructions".len() as u64);
    assert_eq!(manifest.files[0].hash_ref, "");
    assert!(manifest.files[0].inline_ciphertext_b64.is_some());
}

#[tokio::test]
async fn snapshot_manifest_stores_large_file_content_in_blob_store() {
    let data = tempfile::tempdir().unwrap();
    let adapter = adapter(data.path()).await;
    let dir = skill_dir(data.path(), "writer");
    std::fs::create_dir_all(&dir).unwrap();
    let large_content = vec![b'x'; 300 * 1024];
    std::fs::write(dir.join("resource.bin"), &large_content).unwrap();

    let change = adapter
        .snapshot_change_for_skill("writer", "op-large-skill", "device-a", hlc(10))
        .await
        .unwrap();

    let manifest: SkillBlobManifest = serde_json::from_value(change.payload).unwrap();
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(manifest.files[0].rel_path, "resource.bin");
    assert_eq!(manifest.files[0].size, large_content.len() as u64);
    assert_ne!(manifest.files[0].hash_ref, "");
    assert!(manifest.files[0].inline_ciphertext_b64.is_none());
}

#[tokio::test]
async fn export_filters_untouched_seeded_pool_skill_records() {
    let data = tempfile::tempdir().unwrap();
    let seeded = skill_dir(data.path(), "guidance");
    let user = skill_dir(data.path(), "writer");
    std::fs::create_dir_all(&seeded).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(seeded.join("SKILL.md"), "seed skill").unwrap();
    std::fs::write(user.join("SKILL.md"), "user skill").unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "skill_pool:guidance",
        "builtin_skill_pool",
        &["skill_pool/guidance/SKILL.md"],
    )
    .unwrap();

    let adapter = adapter(data.path()).await;
    let exported = adapter.export_records().await.unwrap();
    let ids: Vec<_> = exported
        .iter()
        .map(|change| change.record_id.as_str())
        .collect();

    assert_eq!(ids, vec!["writer"]);
}

#[tokio::test]
async fn export_keeps_seeded_pool_skill_record_after_file_modification() {
    let data = tempfile::tempdir().unwrap();
    let seeded = skill_dir(data.path(), "guidance");
    std::fs::create_dir_all(&seeded).unwrap();
    std::fs::write(seeded.join("SKILL.md"), "seed skill").unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "skill_pool:guidance",
        "builtin_skill_pool",
        &["skill_pool/guidance/SKILL.md"],
    )
    .unwrap();
    std::fs::write(seeded.join("SKILL.md"), "custom skill").unwrap();

    let adapter = adapter(data.path()).await;
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].record_id, "guidance");
}

#[tokio::test]
async fn export_filters_untouched_seeded_workspace_skill_records() {
    let data = tempfile::tempdir().unwrap();
    let seeded = workspace_skill_dir(data.path(), "default", "guidance");
    let user = workspace_skill_dir(data.path(), "default", "writer");
    std::fs::create_dir_all(&seeded).unwrap();
    std::fs::create_dir_all(&user).unwrap();
    std::fs::write(seeded.join("SKILL.md"), "seed skill").unwrap();
    std::fs::write(user.join("SKILL.md"), "user skill").unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "workspace_skill:default:guidance",
        "agent_seed_catalog",
        &["workspaces/default/skills/guidance/SKILL.md"],
    )
    .unwrap();

    let adapter = adapter(data.path()).await;
    let exported = adapter.export_records().await.unwrap();
    let ids: Vec<_> = exported
        .iter()
        .map(|change| change.record_id.as_str())
        .collect();

    assert_eq!(ids, vec![workspace_record_id("default", "writer")]);
}

#[tokio::test]
async fn workspace_snapshot_and_apply_preserve_manifest_entry() {
    let data = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (store, key) = blob_context();
    let adapter = Arc::new(
        SkillBlobManifestSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_blob_store(store.clone(), key.clone()),
    );
    let remote_adapter = SkillBlobManifestSyncAdapter::new(remote.path().to_path_buf(), "device-a")
        .with_blob_store(store, key);
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let remote_dir = workspace_skill_dir(remote.path(), "agent1", "writer");
    std::fs::create_dir_all(&remote_dir).unwrap();
    std::fs::write(remote_dir.join("SKILL.md"), "workspace secret").unwrap();
    let manifest_path = remote
        .path()
        .join("workspaces")
        .join("agent1")
        .join("skill.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "skills": {
                "writer": {
                    "enabled": true,
                    "channels": ["all"],
                    "config": {"tone": "direct"}
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let change = remote_adapter
        .snapshot_change_for_workspace_skill(
            "agent1",
            "writer",
            "op-workspace-skill",
            "device-a",
            hlc(20),
        )
        .await
        .unwrap();

    assert_eq!(change.record_id, workspace_record_id("agent1", "writer"));
    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert_eq!(
        std::fs::read_to_string(
            workspace_skill_dir(data.path(), "agent1", "writer").join("SKILL.md")
        )
        .unwrap(),
        "workspace secret"
    );
    let local_manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(data.path().join("workspaces/agent1/skill.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        local_manifest["skills"]["writer"]["config"]["tone"],
        "direct"
    );
}

#[tokio::test]
async fn workspace_delete_removes_skill_dir_and_manifest_entry() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = adapter(data.path()).await;
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let dir = workspace_skill_dir(data.path(), "agent1", "writer");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest_path = data
        .path()
        .join("workspaces")
        .join("agent1")
        .join("skill.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "skills": {"writer": {"enabled": false}}
        }))
        .unwrap(),
    )
    .unwrap();

    let change = SkillBlobManifestSyncAdapter::workspace_delete_change(
        "op-workspace-delete",
        "agent1",
        "writer",
        "device-a",
        hlc(40),
    );
    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert!(!dir.exists());
    let local_manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(manifest_path).unwrap()).unwrap();
    assert!(local_manifest["skills"].get("writer").is_none());
}

#[tokio::test]
async fn inbound_text_file_uses_three_way_merge() {
    let data = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (store, key) = blob_context();
    let adapter = Arc::new(
        SkillBlobManifestSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_blob_store(store.clone(), key.clone()),
    );
    let remote_adapter = SkillBlobManifestSyncAdapter::new(remote.path().to_path_buf(), "device-a")
        .with_blob_store(store, key);
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let local_dir = skill_dir(data.path(), "writer");
    std::fs::create_dir_all(&local_dir).unwrap();
    std::fs::write(local_dir.join("SKILL.md"), "base\nlocal\n").unwrap();

    let remote_dir = skill_dir(remote.path(), "writer");
    std::fs::create_dir_all(&remote_dir).unwrap();
    std::fs::write(remote_dir.join("SKILL.md"), "base\nremote\n").unwrap();
    let change = remote_adapter
        .snapshot_change_for_skill("writer", "op-skill-text", "device-a", hlc(20))
        .await
        .unwrap();

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let merged = std::fs::read_to_string(local_dir.join("SKILL.md")).unwrap();
    assert!(merged.contains("remote"));
}

#[tokio::test]
async fn inbound_binary_file_uses_whole_file_lww_and_preserves_executable_bit() {
    let data = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (store, key) = blob_context();
    let adapter = Arc::new(
        SkillBlobManifestSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_blob_store(store.clone(), key.clone()),
    );
    let remote_adapter = SkillBlobManifestSyncAdapter::new(remote.path().to_path_buf(), "device-a")
        .with_blob_store(store, key);
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let remote_dir = skill_dir(remote.path(), "runner");
    std::fs::create_dir_all(remote_dir.join("bin")).unwrap();
    let script = remote_dir.join("bin/run");
    std::fs::write(&script, b"\x00\x01remote").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let change = remote_adapter
        .snapshot_change_for_skill("runner", "op-skill-bin", "device-a", hlc(20))
        .await
        .unwrap();

    registry.apply_change(&change, &merge_store).await.unwrap();

    let local_script = skill_dir(data.path(), "runner").join("bin/run");
    assert_eq!(std::fs::read(&local_script).unwrap(), b"\x00\x01remote");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(
            std::fs::metadata(&local_script)
                .unwrap()
                .permissions()
                .mode()
                & 0o111
                != 0
        );
    }
}

#[tokio::test]
async fn delete_removes_skill_directory() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = adapter(data.path()).await;
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let dir = skill_dir(data.path(), "writer");
    std::fs::create_dir_all(&dir).unwrap();

    let change = SkillBlobManifestSyncAdapter::delete_change(
        "op-skill-delete",
        "writer",
        "device-a",
        hlc(30),
    );
    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert!(!dir.exists());
    assert_eq!(change.kind, SKILL_BLOB_KIND_DELETE);
}

#[tokio::test]
async fn stale_pool_skill_delete_does_not_remove_newer_skill_files() {
    let data = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (store, key) = blob_context();
    let adapter = Arc::new(
        SkillBlobManifestSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_blob_store(store.clone(), key.clone()),
    );
    let remote_adapter = SkillBlobManifestSyncAdapter::new(remote.path().to_path_buf(), "device-a")
        .with_blob_store(store, key);
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let remote_dir = skill_dir(remote.path(), "writer");
    std::fs::create_dir_all(&remote_dir).unwrap();
    std::fs::write(remote_dir.join("SKILL.md"), "newer skill").unwrap();
    let newer_upsert = remote_adapter
        .snapshot_change_for_skill("writer", "op-newer-skill", "device-a", hlc(200))
        .await
        .unwrap();
    registry
        .apply_change(&newer_upsert, &merge_store)
        .await
        .unwrap();
    let stale_delete = SkillBlobManifestSyncAdapter::delete_change(
        "op-stale-delete",
        "writer",
        "device-z",
        hlc(100),
    );

    let result = registry
        .apply_change(&stale_delete, &merge_store)
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Skipped);
    assert_eq!(
        std::fs::read_to_string(skill_dir(data.path(), "writer").join("SKILL.md")).unwrap(),
        "newer skill"
    );
}

#[tokio::test]
async fn stale_workspace_skill_delete_does_not_remove_newer_skill_or_manifest_entry() {
    let data = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (store, key) = blob_context();
    let adapter = Arc::new(
        SkillBlobManifestSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_blob_store(store.clone(), key.clone()),
    );
    let remote_adapter = SkillBlobManifestSyncAdapter::new(remote.path().to_path_buf(), "device-a")
        .with_blob_store(store, key);
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let remote_dir = workspace_skill_dir(remote.path(), "agent1", "writer");
    std::fs::create_dir_all(&remote_dir).unwrap();
    std::fs::write(remote_dir.join("SKILL.md"), "newer workspace skill").unwrap();
    std::fs::write(
        remote.path().join("workspaces/agent1/skill.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "skills": {"writer": {"enabled": true, "config": {"tone": "newer"}}}
        }))
        .unwrap(),
    )
    .unwrap();
    let newer_upsert = remote_adapter
        .snapshot_change_for_workspace_skill(
            "agent1",
            "writer",
            "op-newer-workspace-skill",
            "device-a",
            hlc(200),
        )
        .await
        .unwrap();
    registry
        .apply_change(&newer_upsert, &merge_store)
        .await
        .unwrap();
    let stale_delete = SkillBlobManifestSyncAdapter::workspace_delete_change(
        "op-stale-workspace-delete",
        "agent1",
        "writer",
        "device-z",
        hlc(100),
    );

    let result = registry
        .apply_change(&stale_delete, &merge_store)
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Skipped);
    assert_eq!(
        std::fs::read_to_string(
            workspace_skill_dir(data.path(), "agent1", "writer").join("SKILL.md")
        )
        .unwrap(),
        "newer workspace skill"
    );
    let local_manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(data.path().join("workspaces/agent1/skill.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        local_manifest["skills"]["writer"]["config"]["tone"],
        "newer"
    );
}

#[tokio::test]
async fn duplicate_skill_delete_is_idempotent() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = adapter(data.path()).await;
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let dir = skill_dir(data.path(), "writer");
    std::fs::create_dir_all(&dir).unwrap();
    let change = SkillBlobManifestSyncAdapter::delete_change(
        "op-delete-same",
        "writer",
        "device-a",
        hlc(100),
    );

    let first = registry.apply_change(&change, &merge_store).await.unwrap();
    let second = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(first.status, RegistryApplyStatus::Applied);
    assert_eq!(second.status, RegistryApplyStatus::AlreadyApplied);
    assert!(!dir.exists());
}

#[tokio::test]
async fn duplicate_skill_upsert_is_idempotent() {
    let data = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (store, key) = blob_context();
    let adapter = Arc::new(
        SkillBlobManifestSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_blob_store(store.clone(), key.clone()),
    );
    let remote_adapter = SkillBlobManifestSyncAdapter::new(remote.path().to_path_buf(), "device-a")
        .with_blob_store(store, key);
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let remote_dir = skill_dir(remote.path(), "writer");
    std::fs::create_dir_all(&remote_dir).unwrap();
    std::fs::write(remote_dir.join("SKILL.md"), "remote skill").unwrap();
    let change = remote_adapter
        .snapshot_change_for_skill("writer", "op-same-upsert", "device-a", hlc(100))
        .await
        .unwrap();

    let first = registry.apply_change(&change, &merge_store).await.unwrap();
    let second = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(first.status, RegistryApplyStatus::Applied);
    assert_eq!(second.status, RegistryApplyStatus::AlreadyApplied);
    assert_eq!(
        std::fs::read_to_string(skill_dir(data.path(), "writer").join("SKILL.md")).unwrap(),
        "remote skill"
    );
}

#[tokio::test]
async fn inbound_small_skill_file_uses_inline_ciphertext() {
    let data = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let source_store = iroh_blobs::store::mem::MemStore::default().into();
    let target_store = iroh_blobs::store::mem::MemStore::default().into();
    let key = SyncKey::from_bytes([9; 32]);
    let adapter = Arc::new(
        SkillBlobManifestSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_blob_store(target_store, key.clone()),
    );
    let remote_adapter = SkillBlobManifestSyncAdapter::new(remote.path().to_path_buf(), "device-a")
        .with_blob_store(source_store, key);
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let remote_dir = skill_dir(remote.path(), "writer");
    std::fs::create_dir_all(&remote_dir).unwrap();
    std::fs::write(remote_dir.join("SKILL.md"), "inline content").unwrap();
    let change = remote_adapter
        .snapshot_change_for_skill("writer", "op-inline", "device-a", hlc(100))
        .await
        .unwrap();
    let manifest: SkillBlobManifest = serde_json::from_value(change.payload.clone()).unwrap();
    assert_eq!(manifest.files[0].hash_ref, "");
    assert!(manifest.files[0].inline_ciphertext_b64.is_some());

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert_eq!(
        std::fs::read_to_string(skill_dir(data.path(), "writer").join("SKILL.md")).unwrap(),
        "inline content"
    );
}

#[tokio::test]
async fn inbound_skill_file_with_inline_content_does_not_read_blob_ref() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let target_store: rua_sync::endpoint::BlobsApiStore =
        iroh_blobs::store::mem::MemStore::default().into();
    let key = SyncKey::from_bytes([9; 32]);
    let corrupt_tag = target_store
        .add_bytes(bytes::Bytes::from_static(b"bad"))
        .await
        .unwrap();
    let endpoint = iroh::Endpoint::builder(presets::Minimal)
        .bind()
        .await
        .unwrap();
    let downloader = StatefulBlobDownloader::new(target_store.downloader(&endpoint), endpoint);
    let adapter = Arc::new(
        SkillBlobManifestSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_blob_store(target_store, key.clone())
            .with_blob_download(downloader, Vec::new()),
    );
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let inline_content = b"inline wins over legacy blob ref";
    let inline_ciphertext_b64 = base64::engine::general_purpose::STANDARD
        .encode(rua_sync::crypto::symmetric_encrypt(inline_content, &key).unwrap());
    let manifest = SkillBlobManifest {
        scope: "pool".to_string(),
        agent_id: None,
        skill_name: "writer".to_string(),
        manifest_entry: None,
        files: vec![SkillBlobFileRef {
            rel_path: "SKILL.md".to_string(),
            hash_ref: corrupt_tag.hash.to_hex(),
            size: inline_content.len() as u64,
            is_executable: false,
            is_text: true,
            inline_ciphertext_b64: Some(inline_ciphertext_b64),
        }],
    };
    let change = SyncChange::new(
        "op-inline-read-error",
        SKILL_BLOB_DOMAIN,
        "writer",
        SKILL_BLOB_KIND_UPSERT,
        "device-a",
        hlc(100),
        serde_json::to_value(manifest).unwrap(),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert_eq!(
        std::fs::read_to_string(skill_dir(data.path(), "writer").join("SKILL.md")).unwrap(),
        "inline wins over legacy blob ref"
    );
}

#[tokio::test]
async fn missing_skill_blob_without_inline_does_not_create_partial_skill_dir() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = adapter(data.path()).await;
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let manifest = SkillBlobManifest {
        scope: "pool".to_string(),
        agent_id: None,
        skill_name: "writer".to_string(),
        manifest_entry: None,
        files: vec![SkillBlobFileRef {
            rel_path: "SKILL.md".to_string(),
            hash_ref: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            size: 7,
            is_executable: false,
            is_text: true,
            inline_ciphertext_b64: None,
        }],
    };
    let change = SyncChange::new(
        "op-missing-blob",
        SKILL_BLOB_DOMAIN,
        "writer",
        SKILL_BLOB_KIND_UPSERT,
        "device-a",
        hlc(100),
        serde_json::to_value(manifest).unwrap(),
    );

    let err = registry
        .apply_change(&change, &merge_store)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("skill blob missing"));
    assert!(!skill_dir(data.path(), "writer").exists());
}

#[tokio::test]
async fn invalid_skill_record_ids_are_rejected_without_touching_files() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = adapter(data.path()).await;
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let valid_dir = skill_dir(data.path(), "writer");
    std::fs::create_dir_all(&valid_dir).unwrap();
    std::fs::write(valid_dir.join("SKILL.md"), "local").unwrap();
    let change = SyncChange::new(
        "op-invalid-skill-record",
        SKILL_BLOB_DOMAIN,
        "../writer",
        SKILL_BLOB_KIND_DELETE,
        "device-a",
        hlc(100),
        serde_json::Value::Object(Default::default()),
    );

    let err = registry
        .apply_change(&change, &merge_store)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("invalid skill name"));
    assert_eq!(
        std::fs::read_to_string(valid_dir.join("SKILL.md")).unwrap(),
        "local"
    );
}

#[tokio::test]
async fn invalid_workspace_skill_record_ids_are_rejected_without_touching_files() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = adapter(data.path()).await;
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let valid_dir = workspace_skill_dir(data.path(), "agent1", "writer");
    std::fs::create_dir_all(&valid_dir).unwrap();
    std::fs::write(valid_dir.join("SKILL.md"), "local").unwrap();
    let change = SyncChange::new(
        "op-invalid-workspace-record",
        SKILL_BLOB_DOMAIN,
        "workspace/../writer",
        SKILL_BLOB_KIND_DELETE,
        "device-a",
        hlc(100),
        serde_json::Value::Object(Default::default()),
    );

    let err = registry
        .apply_change(&change, &merge_store)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("invalid agent id"));
    assert_eq!(
        std::fs::read_to_string(valid_dir.join("SKILL.md")).unwrap(),
        "local"
    );
}

#[tokio::test]
async fn inbound_manifest_rejects_parent_relative_path_without_writing_outside_skill_dir() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (store, key) = blob_context();
    let adapter = Arc::new(
        SkillBlobManifestSyncAdapter::new(data.path().to_path_buf(), "device-b")
            .with_blob_store(store.clone(), key.clone()),
    );
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let hash_ref = put_encrypted_blob(&store, &key, b"escape").await.unwrap();
    let manifest = SkillBlobManifest {
        scope: "pool".to_string(),
        agent_id: None,
        skill_name: "writer".to_string(),
        manifest_entry: None,
        files: vec![SkillBlobFileRef {
            rel_path: "../escape.txt".to_string(),
            hash_ref,
            size: 6,
            is_executable: false,
            is_text: true,
            inline_ciphertext_b64: None,
        }],
    };
    let change = SyncChange::new(
        "op-bad-rel-path",
        SKILL_BLOB_DOMAIN,
        "writer",
        SKILL_BLOB_KIND_UPSERT,
        "device-a",
        hlc(100),
        serde_json::to_value(manifest).unwrap(),
    );

    let err = registry
        .apply_change(&change, &merge_store)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("invalid skill file path"));
    assert!(!data.path().join("escape.txt").exists());
}
