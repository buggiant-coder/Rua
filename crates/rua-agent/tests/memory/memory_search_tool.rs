use std::sync::Arc;

use async_trait::async_trait;
use rua_agent::memory_manager::{
    DreamMemoryRequest, DreamMemoryResult, LoadContextRequest, MemoryContext, MemoryManager,
    MemorySearchHit, MemorySearchRequest, MemoryStatus,
};
use rua_contract::Tool;
use rua_core::tool::ToolContext;

struct FakeMemoryManager {
    hits: Vec<MemorySearchHit>,
}

#[async_trait]
impl MemoryManager for FakeMemoryManager {
    async fn dream_memory(
        &self,
        _req: DreamMemoryRequest,
    ) -> Result<DreamMemoryResult, rua_agent::AgentError> {
        Ok(DreamMemoryResult {
            items_consolidated: 0,
            new_insights: vec![],
        })
    }

    async fn memory_search(
        &self,
        req: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchHit>, rua_agent::AgentError> {
        assert_eq!(req.agent_id, "test-agent");
        assert_eq!(req.query, "Alpha");
        Ok(self.hits.clone())
    }
    async fn load_context(
        &self,
        _req: LoadContextRequest,
    ) -> Result<MemoryContext, rua_agent::AgentError> {
        Ok(MemoryContext {
            recent_items: vec![],
        })
    }

    async fn delete_memory(&self, _memory_id: &str) -> Result<bool, rua_agent::AgentError> {
        Ok(false)
    }

    async fn health_check(&self) -> Result<MemoryStatus, rua_agent::AgentError> {
        Ok(MemoryStatus {
            healthy: true,
            document_count: self.hits.len(),
            index_size_bytes: 0,
            issues: vec![],
        })
    }

    async fn reindex(
        &self,
        _workspace_dir: &std::path::Path,
    ) -> Result<usize, rua_agent::AgentError> {
        Ok(0)
    }
}

#[tokio::test]
async fn memory_search_tool_calls_memory_manager_directly() {
    let manager: Arc<dyn MemoryManager> = Arc::new(FakeMemoryManager {
        hits: vec![
            MemorySearchHit {
                content: "We discussed project Alpha last week".into(),
                score: 0.95,
                source: "session-abc".into(),
                metadata: serde_json::json!({ "id": "mem_alpha_1" }),
            },
            MemorySearchHit {
                content: "Alpha milestone 2 is due Friday".into(),
                score: 0.88,
                source: "session-def".into(),
                metadata: serde_json::Value::Null,
            },
        ],
    });
    let tool = rua_agent::memory_search_tool::MemorySearchTool::new(manager);
    let ctx = ToolContext {
        agent_id: "test-agent".into(),
        session_id: "test-session".into(),
        working_dir: std::path::PathBuf::from("/tmp"),
        ..Default::default()
    };

    let result = tool
        .call(serde_json::json!({ "keywords": "Alpha" }), ctx)
        .await
        .expect("call should succeed");
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .unwrap_or_default();

    assert!(text.contains("project Alpha"), "got: {text}");
    assert!(text.contains("0.950"), "got: {text}");
    assert!(text.contains("id=mem_alpha_1"), "got: {text}");
    assert!(!text.contains("Memory search results"), "got: {text}");
    assert!(!text.contains("for 'Alpha'"), "got: {text}");
    assert!(!text.contains("source="), "got: {text}");
    assert!(!text.contains("owner="), "got: {text}");
    assert!(!text.contains("scope="), "got: {text}");
    assert_eq!(text.lines().count(), 2, "got: {text}");
}

#[test]
fn memory_search_schema_uses_keywords_parameter() {
    let schema = rua_agent::memory_search_tool::MemorySearchTool::spec().parameters;

    assert!(schema["properties"]["keywords"].is_object());
    assert!(schema["properties"].get("query").is_none());
    assert_eq!(schema["required"], serde_json::json!(["keywords"]));
}

#[test]
fn memory_search_results_truncate_content_with_get_hint() {
    let long_content = "alpha ".repeat(40);
    let text = rua_agent::memory_search_tool::format_memory_results(
        "alpha",
        &[MemorySearchHit {
            content: long_content,
            score: 0.9,
            source: "structured".into(),
            metadata: serde_json::json!({
                "id": "mem_alpha",
                "kind": "preference",
                "title": "Response style"
            }),
        }],
    );

    assert!(text.contains("id=mem_alpha"), "got: {text}");
    assert!(text.contains("kind=preference"), "got: {text}");
    assert!(
        text.contains("use memory_get for full record"),
        "got: {text}"
    );
    assert!(!text.contains(&"alpha ".repeat(35)), "got: {text}");
}

#[test]
fn memory_search_results_compact_after_five_hits() {
    let hits = (0..6)
        .map(|index| MemorySearchHit {
            content: format!("Memory content {index} with enough detail to render."),
            score: 1.0 - (index as f64 * 0.1),
            source: "structured".into(),
            metadata: serde_json::json!({
                "id": format!("mem_{index}"),
                "kind": "fact",
                "title": format!("Memory title {index}"),
                "confidence": 0.7,
                "updated_at": "2026-07-04T00:00:00Z",
                "written_on_device_id": "device-a",
                "written_on_device_name_snapshot": "Mac",
                "resource_device_id": "device-b",
                "resource_device_name_snapshot": "Phone"
            }),
        })
        .collect::<Vec<_>>();

    let text = rua_agent::memory_search_tool::format_memory_results("memory", &hits);
    let lines = text.lines().collect::<Vec<_>>();

    assert_eq!(lines.len(), 6, "got: {text}");
    assert!(lines[0].contains("confidence=0.700"), "got: {text}");
    assert!(
        lines[0].contains("resource_device=Phone(device-b)"),
        "got: {text}"
    );
    assert!(lines[2].contains("confidence=0.700"), "got: {text}");
    assert!(lines[3].contains("id=mem_3"), "got: {text}");
    assert!(lines[3].contains("kind=fact"), "got: {text}");
    assert!(!lines[3].contains("confidence="), "got: {text}");
    assert!(!lines[3].contains("resource_device="), "got: {text}");
}
