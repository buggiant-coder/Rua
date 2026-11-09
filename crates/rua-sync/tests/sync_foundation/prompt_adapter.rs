use std::sync::Arc;

use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::{live_cursor, MergeStore};
use rua_sync::prompt_adapter::{
    PromptLibrarySyncAdapter, PROMPT_DOMAIN, PROMPT_KIND_DELETE, PROMPT_KIND_SNAPSHOT,
    PROMPT_KIND_UPSERT,
};
use rua_sync::sidecar::RecordState;
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn prompt_file(data_dir: &std::path::Path, record_id: &str) -> std::path::PathBuf {
    data_dir.join("prompt_library").join(record_id)
}

#[tokio::test]
async fn export_reads_local_prompt_files_without_sync_state() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("prompt_library");
    std::fs::create_dir_all(root.join("GROUP")).unwrap();
    std::fs::write(root.join("a.md"), "A").unwrap();
    std::fs::write(root.join("GROUP").join("b.md"), "B").unwrap();

    let adapter = PromptLibrarySyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();
    let ids: Vec<_> = exported
        .iter()
        .map(|change| change.record_id.as_str())
        .collect();

    assert_eq!(ids, vec!["GROUP/b.md", "a.md"]);
    assert!(exported.iter().all(|change| change.domain == PROMPT_DOMAIN));
    assert!(exported
        .iter()
        .all(|change| change.kind == PROMPT_KIND_SNAPSHOT));
}

#[tokio::test]
async fn export_filters_untouched_seeded_prompt_files() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("prompt_library");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("seed.md"), "seed prompt").unwrap();
    std::fs::write(root.join("user.md"), "user prompt").unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "prompt_library:builtin",
        "builtin_prompt_library",
        &["prompt_library/seed.md"],
    )
    .unwrap();

    let adapter = PromptLibrarySyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();
    let ids: Vec<_> = exported
        .iter()
        .map(|change| change.record_id.as_str())
        .collect();

    assert_eq!(ids, vec!["user.md"]);
}

#[tokio::test]
async fn export_keeps_seeded_prompt_file_after_modification() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("prompt_library");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("seed.md"), "seed prompt").unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "prompt_library:builtin",
        "builtin_prompt_library",
        &["prompt_library/seed.md"],
    )
    .unwrap();
    std::fs::write(root.join("seed.md"), "custom prompt").unwrap();

    let adapter = PromptLibrarySyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].record_id, "seed.md");
    assert_eq!(exported[0].payload["content"], "custom prompt");
}

#[tokio::test]
async fn exported_prompt_applies_on_other_side_through_registry() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let source_root = source.path().join("prompt_library");
    std::fs::create_dir_all(&source_root).unwrap();
    std::fs::write(source_root.join("note.md"), "hello from source").unwrap();

    let source_adapter = PromptLibrarySyncAdapter::new(source.path().to_path_buf(), "device-a");
    let changes = source_adapter.export_records().await.unwrap();
    let target_adapter = Arc::new(PromptLibrarySyncAdapter::new(
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
        std::fs::read_to_string(prompt_file(target.path(), "note.md")).unwrap(),
        "hello from source"
    );
    assert!(merge_store.has_applied_op(&changes[0].op_id).unwrap());
}

#[tokio::test]
async fn upsert_uses_three_way_text_merge_when_base_exists() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(PromptLibrarySyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let base = "line one\nline two\n";
    let base_hash = merge_store.put_base_snapshot(base.as_bytes()).unwrap();
    merge_store
        .set_record_cursor("prompts:note.md", &live_cursor(hlc(10), Some(base_hash)))
        .unwrap();
    let file = prompt_file(data.path(), "note.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "line one local\nline two\n").unwrap();

    let change = PromptLibrarySyncAdapter::upsert_change(
        "op-merge",
        "note.md",
        "line one\nline two remote\n",
        "device-a",
        hlc(20),
    );
    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let merged = std::fs::read_to_string(&file).unwrap();
    assert!(merged.contains("line one local"));
    assert!(merged.contains("line two remote"));
}

#[tokio::test]
async fn delete_applies_tombstone_and_removes_file() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let file = prompt_file(data.path(), "old.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "old").unwrap();

    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(PromptLibrarySyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change =
        PromptLibrarySyncAdapter::delete_change("op-delete", "old.md", "device-a", hlc(30));

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert!(!file.exists());
    let cursor = merge_store
        .get_record_cursor("prompts:old.md")
        .unwrap()
        .unwrap();
    assert_eq!(cursor.state, RecordState::Tombstoned);
}

#[tokio::test]
async fn invalid_prompt_record_id_is_rejected() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(PromptLibrarySyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = PromptLibrarySyncAdapter::upsert_change(
        "op-bad",
        "../secret.md",
        "bad",
        "device-a",
        hlc(40),
    );

    let err = registry
        .apply_change(&change, &merge_store)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("invalid prompt record id"));
}

#[test]
fn prompt_change_helpers_do_not_put_filename_in_op_id() {
    let change = PromptLibrarySyncAdapter::upsert_change(
        PromptLibrarySyncAdapter::new_op_id(),
        "private-name.md",
        "content",
        "device-a",
        hlc(50),
    );

    assert_eq!(change.kind, PROMPT_KIND_UPSERT);
    assert!(!change.op_id.contains("private-name"));
    assert!(!change.op_id.contains(".md"));
}

#[test]
fn delete_change_uses_prompt_delete_kind() {
    let change =
        PromptLibrarySyncAdapter::delete_change("op-delete", "old.md", "device-a", hlc(60));

    assert_eq!(change.domain, PROMPT_DOMAIN);
    assert_eq!(change.kind, PROMPT_KIND_DELETE);
    assert_eq!(change.record_id, "old.md");
}

#[tokio::test]
async fn library_offline_rename_and_delete_survive_restart() {
    let data = tempfile::tempdir().unwrap();
    let lib = rua_prompts::PromptLibrary::new(data.path().into());
    lib.update_file("old", " original bytes ").await.unwrap();
    lib.rename_file("old", "new").await.unwrap();
    let adapter = PromptLibrarySyncAdapter::new(data.path().into(), "device-a");
    let records = adapter.export_records().await.unwrap();
    assert!(records
        .iter()
        .any(|c| c.record_id == "old.md" && c.kind == PROMPT_KIND_DELETE));
    assert!(records
        .iter()
        .any(|c| c.record_id == "new.md" && c.payload["content"] == " original bytes "));
    lib.delete_file("new").await.unwrap();
    let records = PromptLibrarySyncAdapter::new(data.path().into(), "device-a")
        .export_records()
        .await
        .unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|c| c.kind == PROMPT_KIND_DELETE));
}

#[tokio::test]
async fn library_explicit_restore_exports_unchanged_seed_content() {
    let data = tempfile::tempdir().unwrap();
    let lib = rua_prompts::PromptLibrary::new(data.path().into());
    let seeded = lib.restore_builtin_now().unwrap();
    let paths: Vec<_> = seeded
        .seeded
        .iter()
        .map(|e| e.relative_path.as_str())
        .collect();
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "prompt_library:builtin",
        "builtin_prompt_library",
        &paths,
    )
    .unwrap();
    let adapter = PromptLibrarySyncAdapter::new(data.path().into(), "device-a");
    assert!(adapter.export_records().await.unwrap().is_empty());
    lib.delete_file("10_AGENT_SAFETY").await.unwrap();
    lib.restore_builtin().await.unwrap();
    let records = adapter.export_records().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].record_id, "10_AGENT_SAFETY.md");
    assert_ne!(records[0].kind, PROMPT_KIND_DELETE);
}

#[tokio::test]
async fn library_recreation_fence_survives_inbound_and_reexport() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let lib = rua_prompts::PromptLibrary::new(source.path().into());
    lib.update_file("note", "old bytes").await.unwrap();
    let source_adapter = PromptLibrarySyncAdapter::new(source.path().into(), "device-a");
    let old = source_adapter.export_records().await.unwrap().remove(0);
    lib.delete_file("note").await.unwrap();
    let deletion = source_adapter.export_records().await.unwrap().remove(0);
    lib.update_file("note", "new bytes").await.unwrap();
    let recreated = source_adapter.export_records().await.unwrap().remove(0);
    assert!(recreated.payload.get("deleted_before").is_some());
    let target_adapter = Arc::new(PromptLibrarySyncAdapter::new(
        target.path().into(),
        "device-b",
    ));
    let mut registry = rua_sync::sync_registry::SyncRegistry::new();
    registry.register(target_adapter.clone()).unwrap();
    let merge = rua_sync::merge_store::MergeStore::open(&target.path().join("sync")).unwrap();
    registry.apply_change(&old, &merge).await.unwrap();
    registry.apply_change(&recreated, &merge).await.unwrap();
    registry.apply_change(&deletion, &merge).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(target.path().join("prompt_library/note.md")).unwrap(),
        "new bytes"
    );
    let reexport = target_adapter.export_records().await.unwrap().remove(0);
    assert_eq!(reexport.hlc, recreated.hlc);
    assert_eq!(
        reexport.payload["deleted_before"],
        recreated.payload["deleted_before"]
    );
}
