use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AgentError;

mod crud;
mod maintenance;
mod publication;
mod reindex;
mod search;
mod visibility;

pub use rua_memory::{
    MemoryBatchApplyOutcome, MemoryBatchApplyRequest, MemoryBatchCreateRecord,
    MemoryBatchOperation, MemoryBatchRelation, MemoryGetRequest, MemoryKind,
    MemoryMaintenanceCandidateRequest, MemoryMaintenancePacket, MemoryRecord, MemoryRecordStatus,
    MemoryScope, MemorySourceEvidence, MemoryTerm, MemoryTermKind, MemoryTermSource,
    MemoryUpdateRequest, MemoryVisibility, MemoryVisibilityUpdateRequest, MemoryWriteOutcome,
    MemoryWriteRequest,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamMemoryRequest {
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamMemoryResult {
    pub items_consolidated: usize,
    pub new_insights: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchRequest {
    pub search_mode: rua_config::MemorySearchMode,
    pub query: String,
    pub agent_id: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

fn default_top_k() -> usize {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySearchHit {
    pub content: String,
    pub score: f64,
    pub source: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadContextRequest {
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryContext {
    pub recent_items: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStatus {
    pub healthy: bool,
    pub document_count: usize,
    pub index_size_bytes: u64,
    #[serde(default)]
    pub issues: Vec<String>,
}

#[async_trait]
pub trait MemoryPublicationHook: Send + Sync {
    async fn on_memory_records_committed(&self, records: &[MemoryRecord])
        -> Result<(), AgentError>;

    async fn on_memory_visibility_committed(
        &self,
        _visibility: &MemoryVisibility,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    async fn on_memory_records_deleted(
        &self,
        agent_id: &str,
        memory_ids: &[rua_memory::MemoryId],
    ) -> Result<(), AgentError>;
}

#[async_trait]
pub trait MemoryManager: Send + Sync {
    async fn dream_memory(&self, req: DreamMemoryRequest) -> Result<DreamMemoryResult, AgentError>;

    async fn memory_search(
        &self,
        req: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchHit>, AgentError>;

    async fn memory_write(
        &self,
        _req: MemoryWriteRequest,
    ) -> Result<MemoryWriteOutcome, AgentError> {
        Ok(MemoryWriteOutcome {
            rejected: true,
            reason: Some("structured memory writes are not supported".to_string()),
            record: None,
        })
    }

    async fn memory_get(&self, _req: MemoryGetRequest) -> Result<Vec<MemoryRecord>, AgentError> {
        Ok(Vec::new())
    }

    async fn memory_get_for_agent(
        &self,
        _requester_agent_id: &str,
        req: MemoryGetRequest,
    ) -> Result<Vec<MemoryRecord>, AgentError> {
        self.memory_get(req).await
    }

    async fn memory_list(&self, _agent_id: &str) -> Result<Vec<MemoryRecord>, AgentError> {
        Ok(Vec::new())
    }

    async fn memory_update(
        &self,
        _req: MemoryUpdateRequest,
    ) -> Result<Option<MemoryRecord>, AgentError> {
        Ok(None)
    }

    async fn memory_visibility(
        &self,
        _owner_agent_id: &str,
        _memory_id: &rua_memory::MemoryId,
    ) -> Result<Option<MemoryVisibility>, AgentError> {
        Ok(None)
    }

    async fn memory_visibility_list(
        &self,
        _owner_agent_id: &str,
    ) -> Result<Vec<MemoryVisibility>, AgentError> {
        Ok(Vec::new())
    }

    async fn memory_visibility_update(
        &self,
        _owner_agent_id: &str,
        _memory_id: rua_memory::MemoryId,
        _req: MemoryVisibilityUpdateRequest,
    ) -> Result<Option<MemoryVisibility>, AgentError> {
        Ok(None)
    }

    async fn maintenance_candidates(
        &self,
        _req: MemoryMaintenanceCandidateRequest,
    ) -> Result<MemoryMaintenancePacket, AgentError> {
        Ok(MemoryMaintenancePacket {
            base_revision: "rev:0".to_string(),
            clusters: vec![],
        })
    }

    async fn memory_batch_apply(
        &self,
        _req: MemoryBatchApplyRequest,
    ) -> Result<MemoryBatchApplyOutcome, AgentError> {
        Ok(MemoryBatchApplyOutcome {
            applied: false,
            reason: Some("structured memory batch apply is not supported".to_string()),
            records: vec![],
        })
    }

    async fn load_context(&self, req: LoadContextRequest) -> Result<MemoryContext, AgentError>;

    async fn delete_memory(&self, memory_id: &str) -> Result<bool, AgentError>;

    async fn delete_memory_for_agent(
        &self,
        requester_agent_id: &str,
        memory_id: &str,
    ) -> Result<bool, AgentError> {
        let id = rua_memory::MemoryId::new(memory_id.to_string());
        let records = self
            .memory_get_for_agent(
                requester_agent_id,
                MemoryGetRequest {
                    ids: vec![id],
                    include_inactive: true,
                },
            )
            .await?;
        let Some(record) = records.first() else {
            return Ok(false);
        };
        if record.agent_id != requester_agent_id || record.status == MemoryRecordStatus::Deleted {
            return Ok(false);
        }
        self.delete_memory(memory_id).await
    }

    async fn health_check(&self) -> Result<MemoryStatus, AgentError>;

    async fn reindex(&self, workspace_dir: &std::path::Path) -> Result<usize, AgentError>;

    async fn replace_system_message(&self, _new_prompt: &str) -> Result<(), AgentError> {
        Ok(())
    }
}

pub type EmbedderSlot = Arc<std::sync::RwLock<Option<Arc<dyn rua_memory::Embedder>>>>;

pub struct LlmMemoryManager {
    summary_generator: Arc<dyn crate::compaction::SummaryGenerator>,
    structured_store: Option<std::sync::Mutex<rua_memory::StructuredMemoryStore>>,
    vec_store: Option<std::sync::Mutex<sqlite_vecstore::VecStore>>,
    embedder: EmbedderSlot,
    publication_hook: Option<Arc<dyn MemoryPublicationHook>>,
    publication_hook_cell: Option<Arc<OnceLock<Arc<dyn MemoryPublicationHook>>>>,
    workspace_memory_root: Option<PathBuf>,
}

impl LlmMemoryManager {
    pub fn new(summary_generator: Arc<dyn crate::compaction::SummaryGenerator>) -> Self {
        Self {
            summary_generator,
            structured_store: None,
            vec_store: None,
            embedder: Arc::new(std::sync::RwLock::new(None)),
            publication_hook: None,
            publication_hook_cell: None,
            workspace_memory_root: None,
        }
    }

    pub(crate) fn summary_generator(&self) -> Arc<dyn crate::compaction::SummaryGenerator> {
        self.summary_generator.clone()
    }

    pub fn with_publication_hook(mut self, hook: Arc<dyn MemoryPublicationHook>) -> Self {
        self.publication_hook = Some(hook);
        self
    }

    pub fn with_publication_hook_cell(
        mut self,
        hook: Arc<OnceLock<Arc<dyn MemoryPublicationHook>>>,
    ) -> Self {
        self.publication_hook_cell = Some(hook);
        self
    }

    pub fn with_workspace_memory_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.workspace_memory_root = Some(root.into());
        self
    }

    pub fn with_vec_store(
        mut self,
        vec_store: sqlite_vecstore::VecStore,
        embedder: Arc<dyn rua_memory::Embedder>,
    ) -> Self {
        self.vec_store = Some(std::sync::Mutex::new(vec_store));
        *self.embedder.write().expect("Memory embedder poisoned") = Some(embedder);
        self
    }

    pub fn with_structured_memory_store(
        mut self,
        store: rua_memory::StructuredMemoryStore,
    ) -> Self {
        self.structured_store = Some(std::sync::Mutex::new(store));
        self
    }

    pub fn embedder_slot(&self) -> EmbedderSlot {
        Arc::clone(&self.embedder)
    }

    fn current_embedder(&self) -> Option<Arc<dyn rua_memory::Embedder>> {
        self.embedder
            .read()
            .expect("Memory embedder poisoned")
            .clone()
    }

    fn external_memory_store_paths(&self, requester_agent_id: &str) -> Vec<(String, PathBuf)> {
        let Some(root) = self.workspace_memory_root.as_ref() else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(root) else {
            return Vec::new();
        };

        let mut stores = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(agent_id) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if agent_id == requester_agent_id {
                continue;
            }
            let db_path = path.join("memory.db");
            if db_path.is_file() {
                stores.push((agent_id, db_path));
            }
        }
        stores.sort_by(|left, right| left.0.cmp(&right.0));
        stores
    }
}

#[async_trait]
impl MemoryManager for LlmMemoryManager {
    async fn dream_memory(
        &self,
        _req: DreamMemoryRequest,
    ) -> Result<DreamMemoryResult, AgentError> {
        Ok(self.dream_index_memory())
    }

    async fn memory_search(
        &self,
        req: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchHit>, AgentError> {
        self.search_memory(req).await
    }

    async fn memory_write(
        &self,
        req: MemoryWriteRequest,
    ) -> Result<MemoryWriteOutcome, AgentError> {
        self.write_memory(req).await
    }

    async fn memory_get(&self, req: MemoryGetRequest) -> Result<Vec<MemoryRecord>, AgentError> {
        self.get_memory(req)
    }

    async fn memory_get_for_agent(
        &self,
        requester_agent_id: &str,
        req: MemoryGetRequest,
    ) -> Result<Vec<MemoryRecord>, AgentError> {
        self.get_memory_for_agent(requester_agent_id, req)
    }

    async fn memory_list(&self, agent_id: &str) -> Result<Vec<MemoryRecord>, AgentError> {
        self.list_memory(agent_id)
    }

    async fn memory_update(
        &self,
        req: MemoryUpdateRequest,
    ) -> Result<Option<MemoryRecord>, AgentError> {
        self.update_memory(req).await
    }

    async fn memory_visibility(
        &self,
        owner_agent_id: &str,
        memory_id: &rua_memory::MemoryId,
    ) -> Result<Option<MemoryVisibility>, AgentError> {
        self.get_memory_visibility(owner_agent_id, memory_id)
    }

    async fn memory_visibility_list(
        &self,
        owner_agent_id: &str,
    ) -> Result<Vec<MemoryVisibility>, AgentError> {
        self.list_memory_visibility(owner_agent_id)
    }

    async fn memory_visibility_update(
        &self,
        owner_agent_id: &str,
        memory_id: rua_memory::MemoryId,
        req: MemoryVisibilityUpdateRequest,
    ) -> Result<Option<MemoryVisibility>, AgentError> {
        self.update_memory_visibility(owner_agent_id, memory_id, req)
            .await
    }

    async fn maintenance_candidates(
        &self,
        req: MemoryMaintenanceCandidateRequest,
    ) -> Result<MemoryMaintenancePacket, AgentError> {
        self.memory_maintenance_candidates(req)
    }

    async fn memory_batch_apply(
        &self,
        req: MemoryBatchApplyRequest,
    ) -> Result<MemoryBatchApplyOutcome, AgentError> {
        self.apply_memory_batch(req).await
    }

    async fn load_context(&self, req: LoadContextRequest) -> Result<MemoryContext, AgentError> {
        let recent_items = if let Some(ref store_mutex) = self.structured_store {
            match store_mutex.lock() {
                Ok(store) => store
                    .pinned_context_for_agent(&req.agent_id)
                    .map_err(|e| AgentError::memory(format!("load pinned Memory slots: {e}")))?,
                Err(e) => {
                    return Err(AgentError::memory(format!(
                        "lock structured memory store for pinned context: {e}"
                    )));
                }
            }
        } else {
            vec![]
        };
        Ok(MemoryContext { recent_items })
    }

    async fn delete_memory(&self, memory_id: &str) -> Result<bool, AgentError> {
        self.delete_memory_by_id(memory_id).await
    }

    async fn health_check(&self) -> Result<MemoryStatus, AgentError> {
        Ok(self.memory_health_check())
    }

    async fn reindex(&self, _workspace_dir: &std::path::Path) -> Result<usize, AgentError> {
        self.reindex_memory().await
    }
}

pub struct NoOpMemoryManager;

#[async_trait]
impl MemoryManager for NoOpMemoryManager {
    async fn dream_memory(
        &self,
        _req: DreamMemoryRequest,
    ) -> Result<DreamMemoryResult, AgentError> {
        Ok(DreamMemoryResult {
            items_consolidated: 0,
            new_insights: vec![],
        })
    }

    async fn memory_search(
        &self,
        _req: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchHit>, AgentError> {
        Ok(vec![])
    }

    async fn memory_write(
        &self,
        _req: MemoryWriteRequest,
    ) -> Result<MemoryWriteOutcome, AgentError> {
        Ok(MemoryWriteOutcome {
            rejected: true,
            reason: Some("memory is disabled".to_string()),
            record: None,
        })
    }

    async fn memory_get(&self, _req: MemoryGetRequest) -> Result<Vec<MemoryRecord>, AgentError> {
        Ok(vec![])
    }

    async fn load_context(&self, _req: LoadContextRequest) -> Result<MemoryContext, AgentError> {
        Ok(MemoryContext {
            recent_items: vec![],
        })
    }

    async fn delete_memory(&self, _memory_id: &str) -> Result<bool, AgentError> {
        Ok(false)
    }

    async fn health_check(&self) -> Result<MemoryStatus, AgentError> {
        Ok(MemoryStatus {
            healthy: true,
            document_count: 0,
            index_size_bytes: 0,
            issues: vec![],
        })
    }

    async fn reindex(&self, _workspace_dir: &std::path::Path) -> Result<usize, AgentError> {
        Ok(0)
    }
}

pub struct InMemoryMemoryManager {
    entries: tokio::sync::Mutex<Vec<MemorySearchHit>>,
}

impl InMemoryMemoryManager {
    pub fn new() -> Self {
        Self {
            entries: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn with_entries(entries: Vec<MemorySearchHit>) -> Self {
        Self {
            entries: tokio::sync::Mutex::new(entries),
        }
    }
}

impl Default for InMemoryMemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryManager for InMemoryMemoryManager {
    async fn dream_memory(
        &self,
        _req: DreamMemoryRequest,
    ) -> Result<DreamMemoryResult, AgentError> {
        Ok(DreamMemoryResult {
            items_consolidated: 0,
            new_insights: vec![],
        })
    }

    async fn memory_search(
        &self,
        req: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchHit>, AgentError> {
        let entries = self.entries.lock().await;
        let query_lower = req.query.to_lowercase();
        let words: Vec<&str> = query_lower
            .split_whitespace()
            .filter(|w| w.len() >= 3)
            .collect();
        let matched: Vec<MemorySearchHit> = entries
            .iter()
            .filter(|h| {
                let content_lower = h.content.to_lowercase();
                words.iter().any(|w| content_lower.contains(w))
            })
            .cloned()
            .collect();
        Ok(matched.into_iter().take(req.top_k).collect())
    }

    async fn memory_write(
        &self,
        req: MemoryWriteRequest,
    ) -> Result<MemoryWriteOutcome, AgentError> {
        let mut entries = self.entries.lock().await;
        let id = rua_memory::MemoryId::new(format!("mem_{}", uuid::Uuid::new_v4().simple()));
        let now = chrono::Utc::now();
        let written_on_device_id = memory_metadata_string(&req.metadata, "written_on_device_id");
        let written_on_device_name_snapshot =
            memory_metadata_string(&req.metadata, "written_on_device_name_snapshot");
        let resource_device_id = memory_metadata_string(&req.metadata, "resource_device_id")
            .or_else(|| written_on_device_id.clone());
        let resource_device_name_snapshot =
            memory_metadata_string(&req.metadata, "resource_device_name_snapshot")
                .or_else(|| written_on_device_name_snapshot.clone());
        let record = MemoryRecord {
            id: id.clone(),
            agent_id: req.agent_id,
            scope: req.scope,
            kind: req.kind,
            title: req.title,
            content: req.content,
            status: MemoryRecordStatus::Active,
            confidence: req.confidence.unwrap_or(0.7),
            importance: req.importance,
            created_at: now,
            updated_at: Some(now),
            last_used_at: None,
            expires_at: None,
            superseded_by: None,
            written_on_device_id,
            written_on_device_name_snapshot,
            written_on_host_profile: memory_metadata_string(
                &req.metadata,
                "written_on_host_profile",
            ),
            resource_device_id,
            resource_device_name_snapshot,
            metadata: req.metadata,
            sources: req.evidence.into_iter().collect(),
            links: vec![],
            terms: req
                .labels
                .iter()
                .map(|label| MemoryTerm {
                    term: label.clone(),
                    normalized_term: label.to_lowercase(),
                    term_kind: MemoryTermKind::Label,
                    source: MemoryTermSource::Agent,
                    language: None,
                    created_at: now,
                })
                .chain(req.search_terms.iter().map(|term| MemoryTerm {
                    term: term.clone(),
                    normalized_term: term.to_lowercase(),
                    term_kind: MemoryTermKind::SearchTerm,
                    source: MemoryTermSource::Agent,
                    language: None,
                    created_at: now,
                }))
                .collect(),
            resources: vec![],
            relations: vec![],
        };
        entries.push(MemorySearchHit {
            content: record.content.clone(),
            score: 1.0,
            source: "structured".into(),
            metadata: serde_json::json!({"id": id.to_string()}),
        });
        Ok(MemoryWriteOutcome {
            rejected: false,
            reason: None,
            record: Some(record),
        })
    }

    async fn memory_get(&self, _req: MemoryGetRequest) -> Result<Vec<MemoryRecord>, AgentError> {
        Ok(vec![])
    }

    async fn load_context(&self, _req: LoadContextRequest) -> Result<MemoryContext, AgentError> {
        Ok(MemoryContext {
            recent_items: vec![],
        })
    }

    async fn delete_memory(&self, _memory_id: &str) -> Result<bool, AgentError> {
        Ok(false)
    }

    async fn health_check(&self) -> Result<MemoryStatus, AgentError> {
        let count = self.entries.lock().await.len();
        Ok(MemoryStatus {
            healthy: true,
            document_count: count,
            index_size_bytes: 0,
            issues: vec![],
        })
    }

    async fn reindex(&self, _workspace_dir: &Path) -> Result<usize, AgentError> {
        Ok(0)
    }
}

fn memory_metadata_string(metadata: &serde_json::Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}
