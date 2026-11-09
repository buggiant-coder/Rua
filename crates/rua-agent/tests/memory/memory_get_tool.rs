use std::sync::Arc;

use rua_agent::memory_manager::{
    LlmMemoryManager, MemoryKind, MemoryManager, MemoryScope, MemorySourceEvidence,
    MemoryWriteRequest,
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
async fn memory_search_returns_structured_hit_then_memory_get_loads_evidence() {
    let store =
        rua_memory::StructuredMemoryStore::open_in_memory().expect("open structured memory store");
    let manager =
        Arc::new(LlmMemoryManager::new(summary_gen()).with_structured_memory_store(store));

    let written = manager
        .memory_write(MemoryWriteRequest {
            title: "Test memory".into(),
            agent_id: "agent-a".into(),
            kind: MemoryKind::Preference,
            scope: MemoryScope::User,
            content: "User prefers concise engineering feedback.".into(),
            confidence: Some(0.9),
            importance: Some(0.8),
            ttl_days: None,
            labels: vec!["feedback".into(), "style".into()],
            search_terms: vec!["brief engineering answer".into()],
            evidence: Some(MemorySourceEvidence {
                chat_id: Some("chat-a".into()),
                turn_id: Some("turn-a".into()),
                message_id: Some("message-a".into()),
                quote: Some("以后直接给工程结论。".into()),
            }),
            metadata: serde_json::json!({
                "source": "test",
                "written_on_device_id": "device-a",
                "written_on_device_name_snapshot": "Laptop A",
                "written_on_host_profile": "desktop",
                "resource_device_id": "device-a",
                "resource_device_name_snapshot": "Laptop A",
            }),
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
    let search_tool = rua_agent::memory_search_tool::MemorySearchTool::new(
        manager.clone() as Arc<dyn MemoryManager>
    );
    let search = search_tool
        .call(
            serde_json::json!({ "keywords": "brief engineering answer" }),
            ctx.clone(),
        )
        .await
        .expect("search call");
    let search_text = search.text_content();

    assert!(
        search_text.contains("User prefers concise engineering feedback."),
        "got: {search_text}"
    );
    assert!(
        search_text.contains("kind=preference"),
        "got: {search_text}"
    );
    assert!(
        search_text.contains("title=Test memory"),
        "got: {search_text}"
    );
    assert!(search_text.contains("Laptop A"), "got: {search_text}");
    assert!(search_text.contains("device-a"), "got: {search_text}");
    assert!(
        search_text.contains(&written.id.to_string()),
        "got: {search_text}"
    );
    assert!(!search_text.contains("scope="), "got: {search_text}");

    let get_tool =
        rua_agent::memory_get_tool::MemoryGetTool::new(manager.clone() as Arc<dyn MemoryManager>);
    let loaded = get_tool
        .call(serde_json::json!({ "ids": [written.id.to_string()] }), ctx)
        .await
        .expect("get call");
    let loaded_text = loaded.text_content();

    assert!(
        loaded_text.contains(&written.id.to_string()),
        "got: {loaded_text}"
    );
    assert!(
        loaded_text.contains("User prefers concise engineering feedback."),
        "got: {loaded_text}"
    );
    assert!(
        loaded_text.contains("以后直接给工程结论。"),
        "got: {loaded_text}"
    );
    assert!(loaded_text.contains("Laptop A"), "got: {loaded_text}");
    assert!(loaded_text.contains("device-a"), "got: {loaded_text}");
}
