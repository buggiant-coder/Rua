#![forbid(unsafe_code)]

pub mod installer;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use cell_store::{CellStore, FileSystemCellStore};
use rua_core::{Result, RuaError};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

pub use installer::{
    plugin_pip_install_launch_plan, plugin_pip_install_launch_plan_with_authority, PluginInstaller,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginRecord {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct InstallRequest {
    pub plugin_id: String,
    pub version: Option<String>,
    pub source_url: Option<String>,
    pub extra: serde_json::Map<String, serde_json::Value>,
}

type CleanupFn = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;

pub struct PluginManager {
    plugins: RwLock<HashMap<String, PluginRecord>>,
    uninstall_hook: RwLock<Option<CleanupFn>>,
    persist_path: Option<PathBuf>,
    store: Arc<FileSystemCellStore>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
            uninstall_hook: RwLock::new(None),
            persist_path: None,
            store: Arc::new(FileSystemCellStore::new()),
        }
    }

    pub fn with_persistence(path: PathBuf) -> Result<Self> {
        Self::with_persistence_store(path, Arc::new(FileSystemCellStore::new()))
    }

    pub fn with_persistence_store(path: PathBuf, store: Arc<FileSystemCellStore>) -> Result<Self> {
        let plugins = if path.exists() {
            let data = std::fs::read_to_string(&path).map_err(|e| RuaError::Io {
                message: format!("failed to read plugin store {}: {}", path.display(), e),
            })?;
            serde_json::from_str::<HashMap<String, PluginRecord>>(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };

        Ok(Self {
            plugins: RwLock::new(plugins),
            uninstall_hook: RwLock::new(None),
            persist_path: Some(path),
            store,
        })
    }

    pub async fn set_uninstall_hook(&self, hook: CleanupFn) {
        *self.uninstall_hook.write().await = Some(hook);
    }

    pub async fn list(&self) -> Vec<PluginRecord> {
        let plugins = self.plugins.read().await;
        let mut list: Vec<PluginRecord> = plugins.values().cloned().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }

    pub async fn get(&self, plugin_id: &str) -> Option<PluginRecord> {
        let plugins = self.plugins.read().await;
        plugins.get(plugin_id).cloned()
    }

    pub async fn is_enabled(&self, plugin_id: &str) -> bool {
        let plugins = self.plugins.read().await;
        plugins.get(plugin_id).is_some_and(|r| r.enabled)
    }

    pub fn is_enabled_sync(&self, plugin_id: &str) -> bool {
        match self.plugins.try_read() {
            Ok(plugins) => plugins.get(plugin_id).is_some_and(|r| r.enabled),
            Err(_) => true,
        }
    }

    pub async fn install(&self, req: InstallRequest) -> Result<PluginRecord> {
        let mut plugins = self.plugins.write().await;
        if plugins.contains_key(&req.plugin_id) {
            return Err(RuaError::Conflict {
                resource: "plugin".into(),
                detail: format!("Plugin '{}' is already installed", req.plugin_id),
            });
        }

        let plugin = PluginRecord {
            id: req.plugin_id.clone(),
            name: req.plugin_id.clone(),
            version: req.version.unwrap_or_else(|| "0.1.0".into()),
            description: String::new(),
            enabled: true,
            installed: true,
            source_url: req.source_url,
            extra: req.extra,
        };

        let plugin_id = req.plugin_id;
        plugins.insert(plugin_id.clone(), plugin.clone());
        if let Err(e) = self.persist_snapshot(&plugins).await {
            plugins.remove(&plugin_id);
            return Err(e);
        }
        Ok(plugin)
    }

    pub async fn uninstall(&self, plugin_id: &str) -> Result<()> {
        let mut plugins = self.plugins.write().await;
        let snapshot = plugins
            .remove(plugin_id)
            .ok_or_else(|| RuaError::NotFound {
                resource: "plugin".into(),
                id: plugin_id.into(),
            })?;

        let hook = self.uninstall_hook.read().await.clone();
        if let Some(ref cleanup) = hook {
            if let Err(e) = cleanup(plugin_id) {
                plugins.insert(plugin_id.to_string(), snapshot);
                return Err(RuaError::Runtime {
                    message: format!(
                        "Plugin '{}' uninstall failed during cleanup, rolled back: {}",
                        plugin_id, e
                    ),
                });
            }
        }
        if let Err(e) = self.persist_snapshot(&plugins).await {
            plugins.insert(plugin_id.to_string(), snapshot);
            return Err(e);
        }
        Ok(())
    }

    pub async fn toggle(&self, plugin_id: &str, enabled: bool) -> Result<PluginRecord> {
        let mut plugins = self.plugins.write().await;
        let plugin = plugins
            .get_mut(plugin_id)
            .ok_or_else(|| RuaError::NotFound {
                resource: "plugin".into(),
                id: plugin_id.into(),
            })?;
        let previous = plugin.enabled;
        plugin.enabled = enabled;
        let plugin = plugin.clone();
        if let Err(e) = self.persist_snapshot(&plugins).await {
            if let Some(current) = plugins.get_mut(plugin_id) {
                current.enabled = previous;
            }
            return Err(e);
        }
        Ok(plugin)
    }

    async fn persist_snapshot(&self, plugins: &HashMap<String, PluginRecord>) -> Result<()> {
        if let Some(ref path) = self.persist_path {
            self.store
                .write(path, "$", plugins.clone())
                .await
                .map_err(|e| RuaError::Io {
                    message: format!("failed to write plugin store {}: {}", path.display(), e),
                })?;
        }
        Ok(())
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}
