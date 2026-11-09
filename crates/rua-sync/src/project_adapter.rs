use std::sync::Arc;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use rand::RngCore;
use rua_project::{ProjectCatalog, ProjectDescriptor};

use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::{LwwDecision, RecordState};
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const PROJECT_CATALOG_DOMAIN: &str = "project_catalog";
pub const PROJECT_DESCRIPTOR_KIND: &str = "descriptor";
pub const PROJECT_DELETE_KIND: &str = "delete";

pub struct ProjectCatalogSyncAdapter {
    catalog: Arc<ProjectCatalog>,
    device_id: String,
}

impl ProjectCatalogSyncAdapter {
    pub fn new(catalog: Arc<ProjectCatalog>, device_id: impl Into<String>) -> Self {
        Self {
            catalog,
            device_id: device_id.into(),
        }
    }

    pub fn descriptor_change(
        op_id: impl Into<String>,
        descriptor: ProjectDescriptor,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            PROJECT_CATALOG_DOMAIN,
            descriptor.project_id.to_string(),
            PROJECT_DESCRIPTOR_KIND,
            device_id,
            hlc,
            serde_json::to_value(descriptor).unwrap_or_default(),
        )
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        project_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            PROJECT_CATALOG_DOMAIN,
            project_id,
            PROJECT_DELETE_KIND,
            device_id,
            hlc,
            serde_json::Value::Object(Default::default()),
        )
    }

    pub fn new_op_id() -> String {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    fn export_hlc(&self, wall_ms: i64) -> HlcTimestamp {
        let mut device = [0u8; 8];
        device.copy_from_slice(&blake3::hash(self.device_id.as_bytes()).as_bytes()[..8]);
        HlcTimestamp {
            wall_ms: wall_ms.max(0) as u64,
            counter: 0,
            device_id: device,
        }
    }
}

#[async_trait]
impl DomainSyncAdapter for ProjectCatalogSyncAdapter {
    fn domain(&self) -> &str {
        PROJECT_CATALOG_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let descriptors = self
            .catalog
            .export_descriptors()
            .map_err(|error| SyncError::Store(error.to_string()))?;
        let tombstones = self
            .catalog
            .export_tombstones()
            .map_err(|error| SyncError::Store(error.to_string()))?;
        let mut changes = descriptors
            .into_iter()
            .map(|descriptor| {
                let hlc = self.export_hlc(descriptor.updated_at.timestamp_millis());
                Self::descriptor_change(Self::new_op_id(), descriptor, self.device_id.clone(), hlc)
            })
            .chain(tombstones.into_iter().map(|tombstone| {
                Self::delete_change(
                    Self::new_op_id(),
                    tombstone.project_id.to_string(),
                    self.device_id.clone(),
                    self.export_hlc(tombstone.deleted_at.timestamp_millis()),
                )
            }))
            .collect::<Vec<_>>();
        changes.sort_by(|left, right| left.record_id.cmp(&right.record_id));
        Ok(changes)
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != PROJECT_CATALOG_DOMAIN {
            return Err(SyncError::Store(format!(
                "Project Catalog adapter received wrong domain: {}",
                change.domain
            )));
        }
        let is_delete = change.kind == PROJECT_DELETE_KIND;
        if !is_delete && change.kind != PROJECT_DESCRIPTOR_KIND {
            return Err(SyncError::Store(format!(
                "unsupported Project Catalog change kind: {}",
                change.kind
            )));
        }
        let cursor_id = sync_cursor_record_id(PROJECT_CATALOG_DOMAIN, &change.record_id);
        if merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)? == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }
        let project_id = rua_core::ProjectId::new(change.record_id.clone());
        if is_delete {
            let deleted_at = Utc
                .timestamp_millis_opt(change.hlc.wall_ms.min(i64::MAX as u64) as i64)
                .single()
                .unwrap_or_else(Utc::now);
            self.catalog
                .apply_tombstone(project_id, deleted_at)
                .map_err(|error| SyncError::Store(error.to_string()))?;
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }
        if self
            .catalog
            .is_deleted(&project_id)
            .map_err(|error| SyncError::Store(error.to_string()))?
        {
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }
        if let Some(cursor) = merge_store.get_record_cursor(&cursor_id)? {
            if cursor.state == RecordState::Live && change.hlc <= cursor.hlc {
                return Ok(ApplyOutcome::Skipped);
            }
        }

        let descriptor: ProjectDescriptor = serde_json::from_value(change.payload.clone())?;
        if descriptor.project_id != project_id {
            return Err(SyncError::Store(
                "Project Descriptor record_id does not match project_id".to_string(),
            ));
        }
        self.catalog
            .apply_descriptor(descriptor)
            .map_err(|error| SyncError::Store(error.to_string()))?;
        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }
}
