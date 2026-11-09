use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rua_cron::{ChatInfo, SessionCleanupStore};

pub struct ChatIndexSessionCleanupStore {
    workspace_dir: PathBuf,
}

impl ChatIndexSessionCleanupStore {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self { workspace_dir }
    }
}

#[async_trait]
impl SessionCleanupStore for ChatIndexSessionCleanupStore {
    async fn list_all_chats(&self) -> Result<Vec<ChatInfo>, String> {
        let workspace_dir = self.workspace_dir.clone();
        tokio::task::spawn_blocking(move || list_all_chat_index_entries(&workspace_dir))
            .await
            .map_err(|e| format!("chat index cleanup task join: {e}"))?
    }

    async fn delete_chats(&self, _ids: &[String]) -> Result<(), String> {
        Ok(())
    }
}

fn list_all_chat_index_entries(workspace_dir: &Path) -> Result<Vec<ChatInfo>, String> {
    rua_session::chat_index::list_all_sync(workspace_dir).map(|records| {
        records
            .into_iter()
            .map(|record| ChatInfo {
                id: record.id,
                updated_at: record.updated_at,
            })
            .collect()
    })
}
