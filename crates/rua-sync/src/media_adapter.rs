use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rua_core::workspace::RuaRoot;
use rua_session::chat_media::ChatMediaManifest;
use uuid::Uuid;

use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::{LwwDecision, RecordState};
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const MEDIA_DOMAIN: &str = "media";
pub const MEDIA_KIND_UPSERT: &str = "upsert";
pub const MEDIA_KIND_DELETE: &str = "delete";
pub const MEDIA_KIND_SNAPSHOT: &str = "snapshot";

pub struct MediaSyncAdapter {
    media_dir: PathBuf,
    device_id: String,
}

impl MediaSyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            media_dir: RuaRoot::new(data_dir).chat_workspace().media_dir(),
            device_id: device_id.into(),
        }
    }

    pub fn record_id(media_id: &str) -> SyncResult<String> {
        Ok(parse_record_id(media_id)?.to_owned())
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        media_id: &str,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            MEDIA_DOMAIN,
            Self::record_id(media_id).expect("validated Media id"),
            MEDIA_KIND_DELETE,
            device_id,
            hlc,
            serde_json::json!({}),
        )
    }

    pub fn new_op_id() -> String {
        Uuid::new_v4().simple().to_string()
    }

    pub async fn snapshot_change_for_file(
        &self,
        media_id: &str,
        op_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncResult<SyncChange> {
        let id = parse_id(media_id)?;
        let manifest = ChatMediaManifest::load(&self.media_dir, id)?
            .ok_or_else(|| SyncError::Store(format!("Missing Chat Media metadata for {id}")))?;
        Ok(SyncChange::new(
            op_id,
            MEDIA_DOMAIN,
            media_id,
            MEDIA_KIND_UPSERT,
            device_id,
            hlc,
            serde_json::to_value(manifest)?,
        ))
    }

    fn export_hlc(&self, path: &Path) -> HlcTimestamp {
        let wall_ms = path
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut device = [0u8; 8];
        device.copy_from_slice(&blake3::hash(self.device_id.as_bytes()).as_bytes()[..8]);
        HlcTimestamp {
            wall_ms,
            counter: 0,
            device_id: device,
        }
    }
}

#[async_trait]
impl DomainSyncAdapter for MediaSyncAdapter {
    fn domain(&self) -> &str {
        MEDIA_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let mut changes = Vec::new();
        let entries = match std::fs::read_dir(&self.media_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(changes),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name
                .to_str()
                .and_then(|s| s.strip_prefix('.'))
                .and_then(|s| s.strip_suffix(".media.json"))
            else {
                continue;
            };
            if parse_id(id).is_err()
                || !entry.file_type()?.is_file()
                || self
                    .media_dir
                    .join(format!(".staged-{id}.json"))
                    .try_exists()?
            {
                continue;
            }
            changes.push(
                self.snapshot_change_for_file(
                    id,
                    Self::new_op_id(),
                    &self.device_id,
                    self.export_hlc(&entry.path()),
                )
                .await?,
            );
        }
        changes.sort_by(|a, b| a.record_id.cmp(&b.record_id));
        Ok(changes)
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != MEDIA_DOMAIN {
            return Err(SyncError::Store(format!(
                "Unexpected Media domain: {}",
                change.domain
            )));
        }
        let id = parse_id(&change.record_id)?;
        let key = sync_cursor_record_id(MEDIA_DOMAIN, &change.record_id);
        let previous = merge_store.get_record_cursor(&key)?;
        match change.kind.as_str() {
            MEDIA_KIND_DELETE => {
                if merge_store.resolve_lww(&key, change.hlc, true)? == LwwDecision::Skip {
                    return Ok(ApplyOutcome::Skipped);
                }
                for path in [
                    self.media_dir.join(id.to_string()),
                    self.media_dir.join(format!(".{id}.media.json")),
                ] {
                    if let Err(error) = std::fs::remove_file(path) {
                        if error.kind() != std::io::ErrorKind::NotFound {
                            return Err(error.into());
                        }
                    }
                }
                if self.media_dir.is_dir() {
                    rua_core::fs::sync_directory(&self.media_dir)?;
                }
                Ok(ApplyOutcome::Applied {
                    cursor: tombstone_cursor(change.hlc, None),
                })
            }
            MEDIA_KIND_UPSERT | MEDIA_KIND_SNAPSHOT => {
                // The UUID identifies an immutable object. A completed reclamation is final.
                if previous
                    .as_ref()
                    .is_some_and(|p| p.state == RecordState::Tombstoned)
                {
                    return Ok(ApplyOutcome::Skipped);
                }
                let manifest: ChatMediaManifest = serde_json::from_value(change.payload.clone())?;
                if manifest.id()? != id {
                    return Err(SyncError::Store("Chat Media metadata ID mismatch".into()));
                }
                manifest.register(&self.media_dir)?;
                let latest = previous
                    .map(|c| c.hlc.max(change.hlc))
                    .unwrap_or(change.hlc);
                Ok(ApplyOutcome::Applied {
                    cursor: live_cursor(latest, None),
                })
            }
            kind => Err(SyncError::Store(format!(
                "Unsupported Media change kind: {kind}"
            ))),
        }
    }
}

fn parse_id(raw: &str) -> SyncResult<Uuid> {
    Uuid::parse_str(raw)
        .ok()
        .filter(|id| id.to_string() == raw)
        .ok_or_else(|| SyncError::Store(format!("Invalid Media id: {raw}")))
}

pub fn parse_record_id(raw: &str) -> SyncResult<&str> {
    parse_id(raw)?;
    Ok(raw)
}
