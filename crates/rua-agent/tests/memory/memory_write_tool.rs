use std::sync::Arc;

use async_trait::async_trait;
use rua_agent::memory_manager::{
    DreamMemoryRequest, DreamMemoryResult, LoadContextRequest, MemoryContext, MemoryKind,
    MemoryManager, MemoryRecord, MemoryRecordStatus, MemoryScope, MemorySearchHit,
    MemorySearchRequest, MemoryStatus, MemoryWriteOutcome, MemoryWriteRequest,
};
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use tokio::sync::Mutex;

struct RecordingMemoryManager {
    last_write: Mutex<Option<MemoryWriteRequest>>,
}

impl RecordingMemoryManager {
    fn new() -> Self {
        Self {
            last_write: Mutex::new(None),
        }
    }

    async fn take_write(&self) -> Option<MemoryWriteRequest> {
        self.last_write.lock().await.take()
    }
}

#[async_trait]
impl MemoryManager for RecordingMemoryManager {
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
        _req: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchHit>, rua_agent::AgentError> {
        Ok(vec![])
    }

    async fn memory_write(
        &self,
        req: MemoryWriteRequest,
    ) -> Result<MemoryWriteOutcome, rua_agent::AgentError> {
        *self.last_write.lock().await = Some(req.clone());
        let now = chrono::Utc::now();
        Ok(MemoryWriteOutcome {
            rejected: false,
            reason: None,
            record: Some(MemoryRecord {
                title: req.title,
                id: rua_memory::MemoryId::new("mem_tool_test"),
                agent_id: req.agent_id,
                scope: req.scope,
                kind: req.kind,
                content: req.content,
                status: MemoryRecordStatus::Active,
                confidence: req.confidence.unwrap_or(1.0),
                importance: req.importance,
                created_at: now,
                updated_at: Some(now),
                last_used_at: None,
                expires_at: None,
                superseded_by: None,
                written_on_device_id: req
                    .metadata
                    .get("written_on_device_id")
                    .and_then(|value| value.as_str())
                    .map(ToOwned::to_owned),
                written_on_device_name_snapshot: req
                    .metadata
                    .get("written_on_device_name_snapshot")
                    .and_then(|value| value.as_str())
                    .map(ToOwned::to_owned),
                written_on_host_profile: req
                    .metadata
                    .get("written_on_host_profile")
                    .and_then(|value| value.as_str())
                    .map(ToOwned::to_owned),
                resource_device_id: req
                    .metadata
                    .get("resource_device_id")
                    .and_then(|value| value.as_str())
                    .map(ToOwned::to_owned),
                resource_device_name_snapshot: req
                    .metadata
                    .get("resource_device_name_snapshot")
                    .and_then(|value| value.as_str())
                    .map(ToOwned::to_owned),
                metadata: req.metadata,
                sources: req.evidence.into_iter().collect(),
                links: vec![],
                terms: vec![],
                resources: vec![],
                relations: vec![],
            }),
        })
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
            document_count: 0,
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

#[test]
fn memory_write_tool_requires_display_title_content_and_search_terms() {
    let parameters = rua_agent::memory_write_tool::MemoryWriteTool::spec().parameters;
    let required = parameters["required"].as_array().expect("required fields");
    for field in ["title", "content", "search_terms"] {
        assert!(
            required.iter().any(|value| value == field),
            "missing required field {field}"
        );
    }
    assert!(parameters["properties"].get("subject").is_none());
    assert_eq!(
        parameters["properties"]["title"]["maxLength"],
        rua_memory::MEMORY_TITLE_MAX_CHARS
    );
    assert_eq!(parameters["properties"]["search_terms"]["minItems"], 1);
}

#[tokio::test]
async fn memory_write_tool_calls_memory_manager_directly() {
    let manager = Arc::new(RecordingMemoryManager::new());
    let tool = rua_agent::memory_write_tool::MemoryWriteTool::new(manager.clone());
    let ctx = ToolContext {
        agent_id: "agent-a".into(),
        session_id: "session-a".into(),
        working_dir: std::path::PathBuf::from("/tmp"),
        metadata: std::collections::HashMap::from([
            ("device_id".to_string(), serde_json::json!("device-b")),
            (
                "device_friendly_name".to_string(),
                serde_json::json!("Workstation B"),
            ),
            ("host_profile".to_string(), serde_json::json!("desktop")),
        ]),
        ..Default::default()
    };

    let result = tool
        .call(
            serde_json::json!({
                "kind": "preference",
                "scope": "user",
                "title": "Concise engineering feedback",
                "content": "User prefers concise engineering feedback.",
                "labels": ["feedback", "style"],
                "ttl_days": null,
                "evidence": {
                    "chat_id": "chat-a",
                    "turn_id": "turn-a",
                    "message_id": "message-a",
                    "quote": "以后直接给工程结论。"
                },
                "search_terms": ["concise feedback", "brief engineering answer", "简洁反馈"],
                "metadata": {
                    "written_on_device_id": "model-forged-device",
                    "resource_device_id": "model-forged-resource",
                    "source": "test",
                    "search_terms": ["existing alias"]
                }
            }),
            ctx,
        )
        .await
        .expect("call should succeed");

    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .unwrap_or_default();
    assert!(text.contains("mem_tool_test"), "got: {text}");
    assert!(text.contains("saved"), "got: {text}");

    let recorded = manager.take_write().await.expect("recorded write request");
    assert_eq!(recorded.agent_id, "agent-a");
    assert_eq!(recorded.kind, MemoryKind::Preference);
    assert_eq!(recorded.scope, MemoryScope::User);
    assert_eq!(recorded.title, "Concise engineering feedback");
    assert_eq!(
        recorded.content,
        "User prefers concise engineering feedback."
    );
    assert_eq!(recorded.labels, vec!["feedback", "style"]);
    assert_eq!(
        recorded.search_terms,
        vec!["concise feedback", "brief engineering answer", "简洁反馈"]
    );
    assert_eq!(
        recorded.evidence.and_then(|e| e.chat_id),
        Some("chat-a".into())
    );
    assert_eq!(
        recorded
            .metadata
            .get("written_on_device_id")
            .and_then(|value| value.as_str()),
        Some("device-b")
    );
    assert_eq!(
        recorded
            .metadata
            .get("resource_device_id")
            .and_then(|value| value.as_str()),
        Some("device-b")
    );
    assert_eq!(
        recorded
            .metadata
            .get("written_on_device_name_snapshot")
            .and_then(|value| value.as_str()),
        Some("Workstation B")
    );
    assert_eq!(
        recorded
            .metadata
            .get("written_on_host_profile")
            .and_then(|value| value.as_str()),
        Some("desktop")
    );
    assert_eq!(
        recorded
            .metadata
            .get("source")
            .and_then(|value| value.as_str()),
        Some("test")
    );
    let search_terms = recorded
        .metadata
        .get("search_terms")
        .and_then(|value| value.as_array())
        .expect("search_terms array")
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>();
    assert!(search_terms.contains(&"existing alias"));
    assert!(search_terms.contains(&"concise feedback"));
    assert!(search_terms.contains(&"brief engineering answer"));
    assert!(search_terms.contains(&"简洁反馈"));
}

#[tokio::test]
async fn memory_write_tool_injects_chat_and_turn_source_from_context() {
    let manager = Arc::new(RecordingMemoryManager::new());
    let tool = rua_agent::memory_write_tool::MemoryWriteTool::new(manager.clone());
    let ctx = ToolContext {
        agent_id: "agent-a".into(),
        session_id: "chat-from-session".into(),
        metadata: std::collections::HashMap::from([(
            "turn_id".to_string(),
            serde_json::json!("turn-from-context"),
        )]),
        ..Default::default()
    };

    tool.call(
        serde_json::json!({
            "kind": "preference",
            "scope": "user",
            "title": "Direct answer preference",
            "content": "User wants direct answers.",
            "labels": ["response style"],
            "search_terms": ["direct answers"]
        }),
        ctx,
    )
    .await
    .expect("call should succeed");

    let recorded = manager.take_write().await.expect("recorded write request");
    let evidence = recorded.evidence.expect("context evidence");
    assert_eq!(evidence.chat_id.as_deref(), Some("chat-from-session"));
    assert_eq!(evidence.turn_id.as_deref(), Some("turn-from-context"));
    assert_eq!(evidence.message_id, None);
    assert_eq!(evidence.quote, None);
}

#[tokio::test]
async fn memory_write_tool_accepts_title_content_labels_and_search_terms() {
    let manager = Arc::new(RecordingMemoryManager::new());
    let tool = rua_agent::memory_write_tool::MemoryWriteTool::new(manager.clone());
    let ctx = ToolContext {
        agent_id: "agent-a".into(),
        session_id: "chat-a".into(),
        ..Default::default()
    };

    tool.call(
        serde_json::json!({
            "title": "Family photos location",
            "content": "User keeps family photos on the Windows PC.",
            "labels": ["family photos"],
            "search_terms": ["photo album"]
        }),
        ctx,
    )
    .await
    .expect("minimal write should succeed");

    let recorded = manager.take_write().await.expect("recorded write request");
    assert_eq!(recorded.title, "Family photos location");
    assert_eq!(recorded.kind, MemoryKind::Fact);
    assert_eq!(recorded.scope, MemoryScope::Agent);
    assert_eq!(recorded.labels, vec!["family photos"]);
    assert_eq!(recorded.search_terms, vec!["photo album"]);
}

#[tokio::test]
async fn memory_write_tool_warns_when_content_is_long() {
    let manager = Arc::new(RecordingMemoryManager::new());
    let tool = rua_agent::memory_write_tool::MemoryWriteTool::new(manager);
    let ctx = ToolContext {
        agent_id: "agent-a".into(),
        session_id: "chat-a".into(),
        ..Default::default()
    };
    let long_content = "User prefers concise durable Memory. ".repeat(20);

    let result = tool
        .call(
            serde_json::json!({
                "title": "Concise memory preference",
                "content": long_content,
                "search_terms": ["long memory warning"]
            }),
            ctx,
        )
        .await
        .expect("write should succeed");
    let text = result.text_content();

    assert!(text.contains("memory saved"), "got: {text}");
    assert!(
        text.contains("warning: Memory content is long"),
        "got: {text}"
    );
}

#[test]
fn register_memory_tools_adds_search_and_write_to_toolkit() {
    let manager: Arc<dyn MemoryManager> = Arc::new(RecordingMemoryManager::new());
    let mut toolkit = react_agent::AgentToolkit::new();

    assert!(!toolkit.has_tool("memory_write"));

    rua_agent::memory_tools::register_memory_tools(&mut toolkit, manager);

    assert!(toolkit.has_tool("memory_search"));
    assert!(toolkit.has_tool("memory_get"));
    assert!(toolkit.has_tool("memory_write"));
    assert!(toolkit.has_tool("memory_update"));
    assert!(toolkit.has_tool("memory_delete"));
    assert!(toolkit.get_definition("memory_get").is_some());
    let definition = toolkit
        .get_definition("memory_write")
        .expect("memory_write definition");
    assert!(definition.description.contains("Do not save task progress"));
}
