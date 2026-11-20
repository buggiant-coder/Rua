mod remote;
pub(crate) use remote::local_chat_media_manifest;
pub use remote::{
    chat_media_manifest, chat_media_manifest_from_device,
    chat_media_manifest_from_device_for_submission, resolve_chat_media,
};

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use rua_core::ChatAddress;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rua_session::chat_media::ChatMediaManifest;
pub use rua_session::chat_media::MAX_CHAT_MEDIA_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MediaId(Uuid);

impl MediaId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for MediaId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for MediaId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaReference(String);

impl MediaReference {
    pub fn from_id(id: MediaId) -> Self {
        Self(format!("media/{id}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredChatMedia {
    pub media_ref: MediaReference,
    pub media_id: String,
    pub mime_type: String,
    pub byte_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_filename: Option<String>,
}

#[derive(Debug)]
pub struct OpenedChatMedia {
    pub path: PathBuf,
    pub mime_type: String,
    pub byte_size: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OrphanMediaReport {
    pub scanned: usize,
    pub removed: usize,
    pub restored_after_recheck: usize,
}

#[derive(Debug)]
pub struct ChatMediaUpload {
    pub bytes: Vec<u8>,
    pub mime_type: Option<String>,
    pub original_filename: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ChatMediaError {
    #[error("media payload is empty")]
    Empty,
    #[error("media payload exceeds the {MAX_CHAT_MEDIA_BYTES} byte limit")]
    TooLarge,
    #[error("invalid media id")]
    InvalidId,
    #[error("Chat Media conflict: {0}")]
    Conflict(String),
    #[error("Chat Media unavailable: {0}")]
    Unavailable(String),
    #[error("media not found")]
    NotFound,
    #[error("media storage error: {0}")]
    Storage(#[from] io::Error),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ChatMediaService;

#[derive(Debug, Serialize, Deserialize)]
pub struct StagedChatMedia {
    pub address: ChatAddress,
    pub project_id: Option<String>,
}

impl ChatMediaService {
    pub async fn collect_staged_scopes(
        &self,
        state: &crate::state::AppState,
    ) -> Result<usize, String> {
        self.collect_staged_directory(
            &state.conversation_authority,
            &state.rua_root,
            rua_chat::ChatService::new(&state.rua_root).media_root(),
            Duration::from_secs(24 * 60 * 60),
        )
        .await
        .map_err(|error| error.to_string())
    }

    pub(crate) fn record_first_references(
        scope_root: &Path,
        staging_root: &Path,
        address: &ChatAddress,
        ids: &[String],
    ) -> Result<(), String> {
        let dir = scope_root.join("media");
        let chat_id = address.chat_id().as_str();
        let committed = rua_session::WorkspaceSessionStore::new(scope_root).chat_dir(chat_id);
        let staged = rua_session::WorkspaceSessionStore::new(staging_root).chat_dir(chat_id);
        for id in ids {
            let id = Uuid::parse_str(id).map_err(|e| e.to_string())?;
            let item = match ChatMediaManifest::load(&dir, id).map_err(|e| e.to_string())? {
                Some(item) => Some(item),
                None => {
                    ChatMediaManifest::load_reference(&committed, id).map_err(|e| e.to_string())?
                }
            }
            .ok_or_else(|| format!("Missing admitted Chat Media reference: media/{id}"))?;
            item.record_reference(&staged, chat_id)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub(crate) async fn confirm_staged_media(root: &Path, ids: &[String]) -> Result<(), String> {
        for id in ids {
            MediaId::from_str(id).map_err(|error| error.to_string())?;
            let path = root.join("media").join(format!(".staged-{id}.json"));
            if let Err(error) = tokio::fs::remove_file(path).await {
                if error.kind() != io::ErrorKind::NotFound {
                    return Err(error.to_string());
                }
            }
        }
        Ok(())
    }

    pub async fn store_staged(
        &self,
        authority: &crate::services::conversation_authority::ConversationAuthority,
        rua_root: &Path,
        draft: StagedChatMedia,
        device_id: &str,
        upload: ChatMediaUpload,
    ) -> Result<StoredChatMedia, ChatMediaError> {
        // Expiration runs in the background. An upload touches only this draft and object.
        let gate = authority.transition_gate_at(&draft.address);
        let _transition = gate.lock().await;
        let scope = rua_core::ChatStore::resolve(rua_root, draft.address.clone());
        if rua_session::chat_deletion::ChatDeleteIntentStore::from_rua_root(rua_root)
            .load(&draft.address)
            .map_err(io::Error::other)?
            .is_some()
        {
            return Err(ChatMediaError::NotFound);
        }
        if rua_session::WorkspaceSessionStore::new(scope.root())
            .load_stored_chat_header(draft.address.chat_id().as_str())
            .await
            .map_err(io::Error::other)?
            .is_some()
            || crate::services::remote_conversation_input::read_source_pending_at(
                rua_root,
                &draft.address,
            )
            .await
            .map_err(io::Error::other)?
            .is_some()
        {
            return Err(ChatMediaError::Conflict(
                "Chat was already created; refresh its Authority before uploading".into(),
            ));
        }
        let id = MediaId::generate();
        let marker = media_dir(rua_root).join(format!(".staged-{id}.json"));
        rua_config::atomic_write_json(&marker, &serde_json::json!(draft))?;
        let result = self
            .store_upload(rua_root, draft.address, device_id, id, upload)
            .await;
        if result.is_err() {
            if let Err(error) = tokio::fs::remove_file(&marker).await {
                tracing::warn!(%error, %id, "Could not remove failed Chat Media staging marker");
            }
        }
        result
    }

    pub async fn collect_expired_staged(
        &self,
        authority: &crate::services::conversation_authority::ConversationAuthority,
        rua_root: &Path,
        _address: &ChatAddress,
        grace: Duration,
    ) -> Result<usize, ChatMediaError> {
        let dir = media_dir(rua_root);
        self.collect_staged_directory(authority, rua_root, dir, grace)
            .await
    }

    async fn collect_staged_directory(
        &self,
        authority: &crate::services::conversation_authority::ConversationAuthority,
        rua_root: &Path,
        dir: PathBuf,
        grace: Duration,
    ) -> Result<usize, ChatMediaError> {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
        };
        let mut removed = 0;
        while let Some(entry) = entries.next_entry().await? {
            if removed == 100 {
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(id) = name
                .strip_prefix(".staged-")
                .and_then(|name| name.strip_suffix(".json"))
            else {
                continue;
            };
            let Ok(media_id) = MediaId::from_str(id) else {
                continue;
            };
            let metadata = tokio::fs::symlink_metadata(entry.path()).await?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.modified()?.elapsed().unwrap_or_default() < grace
            {
                continue;
            }
            let value: serde_json::Value = match tokio::fs::read(entry.path())
                .await
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            {
                Some(value) => value,
                None => continue,
            };
            let owner: ChatAddress = match value
                .get("address")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
            {
                Ok(Some(owner)) => owner,
                _ => continue,
            };
            let scope = rua_core::ChatStore::resolve(rua_root, owner.clone());
            let gate = authority.transition_gate_at(&owner);
            let _transition = gate.lock().await;
            match rua_session::WorkspaceSessionStore::new(scope.root())
                .load_chat_header(owner.chat_id().as_str())
                .await
            {
                Ok(None) => {}
                Ok(Some(_)) => {
                    tokio::fs::remove_file(entry.path()).await?;
                    continue;
                }
                Err(_) => continue,
            }
            if let Some(manifest) = ChatMediaManifest::load(&dir, media_id.as_uuid())? {
                if retained_by_another_chat(rua_root, &manifest, &owner)? {
                    tokio::fs::remove_file(entry.path()).await?;
                    continue;
                }
            }
            let file = dir.join(media_id.to_string());
            let metadata = match tokio::fs::symlink_metadata(&file).await {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                continue;
            }
            tokio::fs::remove_file(file).await?;
            let sidecar = metadata_path(&dir, media_id);
            if let Err(error) = tokio::fs::remove_file(sidecar).await {
                if error.kind() != io::ErrorKind::NotFound {
                    return Err(error.into());
                }
            }
            tokio::fs::remove_file(entry.path()).await?;
            removed += 1;
        }
        Ok(removed)
    }

    pub async fn store(
        &self,
        rua_root: &Path,
        address: ChatAddress,
        device_id: &str,
        upload: ChatMediaUpload,
    ) -> Result<StoredChatMedia, ChatMediaError> {
        self.store_upload(rua_root, address, device_id, MediaId::generate(), upload)
            .await
    }

    async fn store_upload(
        &self,
        rua_root: &Path,
        address: ChatAddress,
        device_id: &str,
        id: MediaId,
        upload: ChatMediaUpload,
    ) -> Result<StoredChatMedia, ChatMediaError> {
        if upload.bytes.is_empty() {
            return Err(ChatMediaError::Empty);
        }
        if upload.bytes.len() > MAX_CHAT_MEDIA_BYTES {
            return Err(ChatMediaError::TooLarge);
        }
        let dir = media_dir(rua_root);
        let device_id = device_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut manifest = ChatMediaManifest::new(
                id.as_uuid(),
                address.chat_id().as_str(),
                &device_id,
                normalize_mime_type(upload.mime_type),
                &upload.bytes,
            )?;
            manifest.original_filename = normalize_original_filename(upload.original_filename);
            manifest.store_copy(&dir, &upload.bytes)?;
            Ok(StoredChatMedia {
                media_ref: MediaReference::from_id(id),
                media_id: id.to_string(),
                mime_type: manifest.mime_type,
                byte_size: manifest.byte_size,
                original_filename: manifest.original_filename,
            })
        })
        .await
        .map_err(|error| io::Error::other(error.to_string()))?
    }

    pub async fn open(
        &self,
        rua_root: &Path,
        address: ChatAddress,
        raw_media_id: &str,
    ) -> Result<OpenedChatMedia, ChatMediaError> {
        let media_id = MediaId::from_str(raw_media_id).map_err(|_| ChatMediaError::InvalidId)?;
        let manifest = local_chat_media_manifest(rua_root, &address, raw_media_id).await?;
        let dir = media_dir(rua_root);
        tokio::task::spawn_blocking(move || {
            manifest
                .verify_copy(&dir)
                .map_err(|error| ChatMediaError::Unavailable(error.to_string()))?;
            Ok(OpenedChatMedia {
                path: dir.join(media_id.to_string()),
                byte_size: manifest.byte_size,
                mime_type: manifest.mime_type,
            })
        })
        .await
        .map_err(|e| io::Error::other(e.to_string()))?
    }

    pub fn collect_orphans<F>(
        &self,
        rua_root: &Path,
        address: ChatAddress,
        grace: Duration,
        is_referenced: F,
    ) -> Result<OrphanMediaReport, ChatMediaError>
    where
        F: Fn(MediaId) -> bool,
    {
        let media_dir = media_dir(rua_root);
        if !media_dir.is_dir() {
            return Ok(OrphanMediaReport::default());
        }
        let mut report = OrphanMediaReport::default();
        for entry in fs::read_dir(&media_dir)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Ok(media_id) = MediaId::from_str(&name) else {
                continue;
            };
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                continue;
            }
            report.scanned += 1;
            let age = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .unwrap_or_default();
            let Some(manifest) = ChatMediaManifest::load(&media_dir, media_id.as_uuid())? else {
                continue;
            };
            if age < grace
                || !manifest.chat_ids.contains(address.chat_id().as_str())
                || retained_by_another_chat(rua_root, &manifest, &address)?
                || is_referenced(media_id)
            {
                continue;
            }
            let quarantine = media_dir.join(format!(".gc-{media_id}-{}", Uuid::new_v4()));
            fs::rename(entry.path(), &quarantine)?;
            let retained =
                ChatMediaManifest::load(&media_dir, media_id.as_uuid()).and_then(|manifest| {
                    match manifest {
                        Some(manifest) => retained_by_another_chat(rua_root, &manifest, &address),
                        None => Ok(true),
                    }
                });
            if !matches!(retained, Ok(false)) || is_referenced(media_id) {
                fs::rename(&quarantine, entry.path())?;
                retained?;
                report.restored_after_recheck += 1;
                continue;
            }
            fs::remove_file(&quarantine)?;
            let _ = fs::remove_file(metadata_path(&media_dir, media_id));
            report.removed += 1;
        }
        rua_core::fs::sync_directory(&media_dir)?;
        tracing::info!(
            scanned = report.scanned,
            removed = report.removed,
            restored_after_recheck = report.restored_after_recheck,
            "Chat Media orphan collection completed"
        );
        Ok(report)
    }
}

fn retained_by_another_chat(
    rua_root: &Path,
    manifest: &ChatMediaManifest,
    owner: &ChatAddress,
) -> io::Result<bool> {
    let deletes = rua_session::chat_deletion::ChatDeleteIntentStore::from_rua_root(rua_root);
    for id in &manifest.chat_ids {
        if id != owner.chat_id().as_str()
            && deletes
                .load(&ChatAddress::new(id.clone()))
                .map_err(io::Error::other)?
                .is_none()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn media_dir(rua_root: &Path) -> PathBuf {
    rua_chat::ChatService::new(rua_root).media_root()
}

fn metadata_path(media_dir: &Path, media_id: MediaId) -> PathBuf {
    media_dir.join(format!(".{media_id}.media.json"))
}

pub(crate) fn normalize_mime_type(value: Option<String>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

pub(crate) fn normalize_original_filename(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        Path::new(&value)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    })
}
