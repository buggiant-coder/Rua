use std::path::Path;

pub use rua_session::chat_index::{
    ChatIndexFilter, ChatIndexPage, ChatIndexRecord as ChatIndexEntry,
};

pub async fn list(
    workspace_dir: &Path,
    filter: ChatIndexFilter,
    offset: u64,
    limit: u64,
) -> Result<ChatIndexPage, String> {
    rua_session::chat_index::list(workspace_dir, filter, offset, limit).await
}

pub async fn get(workspace_dir: &Path, chat_id: &str) -> Result<Option<ChatIndexEntry>, String> {
    rua_session::chat_index::get(workspace_dir, chat_id).await
}

pub async fn list_all(workspace_dir: &Path) -> Result<Vec<ChatIndexEntry>, String> {
    rua_session::chat_index::list_all(workspace_dir).await
}

pub async fn upsert(workspace_dir: &Path, entry: ChatIndexEntry) -> Result<(), String> {
    rua_session::chat_index::upsert(workspace_dir, entry).await
}

pub async fn insert(workspace_dir: &Path, entry: ChatIndexEntry) -> Result<(), String> {
    rua_session::chat_index::insert(workspace_dir, entry).await
}

pub async fn delete(workspace_dir: &Path, chat_id: &str) -> Result<(), String> {
    rua_session::chat_index::delete(workspace_dir, chat_id).await
}

pub async fn delete_many(workspace_dir: &Path, chat_ids: Vec<String>) -> Result<(), String> {
    rua_session::chat_index::delete_many(workspace_dir, chat_ids).await
}

pub async fn update_pin_order(
    workspace_dir: &Path,
    items: Vec<(String, i64)>,
) -> Result<(), String> {
    rua_session::chat_index::update_pin_order(workspace_dir, items).await
}
