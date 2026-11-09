use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rand::RngCore;

use crate::device_list::{DeviceEntry, DeviceList};
use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::{LwwDecision, RecordState};
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const DEVICE_LIST_DOMAIN: &str = "device_list";
pub const DEVICE_LIST_KIND_SNAPSHOT: &str = "snapshot";
pub const DEVICE_LIST_KIND_REMOVE: &str = "remove";
pub const DEVICE_LIST_KIND_RENAME: &str = "rename";

pub struct DeviceListSyncAdapter {
    data_dir: PathBuf,
    device_id: String,
}

impl DeviceListSyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
        }
    }

    pub fn snapshot_change(
        op_id: impl Into<String>,
        entry: DeviceEntry,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        let record_id = hex::encode(entry.device_id);
        SyncChange::new(
            op_id,
            DEVICE_LIST_DOMAIN,
            record_id,
            DEVICE_LIST_KIND_SNAPSHOT,
            device_id,
            hlc,
            serde_json::to_value(entry).unwrap_or_default(),
        )
    }

    pub fn remove_change(
        op_id: impl Into<String>,
        device_id_hex: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        let device_id_hex = device_id_hex.into();
        SyncChange::new(
            op_id,
            DEVICE_LIST_DOMAIN,
            device_id_hex.clone(),
            DEVICE_LIST_KIND_REMOVE,
            device_id,
            hlc,
            serde_json::json!({ "device_id": device_id_hex }),
        )
    }

    pub fn rename_change(
        op_id: impl Into<String>,
        device_id_hex: impl Into<String>,
        friendly_name: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        let device_id_hex = device_id_hex.into();
        SyncChange::new(
            op_id,
            DEVICE_LIST_DOMAIN,
            device_id_hex.clone(),
            DEVICE_LIST_KIND_RENAME,
            device_id,
            hlc,
            serde_json::json!({
                "device_id": device_id_hex,
                "friendly_name": friendly_name.into(),
            }),
        )
    }

    pub fn new_op_id() -> String {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    fn device_list_path(&self) -> PathBuf {
        self.data_dir.join("sync").join("device_list.json")
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
}

#[async_trait]
impl DomainSyncAdapter for DeviceListSyncAdapter {
    fn domain(&self) -> &str {
        DEVICE_LIST_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let path = self.device_list_path();
        let Some(device_list) = read_device_list(&path)? else {
            return Ok(Vec::new());
        };
        let hlc = Self::export_hlc(&path, &self.device_id);
        let mut changes = device_list
            .devices
            .into_iter()
            .map(|entry| {
                Self::snapshot_change(Self::new_op_id(), entry, self.device_id.clone(), hlc)
            })
            .collect::<Vec<_>>();
        changes.sort_by(|a, b| a.record_id.cmp(&b.record_id));
        Ok(changes)
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != DEVICE_LIST_DOMAIN {
            return Err(SyncError::Store(format!(
                "device-list adapter received wrong domain: {}",
                change.domain
            )));
        }
        let device_id = parse_device_id_hex(&change.record_id)?;
        let cursor_id = sync_cursor_record_id(DEVICE_LIST_DOMAIN, &change.record_id);
        let is_remove = change.kind == DEVICE_LIST_KIND_REMOVE;
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, is_remove)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }
        if !is_remove {
            if let Some(cursor) = merge_store.get_record_cursor(&cursor_id)? {
                if cursor.state == RecordState::Live && change.hlc <= cursor.hlc {
                    return Ok(ApplyOutcome::Skipped);
                }
            }
        }

        let path = self.device_list_path();
        let mut device_list = read_device_list(&path)?.unwrap_or_default();
        match change.kind.as_str() {
            DEVICE_LIST_KIND_REMOVE => {
                device_list.remove_device(&device_id);
                write_device_list(&path, &device_list)?;
                Ok(ApplyOutcome::Applied {
                    cursor: tombstone_cursor(change.hlc, None),
                })
            }
            DEVICE_LIST_KIND_RENAME => {
                let friendly_name = change
                    .payload
                    .get("friendly_name")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        SyncError::Store("device-list rename missing friendly_name".to_string())
                    })?;
                device_list.rename_device(&device_id, friendly_name);
                write_device_list(&path, &device_list)?;
                Ok(ApplyOutcome::Applied {
                    cursor: live_cursor(change.hlc, None),
                })
            }
            DEVICE_LIST_KIND_SNAPSHOT => {
                let remote: DeviceEntry = serde_json::from_value(change.payload.clone())?;
                if remote.device_id != device_id {
                    return Err(SyncError::Store(
                        "device-list snapshot record_id does not match payload device_id"
                            .to_string(),
                    ));
                }
                if let Some(existing) = device_list
                    .devices
                    .iter_mut()
                    .find(|entry| entry.device_id == remote.device_id)
                {
                    existing.public_key = remote.public_key;
                    existing.label = remote.label;
                    existing.friendly_name = remote.friendly_name;
                    existing.added_at_ms = existing.added_at_ms.min(remote.added_at_ms);
                    existing.last_seen_ms = existing.last_seen_ms.max(remote.last_seen_ms);
                    existing.direct_addrs = remote.direct_addrs;
                } else {
                    device_list.devices.push(remote);
                }
                write_device_list(&path, &device_list)?;
                Ok(ApplyOutcome::Applied {
                    cursor: live_cursor(change.hlc, None),
                })
            }
            other => Err(SyncError::Store(format!(
                "unsupported device-list change kind: {other}"
            ))),
        }
    }
}

pub fn parse_device_id_hex(hex_str: &str) -> SyncResult<[u8; 8]> {
    let bytes = hex::decode(hex_str)
        .map_err(|e| SyncError::Store(format!("invalid device id hex: {e}")))?;
    if bytes.len() != 8 {
        return Err(SyncError::Store(format!(
            "device id must be 8 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 8];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn read_device_list(path: &Path) -> SyncResult<Option<DeviceList>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&content)?))
}

fn write_device_list(path: &Path, device_list: &DeviceList) -> SyncResult<()> {
    crate::control_store::write_device_list_file(path, device_list)
}
