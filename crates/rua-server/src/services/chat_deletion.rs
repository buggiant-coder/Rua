use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rua_core::chat::ChatAddress;
use rua_core::ChatScopeStore;
use rua_session::WorkspaceSessionStore;
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

#[derive(Debug, Clone)]
pub struct ChatDeletionCoordinator {
    intents: ChatDeleteIntentStore,
}

#[derive(Debug)]
pub struct LocalChatDeletionPlan {
    address: ChatAddress,
    scope_root: PathBuf,
    references: crate::services::chat_storage::ChatReferences,
}

impl LocalChatDeletionPlan {
    pub fn address(&self) -> &ChatAddress {
        &self.address
    }

    pub async fn prepare(rua_root: impl AsRef<Path>, address: ChatAddress) -> Result<Self, String> {
        let scope_root = ChatScopeStore::resolve(rua_root.as_ref(), address.clone())
            .root()
            .to_path_buf();
        let store = WorkspaceSessionStore::new(&scope_root);
        let record =
            crate::services::chat_index::get(&scope_root, address.chat_id().as_str()).await?;
        let references = match record.as_ref() {
            Some(record) => {
                crate::services::chat_storage::collect_chat_references(&store, record).await?
            }
            None => Default::default(),
        };
        Ok(Self {
            address,
            scope_root,
            references,
        })
    }

    pub async fn apply(self) -> Result<(), String> {
        crate::services::chat_storage::remove_tool_archives(&self.scope_root, &self.references)?;
        let store = WorkspaceSessionStore::new(&self.scope_root);
        store
            .delete_chat_storage_by_chat_id(self.address.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?;
        crate::services::chat_index::delete(&self.scope_root, self.address.chat_id().as_str())
            .await?;
        Ok(())
    }
}

impl ChatDeletionCoordinator {
    pub fn from_rua_root(rua_root: impl AsRef<Path>) -> Self {
        Self {
            intents: ChatDeleteIntentStore::from_rua_root(rua_root),
        }
    }

    pub fn commit_intent(&self, address: &ChatAddress) -> Result<ChatDeleteIntent, String> {
        self.intents.persist(address)
    }
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
