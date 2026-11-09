use std::sync::Arc;

use async_trait::async_trait;
use rua_agent::error::AgentError;
use rua_agent::memory_manager::*;

struct StubSummaryGenerator;

#[async_trait]
impl rua_agent::compaction::SummaryGenerator for StubSummaryGenerator {
    async fn summarize(
        &self,
        _messages: &[agent_types::ModelDialogueItem],
        _previous_summary: Option<&str>,
        _max_tokens: Option<usize>,
        _model: Option<&rua_session::CheckpointModelIdentity>,
    ) -> Result<String, AgentError> {
        Ok("stub summary".into())
    }
}

fn build_manager() -> LlmMemoryManager {
    let summary_gen: Arc<dyn rua_agent::compaction::SummaryGenerator> =
        Arc::new(StubSummaryGenerator);
    let structured_store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("structured store");
    let store =
        sqlite_vecstore::VecStore::open_in_memory(sqlite_vecstore::VecStoreConfig::default())
            .expect("in-memory store");
    let embedder: Arc<dyn rua_memory::Embedder> = Arc::new(rua_memory::MockEmbedder::new(32));

    LlmMemoryManager::new(summary_gen)
        .with_structured_memory_store(structured_store)
        .with_vec_store(store, embedder)
}

async fn write_memory(mm: &LlmMemoryManager, content: String) {
    mm.memory_write(MemoryWriteRequest {
        title: "Test memory".into(),
        agent_id: "a1".into(),
        kind: MemoryKind::ProjectFact,
        scope: MemoryScope::Project,
        content,
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
}

#[tokio::test]
async fn dream_memory_returns_nonempty_after_multiple_records() {
    let mm = build_manager();

    for i in 0..6 {
        write_memory(&mm, format!("Discussion about database migration step {i}")).await;
    }

    for i in 0..4 {
        write_memory(
            &mm,
            format!("Frontend React component refactoring task {i}"),
        )
        .await;
    }

    let result = mm
        .dream_memory(DreamMemoryRequest {
            agent_id: "a1".into(),
        })
        .await
        .unwrap();

    assert!(
        result.items_consolidated > 0,
        "should consolidate items, got {}",
        result.items_consolidated
    );
}

#[tokio::test]
async fn dream_memory_empty_store_returns_zero() {
    let mm = build_manager();

    let result = mm
        .dream_memory(DreamMemoryRequest {
            agent_id: "a1".into(),
        })
        .await
        .unwrap();

    assert_eq!(result.items_consolidated, 0);
    assert!(result.new_insights.is_empty());
}

#[tokio::test]
async fn dream_memory_without_vec_store_returns_zero() {
    let summary_gen: Arc<dyn rua_agent::compaction::SummaryGenerator> =
        Arc::new(StubSummaryGenerator);
    let mm = LlmMemoryManager::new(summary_gen);

    let result = mm
        .dream_memory(DreamMemoryRequest {
            agent_id: "a1".into(),
        })
        .await
        .unwrap();

    assert_eq!(result.items_consolidated, 0);
    assert!(result.new_insights.is_empty());
}
