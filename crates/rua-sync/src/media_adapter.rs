use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use base64::Engine;
use rand::RngCore;
use rua_core::workspace::RuaRoot;
use serde::{Deserialize, Serialize};

use crate::crypto::{self, SyncKey};
use crate::endpoint::BlobsApiStore;
use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::{
    put_encrypted_blob, read_encrypted_blob, read_encrypted_blob_with_peer_download, SyncChange,
};
use crate::sidecar::{LwwDecision, RecordState};
use crate::sync_blob_store::StatefulBlobDownloader;
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const MEDIA_DOMAIN: &str = "media";
pub const MEDIA_KIND_UPSERT: &str = "upsert";
pub const MEDIA_KIND_DELETE: &str = "delete";
pub const MEDIA_KIND_SNAPSHOT: &str = "snapshot";
const INLINE_ENCRYPTED_FILE_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaFilePayload {
    pub media_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hash_ref: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_ciphertext_b64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub struct MediaSyncAdapter {
    data_dir: PathBuf,
    device_id: String,
    blob_context: Option<(BlobsApiStore, SyncKey)>,
    blob_download: Option<(StatefulBlobDownloader, Vec<iroh::EndpointId>)>,
}

impl MediaSyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
            blob_context: None,
            blob_download: None,
        }
    }

    pub fn with_blob_store(mut self, blobs_store: BlobsApiStore, sync_key: SyncKey) -> Self {
        self.blob_context = Some((blobs_store, sync_key));
        self
    }

    pub fn with_blob_download(
        mut self,
        downloader: StatefulBlobDownloader,
        providers: Vec<iroh::EndpointId>,
    ) -> Self {
        self.blob_download = Some((downloader, providers));
        self
    }

    pub fn record_id(media_id: &str) -> SyncResult<String> {
        validate_media_id(media_id)?;
        Ok(media_id.to_owned())
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
            serde_json::Value::Object(Default::default()),
        )
    }

    pub fn new_op_id() -> String {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    pub async fn snapshot_change_for_file(
        &self,
        media_id: &str,
        op_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncResult<SyncChange> {
        let path = self.media_file_path(media_id)?;
        self.snapshot_change_for_media_path(media_id, &path, op_id, device_id, hlc)
            .await
    }

    pub async fn snapshot_change_for_media_path(
        &self,
        media_id: &str,
        path: &Path,
        op_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncResult<SyncChange> {
        validate_media_id(media_id)?;
        let payload = self.payload_from_path(media_id, path).await?;
        Ok(SyncChange::new(
            op_id,
            MEDIA_DOMAIN,
            Self::record_id(media_id)?,
            MEDIA_KIND_UPSERT,
            device_id,
            hlc,
            serde_json::to_value(payload)?,
        ))
    }

    fn media_file_path(&self, media_id: &str) -> SyncResult<PathBuf> {
        validate_media_id(media_id)?;
        let root = RuaRoot::new(self.data_dir.clone());
        Ok(root.chat_workspace().media_dir().join(media_id))
    }

    async fn payload_from_path(&self, media_id: &str, path: &Path) -> SyncResult<MediaFilePayload> {
        let (blobs_store, sync_key) = self.blob_context.as_ref().ok_or_else(|| {
            SyncError::Store("media adapter missing encrypted blob context".into())
        })?;
        let content = std::fs::read(path)?;
        let (hash_ref, inline_ciphertext_b64) = if content.len() <= INLINE_ENCRYPTED_FILE_LIMIT {
            (
                String::new(),
                Some(
                    base64::engine::general_purpose::STANDARD
                        .encode(crypto::symmetric_encrypt(&content, sync_key)?),
                ),
            )
        } else {
            (
                put_encrypted_blob(blobs_store, sync_key, &content).await?,
                None,
            )
        };
        Ok(MediaFilePayload {
            media_id: media_id.to_string(),
            hash_ref,
            size: content.len() as u64,
            mime: read_media_mime(path),
            inline_ciphertext_b64,
            metadata: read_media_metadata(path)?,
            extra: Default::default(),
        })
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
impl DomainSyncAdapter for MediaSyncAdapter {
    fn domain(&self) -> &str {
        MEDIA_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let mut changes = Vec::new();
        let media_dir = RuaRoot::new(&self.data_dir).chat_workspace().media_dir();
        let files = match std::fs::read_dir(media_dir) {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(changes),
            Err(error) => return Err(error.into()),
        };
        for file in files {
            let file = file?;
            if !file.file_type()?.is_file() {
                continue;
            }
            let name = file.file_name();
            let Some(media_id) = name.to_str() else {
                continue;
            };
            if validate_media_id(media_id).is_err() {
                continue;
            }
            let path = file.path();
            changes.push(
                self.snapshot_change_for_media_path(
                    media_id,
                    &path,
                    Self::new_op_id(),
                    &self.device_id,
                    Self::export_hlc(&path, &self.device_id),
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
                "media adapter received wrong domain: {}",
                change.domain
            )));
        }

        let media_id = parse_record_id(&change.record_id)?;
        let cursor_id = sync_cursor_record_id(MEDIA_DOMAIN, &change.record_id);
        let is_delete = change.kind == MEDIA_KIND_DELETE;
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }
        if !is_delete {
            if let Some(cursor) = merge_store.get_record_cursor(&cursor_id)? {
                if cursor.state == RecordState::Live && change.hlc <= cursor.hlc {
                    return Ok(ApplyOutcome::Skipped);
                }
            }
        }

        let path = self.media_file_path(media_id)?;
        if is_delete {
            remove_file_if_exists(&path)?;
            remove_file_if_exists(&blobhash_sidecar_path(&path))?;
            remove_file_if_exists(&media_metadata_path(&path))?;
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }

        if change.kind != MEDIA_KIND_UPSERT && change.kind != MEDIA_KIND_SNAPSHOT {
            return Err(SyncError::Store(format!(
                "unsupported media change kind: {}",
                change.kind
            )));
        }

        let payload: MediaFilePayload = serde_json::from_value(change.payload.clone())?;
        if payload.media_id != media_id {
            return Err(SyncError::Store(format!(
                "media id mismatch: {} != {}",
                payload.media_id, media_id
            )));
        }
        let (blobs_store, sync_key) = self.blob_context.as_ref().ok_or_else(|| {
            SyncError::Store("media adapter missing encrypted blob context".into())
        })?;
        let content = if let Some(content) = read_inline_media(&payload, sync_key)? {
            content
        } else {
            if payload.hash_ref.is_empty() {
                return Err(SyncError::Store(format!(
                    "media {} has neither inline content nor blob hash",
                    change.record_id
                )));
            }
            let content = if let Some((downloader, providers)) = &self.blob_download {
                read_encrypted_blob_with_peer_download(
                    blobs_store,
                    sync_key,
                    &payload.hash_ref,
                    downloader,
                    providers.clone(),
                )
                .await?
            } else {
                read_encrypted_blob(blobs_store, sync_key, &payload.hash_ref).await?
            };
            content.ok_or_else(|| {
                SyncError::Store(format!(
                    "media encrypted blob missing: {}",
                    payload.hash_ref
                ))
            })?
        };
        if content.len() as u64 != payload.size {
            return Err(SyncError::Store(format!(
                "media size mismatch for {}",
                change.record_id
            )));
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        atomic_write_media_replica(&path, &content)?;
        remove_file_if_exists(&blobhash_sidecar_path(&path))?;
        write_media_metadata(&path, payload.mime.as_deref(), payload.metadata)?;
        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }
}

pub fn parse_record_id(record_id: &str) -> SyncResult<&str> {
    validate_media_id(record_id)?;
    Ok(record_id)
}

fn validate_media_id(media_id: &str) -> SyncResult<()> {
    uuid::Uuid::parse_str(media_id)
        .map(|_| ())
        .map_err(|_| SyncError::Store(format!("invalid Media id: {media_id}")))
}

fn remove_file_if_exists(path: &Path) -> SyncResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn blobhash_sidecar_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.blobhash", path.display()))
}

fn read_inline_media(
    payload: &MediaFilePayload,
    sync_key: &SyncKey,
) -> SyncResult<Option<Vec<u8>>> {
    let Some(ciphertext_b64) = payload.inline_ciphertext_b64.as_deref() else {
        return Ok(None);
    };
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(ciphertext_b64)
        .map_err(|e| SyncError::Store(format!("invalid inline media ciphertext: {e}")))?;
    let plaintext = crypto::symmetric_decrypt(&ciphertext, sync_key)?;
    if plaintext.len() as u64 != payload.size {
        return Err(SyncError::Store(format!(
            "inline media size mismatch for {}",
            payload.media_id
        )));
    }
    Ok(Some(plaintext))
}

fn media_metadata_path(path: &Path) -> PathBuf {
    let media_id = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    path.with_file_name(format!(".{media_id}.media.json"))
}

fn read_media_mime(path: &Path) -> Option<String> {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(media_metadata_path(path)).ok()?).ok()?;
    value
        .get("mime_type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn read_media_metadata(
    path: &Path,
) -> SyncResult<Option<serde_json::Map<String, serde_json::Value>>> {
    match std::fs::read(media_metadata_path(path)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_media_metadata(
    path: &Path,
    mime: Option<&str>,
    metadata: Option<serde_json::Map<String, serde_json::Value>>,
) -> SyncResult<()> {
    let metadata_path = media_metadata_path(path);
    let mut metadata = metadata.unwrap_or_default();
    if let Some(mime) = mime {
        metadata.insert("mime_type".into(), mime.into());
    }
    if metadata.is_empty() {
        remove_file_if_exists(&metadata_path)?;
    } else {
        atomic_write_media_replica(&metadata_path, &serde_json::to_vec(&metadata)?)?;
    }
    Ok(())
}

fn atomic_write_media_replica(path: &Path, bytes: &[u8]) -> SyncResult<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| SyncError::Store("media path has no parent".into()))?;
    let temp = parent.join(format!(".{}.tmp", MediaSyncAdapter::new_op_id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        rua_core::fs::sync_directory(parent)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.map_err(Into::into)
}
