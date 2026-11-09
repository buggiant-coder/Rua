use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use rua_contract::agent::AgentInputMessage;
use rua_core::{ChatAddress, ContentPart};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_CHAT_MEDIA_BYTES: usize = 50 * 1024 * 1024;

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

#[derive(Debug, Serialize, Deserialize)]
struct ChatMediaMetadata {
    mime_type: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ChatMediaError {
    #[error("media payload is empty")]
    Empty,
    #[error("media payload exceeds the {MAX_CHAT_MEDIA_BYTES} byte limit")]
    TooLarge,
    #[error("invalid media id")]
    InvalidId,
    #[error("media not found")]
    NotFound,
    #[error("media storage error: {0}")]
    Storage(#[from] io::Error),
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ChatMediaService;

impl ChatMediaService {
    pub async fn collect_staged_scopes(
        &self,
        state: &crate::state::AppState,
    ) -> Result<usize, String> {
        let mut removed = 0;
        for agent in state
            .agent_service
            .list_agents()
            .await
            .map_err(|error| error.to_string())?
        {
            let root = PathBuf::from(agent.workspace_dir);
            removed += self
                .collect_staged_directory(
                    &state.conversation_authority,
                    &state.rua_root,
                    root.join("media"),
                    Duration::from_secs(24 * 60 * 60),
                )
                .await
                .map_err(|error| error.to_string())?;
        }
        for project in state
            .project_catalog
            .list()
            .map_err(|error| error.to_string())?
        {
            let root = state
                .rua_root
                .join("projects")
                .join(project.project_id.as_str())
                .join("media");
            removed += self
                .collect_staged_directory(
                    &state.conversation_authority,
                    &state.rua_root,
                    root,
                    Duration::from_secs(24 * 60 * 60),
                )
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(removed)
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
        address: ChatAddress,
        bytes: Vec<u8>,
        mime_type: Option<String>,
        original_filename: Option<String>,
    ) -> Result<StoredChatMedia, ChatMediaError> {
        self.collect_expired_staged(
            authority,
            rua_root,
            &address,
            Duration::from_secs(24 * 60 * 60),
        )
        .await?;
        let stored = self
            .store(
                rua_root,
                address.clone(),
                bytes,
                mime_type,
                original_filename,
            )
            .await?;
        let marker =
            media_dir(rua_root, address.clone()).join(format!(".staged-{}.json", stored.media_id));
        rua_config::atomic_write_json(&marker, &serde_json::json!({"address": address}))
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(stored)
    }

    pub async fn collect_expired_staged(
        &self,
        authority: &crate::services::conversation_authority::ConversationAuthority,
        rua_root: &Path,
        address: &ChatAddress,
        grace: Duration,
    ) -> Result<usize, ChatMediaError> {
        let dir = media_dir(rua_root, address.clone());
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
            let scope = rua_core::ChatScopeStore::resolve(rua_root, owner.clone());
            if media_dir(rua_root, owner.clone()) != dir {
                continue;
            }
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

    pub fn materialize_message_parts(
        &self,
        rua_root: &Path,
        address: &ChatAddress,
        messages: &mut [AgentInputMessage],
    ) {
        let workspace_root = rua_chat::ChatService::new(rua_root)
            .storage_root(address)
            .expect("Chat Media address resolves within Rua-owned storage");
        self.materialize_message_parts_at(&workspace_root, messages);
    }

    pub fn materialize_message_parts_at(
        &self,
        workspace_root: &Path,
        messages: &mut [AgentInputMessage],
    ) {
        for message in messages {
            for part in &mut message.content {
                let url = match part {
                    ContentPart::Image { url, .. }
                    | ContentPart::Audio { url, .. }
                    | ContentPart::Video { url, .. }
                    | ContentPart::File { url, .. } => url,
                    _ => continue,
                };
                if agent_types::content::is_managed_media_ref(url) {
                    *url = workspace_root
                        .join(url.as_str())
                        .to_string_lossy()
                        .into_owned();
                }
            }
        }
    }

    pub async fn store(
        &self,
        rua_root: &Path,
        address: ChatAddress,
        bytes: Vec<u8>,
        mime_type: Option<String>,
        original_filename: Option<String>,
    ) -> Result<StoredChatMedia, ChatMediaError> {
        if bytes.is_empty() {
            return Err(ChatMediaError::Empty);
        }
        if bytes.len() > MAX_CHAT_MEDIA_BYTES {
            return Err(ChatMediaError::TooLarge);
        }

        let media_id = MediaId::generate();
        let media_dir = media_dir(rua_root, address);
        let target = media_dir.join(media_id.to_string());
        let metadata_path = metadata_path(&media_dir, media_id);
        let normalized_mime_type = normalize_mime_type(mime_type);
        let metadata_bytes = serde_json::to_vec(&ChatMediaMetadata {
            mime_type: normalized_mime_type.clone(),
        })
        .map_err(|error| io::Error::other(format!("serialize media metadata: {error}")))?;
        let byte_size = bytes.len() as u64;
        tokio::task::spawn_blocking(move || {
            atomic_write_media(&media_dir, &target, &bytes, &metadata_path, &metadata_bytes)
        })
        .await
        .map_err(|error| io::Error::other(format!("media write task failed: {error}")))??;

        Ok(StoredChatMedia {
            media_ref: MediaReference::from_id(media_id),
            media_id: media_id.to_string(),
            mime_type: normalized_mime_type,
            byte_size,
            original_filename: normalize_original_filename(original_filename),
        })
    }

    pub async fn store_existing(
        &self,
        rua_root: &Path,
        address: ChatAddress,
        media_id: MediaId,
        bytes: Vec<u8>,
        mime_type: String,
    ) -> Result<(), ChatMediaError> {
        if bytes.is_empty() {
            return Err(ChatMediaError::Empty);
        }
        if bytes.len() > MAX_CHAT_MEDIA_BYTES {
            return Err(ChatMediaError::TooLarge);
        }
        let media_dir = media_dir(rua_root, address);
        let target = media_dir.join(media_id.to_string());
        let metadata_path = metadata_path(&media_dir, media_id);
        let metadata_bytes = serde_json::to_vec(&ChatMediaMetadata {
            mime_type: normalize_mime_type(Some(mime_type)),
        })
        .map_err(|error| io::Error::other(format!("serialize media metadata: {error}")))?;
        tokio::task::spawn_blocking(move || {
            atomic_write_media(&media_dir, &target, &bytes, &metadata_path, &metadata_bytes)
        })
        .await
        .map_err(|error| io::Error::other(format!("media write task failed: {error}")))??;
        Ok(())
    }

    pub async fn open(
        &self,
        rua_root: &Path,
        address: ChatAddress,
        raw_media_id: &str,
    ) -> Result<OpenedChatMedia, ChatMediaError> {
        let media_id = MediaId::from_str(raw_media_id).map_err(|_| ChatMediaError::InvalidId)?;
        let media_dir = media_dir(rua_root, address);
        let path = media_dir.join(media_id.to_string());
        let metadata = tokio::fs::symlink_metadata(&path).await.map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                ChatMediaError::NotFound
            } else {
                ChatMediaError::Storage(error)
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ChatMediaError::NotFound);
        }
        let mime_type = read_mime_type(&media_dir, media_id).await;
        Ok(OpenedChatMedia {
            mime_type,
            path,
            byte_size: metadata.len(),
        })
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
        let media_dir = media_dir(rua_root, address);
        if !media_dir.is_dir() {
            return Ok(OrphanMediaReport::default());
        }
        let mut report = OrphanMediaReport::default();
        for entry in fs::read_dir(&media_dir)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Ok(media_id) = MediaId::from_str(&name) else {
                continue;
            };
            report.scanned += 1;
            let age = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .unwrap_or_default();
            if age < grace || is_referenced(media_id) {
                continue;
            }
            let quarantine = media_dir.join(format!(".gc-{media_id}-{}", Uuid::new_v4()));
            fs::rename(entry.path(), &quarantine)?;
            if is_referenced(media_id) {
                fs::rename(&quarantine, entry.path())?;
                report.restored_after_recheck += 1;
                continue;
            }
            fs::remove_file(&quarantine)?;
            let _ = fs::remove_file(metadata_path(&media_dir, media_id));
            report.removed += 1;
        }
        File::open(&media_dir)?.sync_all()?;
        tracing::info!(
            scanned = report.scanned,
            removed = report.removed,
            restored_after_recheck = report.restored_after_recheck,
            "Chat Media orphan collection completed"
        );
        Ok(report)
    }
}

fn media_dir(rua_root: &Path, address: ChatAddress) -> PathBuf {
    rua_chat::ChatService::new(rua_root)
        .media_root(&address)
        .expect("Chat Media address resolves within Rua-owned storage")
}

fn metadata_path(media_dir: &Path, media_id: MediaId) -> PathBuf {
    media_dir.join(format!(".{media_id}.media.json"))
}

async fn read_mime_type(media_dir: &Path, media_id: MediaId) -> String {
    let metadata = tokio::fs::read(metadata_path(media_dir, media_id))
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ChatMediaMetadata>(&bytes).ok());
    metadata
        .map(|metadata| normalize_mime_type(Some(metadata.mime_type)))
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

fn atomic_write_media(
    media_dir: &Path,
    target: &Path,
    bytes: &[u8],
    metadata_path: &Path,
    metadata_bytes: &[u8],
) -> io::Result<()> {
    fs::create_dir_all(media_dir)?;
    atomic_write_file(media_dir, metadata_path, metadata_bytes)?;
    let result = atomic_write_file(media_dir, target, bytes);
    if result.is_err() {
        let _ = fs::remove_file(metadata_path);
        let _ = File::open(media_dir).and_then(|directory| directory.sync_all());
    }
    result
}

fn atomic_write_file(parent: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp_path = parent.join(format!(".upload-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut temp = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        temp.write_all(bytes)?;
        temp.sync_all()?;
        fs::rename(&temp_path, target)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn normalize_mime_type(value: Option<String>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

fn normalize_original_filename(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        Path::new(&value)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    })
}
