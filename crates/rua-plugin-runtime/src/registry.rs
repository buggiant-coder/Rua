use std::collections::HashMap;
use std::time::Instant;

use tokio::sync::RwLock;

use crate::client::PluginClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatus {
    Starting,
    Running,
    Stopping,
    Stopped,
    Crashed,
    Disabled,
}

impl std::fmt::Display for PluginStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting => write!(f, "starting"),
            Self::Running => write!(f, "running"),
            Self::Stopping => write!(f, "stopping"),
            Self::Stopped => write!(f, "stopped"),
            Self::Crashed => write!(f, "crashed"),
            Self::Disabled => write!(f, "disabled"),
        }
    }
}

pub struct ManagedPlugin {
    pub client: PluginClient,
    pub status: PluginStatus,
    pub crash_count: u32,
    pub last_crash: Option<Instant>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PluginInfo {
    pub plugin_id: String,
    pub status: PluginStatus,
    pub crash_count: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum ActiveRegistryError {
    #[error("plugin registry is shutting down")]
    ShuttingDown,
    #[error("plugin '{id}' already running")]
    AlreadyRunning { id: String },
    #[error("plugin '{id}' not found in active registry")]
    NotFound { id: String },
}

pub struct ActivePluginRegistry {
    plugins: RwLock<HashMap<String, ManagedPlugin>>,
    shutdown: tokio_util::sync::CancellationToken,
    event_tx: tokio::sync::broadcast::Sender<PluginLifecycleEvent>,
}

#[derive(Debug, Clone)]
pub enum PluginLifecycleEvent {
    Started { plugin_id: String },
    Stopped { plugin_id: String },
    Crashed { plugin_id: String, reason: String },
    AutoDisabled { plugin_id: String, crash_count: u32 },
}

impl Default for ActivePluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivePluginRegistry {
    pub fn new() -> Self {
        let (event_tx, _) = tokio::sync::broadcast::channel(64);
        Self {
            plugins: RwLock::new(HashMap::new()),
            shutdown: tokio_util::sync::CancellationToken::new(),
            event_tx,
        }
    }

    /// Stop active requests before taking the registry write lock, then close every process owner.
    pub async fn shutdown_all(&self) {
        self.shutdown.cancel();
        let clients = std::mem::take(&mut *self.plugins.write().await);
        for (plugin_id, mut managed) in clients {
            if let Err(error) = managed.client.shutdown().await {
                tracing::warn!(%plugin_id, %error, "plugin shutdown failed");
            }
            let _ = self
                .event_tx
                .send(PluginLifecycleEvent::Stopped { plugin_id });
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PluginLifecycleEvent> {
        self.event_tx.subscribe()
    }

    pub async fn insert(
        &self,
        plugin_id: String,
        client: PluginClient,
    ) -> Result<(), ActiveRegistryError> {
        let mut map = self.plugins.write().await;
        if self.shutdown.is_cancelled() {
            return Err(ActiveRegistryError::ShuttingDown);
        }
        if map.contains_key(&plugin_id) {
            return Err(ActiveRegistryError::AlreadyRunning { id: plugin_id });
        }
        map.insert(
            plugin_id.clone(),
            ManagedPlugin {
                client,
                status: PluginStatus::Running,
                crash_count: 0,
                last_crash: None,
            },
        );
        let _ = self
            .event_tx
            .send(PluginLifecycleEvent::Started { plugin_id });
        Ok(())
    }

    pub async fn remove(&self, plugin_id: &str) -> Result<ManagedPlugin, ActiveRegistryError> {
        let mut map = self.plugins.write().await;
        map.remove(plugin_id)
            .ok_or_else(|| ActiveRegistryError::NotFound {
                id: plugin_id.to_string(),
            })
    }

    pub async fn set_status(&self, plugin_id: &str, status: PluginStatus) {
        let mut map = self.plugins.write().await;
        if let Some(entry) = map.get_mut(plugin_id) {
            entry.status = status;
        }
    }

    pub async fn record_crash(&self, plugin_id: &str) -> Option<u32> {
        let mut map = self.plugins.write().await;
        if let Some(entry) = map.get_mut(plugin_id) {
            entry.status = PluginStatus::Crashed;
            entry.crash_count += 1;
            entry.last_crash = Some(Instant::now());
            let count = entry.crash_count;
            let _ = self.event_tx.send(PluginLifecycleEvent::Crashed {
                plugin_id: plugin_id.to_string(),
                reason: format!("crash #{count}"),
            });
            Some(count)
        } else {
            None
        }
    }

    pub async fn get_crash_info(&self, plugin_id: &str) -> Option<(u32, Option<Instant>)> {
        let map = self.plugins.read().await;
        map.get(plugin_id).map(|e| (e.crash_count, e.last_crash))
    }

    pub async fn reset_crash_count(&self, plugin_id: &str) {
        let mut map = self.plugins.write().await;
        if let Some(entry) = map.get_mut(plugin_id) {
            entry.crash_count = 0;
            entry.last_crash = None;
        }
    }

    pub async fn list(&self) -> Vec<PluginInfo> {
        let map = self.plugins.read().await;
        let mut result: Vec<PluginInfo> = map
            .iter()
            .map(|(id, entry)| PluginInfo {
                plugin_id: id.clone(),
                status: entry.status,
                crash_count: entry.crash_count,
            })
            .collect();
        result.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
        result
    }

    pub async fn get_status(&self, plugin_id: &str) -> Option<PluginStatus> {
        let map = self.plugins.read().await;
        map.get(plugin_id).map(|e| e.status)
    }

    pub async fn contains(&self, plugin_id: &str) -> bool {
        let map = self.plugins.read().await;
        map.contains_key(plugin_id)
    }

    pub async fn len(&self) -> usize {
        let map = self.plugins.read().await;
        map.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    pub async fn with_client_mut<F, R>(
        &self,
        plugin_id: &str,
        f: F,
    ) -> Result<R, ActiveRegistryError>
    where
        F: FnOnce(&mut PluginClient) -> R,
    {
        let mut map = self.plugins.write().await;
        let entry = map
            .get_mut(plugin_id)
            .ok_or_else(|| ActiveRegistryError::NotFound {
                id: plugin_id.to_string(),
            })?;
        Ok(f(&mut entry.client))
    }

    pub async fn list_all_tools(&self) -> Vec<(String, mcp_manager::types::McpToolInfo)> {
        let map = self.plugins.read().await;
        let mut result = Vec::new();
        for (plugin_id, managed) in map.iter() {
            if managed.status != PluginStatus::Running {
                continue;
            }
            let tools = tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => return result,
                tools = managed.client.list_tools() => tools,
            };
            match tools {
                Ok(tools) => {
                    for tool in tools {
                        result.push((plugin_id.clone(), tool));
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        plugin_id = %plugin_id,
                        error = %e,
                        "failed to list tools from active plugin"
                    );
                }
            }
        }
        result
    }

    pub async fn call_plugin_tool(
        &self,
        plugin_id: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<mcp_manager::types::McpCallResult, crate::client::PluginClientError> {
        let map = self.plugins.read().await;
        let managed =
            map.get(plugin_id)
                .ok_or_else(|| crate::client::PluginClientError::Protocol {
                    message: format!("plugin '{plugin_id}' not found in active registry"),
                })?;
        tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => Err(crate::client::PluginClientError::NotRunning),
            result = managed.client.call_tool(tool_name, args) => result,
        }
    }

    pub async fn invoke_hook(
        &self,
        plugin_id: &str,
        params: crate::protocol::InvokeHookParams,
    ) -> Result<crate::protocol::InvokeHookResult, crate::client::PluginClientError> {
        let map = self.plugins.read().await;
        let managed =
            map.get(plugin_id)
                .ok_or_else(|| crate::client::PluginClientError::Protocol {
                    message: format!("plugin '{plugin_id}' not found in active registry"),
                })?;
        tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => Err(crate::client::PluginClientError::NotRunning),
            result = managed.client.invoke_hook(params) => result,
        }
    }

    pub(crate) async fn health_watch(
        &self,
        plugin_id: &str,
    ) -> Option<tokio::sync::watch::Receiver<crate::client::PluginHealth>> {
        let map = self.plugins.read().await;
        map.get(plugin_id).map(|e| e.client.health_watch())
    }
}
