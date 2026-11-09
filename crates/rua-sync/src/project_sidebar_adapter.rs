use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::LwwDecision;
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};
use async_trait::async_trait;
use rua_project::{ProjectSidebarRecord, ProjectSidebarStore};
use std::path::PathBuf;

pub const PROJECT_SIDEBAR_DOMAIN: &str = "project_sidebar";
pub struct ProjectSidebarSyncAdapter {
    store: ProjectSidebarStore,
    device_id: String,
}
impl ProjectSidebarSyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            store: ProjectSidebarStore::new(data_dir),
            device_id: device_id.into(),
        }
    }
    pub fn record_change(
        record: ProjectSidebarRecord,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            crate::project_adapter::ProjectCatalogSyncAdapter::new_op_id(),
            PROJECT_SIDEBAR_DOMAIN,
            record.id.clone(),
            "record",
            device_id,
            hlc,
            serde_json::to_value(record).expect("sidebar record serializable"),
        )
    }
}
#[async_trait]
impl DomainSyncAdapter for ProjectSidebarSyncAdapter {
    fn domain(&self) -> &str {
        PROJECT_SIDEBAR_DOMAIN
    }
    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let mut device_id = [0u8; 8];
        device_id.copy_from_slice(&blake3::hash(self.device_id.as_bytes()).as_bytes()[..8]);
        Ok(self
            .store
            .records()
            .map_err(|error| SyncError::Store(error.to_string()))?
            .into_iter()
            .map(|record| {
                let hlc = HlcTimestamp {
                    wall_ms: record.updated_at_ms.max(0) as u64,
                    counter: 0,
                    device_id,
                };
                Self::record_change(record, self.device_id.clone(), hlc)
            })
            .collect())
    }
    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != PROJECT_SIDEBAR_DOMAIN || change.kind != "record" {
            return Err(SyncError::InvalidPayload(
                "Unsupported sidebar change".into(),
            ));
        }
        let record: ProjectSidebarRecord = serde_json::from_value(change.payload.clone())?;
        if record.id != change.record_id {
            return Err(SyncError::InvalidPayload(
                "Sidebar record id mismatch".into(),
            ));
        }
        let id = sync_cursor_record_id(PROJECT_SIDEBAR_DOMAIN, &change.record_id);
        let existing = merge_store.get_record_cursor(&id)?;
        if existing
            .as_ref()
            .is_some_and(|cursor| cursor.state == crate::sidecar::RecordState::Tombstoned)
            && !record.is_tombstone()
        {
            return Ok(ApplyOutcome::Skipped);
        }
        let deleted = record.is_tombstone()
            || existing
                .as_ref()
                .is_some_and(|cursor| cursor.state == crate::sidecar::RecordState::Tombstoned);
        if !deleted
            && (merge_store.resolve_lww(&id, change.hlc, false)? == LwwDecision::Skip
                || existing
                    .as_ref()
                    .is_some_and(|cursor| cursor.hlc >= change.hlc))
        {
            return Ok(ApplyOutcome::Skipped);
        }
        let cursor_hlc = existing
            .map(|cursor| cursor.hlc.max(change.hlc))
            .unwrap_or(change.hlc);
        self.store
            .apply_record(record)
            .map_err(|error| SyncError::Store(error.to_string()))?;
        Ok(ApplyOutcome::Applied {
            cursor: if deleted {
                tombstone_cursor(cursor_hlc, None)
            } else {
                live_cursor(change.hlc, None)
            },
        })
    }
}
