use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rand::RngCore;

use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::LwwDecision;
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const SHARED_MCP_DOMAIN: &str = "shared_mcp";
pub const SHARED_MCP_RECORD_CATALOG: &str = "catalog";
pub const SHARED_MCP_KIND_UPSERT: &str = "upsert";
pub const SHARED_MCP_KIND_DELETE: &str = "delete";
pub const SHARED_MCP_KIND_SNAPSHOT: &str = "snapshot";
const SHARED_MCP_CLIENT_PREFIX: &str = "client/";

pub struct SharedMcpSyncAdapter {
    data_dir: PathBuf,
    device_id: String,
    local_mcp_key: Option<[u8; 32]>,
}

impl SharedMcpSyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
            local_mcp_key: None,
        }
    }

    pub fn with_local_mcp_key(mut self, key: [u8; 32]) -> Self {
        self.local_mcp_key = Some(key);
        self
    }

    pub fn upsert_change(
        op_id: impl Into<String>,
        catalog: serde_json::Value,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            SHARED_MCP_DOMAIN,
            SHARED_MCP_RECORD_CATALOG,
            SHARED_MCP_KIND_UPSERT,
            device_id,
            hlc,
            catalog,
        )
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            SHARED_MCP_DOMAIN,
            SHARED_MCP_RECORD_CATALOG,
            SHARED_MCP_KIND_DELETE,
            device_id,
            hlc,
            serde_json::Value::Object(Default::default()),
        )
    }

    pub fn client_delete_change(
        op_id: impl Into<String>,
        client_key: &str,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            SHARED_MCP_DOMAIN,
            format!("{SHARED_MCP_CLIENT_PREFIX}{client_key}"),
            SHARED_MCP_KIND_DELETE,
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

    fn path(&self) -> PathBuf {
        self.data_dir.join("shared_mcp.json")
    }

    fn export_hlc(path: &Path, device_id: &str) -> HlcTimestamp {
        let wall_ms = path
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let mut device = [0u8; 8];
        device.copy_from_slice(&blake3::hash(device_id.as_bytes()).as_bytes()[..8]);
        HlcTimestamp {
            wall_ms,
            counter: 0,
            device_id: device,
        }
    }

    fn normalize_for_sync(&self, value: &serde_json::Value) -> SyncResult<serde_json::Value> {
        rua_mcp::secret_field_io::decrypt_client_value(value, self.local_mcp_key.as_ref())
            .map_err(|e| SyncError::Store(format!("decrypt shared MCP for sync: {e}")))
    }

    fn encrypt_for_local(&self, value: &serde_json::Value) -> SyncResult<serde_json::Value> {
        rua_mcp::secret_field_io::encrypt_client_value(
            value,
            self.local_mcp_key.as_ref(),
            "shared:mcp",
        )
        .map_err(|e| SyncError::Store(format!("encrypt shared MCP for local disk: {e}")))
    }

    fn read_logical_or_empty(&self) -> SyncResult<serde_json::Value> {
        let path = self.path();
        let value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
            .unwrap_or_else(|| serde_json::json!({ "clients": {} }));
        self.normalize_for_sync(&value)
    }
}

#[async_trait]
impl DomainSyncAdapter for SharedMcpSyncAdapter {
    fn domain(&self) -> &str {
        SHARED_MCP_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let path = self.path();
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        let logical = self.normalize_for_sync(&raw)?;
        Ok(vec![SyncChange::new(
            Self::new_op_id(),
            SHARED_MCP_DOMAIN,
            SHARED_MCP_RECORD_CATALOG,
            SHARED_MCP_KIND_SNAPSHOT,
            self.device_id.clone(),
            Self::export_hlc(&path, &self.device_id),
            logical,
        )])
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != SHARED_MCP_DOMAIN {
            return Err(SyncError::Store(format!(
                "shared MCP adapter received wrong domain: {}",
                change.domain
            )));
        }
        if let Some(client_key) = change.record_id.strip_prefix(SHARED_MCP_CLIENT_PREFIX) {
            return self
                .apply_client_change(client_key, change, merge_store)
                .await;
        }

        if change.record_id != SHARED_MCP_RECORD_CATALOG {
            return Err(SyncError::Store(format!(
                "unsupported shared MCP record id: {}",
                change.record_id
            )));
        }

        let cursor_id = sync_cursor_record_id(SHARED_MCP_DOMAIN, SHARED_MCP_RECORD_CATALOG);
        let is_delete = change.kind == SHARED_MCP_KIND_DELETE;
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }

        let path = self.path();
        if is_delete {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }
        if change.kind != SHARED_MCP_KIND_UPSERT && change.kind != SHARED_MCP_KIND_SNAPSHOT {
            return Err(SyncError::Store(format!(
                "unsupported shared MCP change kind: {}",
                change.kind
            )));
        }

        let merged = change.payload.clone();
        let disk_value = self.encrypt_for_local(&merged)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        rua_config::atomic_write_json(&path, &disk_value)?;

        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }
}

impl SharedMcpSyncAdapter {
    async fn apply_client_change(
        &self,
        client_key: &str,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if client_key.is_empty() || client_key.contains("..") || client_key.contains('/') {
            return Err(SyncError::Store(format!(
                "invalid shared MCP client record id: {}",
                change.record_id
            )));
        }
        if change.kind != SHARED_MCP_KIND_DELETE {
            return Err(SyncError::Store(format!(
                "unsupported shared MCP client change kind: {}",
                change.kind
            )));
        }

        let cursor_id = sync_cursor_record_id(SHARED_MCP_DOMAIN, &change.record_id);
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, true)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }

        let path = self.path();
        let mut logical = self.read_logical_or_empty()?;
        if let Some(clients) = logical
            .get_mut("clients")
            .and_then(|clients| clients.as_object_mut())
        {
            clients.remove(client_key);
        }

        let disk_value = self.encrypt_for_local(&logical)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        rua_config::atomic_write_json(&path, &disk_value)?;

        Ok(ApplyOutcome::Applied {
            cursor: tombstone_cursor(change.hlc, None),
        })
    }
}
