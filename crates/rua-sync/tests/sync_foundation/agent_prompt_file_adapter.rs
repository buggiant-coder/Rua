use std::sync::Arc;

use rua_sync::agent_prompt_file_adapter::{
    AgentPromptFileSyncAdapter, AGENT_PROMPT_FILE_DOMAIN, AGENT_PROMPT_FILE_KIND_DELETE,
    AGENT_PROMPT_FILE_KIND_SNAPSHOT, AGENT_PROMPT_FILE_KIND_UPSERT,
};
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::{live_cursor, MergeStore};
use rua_sync::sidecar::RecordState;
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn root_file(data_dir: &std::path::Path, agent_id: &str, filename: &str) -> std::path::PathBuf {
    data_dir
        .join("workspaces")
        .join(agent_id)
        .join("prompts")
        .join(filename)
}

fn subdir_file(
    data_dir: &std::path::Path,
    agent_id: &str,
    subdir: &str,
    filename: &str,
) -> std::path::PathBuf {
    data_dir
        .join("workspaces")
        .join(agent_id)
        .join("prompts")
        .join(subdir)
        .join(filename)
}

#[tokio::test]
async fn export_reads_agent_prompt_root_markdown_files_only() {
    let data = tempfile::tempdir().unwrap();
    let prompt = root_file(data.path(), "default", "AGENT.md");
    std::fs::create_dir_all(prompt.parent().unwrap()).unwrap();
    std::fs::write(&prompt, "root prompt").unwrap();
    let memory_md = data.path().join("workspaces/default/MEMORY.md");
    std::fs::write(&memory_md, "removed memory").unwrap();
    let memory_dir = data.path().join("workspaces/default/MEMORY");
    std::fs::create_dir_all(&memory_dir).unwrap();
    std::fs::write(memory_dir.join("2026-06-26.md"), "removed daily memory").unwrap();

    let adapter = AgentPromptFileSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].domain, AGENT_PROMPT_FILE_DOMAIN);
    assert_eq!(exported[0].kind, AGENT_PROMPT_FILE_KIND_SNAPSHOT);
    assert_eq!(exported[0].record_id, "default/root/AGENT.md");
    assert_eq!(exported[0].payload["content"], "root prompt");
}

#[tokio::test]
async fn export_reads_agent_prompt_subdir_markdown_files() {
    let data = tempfile::tempdir().unwrap();
    let file = subdir_file(data.path(), "default", "AGENT", "DETAILS.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "nested prompt").unwrap();

    let adapter = AgentPromptFileSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].domain, AGENT_PROMPT_FILE_DOMAIN);
    assert_eq!(exported[0].kind, AGENT_PROMPT_FILE_KIND_SNAPSHOT);
    assert_eq!(exported[0].record_id, "default/subdir/AGENT/DETAILS.md");
    assert_eq!(exported[0].payload["content"], "nested prompt");
}

#[tokio::test]
async fn export_filters_untouched_seeded_workspace_prompt_files() {
    let data = tempfile::tempdir().unwrap();
    let seeded = root_file(data.path(), "default", "AGENT.md");
    let user = root_file(data.path(), "default", "USER.md");
    std::fs::create_dir_all(seeded.parent().unwrap()).unwrap();
    std::fs::write(&seeded, "seed prompt").unwrap();
    std::fs::write(&user, "user prompt").unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "agent:default",
        "agent_seed_catalog",
        &["workspaces/default/prompts/AGENT.md"],
    )
    .unwrap();

    let adapter = AgentPromptFileSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();
    let ids: Vec<_> = exported
        .iter()
        .map(|change| change.record_id.as_str())
        .collect();

    assert_eq!(ids, vec!["default/root/USER.md"]);
}

#[tokio::test]
async fn export_keeps_seeded_workspace_prompt_after_modification() {
    let data = tempfile::tempdir().unwrap();
    let seeded = root_file(data.path(), "default", "AGENT.md");
    std::fs::create_dir_all(seeded.parent().unwrap()).unwrap();
    std::fs::write(&seeded, "seed prompt").unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "agent:default",
        "agent_seed_catalog",
        &["workspaces/default/prompts/AGENT.md"],
    )
    .unwrap();
    std::fs::write(&seeded, "custom prompt").unwrap();

    let adapter = AgentPromptFileSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].record_id, "default/root/AGENT.md");
    assert_eq!(exported[0].payload["content"], "custom prompt");

    // Restoring the exact seed after deletion must still supersede the tombstone.
    let guard =
        rua_prompts::agent_files::AgentPromptFileStore::new(data.path().join("workspaces/default"))
            .lock()
            .await;
    guard.delete("AGENT.md").unwrap();
    guard.write("AGENT.md", "seed prompt", false).unwrap();
    drop(guard);
    let restored = adapter.export_records().await.unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].payload["content"], "seed prompt");
    assert!(!restored[0].payload["deleted_before"].is_null());
}

#[tokio::test]
async fn inresource_root_prompt_file_writes_workspace_prompt_file() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentPromptFileSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = AgentPromptFileSyncAdapter::root_upsert_change(
        "op-root-prompt",
        "default",
        "AGENT.md",
        "remote prompt",
        "device-a",
        hlc(25),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert_eq!(
        std::fs::read_to_string(root_file(data.path(), "default", "AGENT.md")).unwrap(),
        "remote prompt"
    );
}

#[tokio::test]
async fn inbound_memory_markdown_record_ids_are_rejected() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentPromptFileSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = rua_sync::opaque_replica_log::SyncChange::new(
        "op-memory-rejected",
        AGENT_PROMPT_FILE_DOMAIN,
        "default/MEMORY.md",
        AGENT_PROMPT_FILE_KIND_UPSERT,
        "device-a",
        hlc(25),
        serde_json::json!({"content": "removed memory"}),
    );

    let err = registry
        .apply_change(&change, &merge_store)
        .await
        .unwrap_err();

    assert!(err
        .to_string()
        .contains("invalid workspace markdown record id"));
    assert!(!data.path().join("workspaces/default/MEMORY.md").exists());
}

#[tokio::test]
async fn inbound_subdir_prompt_file_writes_workspace_prompt_subdir_file() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentPromptFileSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = AgentPromptFileSyncAdapter::subdir_upsert_change(
        "op-subdir-prompt",
        "default",
        "AGENT",
        "DETAILS.md",
        "remote nested prompt",
        "device-a",
        hlc(26),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert_eq!(
        std::fs::read_to_string(subdir_file(data.path(), "default", "AGENT", "DETAILS.md"))
            .unwrap(),
        "remote nested prompt"
    );
}

#[tokio::test]
async fn exported_prompt_applies_on_other_side_through_registry() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let file = root_file(source.path(), "default", "AGENT.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "source prompt").unwrap();

    let source_adapter = AgentPromptFileSyncAdapter::new(source.path().to_path_buf(), "device-a");
    let changes = source_adapter.export_records().await.unwrap();
    let target_adapter = Arc::new(AgentPromptFileSyncAdapter::new(
        target.path().to_path_buf(),
        "device-b",
    ));
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let mut registry = SyncRegistry::new();
    registry.register(target_adapter).unwrap();

    let result = registry
        .apply_change(&changes[0], &merge_store)
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert_eq!(
        std::fs::read_to_string(root_file(target.path(), "default", "AGENT.md")).unwrap(),
        "source prompt"
    );
}

#[tokio::test]
async fn upsert_uses_three_way_text_merge_when_base_exists() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentPromptFileSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let base = "line one\nline two\n";
    let base_hash = merge_store.put_base_snapshot(base.as_bytes()).unwrap();
    merge_store
        .set_record_cursor(
            "agent_prompt_file:default/root/AGENT.md",
            &live_cursor(hlc(10), Some(base_hash)),
        )
        .unwrap();
    let file = root_file(data.path(), "default", "AGENT.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "line one\nlocal prompt\nline two\n").unwrap();

    let change = AgentPromptFileSyncAdapter::root_upsert_change(
        "op-prompt-merge",
        "default",
        "AGENT.md",
        "line one\nline two\nremote prompt\n",
        "device-a",
        hlc(20),
    );
    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let merged = std::fs::read_to_string(&file).unwrap();
    assert!(merged.contains("local prompt"));
    assert!(merged.contains("remote prompt"));
}

#[tokio::test]
async fn delete_applies_tombstone_and_removes_prompt_file() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let file = root_file(data.path(), "default", "old.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "old").unwrap();

    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentPromptFileSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = AgentPromptFileSyncAdapter::root_delete_change(
        "op-delete",
        "default",
        "old.md",
        "device-a",
        hlc(30),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert!(!file.exists());
    let cursor = merge_store
        .get_record_cursor("agent_prompt_file:default/root/old.md")
        .unwrap()
        .unwrap();
    assert_eq!(cursor.state, RecordState::Tombstoned);
}

#[test]
fn root_and_subdir_change_helpers_use_agent_prompt_file_domain() {
    let root = AgentPromptFileSyncAdapter::root_upsert_change(
        "op-root",
        "default",
        "AGENT.md",
        "content",
        "device-a",
        hlc(70),
    );
    let subdir = AgentPromptFileSyncAdapter::subdir_delete_change(
        "op-subdir-delete",
        "default",
        "AGENT",
        "DETAILS.md",
        "device-a",
        hlc(71),
    );

    assert_eq!(root.domain, AGENT_PROMPT_FILE_DOMAIN);
    assert_eq!(root.kind, AGENT_PROMPT_FILE_KIND_UPSERT);
    assert_eq!(root.record_id, "default/root/AGENT.md");
    assert_eq!(subdir.domain, AGENT_PROMPT_FILE_DOMAIN);
    assert_eq!(subdir.kind, AGENT_PROMPT_FILE_KIND_DELETE);
    assert_eq!(subdir.record_id, "default/subdir/AGENT/DETAILS.md");
    assert!(!root.op_id.contains("AGENT.md"));
}

#[tokio::test]
async fn offline_rename_exports_stable_delete_and_recreation_rejects_delayed_delete() {
    use rua_prompts::agent_files::AgentPromptFileStore;
    let source = tempfile::tempdir().unwrap();
    let workspace = source.path().join("workspaces/default");
    let guard = AgentPromptFileStore::new(&workspace).lock().await;
    let original = guard.write("old.md", "original", false).unwrap();
    let initial = AgentPromptFileSyncAdapter::committed_change("default", original, "source");
    let renamed = guard.rename("old.md", "new.md").unwrap();
    let deleted =
        AgentPromptFileSyncAdapter::committed_change("default", renamed[0].clone(), "source");
    drop(guard);
    let adapter = AgentPromptFileSyncAdapter::new(source.path().into(), "source");
    let first_export = adapter.export_records().await.unwrap();
    let second_export = adapter.export_records().await.unwrap();
    assert_eq!(first_export.len(), 2);
    assert_eq!(
        first_export
            .iter()
            .find(|c| c.kind == "delete")
            .unwrap()
            .hlc,
        deleted.hlc
    );
    assert_eq!(
        first_export
            .iter()
            .map(|c| (&c.record_id, c.hlc))
            .collect::<Vec<_>>(),
        second_export
            .iter()
            .map(|c| (&c.record_id, c.hlc))
            .collect::<Vec<_>>()
    );

    let target = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge = MergeStore::open(merge_dir.path()).unwrap();
    let target_adapter = Arc::new(AgentPromptFileSyncAdapter::new(
        target.path().into(),
        "target",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(target_adapter.clone()).unwrap();
    registry.apply_change(&initial, &merge).await.unwrap();
    // Both rename records are recoverable even when neither was published live.
    for change in &first_export {
        registry.apply_change(change, &merge).await.unwrap();
    }
    assert!(!root_file(target.path(), "default", "old.md").exists());
    assert_eq!(
        std::fs::read_to_string(root_file(target.path(), "default", "new.md")).unwrap(),
        "original"
    );
    let reexport = target_adapter.export_records().await.unwrap();
    assert_eq!(
        reexport.iter().find(|c| c.kind == "delete").unwrap().hlc,
        deleted.hlc
    );

    let guard = AgentPromptFileStore::new(&workspace).lock().await;
    let rebuilt = guard.write("old.md", "rebuilt", false).unwrap();
    let rebuilt = AgentPromptFileSyncAdapter::committed_change("default", rebuilt, "source");
    drop(guard);
    registry.apply_change(&rebuilt, &merge).await.unwrap();
    let mut delayed = deleted;
    delayed.op_id = "delayed-delete".into();
    assert_eq!(
        registry
            .apply_change(&delayed, &merge)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    assert_eq!(
        std::fs::read_to_string(root_file(target.path(), "default", "old.md")).unwrap(),
        "rebuilt"
    );
}

#[tokio::test]
async fn offline_delete_blocks_stale_inbound_but_allows_newer_remote_recreation() {
    use rua_prompts::agent_files::AgentPromptFileStore;
    let data = tempfile::tempdir().unwrap();
    let guard = AgentPromptFileStore::new(data.path().join("workspaces/default"))
        .lock()
        .await;
    let initial = guard.write("note.md", "old", false).unwrap();
    let deletion = guard.delete("note.md").unwrap();
    drop(guard);
    let merge_dir = tempfile::tempdir().unwrap();
    let merge = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentPromptFileSyncAdapter::new(data.path().into(), "local"));
    let mut registry = SyncRegistry::new();
    registry.register(adapter.clone()).unwrap();
    let old = AgentPromptFileSyncAdapter::committed_change("default", initial, "remote");
    assert_eq!(
        registry.apply_change(&old, &merge).await.unwrap().status,
        RegistryApplyStatus::Skipped
    );
    let newer = AgentPromptFileSyncAdapter::root_upsert_change(
        "newer",
        "default",
        "note.md",
        "remote recreation",
        "remote",
        hlc(deletion.version.modified_ms + 1),
    );
    assert_eq!(
        registry.apply_change(&newer, &merge).await.unwrap().status,
        RegistryApplyStatus::Applied
    );
    let exported = adapter.export_records().await.unwrap();
    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].kind, "snapshot");
    assert_eq!(exported[0].hlc, newer.hlc);
    assert_eq!(exported[0].payload["content"], "remote recreation");
}

#[tokio::test]
async fn stale_sidecar_tombstone_does_not_block_delete_after_local_recreation() {
    use rua_prompts::agent_files::AgentPromptFileStore;
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge = MergeStore::open(merge_dir.path()).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(AgentPromptFileSyncAdapter::new(
            data.path().into(),
            "local",
        )))
        .unwrap();
    let old_delete = AgentPromptFileSyncAdapter::root_delete_change(
        "old-delete",
        "default",
        "note.md",
        "remote",
        hlc(100),
    );
    registry.apply_change(&old_delete, &merge).await.unwrap();
    let guard = AgentPromptFileStore::new(data.path().join("workspaces/default"))
        .lock()
        .await;
    let rebuilt = guard.write("note.md", "rebuilt", false).unwrap();
    drop(guard);
    let new_delete = AgentPromptFileSyncAdapter::root_delete_change(
        "new-delete",
        "default",
        "note.md",
        "remote",
        hlc(rebuilt.version.modified_ms + 1),
    );
    assert_eq!(
        registry
            .apply_change(&new_delete, &merge)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Applied
    );
    assert!(!root_file(data.path(), "default", "note.md").exists());
}

#[tokio::test]
async fn recreation_keeps_committed_deletion_fence_against_delayed_upserts() {
    use rua_prompts::agent_files::AgentPromptFileStore;
    for offline_delete in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let merge_dir = tempfile::tempdir().unwrap();
        let merge = MergeStore::open(merge_dir.path()).unwrap();
        let mut registry = SyncRegistry::new();
        registry
            .register(Arc::new(AgentPromptFileSyncAdapter::new(
                data.path().into(),
                "local",
            )))
            .unwrap();
        let store = AgentPromptFileStore::new(data.path().join("workspaces/default"));
        let guard = store.lock().await;
        let initial = guard.write("note.md", "old", false).unwrap();
        if offline_delete {
            guard.delete("note.md").unwrap();
        }
        drop(guard);
        if !offline_delete {
            let deletion = AgentPromptFileSyncAdapter::root_delete_change(
                "delete",
                "default",
                "note.md",
                "remote",
                hlc(initial.version.modified_ms + 1),
            );
            registry.apply_change(&deletion, &merge).await.unwrap();
        }
        let guard = store.lock().await;
        guard.write("note.md", "", false).unwrap();
        drop(guard);
        let old = AgentPromptFileSyncAdapter::committed_change("default", initial, "local");
        assert_eq!(
            registry.apply_change(&old, &merge).await.unwrap().status,
            RegistryApplyStatus::Skipped
        );
        assert_eq!(
            std::fs::read_to_string(root_file(data.path(), "default", "note.md")).unwrap(),
            ""
        );
    }
}

#[tokio::test]
async fn recreated_snapshot_carries_deletion_fence_across_devices_and_restart() {
    use rua_prompts::agent_files::AgentPromptFileStore;
    let source = tempfile::tempdir().unwrap();
    let guard = AgentPromptFileStore::new(source.path().join("workspaces/default"))
        .lock()
        .await;
    let initial = guard.write("note.md", "old content", false).unwrap();
    guard.delete("note.md").unwrap();
    let recreated = guard.write("note.md", "", false).unwrap();
    drop(guard);
    let initial = AgentPromptFileSyncAdapter::committed_change("default", initial, "source");
    let ordinary = AgentPromptFileSyncAdapter::committed_change("default", recreated, "source");
    let snapshot = AgentPromptFileSyncAdapter::new(source.path().into(), "source")
        .export_records()
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        ordinary.payload["deleted_before"],
        snapshot.payload["deleted_before"]
    );
    assert!(!snapshot.payload["deleted_before"].is_null());
    for seeded in [false, true] {
        let target = tempfile::tempdir().unwrap();
        let merge_dir = tempfile::tempdir().unwrap();
        let merge = MergeStore::open(merge_dir.path()).unwrap();
        let mut registry = SyncRegistry::new();
        registry
            .register(Arc::new(AgentPromptFileSyncAdapter::new(
                target.path().into(),
                "target",
            )))
            .unwrap();
        if seeded {
            registry.apply_change(&initial, &merge).await.unwrap();
        }
        registry.apply_change(&snapshot, &merge).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(root_file(target.path(), "default", "note.md")).unwrap(),
            ""
        );
        drop(registry);
        drop(merge);
        let merge = MergeStore::open(merge_dir.path()).unwrap();
        let adapter = Arc::new(AgentPromptFileSyncAdapter::new(
            target.path().into(),
            "target",
        ));
        let mut registry = SyncRegistry::new();
        registry.register(adapter.clone()).unwrap();
        let mut delayed = initial.clone();
        delayed.op_id = "delayed-old-write".into();
        assert_eq!(
            registry
                .apply_change(&delayed, &merge)
                .await
                .unwrap()
                .status,
            RegistryApplyStatus::Skipped
        );
        let forwarded = adapter.export_records().await.unwrap().remove(0);
        assert_eq!(
            forwarded.payload["deleted_before"],
            snapshot.payload["deleted_before"]
        );
        let third = tempfile::tempdir().unwrap();
        let third_merge_dir = tempfile::tempdir().unwrap();
        let third_merge = MergeStore::open(third_merge_dir.path()).unwrap();
        let mut third_registry = SyncRegistry::new();
        third_registry
            .register(Arc::new(AgentPromptFileSyncAdapter::new(
                third.path().into(),
                "third",
            )))
            .unwrap();
        third_registry
            .apply_change(&forwarded, &third_merge)
            .await
            .unwrap();
        assert_eq!(
            third_registry
                .apply_change(&delayed, &third_merge)
                .await
                .unwrap()
                .status,
            RegistryApplyStatus::Skipped
        );
        assert_eq!(
            std::fs::read_to_string(root_file(third.path(), "default", "note.md")).unwrap(),
            ""
        );
    }
}
