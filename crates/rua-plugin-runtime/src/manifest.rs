use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cli_registry::CliCommandSpec;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginProviderDecl {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub chat_model: Option<String>,
    #[serde(default)]
    pub preset_models: Vec<String>,
    #[serde(default)]
    pub require_api_key: bool,
    #[serde(default)]
    pub meta: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_entry_point")]
    pub entry_point: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default = "default_min_rua_version")]
    pub min_rua_version: String,
    #[serde(default)]
    pub meta: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub cli_commands: Vec<CliCommandSpec>,
    #[serde(default)]
    pub providers: Vec<PluginProviderDecl>,
}

fn default_entry_point() -> String {
    "main".into()
}

fn default_min_rua_version() -> String {
    "0.1.0".into()
}

impl PluginManifest {
    pub fn from_json(data: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(data)
    }

    pub fn from_file(path: &Path) -> Result<Self, PluginManifestError> {
        let data = std::fs::read_to_string(path).map_err(|e| PluginManifestError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        Self::from_json(&data).map_err(|e| PluginManifestError::Parse {
            path: path.display().to_string(),
            source: e,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginManifestError {
    #[error("failed to read manifest at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse manifest at {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}
