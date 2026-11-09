use std::sync::Arc;

use rua_sync::agent_template_adapter::{
    AgentTemplateSyncAdapter, AGENT_TEMPLATE_DOMAIN, AGENT_TEMPLATE_KIND_SNAPSHOT,
};
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

#[tokio::test]
async fn export_reads_agent_templates_as_v4_records() {
    let data = tempfile::tempdir().unwrap();
    rua_config::atomic_write_json(
        &data.path().join("agent_templates.json"),
        &serde_json::json!({
            "version": 1,
            "templates": {
                "default": {"name": "Default", "prompt_files": ["SOUL.md"]},
                "writer": {"name": "Writer", "prompt_files": ["WRITER.md"]}
            }
        }),
    )
    .unwrap();

    let adapter = AgentTemplateSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let changes = adapter.export_records().await.unwrap();
    let ids: Vec<_> = changes
        .iter()
        .map(|change| change.record_id.as_str())
        .collect();

    assert_eq!(ids, vec!["default", "writer"]);
    assert!(changes
        .iter()
        .all(|change| change.domain == AGENT_TEMPLATE_DOMAIN));
    assert!(changes
        .iter()
        .all(|change| change.kind == AGENT_TEMPLATE_KIND_SNAPSHOT));
}

#[tokio::test]
async fn export_filters_untouched_seeded_agent_template_file() {
    let data = tempfile::tempdir().unwrap();
    rua_config::atomic_write_json(
        &data.path().join("agent_templates.json"),
        &serde_json::json!({
            "version": 1,
            "templates": {
                "default": {"name": "Default", "prompt_files": ["SOUL.md"]}
            }
        }),
    )
    .unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "agent_templates:builtin",
        "agent_template_seed",
        &["agent_templates.json"],
    )
    .unwrap();

    let adapter = AgentTemplateSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let changes = adapter.export_records().await.unwrap();

    assert!(changes.is_empty());
}

#[tokio::test]
async fn export_keeps_seeded_agent_template_file_after_modification() {
    let data = tempfile::tempdir().unwrap();
    rua_config::atomic_write_json(
        &data.path().join("agent_templates.json"),
        &serde_json::json!({
            "version": 1,
            "templates": {
                "default": {"name": "Default", "prompt_files": ["SOUL.md"]}
            }
        }),
    )
    .unwrap();
    rua_core::seed_manifest::record_seed_entry(
        data.path(),
        "agent_templates:builtin",
        "agent_template_seed",
        &["agent_templates.json"],
    )
    .unwrap();
    rua_config::atomic_write_json(
        &data.path().join("agent_templates.json"),
        &serde_json::json!({
            "version": 1,
            "templates": {
                "default": {"name": "Customized Default", "prompt_files": ["SOUL.md"]}
            }
        }),
    )
    .unwrap();

    let adapter = AgentTemplateSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let changes = adapter.export_records().await.unwrap();

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].record_id, "default");
    assert_eq!(changes[0].payload["name"], "Customized Default");
}

#[tokio::test]
async fn inbound_upsert_replaces_template_by_lww() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    rua_config::atomic_write_json(
        &data.path().join("agent_templates.json"),
        &serde_json::json!({
            "version": 1,
            "templates": {
                "writer": {"name": "Local Writer", "description": "local"}
            }
        }),
    )
    .unwrap();
    let adapter = Arc::new(AgentTemplateSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change = AgentTemplateSyncAdapter::upsert_change(
        "op-template",
        "writer",
        serde_json::json!({"name": "Remote Writer", "prompt_files": ["WRITER.md"]}),
        "device-a",
        hlc(100),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let file: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(data.path().join("agent_templates.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(file["templates"]["writer"]["name"], "Remote Writer");
    assert!(file["templates"]["writer"]["description"].is_null());
    assert_eq!(file["templates"]["writer"]["prompt_files"][0], "WRITER.md");
    let cursor = merge_store
        .get_record_cursor("agent_template:writer")
        .unwrap()
        .expect("template cursor");
    assert_eq!(cursor.base_blob_hash, None);
}

#[tokio::test]
async fn inbound_upsert_uses_lww_without_base_snapshot() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(AgentTemplateSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let base_change = AgentTemplateSyncAdapter::upsert_change(
        "op-template-base",
        "writer",
        serde_json::json!({
            "name": "Writer",
            "description": "base description",
            "prompt_files": ["BASE.md"]
        }),
        "device-a",
        hlc(100),
    );
    registry
        .apply_change(&base_change, &merge_store)
        .await
        .unwrap();

    let path = data.path().join("agent_templates.json");
    let mut file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    file["templates"]["writer"]["description"] = serde_json::json!("local description");
    rua_config::atomic_write_json(&path, &file).unwrap();

    let remote_change = AgentTemplateSyncAdapter::upsert_change(
        "op-template-remote",
        "writer",
        serde_json::json!({
            "name": "Writer",
            "description": "base description",
            "prompt_files": ["REMOTE.md"]
        }),
        "device-a",
        hlc(200),
    );

    let result = registry
        .apply_change(&remote_change, &merge_store)
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        file["templates"]["writer"]["description"],
        "base description"
    );
    assert_eq!(file["templates"]["writer"]["prompt_files"][0], "REMOTE.md");
    let cursor = merge_store
        .get_record_cursor("agent_template:writer")
        .unwrap()
        .expect("template cursor");
    assert_eq!(cursor.base_blob_hash, None);
}

#[tokio::test]
async fn inbound_delete_removes_template() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    rua_config::atomic_write_json(
        &data.path().join("agent_templates.json"),
        &serde_json::json!({
            "version": 1,
            "templates": {
                "writer": {"name": "Writer"}
            }
        }),
    )
    .unwrap();
    let adapter = Arc::new(AgentTemplateSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let change =
        AgentTemplateSyncAdapter::delete_change("op-delete", "writer", "device-a", hlc(200));

    registry.apply_change(&change, &merge_store).await.unwrap();

    let file: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(data.path().join("agent_templates.json")).unwrap(),
    )
    .unwrap();
    assert!(file["templates"].get("writer").is_none());
}
