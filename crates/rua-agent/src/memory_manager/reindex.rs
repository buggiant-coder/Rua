use std::collections::HashSet;

use crate::error::AgentError;

use super::{LlmMemoryManager, MemoryRecord, MemoryStatus};

pub(super) fn structured_record_to_document(
    record: &MemoryRecord,
    embedding: Option<Vec<f32>>,
) -> sqlite_vecstore::Document {
    let semantic_text = structured_record_semantic_text(record);
    sqlite_vecstore::Document {
        id: record.id.to_string(),
        content: record.content.clone(),
        metadata: serde_json::json!({
            "id": record.id,
            "source": "structured",
            "kind": record.kind,
            "scope": record.scope,
            "title": record.title,
            "status": record.status,
            "confidence": record.confidence,
            "importance": record.importance,
            "updated_at": record.updated_at,
            "written_on_device_id": record.written_on_device_id,
            "written_on_device_name_snapshot": record.written_on_device_name_snapshot,
            "written_on_host_profile": record.written_on_host_profile,
            "resource_device_id": record.resource_device_id,
            "resource_device_name_snapshot": record.resource_device_name_snapshot,
            "semantic_text": semantic_text,
        }),
        embedding,
    }
}

pub(super) fn structured_record_semantic_text(record: &MemoryRecord) -> String {
    let mut parts = vec![record.title.clone(), record.content.clone()];
    parts.extend(record.terms.iter().map(|term| term.term.clone()));
    for resource in &record.resources {
        parts.push(resource.resource_kind.clone());
        parts.push(resource.locator.clone());
        parts.push(resource.normalized_locator.clone());
        if let Some(device_id) = resource.resource_device_id.as_ref() {
            parts.push(device_id.clone());
        }
        if let Some(device_name) = resource.resource_device_name_snapshot.as_ref() {
            parts.push(device_name.clone());
        }
        if let Some(host_profile) = resource.host_profile.as_ref() {
            parts.push(host_profile.clone());
        }
    }
    let mut seen = HashSet::new();
    parts
        .into_iter()
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .filter(|part| seen.insert(part.to_lowercase()))
        .collect::<Vec<_>>()
        .join("\n")
}

impl LlmMemoryManager {
    pub(super) fn memory_health_check(&self) -> MemoryStatus {
        let structured_count = self
            .structured_store
            .as_ref()
            .and_then(|m| m.lock().ok())
            .and_then(|s| s.count().ok())
            .unwrap_or(0);
        let vec_count = if self.structured_store.is_some() {
            0
        } else {
            self.vec_store
                .as_ref()
                .and_then(|m| m.lock().ok())
                .and_then(|s| s.count().ok())
                .unwrap_or(0)
        };

        MemoryStatus {
            healthy: true,
            document_count: vec_count + structured_count,
            index_size_bytes: 0,
            issues: vec![],
        }
    }

    pub(super) async fn reindex_memory(&self) -> Result<usize, AgentError> {
        let records = if let Some(ref store_mutex) = self.structured_store {
            match store_mutex.lock() {
                Ok(store) => {
                    store.rebuild_derived_indexes().map_err(|e| {
                        AgentError::memory(format!("rebuild structured index: {e}"))
                    })?;
                    store
                        .list_records(false)
                        .map_err(|e| AgentError::memory(format!("list structured memory: {e}")))?
                }
                Err(e) => {
                    return Err(AgentError::memory(format!(
                        "lock structured memory store for reindex: {e}"
                    )));
                }
            }
        } else {
            Vec::new()
        };

        if let Some(ref vec_store_mutex) = self.vec_store {
            let embedder = self.current_embedder();
            let embeddings = if let Some(ref embedder) = embedder {
                if embedder.dimension() > 0 && !records.is_empty() {
                    let semantic_texts = records
                        .iter()
                        .map(structured_record_semantic_text)
                        .collect::<Vec<_>>();
                    match embedder.embed(&semantic_texts).await {
                        Ok(embeddings) if embeddings.len() == records.len() => embeddings
                            .into_iter()
                            .map(|embedding| (!embedding.is_empty()).then_some(embedding))
                            .collect::<Vec<_>>(),
                        Ok(embeddings) => {
                            tracing::warn!(
                                expected = records.len(),
                                actual = embeddings.len(),
                                "structured Memory reindex embedder returned wrong number of embeddings"
                            );
                            vec![None; records.len()]
                        }
                        Err(e) => {
                            tracing::warn!("Failed to embed structured memory during reindex: {e}");
                            vec![None; records.len()]
                        }
                    }
                } else {
                    vec![None; records.len()]
                }
            } else {
                vec![None; records.len()]
            };
            let docs = records
                .iter()
                .zip(embeddings)
                .map(|(record, embedding)| {
                    let mut document = structured_record_to_document(record, embedding);
                    if let Some(embedder) = embedder.as_ref() {
                        document.metadata["embedding_model"] =
                            serde_json::json!(embedder.model_id());
                    }
                    document
                })
                .collect::<Vec<_>>();
            match vec_store_mutex.lock() {
                Ok(mut store) => {
                    store
                        .clear()
                        .map_err(|e| AgentError::memory(format!("clear memory vec index: {e}")))?;
                    store.batch_upsert(&docs).map_err(|e| {
                        AgentError::memory(format!("rebuild memory vec index: {e}"))
                    })?;
                }
                Err(e) => {
                    return Err(AgentError::memory(format!(
                        "lock vec store for memory reindex: {e}"
                    )));
                }
            }
        }

        Ok(records.len())
    }
}
