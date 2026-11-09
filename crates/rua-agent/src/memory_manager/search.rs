use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::error::AgentError;

use super::{LlmMemoryManager, MemoryGetRequest, MemorySearchHit, MemorySearchRequest};
use rua_memory::MemoryRecordStatus;

impl LlmMemoryManager {
    pub(super) async fn search_memory(
        &self,
        req: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchHit>, AgentError> {
        if req.search_mode == rua_config::MemorySearchMode::Grep {
            return self.memory_grep_search(req);
        }

        let mut routes = Vec::new();

        if let Some(ref store_mutex) = self.structured_store {
            match store_mutex.lock() {
                Ok(store) => {
                    let hits = store
                        .search(&req.agent_id, &req.query, req.top_k)
                        .map_err(|e| AgentError::memory(format!("search structured memory: {e}")))?
                        .into_iter()
                        .map(|(record, score)| structured_record_to_memory_hit(record, score))
                        .collect::<Vec<_>>();
                    if !hits.is_empty() {
                        routes.push(hits);
                    }
                }
                Err(e) => {
                    return Err(AgentError::memory(format!(
                        "lock structured memory store for search: {e}"
                    )));
                }
            }
        }

        for (owner_agent_id, db_path) in self.external_memory_store_paths(&req.agent_id) {
            match rua_memory::StructuredMemoryStore::open(&db_path) {
                Ok(store) => {
                    let hits = store
                        .search_visible_to_agent(&req.agent_id, &req.query, req.top_k)
                        .map_err(|e| {
                            AgentError::memory(format!(
                                "search shared structured memory from {owner_agent_id}: {e}"
                            ))
                        })?
                        .into_iter()
                        .filter(|(record, _)| record.agent_id != req.agent_id)
                        .map(|(record, score)| structured_record_to_memory_hit(record, score))
                        .collect::<Vec<_>>();
                    if !hits.is_empty() {
                        routes.push(hits);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        owner_agent_id = %owner_agent_id,
                        path = %db_path.display(),
                        error = %e,
                        "failed to open shared structured Memory store"
                    );
                }
            }
        }

        let Some(ref vec_store_mutex) = self.vec_store else {
            return Ok(merge_hybrid_hits(routes, req.top_k));
        };

        let embedder = self.current_embedder().ok_or_else(|| {
            AgentError::memory(
                "Embedding resource unavailable; saved configuration has not been applied",
            )
        })?;
        {
            if embedder.dimension() > 0 {
                let compatible = vec_store_mutex
                    .lock()
                    .map_err(|error| AgentError::memory(error.to_string()))?
                    .embeddings_match(embedder.model_id(), embedder.dimension())
                    .map_err(|error| AgentError::memory(error.to_string()))?;
                if !compatible {
                    return Err(AgentError::memory("Vector search unavailable: index does not match the configured embedding model"));
                }
                match embedder.embed(std::slice::from_ref(&req.query)).await {
                    Ok(mut vecs) => {
                        if let Some(query_emb) = vecs.pop() {
                            if !query_emb.is_empty() {
                                match vec_store_mutex.lock() {
                                    Ok(store) => {
                                        if !store
                                            .embeddings_match(
                                                embedder.model_id(),
                                                embedder.dimension(),
                                            )
                                            .map_err(|error| {
                                                AgentError::memory(error.to_string())
                                            })?
                                        {
                                            return Err(AgentError::memory("Vector search unavailable: index changed to an incompatible embedding model"));
                                        }
                                        match store.vector_search(&query_emb, req.top_k) {
                                            Ok(results) if !results.is_empty() => {
                                                routes.push(
                                                    results
                                                        .into_iter()
                                                        .map(|r| {
                                                            vecstore_hit_to_memory_hit(r, "vector")
                                                        })
                                                        .collect(),
                                                );
                                            }
                                            Ok(_) => {}
                                            Err(e) => {
                                                return Err(AgentError::memory(format!(
                                                    "Vector search unavailable: {e}"
                                                )))
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "Failed to lock vec store for vector search: {e}"
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        return Err(AgentError::memory(format!(
                            "Embedding resource unavailable: {e}"
                        )));
                    }
                }
            }
        }

        match vec_store_mutex.lock() {
            Ok(store) => match store.text_search(&req.query, req.top_k) {
                Ok(results) if !results.is_empty() => {
                    routes.push(
                        results
                            .into_iter()
                            .map(|r| vecstore_hit_to_memory_hit(r, "text"))
                            .collect(),
                    );
                    Ok(merge_hybrid_hits(routes, req.top_k))
                }
                Ok(_) => Ok(merge_hybrid_hits(routes, req.top_k)),
                Err(e) => {
                    tracing::warn!("Text search failed: {e}");
                    Ok(merge_hybrid_hits(routes, req.top_k))
                }
            },
            Err(e) => {
                tracing::warn!("Failed to lock vec store for text search: {e}");
                Ok(merge_hybrid_hits(routes, req.top_k))
            }
        }
    }

    pub(super) fn memory_grep_search(
        &self,
        req: MemorySearchRequest,
    ) -> Result<Vec<MemorySearchHit>, AgentError> {
        let mut records = Vec::new();

        if let Some(ref store_mutex) = self.structured_store {
            match store_mutex.lock() {
                Ok(store) => {
                    let mut local = store
                        .list_records_for_agent(&req.agent_id, false)
                        .map_err(|e| AgentError::memory(format!("grep structured memory: {e}")))?;
                    records.append(&mut local);
                }
                Err(e) => {
                    return Err(AgentError::memory(format!(
                        "lock structured memory store for grep search: {e}"
                    )));
                }
            }
        }

        for (owner_agent_id, db_path) in self.external_memory_store_paths(&req.agent_id) {
            match rua_memory::StructuredMemoryStore::open(&db_path) {
                Ok(store) => {
                    let visible_ids = store
                        .list_records(false)
                        .map_err(|e| {
                            AgentError::memory(format!(
                                "list shared structured memory from {owner_agent_id}: {e}"
                            ))
                        })?
                        .into_iter()
                        .map(|record| record.id)
                        .collect::<Vec<_>>();
                    let mut external = store
                        .get_visible_to_agent(
                            &req.agent_id,
                            MemoryGetRequest {
                                ids: visible_ids,
                                include_inactive: false,
                            },
                        )
                        .map_err(|e| {
                            AgentError::memory(format!(
                                "load shared structured memory from {owner_agent_id}: {e}"
                            ))
                        })?
                        .into_iter()
                        .filter(|record| record.agent_id != req.agent_id)
                        .collect::<Vec<_>>();
                    records.append(&mut external);
                }
                Err(e) => {
                    tracing::warn!(
                        owner_agent_id = %owner_agent_id,
                        path = %db_path.display(),
                        error = %e,
                        "failed to open shared structured Memory store for grep search"
                    );
                }
            }
        }

        Ok(grep_memory_records(records, &req.query, req.top_k))
    }
}

fn vecstore_hit_to_memory_hit(
    result: sqlite_vecstore::SearchResult,
    source: &'static str,
) -> MemorySearchHit {
    let mut metadata = match result.document.metadata {
        serde_json::Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("value".into(), other);
            map
        }
    };
    metadata.insert("id".into(), serde_json::Value::String(result.document.id));

    MemorySearchHit {
        content: result.document.content,
        score: result.score,
        source: source.into(),
        metadata: serde_json::Value::Object(metadata),
    }
}

fn structured_record_to_memory_hit(
    record: rua_memory::MemoryRecord,
    score: f64,
) -> MemorySearchHit {
    let mut metadata = serde_json::Map::new();
    metadata.insert("id".into(), serde_json::json!(record.id));
    metadata.insert("agent_id".into(), serde_json::json!(record.agent_id));
    metadata.insert("kind".into(), serde_json::json!(record.kind));
    metadata.insert("scope".into(), serde_json::json!(record.scope));
    metadata.insert("title".into(), serde_json::json!(record.title));
    metadata.insert("status".into(), serde_json::json!(record.status));
    metadata.insert("confidence".into(), serde_json::json!(record.confidence));
    metadata.insert("importance".into(), serde_json::json!(record.importance));
    metadata.insert("updated_at".into(), serde_json::json!(record.updated_at));
    metadata.insert(
        "written_on_device_id".into(),
        serde_json::json!(record.written_on_device_id),
    );
    metadata.insert(
        "written_on_device_name_snapshot".into(),
        serde_json::json!(record.written_on_device_name_snapshot),
    );
    metadata.insert(
        "written_on_host_profile".into(),
        serde_json::json!(record.written_on_host_profile),
    );
    metadata.insert(
        "resource_device_id".into(),
        serde_json::json!(record.resource_device_id),
    );
    metadata.insert(
        "resource_device_name_snapshot".into(),
        serde_json::json!(record.resource_device_name_snapshot),
    );
    metadata.insert("terms".into(), serde_json::json!(record.terms));
    metadata.insert("sources".into(), serde_json::json!(record.sources));
    metadata.insert("resources".into(), serde_json::json!(record.resources));
    let conflicts = record
        .relations
        .iter()
        .filter(|relation| relation.relation == "contradicts")
        .cloned()
        .collect::<Vec<_>>();
    metadata.insert("relations".into(), serde_json::json!(record.relations));
    metadata.insert("conflicts".into(), serde_json::json!(conflicts));

    MemorySearchHit {
        content: record.content,
        score,
        source: "structured".into(),
        metadata: serde_json::Value::Object(metadata),
    }
}

struct GrepMatch {
    score: f64,
    matched_terms: Vec<String>,
}

fn grep_memory_records(
    records: Vec<rua_memory::MemoryRecord>,
    query: &str,
    limit: usize,
) -> Vec<MemorySearchHit> {
    if limit == 0 {
        return Vec::new();
    }
    let terms = grep_query_terms(query);
    if terms.is_empty() {
        return Vec::new();
    }

    let query_lower = query.trim().to_lowercase();
    let mut hits = records
        .into_iter()
        .filter(|record| record.status == MemoryRecordStatus::Active)
        .filter_map(|record| {
            let text = memory_grep_text(&record).to_lowercase();
            let matched = grep_match_score(&text, &query_lower, &terms)?;
            let updated_at = record
                .updated_at
                .as_ref()
                .unwrap_or(&record.created_at)
                .timestamp_millis();
            let mut hit = structured_record_to_memory_hit(record, matched.score);
            hit.source = "grep".into();
            if let serde_json::Value::Object(metadata) = &mut hit.metadata {
                metadata.insert("matched_by".into(), serde_json::json!(["grep"]));
                metadata.insert(
                    "matched_terms".into(),
                    serde_json::json!(matched.matched_terms),
                );
            }
            Some((hit, updated_at))
        })
        .collect::<Vec<_>>();

    hits.sort_by(|(left_hit, left_time), (right_hit, right_time)| {
        right_hit
            .score
            .partial_cmp(&left_hit.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right_time.cmp(left_time))
    });
    hits.truncate(limit);
    hits.into_iter().map(|(hit, _)| hit).collect()
}

fn grep_query_terms(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut terms = query
        .split_whitespace()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .filter(|term| seen.insert(term.clone()))
        .collect::<Vec<_>>();
    if terms.is_empty() {
        let query = query.trim().to_lowercase();
        if !query.is_empty() {
            terms.push(query);
        }
    }
    terms
}

fn grep_match_score(text: &str, query: &str, terms: &[String]) -> Option<GrepMatch> {
    let mut occurrence_count = 0usize;
    let mut matched_terms = Vec::new();
    for term in terms {
        let count = count_occurrences(text, term);
        if count == 0 {
            continue;
        }
        occurrence_count += count;
        matched_terms.push(term.clone());
    }

    let phrase_bonus = if !query.is_empty() && text.contains(query) {
        5.0
    } else {
        0.0
    };
    if occurrence_count == 0 && phrase_bonus == 0.0 {
        return None;
    }

    Some(GrepMatch {
        score: phrase_bonus + matched_terms.len() as f64 + (occurrence_count as f64 * 0.1),
        matched_terms,
    })
}

fn count_occurrences(text: &str, term: &str) -> usize {
    if term.is_empty() {
        0
    } else {
        text.match_indices(term).count()
    }
}

fn memory_grep_text(record: &rua_memory::MemoryRecord) -> String {
    let mut parts = Vec::new();
    push_grep_part(&mut parts, record.id.to_string());
    push_grep_part(&mut parts, record.agent_id.clone());
    push_grep_json(&mut parts, &record.kind);
    push_grep_json(&mut parts, &record.scope);
    push_grep_part(&mut parts, Some(record.title.clone()));
    push_grep_part(&mut parts, record.content.clone());
    push_grep_json(&mut parts, &record.status);
    push_grep_part(&mut parts, record.written_on_device_id.clone());
    push_grep_part(&mut parts, record.written_on_device_name_snapshot.clone());
    push_grep_part(&mut parts, record.written_on_host_profile.clone());
    push_grep_part(&mut parts, record.resource_device_id.clone());
    push_grep_part(&mut parts, record.resource_device_name_snapshot.clone());
    push_grep_json(&mut parts, &record.metadata);

    for term in &record.terms {
        push_grep_part(&mut parts, term.term.clone());
        push_grep_part(&mut parts, term.normalized_term.clone());
        push_grep_json(&mut parts, &term.term_kind);
        push_grep_json(&mut parts, &term.source);
        push_grep_part(&mut parts, term.language.clone());
    }

    for resource in &record.resources {
        push_grep_part(&mut parts, resource.resource_kind.clone());
        push_grep_part(&mut parts, resource.locator.clone());
        push_grep_part(&mut parts, resource.normalized_locator.clone());
        push_grep_part(&mut parts, resource.resource_device_id.clone());
        push_grep_part(&mut parts, resource.resource_device_name_snapshot.clone());
        push_grep_part(&mut parts, resource.host_profile.clone());
        push_grep_json(&mut parts, &resource.metadata);
    }

    for source in &record.sources {
        push_grep_part(&mut parts, source.chat_id.clone());
        push_grep_part(&mut parts, source.turn_id.clone());
        push_grep_part(&mut parts, source.message_id.clone());
        push_grep_part(&mut parts, source.quote.clone());
    }

    for relation in &record.relations {
        push_grep_part(&mut parts, relation.from_id.to_string());
        push_grep_part(&mut parts, relation.to_id.to_string());
        push_grep_part(&mut parts, relation.relation.clone());
    }

    parts.join("\n")
}

fn push_grep_part(parts: &mut Vec<String>, value: impl Into<Option<String>>) {
    if let Some(value) = value.into().map(|value| value.trim().to_string()) {
        if !value.is_empty() {
            parts.push(value);
        }
    }
}

fn push_grep_json<T: Serialize>(parts: &mut Vec<String>, value: &T) {
    if let Ok(value) = serde_json::to_string(value) {
        push_grep_part(parts, value);
    }
}

fn hit_key(hit: &MemorySearchHit) -> String {
    hit.metadata
        .get("id")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{}:{}", hit.source, hit.content))
}

fn merge_hybrid_hits(routes: Vec<Vec<MemorySearchHit>>, limit: usize) -> Vec<MemorySearchHit> {
    if limit == 0 {
        return Vec::new();
    }

    struct Entry {
        hit: MemorySearchHit,
        fused_score: f64,
        sources: HashSet<String>,
    }

    let mut entries: HashMap<String, Entry> = HashMap::new();
    for route in routes {
        for (rank, hit) in route.into_iter().enumerate() {
            let source = hit.source.clone();
            let key = hit_key(&hit);
            let route_weight = match source.as_str() {
                "structured" => 1.25,
                "text" => 0.85,
                _ => 1.0,
            };
            let route_score = route_weight / (60.0 + (rank + 1) as f64);
            let original_score = hit.score.max(0.0).ln_1p() * 0.001;
            let entry = entries.entry(key).or_insert_with(|| Entry {
                hit: hit.clone(),
                fused_score: 0.0,
                sources: HashSet::new(),
            });
            entry.fused_score += route_score + original_score;
            entry.sources.insert(source);
            if hit.source == "structured" || entry.hit.source != "structured" {
                entry.hit = hit;
            }
        }
    }

    let mut hits = entries
        .into_values()
        .map(|mut entry| {
            let mut sources = entry.sources.into_iter().collect::<Vec<_>>();
            sources.sort();
            entry.hit.score = entry.fused_score;
            entry.hit.source = if sources.len() == 1 {
                sources[0].clone()
            } else {
                format!("hybrid:{}", sources.join("+"))
            };
            if let serde_json::Value::Object(ref mut metadata) = entry.hit.metadata {
                metadata.insert("matched_by".into(), serde_json::json!(sources));
            }
            entry.hit
        })
        .collect::<Vec<_>>();

    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);
    hits
}
