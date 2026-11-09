use std::collections::HashSet;

use crate::error::AgentError;

use super::reindex::{structured_record_semantic_text, structured_record_to_document};
use super::{
    LlmMemoryManager, MemoryGetRequest, MemoryRecord, MemoryRecordStatus, MemoryUpdateRequest,
    MemoryWriteOutcome, MemoryWriteRequest,
};

impl LlmMemoryManager {
    pub(super) async fn write_memory(
        &self,
        req: MemoryWriteRequest,
    ) -> Result<MemoryWriteOutcome, AgentError> {
        let Some(ref store_mutex) = self.structured_store else {
            return Ok(MemoryWriteOutcome {
                rejected: true,
                reason: Some("structured memory store is not configured".to_string()),
                record: None,
            });
        };

        let outcome = match store_mutex.lock() {
            Ok(mut store) => store
                .write(req)
                .map_err(|e| AgentError::memory(format!("write structured memory: {e}"))),
            Err(e) => Err(AgentError::memory(format!(
                "lock structured memory store for write: {e}"
            ))),
        }?;

        if let Some(record) = outcome.record.as_ref() {
            self.upsert_vec_index_for_record(record, "structured memory")
                .await;
            self.publish_committed_records(std::slice::from_ref(record))
                .await;
        }

        Ok(outcome)
    }

    pub(super) fn get_memory(
        &self,
        req: MemoryGetRequest,
    ) -> Result<Vec<MemoryRecord>, AgentError> {
        let Some(ref store_mutex) = self.structured_store else {
            return Ok(Vec::new());
        };

        match store_mutex.lock() {
            Ok(store) => store
                .get(req)
                .map_err(|e| AgentError::memory(format!("get structured memory: {e}"))),
            Err(e) => Err(AgentError::memory(format!(
                "lock structured memory store for get: {e}"
            ))),
        }
    }

    pub(super) fn get_memory_for_agent(
        &self,
        requester_agent_id: &str,
        req: MemoryGetRequest,
    ) -> Result<Vec<MemoryRecord>, AgentError> {
        let mut records = if let Some(ref store_mutex) = self.structured_store {
            match store_mutex.lock() {
                Ok(store) => store
                    .get_visible_to_agent(requester_agent_id, req.clone())
                    .map_err(|e| AgentError::memory(format!("get structured memory: {e}")))?,
                Err(e) => {
                    return Err(AgentError::memory(format!(
                        "lock structured memory store for get: {e}"
                    )));
                }
            }
        } else {
            Vec::new()
        };

        let found = records
            .iter()
            .map(|record| record.id.clone())
            .collect::<HashSet<_>>();
        let missing_ids = req
            .ids
            .into_iter()
            .filter(|id| !found.contains(id))
            .collect::<Vec<_>>();
        if missing_ids.is_empty() {
            return Ok(records);
        }

        for (owner_agent_id, db_path) in self.external_memory_store_paths(requester_agent_id) {
            match rua_memory::StructuredMemoryStore::open(&db_path) {
                Ok(store) => {
                    let mut external = store
                        .get_visible_to_agent(
                            requester_agent_id,
                            MemoryGetRequest {
                                ids: missing_ids.clone(),
                                include_inactive: req.include_inactive,
                            },
                        )
                        .map_err(|e| {
                            AgentError::memory(format!(
                                "get shared structured memory from {owner_agent_id}: {e}"
                            ))
                        })?;
                    records.append(&mut external);
                }
                Err(e) => {
                    tracing::warn!(
                        owner_agent_id = %owner_agent_id,
                        path = %db_path.display(),
                        error = %e,
                        "failed to open shared structured Memory store for get"
                    );
                }
            }
        }

        Ok(records)
    }

    pub(super) fn list_memory(&self, agent_id: &str) -> Result<Vec<MemoryRecord>, AgentError> {
        let Some(ref store_mutex) = self.structured_store else {
            return Ok(Vec::new());
        };

        match store_mutex.lock() {
            Ok(store) => store
                .list_records_for_agent(agent_id, false)
                .map_err(|e| AgentError::memory(format!("list structured memory: {e}"))),
            Err(e) => Err(AgentError::memory(format!(
                "lock structured memory store for list: {e}"
            ))),
        }
    }

    pub(super) async fn update_memory(
        &self,
        req: MemoryUpdateRequest,
    ) -> Result<Option<MemoryRecord>, AgentError> {
        let Some(ref store_mutex) = self.structured_store else {
            return Ok(None);
        };

        let updated = match store_mutex.lock() {
            Ok(mut store) => store
                .update(req)
                .map_err(|e| AgentError::memory(format!("update structured memory: {e}"))),
            Err(e) => Err(AgentError::memory(format!(
                "lock structured memory store for update: {e}"
            ))),
        }?;

        if let Some(record) = updated.as_ref() {
            self.upsert_vec_index_for_record(record, "updated structured memory")
                .await;
            self.publish_committed_records(std::slice::from_ref(record))
                .await;
        }

        Ok(updated)
    }

    pub(super) async fn delete_memory_by_id(&self, memory_id: &str) -> Result<bool, AgentError> {
        let memory_id_obj = rua_memory::MemoryId::new(memory_id.to_string());
        let mut deleted = false;
        let mut deleted_agent_id = None;

        if let Some(ref store_mutex) = self.structured_store {
            match store_mutex.lock() {
                Ok(store) => {
                    let existing = store.get(MemoryGetRequest {
                        ids: vec![memory_id_obj.clone()],
                        include_inactive: true,
                    });
                    if let Ok(records) = existing {
                        let Some(record) = records.first() else {
                            return Ok(false);
                        };
                        if record.status == MemoryRecordStatus::Deleted {
                            return Ok(false);
                        }
                        deleted_agent_id = Some(record.agent_id.clone());
                    }
                    deleted |= store
                        .set_status(&memory_id_obj, MemoryRecordStatus::Deleted)
                        .map_err(|e| {
                            AgentError::memory(format!("delete structured memory {memory_id}: {e}"))
                        })?;
                }
                Err(e) => {
                    return Err(AgentError::memory(format!(
                        "lock structured memory store for delete: {e}"
                    )));
                }
            }
        }

        if let Some(ref vec_store_mutex) = self.vec_store {
            match vec_store_mutex.lock() {
                Ok(store) => {
                    deleted |= store.delete(memory_id).map_err(|e| {
                        AgentError::memory(format!("delete memory vec index {memory_id}: {e}"))
                    })?;
                }
                Err(e) => {
                    return Err(AgentError::memory(format!(
                        "lock vec store for memory delete: {e}"
                    )));
                }
            }
        }

        if deleted {
            if let Some(agent_id) = deleted_agent_id {
                self.publish_deleted_records(&agent_id, &[memory_id_obj])
                    .await;
            }
        }

        Ok(deleted)
    }

    async fn upsert_vec_index_for_record(&self, record: &MemoryRecord, label: &str) {
        let Some(ref vec_store_mutex) = self.vec_store else {
            return;
        };

        let embedder = self.current_embedder();
        let embedding = if let Some(ref embedder) = embedder {
            if embedder.dimension() > 0 {
                let semantic_text = structured_record_semantic_text(record);
                match embedder.embed(std::slice::from_ref(&semantic_text)).await {
                    Ok(mut embeddings) => {
                        embeddings.pop().filter(|embedding| !embedding.is_empty())
                    }
                    Err(e) => {
                        tracing::warn!("Failed to embed {label} for index: {e}");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        match vec_store_mutex.lock() {
            Ok(store) => {
                let mut document = structured_record_to_document(record, embedding);
                if let Some(embedder) = embedder {
                    document.metadata["embedding_model"] = serde_json::json!(embedder.model_id());
                }
                if let Err(e) = store.upsert(&document) {
                    tracing::warn!("Failed to upsert {label} to vec store: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("Failed to lock vec store for {label} upsert: {e}");
            }
        }
    }
}
