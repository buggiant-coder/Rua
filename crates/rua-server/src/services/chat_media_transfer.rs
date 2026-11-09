use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct AuthorizedMediaUpload {
    pub operation_id: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub media_dir: PathBuf,
    pub media_id: String,
    pub byte_size: u64,
    pub mime_type: String,
    pub authorized_at: Instant,
}

#[derive(Debug, Default, Clone)]
pub struct ChatMediaTransferRegistry {
    uploads: Arc<Mutex<HashMap<String, AuthorizedMediaUpload>>>,
}

impl ChatMediaTransferRegistry {
    pub const AUTHORIZATION_TTL: Duration = Duration::from_secs(5 * 60);

    pub fn authorize_upload(&self, upload: AuthorizedMediaUpload) -> Result<(), String> {
        std::fs::create_dir_all(&upload.media_dir)
            .map_err(|error| format!("create Chat Media directory: {error}"))?;
        std::fs::File::open(&upload.media_dir)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync Chat Media directory: {error}"))?;
        let mut uploads = self.uploads.lock().unwrap_or_else(|lock| lock.into_inner());
        uploads.retain(|_, upload| upload.authorized_at.elapsed() < Self::AUTHORIZATION_TTL);
        if uploads.contains_key(&upload.operation_id) {
            return Err("Media transfer operation already exists".to_string());
        }
        uploads.insert(upload.operation_id.clone(), upload);
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
            return Err("Media transfer authorization expired".to_string());
        }
        if upload.source_device_id != source_device_id
            || upload.target_device_id != target_device_id
            || relative_path != Path::new(&upload.media_id)
            || upload.byte_size != byte_size
        {
            return Err("Media transfer does not match its authorized operation".to_string());
        }
        Ok(Some(upload.clone()))
    }

    pub fn complete_upload(&self, operation_id: &str) -> Result<bool, String> {
        let Some(upload) = self
            .uploads
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .remove(operation_id)
        else {
            return Ok(false);
        };
        let metadata_path = upload
            .media_dir
            .join(format!(".{}.media.json", upload.media_id));
        let bytes = serde_json::to_vec(&MediaMetadata {
            mime_type: upload.mime_type,
        })
        .map_err(|error| error.to_string())?;
        let temp_path = upload
            .media_dir
            .join(format!(".metadata-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            use std::io::Write;
            let mut temp = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp_path)?;
            temp.write_all(&bytes)?;
            temp.sync_all()?;
            std::fs::rename(&temp_path, &metadata_path)?;
            std::fs::File::open(&upload.media_dir)?.sync_all()
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp_path);
        }
        result.map_err(|error| error.to_string())?;
        tracing::info!(
            operation_id,
            byte_size = upload.byte_size,
            result = "completed",
            "Chat Media transfer completed"
        );
        Ok(true)
    }

    pub fn abort(&self, operation_id: &str) -> bool {
        let removed = self
            .uploads
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .remove(operation_id)
            .is_some();
        if removed {
            tracing::info!(
                operation_id,
                result = "aborted",
                "Chat Media transfer aborted"
            );
        }
        removed
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct MediaMetadata {
    mime_type: String,
}
