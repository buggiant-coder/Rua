use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rua_agent::context_manager::{
    CompactContextRequest, ContextManager, ContextSummaryRequest,
    HistoryReplacementGenerationRequest, HistoryReplacementGenerationResult,
};
use rua_agent::error::AgentError;
use rua_agent::memory_manager::{
    LlmMemoryManager, MemoryKind, MemoryManager, MemoryScope, MemorySearchRequest,
    MemoryWriteRequest,
};

#[derive(Default)]
struct StubSummaryGenerator {
    models: Mutex<Vec<Option<rua_session::CheckpointModelIdentity>>>,
    history_replacement_models: Mutex<Vec<Option<rua_session::CheckpointModelIdentity>>>,
}

#[async_trait]
impl rua_agent::compaction::SummaryGenerator for StubSummaryGenerator {
    async fn summarize(
        &self,
        messages: &[agent_types::ModelDialogueItem],
        _previous_summary: Option<&str>,
        max_tokens: Option<usize>,
        model: Option<&rua_session::CheckpointModelIdentity>,
    ) -> Result<String, AgentError> {
        self.models.lock().unwrap().push(model.cloned());
        Ok(format!(
            "summary(max={max_tokens:?}): {}",
            messages
                .iter()
                .map(agent_types::ModelDialogueItem::text_content)
                .collect::<Vec<_>>()
                .join(" | ")
        ))
    }

    async fn generate_history_replacement(
        &self,
        request: &HistoryReplacementGenerationRequest,
    ) -> Result<HistoryReplacementGenerationResult, AgentError> {
        self.history_replacement_models
            .lock()
            .unwrap()
            .push(Some(request.model.clone()));
        Ok(HistoryReplacementGenerationResult {
            digest: format!("replacement(max={})", request.max_tokens),
            finish_reason: "stop".to_string(),
            usage: None,
        })
    }
}

fn summary_gen() -> Arc<dyn rua_agent::compaction::SummaryGenerator> {
    Arc::new(StubSummaryGenerator::default())
}

fn vecstore_manager() -> LlmMemoryManager {
    let structured_store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("structured store");
    let store =
        sqlite_vecstore::VecStore::open_in_memory(sqlite_vecstore::VecStoreConfig::default())
            .expect("in-memory vecstore");
    let embedder: Arc<dyn rua_memory::Embedder> = Arc::new(rua_memory::MockEmbedder::new(32));
    LlmMemoryManager::new(summary_gen())
        .with_structured_memory_store(structured_store)
        .with_vec_store(store, embedder)
}

fn text_only_vecstore_manager() -> LlmMemoryManager {
    let store =
        sqlite_vecstore::VecStore::open_in_memory(sqlite_vecstore::VecStoreConfig::default())
            .expect("in-memory vecstore");
    let embedder: Arc<dyn rua_memory::Embedder> = Arc::new(rua_memory::NullEmbedder);
    LlmMemoryManager::new(summary_gen()).with_vec_store(store, embedder)
}

async fn write_test_memory(manager: &LlmMemoryManager, agent_id: &str, content: &str) -> String {
    manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: agent_id.to_string(),
            kind: MemoryKind::ProjectFact,
            scope: MemoryScope::Project,
            content: content.to_string(),
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
        .expect("record")
        .id
        .to_string()
}

#[tokio::test]
async fn summarize_context_delegates_without_an_output_token_limit() {
    let generator = Arc::new(StubSummaryGenerator::default());
    let manager = LlmMemoryManager::new(generator.clone());
    let model = rua_session::CheckpointModelIdentity {
        provider: "provider-a".into(),
        model: "model-a".into(),
    };

    let result = manager
        .summarize_context(ContextSummaryRequest {
            session_id: "s1".into(),
            agent_id: "a1".into(),
            messages: vec![agent_types::ModelDialogueItem::text(
                agent_types::Role::User,
                "database migration plan",
            )],
            previous_summary: None,
            max_tokens: None,
            model: Some(model.clone()),
            model_call_context: None,
        })
        .await
        .expect("summary");

    assert_eq!(result.summary, "summary(max=None): database migration plan");
    assert_eq!(generator.models.lock().unwrap().as_slice(), &[Some(model)]);
}

#[tokio::test]
async fn summarize_context_forwards_an_output_token_limit() {
    let generator = Arc::new(StubSummaryGenerator::default());
    let manager = LlmMemoryManager::new(generator);

    let result = manager
        .summarize_context(ContextSummaryRequest {
            session_id: "s1".into(),
            agent_id: "a1".into(),
            messages: vec![agent_types::ModelDialogueItem::text(
                agent_types::Role::User,
                "bounded summary",
            )],
            previous_summary: None,
            max_tokens: Some(4_096),
            model: None,
            model_call_context: None,
        })
        .await
        .expect("summary");

    assert_eq!(result.summary, "summary(max=Some(4096)): bounded summary");
}

#[tokio::test]
async fn history_replacement_delegates_with_active_chat_model_and_4k_budget() {
    let generator = Arc::new(StubSummaryGenerator::default());
    let manager = LlmMemoryManager::new(generator.clone());
    let model = rua_session::CheckpointModelIdentity {
        provider: "provider-a".into(),
        model: "model-a".into(),
    };

    let result = manager
        .generate_history_replacement(HistoryReplacementGenerationRequest {
            session_id: "s1".into(),
            agent_id: "a1".into(),
            turn_id: "turn-1".into(),
            source_block_id: "hr-block-1".into(),
            usage_id: "usage-1".into(),
            source_content: "tool trace".into(),
            max_tokens: 4096,
            model: model.clone(),
            model_call_context: None,
        })
        .await
        .expect("history replacement");

    assert_eq!(result.digest, "replacement(max=4096)");
    assert_eq!(
        generator
            .history_replacement_models
            .lock()
            .unwrap()
            .as_slice(),
        &[Some(model)]
    );
}

#[tokio::test]
async fn summarize_context_does_not_derive_an_output_budget_from_context_settings() {
    let manager = LlmMemoryManager::new(summary_gen());

    let result = manager
        .summarize_context(ContextSummaryRequest {
            session_id: "s1".into(),
            agent_id: "a1".into(),
            messages: vec![agent_types::ModelDialogueItem::text(
                agent_types::Role::User,
                "database migration plan",
            )],
            previous_summary: None,
            max_tokens: None,
            model: None,
            model_call_context: None,
        })
        .await
        .expect("summary");

    assert_eq!(result.summary, "summary(max=None): database migration plan");
}

#[tokio::test]
async fn context_compaction_disabled_in_running_config_returns_noop() {
    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.context_compact_enabled = false;
    let manager = LlmMemoryManager::new(summary_gen());

    let messages = vec![
        serde_json::to_value(agent_types::ModelDialogueItem::text(
            agent_types::Role::User,
            "one",
        ))
        .unwrap(),
        serde_json::to_value(agent_types::ModelDialogueItem::text(
            agent_types::Role::Assistant,
            "two",
        ))
        .unwrap(),
        serde_json::to_value(agent_types::ModelDialogueItem::text(
            agent_types::Role::User,
            "three",
        ))
        .unwrap(),
    ];

    let compacted = manager
        .compact_context(CompactContextRequest {
            running_config: running,
            session_id: "s1".into(),
            agent_id: "a1".into(),
            messages: messages.clone(),
            token_budget: 100,
            existing_summary: None,
            model_call_context: None,
        })
        .await
        .expect("compact");
    assert_eq!(compacted.compacted_messages, messages);
    assert_eq!(compacted.tokens_saved, 0);

    let summarized = manager
        .summarize_context(ContextSummaryRequest {
            session_id: "s1".into(),
            agent_id: "a1".into(),
            messages: vec![agent_types::ModelDialogueItem::text(
                agent_types::Role::User,
                "database migration plan",
            )],
            previous_summary: None,
            max_tokens: None,
            model: None,
            model_call_context: None,
        })
        .await
        .expect("summary");
    assert!(
        !summarized.summary.is_empty(),
        "summary generation must not inherit a stale workspace compaction switch"
    );
}

#[tokio::test]
async fn structured_memory_search_honors_top_k_and_health_reports_document_count() {
    let manager = vecstore_manager();

    for i in 0..4 {
        write_test_memory(
            &manager,
            "a1",
            &format!("PostgreSQL migration checkpoint {i}"),
        )
        .await;
    }

    let health = manager.health_check().await.expect("health");
    assert!(health.healthy);
    assert_eq!(health.document_count, 4);
    assert!(health.issues.is_empty());

    let hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "PostgreSQL migration".into(),
            agent_id: "a1".into(),
            top_k: 2,
        })
        .await
        .expect("search");

    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|hit| hit
        .metadata
        .get("id")
        .and_then(|v| v.as_str())
        .is_some_and(|id| id.starts_with("mem_"))));
    assert!(hits
        .iter()
        .all(|hit| hit.content.contains("PostgreSQL migration checkpoint")));
}

#[tokio::test]
async fn delete_memory_removes_vecstore_document_and_search_result_by_id() {
    let manager = vecstore_manager();

    let memory_id = write_test_memory(&manager, "a1", "Delete contract marker alpha").await;

    let before = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "Delete contract marker alpha".into(),
            agent_id: "a1".into(),
            top_k: 1,
        })
        .await
        .expect("search before delete");
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].content, "Delete contract marker alpha");

    assert!(manager
        .delete_memory(&memory_id)
        .await
        .expect("delete existing memory"));
    assert!(!manager
        .delete_memory(&memory_id)
        .await
        .expect("delete missing memory"));

    let health = manager.health_check().await.expect("health after delete");
    assert_eq!(health.document_count, 0);

    let after = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "Delete contract marker alpha".into(),
            agent_id: "a1".into(),
            top_k: 10,
        })
        .await
        .expect("search after delete");
    assert!(after.is_empty(), "deleted memory should not be searchable");
}

#[tokio::test]
async fn manager_without_structured_store_or_vecstore_searches_empty_and_reindexes_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager = LlmMemoryManager::new(summary_gen());

    let hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "no vecstore".into(),
            agent_id: "a1".into(),
            top_k: 10,
        })
        .await
        .expect("search without vecstore");
    assert!(hits.is_empty());

    let health = manager.health_check().await.expect("health");
    assert!(health.healthy);
    assert_eq!(health.document_count, 0);

    assert_eq!(
        manager
            .reindex(tmp.path())
            .await
            .expect("reindex without vecstore"),
        0
    );
}

#[tokio::test]
async fn reindex_ignores_removed_memory_markdown_and_removes_stale_docs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace = tmp.path();
    let daily_dir = workspace.join("MEMORY");
    tokio::fs::create_dir_all(&daily_dir)
        .await
        .expect("create MEMORY dir");
    tokio::fs::write(
        workspace.join("MEMORY.md"),
        "# Memory\n\n## User Preferences\n\npineappletoken prefers compact answers.\n\n## Empty\n\n<!-- intentionally empty -->\n\n## Project Facts\n\nrustmemoryreindex tracks source markdown.\n",
    )
    .await
    .expect("write MEMORY.md");
    tokio::fs::write(
        daily_dir.join("2026-06-16.md"),
        "# Daily\n\n## Decisions\n\ndailyguavatoken moved reindex to derived vecstore data.\n",
    )
    .await
    .expect("write daily memory");

    let manager = text_only_vecstore_manager();

    assert_eq!(
        manager.reindex(workspace).await.expect("reindex workspace"),
        0
    );

    let health = manager.health_check().await.expect("health after reindex");
    assert_eq!(health.document_count, 0);

    let memory_hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "pineappletoken".into(),
            agent_id: "a1".into(),
            top_k: 5,
        })
        .await
        .expect("search removed MEMORY.md item");
    assert!(
        memory_hits.is_empty(),
        "Removed Memory Markdown must not be reindexed into long-term Memory"
    );

    let daily_hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "dailyguavatoken".into(),
            agent_id: "a1".into(),
            top_k: 5,
        })
        .await
        .expect("search daily item");
    assert!(
        daily_hits.is_empty(),
        "Removed daily Memory Markdown must not be reindexed into long-term Memory"
    );
}
