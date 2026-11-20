use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rua_session::chat_media::ChatMediaManifest;
use tokio::sync::Mutex as AsyncMutex;

use super::device_command_bus::ChatMediaPrepareCommand;

impl ChatMediaPrepareCommand {
    pub(crate) fn manifest(&self) -> ChatMediaManifest {
        ChatMediaManifest {
            media_ref: format!("media/{}", self.media_id),
            chat_ids: [self.address.chat_id().to_string()].into(),
            device_ids: [self.target_device_id.clone()].into(),
            mime_type: self.mime_type.clone(),
            byte_size: self.byte_size,
            sha256: self.sha256.clone(),
            original_filename: self.original_filename.clone(),
            origin: None,
            extra: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthorizedMediaUpload {
    pub command: ChatMediaPrepareCommand,
    pub media_dir: PathBuf,
    pub authorized_at: Instant,
    pub(crate) staging: Arc<tempfile::TempDir>,
    transition_gate: Option<Arc<AsyncMutex<()>>>,
}

impl AuthorizedMediaUpload {
    pub fn new(
        command: ChatMediaPrepareCommand,
        media_dir: PathBuf,
        transition_gate: Option<Arc<AsyncMutex<()>>>,
    ) -> std::io::Result<Self> {
        Ok(Self {
            command,
            media_dir,
            staging: Arc::new(tempfile::tempdir()?),
            transition_gate,
            authorized_at: Instant::now(),
        })
    }

    pub(crate) async fn check_placement(&self) -> Result<(), String> {
        let workspace = self.media_dir.parent().ok_or("Missing Chat workspace")?;
        let placement =
            super::conversation_authority_target::load_optional_chat_authority_placement(
                workspace,
                self.command.address.chat_id().as_str(),
            )
            .await?
            .ok_or("Conversation Authority placement is not materialized")?;
        if placement.device_id != self.command.target_device_id
            || placement.authority_epoch != self.command.authority_epoch
        {
            return Err(format!(
                "Conversation Authority changed to '{}' at epoch '{}'",
                placement.device_id, placement.authority_epoch
            ));
        }
        Ok(())
    }

    pub(crate) fn check_existing(&self) -> Result<bool, String> {
        let manifest = self.command.manifest();
        let id = manifest.id().map_err(|e| e.to_string())?;
        if let Some(existing) =
            ChatMediaManifest::load(&self.media_dir, id).map_err(|e| e.to_string())?
        {
            if !existing
                .chat_ids
                .contains(self.command.address.chat_id().as_str())
                || existing.sha256 != manifest.sha256
                || existing.byte_size != manifest.byte_size
                || existing.mime_type != manifest.mime_type
                || existing.original_filename != manifest.original_filename
            {
                return Err(
                    "Existing Chat Media belongs to a different Chat or has different metadata"
                        .into(),
                );
            }
            return Ok(existing.verify_copy(&self.media_dir).is_ok());
        }
        if self.media_dir.join(id.to_string()).exists() {
            return Err("Existing Chat Media has no authorized reference".into());
        }
        Ok(false)
    }
}

#[derive(Debug, Default, Clone)]
pub struct ChatMediaTransferRegistry {
    uploads: Arc<Mutex<HashMap<String, AuthorizedMediaUpload>>>,
}

impl ChatMediaTransferRegistry {
    pub const AUTHORIZATION_TTL: Duration = Duration::from_secs(5 * 60);

    pub fn authorize_upload(&self, upload: AuthorizedMediaUpload) -> Result<(), String> {
        let mut uploads = self.uploads.lock().unwrap_or_else(|lock| lock.into_inner());
        uploads.retain(|_, upload| upload.authorized_at.elapsed() < Self::AUTHORIZATION_TTL);
        if uploads.contains_key(&upload.command.operation_id)
            || uploads.values().any(|active| {
                active.media_dir == upload.media_dir
                    && active.command.media_id == upload.command.media_id
            })
        {
            return Err("Chat Media already has a pending upload".into());
        }
        uploads.insert(upload.command.operation_id.clone(), upload);
        Ok(())
    }

    pub fn validate_upload(
        &self,
        operation_id: &str,
        source_device_id: &str,
        target_device_id: &str,
        relative_path: &Path,
        byte_size: u64,
    ) -> Result<Option<AuthorizedMediaUpload>, String> {
        let uploads = self.uploads.lock().unwrap_or_else(|lock| lock.into_inner());
        let Some(upload) = uploads.get(operation_id) else {
            return Ok(None);
        };
        if upload.authorized_at.elapsed() >= Self::AUTHORIZATION_TTL {
            return Err("Media transfer authorization expired".into());
        }
        let command = &upload.command;
        if command.source_device_id != source_device_id
            || command.target_device_id != target_device_id
            || relative_path != Path::new(&command.media_id)
            || command.byte_size != byte_size
        {
            return Err("Media transfer does not match its authorized operation".into());
        }
        Ok(Some(upload.clone()))
    }

    pub async fn complete_upload(&self, operation_id: &str) -> Result<(), String> {
        let upload = self
            .uploads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(operation_id)
            .cloned()
            .ok_or("Media transfer is not authorized")?;
        let transition = match upload.transition_gate.clone() {
            Some(gate) => Some(gate.lock_owned().await),
            None => None,
        };
        upload.check_placement().await?;
        let registry = self.clone();
        tokio::task::spawn_blocking(move || {
            let _transition = transition;
            let bytes = std::fs::read(upload.staging.path().join(&upload.command.media_id))
                .map_err(|e| e.to_string())?;
            let mut uploads = registry.uploads.lock().unwrap_or_else(|e| e.into_inner());
            if upload.authorized_at.elapsed() >= Self::AUTHORIZATION_TTL
                || !uploads
                    .get(&upload.command.operation_id)
                    .is_some_and(|active| Arc::ptr_eq(&active.staging, &upload.staging))
            {
                return Err("Media transfer authorization expired".into());
            }
            upload.check_existing()?;
            upload
                .command
                .manifest()
                .store_copy(&upload.media_dir, &bytes)
                .map_err(|e| e.to_string())?;
            uploads.remove(&upload.command.operation_id);
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    pub fn abort(&self, operation_id: &str) -> bool {
        self.uploads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(operation_id)
            .is_some()
    }
}
