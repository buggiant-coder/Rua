use std::sync::Arc;

use async_trait::async_trait;
use rua_agent::error::AgentError;
use rua_agent::memory_manager::{
    LlmMemoryManager, MemoryBatchApplyRequest, MemoryBatchCreateRecord, MemoryBatchOperation,
    MemoryBatchRelation, MemoryGetRequest, MemoryKind, MemoryMaintenanceCandidateRequest,
    MemoryManager, MemoryPublicationHook, MemoryRecord, MemoryRecordStatus, MemoryScope,
    MemorySearchRequest, MemorySourceEvidence, MemoryVisibilityUpdateRequest, MemoryWriteRequest,
};
use tempfile::TempDir;
use tokio::sync::Mutex;

struct StubSummaryGenerator;

#[async_trait]
impl rua_agent::compaction::SummaryGenerator for StubSummaryGenerator {
    async fn summarize(
        &self,
        messages: &[agent_types::ModelDialogueItem],
        _previous_summary: Option<&str>,
        _max_tokens: Option<usize>,
        _model: Option<&rua_session::CheckpointModelIdentity>,
    ) -> Result<String, AgentError> {
        Ok(messages
            .iter()
            .map(agent_types::ModelDialogueItem::text_content)
            .collect::<Vec<_>>()
            .join(" "))
    }
}

fn summary_gen() -> Arc<dyn rua_agent::compaction::SummaryGenerator> {
    Arc::new(StubSummaryGenerator)
}

#[derive(Default)]
struct RecordingPublicationHook {
    committed: Mutex<Vec<MemoryRecord>>,
    deleted: Mutex<Vec<(String, rua_memory::MemoryId)>>,
    fail_committed: bool,
}

#[async_trait]
impl MemoryPublicationHook for RecordingPublicationHook {
    async fn on_memory_records_committed(
        &self,
        records: &[MemoryRecord],
    ) -> Result<(), AgentError> {
        if self.fail_committed {
            return Err(AgentError::memory("publication failed"));
        }
        self.committed.lock().await.extend_from_slice(records);
        Ok(())
    }

    async fn on_memory_records_deleted(
        &self,
        agent_id: &str,
        memory_ids: &[rua_memory::MemoryId],
    ) -> Result<(), AgentError> {
        self.deleted.lock().await.extend(
            memory_ids
                .iter()
                .cloned()
                .map(|memory_id| (agent_id.to_string(), memory_id)),
        );
        Ok(())
    }
}

#[tokio::test]
async fn memory_write_creates_active_structured_record_with_evidence() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let manager = LlmMemoryManager::new(summary_gen()).with_structured_memory_store(store);

    let outcome = manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "User prefers concise engineering feedback.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: Some(MemorySourceEvidence {
                chat_id: Some("chat-1".into()),
                turn_id: Some("turn-1".into()),
                message_id: Some("message-1".into()),
                quote: Some("以后回答直接给工程结论。".into()),
            }),
            metadata: serde_json::json!({"source": "test"}),
        })
        .await
        .expect("memory write");

    assert!(!outcome.rejected, "durable preference should be accepted");
    let record = outcome.record.expect("created record");
    assert_eq!(record.status, MemoryRecordStatus::Active);
    assert_eq!(record.kind, MemoryKind::Preference);
    assert_eq!(record.scope, MemoryScope::User);
    assert_eq!(record.content, "User prefers concise engineering feedback.");
    assert_eq!(record.sources.len(), 1);
    assert_eq!(
        record.sources[0].quote.as_deref(),
        Some("以后回答直接给工程结论。")
    );

    let loaded = manager
        .memory_get(MemoryGetRequest {
            ids: vec![record.id.clone()],
            include_inactive: false,
        })
        .await
        .expect("memory get");

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, record.id);
    assert_eq!(loaded[0].status, MemoryRecordStatus::Active);
    assert_eq!(loaded[0].sources.len(), 1);
    assert_eq!(loaded[0].sources[0].chat_id.as_deref(), Some("chat-1"));
}

#[tokio::test]
async fn memory_write_rejects_empty_content_without_partial_record() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let manager = LlmMemoryManager::new(summary_gen()).with_structured_memory_store(store);

    let outcome = manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "   ".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: Some(MemorySourceEvidence {
                chat_id: Some("chat-1".into()),
                turn_id: Some("turn-1".into()),
                message_id: Some("message-1".into()),
                quote: Some("以后回答直接给工程结论。".into()),
            }),
            metadata: serde_json::json!({"source": "test"}),
        })
        .await
        .expect("memory write");

    assert!(outcome.rejected, "empty content should be rejected");
    assert!(outcome.record.is_none());

    let health = manager.health_check().await.expect("health");
    assert_eq!(health.document_count, 0);
}

#[tokio::test]
async fn memory_write_publishes_committed_structured_record_after_local_commit() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let hook = Arc::new(RecordingPublicationHook::default());
    let manager = LlmMemoryManager::new(summary_gen())
        .with_structured_memory_store(store)
        .with_publication_hook(hook.clone());

    let outcome = manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "User prefers concise engineering feedback.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("memory write");
    let record = outcome.record.expect("created record");

    let published = hook.committed.lock().await.clone();
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].id, record.id);
    assert_eq!(published[0].agent_id, "default");
    assert_eq!(
        published[0].content,
        "User prefers concise engineering feedback."
    );
}

#[tokio::test]
async fn publication_failure_does_not_roll_back_local_memory_write() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let hook = Arc::new(RecordingPublicationHook {
        fail_committed: true,
        ..Default::default()
    });
    let manager = LlmMemoryManager::new(summary_gen())
        .with_structured_memory_store(store)
        .with_publication_hook(hook);

    let record = manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "User prefers concise engineering feedback.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("memory write should not fail when publication fails")
        .record
        .expect("record");

    let loaded = manager
        .memory_get(MemoryGetRequest {
            ids: vec![record.id],
            include_inactive: false,
        })
        .await
        .expect("memory get");
    assert_eq!(loaded.len(), 1);
}

#[tokio::test]
async fn delete_memory_publishes_structured_memory_tombstone_after_local_commit() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let hook = Arc::new(RecordingPublicationHook::default());
    let manager = LlmMemoryManager::new(summary_gen())
        .with_structured_memory_store(store)
        .with_publication_hook(hook.clone());

    let record = manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "User prefers concise engineering feedback.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("memory write")
        .record
        .expect("record");

    assert!(manager
        .delete_memory(&record.id.to_string())
        .await
        .expect("delete memory"));

    let deleted = hook.deleted.lock().await.clone();
    assert_eq!(deleted, vec![("default".to_string(), record.id.clone())]);

    let loaded = manager
        .memory_get(MemoryGetRequest {
            ids: vec![record.id],
            include_inactive: true,
        })
        .await
        .expect("memory get");
    assert_eq!(loaded[0].status, MemoryRecordStatus::Deleted);
}

#[tokio::test]
async fn structured_memory_search_falls_back_to_vector_index_when_text_does_not_match() {
    let structured_store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let vec_store =
        sqlite_vecstore::VecStore::open_in_memory(sqlite_vecstore::VecStoreConfig::default())
            .expect("vecstore");
    let embedder: Arc<dyn rua_memory::Embedder> = Arc::new(rua_memory::MockEmbedder::new(32));
    let manager = LlmMemoryManager::new(summary_gen())
        .with_structured_memory_store(structured_store)
        .with_vec_store(vec_store, embedder);

    manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "User prefers concise engineering feedback.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("memory write");

    let hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "brief direct answers".into(),
            agent_id: "default".into(),
            top_k: 1,
        })
        .await
        .expect("memory search");

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source, "vector");
    *manager.embedder_slot().write().unwrap() = Some(Arc::new(rua_memory::MockEmbedder::new(64)));
    let mismatched = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::Precise,
            query: "brief direct answers".into(),
            agent_id: "default".into(),
            top_k: 1,
        })
        .await;
    assert!(mismatched
        .err()
        .is_some_and(|error| error.to_string().contains("index does not match")));
    let text = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::Grep,
            query: "concise engineering".into(),
            agent_id: "default".into(),
            top_k: 1,
        })
        .await
        .unwrap();
    assert_eq!(text.len(), 1, "original Memory remains available");
    assert!(hits[0]
        .metadata
        .get("id")
        .and_then(|value| value.as_str())
        .is_some_and(|id| id.starts_with("mem_")));
}

#[tokio::test]
async fn grep_memory_search_matches_expanded_search_terms() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let manager = LlmMemoryManager::new(summary_gen()).with_structured_memory_store(store);

    manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "User prefers concise engineering feedback.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec!["brief direct answers".into()],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("memory write");

    let hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::Grep,
            query: "direct answers".into(),
            agent_id: "default".into(),
            top_k: 10,
        })
        .await
        .expect("memory search");

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source, "grep");
    assert_eq!(
        hits[0].content,
        "User prefers concise engineering feedback."
    );
    assert_eq!(hits[0].metadata["matched_by"], serde_json::json!(["grep"]));
    assert_eq!(
        hits[0].metadata["matched_terms"],
        serde_json::json!(["direct", "answers"])
    );
}

#[tokio::test]
async fn grep_memory_search_does_not_use_vector_fallback() {
    let structured_store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let vec_store =
        sqlite_vecstore::VecStore::open_in_memory(sqlite_vecstore::VecStoreConfig::default())
            .expect("vecstore");
    let embedder: Arc<dyn rua_memory::Embedder> = Arc::new(rua_memory::MockEmbedder::new(32));
    let manager = LlmMemoryManager::new(summary_gen())
        .with_structured_memory_store(structured_store)
        .with_vec_store(vec_store, embedder);

    manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "User prefers concise engineering feedback.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("memory write");

    let hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::Grep,
            query: "brief direct answers".into(),
            agent_id: "default".into(),
            top_k: 10,
        })
        .await
        .expect("memory search");

    assert!(
        hits.is_empty(),
        "grep mode should not use vector-only semantic matches"
    );
}

#[tokio::test]
async fn manager_search_and_autodream_context_include_visible_foreign_memory_readonly() {
    let tmp = TempDir::new().expect("tempdir");
    let workspaces = tmp.path().join("workspaces");
    let agent_a_dir = workspaces.join("agent-a");
    let agent_b_dir = workspaces.join("agent-b");
    std::fs::create_dir_all(&agent_a_dir).expect("agent-a dir");
    std::fs::create_dir_all(&agent_b_dir).expect("agent-b dir");

    let manager = LlmMemoryManager::new(summary_gen())
        .with_structured_memory_store(
            rua_memory::StructuredMemoryStore::open(agent_a_dir.join("memory.db"))
                .expect("agent-a store"),
        )
        .with_workspace_memory_root(&workspaces);

    for content in [
        "Agent A prefers direct planning notes.",
        "Agent A prefers concise planning decisions.",
    ] {
        manager
            .memory_write(MemoryWriteRequest {
                title: "Test memory".into(),
                agent_id: "agent-a".into(),
                kind: MemoryKind::Preference,
                scope: MemoryScope::User,
                content: content.into(),
                confidence: Some(0.8),
                importance: Some(0.7),
                ttl_days: None,
                labels: vec![],
                search_terms: vec![],
                evidence: None,
                metadata: serde_json::Value::Null,
            })
            .await
            .expect("write owner memory");
    }

    let mut foreign_store = rua_memory::StructuredMemoryStore::open(agent_b_dir.join("memory.db"))
        .expect("agent-b store");
    let foreign = foreign_store
        .write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "agent-b".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "Agent B shared planning memory is readable by agent A.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .expect("write foreign memory")
        .record
        .expect("foreign record");
    foreign_store
        .replace_visibility(
            "agent-b",
            &foreign.id,
            MemoryVisibilityUpdateRequest {
                mode: rua_memory::MemoryVisibilityMode::Selected,
                grantee_agent_ids: vec!["agent-a".into()],
            },
            "user",
        )
        .expect("share foreign memory");
    drop(foreign_store);

    let hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            agent_id: "agent-a".into(),
            query: "shared planning".into(),
            top_k: 10,
        })
        .await
        .expect("search");
    assert!(hits.iter().any(|hit| {
        hit.metadata["id"] == serde_json::json!(foreign.id)
            && hit.metadata["agent_id"] == serde_json::json!("agent-b")
    }));

    let loaded = manager
        .memory_get_for_agent(
            "agent-a",
            MemoryGetRequest {
                ids: vec![foreign.id.clone()],
                include_inactive: false,
            },
        )
        .await
        .expect("memory get for agent");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].agent_id, "agent-b");

    let packet = manager
        .maintenance_candidates(MemoryMaintenanceCandidateRequest {
            agent_id: "agent-a".into(),
            limit: 5,
        })
        .await
        .expect("maintenance candidates");
    assert_eq!(packet.clusters.len(), 1);
    assert!(packet.clusters[0]
        .external_context
        .iter()
        .any(|record| record.id == foreign.id && record.agent_id == "agent-b"));
}

#[tokio::test]
async fn maintenance_candidates_and_batch_apply_consolidate_without_deleting_sources() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let manager = LlmMemoryManager::new(summary_gen()).with_structured_memory_store(store);

    let mut old_ids = Vec::new();
    for content in [
        "User prefers concise feedback in engineering reviews.",
        "User prefers direct engineering conclusions.",
        "User prefers short answers for architecture discussions.",
    ] {
        let record = manager
            .memory_write(MemoryWriteRequest {
                title: "Test memory".into(),
                agent_id: "default".into(),
                kind: MemoryKind::Preference,
                scope: MemoryScope::User,
                content: content.into(),
                confidence: Some(0.8),
                importance: Some(0.7),
                ttl_days: None,
                labels: vec![],
                search_terms: vec![],
                evidence: None,
                metadata: serde_json::Value::Null,
            })
            .await
            .expect("write memory")
            .record
            .expect("record");
        old_ids.push(record.id);
    }

    let packet = manager
        .maintenance_candidates(MemoryMaintenanceCandidateRequest {
            agent_id: "default".into(),
            limit: 5,
        })
        .await
        .expect("maintenance candidates");
    assert_eq!(packet.clusters.len(), 1);
    assert_eq!(packet.clusters[0].records.len(), 3);

    let new_a = rua_memory::MemoryId::new("mem_consolidated_a");
    let new_b = rua_memory::MemoryId::new("mem_consolidated_b");
    let outcome = manager
        .memory_batch_apply(MemoryBatchApplyRequest {
            agent_id: "default".into(),
            base_revision: packet.base_revision,
            ops: vec![
                MemoryBatchOperation::Create {
                    record: Box::new(MemoryBatchCreateRecord {
                        title: "Test memory".into(),
                        id: Some(new_a.clone()),
                        kind: MemoryKind::Preference,
                        scope: MemoryScope::User,
                        content: "User prefers concise engineering feedback.".into(),
                        confidence: Some(0.9),
                        importance: Some(0.9),
                        labels: vec![],
                        search_terms: vec![],
                        metadata: serde_json::Value::Null,
                        sources: vec![],
                    }),
                    supersedes: old_ids[..2].to_vec(),
                },
                MemoryBatchOperation::Create {
                    record: Box::new(MemoryBatchCreateRecord {
                        title: "Test memory".into(),
                        id: Some(new_b.clone()),
                        kind: MemoryKind::Preference,
                        scope: MemoryScope::User,
                        content: "User prefers short architecture answers.".into(),
                        confidence: Some(0.9),
                        importance: Some(0.8),
                        labels: vec![],
                        search_terms: vec![],
                        metadata: serde_json::Value::Null,
                        sources: vec![],
                    }),
                    supersedes: vec![old_ids[2].clone()],
                },
            ],
        })
        .await
        .expect("batch apply");

    assert!(outcome.applied);
    assert_eq!(outcome.records.len(), 2);

    let loaded = manager
        .memory_get(MemoryGetRequest {
            ids: old_ids
                .iter()
                .cloned()
                .chain([new_a.clone(), new_b.clone()])
                .collect(),
            include_inactive: true,
        })
        .await
        .expect("memory get");
    let superseded = loaded
        .iter()
        .filter(|record| old_ids.contains(&record.id))
        .collect::<Vec<_>>();
    assert_eq!(superseded.len(), 3);
    assert!(superseded
        .iter()
        .all(|record| record.status == MemoryRecordStatus::Superseded));
    assert!(loaded
        .iter()
        .any(|record| record.id == new_a && record.status == MemoryRecordStatus::Active));
    assert!(loaded
        .iter()
        .any(|record| record.id == new_b && record.status == MemoryRecordStatus::Active));
    let new_a_loaded = loaded
        .iter()
        .find(|record| record.id == new_a)
        .expect("new_a loaded");
    assert!(new_a_loaded.relations.iter().any(|relation| {
        relation.relation == "supersedes" && old_ids[..2].contains(&relation.to_id)
    }));

    let active_hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "concise engineering feedback".into(),
            agent_id: "default".into(),
            top_k: 10,
        })
        .await
        .expect("search");
    assert!(active_hits.iter().all(|hit| hit
        .metadata
        .get("status")
        .and_then(|value| value.as_str())
        == Some("active")));
}

#[tokio::test]
async fn batch_apply_rejects_stale_base_revision_without_partial_records() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let manager = LlmMemoryManager::new(summary_gen()).with_structured_memory_store(store);

    let outcome = manager
        .memory_batch_apply(MemoryBatchApplyRequest {
            agent_id: "default".into(),
            base_revision: "rev:stale".into(),
            ops: vec![MemoryBatchOperation::Create {
                record: Box::new(MemoryBatchCreateRecord {
                    title: "Test memory".into(),
                    id: Some(rua_memory::MemoryId::new("mem_should_not_exist")),
                    kind: MemoryKind::Preference,
                    scope: MemoryScope::User,
                    content: "This should not be inserted.".into(),
                    confidence: Some(0.9),
                    importance: Some(0.8),
                    labels: vec![],
                    search_terms: vec![],
                    metadata: serde_json::Value::Null,
                    sources: vec![],
                }),
                supersedes: vec![],
            }],
        })
        .await
        .expect("batch apply");

    assert!(!outcome.applied);
    let loaded = manager
        .memory_get(MemoryGetRequest {
            ids: vec![rua_memory::MemoryId::new("mem_should_not_exist")],
            include_inactive: true,
        })
        .await
        .expect("memory get");
    assert!(loaded.is_empty());
}

#[tokio::test]
async fn batch_apply_adds_relations_sets_status_and_surfaces_conflicts() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let manager = LlmMemoryManager::new(summary_gen()).with_structured_memory_store(store);

    let bun = manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::ProjectFact,
            scope: MemoryScope::User,
            content: "Project should use Bun for local scripts.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("write bun")
        .record
        .expect("bun record");
    let npm = manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::ProjectFact,
            scope: MemoryScope::User,
            content: "Project should use npm for local scripts.".into(),
            confidence: Some(0.7),
            importance: Some(0.5),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("write npm")
        .record
        .expect("npm record");

    let packet = manager
        .maintenance_candidates(MemoryMaintenanceCandidateRequest {
            agent_id: "default".into(),
            limit: 5,
        })
        .await
        .expect("maintenance candidates");

    let outcome = manager
        .memory_batch_apply(MemoryBatchApplyRequest {
            agent_id: "default".into(),
            base_revision: packet.base_revision,
            ops: vec![
                MemoryBatchOperation::Relate {
                    relations: vec![
                        MemoryBatchRelation {
                            from_id: bun.id.clone(),
                            to_id: npm.id.clone(),
                            relation: "contradicts".into(),
                            confidence: Some(0.95),
                        },
                        MemoryBatchRelation {
                            from_id: bun.id.clone(),
                            to_id: npm.id.clone(),
                            relation: "same_title".into(),
                            confidence: Some(1.0),
                        },
                    ],
                },
                MemoryBatchOperation::SetStatus {
                    ids: vec![npm.id.clone()],
                    status: MemoryRecordStatus::Expired,
                },
            ],
        })
        .await
        .expect("batch apply");
    assert!(outcome.applied);

    let loaded = manager
        .memory_get(MemoryGetRequest {
            ids: vec![bun.id.clone(), npm.id.clone()],
            include_inactive: true,
        })
        .await
        .expect("memory get");
    let bun_loaded = loaded
        .iter()
        .find(|record| record.id == bun.id)
        .expect("bun loaded");
    assert!(bun_loaded
        .relations
        .iter()
        .any(|relation| relation.relation == "contradicts" && relation.to_id == npm.id));
    let npm_loaded = loaded
        .iter()
        .find(|record| record.id == npm.id)
        .expect("npm loaded");
    assert_eq!(npm_loaded.status, MemoryRecordStatus::Expired);

    let active_hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "npm local scripts".into(),
            agent_id: "default".into(),
            top_k: 10,
        })
        .await
        .expect("search");
    assert!(
        active_hits.iter().all(
            |hit| hit.metadata.get("id").and_then(|value| value.as_str())
                != Some(npm.id.0.as_str())
        ),
        "expired Memory should not be in default search: {active_hits:#?}"
    );

    let conflict_hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "Bun local scripts".into(),
            agent_id: "default".into(),
            top_k: 10,
        })
        .await
        .expect("search");
    let bun_hit = conflict_hits
        .iter()
        .find(|hit| hit.content.contains("Bun"))
        .expect("bun hit");
    assert!(!bun_hit
        .metadata
        .get("conflicts")
        .and_then(|value| value.as_array())
        .expect("conflicts")
        .is_empty());
}

#[tokio::test]
async fn load_context_renders_enabled_pinned_memory_slots() {
    let mut store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let record = store
        .write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "Family photos live on the Windows PC.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::json!({
                "written_on_device_id": "device-win",
                "written_on_device_name_snapshot": "Windows PC",
                "resource_device_id": "device-win",
                "resource_device_name_snapshot": "Windows PC",
            }),
        })
        .expect("write memory")
        .record
        .expect("record");
    let now = chrono::Utc::now();
    store
        .upsert_slot(&rua_memory::MemorySlot {
            id: "slot-default".into(),
            agent_id: "default".into(),
            slot_key: "pinned_user_memory".into(),
            scope: "user".into(),
            size_limit: 2000,
            enabled: true,
            created_at: now,
            updated_at: now,
        })
        .expect("slot");
    store
        .set_slot_items(
            "slot-default",
            &[rua_memory::MemorySlotItem {
                slot_id: "slot-default".into(),
                memory_id: record.id,
                render_order: 1,
                render_mode: "with_device".into(),
            }],
        )
        .expect("slot item");
    let manager = LlmMemoryManager::new(summary_gen()).with_structured_memory_store(store);

    let context = manager
        .load_context(rua_agent::memory_manager::LoadContextRequest {
            agent_id: "default".into(),
        })
        .await
        .expect("load context");

    assert_eq!(context.recent_items.len(), 1);
    assert!(context.recent_items[0].contains("Family photos"));
    assert!(context.recent_items[0].contains("Windows PC"));
}

#[tokio::test]
async fn memory_batch_apply_publishes_created_and_superseded_records() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let hook = Arc::new(RecordingPublicationHook::default());
    let manager = LlmMemoryManager::new(summary_gen())
        .with_structured_memory_store(store)
        .with_publication_hook(hook.clone());

    let mut old_ids = Vec::new();
    for content in [
        "User prefers concise feedback.",
        "User prefers direct conclusions.",
    ] {
        let record = manager
            .memory_write(MemoryWriteRequest {
                title: "Test memory".into(),
                agent_id: "default".into(),
                kind: MemoryKind::Preference,
                scope: MemoryScope::User,
                content: content.into(),
                confidence: Some(0.8),
                importance: Some(0.7),
                ttl_days: None,
                labels: vec![],
                search_terms: vec![],
                evidence: None,
                metadata: serde_json::Value::Null,
            })
            .await
            .expect("write memory")
            .record
            .expect("record");
        old_ids.push(record.id);
    }
    hook.committed.lock().await.clear();

    let packet = manager
        .maintenance_candidates(MemoryMaintenanceCandidateRequest {
            agent_id: "default".into(),
            limit: 5,
        })
        .await
        .expect("maintenance candidates");
    let new_id = rua_memory::MemoryId::new("mem_consolidated_response_style");
    let outcome = manager
        .memory_batch_apply(MemoryBatchApplyRequest {
            agent_id: "default".into(),
            base_revision: packet.base_revision,
            ops: vec![MemoryBatchOperation::Create {
                record: Box::new(MemoryBatchCreateRecord {
                    title: "Test memory".into(),
                    id: Some(new_id.clone()),
                    kind: MemoryKind::Preference,
                    scope: MemoryScope::User,
                    content: "User prefers concise direct feedback.".into(),
                    confidence: Some(0.9),
                    importance: Some(0.9),
                    labels: vec![],
                    search_terms: vec![],
                    metadata: serde_json::Value::Null,
                    sources: vec![],
                }),
                supersedes: old_ids.clone(),
            }],
        })
        .await
        .expect("batch apply");

    assert!(outcome.applied);
    let published = hook.committed.lock().await.clone();
    let published_ids = published
        .iter()
        .map(|record| record.id.clone())
        .collect::<Vec<_>>();
    assert!(published_ids.contains(&new_id));
    for old_id in old_ids {
        assert!(published_ids.contains(&old_id));
        let old = published
            .iter()
            .find(|record| record.id == old_id)
            .expect("old record was published");
        assert_eq!(old.status, MemoryRecordStatus::Superseded);
    }
}

#[tokio::test]
async fn memory_batch_apply_publishes_relation_and_status_updates() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let hook = Arc::new(RecordingPublicationHook::default());
    let manager = LlmMemoryManager::new(summary_gen())
        .with_structured_memory_store(store)
        .with_publication_hook(hook.clone());

    let preferred = manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::ProjectFact,
            scope: MemoryScope::User,
            content: "Project should use Bun for local scripts.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("write preferred")
        .record
        .expect("preferred record");
    let old = manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "default".into(),
            kind: MemoryKind::ProjectFact,
            scope: MemoryScope::User,
            content: "Project should use npm for local scripts.".into(),
            confidence: Some(0.7),
            importance: Some(0.5),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("write old")
        .record
        .expect("old record");
    hook.committed.lock().await.clear();

    let packet = manager
        .maintenance_candidates(MemoryMaintenanceCandidateRequest {
            agent_id: "default".into(),
            limit: 5,
        })
        .await
        .expect("maintenance candidates");
    let outcome = manager
        .memory_batch_apply(MemoryBatchApplyRequest {
            agent_id: "default".into(),
            base_revision: packet.base_revision,
            ops: vec![
                MemoryBatchOperation::Relate {
                    relations: vec![MemoryBatchRelation {
                        from_id: preferred.id.clone(),
                        to_id: old.id.clone(),
                        relation: "supersedes".into(),
                        confidence: Some(0.95),
                    }],
                },
                MemoryBatchOperation::SetStatus {
                    ids: vec![old.id.clone()],
                    status: MemoryRecordStatus::Expired,
                },
            ],
        })
        .await
        .expect("batch apply");

    assert!(outcome.applied);
    let published = hook.committed.lock().await.clone();
    let preferred_published = published
        .iter()
        .find(|record| record.id == preferred.id)
        .expect("preferred record published");
    assert!(preferred_published
        .relations
        .iter()
        .any(|relation| relation.relation == "supersedes" && relation.to_id == old.id));
    let old_published = published
        .iter()
        .find(|record| record.id == old.id)
        .expect("old record published");
    assert_eq!(old_published.status, MemoryRecordStatus::Expired);
}
