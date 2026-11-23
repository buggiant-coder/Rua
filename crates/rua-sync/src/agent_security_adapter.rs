use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rand::RngCore;

use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::LwwDecision;
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const AGENT_SECURITY_DOMAIN: &str = "agent_security";
pub const AGENT_SECURITY_KIND_UPSERT: &str = "upsert";
pub const AGENT_SECURITY_KIND_DELETE: &str = "delete";
pub const AGENT_SECURITY_KIND_SNAPSHOT: &str = "snapshot";

pub struct AgentSecuritySyncAdapter {
    data_dir: PathBuf,
    device_id: String,
}

impl AgentSecuritySyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
        }
    }

    pub fn upsert_change(
        op_id: impl Into<String>,
        agent_id: impl Into<String>,
        config: serde_json::Value,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            AGENT_SECURITY_DOMAIN,
            agent_id,
            AGENT_SECURITY_KIND_UPSERT,
            device_id,
            hlc,
            strip_device_local_security(config),
        )
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        agent_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            AGENT_SECURITY_DOMAIN,
            agent_id,
            AGENT_SECURITY_KIND_DELETE,
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

    /// The persisted portable record, retaining its ordinary snapshot timestamp.
    pub fn snapshot_for_agent(&self, agent_id: &str) -> SyncResult<Option<SyncChange>> {
        let path = self.security_path(agent_id)?;
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let config = strip_device_local_security(serde_json::from_reader(&file)?);
        if !config.is_object() {
            return Err(SyncError::InvalidPayload(
                "Agent security must be an object".into(),
            ));
        }
        Ok(Some(SyncChange::new(
            Self::new_op_id(),
            AGENT_SECURITY_DOMAIN,
            agent_id,
            AGENT_SECURITY_KIND_SNAPSHOT,
            self.device_id.clone(),
            Self::export_hlc(&file, &self.device_id),
            config,
        )))
    }

    fn workspaces_dir(&self) -> PathBuf {
        self.data_dir.join("workspaces")
    }

    fn security_path(&self, agent_id: &str) -> SyncResult<PathBuf> {
        validate_agent_id(agent_id)?;
        Ok(self.workspaces_dir().join(agent_id).join("security.json"))
    }

    fn export_hlc(file: &std::fs::File, device_id: &str) -> HlcTimestamp {
        let wall_ms = file
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
}

#[async_trait]
impl DomainSyncAdapter for AgentSecuritySyncAdapter {
    fn domain(&self) -> &str {
        AGENT_SECURITY_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let workspaces = self.workspaces_dir();
        if !workspaces.is_dir() {
            return Ok(Vec::new());
        }

        let mut changes = Vec::new();
        for entry in std::fs::read_dir(&workspaces)? {
            let entry = entry?;
            let path = entry.path().join("security.json");
            if !path.is_file() {
                continue;
            }
            let Some(agent_id) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if let Some(change) = self.snapshot_for_agent(&agent_id)? {
                changes.push(change);
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
        if change.domain != AGENT_SECURITY_DOMAIN {
            return Err(SyncError::Store(format!(
                "agent security adapter received wrong domain: {}",
                change.domain
            )));
        }
        validate_agent_id(&change.record_id)?;

        let cursor_id = sync_cursor_record_id(AGENT_SECURITY_DOMAIN, &change.record_id);
        let is_delete = change.kind == AGENT_SECURITY_KIND_DELETE;
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }

        let path = self.security_path(&change.record_id)?;
        if is_delete {
            match rua_config::remove_file_committed(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }
        if change.kind != AGENT_SECURITY_KIND_UPSERT && change.kind != AGENT_SECURITY_KIND_SNAPSHOT
        {
            return Err(SyncError::Store(format!(
                "unsupported agent security change kind: {}",
                change.kind
            )));
        }

        let merged = strip_device_local_security(change.payload.clone());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        rua_config::atomic_write_json(&path, &merged)?;

        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }
}

fn strip_device_local_security(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = value.as_object_mut() {
        obj.remove("allow_no_auth_hosts");
    }
    value
}

fn validate_agent_id(agent_id: &str) -> SyncResult<()> {
    if agent_id.is_empty()
        || agent_id.contains("..")
        || agent_id.contains('/')
        || agent_id.contains('\\')
    {
        return Err(SyncError::Store(format!("invalid agent id: {agent_id}")));
    }
    Ok(())
}
