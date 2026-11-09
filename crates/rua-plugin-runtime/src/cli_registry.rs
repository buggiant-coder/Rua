use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::client::{PluginClient, PluginClientConfig, PluginClientError};
use crate::manifest::PluginManifestError;
use crate::protocol::InvokeCliParams;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CliCommandSpec {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub args: Vec<CliArgSpec>,
    pub callback: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CliArgSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone)]
pub struct PluginCliCommand {
    pub plugin_id: String,
    pub spec: CliCommandSpec,
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("cli command '{cmd_name}' already registered for plugin '{plugin_id}'")]
    AlreadyRegistered { plugin_id: String, cmd_name: String },
    #[error("cli command '{cmd_name}' not found for plugin '{plugin_id}'")]
    NotFound { plugin_id: String, cmd_name: String },
    #[error("plugin '{plugin_id}' not found")]
    PluginNotFound { plugin_id: String },
    #[error("invalid cli spec: {0}")]
    InvalidSpec(#[from] serde_json::Error),
    #[error("cli invocation failed: {0}")]
    InvocationFailed(String),
    #[error("mutex poisoned")]
    Poisoned,
}

#[derive(Debug, thiserror::Error)]
pub enum CliDispatchError {
    #[error("plugin directory not found for '{0}'")]
    PluginDirNotFound(String),
    #[error("manifest error: {0}")]
    Manifest(#[from] PluginManifestError),
    #[error("plugin client error: {0}")]
    Client(#[from] PluginClientError),
}

pub struct CliRegistry {
    commands: Vec<PluginCliCommand>,
    plugin_dirs: HashMap<String, PathBuf>,
}

impl Default for CliRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CliRegistry {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            plugin_dirs: HashMap::new(),
        }
    }

    pub fn from_plugins_dir(plugins_dir: &Path) -> Self {
        let mut registry = Self::new();

        let entries = match std::fs::read_dir(plugins_dir) {
            Ok(e) => e,
            Err(_) => return registry,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("plugin.json");
            if !manifest_path.is_file() {
                continue;
            }
            let manifest = match crate::PluginManifest::from_file(&manifest_path) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(
                        path = %manifest_path.display(),
                        error = %e,
                        "skipping plugin with bad manifest"
                    );
                    continue;
                }
            };
            for cmd in &manifest.cli_commands {
                registry.commands.push(PluginCliCommand {
                    plugin_id: manifest.id.clone(),
                    spec: cmd.clone(),
                });
            }
            registry.plugin_dirs.insert(manifest.id.clone(), path);
        }

        registry
    }

    pub fn register(&mut self, cmd: PluginCliCommand, plugin_dir: PathBuf) {
        self.plugin_dirs
            .entry(cmd.plugin_id.clone())
            .or_insert(plugin_dir);
        self.commands.push(cmd);
    }

    pub fn unregister_plugin(&mut self, plugin_id: &str) {
        self.commands.retain(|c| c.plugin_id != plugin_id);
        self.plugin_dirs.remove(plugin_id);
    }

    pub fn commands(&self) -> &[PluginCliCommand] {
        &self.commands
    }

    pub fn find_command(&self, name: &str) -> Option<&PluginCliCommand> {
        self.commands.iter().find(|c| c.spec.name == name)
    }

    pub async fn dispatch(
        &self,
        pc: &PluginCliCommand,
        args: serde_json::Value,
        working_dir: &Path,
        secret_dir: &Path,
        cache_dir: &Path,
    ) -> Result<String, CliDispatchError> {
        let plugin_dir = self
            .plugin_dirs
            .get(&pc.plugin_id)
            .ok_or_else(|| CliDispatchError::PluginDirNotFound(pc.plugin_id.clone()))?;

        let manifest = crate::PluginManifest::from_file(&plugin_dir.join("plugin.json"))?;

        let mut config = PluginClientConfig::new(&pc.plugin_id, &manifest.entry_point);
        config.working_dir = working_dir.to_path_buf();
        config.secret_dir = secret_dir.to_path_buf();
        config.runtime_cache_dir = cache_dir.to_path_buf();

        let mut client = PluginClient::spawn(config).await?;

        let params = InvokeCliParams {
            plugin_id: pc.plugin_id.clone(),
            cmd_name: pc.spec.name.clone(),
            args,
        };

        let result = client.invoke_cli(params).await?;
        client.shutdown().await.ok();

        Ok(result.output)
    }
}
