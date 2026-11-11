use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rand::RngCore;

use crate::device_list::{is_generated_friendly_name, DeviceEntry, DeviceList, DeviceNameRecord};
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
        Self::snapshot_change_with_name_record(op_id, entry, None, device_id, hlc)
    }

    pub fn snapshot_change_with_name_record(
        op_id: impl Into<String>,
        mut entry: DeviceEntry,
        name_record: Option<DeviceNameRecord>,
        device_id: impl Into<String>,
        mut hlc: HlcTimestamp,
    ) -> SyncChange {
        if let Some(record) = &name_record {
            entry.friendly_name.clone_from(&record.friendly_name);
            hlc = hlc.max(HlcTimestamp::from_sortable_bytes(&record.revision));
        }
        let record_id = hex::encode(entry.device_id);
        let mut change = SyncChange::new(
            op_id,
            DEVICE_LIST_DOMAIN,
            record_id,
            DEVICE_LIST_KIND_SNAPSHOT,
            device_id,
            hlc,
            serde_json::to_value(entry).unwrap_or_default(),
        );
        if let Some(record) = name_record {
            change.payload["name_record"] = serde_json::to_value(record).unwrap_or_default();
        }
        change
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
        let record = DeviceNameRecord {
            friendly_name: friendly_name.into(),
            revision: hlc.to_sortable_bytes(),
            ..Default::default()
        };
        Self::rename_record_change(op_id, device_id_hex, record, device_id)
    }

    pub fn rename_record_change(
        op_id: impl Into<String>,
        device_id_hex: impl Into<String>,
        record: DeviceNameRecord,
        device_id: impl Into<String>,
    ) -> SyncChange {
        let device_id_hex = device_id_hex.into();
        SyncChange::new(
            op_id,
            DEVICE_LIST_DOMAIN,
            device_id_hex.clone(),
            DEVICE_LIST_KIND_RENAME,
            device_id,
            HlcTimestamp::from_sortable_bytes(&record.revision),
            serde_json::json!({
                "device_id": device_id_hex,
                "friendly_name": record.friendly_name,
                "name_record": record,
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
        let mut changes = Vec::new();
        for entry in device_list.devices {
            let record = device_list
                .name_records
                .get(&hex::encode(entry.device_id))
                .cloned();
            let change = Self::snapshot_change_with_name_record(
                Self::new_op_id(),
                entry,
                record,
                self.device_id.clone(),
                hlc,
            );
            changes.push(change);
        }
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
        let name_cursor_id = format!("{cursor_id}:name");
        let record_cursor = merge_store.get_record_cursor(&cursor_id)?;
        let name_cursor = merge_store.get_record_cursor(&name_cursor_id)?;
        let is_remove = change.kind == DEVICE_LIST_KIND_REMOVE;
        if record_cursor.as_ref().is_some_and(|cursor| {
            cursor.state == RecordState::Tombstoned && change.hlc <= cursor.hlc
        }) {
            return Ok(ApplyOutcome::Skipped);
        }
        let presence_is_newer = record_cursor
            .as_ref()
            .is_none_or(|cursor| change.hlc > cursor.hlc);
        let path = self.device_list_path();
        crate::control_store::update_device_list_file(&path, |device_list| {
            let existing_name_hlc = device_list
                .name_records
                .get(&change.record_id)
                .map(|record| HlcTimestamp::from_sortable_bytes(&record.revision));
            let applied_name = match change.kind.as_str() {
                DEVICE_LIST_KIND_RENAME => {
                    let friendly_name = change
                        .payload
                        .get("friendly_name")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| {
                            SyncError::Store("device-list rename missing friendly_name".into())
                        })?;
                    let record = match change.payload.get("name_record") {
                        Some(value) => serde_json::from_value::<DeviceNameRecord>(value.clone())?,
                        None => DeviceNameRecord {
                            friendly_name: friendly_name.to_owned(),
                            revision: change.hlc.to_sortable_bytes(),
                            ..Default::default()
                        },
                    };
                    if record.friendly_name != friendly_name
                        || record.revision != change.hlc.to_sortable_bytes()
                    {
                        return Err(SyncError::Store(
                            "device-list rename does not match committed name record".into(),
                        ));
                    }
                    Some(record)
                }
                DEVICE_LIST_KIND_SNAPSHOT => change
                    .payload
                    .get("name_record")
                    .map(|value| serde_json::from_value::<DeviceNameRecord>(value.clone()))
                    .transpose()?,
                DEVICE_LIST_KIND_REMOVE => None,
                other => {
                    return Err(SyncError::Store(format!(
                        "unsupported device-list change kind: {other}"
                    )))
                }
            };
            let existing_baseline = device_list
                .name_records
                .get(&change.record_id)
                .is_some_and(|record| record.is_saved_baseline(&device_id));
            let incoming_baseline = applied_name
                .as_ref()
                .is_some_and(|record| record.is_saved_baseline(&device_id));
            let generated_default = device_list
                .devices
                .iter()
                .find(|entry| entry.device_id == device_id)
                .is_some_and(is_generated_friendly_name);
            // Legacy main cursors may include genuine old name edits. Preserve
            // that frontier until an explicit name fact exists, but do not turn
            // a generated default's presence timestamp into name authority.
            let legacy_frontier = match name_cursor.as_ref() {
                Some(cursor) => (cursor.hlc.to_sortable_bytes() != [0; 20]).then_some(cursor.hlc),
                None if incoming_baseline && generated_default && existing_name_hlc.is_none() => {
                    None
                }
                None => record_cursor.as_ref().map(|cursor| cursor.hlc),
            };
            let name_hlc = if existing_baseline && !incoming_baseline {
                existing_name_hlc.into_iter().chain(legacy_frontier).max()
            } else if existing_name_hlc.is_some() {
                existing_name_hlc
            } else {
                legacy_frontier
            };
            let applied_name = applied_name.filter(|record| {
                let revision = HlcTimestamp::from_sortable_bytes(&record.revision);
                name_hlc.is_none_or(|old| revision > old)
                    && record_cursor.as_ref().is_none_or(|cursor| {
                        cursor.state != RecordState::Tombstoned || revision > cursor.hlc
                    })
            });
            if is_remove {
                if !presence_is_newer
                    || existing_name_hlc.is_some_and(|revision| change.hlc <= revision)
                    || merge_store.resolve_lww(&cursor_id, change.hlc, true)? == LwwDecision::Skip
                {
                    return Ok((ApplyOutcome::Skipped, false));
                }
                device_list.remove_device(&device_id);
                return Ok((
                    ApplyOutcome::Applied {
                        cursor: tombstone_cursor(change.hlc, None),
                    },
                    true,
                ));
            }
            if !presence_is_newer && applied_name.is_none() {
                return Ok((ApplyOutcome::Skipped, false));
            }
            match change.kind.as_str() {
                DEVICE_LIST_KIND_RENAME => {
                    if applied_name.is_none() {
                        return Ok((ApplyOutcome::Skipped, false));
                    }
                    if !device_list.contains_device(&device_id) {
                        return Err(SyncError::Store(
                            "device-list rename awaiting device snapshot".into(),
                        ));
                    }
                }
                DEVICE_LIST_KIND_SNAPSHOT => {
                    let remote: DeviceEntry = serde_json::from_value(change.payload.clone())?;
                    if remote.device_id != device_id {
                        return Err(SyncError::Store(
                            "device-list snapshot record_id does not match payload device_id"
                                .into(),
                        ));
                    }
                    if let Some(record) = change.payload.get("name_record") {
                        let record: DeviceNameRecord = serde_json::from_value(record.clone())?;
                        if record.friendly_name != remote.friendly_name {
                            return Err(SyncError::Store(
                                "device-list snapshot does not match committed name record".into(),
                            ));
                        }
                    }
                    if let Some(existing) = device_list
                        .devices
                        .iter_mut()
                        .find(|entry| entry.device_id == device_id)
                    {
                        if presence_is_newer {
                            existing.public_key = remote.public_key;
                            existing.label = remote.label;
                            existing.added_at_ms = existing.added_at_ms.min(remote.added_at_ms);
                            existing.last_seen_ms = existing.last_seen_ms.max(remote.last_seen_ms);
                            existing.direct_addrs = remote.direct_addrs;
                        }
                        // An unversioned snapshot only supplies an initial name.
                        // Export time and observer presence are never name edits.
                        if existing.friendly_name.is_empty() {
                            existing.friendly_name = remote.friendly_name;
                        }
                    } else {
                        device_list.devices.push(remote);
                    }
                }
                _ => unreachable!(),
            }
            let mut additional_cursors = Vec::new();
            if let Some(record) = applied_name {
                let revision = HlcTimestamp::from_sortable_bytes(&record.revision);
                device_list.rename_device(&device_id, &record.friendly_name);
                device_list
                    .name_records
                    .insert(change.record_id.clone(), record);
                additional_cursors.push((name_cursor_id.clone(), live_cursor(revision, None)));
            } else if let Some(record) = device_list.name_records.get(&change.record_id).cloned() {
                device_list.rename_device(&device_id, &record.friendly_name);
            }
            if name_cursor.is_none() && additional_cursors.is_empty() {
                additional_cursors.push((
                    name_cursor_id.clone(),
                    live_cursor(
                        name_hlc.unwrap_or(HlcTimestamp {
                            wall_ms: 0,
                            counter: 0,
                            device_id: [0; 8],
                        }),
                        None,
                    ),
                ));
            }
            let record_hlc = record_cursor
                .as_ref()
                .map_or(change.hlc, |cursor| cursor.hlc.max(change.hlc))
                .max(
                    device_list
                        .name_records
                        .get(&change.record_id)
                        .map_or(change.hlc, |record| {
                            HlcTimestamp::from_sortable_bytes(&record.revision)
                        }),
                );
            Ok((
                ApplyOutcome::AppliedWithCursors {
                    cursor: live_cursor(record_hlc, None),
                    additional_cursors,
                },
                true,
            ))
        })
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
    Ok(Some(crate::control_store::read_device_list_file(path)?))
}
