use std::sync::Arc;

use async_trait::async_trait;
use rua_contract::agent::SessionReplicationHook;

use crate::services::console_chat::SessionSyncEmitter;

pub struct SyncSessionReplicationHook {
    emitter: SessionSyncEmitter,
}

impl SyncSessionReplicationHook {
    pub fn new(emitter: SessionSyncEmitter) -> Arc<Self> {
        Arc::new(Self { emitter })
    }
}

#[async_trait]
impl SessionReplicationHook for SyncSessionReplicationHook {
    async fn on_session_state_changed(
        &self,
        chat_address: &rua_core::ChatAddress,
        state: &serde_json::Value,
    ) {
        if state.get("kind").and_then(serde_json::Value::as_str)
            != Some("summary_checkpoint_published")
        {
            return;
        }
        let Some(workspace_dir) = state
            .get("workspace_dir")
            .and_then(serde_json::Value::as_str)
            .map(std::path::PathBuf::from)
        else {
            return;
        };
        self.emitter
            .emit_evergreen_chat_snapshot(&workspace_dir, chat_address)
            .await;
    }
}
