#![forbid(unsafe_code)]

pub mod cli_registry;
pub mod client;
pub mod hook_registry;
pub mod loader;
pub mod manager;
pub mod manifest;
pub mod protocol;
pub mod registry;
pub mod skill_registry;
pub mod watcher;

pub use cli_registry::{
    CliArgSpec, CliCommandSpec, CliDispatchError, CliError, CliRegistry, PluginCliCommand,
};
pub use client::{PluginClient, PluginClientConfig, PluginClientError, PluginHealth};
pub use hook_registry::{
    DispatchOutcome, DispatchResult, HookError, HookPoint, HookRegistration, HookRegistry,
    HookResult, HookSpec,
};
pub use loader::{safe_extract_zip, Plugin, PluginLoadError, PluginLoader};
pub use manager::{LifecycleError, PluginLifecycleManager};
pub use manifest::{PluginManifest, PluginManifestError, PluginProviderDecl};
pub use registry::{
    ActivePluginRegistry, ActiveRegistryError, PluginInfo, PluginLifecycleEvent, PluginStatus,
};
pub use skill_registry::{
    PluginSkillRegistry, PluginSkillSpec, SkillError, SkillRegistrar, SkillRegistryError,
};
pub use watcher::{PluginWatcher, WatcherError};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub type EnabledChecker = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub parameters: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool '{tool_name}' already registered for plugin '{plugin_id}'")]
    AlreadyRegistered {
        plugin_id: String,
        tool_name: String,
    },
    #[error("tool '{tool_name}' not found for plugin '{plugin_id}'")]
    NotFound {
        plugin_id: String,
        tool_name: String,
    },
    #[error("plugin '{plugin_id}' not found")]
    PluginNotFound { plugin_id: String },
    #[error("invalid tool spec: {0}")]
    InvalidSpec(#[from] serde_json::Error),
    #[error("tool invocation failed: {0}")]
    InvocationFailed(String),
    #[error("mutex poisoned")]
    Poisoned,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub enum PluginEvent {
    Installed { plugin_id: String },
    Uninstalled { plugin_id: String },
}

#[derive(Clone)]
pub struct PluginRegistry {
    plugins: Arc<Mutex<HashMap<String, Plugin>>>,
    tools: Arc<Mutex<HashMap<String, HashMap<String, ToolSpec>>>>,
    hook_registry: Arc<HookRegistry>,
    skill_owners: Arc<Mutex<HashMap<String, Vec<String>>>>,
    cli_commands: Arc<Mutex<HashMap<String, HashMap<String, CliCommandSpec>>>>,
    enabled_checker: Arc<Mutex<Option<EnabledChecker>>>,
    event_tx: tokio::sync::broadcast::Sender<PluginEvent>,
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginRegistry {
    pub fn new() -> Self {
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        Self {
            plugins: Arc::new(Mutex::new(HashMap::new())),
            tools: Arc::new(Mutex::new(HashMap::new())),
            hook_registry: Arc::new(HookRegistry::new()),
            skill_owners: Arc::new(Mutex::new(HashMap::new())),
            cli_commands: Arc::new(Mutex::new(HashMap::new())),
            enabled_checker: Arc::new(Mutex::new(None)),
            event_tx,
        }
    }

    pub fn add(&self, plugin: Plugin) -> Result<(), RegistryError> {
        let id = plugin.id.clone();
        let mut map = self.plugins.lock().map_err(|_| RegistryError::Poisoned)?;
        if map.contains_key(&id) {
            return Err(RegistryError::AlreadyExists { id });
        }
        map.insert(id.clone(), plugin);
        drop(map);
        let _ = self.event_tx.send(PluginEvent::Installed { plugin_id: id });
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<Plugin, RegistryError> {
        let mut map = self.plugins.lock().map_err(|_| RegistryError::Poisoned)?;
        let plugin = map
            .remove(id)
            .ok_or_else(|| RegistryError::NotFound { id: id.to_string() })?;
        drop(map);
        let _ = self.event_tx.send(PluginEvent::Uninstalled {
            plugin_id: id.to_string(),
        });
        Ok(plugin)
    }

    pub fn get_manifest_providers(&self, id: &str) -> Vec<PluginProviderDecl> {
        self.plugins
            .lock()
            .ok()
            .and_then(|map| map.get(id).map(|p| p.manifest.providers.clone()))
            .unwrap_or_default()
    }

    pub fn list(&self) -> Result<Vec<String>, RegistryError> {
        let map = self.plugins.lock().map_err(|_| RegistryError::Poisoned)?;
        let mut ids: Vec<String> = map.keys().cloned().collect();
        ids.sort();
        Ok(ids)
    }

    pub fn len(&self) -> Result<usize, RegistryError> {
        let map = self.plugins.lock().map_err(|_| RegistryError::Poisoned)?;
        Ok(map.len())
    }

    pub fn is_empty(&self) -> Result<bool, RegistryError> {
        Ok(self.len()? == 0)
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PluginEvent> {
        self.event_tx.subscribe()
    }

    pub fn set_enabled_checker(&self, checker: EnabledChecker) {
        if let Ok(mut guard) = self.enabled_checker.lock() {
            *guard = Some(checker);
        }
    }

    pub fn is_plugin_enabled(&self, plugin_id: &str) -> bool {
        match self.enabled_checker.lock() {
            Ok(guard) => match guard.as_ref() {
                Some(checker) => checker(plugin_id),
                None => true,
            },
            Err(_) => true,
        }
    }

    pub fn register_tool(&self, plugin_id: &str, spec: ToolSpec) -> Result<(), ToolError> {
        let mut map = self.tools.lock().map_err(|_| ToolError::Poisoned)?;
        let plugin_tools = map.entry(plugin_id.to_string()).or_default();
        if plugin_tools.contains_key(&spec.name) {
            return Err(ToolError::AlreadyRegistered {
                plugin_id: plugin_id.to_string(),
                tool_name: spec.name,
            });
        }
        plugin_tools.insert(spec.name.clone(), spec);
        Ok(())
    }

    pub fn get_tool(&self, plugin_id: &str, tool_name: &str) -> Result<ToolSpec, ToolError> {
        let map = self.tools.lock().map_err(|_| ToolError::Poisoned)?;
        map.get(plugin_id)
            .and_then(|tools| tools.get(tool_name))
            .cloned()
            .ok_or_else(|| ToolError::NotFound {
                plugin_id: plugin_id.to_string(),
                tool_name: tool_name.to_string(),
            })
    }

    pub fn list_tools(&self, plugin_id: &str) -> Vec<ToolSpec> {
        self.tools
            .lock()
            .map(|map| {
                map.get(plugin_id)
                    .map(|tools| tools.values().cloned().collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    pub fn list_all_tools(&self) -> Vec<(String, ToolSpec)> {
        self.tools
            .lock()
            .map(|map| {
                map.iter()
                    .flat_map(|(plugin_id, tools)| {
                        tools
                            .values()
                            .map(move |spec| (plugin_id.clone(), spec.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn remove_plugin_tools(&self, plugin_id: &str) {
        if let Ok(mut map) = self.tools.lock() {
            map.remove(plugin_id);
        }
    }

    pub fn register_cli(&self, plugin_id: &str, spec: CliCommandSpec) -> Result<(), CliError> {
        let mut map = self.cli_commands.lock().map_err(|_| CliError::Poisoned)?;
        let plugin_cmds = map.entry(plugin_id.to_string()).or_default();
        if plugin_cmds.contains_key(&spec.name) {
            return Err(CliError::AlreadyRegistered {
                plugin_id: plugin_id.to_string(),
                cmd_name: spec.name,
            });
        }
        plugin_cmds.insert(spec.name.clone(), spec);
        Ok(())
    }

    pub fn get_cli_command(
        &self,
        plugin_id: &str,
        cmd_name: &str,
    ) -> Result<CliCommandSpec, CliError> {
        let map = self.cli_commands.lock().map_err(|_| CliError::Poisoned)?;
        map.get(plugin_id)
            .and_then(|cmds| cmds.get(cmd_name))
            .cloned()
            .ok_or_else(|| CliError::NotFound {
                plugin_id: plugin_id.to_string(),
                cmd_name: cmd_name.to_string(),
            })
    }

    pub fn list_cli_commands(&self, plugin_id: &str) -> Vec<CliCommandSpec> {
        self.cli_commands
            .lock()
            .map(|map| {
                map.get(plugin_id)
                    .map(|cmds| cmds.values().cloned().collect())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    pub fn list_all_cli_commands(&self) -> Vec<(String, CliCommandSpec)> {
        self.cli_commands
            .lock()
            .map(|map| {
                map.iter()
                    .flat_map(|(plugin_id, cmds)| {
                        cmds.values()
                            .map(move |spec| (plugin_id.clone(), spec.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn remove_plugin_cli_commands(&self, plugin_id: &str) {
        if let Ok(mut map) = self.cli_commands.lock() {
            map.remove(plugin_id);
        }
    }

    pub fn hooks(&self) -> &HookRegistry {
        &self.hook_registry
    }

    pub async fn register_hook(&self, plugin_id: &str, spec: HookSpec) -> Result<(), HookError> {
        self.hook_registry.register(plugin_id, spec).await?;
        Ok(())
    }

    pub async fn get_hooks_for_point(&self, point: HookPoint) -> Vec<(String, HookSpec)> {
        self.hook_registry
            .get_for_point(point)
            .await
            .into_iter()
            .map(|r| (r.plugin_id, r.spec))
            .collect()
    }

    pub async fn list_hooks(&self, plugin_id: &str) -> Vec<HookSpec> {
        self.hook_registry
            .list_for_plugin(plugin_id)
            .await
            .into_iter()
            .map(|r| r.spec)
            .collect()
    }

    pub async fn dispatch_hook(
        &self,
        point: HookPoint,
        _payload: &str,
    ) -> Result<Vec<HookResult>, HookError> {
        let result = self.dispatch_hook_with_abort(point, _payload).await?;
        Ok(result.results)
    }

    pub async fn dispatch_hook_with_abort(
        &self,
        point: HookPoint,
        _payload: &str,
    ) -> Result<DispatchResult, HookError> {
        let _entries = self.hook_registry.get_for_point(point).await;
        Ok(DispatchResult {
            results: Vec::new(),
            outcome: DispatchOutcome::Continue,
        })
    }

    pub fn track_skill(&self, plugin_id: &str, skill_name: &str) -> Result<(), SkillError> {
        let mut map = self.skill_owners.lock().map_err(|_| SkillError::Poisoned)?;
        let names = map.entry(plugin_id.to_string()).or_default();
        if names.iter().any(|n| n == skill_name) {
            return Err(SkillError::AlreadyRegistered {
                plugin_id: plugin_id.to_string(),
                skill_name: skill_name.to_string(),
            });
        }
        names.push(skill_name.to_string());
        Ok(())
    }

    pub fn untrack_plugin_skills(&self, plugin_id: &str) -> Vec<String> {
        self.skill_owners
            .lock()
            .ok()
            .and_then(|mut map| map.remove(plugin_id))
            .unwrap_or_default()
    }

    pub fn tracked_skills(&self, plugin_id: &str) -> Vec<String> {
        self.skill_owners
            .lock()
            .map(|map| map.get(plugin_id).cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    pub async fn remove_plugin_hooks(&self, plugin_id: &str) {
        self.hook_registry.remove_by_plugin(plugin_id).await;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("plugin '{id}' already registered")]
    AlreadyExists { id: String },
    #[error("plugin '{id}' not found")]
    NotFound { id: String },
    #[error("registry mutex poisoned")]
    Poisoned,
}
