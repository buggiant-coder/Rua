use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rua_core::ChatAddress;
use serde::{Deserialize, Serialize};

const DELETE_INTENT_DIR: &str = "chat-delete-intents";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatDeleteIntent {
    pub address: ChatAddress,
    pub intent_id: String,
    pub deleted_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ChatDeleteIntentStore {
    root: PathBuf,
}

impl ChatDeleteIntentStore {
    pub fn from_rua_root(rua_root: impl AsRef<Path>) -> Self {
        Self {
            root: rua_root.as_ref().join("sync").join(DELETE_INTENT_DIR),
        }
    }

    pub fn persist(&self, address: &ChatAddress) -> Result<ChatDeleteIntent, String> {
        if let Some(intent) = self.load(address)? {
            return Ok(intent);
        }
        // Local control and Sync may receive the same deletion concurrently.
        std::fs::create_dir_all(&self.root)
            .map_err(|error| format!("create Chat Delete Intent directory: {error}"))?;
        let writer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join(".write.lock"))
            .map_err(|error| format!("open Chat Delete Intent writer: {error}"))?;
        writer
            .lock()
            .map_err(|error| format!("lock Chat Delete Intent writer: {error}"))?;
        if let Some(intent) = self.load(address)? {
            return Ok(intent);
        }
        let intent = ChatDeleteIntent {
            address: address.clone(),
            intent_id: uuid::Uuid::new_v4().to_string(),
            deleted_at: Utc::now(),
        };
        let value = serde_json::to_value(&intent)
            .map_err(|error| format!("serialize Chat Delete Intent: {error}"))?;
        rua_config::atomic_write_json(&self.path_for(address), &value)
            .map_err(|error| format!("persist Chat Delete Intent: {error}"))?;
        Ok(intent)
    }

    pub fn load(&self, address: &ChatAddress) -> Result<Option<ChatDeleteIntent>, String> {
        let path = self.path_for(address);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("read {}: {error}", path.display())),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("decode {}: {error}", path.display()))
    }

    fn path_for(&self, address: &ChatAddress) -> PathBuf {
        let digest = blake3::hash(address.identity_key().as_bytes());
        self.root.join(format!("{}.json", digest.to_hex()))
    }
}
