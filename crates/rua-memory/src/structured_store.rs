use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{MemoryError, MemoryResult};
use crate::types::{
    normalize_memory_title, MemoryBatchApplyOutcome, MemoryBatchApplyRequest,
    MemoryBatchCreateRecord, MemoryBatchOperation, MemoryBatchRelation, MemoryGetRequest, MemoryId,
    MemoryLink, MemoryMaintenanceCandidateRequest, MemoryMaintenanceCluster,
    MemoryMaintenancePacket, MemoryRecord, MemoryRecordStatus, MemoryRelation, MemoryResource,
    MemorySearchDoc, MemorySlot, MemorySlotItem, MemorySlotSnapshot, MemorySourceEvidence,
    MemoryTerm, MemoryTermKind, MemoryTermSource, MemoryUpdateRequest, MemoryVisibility,
    MemoryVisibilityGrant, MemoryVisibilityMode, MemoryVisibilityUpdateRequest, MemoryWriteOutcome,
    MemoryWriteRequest,
};

pub struct StructuredMemoryStore {
    conn: Connection,
}

impl StructuredMemoryStore {
    pub fn open(path: impl AsRef<Path>) -> MemoryResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        let store = Self { conn };
        init_schema(&store)?;
        Ok(store)
    }

    pub fn open_in_memory() -> MemoryResult<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        init_schema(&store)?;
        Ok(store)
    }

    pub fn write(&mut self, req: MemoryWriteRequest) -> MemoryResult<MemoryWriteOutcome> {
        let content = req.content.trim();
        if content.is_empty() {
            return Ok(MemoryWriteOutcome {
                rejected: true,
                reason: Some("memory content is empty".to_string()),
                record: None,
            });
        }
        let title = match normalize_memory_title(&req.title) {
            Ok(title) => title,
            Err(reason) => {
                return Ok(MemoryWriteOutcome {
                    rejected: true,
                    reason: Some(reason),
                    record: None,
                });
            }
        };
        let now = Utc::now();
        let id = MemoryId::new(format!("mem_{}", uuid::Uuid::new_v4().simple()));
        let expires_at = req.ttl_days.map(|days| now + Duration::days(days));
        let metadata_json = serde_json::to_string(&req.metadata)?;
        let confidence = req.confidence.unwrap_or(1.0);
        let device_context = memory_device_context_from_metadata(&req.metadata);
        let terms = write_terms_from_inputs(&req.labels, &req.search_terms, &req.metadata, now);
        let resources = detect_memory_resources(content, &device_context, now);
        let search_doc = build_search_doc(
            &id,
            MemorySearchText {
                title: &title,
                content,
            },
            &req.metadata,
            &terms,
            &resources,
            now,
        );

        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO memory_records (
                id, agent_id, scope, kind, title, content, status, confidence,
                importance, created_at, updated_at, expires_at, written_on_device_id,
                written_on_device_name_snapshot, written_on_host_profile, resource_device_id,
                resource_device_name_snapshot, metadata_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                id.0,
                req.agent_id,
                enum_name(&req.scope)?,
                enum_name(&req.kind)?,
                title,
                content,
                enum_name(&MemoryRecordStatus::Active)?,
                confidence,
                req.importance,
                now.to_rfc3339(),
                now.to_rfc3339(),
                expires_at.map(|dt| dt.to_rfc3339()),
                device_context.written_on_device_id,
                device_context.written_on_device_name_snapshot,
                device_context.written_on_host_profile,
                device_context.resource_device_id,
                device_context.resource_device_name_snapshot,
                metadata_json,
            ],
        )?;

        if let Some(evidence) = req.evidence.as_ref() {
            tx.execute(
                "INSERT INTO memory_sources (
                    memory_id, chat_id, turn_id, message_id, quote, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id.0,
                    evidence.chat_id,
                    evidence.turn_id,
                    evidence.message_id,
                    evidence.quote,
                    now.to_rfc3339(),
                ],
            )?;
        }
        insert_terms_tx(&tx, &id, &terms)?;
        insert_resources_tx(&tx, &id, &resources)?;
        insert_search_doc_tx(&tx, &search_doc)?;

        tx.execute(
            "INSERT INTO memory_ops (
                op_id, actor, op_kind, payload_json, sync_status, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                format!("op_{}", uuid::Uuid::new_v4().simple()),
                "agent",
                "create",
                serde_json::to_string(&req)?,
                "pending",
                now.to_rfc3339(),
            ],
        )?;
        tx.execute(
            "INSERT INTO memory_records_fts (id, content) VALUES (?1, ?2)",
            params![id.0, search_doc.lexical_text],
        )?;
        tx.commit()?;

        let record = self.get_one(&id, true)?.ok_or_else(|| {
            MemoryError::Invalid(format!("created memory record missing after commit: {id}"))
        })?;
        Ok(MemoryWriteOutcome {
            rejected: false,
            reason: None,
            record: Some(record),
        })
    }

    pub fn get(&self, req: MemoryGetRequest) -> MemoryResult<Vec<MemoryRecord>> {
        let mut out = Vec::new();
        for id in req.ids {
            if let Some(record) = self.get_one(&id, req.include_inactive)? {
                out.push(record);
            }
        }
        Ok(out)
    }

    pub fn update(&mut self, req: MemoryUpdateRequest) -> MemoryResult<Option<MemoryRecord>> {
        let content = req.content.trim();
        if content.is_empty() {
            return Err(MemoryError::Invalid("memory content is empty".to_string()));
        }
        let title = normalize_memory_title(&req.title).map_err(MemoryError::Invalid)?;

        let Some(existing) = self.get_one(&req.id, false)? else {
            return Ok(None);
        };
        if existing.agent_id != req.agent_id {
            return Ok(None);
        }
        let now = Utc::now();
        let mut terms = self.terms_for(&req.id)?;
        if let Some(search_terms) = req.search_terms.as_ref() {
            terms.retain(|term| {
                term.term_kind != MemoryTermKind::SearchTerm
                    || term.source != MemoryTermSource::Agent
            });
            push_memory_terms(
                &mut terms,
                search_terms.iter().map(String::as_str),
                MemoryTermKind::SearchTerm,
                MemoryTermSource::Agent,
                now,
            );
        }
        let resources = detect_memory_resources(
            content,
            &memory_device_context_from_metadata(&existing.metadata),
            now,
        );
        let search_doc = build_search_doc(
            &req.id,
            MemorySearchText {
                title: &title,
                content,
            },
            &existing.metadata,
            &terms,
            &resources,
            now,
        );
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE memory_records
             SET scope = ?2,
                 kind = ?3,
                 title = ?4,
                 content = ?5,
                 confidence = ?6,
                 importance = ?7,
                 updated_at = ?8
             WHERE id = ?1 AND agent_id = ?9 AND status = ?10",
            params![
                req.id.0,
                enum_name(&req.scope)?,
                enum_name(&req.kind)?,
                title,
                content,
                req.confidence,
                req.importance,
                now.to_rfc3339(),
                req.agent_id,
                enum_name(&MemoryRecordStatus::Active)?,
            ],
        )?;
        if changed == 0 {
            tx.rollback()?;
            return Ok(None);
        }
        tx.execute(
            "DELETE FROM memory_records_fts WHERE id = ?1",
            params![req.id.0],
        )?;
        tx.execute(
            "DELETE FROM memory_search_docs_fts WHERE id = ?1",
            params![req.id.0],
        )?;
        tx.execute(
            "DELETE FROM memory_search_docs WHERE memory_id = ?1",
            params![req.id.0],
        )?;
        tx.execute(
            "DELETE FROM memory_resources WHERE memory_id = ?1",
            params![req.id.0],
        )?;
        if req.search_terms.is_some() {
            tx.execute(
                "DELETE FROM memory_terms WHERE memory_id = ?1",
                params![req.id.0],
            )?;
            insert_terms_tx(&tx, &req.id, &terms)?;
        }
        insert_resources_tx(&tx, &req.id, &resources)?;
        insert_search_doc_tx(&tx, &search_doc)?;
        tx.execute(
            "INSERT INTO memory_records_fts (id, content) VALUES (?1, ?2)",
            params![req.id.0, search_doc.lexical_text],
        )?;
        tx.execute(
            "INSERT INTO memory_ops (
                op_id, actor, op_kind, payload_json, sync_status, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                format!("op_{}", uuid::Uuid::new_v4().simple()),
                "user",
                "update",
                serde_json::to_string(&req)?,
                "pending",
                now.to_rfc3339(),
            ],
        )?;
        tx.commit()?;
        self.get_one(&req.id, true)
    }

    pub fn list_records_for_agent(
        &self,
        agent_id: &str,
        include_deleted: bool,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        let sql = if include_deleted {
            "SELECT id FROM memory_records
             WHERE agent_id = ?1
             ORDER BY updated_at DESC, created_at DESC"
        } else {
            "SELECT id FROM memory_records
             WHERE agent_id = ?1 AND status != ?2
             ORDER BY updated_at DESC, created_at DESC"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let ids = if include_deleted {
            stmt.query_map(params![agent_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(
                params![agent_id, enum_name(&MemoryRecordStatus::Deleted)?],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?
        };
        let mut records = Vec::new();
        for id in ids {
            if let Some(record) = self.get_one(&MemoryId::new(id), true)? {
                records.push(record);
            }
        }
        Ok(records)
    }

    pub fn list_records(&self, include_deleted: bool) -> MemoryResult<Vec<MemoryRecord>> {
        let sql = if include_deleted {
            "SELECT id FROM memory_records
             ORDER BY updated_at DESC, created_at DESC"
        } else {
            "SELECT id FROM memory_records
             WHERE status != ?1
             ORDER BY updated_at DESC, created_at DESC"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let ids = if include_deleted {
            stmt.query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![enum_name(&MemoryRecordStatus::Deleted)?], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        let mut records = Vec::new();
        for id in ids {
            if let Some(record) = self.get_one(&MemoryId::new(id), true)? {
                records.push(record);
            }
        }
        Ok(records)
    }

    pub fn visibility_for_memory(
        &self,
        owner_agent_id: &str,
        id: &MemoryId,
    ) -> MemoryResult<Option<MemoryVisibility>> {
        let Some(record) = self.get_one(id, true)? else {
            return Ok(None);
        };
        if record.agent_id != owner_agent_id {
            return Ok(None);
        }
        let grants = self.visibility_grants_for(id)?;
        Ok(Some(visibility_from_grants(
            id.clone(),
            owner_agent_id.to_string(),
            grants,
        )))
    }

    pub fn list_visibility_for_agent(
        &self,
        owner_agent_id: &str,
    ) -> MemoryResult<Vec<MemoryVisibility>> {
        let records = self.list_records_for_agent(owner_agent_id, false)?;
        let mut out = Vec::with_capacity(records.len());
        for record in records {
            let grants = self.visibility_grants_for(&record.id)?;
            out.push(visibility_from_grants(
                record.id,
                owner_agent_id.to_string(),
                grants,
            ));
        }
        Ok(out)
    }

    pub fn replace_visibility(
        &self,
        owner_agent_id: &str,
        id: &MemoryId,
        req: MemoryVisibilityUpdateRequest,
        actor: &str,
    ) -> MemoryResult<Option<MemoryVisibility>> {
        let Some(record) = self.get_one(id, true)? else {
            return Ok(None);
        };
        if record.agent_id != owner_agent_id {
            return Ok(None);
        }

        let grantees = normalized_visibility_grantees(owner_agent_id, &req);
        let now = Utc::now();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM memory_visibility_grants WHERE memory_id = ?1",
            params![id.0],
        )?;
        for grantee in &grantees {
            tx.execute(
                "INSERT INTO memory_visibility_grants (
                    memory_id, owner_agent_id, grantee_agent_id,
                    permission, created_by, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id.0,
                    owner_agent_id,
                    grantee,
                    "read",
                    actor,
                    now.to_rfc3339(),
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO memory_ops (
                op_id, actor, op_kind, payload_json, sync_status, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                format!("op_{}", uuid::Uuid::new_v4().simple()),
                actor,
                "visibility_update",
                serde_json::to_string(&serde_json::json!({
                    "memory_id": id,
                    "owner_agent_id": owner_agent_id,
                    "mode": req.mode,
                    "grantee_agent_ids": grantees,
                }))?,
                "pending",
                now.to_rfc3339(),
            ],
        )?;
        tx.commit()?;
        self.visibility_for_memory(owner_agent_id, id)
    }

    pub fn upsert_visibility_snapshot(&self, visibility: &MemoryVisibility) -> MemoryResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM memory_visibility_grants WHERE memory_id = ?1",
            params![visibility.memory_id.0],
        )?;
        let now = Utc::now();
        let grantees = if visibility.mode == MemoryVisibilityMode::Private {
            Vec::new()
        } else if visibility.mode == MemoryVisibilityMode::AllAgents {
            vec!["*".to_string()]
        } else {
            visibility
                .grantee_agent_ids
                .iter()
                .map(|id| id.trim())
                .filter(|id| !id.is_empty() && *id != visibility.owner_agent_id)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        };
        for grantee in grantees {
            let grant = visibility
                .grants
                .iter()
                .find(|grant| grant.grantee_agent_id == grantee);
            tx.execute(
                "INSERT INTO memory_visibility_grants (
                    memory_id, owner_agent_id, grantee_agent_id,
                    permission, created_by, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    visibility.memory_id.0,
                    visibility.owner_agent_id,
                    grantee,
                    grant
                        .map(|grant| grant.permission.as_str())
                        .unwrap_or("read"),
                    grant
                        .map(|grant| grant.created_by.as_str())
                        .unwrap_or("sync"),
                    grant
                        .map(|grant| grant.created_at)
                        .unwrap_or(now)
                        .to_rfc3339(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn search_visible_to_agent(
        &self,
        requester_agent_id: &str,
        query: &str,
        limit: usize,
    ) -> MemoryResult<Vec<(MemoryRecord, f64)>> {
        let words = query_words(query);
        if words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let fts_query = fts_query(&words);
        let mut stmt = self.conn.prepare(
            "SELECT r.id, rank
             FROM memory_records_fts f
             JOIN memory_records r ON r.id = f.id
             WHERE memory_records_fts MATCH ?1
               AND r.status = ?2
               AND (
                 r.agent_id = ?3 OR EXISTS (
                   SELECT 1 FROM memory_visibility_grants grant
                   WHERE grant.memory_id = r.id
                     AND grant.permission = 'read'
                     AND (grant.grantee_agent_id = ?3 OR grant.grantee_agent_id = '*')
                 )
               )
             ORDER BY rank, r.updated_at DESC, r.created_at DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                fts_query,
                enum_name(&MemoryRecordStatus::Active)?,
                requester_agent_id,
                (limit.saturating_mul(3).max(limit)) as i64
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
        )?;

        let mut scores = std::collections::HashMap::<String, f64>::new();
        for row in rows {
            let (id, rank) = row?;
            scores
                .entry(id)
                .and_modify(|score| *score = score.max(-rank))
                .or_insert(-rank);
        }

        let mut fuzzy_stmt = self.conn.prepare(
            "SELECT f.id, rank
             FROM memory_search_docs_fts f
             JOIN memory_records r ON r.id = f.id
             WHERE memory_search_docs_fts MATCH ?1
               AND r.status = ?2
               AND (
                 r.agent_id = ?3 OR EXISTS (
                   SELECT 1 FROM memory_visibility_grants grant
                   WHERE grant.memory_id = r.id
                     AND grant.permission = 'read'
                     AND (grant.grantee_agent_id = ?3 OR grant.grantee_agent_id = '*')
                 )
               )
             ORDER BY rank, r.updated_at DESC, r.created_at DESC
             LIMIT ?4",
        )?;
        let fuzzy_rows = fuzzy_stmt.query_map(
            params![
                fts_query,
                enum_name(&MemoryRecordStatus::Active)?,
                requester_agent_id,
                (limit.saturating_mul(3).max(limit)) as i64
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
        )?;
        for row in fuzzy_rows {
            let (id, rank) = row?;
            let fuzzy_score = -rank * 0.75;
            scores
                .entry(id)
                .and_modify(|score| *score = score.max(fuzzy_score))
                .or_insert(fuzzy_score);
        }

        let seed_scores = scores.clone();
        self.expand_relation_scores(&mut scores, &seed_scores, 0.55)?;
        self.expand_resource_scores(&mut scores, &seed_scores, 0.45)?;

        let mut hits = Vec::new();
        for (id, route_score) in scores {
            let id = MemoryId::new(id);
            if !self.memory_is_visible_to_agent(&id, requester_agent_id)? {
                continue;
            }
            if let Some(record) = self.get_one(&id, false)? {
                let score = route_score
                    + f64::from(record.confidence)
                    + record.importance.map(f64::from).unwrap_or_default();
                hits.push((record, score));
            }
        }

        hits.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        Ok(hits)
    }

    pub fn get_visible_to_agent(
        &self,
        requester_agent_id: &str,
        req: MemoryGetRequest,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        let mut out = Vec::new();
        for id in req.ids {
            if !self.memory_is_visible_to_agent(&id, requester_agent_id)? {
                continue;
            }
            if let Some(record) = self.get_one(&id, req.include_inactive)? {
                out.push(record);
            }
        }
        Ok(out)
    }

    pub fn visible_records_for_group(
        &self,
        requester_agent_id: &str,
        scope: &str,
        kind: &str,
        title: &str,
        limit: usize,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT id FROM memory_records record
             WHERE record.status = ?1
               AND record.scope = ?2
               AND record.kind = ?3
               AND record.title = ?4
               AND (
                 record.agent_id = ?5 OR EXISTS (
                   SELECT 1 FROM memory_visibility_grants grant
                   WHERE grant.memory_id = record.id
                     AND grant.permission = 'read'
                     AND (grant.grantee_agent_id = ?5 OR grant.grantee_agent_id = '*')
                 )
               )
             ORDER BY record.updated_at DESC, record.created_at DESC
             LIMIT ?6",
        )?;
        let active = enum_name(&MemoryRecordStatus::Active)?;
        let ids = stmt
            .query_map(
                params![active, scope, kind, title, requester_agent_id, limit as i64],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;

        let mut records = Vec::new();
        for id in ids {
            if let Some(record) = self.get_one(&MemoryId::new(id), false)? {
                records.push(record);
            }
        }
        Ok(records)
    }

    pub fn upsert_snapshot(&mut self, record: &MemoryRecord) -> MemoryResult<()> {
        let metadata_json = serde_json::to_string(&record.metadata)?;
        let title = normalize_memory_title(&record.title).map_err(MemoryError::Invalid)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO memory_records (
                id, agent_id, scope, kind, title, content, status, confidence,
                importance, created_at, updated_at, last_used_at, expires_at,
                superseded_by, written_on_device_id, written_on_device_name_snapshot,
                written_on_host_profile, resource_device_id, resource_device_name_snapshot,
                metadata_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
             ON CONFLICT(id) DO UPDATE SET
                agent_id = excluded.agent_id,
                scope = excluded.scope,
                kind = excluded.kind,
                title = excluded.title,
                content = excluded.content,
                status = excluded.status,
                confidence = excluded.confidence,
                importance = excluded.importance,
                updated_at = excluded.updated_at,
                last_used_at = excluded.last_used_at,
                expires_at = excluded.expires_at,
                superseded_by = excluded.superseded_by,
                written_on_device_id = excluded.written_on_device_id,
                written_on_device_name_snapshot = excluded.written_on_device_name_snapshot,
                written_on_host_profile = excluded.written_on_host_profile,
                resource_device_id = excluded.resource_device_id,
                resource_device_name_snapshot = excluded.resource_device_name_snapshot,
                metadata_json = excluded.metadata_json",
            params![
                record.id.0,
                record.agent_id,
                enum_name(&record.scope)?,
                enum_name(&record.kind)?,
                title,
                record.content,
                enum_name(&record.status)?,
                record.confidence,
                record.importance,
                record.created_at.to_rfc3339(),
                record.updated_at.map(|dt| dt.to_rfc3339()),
                record.last_used_at.map(|dt| dt.to_rfc3339()),
                record.expires_at.map(|dt| dt.to_rfc3339()),
                record.superseded_by.as_ref().map(|id| id.to_string()),
                record.written_on_device_id,
                record.written_on_device_name_snapshot,
                record.written_on_host_profile,
                record.resource_device_id,
                record.resource_device_name_snapshot,
                metadata_json,
            ],
        )?;
        tx.execute(
            "DELETE FROM memory_sources WHERE memory_id = ?1",
            params![record.id.0],
        )?;
        for source in &record.sources {
            tx.execute(
                "INSERT INTO memory_sources (
                    memory_id, chat_id, turn_id, message_id, quote, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    record.id.0,
                    source.chat_id,
                    source.turn_id,
                    source.message_id,
                    source.quote,
                    Utc::now().to_rfc3339(),
                ],
            )?;
        }
        tx.execute(
            "DELETE FROM memory_terms WHERE memory_id = ?1",
            params![record.id.0],
        )?;
        insert_terms_tx(&tx, &record.id, &record.terms)?;
        tx.execute(
            "DELETE FROM memory_resources WHERE memory_id = ?1",
            params![record.id.0],
        )?;
        insert_resources_tx(&tx, &record.id, &record.resources)?;
        tx.execute(
            "DELETE FROM memory_search_docs WHERE memory_id = ?1",
            params![record.id.0],
        )?;
        tx.execute(
            "DELETE FROM memory_links WHERE from_id = ?1 OR to_id = ?1",
            params![record.id.0],
        )?;
        for link in &record.links {
            tx.execute(
                "INSERT OR IGNORE INTO memory_links (from_id, to_id, relation)
                 VALUES (?1, ?2, ?3)",
                params![link.from_id.0, link.to_id.0, link.relation],
            )?;
        }
        tx.execute(
            "DELETE FROM memory_relations WHERE from_id = ?1 OR to_id = ?1",
            params![record.id.0],
        )?;
        insert_relations_tx(&tx, &record.relations)?;
        tx.execute(
            "DELETE FROM memory_records_fts WHERE id = ?1",
            params![record.id.0],
        )?;
        tx.execute(
            "DELETE FROM memory_search_docs_fts WHERE id = ?1",
            params![record.id.0],
        )?;
        if record.status == MemoryRecordStatus::Active {
            let search_doc = build_search_doc(
                &record.id,
                MemorySearchText {
                    title: &title,
                    content: &record.content,
                },
                &record.metadata,
                &record.terms,
                &record.resources,
                Utc::now(),
            );
            insert_search_doc_tx(&tx, &search_doc)?;
            tx.execute(
                "INSERT INTO memory_records_fts (id, content) VALUES (?1, ?2)",
                params![record.id.0, search_doc.lexical_text],
            )?;
        }
        tx.execute(
            "INSERT INTO memory_ops (
                op_id, actor, op_kind, payload_json, sync_status, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                format!("op_{}", uuid::Uuid::new_v4().simple()),
                "sync",
                "snapshot_apply",
                serde_json::to_string(record)?,
                "applied",
                Utc::now().to_rfc3339(),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn search(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> MemoryResult<Vec<(MemoryRecord, f64)>> {
        let words = query_words(query);
        if words.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let fts_query = fts_query(&words);
        let mut stmt = self.conn.prepare(
            "SELECT r.id, rank
             FROM memory_records_fts f
             JOIN memory_records r ON r.id = f.id
             WHERE memory_records_fts MATCH ?1
               AND r.agent_id = ?2
               AND r.status = ?3
             ORDER BY rank, r.updated_at DESC, r.created_at DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                fts_query,
                agent_id,
                enum_name(&MemoryRecordStatus::Active)?,
                (limit.saturating_mul(3).max(limit)) as i64
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
        )?;

        let mut scores = std::collections::HashMap::<String, f64>::new();
        for row in rows {
            let (id, rank) = row?;
            scores
                .entry(id)
                .and_modify(|score| *score = score.max(-rank))
                .or_insert(-rank);
        }

        let mut fuzzy_stmt = self.conn.prepare(
            "SELECT f.id, rank
             FROM memory_search_docs_fts f
             JOIN memory_records r ON r.id = f.id
             WHERE memory_search_docs_fts MATCH ?1
               AND r.agent_id = ?2
               AND r.status = ?3
             ORDER BY rank, r.updated_at DESC, r.created_at DESC
             LIMIT ?4",
        )?;
        let fuzzy_rows = fuzzy_stmt.query_map(
            params![
                fts_query,
                agent_id,
                enum_name(&MemoryRecordStatus::Active)?,
                (limit.saturating_mul(3).max(limit)) as i64
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
        )?;
        for row in fuzzy_rows {
            let (id, rank) = row?;
            let fuzzy_score = -rank * 0.75;
            scores
                .entry(id)
                .and_modify(|score| *score = score.max(fuzzy_score))
                .or_insert(fuzzy_score);
        }

        let seed_scores = scores.clone();
        self.expand_relation_scores(&mut scores, &seed_scores, 0.55)?;
        self.expand_resource_scores(&mut scores, &seed_scores, 0.45)?;
        self.expand_superseded_query_scores(&mut scores, agent_id, &fts_query, limit)?;

        let mut hits = Vec::new();
        for (id, route_score) in scores {
            let id = MemoryId::new(id);
            if let Some(record) = self.get_one(&id, false)? {
                let score = route_score
                    + f64::from(record.confidence)
                    + record.importance.map(f64::from).unwrap_or_default();
                hits.push((record, score));
            }
        }

        hits.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        Ok(hits)
    }

    pub fn rebuild_derived_indexes(&self) -> MemoryResult<usize> {
        self.conn.execute("DELETE FROM memory_records_fts", [])?;
        self.conn
            .execute("DELETE FROM memory_search_docs_fts", [])?;
        self.conn.execute("DELETE FROM memory_search_docs", [])?;
        let mut stmt = self.conn.prepare(
            "SELECT id, title, content, metadata_json
             FROM memory_records
             WHERE status = ?1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![enum_name(&MemoryRecordStatus::Active)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut count = 0usize;
        for row in rows {
            let (id, title, content, metadata_json) = row?;
            let metadata = parse_metadata_json(&metadata_json);
            let record_id = MemoryId::new(id.clone());
            let terms = self.terms_for(&record_id)?;
            let resources = self.resources_for(&record_id)?;
            let search_doc = build_search_doc(
                &record_id,
                MemorySearchText {
                    title: &title,
                    content: &content,
                },
                &metadata,
                &terms,
                &resources,
                Utc::now(),
            );
            insert_search_doc_conn(&self.conn, &search_doc)?;
            self.conn.execute(
                "INSERT INTO memory_records_fts (id, content) VALUES (?1, ?2)",
                params![id, search_doc.lexical_text],
            )?;
            count += 1;
        }
        Ok(count)
    }

    pub fn maintenance_candidates(
        &self,
        req: MemoryMaintenanceCandidateRequest,
    ) -> MemoryResult<MemoryMaintenancePacket> {
        let mut stmt = self.conn.prepare(
            "SELECT scope, kind, title
             FROM memory_records
             WHERE agent_id = ?1 AND status = ?2
             GROUP BY scope, kind, title
             HAVING COUNT(*) > 1
             ORDER BY COUNT(*) DESC
             LIMIT ?3",
        )?;
        let groups = stmt.query_map(
            params![
                req.agent_id,
                enum_name(&MemoryRecordStatus::Active)?,
                req.limit as i64
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;

        let mut clusters = Vec::new();
        for group in groups {
            let (scope, kind, title) = group?;
            let records = self.active_records_for_group(&req.agent_id, &scope, &kind, &title)?;
            if records.len() > 1 {
                clusters.push(MemoryMaintenanceCluster {
                    reason: "same_title".to_string(),
                    records,
                    external_context: Vec::new(),
                });
            }
        }

        Ok(MemoryMaintenancePacket {
            base_revision: self.revision()?,
            clusters,
        })
    }

    pub fn batch_apply(
        &mut self,
        req: MemoryBatchApplyRequest,
    ) -> MemoryResult<MemoryBatchApplyOutcome> {
        let current_revision = self.revision()?;
        if req.base_revision != current_revision {
            return Ok(MemoryBatchApplyOutcome {
                applied: false,
                reason: Some(format!(
                    "base revision mismatch: expected {current_revision}, got {}",
                    req.base_revision
                )),
                records: vec![],
            });
        }
        self.validate_batch_apply_ownership(&req)?;

        let now = Utc::now();
        let mut created_ids = Vec::new();
        let tx = self.conn.transaction()?;
        for op in &req.ops {
            match op {
                MemoryBatchOperation::Create { record, supersedes } => {
                    let id = record.id.clone().unwrap_or_else(|| {
                        MemoryId::new(format!("mem_{}", uuid::Uuid::new_v4().simple()))
                    });
                    insert_record_tx(&tx, &req.agent_id, &id, record, now)?;
                    for old_id in supersedes {
                        supersede_record_tx(&tx, old_id, &id, now)?;
                    }
                    created_ids.push(id);
                }
                MemoryBatchOperation::Supersede { ids, by } => {
                    let Some(target) = by.first() else {
                        return Err(MemoryError::Invalid(
                            "supersede operation requires at least one target".to_string(),
                        ));
                    };
                    for id in ids {
                        supersede_record_tx(&tx, id, target, now)?;
                    }
                }
                MemoryBatchOperation::Relate { relations } => {
                    for relation in relations {
                        insert_batch_relation_tx(&tx, relation, now)?;
                    }
                }
                MemoryBatchOperation::SetStatus { ids, status } => {
                    for id in ids {
                        set_record_status_tx(&tx, id, *status, now)?;
                    }
                }
            }
        }
        tx.execute(
            "INSERT INTO memory_ops (
                op_id, actor, op_kind, payload_json, sync_status, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                format!("op_{}", uuid::Uuid::new_v4().simple()),
                "autodream",
                "batch_apply",
                serde_json::to_string(&req)?,
                "pending",
                now.to_rfc3339(),
            ],
        )?;
        tx.commit()?;

        let mut records = Vec::new();
        for id in created_ids {
            if let Some(record) = self.get_one(&id, true)? {
                records.push(record);
            }
        }
        Ok(MemoryBatchApplyOutcome {
            applied: true,
            reason: None,
            records,
        })
    }

    fn validate_batch_apply_ownership(&self, req: &MemoryBatchApplyRequest) -> MemoryResult<()> {
        for op in &req.ops {
            match op {
                MemoryBatchOperation::Create { record, supersedes } => {
                    if let Some(id) = record.id.as_ref() {
                        self.ensure_existing_record_owner(id, &req.agent_id)?;
                    }
                    for id in supersedes {
                        self.ensure_record_owner(id, &req.agent_id)?;
                    }
                }
                MemoryBatchOperation::Supersede { ids, by } => {
                    for id in ids.iter().chain(by.iter()) {
                        self.ensure_record_owner(id, &req.agent_id)?;
                    }
                }
                MemoryBatchOperation::Relate { relations } => {
                    for relation in relations {
                        self.ensure_record_owner(&relation.from_id, &req.agent_id)?;
                        self.ensure_record_owner(&relation.to_id, &req.agent_id)?;
                    }
                }
                MemoryBatchOperation::SetStatus { ids, .. } => {
                    for id in ids {
                        self.ensure_record_owner(id, &req.agent_id)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn ensure_record_owner(&self, id: &MemoryId, agent_id: &str) -> MemoryResult<()> {
        match self.record_owner(id)? {
            Some(owner) if owner == agent_id => Ok(()),
            Some(owner) => Err(MemoryError::Invalid(format!(
                "memory {id} is owned by {owner}, not {agent_id}"
            ))),
            None => Err(MemoryError::Invalid(format!(
                "memory {id} does not exist in owner store"
            ))),
        }
    }

    fn ensure_existing_record_owner(&self, id: &MemoryId, agent_id: &str) -> MemoryResult<()> {
        if let Some(owner) = self.record_owner(id)? {
            if owner != agent_id {
                return Err(MemoryError::Invalid(format!(
                    "memory {id} is owned by {owner}, not {agent_id}"
                )));
            }
        }
        Ok(())
    }

    fn record_owner(&self, id: &MemoryId) -> MemoryResult<Option<String>> {
        self.conn
            .query_row(
                "SELECT agent_id FROM memory_records WHERE id = ?1",
                params![id.0],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn set_status(&self, id: &MemoryId, status: MemoryRecordStatus) -> MemoryResult<bool> {
        let changed = self.conn.execute(
            "UPDATE memory_records
             SET status = ?2, updated_at = ?3
             WHERE id = ?1",
            params![id.0, enum_name(&status)?, Utc::now().to_rfc3339()],
        )?;
        if changed == 0 {
            return Ok(false);
        }

        if status == MemoryRecordStatus::Active {
            if let Some(record) = self.get_one(id, true)? {
                self.conn.execute(
                    "DELETE FROM memory_records_fts WHERE id = ?1",
                    params![id.0],
                )?;
                self.conn.execute(
                    "DELETE FROM memory_search_docs_fts WHERE id = ?1",
                    params![id.0],
                )?;
                self.conn.execute(
                    "DELETE FROM memory_search_docs WHERE memory_id = ?1",
                    params![id.0],
                )?;
                let search_doc = build_search_doc(
                    &record.id,
                    MemorySearchText {
                        title: &record.title,
                        content: &record.content,
                    },
                    &record.metadata,
                    &record.terms,
                    &record.resources,
                    Utc::now(),
                );
                insert_search_doc_conn(&self.conn, &search_doc)?;
                self.conn.execute(
                    "INSERT INTO memory_records_fts (id, content) VALUES (?1, ?2)",
                    params![record.id.0, search_doc.lexical_text],
                )?;
            }
        } else if matches!(
            status,
            MemoryRecordStatus::Expired | MemoryRecordStatus::Deleted
        ) {
            self.conn.execute(
                "DELETE FROM memory_records_fts WHERE id = ?1",
                params![id.0],
            )?;
            self.conn.execute(
                "DELETE FROM memory_search_docs_fts WHERE id = ?1",
                params![id.0],
            )?;
            self.conn.execute(
                "DELETE FROM memory_search_docs WHERE memory_id = ?1",
                params![id.0],
            )?;
            if status == MemoryRecordStatus::Deleted {
                self.conn.execute(
                    "DELETE FROM memory_visibility_grants WHERE memory_id = ?1",
                    params![id.0],
                )?;
            }
        }
        Ok(true)
    }

    pub fn count(&self) -> MemoryResult<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memory_records WHERE status != ?1",
            params![enum_name(&MemoryRecordStatus::Deleted)?],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    pub fn get_search_doc(&self, id: &MemoryId) -> MemoryResult<Option<MemorySearchDoc>> {
        let row = self
            .conn
            .query_row(
                "SELECT memory_id, lexical_text, fuzzy_text, semantic_text,
                        resource_text, updated_at, content_hash
                 FROM memory_search_docs
                 WHERE memory_id = ?1",
                params![id.0],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?;
        row.map(
            |(
                memory_id,
                lexical_text,
                fuzzy_text,
                semantic_text,
                resource_text,
                updated_at,
                content_hash,
            )| {
                Ok(MemorySearchDoc {
                    memory_id: MemoryId::new(memory_id),
                    lexical_text,
                    fuzzy_text,
                    semantic_text,
                    resource_text,
                    updated_at: parse_datetime(&updated_at)?,
                    content_hash,
                })
            },
        )
        .transpose()
    }

    pub fn upsert_slot(&self, slot: &MemorySlot) -> MemoryResult<()> {
        self.conn.execute(
            "INSERT INTO memory_slots (
                id, agent_id, slot_key, scope, size_limit, enabled, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                agent_id = excluded.agent_id,
                slot_key = excluded.slot_key,
                scope = excluded.scope,
                size_limit = excluded.size_limit,
                enabled = excluded.enabled,
                updated_at = excluded.updated_at",
            params![
                slot.id,
                slot.agent_id,
                slot.slot_key,
                slot.scope,
                slot.size_limit as i64,
                if slot.enabled { 1 } else { 0 },
                slot.created_at.to_rfc3339(),
                slot.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn delete_slot(&self, slot_id: &str) -> MemoryResult<bool> {
        self.conn.execute(
            "DELETE FROM memory_slot_items WHERE slot_id = ?1",
            params![slot_id],
        )?;
        let deleted = self
            .conn
            .execute("DELETE FROM memory_slots WHERE id = ?1", params![slot_id])?;
        Ok(deleted > 0)
    }

    pub fn set_slot_items(&self, slot_id: &str, items: &[MemorySlotItem]) -> MemoryResult<()> {
        let mut stmt = self
            .conn
            .prepare("DELETE FROM memory_slot_items WHERE slot_id = ?1")?;
        stmt.execute(params![slot_id])?;
        for item in items {
            self.conn.execute(
                "INSERT INTO memory_slot_items (
                    slot_id, memory_id, render_order, render_mode
                ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    item.slot_id,
                    item.memory_id.0,
                    item.render_order,
                    item.render_mode,
                ],
            )?;
        }
        Ok(())
    }

    pub fn list_slot_snapshots_for_agent(
        &self,
        agent_id: &str,
    ) -> MemoryResult<Vec<MemorySlotSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, agent_id, slot_key, scope, size_limit, enabled, created_at, updated_at
             FROM memory_slots
             WHERE agent_id = ?1
             ORDER BY slot_key ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![agent_id], |row| {
            Ok(MemorySlot {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                slot_key: row.get(2)?,
                scope: row.get(3)?,
                size_limit: row.get::<_, i64>(4)? as usize,
                enabled: row.get::<_, i64>(5)? != 0,
                created_at: parse_datetime(&row.get::<_, String>(6)?).map_err(to_sql_error)?,
                updated_at: parse_datetime(&row.get::<_, String>(7)?).map_err(to_sql_error)?,
            })
        })?;

        let mut snapshots = Vec::new();
        for row in rows {
            let slot = row?;
            let items = self.slot_items_for(&slot.id)?;
            snapshots.push(MemorySlotSnapshot { slot, items });
        }
        Ok(snapshots)
    }

    fn slot_items_for(&self, slot_id: &str) -> MemoryResult<Vec<MemorySlotItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT slot_id, memory_id, render_order, render_mode
             FROM memory_slot_items
             WHERE slot_id = ?1
             ORDER BY render_order ASC, memory_id ASC",
        )?;
        let rows = stmt.query_map(params![slot_id], |row| {
            Ok(MemorySlotItem {
                slot_id: row.get(0)?,
                memory_id: MemoryId::new(row.get::<_, String>(1)?),
                render_order: row.get(2)?,
                render_mode: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn upsert_relation(&self, relation: &MemoryRelation) -> MemoryResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        insert_relations_tx(&tx, std::slice::from_ref(relation))?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_relation(&self, relation_id: &str) -> MemoryResult<bool> {
        let deleted = self.conn.execute(
            "DELETE FROM memory_relations WHERE id = ?1",
            params![relation_id],
        )?;
        Ok(deleted > 0)
    }

    pub fn list_relations_for_agent(&self, agent_id: &str) -> MemoryResult<Vec<MemoryRelation>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT rel.id, rel.from_id, rel.to_id, rel.relation,
                    rel.confidence, rel.created_by, rel.created_at
             FROM memory_relations rel
             JOIN memory_records owner
               ON owner.id = rel.from_id OR owner.id = rel.to_id
             WHERE owner.agent_id = ?1
             ORDER BY rel.created_at ASC, rel.id ASC",
        )?;
        let rows = stmt.query_map(params![agent_id], |row| {
            Ok(MemoryRelation {
                id: row.get(0)?,
                from_id: MemoryId::new(row.get::<_, String>(1)?),
                to_id: MemoryId::new(row.get::<_, String>(2)?),
                relation: row.get(3)?,
                confidence: row.get::<_, Option<f64>>(4)?.map(|value| value as f32),
                created_by: row.get(5)?,
                created_at: parse_datetime(&row.get::<_, String>(6)?).map_err(to_sql_error)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn pinned_context_for_agent(&self, agent_id: &str) -> MemoryResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT item.memory_id, item.render_mode
             FROM memory_slots slot
             JOIN memory_slot_items item ON item.slot_id = slot.id
             JOIN memory_records record ON record.id = item.memory_id
             WHERE slot.agent_id = ?1
               AND slot.enabled = 1
               AND record.status = ?2
             ORDER BY slot.slot_key ASC, item.render_order ASC, item.memory_id ASC",
        )?;
        let rows = stmt.query_map(
            params![agent_id, enum_name(&MemoryRecordStatus::Active)?],
            |row| {
                Ok((
                    MemoryId::new(row.get::<_, String>(0)?),
                    row.get::<_, String>(1)?,
                ))
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (memory_id, render_mode) = row?;
            if let Some(record) = self.get_one(&memory_id, false)? {
                out.push(render_pinned_record(&record, &render_mode));
            }
        }
        Ok(out)
    }

    fn revision(&self) -> MemoryResult<String> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memory_ops", [], |row| row.get(0))?;
        Ok(format!("rev:{count}"))
    }

    fn active_records_for_group(
        &self,
        agent_id: &str,
        scope: &str,
        kind: &str,
        title: &str,
    ) -> MemoryResult<Vec<MemoryRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM memory_records
             WHERE agent_id = ?1 AND status = ?2 AND scope = ?3 AND kind = ?4 AND title = ?5
             ORDER BY updated_at DESC, created_at DESC",
        )?;
        let active = enum_name(&MemoryRecordStatus::Active)?;
        let ids = stmt
            .query_map(params![agent_id, active, scope, kind, title], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut records = Vec::new();
        for id in ids {
            if let Some(record) = self.get_one(&MemoryId::new(id), true)? {
                records.push(record);
            }
        }
        Ok(records)
    }

    fn visibility_grants_for(&self, id: &MemoryId) -> MemoryResult<Vec<MemoryVisibilityGrant>> {
        let mut stmt = self.conn.prepare(
            "SELECT memory_id, owner_agent_id, grantee_agent_id,
                    permission, created_by, created_at
             FROM memory_visibility_grants
             WHERE memory_id = ?1
             ORDER BY grantee_agent_id ASC",
        )?;
        let rows = stmt.query_map(params![id.0], |row| {
            Ok(MemoryVisibilityGrant {
                memory_id: MemoryId::new(row.get::<_, String>(0)?),
                owner_agent_id: row.get(1)?,
                grantee_agent_id: row.get(2)?,
                permission: row.get(3)?,
                created_by: row.get(4)?,
                created_at: parse_datetime(&row.get::<_, String>(5)?).map_err(to_sql_error)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn memory_is_visible_to_agent(
        &self,
        id: &MemoryId,
        requester_agent_id: &str,
    ) -> MemoryResult<bool> {
        let Some(record) = self.get_one(id, true)? else {
            return Ok(false);
        };
        if record.agent_id == requester_agent_id {
            return Ok(true);
        }
        let visible: i64 = self.conn.query_row(
            "SELECT COUNT(*)
             FROM memory_visibility_grants
             WHERE memory_id = ?1
               AND owner_agent_id = ?2
               AND permission = 'read'
               AND (grantee_agent_id = ?3 OR grantee_agent_id = '*')",
            params![id.0, record.agent_id, requester_agent_id],
            |row| row.get(0),
        )?;
        Ok(visible > 0)
    }

    fn get_one(&self, id: &MemoryId, include_inactive: bool) -> MemoryResult<Option<MemoryRecord>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, agent_id, scope, kind, title, content, status,
                        confidence, importance, created_at, updated_at, last_used_at,
                        expires_at, superseded_by, written_on_device_id,
                        written_on_device_name_snapshot, written_on_host_profile,
                        resource_device_id, resource_device_name_snapshot, metadata_json
                 FROM memory_records
                 WHERE id = ?1",
                params![id.0],
                |row| {
                    Ok(RecordRow {
                        id: row.get(0)?,
                        agent_id: row.get(1)?,
                        scope: row.get(2)?,
                        kind: row.get(3)?,
                        title: row.get(4)?,
                        content: row.get(5)?,
                        status: row.get(6)?,
                        confidence: row.get(7)?,
                        importance: row.get(8)?,
                        created_at: row.get(9)?,
                        updated_at: row.get(10)?,
                        last_used_at: row.get(11)?,
                        expires_at: row.get(12)?,
                        superseded_by: row.get(13)?,
                        written_on_device_id: row.get(14)?,
                        written_on_device_name_snapshot: row.get(15)?,
                        written_on_host_profile: row.get(16)?,
                        resource_device_id: row.get(17)?,
                        resource_device_name_snapshot: row.get(18)?,
                        metadata_json: row.get(19)?,
                    })
                },
            )
            .optional()?;

        let Some(row) = row else {
            return Ok(None);
        };
        let status: MemoryRecordStatus = parse_enum(&row.status)?;
        if !include_inactive && status != MemoryRecordStatus::Active {
            return Ok(None);
        }

        let record_id = MemoryId::new(row.id);
        Ok(Some(MemoryRecord {
            id: record_id.clone(),
            agent_id: row.agent_id,
            scope: parse_enum(&row.scope)?,
            kind: parse_enum(&row.kind)?,
            title: row.title,
            content: row.content,
            status,
            confidence: row.confidence as f32,
            importance: row.importance.map(|v| v as f32),
            created_at: parse_datetime(&row.created_at)?,
            updated_at: parse_optional_datetime(row.updated_at)?,
            last_used_at: parse_optional_datetime(row.last_used_at)?,
            expires_at: parse_optional_datetime(row.expires_at)?,
            superseded_by: row.superseded_by.map(MemoryId::new),
            written_on_device_id: row.written_on_device_id,
            written_on_device_name_snapshot: row.written_on_device_name_snapshot,
            written_on_host_profile: row.written_on_host_profile,
            resource_device_id: row.resource_device_id,
            resource_device_name_snapshot: row.resource_device_name_snapshot,
            metadata: serde_json::from_str(&row.metadata_json)?,
            sources: self.sources_for(&record_id)?,
            links: self.links_for(&record_id)?,
            terms: self.terms_for(&record_id)?,
            resources: self.resources_for(&record_id)?,
            relations: self.relations_for(&record_id)?,
        }))
    }

    fn sources_for(&self, id: &MemoryId) -> MemoryResult<Vec<MemorySourceEvidence>> {
        let mut stmt = self.conn.prepare(
            "SELECT chat_id, turn_id, message_id, quote
             FROM memory_sources
             WHERE memory_id = ?1
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![id.0], |row| {
            Ok(MemorySourceEvidence {
                chat_id: row.get(0)?,
                turn_id: row.get(1)?,
                message_id: row.get(2)?,
                quote: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn links_for(&self, id: &MemoryId) -> MemoryResult<Vec<MemoryLink>> {
        let mut stmt = self.conn.prepare(
            "SELECT from_id, to_id, relation
             FROM memory_links
             WHERE from_id = ?1 OR to_id = ?1
             ORDER BY from_id ASC, to_id ASC, relation ASC",
        )?;
        let rows = stmt.query_map(params![id.0], |row| {
            Ok(MemoryLink {
                from_id: MemoryId::new(row.get::<_, String>(0)?),
                to_id: MemoryId::new(row.get::<_, String>(1)?),
                relation: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn terms_for(&self, id: &MemoryId) -> MemoryResult<Vec<MemoryTerm>> {
        let mut stmt = self.conn.prepare(
            "SELECT term, normalized_term, term_kind, source, language, created_at
             FROM memory_terms
             WHERE memory_id = ?1
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![id.0], |row| {
            Ok(MemoryTerm {
                term: row.get(0)?,
                normalized_term: row.get(1)?,
                term_kind: parse_enum(&row.get::<_, String>(2)?).map_err(to_sql_error)?,
                source: parse_enum(&row.get::<_, String>(3)?).map_err(to_sql_error)?,
                language: row.get(4)?,
                created_at: parse_datetime(&row.get::<_, String>(5)?).map_err(to_sql_error)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn resources_for(&self, id: &MemoryId) -> MemoryResult<Vec<MemoryResource>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, resource_kind, locator, normalized_locator,
                    resource_device_id, resource_device_name_snapshot,
                    host_profile, portable, created_by, metadata_json, created_at
             FROM memory_resources
             WHERE memory_id = ?1
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![id.0], |row| {
            let metadata_json: String = row.get(9)?;
            Ok(MemoryResource {
                id: row.get(0)?,
                resource_kind: row.get(1)?,
                locator: row.get(2)?,
                normalized_locator: row.get(3)?,
                resource_device_id: row.get(4)?,
                resource_device_name_snapshot: row.get(5)?,
                host_profile: row.get(6)?,
                portable: row.get::<_, i64>(7)? != 0,
                created_by: row.get(8)?,
                metadata: parse_metadata_json(&metadata_json),
                created_at: parse_datetime(&row.get::<_, String>(10)?).map_err(to_sql_error)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn relations_for(&self, id: &MemoryId) -> MemoryResult<Vec<MemoryRelation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, from_id, to_id, relation, confidence, created_by, created_at
             FROM memory_relations
             WHERE from_id = ?1 OR to_id = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![id.0], |row| {
            Ok(MemoryRelation {
                id: row.get(0)?,
                from_id: MemoryId::new(row.get::<_, String>(1)?),
                to_id: MemoryId::new(row.get::<_, String>(2)?),
                relation: row.get(3)?,
                confidence: row.get::<_, Option<f64>>(4)?.map(|value| value as f32),
                created_by: row.get(5)?,
                created_at: parse_datetime(&row.get::<_, String>(6)?).map_err(to_sql_error)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    fn expand_relation_scores(
        &self,
        scores: &mut std::collections::HashMap<String, f64>,
        seed_scores: &std::collections::HashMap<String, f64>,
        weight: f64,
    ) -> MemoryResult<()> {
        if seed_scores.is_empty() {
            return Ok(());
        }
        let mut stmt = self.conn.prepare(
            "SELECT rel.from_id, rel.to_id, rel.relation, target.status
             FROM memory_relations rel
             JOIN memory_records target
               ON target.id = CASE
                 WHEN rel.from_id = ?1 THEN rel.to_id
                 ELSE rel.from_id
               END
             WHERE (rel.from_id = ?1 OR rel.to_id = ?1)
               AND target.status = ?2",
        )?;
        let active = enum_name(&MemoryRecordStatus::Active)?;
        for (seed_id, seed_score) in seed_scores {
            let rows = stmt.query_map(params![seed_id, active], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (from_id, to_id, relation) = row?;
                if !relation_expands_search(&relation) {
                    continue;
                }
                let candidate_id = if from_id == *seed_id { to_id } else { from_id };
                if candidate_id == *seed_id {
                    continue;
                }
                let relation_weight = relation_expansion_weight(&relation) * weight;
                let candidate_score = seed_score * relation_weight;
                scores
                    .entry(candidate_id)
                    .and_modify(|score| *score = score.max(candidate_score))
                    .or_insert(candidate_score);
            }
        }
        Ok(())
    }

    fn expand_resource_scores(
        &self,
        scores: &mut std::collections::HashMap<String, f64>,
        seed_scores: &std::collections::HashMap<String, f64>,
        weight: f64,
    ) -> MemoryResult<()> {
        if seed_scores.is_empty() {
            return Ok(());
        }
        let mut seed_resource_stmt = self.conn.prepare(
            "SELECT normalized_locator
             FROM memory_resources
             WHERE memory_id = ?1
               AND normalized_locator != ''",
        )?;
        let mut candidate_stmt = self.conn.prepare(
            "SELECT res.memory_id
             FROM memory_resources res
             JOIN memory_records record ON record.id = res.memory_id
             WHERE res.normalized_locator = ?1
               AND res.memory_id != ?2
               AND record.status = ?3",
        )?;
        let active = enum_name(&MemoryRecordStatus::Active)?;
        for (seed_id, seed_score) in seed_scores {
            let locators = seed_resource_stmt
                .query_map(params![seed_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for locator in locators {
                let rows = candidate_stmt.query_map(params![locator, seed_id, active], |row| {
                    row.get::<_, String>(0)
                })?;
                for row in rows {
                    let candidate_id = row?;
                    let candidate_score = seed_score * weight;
                    scores
                        .entry(candidate_id)
                        .and_modify(|score| *score = score.max(candidate_score))
                        .or_insert(candidate_score);
                }
            }
        }
        Ok(())
    }

    fn expand_superseded_query_scores(
        &self,
        scores: &mut std::collections::HashMap<String, f64>,
        agent_id: &str,
        fts_query: &str,
        limit: usize,
    ) -> MemoryResult<()> {
        let inactive_hits = self.inactive_query_hits(agent_id, fts_query, limit)?;
        for (old_id, route_score) in inactive_hits {
            for replacement_id in self.active_replacements_for(&old_id)? {
                let candidate_score = route_score * 0.85;
                scores
                    .entry(replacement_id)
                    .and_modify(|score| *score = score.max(candidate_score))
                    .or_insert(candidate_score);
            }
        }
        Ok(())
    }

    fn inactive_query_hits(
        &self,
        agent_id: &str,
        fts_query: &str,
        limit: usize,
    ) -> MemoryResult<Vec<(String, f64)>> {
        let mut hits = Vec::new();
        let inactive = enum_name(&MemoryRecordStatus::Active)?;
        let query_limit = (limit.saturating_mul(2).max(limit)) as i64;

        let mut stmt = self.conn.prepare(
            "SELECT r.id, rank
             FROM memory_records_fts f
             JOIN memory_records r ON r.id = f.id
             WHERE memory_records_fts MATCH ?1
               AND r.agent_id = ?2
               AND r.status != ?3
             ORDER BY rank, r.updated_at DESC, r.created_at DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(params![fts_query, agent_id, inactive, query_limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })?;
        for row in rows {
            let (id, rank) = row?;
            hits.push((id, -rank));
        }

        let mut fuzzy_stmt = self.conn.prepare(
            "SELECT r.id, rank
             FROM memory_search_docs_fts f
             JOIN memory_records r ON r.id = f.id
             WHERE memory_search_docs_fts MATCH ?1
               AND r.agent_id = ?2
               AND r.status != ?3
             ORDER BY rank, r.updated_at DESC, r.created_at DESC
             LIMIT ?4",
        )?;
        let fuzzy_rows = fuzzy_stmt
            .query_map(params![fts_query, agent_id, inactive, query_limit], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
            })?;
        for row in fuzzy_rows {
            let (id, rank) = row?;
            hits.push((id, -rank * 0.75));
        }
        Ok(hits)
    }

    fn active_replacements_for(&self, old_id: &str) -> MemoryResult<Vec<String>> {
        let active = enum_name(&MemoryRecordStatus::Active)?;
        let mut out = Vec::new();

        let direct = self
            .conn
            .query_row(
                "SELECT new.id
                 FROM memory_records old
                 JOIN memory_records new ON new.id = old.superseded_by
                 WHERE old.id = ?1 AND new.status = ?2",
                params![old_id, active],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(id) = direct {
            out.push(id);
        }

        let mut stmt = self.conn.prepare(
            "SELECT rel.from_id
             FROM memory_relations rel
             JOIN memory_records new ON new.id = rel.from_id
             WHERE rel.to_id = ?1
               AND rel.relation = 'supersedes'
               AND new.status = ?2",
        )?;
        let rows = stmt.query_map(params![old_id, active], |row| row.get::<_, String>(0))?;
        for row in rows {
            out.push(row?);
        }
        Ok(dedupe_terms(out))
    }
}

struct RecordRow {
    id: String,
    agent_id: String,
    scope: String,
    kind: String,
    title: String,
    content: String,
    status: String,
    confidence: f64,
    importance: Option<f64>,
    created_at: String,
    updated_at: Option<String>,
    last_used_at: Option<String>,
    expires_at: Option<String>,
    superseded_by: Option<String>,
    written_on_device_id: Option<String>,
    written_on_device_name_snapshot: Option<String>,
    written_on_host_profile: Option<String>,
    resource_device_id: Option<String>,
    resource_device_name_snapshot: Option<String>,
    metadata_json: String,
}

fn init_schema(store: &StructuredMemoryStore) -> MemoryResult<()> {
    // Serialize schema upgrades with other writers and keep canonical backfill
    // and derived-index refresh in the same rollback boundary.
    let tx = Transaction::new_unchecked(&store.conn, TransactionBehavior::Immediate)?;
    let conn = &tx;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS memory_records (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            scope TEXT NOT NULL,
            kind TEXT NOT NULL,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            status TEXT NOT NULL,
            confidence REAL NOT NULL,
            importance REAL,
            created_at TEXT NOT NULL,
            updated_at TEXT,
            last_used_at TEXT,
            expires_at TEXT,
            superseded_by TEXT,
            written_on_device_id TEXT,
            written_on_device_name_snapshot TEXT,
            written_on_host_profile TEXT,
            resource_device_id TEXT,
            resource_device_name_snapshot TEXT,
            metadata_json TEXT NOT NULL DEFAULT '{}'
        );

        CREATE TABLE IF NOT EXISTS memory_sources (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id TEXT NOT NULL,
            chat_id TEXT,
            turn_id TEXT,
            message_id TEXT,
            quote TEXT,
            created_at TEXT NOT NULL,
            FOREIGN KEY(memory_id) REFERENCES memory_records(id)
        );

        CREATE TABLE IF NOT EXISTS memory_terms (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            memory_id TEXT NOT NULL,
            term TEXT NOT NULL,
            normalized_term TEXT NOT NULL,
            term_kind TEXT NOT NULL,
            source TEXT NOT NULL,
            language TEXT,
            created_at TEXT NOT NULL,
            UNIQUE(memory_id, normalized_term, term_kind),
            FOREIGN KEY(memory_id) REFERENCES memory_records(id)
        );

        CREATE TABLE IF NOT EXISTS memory_resources (
            id TEXT PRIMARY KEY,
            memory_id TEXT NOT NULL,
            resource_kind TEXT NOT NULL,
            locator TEXT NOT NULL,
            normalized_locator TEXT NOT NULL,
            resource_device_id TEXT,
            resource_device_name_snapshot TEXT,
            host_profile TEXT,
            portable INTEGER NOT NULL DEFAULT 0,
            created_by TEXT NOT NULL,
            metadata_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            FOREIGN KEY(memory_id) REFERENCES memory_records(id)
        );

        CREATE TABLE IF NOT EXISTS memory_search_docs (
            memory_id TEXT PRIMARY KEY,
            lexical_text TEXT NOT NULL,
            fuzzy_text TEXT NOT NULL,
            semantic_text TEXT NOT NULL,
            resource_text TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            FOREIGN KEY(memory_id) REFERENCES memory_records(id)
        );

        CREATE TABLE IF NOT EXISTS memory_links (
            from_id TEXT NOT NULL,
            to_id TEXT NOT NULL,
            relation TEXT NOT NULL,
            PRIMARY KEY (from_id, to_id, relation)
        );

        CREATE TABLE IF NOT EXISTS memory_relations (
            id TEXT PRIMARY KEY,
            from_id TEXT NOT NULL,
            to_id TEXT NOT NULL,
            relation TEXT NOT NULL,
            confidence REAL,
            created_by TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE(from_id, to_id, relation)
        );

        CREATE TABLE IF NOT EXISTS memory_slots (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            slot_key TEXT NOT NULL,
            scope TEXT NOT NULL,
            size_limit INTEGER NOT NULL,
            enabled INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS memory_slot_items (
            slot_id TEXT NOT NULL,
            memory_id TEXT NOT NULL,
            render_order INTEGER NOT NULL,
            render_mode TEXT NOT NULL,
            PRIMARY KEY (slot_id, memory_id),
            FOREIGN KEY(slot_id) REFERENCES memory_slots(id),
            FOREIGN KEY(memory_id) REFERENCES memory_records(id)
        );

        CREATE TABLE IF NOT EXISTS memory_visibility_grants (
            memory_id TEXT NOT NULL,
            owner_agent_id TEXT NOT NULL,
            grantee_agent_id TEXT NOT NULL,
            permission TEXT NOT NULL,
            created_by TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (memory_id, grantee_agent_id),
            FOREIGN KEY(memory_id) REFERENCES memory_records(id)
        );

        CREATE TABLE IF NOT EXISTS memory_ops (
            op_id TEXT PRIMARY KEY,
            actor TEXT NOT NULL,
            op_kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            base_revision TEXT,
            hlc TEXT,
            sync_status TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS memory_records_fts
        USING fts5(id UNINDEXED, content);

        CREATE VIRTUAL TABLE IF NOT EXISTS memory_search_docs_fts
        USING fts5(id UNINDEXED, content, tokenize = 'unicode61');
        "#,
    )?;
    let title_added = ensure_column(conn, "memory_records", "title", "TEXT NOT NULL DEFAULT ''")?;
    if title_added {
        let has_subject = table_columns(conn, "memory_records")?
            .iter()
            .any(|column| column == "subject");
        let subject = if has_subject { "subject" } else { "NULL" };
        let rows = {
            let mut statement = conn.prepare(&format!(
                "SELECT id, {subject}, content FROM memory_records"
            ))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        for (id, subject, content) in rows {
            let subject = subject.unwrap_or_default();
            let source = if subject.trim().is_empty() {
                &content
            } else {
                &subject
            };
            let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
            let title = normalized
                .chars()
                .take(crate::types::MEMORY_TITLE_MAX_CHARS)
                .collect::<String>();
            let title = if title.is_empty() { "Memory" } else { &title };
            conn.execute(
                "UPDATE memory_records SET title = ?1 WHERE id = ?2",
                params![title, id],
            )?;
        }
    }
    ensure_column(conn, "memory_records", "written_on_device_id", "TEXT")?;
    ensure_column(
        conn,
        "memory_records",
        "written_on_device_name_snapshot",
        "TEXT",
    )?;
    ensure_column(conn, "memory_records", "written_on_host_profile", "TEXT")?;
    ensure_column(conn, "memory_records", "resource_device_id", "TEXT")?;
    ensure_column(
        conn,
        "memory_records",
        "resource_device_name_snapshot",
        "TEXT",
    )?;
    if title_added {
        // The old projection indexed nullable `subject`; it is disposable.
        // Preserve the canonical subject column and every unknown record field.
        conn.execute_batch("DROP TABLE memory_records_fts; CREATE VIRTUAL TABLE memory_records_fts USING fts5(id UNINDEXED, content);")?;
        store.rebuild_derived_indexes()?;
    }
    tx.commit()?;
    Ok(())
}

fn table_columns(conn: &Connection, table: &str) -> MemoryResult<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns)
}

fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    column_type: &str,
) -> MemoryResult<bool> {
    let columns = table_columns(conn, table)?;
    if !columns.iter().any(|name| name == column) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {column_type}"),
            [],
        )?;
        return Ok(true);
    }
    Ok(false)
}

fn insert_record_tx(
    tx: &rusqlite::Transaction<'_>,
    agent_id: &str,
    id: &MemoryId,
    record: &MemoryBatchCreateRecord,
    now: DateTime<Utc>,
) -> MemoryResult<()> {
    let content = record.content.trim();
    if content.is_empty() {
        return Err(MemoryError::Invalid(
            "batch create memory content is empty".to_string(),
        ));
    }
    let title = normalize_memory_title(&record.title).map_err(MemoryError::Invalid)?;
    let device_context = memory_device_context_from_metadata(&record.metadata);
    tx.execute(
        "INSERT INTO memory_records (
            id, agent_id, scope, kind, title, content, status, confidence,
            importance, created_at, updated_at, written_on_device_id,
            written_on_device_name_snapshot, written_on_host_profile, resource_device_id,
            resource_device_name_snapshot, metadata_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            id.0,
            agent_id,
            enum_name(&record.scope)?,
            enum_name(&record.kind)?,
            title,
            content,
            enum_name(&MemoryRecordStatus::Active)?,
            record.confidence.unwrap_or(1.0),
            record.importance,
            now.to_rfc3339(),
            now.to_rfc3339(),
            device_context.written_on_device_id,
            device_context.written_on_device_name_snapshot,
            device_context.written_on_host_profile,
            device_context.resource_device_id,
            device_context.resource_device_name_snapshot,
            serde_json::to_string(&record.metadata)?,
        ],
    )?;
    for source in &record.sources {
        tx.execute(
            "INSERT INTO memory_sources (
                memory_id, chat_id, turn_id, message_id, quote, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id.0,
                source.chat_id,
                source.turn_id,
                source.message_id,
                source.quote,
                now.to_rfc3339(),
            ],
        )?;
    }
    let terms =
        write_terms_from_inputs(&record.labels, &record.search_terms, &record.metadata, now);
    insert_terms_tx(tx, id, &terms)?;
    let resources = detect_memory_resources(
        content,
        &memory_device_context_from_metadata(&record.metadata),
        now,
    );
    insert_resources_tx(tx, id, &resources)?;
    let search_doc = build_search_doc(
        id,
        MemorySearchText {
            title: &title,
            content,
        },
        &record.metadata,
        &terms,
        &resources,
        now,
    );
    insert_search_doc_tx(tx, &search_doc)?;
    tx.execute(
        "INSERT INTO memory_records_fts (id, content) VALUES (?1, ?2)",
        params![id.0, search_doc.lexical_text],
    )?;
    Ok(())
}

fn insert_terms_tx(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &MemoryId,
    terms: &[MemoryTerm],
) -> MemoryResult<()> {
    for term in terms {
        tx.execute(
            "INSERT OR IGNORE INTO memory_terms (
                memory_id, term, normalized_term, term_kind, source, language, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                memory_id.0,
                term.term,
                term.normalized_term,
                enum_name(&term.term_kind)?,
                enum_name(&term.source)?,
                term.language,
                term.created_at.to_rfc3339(),
            ],
        )?;
    }
    Ok(())
}

fn insert_resources_tx(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &MemoryId,
    resources: &[MemoryResource],
) -> MemoryResult<()> {
    for resource in resources {
        tx.execute(
            "INSERT OR REPLACE INTO memory_resources (
                id, memory_id, resource_kind, locator, normalized_locator,
                resource_device_id, resource_device_name_snapshot, host_profile,
                portable, created_by, metadata_json, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                resource.id,
                memory_id.0,
                resource.resource_kind,
                resource.locator,
                resource.normalized_locator,
                resource.resource_device_id,
                resource.resource_device_name_snapshot,
                resource.host_profile,
                if resource.portable { 1 } else { 0 },
                resource.created_by,
                serde_json::to_string(&resource.metadata)?,
                resource.created_at.to_rfc3339(),
            ],
        )?;
    }
    Ok(())
}

fn insert_relations_tx(
    tx: &rusqlite::Transaction<'_>,
    relations: &[MemoryRelation],
) -> MemoryResult<()> {
    for relation in relations {
        tx.execute(
            "INSERT OR REPLACE INTO memory_relations (
                id, from_id, to_id, relation, confidence, created_by, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                relation.id,
                relation.from_id.0,
                relation.to_id.0,
                relation.relation,
                relation.confidence,
                relation.created_by,
                relation.created_at.to_rfc3339(),
            ],
        )?;
    }
    Ok(())
}

fn insert_search_doc_tx(tx: &rusqlite::Transaction<'_>, doc: &MemorySearchDoc) -> MemoryResult<()> {
    tx.execute(
        "INSERT OR REPLACE INTO memory_search_docs (
            memory_id, lexical_text, fuzzy_text, semantic_text,
            resource_text, updated_at, content_hash
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            doc.memory_id.0,
            doc.lexical_text,
            doc.fuzzy_text,
            doc.semantic_text,
            doc.resource_text,
            doc.updated_at.to_rfc3339(),
            doc.content_hash,
        ],
    )?;
    tx.execute(
        "DELETE FROM memory_search_docs_fts WHERE id = ?1",
        params![doc.memory_id.0],
    )?;
    tx.execute(
        "INSERT INTO memory_search_docs_fts (id, content) VALUES (?1, ?2)",
        params![doc.memory_id.0, doc.fuzzy_text],
    )?;
    Ok(())
}

fn insert_search_doc_conn(conn: &Connection, doc: &MemorySearchDoc) -> MemoryResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO memory_search_docs (
            memory_id, lexical_text, fuzzy_text, semantic_text,
            resource_text, updated_at, content_hash
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            doc.memory_id.0,
            doc.lexical_text,
            doc.fuzzy_text,
            doc.semantic_text,
            doc.resource_text,
            doc.updated_at.to_rfc3339(),
            doc.content_hash,
        ],
    )?;
    conn.execute(
        "DELETE FROM memory_search_docs_fts WHERE id = ?1",
        params![doc.memory_id.0],
    )?;
    conn.execute(
        "INSERT INTO memory_search_docs_fts (id, content) VALUES (?1, ?2)",
        params![doc.memory_id.0, doc.fuzzy_text],
    )?;
    Ok(())
}

fn supersede_record_tx(
    tx: &rusqlite::Transaction<'_>,
    old_id: &MemoryId,
    new_id: &MemoryId,
    now: DateTime<Utc>,
) -> MemoryResult<()> {
    set_record_status_tx(tx, old_id, MemoryRecordStatus::Superseded, now)?;
    tx.execute(
        "UPDATE memory_records
         SET superseded_by = ?2, updated_at = ?3
         WHERE id = ?1",
        params![old_id.0, new_id.0, now.to_rfc3339()],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO memory_links (from_id, to_id, relation)
         VALUES (?1, ?2, ?3)",
        params![old_id.0, new_id.0, "supersedes"],
    )?;
    insert_batch_relation_tx(
        tx,
        &MemoryBatchRelation {
            from_id: new_id.clone(),
            to_id: old_id.clone(),
            relation: "supersedes".to_string(),
            confidence: Some(1.0),
        },
        now,
    )?;
    Ok(())
}

fn set_record_status_tx(
    tx: &rusqlite::Transaction<'_>,
    id: &MemoryId,
    status: MemoryRecordStatus,
    now: DateTime<Utc>,
) -> MemoryResult<()> {
    tx.execute(
        "UPDATE memory_records
         SET status = ?2, updated_at = ?3
         WHERE id = ?1",
        params![id.0, enum_name(&status)?, now.to_rfc3339()],
    )?;
    if matches!(
        status,
        MemoryRecordStatus::Expired | MemoryRecordStatus::Deleted
    ) {
        tx.execute(
            "DELETE FROM memory_records_fts WHERE id = ?1",
            params![id.0],
        )?;
        tx.execute(
            "DELETE FROM memory_search_docs_fts WHERE id = ?1",
            params![id.0],
        )?;
        tx.execute(
            "DELETE FROM memory_search_docs WHERE memory_id = ?1",
            params![id.0],
        )?;
        if status == MemoryRecordStatus::Deleted {
            tx.execute(
                "DELETE FROM memory_visibility_grants WHERE memory_id = ?1",
                params![id.0],
            )?;
        }
    }
    Ok(())
}

fn insert_batch_relation_tx(
    tx: &rusqlite::Transaction<'_>,
    relation: &MemoryBatchRelation,
    now: DateTime<Utc>,
) -> MemoryResult<()> {
    tx.execute(
        "INSERT OR IGNORE INTO memory_relations (
            id, from_id, to_id, relation, confidence, created_by, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            format!("rel_{}", uuid::Uuid::new_v4().simple()),
            relation.from_id.0,
            relation.to_id.0,
            relation.relation,
            relation.confidence,
            "autodream",
            now.to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn enum_name(value: &impl Serialize) -> MemoryResult<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| MemoryError::Invalid("enum did not serialize as string".to_string()))
}

fn parse_enum<T: DeserializeOwned>(raw: &str) -> MemoryResult<T> {
    Ok(serde_json::from_value(serde_json::Value::String(
        raw.to_string(),
    ))?)
}

fn parse_datetime(raw: &str) -> MemoryResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| MemoryError::Invalid(format!("invalid memory timestamp {raw}: {e}")))
}

fn parse_optional_datetime(raw: Option<String>) -> MemoryResult<Option<DateTime<Utc>>> {
    raw.as_deref().map(parse_datetime).transpose()
}

fn normalized_visibility_grantees(
    owner_agent_id: &str,
    req: &MemoryVisibilityUpdateRequest,
) -> Vec<String> {
    if req.mode == MemoryVisibilityMode::Private {
        return Vec::new();
    }
    if req.mode == MemoryVisibilityMode::AllAgents {
        return vec!["*".to_string()];
    }

    let mut grantees = req
        .grantee_agent_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty() && *id != owner_agent_id && *id != "*")
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    grantees.sort();
    grantees.dedup();
    grantees
}

fn visibility_from_grants(
    memory_id: MemoryId,
    owner_agent_id: String,
    grants: Vec<MemoryVisibilityGrant>,
) -> MemoryVisibility {
    let mut grantee_agent_ids = grants
        .iter()
        .map(|grant| grant.grantee_agent_id.clone())
        .collect::<Vec<_>>();
    grantee_agent_ids.sort();
    grantee_agent_ids.dedup();
    let mode = if grantee_agent_ids.iter().any(|id| id == "*") {
        MemoryVisibilityMode::AllAgents
    } else if grantee_agent_ids.is_empty() {
        MemoryVisibilityMode::Private
    } else {
        MemoryVisibilityMode::Selected
    };

    MemoryVisibility {
        memory_id,
        owner_agent_id,
        mode,
        grantee_agent_ids,
        grants,
    }
}

fn relation_expands_search(relation: &str) -> bool {
    matches!(
        relation,
        "same_title" | "related" | "supports" | "contradicts" | "resource_alias"
    )
}

fn relation_expansion_weight(relation: &str) -> f64 {
    match relation {
        "same_title" => 1.0,
        "supports" => 0.9,
        "resource_alias" => 0.85,
        "related" => 0.75,
        "contradicts" => 0.65,
        _ => 0.0,
    }
}

fn render_pinned_record(record: &MemoryRecord, render_mode: &str) -> String {
    let mut line = record.content.clone();
    if matches!(render_mode, "with_device" | "with_source") {
        let device_name = record
            .resource_device_name_snapshot
            .as_deref()
            .or(record.written_on_device_name_snapshot.as_deref());
        let device_id = record
            .resource_device_id
            .as_deref()
            .or(record.written_on_device_id.as_deref());
        if device_name.is_some() || device_id.is_some() {
            line.push_str(&format!(
                " [device: {} ({})]",
                device_name.unwrap_or("unknown"),
                device_id.unwrap_or("unknown")
            ));
        }
    }
    if render_mode == "with_source" {
        if let Some(source) = record.sources.first() {
            if source.chat_id.is_some() || source.turn_id.is_some() {
                line.push_str(&format!(
                    " [source: chat={} turn={}]",
                    source.chat_id.as_deref().unwrap_or(""),
                    source.turn_id.as_deref().unwrap_or("")
                ));
            }
        }
    }
    line
}

fn to_sql_error(err: MemoryError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(err))
}

#[derive(Debug, Clone, Default)]
struct MemoryDeviceContext {
    written_on_device_id: Option<String>,
    written_on_device_name_snapshot: Option<String>,
    written_on_host_profile: Option<String>,
    resource_device_id: Option<String>,
    resource_device_name_snapshot: Option<String>,
}

fn memory_device_context_from_metadata(metadata: &serde_json::Value) -> MemoryDeviceContext {
    let written_on_device_id = metadata_string(metadata, "written_on_device_id");
    let written_on_device_name_snapshot =
        metadata_string(metadata, "written_on_device_name_snapshot");
    let resource_device_id =
        metadata_string(metadata, "resource_device_id").or_else(|| written_on_device_id.clone());
    let resource_device_name_snapshot = metadata_string(metadata, "resource_device_name_snapshot")
        .or_else(|| written_on_device_name_snapshot.clone());

    MemoryDeviceContext {
        written_on_device_id,
        written_on_device_name_snapshot,
        written_on_host_profile: metadata_string(metadata, "written_on_host_profile"),
        resource_device_id,
        resource_device_name_snapshot,
    }
}

fn detect_memory_resources(
    content: &str,
    device_context: &MemoryDeviceContext,
    created_at: DateTime<Utc>,
) -> Vec<MemoryResource> {
    let mut resources = extract_path_like_locators(content)
        .into_iter()
        .map(|locator| {
            let normalized_locator = normalize_locator(&locator);
            MemoryResource {
                id: format!("res_{}", uuid::Uuid::new_v4().simple()),
                resource_kind: path_resource_kind(content).to_string(),
                locator,
                normalized_locator,
                resource_device_id: device_context.resource_device_id.clone(),
                resource_device_name_snapshot: device_context.resource_device_name_snapshot.clone(),
                host_profile: device_context.written_on_host_profile.clone(),
                portable: false,
                created_by: "memory_manager".to_string(),
                metadata: serde_json::Value::Object(Default::default()),
                created_at,
            }
        })
        .collect::<Vec<_>>();
    resources.extend(detect_named_local_resources(
        content,
        device_context,
        created_at,
    ));
    let mut seen = std::collections::HashSet::new();
    resources
        .into_iter()
        .filter(|resource| {
            seen.insert((
                resource.resource_kind.clone(),
                resource.normalized_locator.clone(),
            ))
        })
        .collect()
}

fn path_resource_kind(content: &str) -> &'static str {
    let lower = content.to_lowercase();
    if lower.contains("model") || content.contains("模型") {
        "model_file"
    } else if lower.contains("app") || content.contains("应用") {
        "app"
    } else {
        "local_path"
    }
}

fn detect_named_local_resources(
    content: &str,
    device_context: &MemoryDeviceContext,
    created_at: DateTime<Utc>,
) -> Vec<MemoryResource> {
    let mut resources = Vec::new();
    let lower = content.to_lowercase();
    for (kind, locator) in [
        (
            "photo_album",
            keyword_locator(content, &lower, &["相册", "album"]),
        ),
        (
            "model_file",
            keyword_locator(content, &lower, &["模型文件", "model file", "local model"]),
        ),
        (
            "app",
            keyword_locator(content, &lower, &["应用", " app", "app "]),
        ),
    ] {
        if let Some(locator) = locator {
            resources.push(MemoryResource {
                id: format!("res_{}", uuid::Uuid::new_v4().simple()),
                normalized_locator: normalize_locator(&locator),
                locator,
                resource_kind: kind.to_string(),
                resource_device_id: device_context.resource_device_id.clone(),
                resource_device_name_snapshot: device_context.resource_device_name_snapshot.clone(),
                host_profile: device_context.written_on_host_profile.clone(),
                portable: false,
                created_by: "memory_manager".to_string(),
                metadata: serde_json::Value::Object(Default::default()),
                created_at,
            });
        }
    }
    resources
}

fn keyword_locator(content: &str, lower_content: &str, keywords: &[&str]) -> Option<String> {
    keywords.iter().find_map(|keyword| {
        if keyword.is_ascii() {
            lower_content
                .contains(keyword.trim())
                .then(|| keyword.trim().to_string())
        } else {
            content.contains(keyword).then(|| (*keyword).to_string())
        }
    })
}

fn extract_path_like_locators(content: &str) -> Vec<String> {
    let chars = content.chars().collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if is_windows_drive_start(&chars, i) || is_unix_path_start(&chars, i) {
            let start = i;
            i += 1;
            while i < chars.len() && is_path_char(chars[i]) {
                i += 1;
            }
            let locator = trim_path_punctuation(chars[start..i].iter().collect::<String>());
            if is_useful_locator(&locator) {
                out.push(locator);
            }
            continue;
        }
        i += 1;
    }
    dedupe_terms(out)
}

fn is_windows_drive_start(chars: &[char], i: usize) -> bool {
    i + 2 < chars.len()
        && chars[i].is_ascii_alphabetic()
        && chars[i + 1] == ':'
        && matches!(chars[i + 2], '\\' | '/')
}

fn is_unix_path_start(chars: &[char], i: usize) -> bool {
    chars[i] == '/'
        && i + 1 < chars.len()
        && !matches!(chars[i + 1], '/' | ' ' | '\t' | '\n' | '\r')
}

fn is_path_char(ch: char) -> bool {
    !ch.is_whitespace()
        && !matches!(
            ch,
            '"' | '\'' | '`' | '<' | '>' | '，' | '。' | '、' | '；' | '：'
        )
}

fn trim_path_punctuation(mut locator: String) -> String {
    while locator.chars().last().is_some_and(|ch| {
        matches!(
            ch,
            '.' | ',' | ';' | ':' | ')' | ']' | '}' | '）' | '】' | '。'
        )
    }) {
        locator.pop();
    }
    locator
}

fn is_useful_locator(locator: &str) -> bool {
    if locator.len() < 3 {
        return false;
    }
    locator.contains('\\')
        || locator.matches('/').count() >= 2
        || locator.starts_with("~/")
        || locator.starts_with("/sdcard/")
}

fn normalize_locator(locator: &str) -> String {
    locator.replace('\\', "/").to_lowercase()
}

fn metadata_string(metadata: &serde_json::Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_metadata_json(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or(serde_json::Value::Object(Default::default()))
}

struct MemorySearchText<'a> {
    title: &'a str,
    content: &'a str,
}

fn build_search_doc(
    memory_id: &MemoryId,
    text: MemorySearchText<'_>,
    metadata: &serde_json::Value,
    terms: &[MemoryTerm],
    resources: &[MemoryResource],
    updated_at: DateTime<Utc>,
) -> MemorySearchDoc {
    let resource_text = memory_resource_text(resources);
    let lexical_text = memory_index_text(
        &format!("{}\n{}", text.title, text.content),
        metadata,
        terms.iter().map(|term| term.term.as_str()),
        Some(resource_text.as_str()),
    );
    let semantic_text = dedupe_terms(
        std::iter::once(text.title.to_string())
            .chain(std::iter::once(text.content.to_string()))
            .chain(terms.iter().map(|term| term.term.clone()))
            .chain((!resource_text.is_empty()).then_some(resource_text.clone()))
            .collect(),
    )
    .join("\n");
    let fuzzy_seed = format!("{lexical_text}\n{resource_text}");
    let fuzzy_text = fuzzy_terms(&fuzzy_seed).join("\n");
    let hash_payload =
        format!("{lexical_text}\n---\n{fuzzy_text}\n---\n{semantic_text}\n---\n{resource_text}");
    MemorySearchDoc {
        memory_id: memory_id.clone(),
        lexical_text,
        fuzzy_text,
        semantic_text,
        resource_text,
        updated_at,
        content_hash: blake3::hash(hash_payload.as_bytes()).to_hex().to_string(),
    }
}

fn memory_resource_text(resources: &[MemoryResource]) -> String {
    let mut terms = Vec::new();
    for resource in resources {
        push_index_term(&mut terms, &resource.resource_kind);
        push_index_term(&mut terms, &resource.locator);
        push_index_term(&mut terms, &resource.normalized_locator);
        if let Some(device_id) = resource.resource_device_id.as_deref() {
            push_index_term(&mut terms, device_id);
        }
        if let Some(device_name) = resource.resource_device_name_snapshot.as_deref() {
            push_index_term(&mut terms, device_name);
        }
        if let Some(host_profile) = resource.host_profile.as_deref() {
            push_index_term(&mut terms, host_profile);
        }
    }
    dedupe_terms(terms).join("\n")
}

fn write_terms_from_inputs(
    labels: &[String],
    search_terms: &[String],
    metadata: &serde_json::Value,
    created_at: DateTime<Utc>,
) -> Vec<MemoryTerm> {
    let mut terms = Vec::new();
    push_memory_terms(
        &mut terms,
        labels.iter().map(String::as_str),
        MemoryTermKind::Label,
        MemoryTermSource::Agent,
        created_at,
    );
    push_memory_terms(
        &mut terms,
        search_terms.iter().map(String::as_str),
        MemoryTermKind::SearchTerm,
        MemoryTermSource::Agent,
        created_at,
    );
    push_memory_terms(
        &mut terms,
        metadata_search_terms(metadata).iter().map(String::as_str),
        MemoryTermKind::SearchTerm,
        MemoryTermSource::Agent,
        created_at,
    );
    terms
}

fn push_memory_terms<'a>(
    out: &mut Vec<MemoryTerm>,
    values: impl IntoIterator<Item = &'a str>,
    term_kind: MemoryTermKind,
    source: MemoryTermSource,
    created_at: DateTime<Utc>,
) {
    let existing = out
        .iter()
        .map(|term| (term.normalized_term.clone(), term.term_kind))
        .collect::<std::collections::HashSet<_>>();
    let mut seen = existing;
    for value in values {
        let term = value.trim();
        if term.is_empty() {
            continue;
        }
        let normalized_term = normalize_term(term);
        if normalized_term.is_empty() || !seen.insert((normalized_term.clone(), term_kind)) {
            continue;
        }
        out.push(MemoryTerm {
            term: term.to_string(),
            normalized_term,
            term_kind,
            source,
            language: None,
            created_at,
        });
    }
}

fn normalize_term(term: &str) -> String {
    split_symbol_tokens(&term.to_lowercase())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn memory_index_text<'a>(
    content: &str,
    metadata: &serde_json::Value,
    record_terms: impl IntoIterator<Item = &'a str>,
    resource_text: Option<&str>,
) -> String {
    let mut terms = Vec::new();
    push_index_term(&mut terms, content);
    for term in record_terms {
        push_index_term(&mut terms, term);
        push_index_term(&mut terms, &split_symbol_tokens(term));
    }
    for term in metadata_search_terms(metadata) {
        push_index_term(&mut terms, &term);
    }
    if let Some(resource_text) = resource_text {
        push_index_term(&mut terms, resource_text);
    }

    for term in generated_alias_terms(&terms.join(" ")) {
        push_index_term(&mut terms, term);
    }

    for gram in cjk_ngrams(&terms.join(" ")) {
        push_index_term(&mut terms, &gram);
    }

    dedupe_terms(terms).join("\n")
}

fn metadata_search_terms(metadata: &serde_json::Value) -> Vec<String> {
    ["search_terms", "aliases", "keywords"]
        .iter()
        .flat_map(|key| metadata_terms_for_key(metadata, key))
        .collect()
}

fn metadata_terms_for_key(metadata: &serde_json::Value, key: &str) -> Vec<String> {
    match metadata.get(key) {
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        Some(serde_json::Value::String(value)) => value
            .split([',', ';', '\n'])
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn generated_alias_terms(index_seed: &str) -> Vec<&'static str> {
    const ALIAS_GROUPS: &[&[&str]] = &[
        &[
            "user name",
            "username",
            "user_name",
            "name",
            "名字",
            "姓名",
            "用户名",
            "用户名称",
            "称呼",
        ],
        &[
            "directory",
            "folder",
            "path",
            "drive",
            "album",
            "目录",
            "文件夹",
            "路径",
            "盘",
            "相册",
        ],
        &[
            "photo", "picture", "image", "album", "照片", "图片", "图像", "相册",
        ],
        &[
            "document",
            "docs",
            "file",
            "work document",
            "文档",
            "文件",
            "工作文档",
        ],
    ];

    let seed = index_seed.to_lowercase();
    let ascii_tokens = split_symbol_tokens(&seed)
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<std::collections::HashSet<_>>();

    ALIAS_GROUPS
        .iter()
        .filter(|group| {
            group
                .iter()
                .any(|term| alias_seed_contains(&seed, &ascii_tokens, term))
        })
        .flat_map(|group| group.iter().copied())
        .collect()
}

fn alias_seed_contains(
    seed: &str,
    ascii_tokens: &std::collections::HashSet<String>,
    term: &str,
) -> bool {
    if term.is_ascii() {
        let normalized = split_symbol_tokens(term);
        let parts = normalized.split_whitespace().collect::<Vec<_>>();
        !parts.is_empty() && parts.iter().all(|part| ascii_tokens.contains(*part))
    } else {
        seed.contains(term)
    }
}

fn split_symbol_tokens(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || is_cjk(ch) {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect()
}

fn cjk_ngrams(text: &str) -> Vec<String> {
    let mut grams = Vec::new();
    let mut run = Vec::new();
    for ch in text.chars() {
        if is_cjk(ch) {
            run.push(ch);
        } else {
            push_cjk_run_ngrams(&run, &mut grams);
            run.clear();
        }
    }
    push_cjk_run_ngrams(&run, &mut grams);
    grams
}

fn fuzzy_terms(text: &str) -> Vec<String> {
    let base = split_symbol_tokens(&text.to_lowercase())
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut terms = base.clone();
    for term in &base {
        terms.extend(english_stem_variants(term));
    }
    terms.extend(cjk_ngrams(text));
    dedupe_terms(terms)
}

fn english_stem_variants(term: &str) -> Vec<String> {
    if !term.is_ascii() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for suffix in ["ing", "ed", "es", "s"] {
        if term.len() > suffix.len() + 2 && term.ends_with(suffix) {
            let stem = term.trim_end_matches(suffix);
            if stem.len() >= 3 {
                out.push(stem.to_string());
            }
        }
    }
    out
}

fn push_cjk_run_ngrams(run: &[char], out: &mut Vec<String>) {
    if run.len() < 2 {
        return;
    }
    let max_n = run.len().min(4);
    for n in 2..=max_n {
        for window in run.windows(n) {
            out.push(window.iter().collect());
        }
    }
}

fn is_cjk(ch: char) -> bool {
    matches!(ch, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}')
}

fn push_index_term(terms: &mut Vec<String>, term: &str) {
    let trimmed = term.trim();
    if !trimmed.is_empty() {
        terms.push(trimmed.to_string());
    }
}

fn dedupe_terms(terms: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for term in terms {
        let key = term.to_lowercase();
        if seen.insert(key) {
            out.push(term);
        }
    }
    out
}

fn query_words(query: &str) -> Vec<String> {
    let mut words = split_symbol_tokens(&query.to_lowercase())
        .split_whitespace()
        .map(str::trim)
        .filter(|word| word.chars().count() >= 2)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    words.extend(
        generated_alias_terms(query)
            .into_iter()
            .map(ToOwned::to_owned),
    );
    let stems = words
        .iter()
        .flat_map(|word| english_stem_variants(word))
        .collect::<Vec<_>>();
    words.extend(stems);
    words.extend(cjk_ngrams(query));
    dedupe_terms(words)
}

fn fts_query(words: &[String]) -> String {
    words
        .iter()
        .map(|word| word.replace('"', ""))
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{word}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}
