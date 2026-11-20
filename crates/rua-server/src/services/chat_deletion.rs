use std::path::{Path, PathBuf};

use rua_core::{ChatAddress, ChatStore};
pub use rua_session::chat_deletion::{ChatDeleteIntent, ChatDeleteIntentStore};
use rua_session::WorkspaceSessionStore;

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
        let scope_root = ChatStore::resolve(rua_root.as_ref(), address.clone())
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
