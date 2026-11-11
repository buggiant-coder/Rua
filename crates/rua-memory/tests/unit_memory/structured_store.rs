use rua_memory::{
    normalize_memory_title, MemoryBatchApplyRequest, MemoryBatchCreateRecord, MemoryBatchOperation,
    MemoryBatchRelation, MemoryGetRequest, MemoryKind, MemoryMaintenanceCandidateRequest,
    MemoryRecordStatus, MemoryScope, MemorySlot, MemorySlotItem, MemorySourceEvidence,
    MemoryVisibilityMode, MemoryVisibilityUpdateRequest, MemoryWriteRequest, StructuredMemoryStore,
    MEMORY_TITLE_MAX_CHARS,
};

#[test]
fn legacy_subject_schema_migrates_titles_preserving_rows_and_unknown_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(include_str!(
        "../fixtures/structured-memory-before-title.sql"
    ))
    .unwrap();
    conn.execute_batch("ALTER TABLE memory_records ADD COLUMN future_metadata TEXT;
        INSERT INTO memory_records (id, agent_id, scope, kind, subject, content, status, confidence, created_at, metadata_json, future_metadata)
        VALUES ('legacy-subject', 'agent-a', 'user', 'preference', '  Workstation\nPreference  ', 'Existing canonical content.', 'active', 0.9, '2026-01-01T00:00:00Z', '{\"future\":{\"kept\":true}}', 'unknown column value');
        INSERT INTO memory_records (id, agent_id, scope, kind, subject, content, status, confidence, created_at)
        VALUES ('legacy-null', 'agent-a', 'user', 'fact', NULL, 'Content provides the missing title.', 'active', 1.0, '2026-01-01T00:00:00Z');").unwrap();
    drop(conn);

    let store = StructuredMemoryStore::open(&path).expect("upgrade old subject schema");
    let records = store
        .get(MemoryGetRequest {
            ids: vec![
                rua_memory::MemoryId::new("legacy-subject"),
                rua_memory::MemoryId::new("legacy-null"),
            ],
            include_inactive: true,
        })
        .unwrap();
    assert_eq!(records[0].title, "Workstation Preference");
    assert_eq!(records[0].content, "Existing canonical content.");
    assert_eq!(
        records[0].metadata,
        serde_json::json!({"future": {"kept": true}})
    );
    assert_eq!(records[1].title, "Content provides the missing title.");
    assert_eq!(
        store.search("agent-a", "Workstation", 5).unwrap()[0].0.id.0,
        "legacy-subject"
    );
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let preserved: (String, String) = conn
        .query_row(
            "SELECT subject, future_metadata FROM memory_records WHERE id='legacy-subject'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        preserved,
        (
            "  Workstation\nPreference  ".into(),
            "unknown column value".into()
        )
    );
    conn.execute(
        "UPDATE memory_records SET title='Current canonical title' WHERE id='legacy-subject'",
        [],
    )
    .unwrap();
    drop(conn);
    let reopened = StructuredMemoryStore::open(&path).unwrap();
    assert_eq!(
        reopened
            .get(MemoryGetRequest {
                ids: vec![rua_memory::MemoryId::new("legacy-subject")],
                include_inactive: true
            })
            .unwrap()[0]
            .title,
        "Current canonical title"
    );
}

#[test]
fn failed_legacy_title_migration_rolls_back_schema_and_preserves_data() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("memory.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(include_str!(
        "../fixtures/structured-memory-before-title.sql"
    ))
    .unwrap();
    conn.execute_batch("INSERT INTO memory_records (id, agent_id, scope, kind, subject, content, status, confidence, created_at, metadata_json)
        VALUES ('preserved', 'agent-a', 'user', 'fact', 'Existing title', 'Existing content', 'active', 1, '2026-01-01T00:00:00Z', '{\"unknown\":42}');
        CREATE TRIGGER fail_upgrade BEFORE UPDATE ON memory_records BEGIN SELECT RAISE(ABORT, 'injected migration failure'); END;").unwrap();
    drop(conn);
    assert!(StructuredMemoryStore::open(&path).is_err());
    let conn = rusqlite::Connection::open(&path).unwrap();
    let columns = conn
        .prepare("PRAGMA table_info(memory_records)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(!columns.iter().any(|name| name == "title"));
    assert_eq!(
        conn.query_row(
            "SELECT metadata_json FROM memory_records WHERE id='preserved'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "{\"unknown\":42}"
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='memory_sources'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

fn write_memory(
    store: &mut StructuredMemoryStore,
    content: &str,
    confidence: f32,
    importance: f32,
) -> rua_memory::MemoryRecord {
    write_memory_with(
        store,
        content,
        Some("response_style".into()),
        serde_json::json!({
            "written_on_device_id": "device-a",
            "written_on_device_name_snapshot": "Workstation",
            "written_on_host_profile": "desktop",
            "resource_device_id": "device-a",
            "resource_device_name_snapshot": "Workstation",
        }),
        confidence,
        importance,
    )
}

fn write_memory_with(
    store: &mut StructuredMemoryStore,
    content: &str,
    title: Option<String>,
    metadata: serde_json::Value,
    confidence: f32,
    importance: f32,
) -> rua_memory::MemoryRecord {
    store
        .write(MemoryWriteRequest {
            title: title.unwrap_or_else(|| "Test memory".into()),
            agent_id: "agent-a".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: content.into(),
            confidence: Some(confidence),
            importance: Some(importance),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: Some(MemorySourceEvidence {
                chat_id: Some("chat-a".into()),
                turn_id: Some("turn-a".into()),
                message_id: Some("message-a".into()),
                quote: Some(content.into()),
            }),
            metadata,
        })
        .expect("write memory")
        .record
        .expect("record")
}

fn write_memory_for_agent(
    store: &mut StructuredMemoryStore,
    agent_id: &str,
    content: &str,
    title: Option<String>,
) -> rua_memory::MemoryRecord {
    store
        .write(MemoryWriteRequest {
            title: title.unwrap_or_else(|| "Test memory".into()),
            agent_id: agent_id.into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: content.into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .expect("write memory")
        .record
        .expect("record")
}

#[test]
fn memory_title_validation_enforces_single_line_unicode_limit() {
    assert_eq!(
        normalize_memory_title("  用户姓名  ").expect("valid title"),
        "用户姓名"
    );
    assert!(normalize_memory_title("first\nsecond").is_err());
    assert!(normalize_memory_title(&"记".repeat(MEMORY_TITLE_MAX_CHARS + 1)).is_err());
}

#[test]
fn structured_store_indexes_memory_title() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let record = store
        .write(MemoryWriteRequest {
            title: "家庭照片存储位置".into(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::PersonalFact,
            scope: MemoryScope::User,
            content: "The files are stored on the Windows workstation.".into(),
            confidence: Some(1.0),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec!["photo storage".into()],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .expect("write memory")
        .record
        .expect("record");

    let hits = store.search("agent-a", "家庭照片", 10).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, record.id);
}

#[test]
fn structured_store_fts_search_returns_active_records_and_reranks_metadata() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let low = write_memory(
        &mut store,
        "User accepts concise feedback when working through drafts.",
        0.2,
        0.1,
    );
    let high = write_memory(
        &mut store,
        "User strongly prefers concise feedback for engineering decisions.",
        0.9,
        0.8,
    );

    let hits = store
        .search("agent-a", "concise feedback", 10)
        .expect("search");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].0.id, high.id);
    assert_eq!(hits[1].0.id, low.id);
    assert!(hits[0].1 > hits[1].1);
}

#[test]
fn structured_store_search_excludes_inactive_records_by_default() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let record = write_memory(
        &mut store,
        "User prefers concise feedback for engineering decisions.",
        0.9,
        0.8,
    );

    assert!(store
        .set_status(&record.id, MemoryRecordStatus::Deleted)
        .expect("set status"));

    let hits = store
        .search("agent-a", "concise feedback", 10)
        .expect("search");
    assert!(hits.is_empty());

    let loaded = store
        .get(MemoryGetRequest {
            ids: vec![record.id],
            include_inactive: true,
        })
        .expect("get inactive");
    assert_eq!(loaded[0].status, MemoryRecordStatus::Deleted);
}

#[test]
fn structured_store_rebuilds_derived_fts_index_from_active_records() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let active = write_memory(
        &mut store,
        "User prefers concise feedback for engineering decisions.",
        0.9,
        0.8,
    );
    let deleted = write_memory(
        &mut store,
        "Deleted concise feedback memory should stay out of search.",
        0.9,
        0.8,
    );
    store
        .set_status(&deleted.id, MemoryRecordStatus::Deleted)
        .expect("delete");

    let rebuilt = store.rebuild_derived_indexes().expect("rebuild");
    assert_eq!(rebuilt, 1);

    let hits = store
        .search("agent-a", "concise feedback", 10)
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, active.id);
}

#[test]
fn structured_store_indexes_memory_search_terms_for_cross_language_lookup() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let record = write_memory_with(
        &mut store,
        "用户首选称呼是南玻。",
        None,
        serde_json::json!({
            "search_terms": ["user name", "username", "name", "名字", "姓名", "用户名"]
        }),
        1.0,
        0.8,
    );

    let hits = store.search("agent-a", "user name", 10).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, record.id);
}

#[test]
fn structured_store_persists_labels_and_search_terms_as_aggregate_terms() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let outcome = store
        .write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "用户的家庭照片在这台电脑的 D:\\Photos\\Family。".into(),
            confidence: Some(1.0),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec!["家庭照片".into(), "本地路径".into()],
            search_terms: vec!["family photos".into(), "photo folder".into(), "相册".into()],
            evidence: Some(MemorySourceEvidence {
                chat_id: Some("chat-photos".into()),
                turn_id: Some("turn-photos".into()),
                message_id: None,
                quote: None,
            }),
            metadata: serde_json::Value::Null,
        })
        .expect("write memory");
    let record = outcome.record.expect("record");

    let labels = record
        .terms
        .iter()
        .filter(|term| term.term_kind == rua_memory::MemoryTermKind::Label)
        .map(|term| term.term.as_str())
        .collect::<Vec<_>>();
    assert_eq!(labels, vec!["家庭照片", "本地路径"]);

    let search_terms = record
        .terms
        .iter()
        .filter(|term| term.term_kind == rua_memory::MemoryTermKind::SearchTerm)
        .map(|term| term.term.as_str())
        .collect::<Vec<_>>();
    assert!(search_terms.contains(&"family photos"));
    assert!(search_terms.contains(&"photo folder"));
    assert!(search_terms.contains(&"相册"));
    assert_eq!(record.sources[0].chat_id.as_deref(), Some("chat-photos"));
    assert_eq!(record.sources[0].turn_id.as_deref(), Some("turn-photos"));

    let hits = store.search("agent-a", "photo folder", 10).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, record.id);
}

#[test]
fn structured_store_creates_and_refreshes_search_projection() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let record = store
        .write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "User prefers concise engineering feedback.".into(),
            confidence: Some(1.0),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec!["feedback".into()],
            search_terms: vec!["brief engineering answer".into()],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .expect("write memory")
        .record
        .expect("record");

    let doc = store
        .get_search_doc(&record.id)
        .expect("search doc read")
        .expect("search doc");
    assert!(doc.lexical_text.contains("User prefers concise"));
    assert!(doc.lexical_text.contains("feedback"));
    assert!(doc.lexical_text.contains("brief engineering answer"));
    assert!(doc.semantic_text.contains("brief engineering answer"));
    assert!(!doc.content_hash.is_empty());
    let old_hash = doc.content_hash;

    let updated = store
        .update(rua_memory::MemoryUpdateRequest {
            title: "Updated memory".into(),
            search_terms: Some(vec!["direct response".into()]),
            id: record.id.clone(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "User prefers concise direct feedback.".into(),
            confidence: 0.9,
            importance: Some(0.7),
        })
        .expect("update memory")
        .expect("updated");
    let refreshed = store
        .get_search_doc(&updated.id)
        .expect("search doc read")
        .expect("search doc");
    assert!(refreshed.lexical_text.contains("concise direct feedback"));
    assert!(refreshed.lexical_text.contains("direct response"));
    assert!(!refreshed.lexical_text.contains("brief engineering answer"));
    assert_ne!(refreshed.content_hash, old_hash);
}

#[test]
fn structured_store_reindex_rebuilds_search_projection_from_aggregate_terms() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let record = store
        .write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "用户喜欢直接的工程结论。".into(),
            confidence: Some(1.0),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec!["工程结论".into()],
            search_terms: vec!["direct engineering answer".into()],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .expect("write memory")
        .record
        .expect("record");

    store.rebuild_derived_indexes().expect("reindex");
    let doc = store
        .get_search_doc(&record.id)
        .expect("search doc read")
        .expect("search doc");
    assert!(doc.lexical_text.contains("工程结论"));
    assert!(doc.lexical_text.contains("direct engineering answer"));

    let hits = store
        .search("agent-a", "direct engineering answer", 10)
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, record.id);
}

#[test]
fn structured_store_no_embedding_fuzzy_route_matches_aliases_stems_and_path_tokens() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let record = store
        .write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::ProjectFact,
            scope: MemoryScope::User,
            content: "The user keeps family photos under D:\\Photos\\FamilyArchive.".into(),
            confidence: Some(1.0),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec!["family pictures".into()],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .expect("write memory")
        .record
        .expect("record");

    let doc = store
        .get_search_doc(&record.id)
        .expect("search doc read")
        .expect("search doc");
    assert!(doc.fuzzy_text.contains("photo"));
    assert!(doc.fuzzy_text.contains("familyarchive"));
    assert!(doc.fuzzy_text.contains("picture"));

    let hits = store.search("agent-a", "photo", 10).expect("photo search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, record.id);

    let hits = store
        .search("agent-a", "familyarchive", 10)
        .expect("path token search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, record.id);

    let hits = store.search("agent-a", "picture", 10).expect("stem search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, record.id);
}

#[test]
fn structured_store_detects_local_path_resource_with_device_context() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let record = store
        .write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::ProjectFact,
            scope: MemoryScope::User,
            content: "我的家庭照片在 D:\\Photos\\Family 目录里。".into(),
            confidence: Some(1.0),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec!["家庭照片".into()],
            search_terms: vec!["family photos".into()],
            evidence: None,
            metadata: serde_json::json!({
                "written_on_device_id": "device-win",
                "written_on_device_name_snapshot": "Windows PC",
                "written_on_host_profile": "desktop",
                "resource_device_id": "device-win",
                "resource_device_name_snapshot": "Windows PC",
            }),
        })
        .expect("write memory")
        .record
        .expect("record");

    assert_eq!(record.resources.len(), 1);
    let resource = &record.resources[0];
    assert_eq!(resource.resource_kind, "local_path");
    assert_eq!(resource.locator, "D:\\Photos\\Family");
    assert_eq!(resource.normalized_locator, "d:/photos/family");
    assert_eq!(resource.resource_device_id.as_deref(), Some("device-win"));
    assert_eq!(
        resource.resource_device_name_snapshot.as_deref(),
        Some("Windows PC")
    );
    assert!(!resource.portable);

    let doc = store
        .get_search_doc(&record.id)
        .expect("search doc read")
        .expect("search doc");
    assert!(doc.resource_text.contains("D:\\Photos\\Family"));
    assert!(doc.resource_text.contains("Windows PC"));
    assert!(doc.lexical_text.contains("D:\\Photos\\Family"));

    let hits = store
        .search("agent-a", "photos family", 10)
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, record.id);
}

#[test]
fn structured_store_detects_named_device_local_resources() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let record = store
        .write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::ProjectFact,
            scope: MemoryScope::User,
            content: "手机相册里有家庭照片，本机模型文件也在这台设备上。".into(),
            confidence: Some(1.0),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::json!({
                "written_on_device_id": "device-phone",
                "written_on_device_name_snapshot": "Android Phone",
                "resource_device_id": "device-phone",
                "resource_device_name_snapshot": "Android Phone",
            }),
        })
        .expect("write memory")
        .record
        .expect("record");

    assert!(record.resources.iter().any(|resource| {
        resource.resource_kind == "photo_album"
            && resource.locator == "相册"
            && resource.resource_device_id.as_deref() == Some("device-phone")
    }));
    assert!(record.resources.iter().any(|resource| {
        resource.resource_kind == "model_file"
            && resource.locator == "模型文件"
            && resource.resource_device_name_snapshot.as_deref() == Some("Android Phone")
    }));
}

#[test]
fn structured_store_graph_route_expands_related_active_memories() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let alpha = write_memory(
        &mut store,
        "Alpha search seed should pull related context.",
        0.9,
        0.8,
    );
    let hidden = write_memory(
        &mut store,
        "Hidden related context has no query token.",
        0.9,
        0.8,
    );
    let packet = store
        .maintenance_candidates(MemoryMaintenanceCandidateRequest {
            agent_id: "agent-a".into(),
            limit: 5,
        })
        .expect("maintenance candidates");

    let outcome = store
        .batch_apply(MemoryBatchApplyRequest {
            agent_id: "agent-a".into(),
            base_revision: packet.base_revision,
            ops: vec![MemoryBatchOperation::Relate {
                relations: vec![MemoryBatchRelation {
                    from_id: alpha.id.clone(),
                    to_id: hidden.id.clone(),
                    relation: "related".into(),
                    confidence: Some(0.8),
                }],
            }],
        })
        .expect("batch apply");
    assert!(outcome.applied);

    let hits = store.search("agent-a", "alpha", 10).expect("search");
    let ids = hits
        .iter()
        .map(|(record, _)| record.id.clone())
        .collect::<Vec<_>>();
    assert!(ids.contains(&alpha.id), "direct hit missing: {ids:?}");
    assert!(ids.contains(&hidden.id), "related hit missing: {ids:?}");
}

#[test]
fn structured_store_superseded_query_routes_to_active_replacement() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let old = write_memory(
        &mut store,
        "Legacy yarn workflow should be replaced.",
        0.7,
        0.4,
    );
    let packet = store
        .maintenance_candidates(MemoryMaintenanceCandidateRequest {
            agent_id: "agent-a".into(),
            limit: 5,
        })
        .expect("maintenance candidates");
    let new_id = rua_memory::MemoryId::new("mem_new_package_manager");
    store
        .batch_apply(MemoryBatchApplyRequest {
            agent_id: "agent-a".into(),
            base_revision: packet.base_revision,
            ops: vec![MemoryBatchOperation::Create {
                record: Box::new(MemoryBatchCreateRecord {
                    title: "Test memory".into(),
                    id: Some(new_id.clone()),
                    kind: MemoryKind::Preference,
                    scope: MemoryScope::User,
                    content: "Use pnpm for the package manager workflow.".into(),
                    confidence: Some(0.9),
                    importance: Some(0.8),
                    labels: vec![],
                    search_terms: vec![],
                    metadata: serde_json::Value::Null,
                    sources: vec![],
                }),
                supersedes: vec![old.id.clone()],
            }],
        })
        .expect("batch apply");

    let hits = store.search("agent-a", "legacy yarn", 10).expect("search");
    assert!(hits
        .iter()
        .any(|(record, _)| record.id == new_id && record.status == MemoryRecordStatus::Active));
    assert!(hits.iter().all(|(record, _)| record.id != old.id));
}

#[test]
fn structured_store_graph_route_expands_shared_resource_locator() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let seed = store
        .write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::ProjectFact,
            scope: MemoryScope::User,
            content: "Family photos are in D:\\Photos\\Family.".into(),
            confidence: Some(1.0),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec!["family photos".into()],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::json!({
                "written_on_device_id": "device-win",
                "written_on_device_name_snapshot": "Windows PC",
            }),
        })
        .expect("write seed")
        .record
        .expect("seed");
    let hidden = store
        .write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::ProjectFact,
            scope: MemoryScope::User,
            content: "The archive backup uses D:\\Photos\\Family.".into(),
            confidence: Some(1.0),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::json!({
                "written_on_device_id": "device-win",
                "written_on_device_name_snapshot": "Windows PC",
            }),
        })
        .expect("write hidden")
        .record
        .expect("hidden");

    let hits = store
        .search("agent-a", "family photos", 10)
        .expect("search");
    let ids = hits
        .iter()
        .map(|(record, _)| record.id.clone())
        .collect::<Vec<_>>();
    assert!(ids.contains(&seed.id));
    assert!(
        ids.contains(&hidden.id),
        "shared resource locator should expand graph route: {ids:?}"
    );
}

#[test]
fn structured_store_visibility_grants_control_cross_agent_search_and_get() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let owned = write_memory_for_agent(
        &mut store,
        "agent-a",
        "Owner-only alpha visibility memory.",
        Some("visibility".into()),
    );
    let shared = write_memory_for_agent(
        &mut store,
        "agent-a",
        "Shared beta visibility memory.",
        Some("visibility".into()),
    );

    assert!(store
        .search_visible_to_agent("agent-b", "visibility", 10)
        .expect("search before grant")
        .is_empty());

    let visibility = store
        .replace_visibility(
            "agent-a",
            &shared.id,
            MemoryVisibilityUpdateRequest {
                mode: MemoryVisibilityMode::Selected,
                grantee_agent_ids: vec!["agent-b".into()],
            },
            "user",
        )
        .expect("replace visibility")
        .expect("visibility");
    assert_eq!(visibility.mode, MemoryVisibilityMode::Selected);
    assert_eq!(visibility.grantee_agent_ids, vec!["agent-b"]);

    let hits = store
        .search_visible_to_agent("agent-b", "visibility", 10)
        .expect("search after grant");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, shared.id);

    let loaded = store
        .get_visible_to_agent(
            "agent-b",
            MemoryGetRequest {
                ids: vec![owned.id.clone(), shared.id.clone()],
                include_inactive: false,
            },
        )
        .expect("get visible");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, shared.id);
}

#[test]
fn structured_store_autodream_batch_rejects_foreign_memory_ids() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let owned = write_memory_for_agent(
        &mut store,
        "agent-a",
        "Owner memory for AutoDream.",
        Some("dream".into()),
    );
    let foreign = write_memory_for_agent(
        &mut store,
        "agent-b",
        "Foreign memory should stay read-only.",
        Some("dream".into()),
    );
    let packet = store
        .maintenance_candidates(MemoryMaintenanceCandidateRequest {
            agent_id: "agent-a".into(),
            limit: 5,
        })
        .expect("maintenance candidates");

    let err = store
        .batch_apply(MemoryBatchApplyRequest {
            agent_id: "agent-a".into(),
            base_revision: packet.base_revision,
            ops: vec![MemoryBatchOperation::Relate {
                relations: vec![MemoryBatchRelation {
                    from_id: owned.id,
                    to_id: foreign.id,
                    relation: "related".into(),
                    confidence: Some(0.8),
                }],
            }],
        })
        .expect_err("foreign relation must be rejected");
    assert!(err.to_string().contains("not agent-a"));
}

#[test]
fn structured_store_renders_enabled_pinned_memory_slots_with_device_context() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let record = write_memory_with(
        &mut store,
        "User's family photos are on this Windows PC.",
        Some("photos".into()),
        serde_json::json!({
            "written_on_device_id": "device-win",
            "written_on_device_name_snapshot": "Windows PC",
            "resource_device_id": "device-win",
            "resource_device_name_snapshot": "Windows PC",
        }),
        1.0,
        0.9,
    );
    let now = chrono::Utc::now();
    store
        .upsert_slot(&MemorySlot {
            id: "slot-pinned".into(),
            agent_id: "agent-a".into(),
            slot_key: "pinned_user_memory".into(),
            scope: "user".into(),
            size_limit: 2000,
            enabled: true,
            created_at: now,
            updated_at: now,
        })
        .expect("upsert slot");
    store
        .set_slot_items(
            "slot-pinned",
            &[MemorySlotItem {
                slot_id: "slot-pinned".into(),
                memory_id: record.id.clone(),
                render_order: 1,
                render_mode: "with_device".into(),
            }],
        )
        .expect("set slot items");

    let context = store
        .pinned_context_for_agent("agent-a")
        .expect("pinned context");
    assert_eq!(context.len(), 1);
    assert!(context[0].contains("family photos"));
    assert!(context[0].contains("Windows PC"));
    assert!(context[0].contains("device-win"));
}

#[test]
fn structured_store_generates_common_alias_terms_without_embedding() {
    let mut store = StructuredMemoryStore::open_in_memory().expect("store");
    let record = write_memory_with(
        &mut store,
        "用户名叫\"南玻\"。",
        None,
        serde_json::Value::Null,
        1.0,
        0.8,
    );

    let hits = store.search("agent-a", "user name", 10).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, record.id);

    let hits = store.search("agent-a", "怎么称呼用户", 10).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, record.id);
}
