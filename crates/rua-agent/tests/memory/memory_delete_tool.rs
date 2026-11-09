use std::sync::Arc;

use rua_agent::memory_manager::{
    LlmMemoryManager, MemoryGetRequest, MemoryKind, MemoryManager, MemoryRecordStatus, MemoryScope,
    MemorySearchRequest, MemoryWriteRequest,
};
use rua_contract::Tool;
use rua_core::tool::ToolContext;

struct StubSummaryGenerator;

#[async_trait::async_trait]
impl rua_agent::compaction::SummaryGenerator for StubSummaryGenerator {
    async fn summarize(
        &self,
        messages: &[agent_types::ModelDialogueItem],
        _previous_summary: Option<&str>,
        _max_tokens: Option<usize>,
        _model: Option<&rua_session::CheckpointModelIdentity>,
    ) -> Result<String, rua_agent::AgentError> {
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

#[tokio::test]
async fn memory_delete_tool_deletes_current_agent_record_by_id() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let manager =
        Arc::new(LlmMemoryManager::new(summary_gen()).with_structured_memory_store(store));

    let written = manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::Fact,
            scope: MemoryScope::Agent,
            content: "Agent A should forget this durable fact.".into(),
            confidence: Some(0.9),
            importance: None,
            ttl_days: None,
            labels: vec![],
            search_terms: vec!["delete me".into()],
            evidence: None,
            metadata: serde_json::Value::Null,
        })
        .await
        .expect("memory write")
        .record
        .expect("created record");

    let ctx = ToolContext {
        agent_id: "agent-a".into(),
        session_id: "session-a".into(),
        working_dir: std::path::PathBuf::from("/tmp"),
        ..Default::default()
    };
    let tool = rua_agent::memory_delete_tool::MemoryDeleteTool::new(
        manager.clone() as Arc<dyn MemoryManager>
    );
    let result = tool
        .call(serde_json::json!({ "id": written.id.to_string() }), ctx)
        .await
        .expect("delete call");
    let text = result.text_content();

    assert!(
        text.contains(&format!("memory deleted: id={}", written.id)),
        "got: {text}"
    );

    let search_hits = manager
        .memory_search(MemorySearchRequest {
            search_mode: rua_config::MemorySearchMode::default(),
            agent_id: "agent-a".into(),
            query: "delete me".into(),
            top_k: 10,
        })
        .await
        .expect("memory search");
    assert!(search_hits.is_empty(), "got: {search_hits:?}");

    let records = manager
        .memory_get(MemoryGetRequest {
            ids: vec![written.id.clone()],
            include_inactive: true,
        })
        .await
        .expect("memory get");
    assert_eq!(records[0].status, MemoryRecordStatus::Deleted);
}
