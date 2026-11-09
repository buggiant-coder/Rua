use crate::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplatesFile {
    #[serde(
        default = "default_version",
        deserialize_with = "deserialize_null_default"
    )]
    pub version: u64,

    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub templates: HashMap<String, AgentTemplate>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_version() -> u64 {
    1
}

impl Default for AgentTemplatesFile {
    fn default() -> Self {
        Self {
            version: 1,
            templates: HashMap::new(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentTemplate {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub description: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub prompt_files: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub builtin: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub created_at: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub updated_at: String,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTemplatesFile {
    #[serde(
        default = "default_version_sub",
        deserialize_with = "deserialize_null_default"
    )]
    pub version: u64,

    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub templates: Vec<serde_json::Value>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_version_sub() -> u64 {
    1
}

impl Default for SubagentTemplatesFile {
    fn default() -> Self {
        Self {
            version: 1,
            templates: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }
}
