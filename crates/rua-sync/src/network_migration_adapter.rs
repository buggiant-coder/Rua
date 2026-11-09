use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::{LwwDecision, RecordState};
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const NETWORK_MIGRATION_DOMAIN: &str = "network_migration";
pub const NETWORK_MIGRATION_RECORD_PENDING: &str = "pending";
pub const NETWORK_MIGRATION_KIND_SNAPSHOT: &str = "snapshot";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkMigrationPayload {
    pub target_namespace_id: String,
    pub target_namespace_secret_bytes: Vec<u8>,
    pub migrated_at_ms: u64,
}

pub struct NetworkMigrationSyncAdapter {
    data_dir: PathBuf,
}

impl NetworkMigrationSyncAdapter {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn marker_change(
        op_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
        payload: NetworkMigrationPayload,
    ) -> SyncChange {
        let value = serde_json::json!({
            "target_namespace_id": payload.target_namespace_id,
            "target_namespace_secret_bytes": payload.target_namespace_secret_bytes,
            "migrated_at_ms": payload.migrated_at_ms,
        });
        SyncChange::new(
            op_id,
            NETWORK_MIGRATION_DOMAIN,
            NETWORK_MIGRATION_RECORD_PENDING,
            NETWORK_MIGRATION_KIND_SNAPSHOT,
            device_id,
            hlc,
            value,
        )
    }

    pub fn new_op_id() -> String {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    fn marker_path(&self) -> PathBuf {
        pending_migration_path(&self.data_dir)
    }
}

#[async_trait]
impl DomainSyncAdapter for NetworkMigrationSyncAdapter {
    fn domain(&self) -> &str {
        NETWORK_MIGRATION_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        Ok(Vec::new())
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != NETWORK_MIGRATION_DOMAIN {
            return Err(SyncError::Store(format!(
                "network migration adapter received wrong domain: {}",
                change.domain
            )));
        }
        if change.record_id != NETWORK_MIGRATION_RECORD_PENDING {
            return Err(SyncError::Store(format!(
                "unsupported network migration record: {}",
                change.record_id
            )));
        }
        if change.kind != NETWORK_MIGRATION_KIND_SNAPSHOT {
            return Err(SyncError::Store(format!(
                "unsupported network migration kind: {}",
                change.kind
            )));
        }

        let cursor_id = sync_cursor_record_id(NETWORK_MIGRATION_DOMAIN, &change.record_id);
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, false)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }
        if let Some(cursor) = merge_store.get_record_cursor(&cursor_id)? {
            if cursor.state == RecordState::Live && change.hlc <= cursor.hlc {
                return Ok(ApplyOutcome::Skipped);
            }
        }

        let payload: NetworkMigrationPayload = serde_json::from_value(change.payload.clone())?;
        if payload.target_namespace_secret_bytes.len() != 32 {
            return Err(SyncError::Store(
                "network migration namespace secret must be 32 bytes".to_string(),
            ));
        }

        write_pending_migration_file(&self.marker_path(), &payload)?;
        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }
}

pub fn pending_migration_path(data_dir: &Path) -> PathBuf {
    crate::control_store::SyncControlStore::from_data_dir(data_dir).pending_migration_path()
}

pub fn write_pending_migration_file(
    path: &Path,
    payload: &NetworkMigrationPayload,
) -> SyncResult<()> {
    let data_dir = path
        .parent()
        .and_then(|sync_dir| sync_dir.parent())
        .ok_or_else(|| crate::SyncError::Store("invalid pending migration path".to_string()))?;
    crate::control_store::SyncControlStore::from_data_dir(data_dir).write_pending_migration(payload)
}
