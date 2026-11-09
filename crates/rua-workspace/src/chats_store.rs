use std::path::Path;
use std::sync::OnceLock;

use rua_session::chat_index::{self, ChatIndexRecord};
use tokio::sync::Mutex;

static CHATS_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub async fn ensure_chat_entry(
    workspace_dir: &Path,
    chat_id: &str,
    channel_id: &str,
    user_id: &str,
    sender_name: Option<&str>,
) -> Result<(), String> {
    let _guard = CHATS_WRITE_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    if chat_index::get(workspace_dir, chat_id).await?.is_some() {
        return Ok(());
    }

    let now = chrono::Utc::now();
    let name = match sender_name {
        Some(name) if !name.is_empty() => format!("{channel_id}: {name}"),
        _ => format!("{channel_id} chat"),
    };
    let user_id = match sender_name {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => user_id.to_string(),
    };
    chat_index::upsert(
        workspace_dir,
        ChatIndexRecord {
            id: chat_id.to_string(),
            name,
            user_id,
            channel: channel_id.to_string(),
            created_at: now,
            updated_at: now,
            meta: serde_json::json!({}),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
}
