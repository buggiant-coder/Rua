use crate::error::AgentError;

use super::{
    DreamMemoryResult, LlmMemoryManager, MemoryBatchApplyOutcome, MemoryBatchApplyRequest,
    MemoryGetRequest, MemoryMaintenanceCandidateRequest, MemoryMaintenancePacket,
};

impl LlmMemoryManager {
    pub(super) fn dream_index_memory(&self) -> DreamMemoryResult {
        let Some(ref vec_store_mutex) = self.vec_store else {
            return empty_dream_result();
        };

        let docs = match vec_store_mutex.lock() {
            Ok(store) => match store.list_recent(50) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("dream_memory: failed to list recent docs: {e}");
                    return empty_dream_result();
                }
            },
            Err(e) => {
                tracing::warn!("dream_memory: failed to lock vec store: {e}");
                return empty_dream_result();
            }
        };

        if docs.is_empty() {
            return empty_dream_result();
        }

        let embedded_docs: Vec<(&sqlite_vecstore::Document, &Vec<f32>)> = docs
            .iter()
            .filter_map(|d| d.embedding.as_ref().map(|e| (d, e)))
            .collect();

        if embedded_docs.is_empty() {
            let combined: String = docs
                .iter()
                .map(|d| d.content.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let snippet = if combined.len() > 200 {
                &combined[..200]
            } else {
                &combined
            };
            return DreamMemoryResult {
                items_consolidated: docs.len(),
                new_insights: vec![format!(
                    "Consolidated {n} Memory index documents: {snippet}",
                    n = docs.len()
                )],
            };
        }

        let threshold = 0.75;
        let mut clusters: Vec<Vec<usize>> = vec![];
        let mut centroids: Vec<Vec<f32>> = vec![];

        for (idx, (_, emb)) in embedded_docs.iter().enumerate() {
            let mut best_cluster = None;
            let mut best_sim = 0.0f64;
            for (ci, centroid) in centroids.iter().enumerate() {
                let sim = sqlite_vecstore::cosine_similarity(centroid, emb);
                if sim > best_sim {
                    best_sim = sim;
                    best_cluster = Some(ci);
                }
            }

            if best_sim >= threshold {
                let ci = best_cluster.unwrap();
                clusters[ci].push(idx);
                let n = clusters[ci].len() as f32;
                for (i, v) in centroids[ci].iter_mut().enumerate() {
                    *v = *v * ((n - 1.0) / n) + emb[i] / n;
                }
            } else {
                clusters.push(vec![idx]);
                centroids.push(emb.to_vec());
            }
        }

        let mut insights = Vec::new();
        for cluster in &clusters {
            if cluster.len() < 2 {
                continue;
            }
            let texts: Vec<&str> = cluster
                .iter()
                .take(5)
                .map(|&i| embedded_docs[i].0.content.as_str())
                .collect();
            let combined = texts.join(" | ");
            let snippet = if combined.len() > 300 {
                &combined[..300]
            } else {
                &combined
            };
            insights.push(format!("Cluster of {} items: {snippet}", cluster.len()));
        }

        DreamMemoryResult {
            items_consolidated: embedded_docs.len(),
            new_insights: insights,
        }
    }

    pub(super) fn memory_maintenance_candidates(
        &self,
        req: MemoryMaintenanceCandidateRequest,
    ) -> Result<MemoryMaintenancePacket, AgentError> {
        let Some(ref store_mutex) = self.structured_store else {
            return Ok(MemoryMaintenancePacket {
                base_revision: "rev:0".to_string(),
                clusters: vec![],
            });
        };

        let mut packet = match store_mutex.lock() {
            Ok(store) => store
                .maintenance_candidates(req)
                .map_err(|e| AgentError::memory(format!("memory maintenance candidates: {e}"))),
            Err(e) => Err(AgentError::memory(format!(
                "lock structured memory store for maintenance candidates: {e}"
            ))),
        }?;

        for cluster in &mut packet.clusters {
            let Some(first) = cluster.records.first() else {
                continue;
            };
            let scope = serde_json::to_value(first.scope)
                .ok()
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_default();
            let kind = serde_json::to_value(first.kind)
                .ok()
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_default();
            for (owner_agent_id, db_path) in self.external_memory_store_paths(&first.agent_id) {
                match rua_memory::StructuredMemoryStore::open(&db_path) {
                    Ok(store) => {
                        let mut external = store
                            .visible_records_for_group(
                                &first.agent_id,
                                &scope,
                                &kind,
                                &first.title,
                                5,
                            )
                            .map_err(|e| {
                                AgentError::memory(format!(
                                    "load shared AutoDream context from {owner_agent_id}: {e}"
                                ))
                            })?;
                        external.retain(|record| record.agent_id != first.agent_id);
                        cluster.external_context.append(&mut external);
                    }
                    Err(e) => {
                        tracing::warn!(
                            owner_agent_id = %owner_agent_id,
                            path = %db_path.display(),
                            error = %e,
                            "failed to open shared structured Memory store for AutoDream context"
                        );
                    }
                }
            }
        }

        Ok(packet)
    }

    pub(super) async fn apply_memory_batch(
        &self,
        req: MemoryBatchApplyRequest,
    ) -> Result<MemoryBatchApplyOutcome, AgentError> {
        let Some(ref store_mutex) = self.structured_store else {
            return Ok(MemoryBatchApplyOutcome {
                applied: false,
                reason: Some("structured memory store is not configured".to_string()),
                records: vec![],
            });
        };

        let superseded_ids = req
            .ops
            .iter()
            .flat_map(|op| match op {
                rua_memory::MemoryBatchOperation::Create { supersedes, .. } => supersedes.clone(),
                rua_memory::MemoryBatchOperation::Supersede { ids, .. } => ids.clone(),
                rua_memory::MemoryBatchOperation::Relate { relations } => relations
                    .iter()
                    .flat_map(|relation| [relation.from_id.clone(), relation.to_id.clone()])
                    .collect(),
                rua_memory::MemoryBatchOperation::SetStatus { ids, .. } => ids.clone(),
            })
            .collect::<Vec<_>>();

        let outcome = match store_mutex.lock() {
            Ok(mut store) => store
                .batch_apply(req)
                .map_err(|e| AgentError::memory(format!("memory batch apply: {e}"))),
            Err(e) => Err(AgentError::memory(format!(
                "lock structured memory store for batch apply: {e}"
            ))),
        }?;

        if outcome.applied {
            let mut changed_records = outcome.records.clone();
            if !superseded_ids.is_empty() {
                if let Some(ref store_mutex) = self.structured_store {
                    match store_mutex.lock() {
                        Ok(store) => match store.get(MemoryGetRequest {
                            ids: superseded_ids,
                            include_inactive: true,
                        }) {
                            Ok(mut records) => changed_records.append(&mut records),
                            Err(e) => tracing::warn!(
                                error = %e,
                                "failed to load superseded Memory records for publication"
                            ),
                        },
                        Err(e) => tracing::warn!(
                            error = %e,
                            "failed to lock structured Memory store for maintenance publication"
                        ),
                    }
                }
            }
            self.publish_committed_records(&changed_records).await;
        }

        Ok(outcome)
    }
}

fn empty_dream_result() -> DreamMemoryResult {
    DreamMemoryResult {
        items_consolidated: 0,
        new_insights: vec![],
    }
}
