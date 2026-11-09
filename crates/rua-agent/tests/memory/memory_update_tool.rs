use std::sync::Arc;

use async_trait::async_trait;
use rua_agent::memory_manager::{
    DreamMemoryRequest, DreamMemoryResult, LoadContextRequest, MemoryContext, MemoryGetRequest,
    MemoryKind, MemoryManager, MemoryRecord, MemoryRecordStatus, MemoryScope, MemorySearchHit,
    MemorySearchRequest, MemoryStatus, MemoryUpdateRequest,
};
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use tokio::sync::Mutex;

struct RecordingMemoryManager {
    last_update: Mutex<Option<MemoryUpdateRequest>>,
    record: Mutex<Option<MemoryRecord>>,
}

impl RecordingMemoryManager {
    fn new(record: MemoryRecord) -> Self {
        Self {
            last_update: Mutex::new(None),
            record: Mutex::new(Some(record)),
        }
    }

    async fn take_update(&self) -> Option<MemoryUpdateRequest> {
        self.last_update.lock().await.take()
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

    async fn memory_get(
        &self,
        req: MemoryGetRequest,
    ) -> Result<Vec<MemoryRecord>, rua_agent::AgentError> {
        let Some(record) = self.record.lock().await.clone() else {
            return Ok(vec![]);
        };
        if !req.ids.contains(&record.id) {
            return Ok(vec![]);
        }
        if !req.include_inactive && record.status != MemoryRecordStatus::Active {
            return Ok(vec![]);
        }
        Ok(vec![record])
    }

    async fn memory_update(
        &self,
        req: MemoryUpdateRequest,
    ) -> Result<Option<MemoryRecord>, rua_agent::AgentError> {
        *self.last_update.lock().await = Some(req.clone());
        let mut slot = self.record.lock().await;
        let Some(record) = slot.as_mut() else {
            return Ok(None);
        };
        if record.id != req.id || record.agent_id != req.agent_id {
            return Ok(None);
        }
        record.kind = req.kind;
        record.scope = req.scope;
        record.title = req.title;
        record.content = req.content;
        record.confidence = req.confidence;
        record.importance = req.importance;
        record.updated_at = Some(chrono::Utc::now());
        Ok(Some(record.clone()))
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

#[tokio::test]
async fn memory_update_tool_calls_memory_update_by_id() {
    let now = chrono::Utc::now();
    let existing = MemoryRecord {
        title: "Verbose answer preference".into(),
        id: rua_memory::MemoryId::new("mem_existing"),
        agent_id: "agent-a".into(),
        scope: MemoryScope::Agent,
        kind: MemoryKind::Fact,
        content: "User prefers verbose answers.".into(),
        status: MemoryRecordStatus::Active,
        confidence: 0.6,
        importance: Some(0.2),
        created_at: now,
        updated_at: Some(now),
        last_used_at: None,
        expires_at: None,
        superseded_by: None,
        written_on_device_id: None,
        written_on_device_name_snapshot: None,
        written_on_host_profile: None,
        resource_device_id: None,
        resource_device_name_snapshot: None,
        metadata: serde_json::Value::Null,
        sources: vec![],
        links: vec![],
        terms: vec![],
        resources: vec![],
        relations: vec![],
    };
    let manager = Arc::new(RecordingMemoryManager::new(existing));
    let tool = rua_agent::memory_update_tool::MemoryUpdateTool::new(manager.clone());
    let ctx = ToolContext {
        agent_id: "agent-a".into(),
        session_id: "chat-a".into(),
        ..Default::default()
    };

    let result = tool
        .call(
            serde_json::json!({
                "id": "mem_existing",
                "kind": "preference",
                "scope": "user",
                "title": "Concise answer preference",
                "content": "User prefers concise answers.",
                "confidence": 0.9,
                "importance": 0.8,
                "search_terms": ["concise answers", "简洁回答"]
            }),
            ctx,
        )
        .await
        .expect("update should succeed");
    let text = result.text_content();
    assert!(text.contains("memory updated"), "got: {text}");
    assert!(text.contains("mem_existing"), "got: {text}");

    let recorded = manager
        .take_update()
        .await
        .expect("recorded update request");
    assert_eq!(recorded.id, rua_memory::MemoryId::new("mem_existing"));
    assert_eq!(recorded.agent_id, "agent-a");
    assert_eq!(recorded.kind, MemoryKind::Preference);
    assert_eq!(recorded.scope, MemoryScope::User);
    assert_eq!(recorded.title, "Concise answer preference");
    assert_eq!(recorded.content, "User prefers concise answers.");
    assert_eq!(recorded.confidence, 0.9);
    assert_eq!(recorded.importance, Some(0.8));
    assert_eq!(
        recorded.search_terms,
        Some(vec!["concise answers".into(), "简洁回答".into()])
    );
}
