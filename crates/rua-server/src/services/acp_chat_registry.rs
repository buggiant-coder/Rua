use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub fn load_acp_config(workspace_dir: &std::path::Path) -> rua_config::types::AcpConfig {
    let agent_json = workspace_dir.join("agent.json");
    let mut config: rua_config::types::AcpConfig = std::fs::read_to_string(agent_json)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|root| root.get("acp").cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    config.backfill_builtin_runners();
    config
}

pub struct AcpChatRuntimeHandle {
    pub service: Arc<rua_acp::service::AcpService>,
    pub manager: Arc<rua_command::AcpChatSessionManager>,
    pub controller: Arc<dyn rua_command::AcpChatController>,
    pub workspace_dir: PathBuf,
}

impl AcpChatRuntimeHandle {
    pub fn refresh_config(&self) {
        self.service
            .replace_config(Arc::new(load_acp_config(&self.workspace_dir)));
    }
}

#[derive(Default)]
pub struct AcpChatRegistry {
    inner: RwLock<HashMap<String, Arc<AcpChatRuntimeHandle>>>,
}

impl AcpChatRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, agent_id: String, handle: Arc<AcpChatRuntimeHandle>) {
        let mut guard = self
            .inner
            .write()
            .expect("acp_chat_registry write lock poisoned");
        guard.insert(agent_id, handle);
    }

    pub fn get(&self, agent_id: &str) -> Option<Arc<AcpChatRuntimeHandle>> {
        let guard = self
            .inner
            .read()
            .expect("acp_chat_registry read lock poisoned");
        guard.get(agent_id).cloned()
    }

    pub async fn close_all(&self) {
        let handles: Vec<_> = self
            .inner
            .read()
            .expect("acp_chat_registry read lock poisoned")
            .values()
            .cloned()
            .collect();
        for handle in handles {
            handle.service.close_all().await;
        }
    }

    pub fn remove(&self, agent_id: &str) -> Option<Arc<AcpChatRuntimeHandle>> {
        let mut guard = self
            .inner
            .write()
            .expect("acp_chat_registry write lock poisoned");
        guard.remove(agent_id)
    }
}
