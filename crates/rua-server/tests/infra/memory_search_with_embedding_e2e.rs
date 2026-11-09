use std::sync::Arc;

use async_trait::async_trait;
use rua_agent::error::AgentError;
use rua_agent::memory_manager::*;
use rua_memory::HttpEmbeddingTransport;
use rua_memory::LlmEmbedder;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

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

const DIM: usize = 32;

fn hash_to_vector(text: &str) -> Vec<f32> {
    let mut vec = vec![0.0f32; DIM];
    for (i, byte) in text.bytes().enumerate() {
        vec[i % DIM] += (byte as f32) / 255.0;
    }
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    }
    vec
}

struct DynamicEmbeddingResponder;

impl Respond for DynamicEmbeddingResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let inputs = body["input"].as_array().unwrap();
        let data: Vec<serde_json::Value> = inputs
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let text = v.as_str().unwrap();
                serde_json::json!({
                    "object": "embedding",
                    "index": i,
                    "embedding": hash_to_vector(text),
                })
            })
            .collect();
        let resp = serde_json::json!({
            "object": "list",
            "data": data,
            "model": "test-embed",
            "usage": { "prompt_tokens": 10, "total_tokens": 10 }
        });
        ResponseTemplate::new(200).set_body_json(&resp)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mock_embedding_endpoint_store_and_recall() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(DynamicEmbeddingResponder)
        .mount(&mock_server)
        .await;

    let transport = Arc::new(HttpEmbeddingTransport::new(&mock_server.uri(), None));
    let embedder: Arc<dyn rua_memory::Embedder> =
        Arc::new(LlmEmbedder::new(transport, "test-embed", DIM));

    let store =
        sqlite_vecstore::VecStore::open_in_memory(sqlite_vecstore::VecStoreConfig::default())
            .expect("in-memory store");

    let summary_gen: Arc<dyn rua_agent::compaction::SummaryGenerator> =
        Arc::new(StubSummaryGenerator);
    let structured_store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("structured store");
    let mm = LlmMemoryManager::new(summary_gen)
        .with_structured_memory_store(structured_store)
        .with_vec_store(store, embedder);

    let dialogs = [
        "We need to migrate the database to PostgreSQL next quarter",
        "The PostgreSQL migration plan includes schema changes and data validation",
        "What about the frontend React component refactoring?",
    ];

    for dialog in &dialogs {
        mm.memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "a1".into(),
            kind: MemoryKind::ProjectFact,
            scope: MemoryScope::Project,
            content: dialog.to_string(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec![],
            search_terms: vec![],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .unwrap();
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

    let first = &hits[0];
    assert!(
        first.content.contains("PostgreSQL") || first.content.contains("database"),
        "top hit should be database-related, got: {}",
        first.content
    );
    assert!(first.score > 0.0, "score must be positive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fallback_null_embedder_no_vecstore_returns_empty() {
    let summary_gen: Arc<dyn rua_agent::compaction::SummaryGenerator> =
        Arc::new(StubSummaryGenerator);
    let mm = LlmMemoryManager::new(summary_gen);

    let hits = mm
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            query: "hello".into(),
            agent_id: "a1".into(),
            top_k: 10,
        })
        .await
        .unwrap();

    assert!(
        hits.is_empty(),
        "no vecstore (NullEmbedder fallback path) -> empty results"
    );
}
