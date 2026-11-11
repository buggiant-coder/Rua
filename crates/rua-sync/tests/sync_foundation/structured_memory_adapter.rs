use std::sync::Arc;

use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::sidecar::RecordState;
use rua_sync::structured_memory_adapter::{
    StructuredMemorySyncAdapter, STRUCTURED_MEMORY_DOMAIN,
    STRUCTURED_MEMORY_KIND_RELATION_SNAPSHOT, STRUCTURED_MEMORY_KIND_SLOT_SNAPSHOT,
    STRUCTURED_MEMORY_KIND_SNAPSHOT, STRUCTURED_MEMORY_KIND_VISIBILITY_SNAPSHOT,
};
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

#[tokio::test]
async fn inbound_snapshot_upgrades_actual_pre_title_workspace_schema() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let record = write_memory(
        source.path(),
        "default",
        "Canonical memory after schema upgrade.",
    );
    let workspace = target.path().join("workspaces/default");
    std::fs::create_dir_all(&workspace).unwrap();
    let connection = rusqlite::Connection::open(workspace.join("memory.db")).unwrap();
    connection
        .execute_batch(include_str!(
            "../../../rua-memory/tests/fixtures/structured-memory-before-title.sql"
        ))
        .unwrap();
    drop(connection);
    let change = StructuredMemorySyncAdapter::snapshot_change(
        "legacy-schema-inbound",
        &record,
        "source",
        hlc(200),
    )
    .unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(StructuredMemorySyncAdapter::new(
            target.path().to_path_buf(),
            "target",
        )))
        .unwrap();
    let merge_store = MergeStore::open(&target.path().join("merge")).unwrap();

    let outcome = registry.apply_change(&change, &merge_store).await.expect(
        "inbound snapshot must migrate the existing pre-title database before canonical upsert",
    );

    assert_eq!(outcome.status, RegistryApplyStatus::Applied);
    let loaded = open_store(target.path(), "default")
        .get(rua_memory::MemoryGetRequest {
            ids: vec![record.id.clone()],
            include_inactive: false,
        })
        .unwrap();
    assert_eq!(loaded[0].title, record.title);
    assert_eq!(loaded[0].metadata, record.metadata);
}

#[tokio::test]
async fn visibility_before_legacy_memory_retries_after_parent_snapshot_upgrade() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let record = write_memory(source.path(), "default", "Shared legacy-schema memory.");
    share_memory(source.path(), "default", &record.id, "reviewer");
    let exported = StructuredMemorySyncAdapter::new(source.path().to_path_buf(), "source")
        .export_records()
        .await
        .unwrap();
    let memory = exported
        .iter()
        .find(|change| change.kind == STRUCTURED_MEMORY_KIND_SNAPSHOT)
        .unwrap();
    let visibility = exported
        .iter()
        .find(|change| change.kind == STRUCTURED_MEMORY_KIND_VISIBILITY_SNAPSHOT)
        .unwrap();
    let workspace = target.path().join("workspaces/default");
    std::fs::create_dir_all(&workspace).unwrap();
    let connection = rusqlite::Connection::open(workspace.join("memory.db")).unwrap();
    connection
        .execute_batch(include_str!(
            "../../../rua-memory/tests/fixtures/structured-memory-before-title.sql"
        ))
        .unwrap();
    drop(connection);
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(StructuredMemorySyncAdapter::new(
            target.path().to_path_buf(),
            "target",
        )))
        .unwrap();
    let merge_store = MergeStore::open(&target.path().join("merge")).unwrap();

    let error = registry
        .apply_change(visibility, &merge_store)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("FOREIGN KEY"),
        "expected missing parent constraint: {error}"
    );
    assert!(!merge_store.has_applied_op(&visibility.op_id).unwrap());
    registry.apply_change(memory, &merge_store).await.unwrap();
    assert_eq!(
        registry
            .apply_change(visibility, &merge_store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Applied
    );
    let grants = open_store(target.path(), "default")
        .list_visibility_for_agent("default")
        .unwrap();
    assert_eq!(grants[0].memory_id, record.id);
    assert_eq!(grants[0].grantee_agent_ids, vec!["reviewer"]);
}

fn write_memory(
    data_dir: &std::path::Path,
    agent_id: &str,
    content: &str,
) -> rua_memory::MemoryRecord {
    let workspace = data_dir.join("workspaces").join(agent_id);
    std::fs::create_dir_all(&workspace).expect("workspace");
    let mut store =
        rua_memory::StructuredMemoryStore::open(workspace.join("memory.db")).expect("store");
    store
        .write(rua_memory::MemoryWriteRequest {
            agent_id: agent_id.to_string(),
            kind: rua_memory::MemoryKind::Preference,
            scope: rua_memory::MemoryScope::User,
            title: "response_style".to_string(),
            content: content.to_string(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec!["feedback".to_string()],
            search_terms: vec!["concise feedback".to_string()],
            evidence: Some(rua_memory::MemorySourceEvidence {
                chat_id: Some("chat-a".to_string()),
                turn_id: Some("turn-a".to_string()),
                message_id: Some("message-a".to_string()),
                quote: Some(content.to_string()),
            }),
            metadata: serde_json::json!({
                "written_on_device_id": "device-a",
                "written_on_device_name_snapshot": "Laptop A",
                "written_on_host_profile": "desktop",
                "resource_device_id": "device-a",
                "resource_device_name_snapshot": "Laptop A",
            }),
        })
        .expect("write")
        .record
        .expect("record")
}

fn open_store(data_dir: &std::path::Path, agent_id: &str) -> rua_memory::StructuredMemoryStore {
    let workspace = data_dir.join("workspaces").join(agent_id);
    std::fs::create_dir_all(&workspace).expect("workspace");
    rua_memory::StructuredMemoryStore::open(workspace.join("memory.db")).expect("store")
}

fn add_relation(data_dir: &std::path::Path, agent_id: &str, relation: rua_memory::MemoryRelation) {
    let store = open_store(data_dir, agent_id);
    store.upsert_relation(&relation).expect("relation");
}

fn add_slot(
    data_dir: &std::path::Path,
    agent_id: &str,
    slot: rua_memory::MemorySlot,
    items: Vec<rua_memory::MemorySlotItem>,
) {
    let store = open_store(data_dir, agent_id);
    store.upsert_slot(&slot).expect("slot");
    store.set_slot_items(&slot.id, &items).expect("slot items");
}

fn share_memory(
    data_dir: &std::path::Path,
    agent_id: &str,
    memory_id: &rua_memory::MemoryId,
    grantee_agent_id: &str,
) -> rua_memory::MemoryVisibility {
    let store = open_store(data_dir, agent_id);
    store
        .replace_visibility(
            agent_id,
            memory_id,
            rua_memory::MemoryVisibilityUpdateRequest {
                mode: rua_memory::MemoryVisibilityMode::Selected,
                grantee_agent_ids: vec![grantee_agent_id.to_string()],
            },
            "user",
        )
        .expect("replace visibility")
        .expect("visibility")
}

#[tokio::test]
async fn export_snapshots_structured_memory_without_sqlite_payloads() {
    let data = tempfile::tempdir().unwrap();
    let record = write_memory(
        data.path(),
        "default",
        "User prefers concise engineering feedback.",
    );

    let adapter = StructuredMemorySyncAdapter::new(data.path().to_path_buf(), "device-a");
    let exported = adapter.export_records().await.unwrap();

    let memory_change = exported
        .iter()
        .find(|change| change.kind == STRUCTURED_MEMORY_KIND_SNAPSHOT)
        .expect("memory snapshot");
    assert_eq!(memory_change.domain, STRUCTURED_MEMORY_DOMAIN);
    assert_eq!(
        memory_change.record_id,
        format!("memory/default/{}", record.id)
    );
    assert_eq!(memory_change.payload["content"], record.content);
    assert!(memory_change.payload["terms"]
        .as_array()
        .expect("terms")
        .iter()
        .any(|term| term["term"] == "feedback"));
    assert!(memory_change.payload["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .any(|source| source["chat_id"] == "chat-a" && source["turn_id"] == "turn-a"));
    assert_eq!(memory_change.payload["written_on_device_id"], "device-a");
    assert_eq!(
        memory_change.payload["resource_device_name_snapshot"],
        "Laptop A"
    );
    assert!(exported.iter().any(|change| {
        change.kind == STRUCTURED_MEMORY_KIND_VISIBILITY_SNAPSHOT
            && change.record_id == format!("visibility/default/{}", record.id)
            && change.payload["mode"] == "private"
    }));
    let payload = serde_json::to_string(&memory_change.payload).unwrap();
    assert!(!payload.contains("memory.db"));
    assert!(!payload.contains("sqlite"));
    assert!(!payload.contains("memory_search_docs"));
    assert!(!payload.contains("embedding"));
}

#[tokio::test]
async fn inbound_snapshot_applies_through_structured_memory_store() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let record = write_memory(
        source.path(),
        "default",
        "User prefers concise engineering feedback.",
    );
    let source_adapter = StructuredMemorySyncAdapter::new(source.path().to_path_buf(), "device-a");
    let exported = source_adapter.export_records().await.unwrap();
    let change = exported
        .into_iter()
        .find(|change| change.kind == STRUCTURED_MEMORY_KIND_SNAPSHOT)
        .expect("memory snapshot");

    let target_adapter = Arc::new(StructuredMemorySyncAdapter::new(
        target.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(target_adapter).unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let cursor = merge_store
        .get_record_cursor(&format!("structured_memory:memory/default/{}", record.id))
        .unwrap()
        .expect("structured memory cursor");
    assert_eq!(cursor.state, RecordState::Live);
    assert_eq!(cursor.base_blob_hash, None);
    assert!(merge_store.has_applied_op(&change.op_id).unwrap());

    let store = rua_memory::StructuredMemoryStore::open(
        target
            .path()
            .join("workspaces")
            .join("default")
            .join("memory.db"),
    )
    .unwrap();
    let loaded = store
        .get(rua_memory::MemoryGetRequest {
            ids: vec![record.id.clone()],
            include_inactive: false,
        })
        .unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].content, record.content);
    assert_eq!(loaded[0].written_on_device_id.as_deref(), Some("device-a"));
    assert_eq!(
        loaded[0].resource_device_name_snapshot.as_deref(),
        Some("Laptop A")
    );
    assert_eq!(loaded[0].sources.len(), 1);
    let hits = store
        .search("default", "concise engineering", 5)
        .expect("search applied memory");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, record.id);
}

#[tokio::test]
async fn visibility_snapshots_roundtrip_and_clear_stale_grants() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let record = write_memory(source.path(), "agent-a", "Shared memory marker.");
    let visibility = share_memory(source.path(), "agent-a", &record.id, "agent-b");
    let source_adapter = StructuredMemorySyncAdapter::new(source.path().to_path_buf(), "device-a");
    let exported = source_adapter.export_records().await.unwrap();
    let visibility_change = exported
        .iter()
        .find(|change| change.kind == STRUCTURED_MEMORY_KIND_VISIBILITY_SNAPSHOT)
        .expect("visibility snapshot")
        .clone();
    assert_eq!(
        visibility_change.record_id,
        format!("visibility/agent-a/{}", record.id)
    );
    assert_eq!(visibility_change.payload["mode"], "selected");
    assert_eq!(visibility_change.payload["grantee_agent_ids"][0], "agent-b");

    let target_adapter = Arc::new(StructuredMemorySyncAdapter::new(
        target.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(target_adapter).unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    for change in &exported {
        registry.apply_change(change, &merge_store).await.unwrap();
    }

    let store = open_store(target.path(), "agent-a");
    let applied = store
        .visibility_for_memory("agent-a", &record.id)
        .unwrap()
        .expect("applied visibility");
    assert_eq!(applied.mode, rua_memory::MemoryVisibilityMode::Selected);
    assert_eq!(applied.grantee_agent_ids, vec!["agent-b"]);
    assert_eq!(
        merge_store
            .get_record_cursor(&format!(
                "structured_memory:visibility/agent-a/{}",
                record.id
            ))
            .unwrap()
            .expect("visibility cursor")
            .state,
        RecordState::Live
    );

    let private = rua_memory::MemoryVisibility {
        memory_id: record.id.clone(),
        owner_agent_id: "agent-a".into(),
        mode: rua_memory::MemoryVisibilityMode::Private,
        grantee_agent_ids: vec![],
        grants: vec![],
    };
    let private_change = StructuredMemorySyncAdapter::visibility_snapshot_change(
        "op-private",
        &private,
        "device-a",
        hlc(visibility.grants[0].created_at.timestamp_millis() as u64 + 1),
    )
    .unwrap();
    registry
        .apply_change(&private_change, &merge_store)
        .await
        .unwrap();
    let cleared = store
        .visibility_for_memory("agent-a", &record.id)
        .unwrap()
        .expect("cleared visibility");
    assert_eq!(cleared.mode, rua_memory::MemoryVisibilityMode::Private);
    assert!(cleared.grantee_agent_ids.is_empty());
}

#[tokio::test]
async fn relation_and_slot_snapshots_roundtrip_without_derived_indexes() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let old = write_memory(source.path(), "default", "Old package manager was npm.");
    let new = write_memory(source.path(), "default", "New package manager is pnpm.");
    let now = chrono::Utc::now();
    let relation = rua_memory::MemoryRelation {
        id: "rel-package-manager".into(),
        from_id: new.id.clone(),
        to_id: old.id.clone(),
        relation: "supersedes".into(),
        confidence: Some(1.0),
        created_by: "autodream".into(),
        created_at: now,
    };
    add_relation(source.path(), "default", relation.clone());
    let slot = rua_memory::MemorySlot {
        id: "slot-pinned".into(),
        agent_id: "default".into(),
        slot_key: "pinned_user_memory".into(),
        scope: "user".into(),
        size_limit: 2000,
        enabled: true,
        created_at: now,
        updated_at: now,
    };
    add_slot(
        source.path(),
        "default",
        slot.clone(),
        vec![rua_memory::MemorySlotItem {
            slot_id: slot.id.clone(),
            memory_id: new.id.clone(),
            render_order: 1,
            render_mode: "content_only".into(),
        }],
    );

    let source_adapter = StructuredMemorySyncAdapter::new(source.path().to_path_buf(), "device-a");
    let exported = source_adapter.export_records().await.unwrap();
    assert!(exported.iter().any(|change| {
        change.kind == STRUCTURED_MEMORY_KIND_RELATION_SNAPSHOT
            && change.record_id == "relation/default/rel-package-manager"
            && change.payload["relation"] == "supersedes"
    }));
    assert!(exported.iter().any(|change| {
        change.kind == STRUCTURED_MEMORY_KIND_SLOT_SNAPSHOT
            && change.record_id == "slot/default/slot-pinned"
            && change.payload["slot"]["slot_key"] == "pinned_user_memory"
    }));
    let exported_payload = serde_json::to_string(&exported).unwrap();
    assert!(!exported_payload.contains("memory_search_docs"));
    assert!(!exported_payload.contains("memory_vec.db"));

    let target_adapter = Arc::new(StructuredMemorySyncAdapter::new(
        target.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(target_adapter).unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    for change in &exported {
        registry.apply_change(change, &merge_store).await.unwrap();
    }

    let store = rua_memory::StructuredMemoryStore::open(
        target
            .path()
            .join("workspaces")
            .join("default")
            .join("memory.db"),
    )
    .unwrap();
    let loaded = store
        .get(rua_memory::MemoryGetRequest {
            ids: vec![new.id.clone()],
            include_inactive: true,
        })
        .unwrap();
    assert!(loaded[0].relations.iter().any(|rel| {
        rel.id == "rel-package-manager" && rel.relation == "supersedes" && rel.to_id == old.id
    }));
    let pinned = store.pinned_context_for_agent("default").unwrap();
    assert_eq!(pinned, vec!["New package manager is pnpm.".to_string()]);
    let hits = store.search("default", "pnpm", 5).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, new.id);
}

#[tokio::test]
async fn relation_and_slot_tombstones_apply_without_touching_memory_record() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let old = write_memory(data.path(), "default", "Old package manager was npm.");
    let new = write_memory(data.path(), "default", "New package manager is pnpm.");
    let now = chrono::Utc::now();
    let relation = rua_memory::MemoryRelation {
        id: "rel-delete-me".into(),
        from_id: new.id.clone(),
        to_id: old.id,
        relation: "related".into(),
        confidence: Some(0.7),
        created_by: "autodream".into(),
        created_at: now,
    };
    add_relation(data.path(), "default", relation);
    let slot = rua_memory::MemorySlot {
        id: "slot-delete-me".into(),
        agent_id: "default".into(),
        slot_key: "pinned_user_memory".into(),
        scope: "user".into(),
        size_limit: 2000,
        enabled: true,
        created_at: now,
        updated_at: now,
    };
    add_slot(
        data.path(),
        "default",
        slot,
        vec![rua_memory::MemorySlotItem {
            slot_id: "slot-delete-me".into(),
            memory_id: new.id.clone(),
            render_order: 1,
            render_mode: "content_only".into(),
        }],
    );

    let adapter = Arc::new(StructuredMemorySyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let relation_delete = StructuredMemorySyncAdapter::relation_delete_change(
        "op-delete-relation",
        "default",
        "rel-delete-me",
        "device-a",
        hlc(99),
    );
    let slot_delete = StructuredMemorySyncAdapter::slot_delete_change(
        "op-delete-slot",
        "default",
        "slot-delete-me",
        "device-a",
        hlc(100),
    );
    registry
        .apply_change(&relation_delete, &merge_store)
        .await
        .unwrap();
    registry
        .apply_change(&slot_delete, &merge_store)
        .await
        .unwrap();

    let store = open_store(data.path(), "default");
    let loaded = store
        .get(rua_memory::MemoryGetRequest {
            ids: vec![new.id.clone()],
            include_inactive: true,
        })
        .unwrap();
    assert_eq!(loaded.len(), 1, "memory record should remain");
    assert!(loaded[0].relations.is_empty());
    assert!(store
        .pinned_context_for_agent("default")
        .unwrap()
        .is_empty());
    assert_eq!(
        merge_store
            .get_record_cursor("structured_memory:relation/default/rel-delete-me")
            .unwrap()
            .expect("relation cursor")
            .state,
        RecordState::Tombstoned
    );
    assert_eq!(
        merge_store
            .get_record_cursor("structured_memory:slot/default/slot-delete-me")
            .unwrap()
            .expect("slot cursor")
            .state,
        RecordState::Tombstoned
    );
}

#[tokio::test]
async fn inbound_delete_marks_memory_deleted_without_removing_sqlite_file() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let record = write_memory(
        data.path(),
        "default",
        "User prefers concise engineering feedback.",
    );
    let db_path = data
        .path()
        .join("workspaces")
        .join("default")
        .join("memory.db");
    assert!(db_path.exists());

    let adapter = Arc::new(StructuredMemorySyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let change = StructuredMemorySyncAdapter::delete_change(
        "op-delete-memory",
        "default",
        &record.id.to_string(),
        "device-a",
        hlc(99),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let cursor = merge_store
        .get_record_cursor(&format!("structured_memory:memory/default/{}", record.id))
        .unwrap()
        .expect("structured memory tombstone cursor");
    assert_eq!(cursor.state, RecordState::Tombstoned);
    assert!(merge_store.has_applied_op(&change.op_id).unwrap());

    assert!(db_path.exists(), "delete must not remove the SQLite file");
    let store = rua_memory::StructuredMemoryStore::open(db_path).unwrap();
    let loaded = store
        .get(rua_memory::MemoryGetRequest {
            ids: vec![record.id],
            include_inactive: true,
        })
        .unwrap();
    assert_eq!(loaded[0].status, rua_memory::MemoryRecordStatus::Deleted);
    let hits = store
        .search("default", "concise engineering", 5)
        .expect("search deleted memory");
    assert!(
        hits.is_empty(),
        "deleted structured Memory must be removed from FTS search"
    );
}
