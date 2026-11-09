use std::sync::{Arc, Mutex};

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

struct SemanticNameEmbedder;

#[async_trait]
impl rua_memory::Embedder for SemanticNameEmbedder {
    async fn embed(&self, texts: &[String]) -> rua_memory::MemoryResult<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| {
                if text.contains("user name") || text.contains("南玻") {
                    vec![1.0, 0.0]
                } else {
                    vec![0.0, 1.0]
                }
            })
            .collect())
    }

    fn dimension(&self) -> usize {
        2
    }

    fn model_id(&self) -> &str {
        "semantic-name-test"
    }
}

#[derive(Default)]
struct RecordingEmbedder {
    inputs: Mutex<Vec<Vec<String>>>,
}

#[async_trait]
impl rua_memory::Embedder for RecordingEmbedder {
    async fn embed(&self, texts: &[String]) -> rua_memory::MemoryResult<Vec<Vec<f32>>> {
        self.inputs.lock().expect("inputs").push(texts.to_vec());
        Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
    }

    fn dimension(&self) -> usize {
        2
    }

    fn model_id(&self) -> &str {
        "recording"
    }
}

struct FailingEmbedder;

#[async_trait]
impl rua_memory::Embedder for FailingEmbedder {
    async fn embed(&self, _texts: &[String]) -> rua_memory::MemoryResult<Vec<Vec<f32>>> {
        Err(rua_memory::MemoryError::Invalid(
            "intentional embedding failure".into(),
        ))
    }

    fn dimension(&self) -> usize {
        2
    }

    fn model_id(&self) -> &str {
        "failing"
    }
}

struct ExactGuardEmbedder;

#[async_trait]
impl rua_memory::Embedder for ExactGuardEmbedder {
    async fn embed(&self, texts: &[String]) -> rua_memory::MemoryResult<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| {
                if text == "precise label" || text.contains("vector bait") {
                    vec![1.0, 0.0]
                } else {
                    vec![0.0, 1.0]
                }
            })
            .collect())
    }

    fn dimension(&self) -> usize {
        2
    }

    fn model_id(&self) -> &str {
        "exact-guard"
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

async fn write_memory(mm: &LlmMemoryManager, content: &str) {
    write_memory_with_metadata(mm, content, serde_json::Value::Null).await;
}

async fn write_memory_with_metadata(
    mm: &LlmMemoryManager,
    content: &str,
    metadata: serde_json::Value,
) {
    mm.memory_write(MemoryWriteRequest {
        title: "Test memory".into(),
        agent_id: "a1".into(),
        kind: MemoryKind::ProjectFact,
        scope: MemoryScope::Project,
        content: content.into(),
        confidence: Some(0.9),
        importance: Some(0.8),
        ttl_days: None,
        labels: vec![],
        search_terms: vec![],
        evidence: None,
        metadata,
    })
    .await
    .expect("memory write");
}

async fn write_memory_full(
    mm: &LlmMemoryManager,
    content: &str,
    labels: Vec<String>,
    search_terms: Vec<String>,
    metadata: serde_json::Value,
) {
    mm.memory_write(MemoryWriteRequest {
        title: "Test memory".into(),
        agent_id: "a1".into(),
        kind: MemoryKind::ProjectFact,
        scope: MemoryScope::Project,
        content: content.into(),
        confidence: Some(0.9),
        importance: Some(0.8),
        ttl_days: None,
        labels,
        search_terms,
        evidence: None,
        metadata,
    })
    .await
    .expect("memory write");
}

#[tokio::test]
async fn write_three_memories_then_search_recalls_related() {
    let mm = build_manager();

    for content in [
        "We need to migrate the database to PostgreSQL next quarter",
        "The PostgreSQL migration plan includes schema changes and data validation",
        "What about the frontend React component refactoring?",
    ] {
        write_memory(&mm, content).await;
    }

    let hits = mm
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "database PostgreSQL migration".into(),
            agent_id: "a1".into(),
            top_k: 10,
        })
        .await
        .unwrap();

    assert!(!hits.is_empty(), "search must return non-empty results");

    let stored_contents = [
        "We need to migrate the database to PostgreSQL next quarter",
        "The PostgreSQL migration plan includes schema changes and data validation",
        "What about the frontend React component refactoring?",
    ];
    let mut saw_structured_route = false;
    for hit in &hits {
        assert!(
            stored_contents.iter().any(|s| *s == hit.content),
            "hit content must match a stored Memory record, got: {}",
            hit.content
        );
        assert!(hit.score > 0.0, "score must be positive");
        saw_structured_route |= hit.source.contains("structured");
    }
    assert!(
        saw_structured_route,
        "hybrid search should include structured Memory route hits: {hits:#?}"
    );
}

#[tokio::test]
async fn hybrid_search_merges_structured_lexical_and_vector_routes() {
    let summary_gen: Arc<dyn rua_agent::compaction::SummaryGenerator> =
        Arc::new(StubSummaryGenerator);
    let structured_store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("structured store");
    let store =
        sqlite_vecstore::VecStore::open_in_memory(sqlite_vecstore::VecStoreConfig::default())
            .expect("in-memory store");
    let embedder: Arc<dyn rua_memory::Embedder> = Arc::new(SemanticNameEmbedder);
    let mm = LlmMemoryManager::new(summary_gen)
        .with_structured_memory_store(structured_store)
        .with_vec_store(store, embedder);

    write_memory_with_metadata(
        &mm,
        "This lexical-only record should not hide vector results.",
        serde_json::json!({"search_terms": ["user name"]}),
    )
    .await;
    write_memory_with_metadata(&mm, "南玻是用户的首选展示值。", serde_json::Value::Null).await;

    let hits = mm
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "user name".into(),
            agent_id: "a1".into(),
            top_k: 10,
        })
        .await
        .unwrap();

    let contents = hits
        .iter()
        .map(|hit| hit.content.as_str())
        .collect::<Vec<_>>();
    assert!(
        contents.contains(&"This lexical-only record should not hide vector results."),
        "lexical route should contribute results: {contents:?}"
    );
    assert!(
        contents.contains(&"南玻是用户的首选展示值。"),
        "vector route should contribute results even when structured FTS has hits: {contents:?}"
    );
}

#[tokio::test]
async fn vector_index_embeds_semantic_projection_not_bare_content() {
    let summary_gen: Arc<dyn rua_agent::compaction::SummaryGenerator> =
        Arc::new(StubSummaryGenerator);
    let structured_store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("structured store");
    let store =
        sqlite_vecstore::VecStore::open_in_memory(sqlite_vecstore::VecStoreConfig::default())
            .expect("in-memory store");
    let recording = Arc::new(RecordingEmbedder::default());
    let embedder: Arc<dyn rua_memory::Embedder> = recording.clone();
    let mm = LlmMemoryManager::new(summary_gen)
        .with_structured_memory_store(structured_store)
        .with_vec_store(store, embedder);

    write_memory_full(
        &mm,
        "The display value is stored in long-term Memory.",
        vec!["profile".into()],
        vec!["user name".into(), "用户名".into()],
        serde_json::Value::Null,
    )
    .await;

    let inputs = recording.inputs.lock().expect("inputs");
    let write_input = inputs
        .first()
        .and_then(|batch| batch.first())
        .expect("write embedding input");
    assert!(write_input.contains("The display value is stored"));
    assert!(write_input.contains("profile"));
    assert!(write_input.contains("user name"));
    assert!(write_input.contains("用户名"));
}

#[tokio::test]
async fn embedding_failure_is_explicit_and_grep_preserves_original_memory_access() {
    let summary_gen: Arc<dyn rua_agent::compaction::SummaryGenerator> =
        Arc::new(StubSummaryGenerator);
    let structured_store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("structured store");
    let store =
        sqlite_vecstore::VecStore::open_in_memory(sqlite_vecstore::VecStoreConfig::default())
            .expect("in-memory store");
    let embedder: Arc<dyn rua_memory::Embedder> = Arc::new(FailingEmbedder);
    let mm = LlmMemoryManager::new(summary_gen)
        .with_structured_memory_store(structured_store)
        .with_vec_store(store, embedder);

    write_memory_full(
        &mm,
        "User prefers compact implementation notes.",
        vec!["implementation notes".into()],
        vec!["compact notes".into()],
        serde_json::Value::Null,
    )
    .await;

    let failed = mm
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "compact notes".into(),
            agent_id: "a1".into(),
            top_k: 10,
        })
        .await;
    assert!(failed
        .err()
        .is_some_and(|error| error.to_string().contains("Embedding resource unavailable")));
    let hits = mm
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::Grep,
            query: "compact notes".into(),
            agent_id: "a1".into(),
            top_k: 10,
        })
        .await
        .unwrap();

    assert!(
        hits.iter()
            .any(|hit| hit.content == "User prefers compact implementation notes."),
        "lexical/fuzzy search should still recall Memory: {hits:#?}"
    );
}

#[tokio::test]
async fn exact_structured_match_is_not_displaced_by_vector_only_match() {
    let summary_gen: Arc<dyn rua_agent::compaction::SummaryGenerator> =
        Arc::new(StubSummaryGenerator);
    let structured_store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("structured store");
    let store =
        sqlite_vecstore::VecStore::open_in_memory(sqlite_vecstore::VecStoreConfig::default())
            .expect("in-memory store");
    let embedder: Arc<dyn rua_memory::Embedder> = Arc::new(ExactGuardEmbedder);
    let mm = LlmMemoryManager::new(summary_gen)
        .with_structured_memory_store(structured_store)
        .with_vec_store(store, embedder);

    write_memory_full(
        &mm,
        "Exact record should win when its search term is queried.",
        vec!["routing".into()],
        vec!["precise label".into()],
        serde_json::Value::Null,
    )
    .await;
    write_memory_full(
        &mm,
        "vector bait unrelated memory",
        vec!["bait".into()],
        vec![],
        serde_json::Value::Null,
    )
    .await;

    let hits = mm
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "precise label".into(),
            agent_id: "a1".into(),
            top_k: 1,
        })
        .await
        .expect("search");

    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].content,
        "Exact record should win when its search term is queried."
    );
    assert!(
        hits[0].source.contains("structured"),
        "exact structured route should be represented: {hits:#?}"
    );
}

#[tokio::test]
async fn search_empty_store_returns_empty() {
    let mm = build_manager();

    let hits = mm
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "anything".into(),
            agent_id: "a1".into(),
            top_k: 10,
        })
        .await
        .unwrap();

    assert!(hits.is_empty(), "empty store -> empty results");
}
