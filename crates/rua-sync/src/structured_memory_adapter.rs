use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use rand::RngCore;

use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::LwwDecision;
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const STRUCTURED_MEMORY_DOMAIN: &str = "structured_memory";
pub const STRUCTURED_MEMORY_KIND_SNAPSHOT: &str = "snapshot";
pub const STRUCTURED_MEMORY_KIND_DELETE: &str = "delete";
pub const STRUCTURED_MEMORY_KIND_RELATION_SNAPSHOT: &str = "relation_snapshot";
pub const STRUCTURED_MEMORY_KIND_SLOT_SNAPSHOT: &str = "slot_snapshot";
pub const STRUCTURED_MEMORY_KIND_VISIBILITY_SNAPSHOT: &str = "visibility_snapshot";

pub struct StructuredMemorySyncAdapter {
    data_dir: PathBuf,
    device_id: String,
}

impl StructuredMemorySyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
        }
    }

    pub fn record_id(agent_id: &str, memory_id: &str) -> String {
        format!("memory/{agent_id}/{memory_id}")
    }

    pub fn relation_record_id(agent_id: &str, relation_id: &str) -> String {
        format!("relation/{agent_id}/{relation_id}")
    }

    pub fn slot_record_id(agent_id: &str, slot_id: &str) -> String {
        format!("slot/{agent_id}/{slot_id}")
    }

    pub fn visibility_record_id(agent_id: &str, memory_id: &str) -> String {
        format!("visibility/{agent_id}/{memory_id}")
    }

    pub fn snapshot_change(
        op_id: impl Into<String>,
        record: &rua_memory::MemoryRecord,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncResult<SyncChange> {
        Ok(SyncChange::new(
            op_id,
            STRUCTURED_MEMORY_DOMAIN,
            Self::record_id(&record.agent_id, &record.id.to_string()),
            STRUCTURED_MEMORY_KIND_SNAPSHOT,
            device_id,
            hlc,
            serde_json::to_value(record)?,
        ))
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        agent_id: &str,
        memory_id: &str,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            STRUCTURED_MEMORY_DOMAIN,
            Self::record_id(agent_id, memory_id),
            STRUCTURED_MEMORY_KIND_DELETE,
            device_id,
            hlc,
            serde_json::Value::Object(Default::default()),
        )
    }

    pub fn relation_snapshot_change(
        op_id: impl Into<String>,
        agent_id: &str,
        relation: &rua_memory::MemoryRelation,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncResult<SyncChange> {
        Ok(SyncChange::new(
            op_id,
            STRUCTURED_MEMORY_DOMAIN,
            Self::relation_record_id(agent_id, &relation.id),
            STRUCTURED_MEMORY_KIND_RELATION_SNAPSHOT,
            device_id,
            hlc,
            serde_json::to_value(relation)?,
        ))
    }

    pub fn relation_delete_change(
        op_id: impl Into<String>,
        agent_id: &str,
        relation_id: &str,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            STRUCTURED_MEMORY_DOMAIN,
            Self::relation_record_id(agent_id, relation_id),
            STRUCTURED_MEMORY_KIND_DELETE,
            device_id,
            hlc,
            serde_json::Value::Object(Default::default()),
        )
    }

    pub fn slot_snapshot_change(
        op_id: impl Into<String>,
        slot: &rua_memory::MemorySlotSnapshot,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncResult<SyncChange> {
        Ok(SyncChange::new(
            op_id,
            STRUCTURED_MEMORY_DOMAIN,
            Self::slot_record_id(&slot.slot.agent_id, &slot.slot.id),
            STRUCTURED_MEMORY_KIND_SLOT_SNAPSHOT,
            device_id,
            hlc,
            serde_json::to_value(slot)?,
        ))
    }

    pub fn slot_delete_change(
        op_id: impl Into<String>,
        agent_id: &str,
        slot_id: &str,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            STRUCTURED_MEMORY_DOMAIN,
            Self::slot_record_id(agent_id, slot_id),
            STRUCTURED_MEMORY_KIND_DELETE,
            device_id,
            hlc,
            serde_json::Value::Object(Default::default()),
        )
    }

    pub fn visibility_snapshot_change(
        op_id: impl Into<String>,
        visibility: &rua_memory::MemoryVisibility,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncResult<SyncChange> {
        Ok(SyncChange::new(
            op_id,
            STRUCTURED_MEMORY_DOMAIN,
            Self::visibility_record_id(
                &visibility.owner_agent_id,
                &visibility.memory_id.to_string(),
            ),
            STRUCTURED_MEMORY_KIND_VISIBILITY_SNAPSHOT,
            device_id,
            hlc,
            serde_json::to_value(visibility)?,
        ))
    }

    pub fn new_op_id() -> String {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    fn workspaces_dir(&self) -> PathBuf {
        self.data_dir.join("workspaces")
    }

    fn workspace_structured_memory_db(&self, agent_id: &str) -> SyncResult<PathBuf> {
        validate_path_part(agent_id)?;
        Ok(self.workspaces_dir().join(agent_id).join("memory.db"))
    }

    fn record_target<'a>(&self, record_id: &'a str) -> SyncResult<StructuredMemoryTarget<'a>> {
        let parts = record_id.split('/').collect::<Vec<_>>();
        match parts.as_slice() {
            ["memory", agent_id, memory_id] => {
                validate_path_part(agent_id)?;
                validate_path_part(memory_id)?;
                Ok(StructuredMemoryTarget::Memory {
                    agent_id,
                    memory_id,
                })
            }
            ["relation", agent_id, relation_id] => {
                validate_path_part(agent_id)?;
                validate_path_part(relation_id)?;
                Ok(StructuredMemoryTarget::Relation {
                    agent_id,
                    relation_id,
                })
            }
            ["slot", agent_id, slot_id] => {
                validate_path_part(agent_id)?;
                validate_path_part(slot_id)?;
                Ok(StructuredMemoryTarget::Slot { agent_id, slot_id })
            }
            ["visibility", agent_id, memory_id] => {
                validate_path_part(agent_id)?;
                validate_path_part(memory_id)?;
                Ok(StructuredMemoryTarget::Visibility {
                    agent_id,
                    memory_id,
                })
            }

            [agent_id, memory_id] => {
                validate_path_part(agent_id)?;
                validate_path_part(memory_id)?;
                Ok(StructuredMemoryTarget::Memory {
                    agent_id,
                    memory_id,
                })
            }
            _ => Err(SyncError::Store(format!(
                "invalid structured memory record id: {record_id}"
            ))),
        }
    }
}

enum StructuredMemoryTarget<'a> {
    Memory {
        agent_id: &'a str,
        memory_id: &'a str,
    },
    Relation {
        agent_id: &'a str,
        relation_id: &'a str,
    },
    Slot {
        agent_id: &'a str,
        slot_id: &'a str,
    },
    Visibility {
        agent_id: &'a str,
        memory_id: &'a str,
    },
}

#[async_trait]
impl DomainSyncAdapter for StructuredMemorySyncAdapter {
    fn domain(&self) -> &str {
        STRUCTURED_MEMORY_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let workspaces = self.workspaces_dir();
        if !workspaces.is_dir() {
            return Ok(Vec::new());
        }

        let mut changes = Vec::new();
        for workspace in std::fs::read_dir(workspaces)? {
            let workspace = workspace?;
            if !workspace.path().is_dir() {
                continue;
            }
            let Some(agent_id) = workspace.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if validate_path_part(&agent_id).is_err() {
                continue;
            }
            let db_path = workspace.path().join("memory.db");
            if !db_path.is_file() {
                continue;
            }
            let store = rua_memory::StructuredMemoryStore::open(&db_path)
                .map_err(|e| SyncError::Store(format!("open structured memory db: {e}")))?;
            for record in store
                .list_records_for_agent(&agent_id, false)
                .map_err(|e| SyncError::Store(format!("list structured memories: {e}")))?
            {
                changes.push(Self::snapshot_change(
                    Self::new_op_id(),
                    &record,
                    self.device_id.clone(),
                    record_hlc(&record, &self.device_id),
                )?);
            }
            for relation in store
                .list_relations_for_agent(&agent_id)
                .map_err(|e| SyncError::Store(format!("list structured memory relations: {e}")))?
            {
                changes.push(Self::relation_snapshot_change(
                    Self::new_op_id(),
                    &agent_id,
                    &relation,
                    self.device_id.clone(),
                    relation_hlc(&relation, &self.device_id),
                )?);
            }
            for slot in store
                .list_slot_snapshots_for_agent(&agent_id)
                .map_err(|e| SyncError::Store(format!("list structured memory slots: {e}")))?
            {
                changes.push(Self::slot_snapshot_change(
                    Self::new_op_id(),
                    &slot,
                    self.device_id.clone(),
                    slot_hlc(&slot, &self.device_id),
                )?);
            }
            for visibility in store
                .list_visibility_for_agent(&agent_id)
                .map_err(|e| SyncError::Store(format!("list structured memory visibility: {e}")))?
            {
                changes.push(Self::visibility_snapshot_change(
                    Self::new_op_id(),
                    &visibility,
                    self.device_id.clone(),
                    visibility_hlc(&visibility, &self.device_id),
                )?);
            }
        }
        changes.sort_by(|a, b| a.record_id.cmp(&b.record_id));
        Ok(changes)
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != STRUCTURED_MEMORY_DOMAIN {
            return Err(SyncError::Store(format!(
                "structured memory adapter received wrong domain: {}",
                change.domain
            )));
        }

        let cursor_id = sync_cursor_record_id(STRUCTURED_MEMORY_DOMAIN, &change.record_id);
        let is_delete = change.kind == STRUCTURED_MEMORY_KIND_DELETE;
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }

        let target = self.record_target(&change.record_id)?;
        let agent_id = match target {
            StructuredMemoryTarget::Memory { agent_id, .. }
            | StructuredMemoryTarget::Relation { agent_id, .. }
            | StructuredMemoryTarget::Slot { agent_id, .. }
            | StructuredMemoryTarget::Visibility { agent_id, .. } => agent_id,
        };
        let db_path = self.workspace_structured_memory_db(agent_id)?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut store = rua_memory::StructuredMemoryStore::open(&db_path)
            .map_err(|e| SyncError::Store(format!("open structured memory db: {e}")))?;

        if is_delete {
            match target {
                StructuredMemoryTarget::Memory { memory_id, .. } => {
                    let id = rua_memory::MemoryId::new(memory_id.to_string());
                    store
                        .set_status(&id, rua_memory::MemoryRecordStatus::Deleted)
                        .map_err(|e| {
                            SyncError::Store(format!("tombstone structured memory: {e}"))
                        })?;
                }
                StructuredMemoryTarget::Relation { relation_id, .. } => {
                    store.delete_relation(relation_id).map_err(|e| {
                        SyncError::Store(format!("delete structured memory relation: {e}"))
                    })?;
                }
                StructuredMemoryTarget::Slot { slot_id, .. } => {
                    store.delete_slot(slot_id).map_err(|e| {
                        SyncError::Store(format!("delete structured memory slot: {e}"))
                    })?;
                }
                StructuredMemoryTarget::Visibility { memory_id, .. } => {
                    let id = rua_memory::MemoryId::new(memory_id.to_string());
                    let visibility = rua_memory::MemoryVisibility {
                        memory_id: id,
                        owner_agent_id: agent_id.to_string(),
                        mode: rua_memory::MemoryVisibilityMode::Private,
                        grantee_agent_ids: vec![],
                        grants: vec![],
                    };
                    store.upsert_visibility_snapshot(&visibility).map_err(|e| {
                        SyncError::Store(format!("delete structured memory visibility grants: {e}"))
                    })?;
                }
            }
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }

        match target {
            StructuredMemoryTarget::Memory { memory_id, .. } => {
                if change.kind != STRUCTURED_MEMORY_KIND_SNAPSHOT {
                    return Err(SyncError::Store(format!(
                        "unsupported structured memory change kind: {}",
                        change.kind
                    )));
                }
                let record: rua_memory::MemoryRecord =
                    serde_json::from_value(change.payload.clone())?;
                if record.agent_id != agent_id || record.id.to_string() != memory_id {
                    return Err(SyncError::Store(
                        "structured memory record id mismatch".to_string(),
                    ));
                }
                store.upsert_snapshot(&record).map_err(|e| {
                    SyncError::Store(format!("upsert structured memory snapshot: {e}"))
                })?;
            }
            StructuredMemoryTarget::Relation { relation_id, .. } => {
                if change.kind != STRUCTURED_MEMORY_KIND_RELATION_SNAPSHOT {
                    return Err(SyncError::Store(format!(
                        "unsupported structured memory relation kind: {}",
                        change.kind
                    )));
                }
                let relation: rua_memory::MemoryRelation =
                    serde_json::from_value(change.payload.clone())?;
                if relation.id != relation_id {
                    return Err(SyncError::Store(
                        "structured memory relation id mismatch".to_string(),
                    ));
                }
                store.upsert_relation(&relation).map_err(|e| {
                    SyncError::Store(format!("upsert structured memory relation: {e}"))
                })?;
            }
            StructuredMemoryTarget::Slot { slot_id, .. } => {
                if change.kind != STRUCTURED_MEMORY_KIND_SLOT_SNAPSHOT {
                    return Err(SyncError::Store(format!(
                        "unsupported structured memory slot kind: {}",
                        change.kind
                    )));
                }
                let slot: rua_memory::MemorySlotSnapshot =
                    serde_json::from_value(change.payload.clone())?;
                if slot.slot.agent_id != agent_id || slot.slot.id != slot_id {
                    return Err(SyncError::Store(
                        "structured memory slot id mismatch".to_string(),
                    ));
                }
                store
                    .upsert_slot(&slot.slot)
                    .map_err(|e| SyncError::Store(format!("upsert structured memory slot: {e}")))?;
                store
                    .set_slot_items(&slot.slot.id, &slot.items)
                    .map_err(|e| {
                        SyncError::Store(format!("upsert structured memory slot items: {e}"))
                    })?;
            }
            StructuredMemoryTarget::Visibility { memory_id, .. } => {
                if change.kind != STRUCTURED_MEMORY_KIND_VISIBILITY_SNAPSHOT {
                    return Err(SyncError::Store(format!(
                        "unsupported structured memory visibility kind: {}",
                        change.kind
                    )));
                }
                let visibility: rua_memory::MemoryVisibility =
                    serde_json::from_value(change.payload.clone())?;
                if visibility.owner_agent_id != agent_id
                    || visibility.memory_id.to_string() != memory_id
                {
                    return Err(SyncError::Store(
                        "structured memory visibility id mismatch".to_string(),
                    ));
                }
                store.upsert_visibility_snapshot(&visibility).map_err(|e| {
                    SyncError::Store(format!("upsert structured memory visibility: {e}"))
                })?;
            }
        };
        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }
}

fn record_hlc(record: &rua_memory::MemoryRecord, device_id: &str) -> HlcTimestamp {
    let wall_ms = record
        .updated_at
        .unwrap_or(record.created_at)
        .timestamp_millis()
        .max(0) as u64;
    let mut device = [0u8; 8];
    device.copy_from_slice(&blake3::hash(device_id.as_bytes()).as_bytes()[..8]);
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: device,
    }
}

fn relation_hlc(relation: &rua_memory::MemoryRelation, device_id: &str) -> HlcTimestamp {
    hlc_from_wall_ms(
        relation.created_at.timestamp_millis().max(0) as u64,
        device_id,
    )
}

fn slot_hlc(slot: &rua_memory::MemorySlotSnapshot, device_id: &str) -> HlcTimestamp {
    hlc_from_wall_ms(
        slot.slot.updated_at.timestamp_millis().max(0) as u64,
        device_id,
    )
}

fn visibility_hlc(visibility: &rua_memory::MemoryVisibility, device_id: &str) -> HlcTimestamp {
    let wall_ms = visibility
        .grants
        .iter()
        .map(|grant| grant.created_at.timestamp_millis().max(0) as u64)
        .max()
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis().max(0) as u64);
    hlc_from_wall_ms(wall_ms, device_id)
}

fn hlc_from_wall_ms(wall_ms: u64, device_id: &str) -> HlcTimestamp {
    let mut device = [0u8; 8];
    device.copy_from_slice(&blake3::hash(device_id.as_bytes()).as_bytes()[..8]);
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: device,
    }
}

fn validate_path_part(part: &str) -> SyncResult<()> {
    if part.is_empty() {
        return Err(SyncError::Store(
            "path segment must not be empty".to_string(),
        ));
    }
    let path = Path::new(part);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) || part.contains('/')
        || part.contains('\\')
    {
        return Err(SyncError::Store(format!("invalid path segment: {part}")));
    }
    Ok(())
}
