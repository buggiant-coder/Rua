use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::types::{SkillRequirements, SkillSource};

pub const WORKSPACE_SCHEMA_VERSION: &str = "workspace-skill-manifest.v1";
pub const POOL_SCHEMA_VERSION: &str = "skill-pool-manifest.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version_text: String,

    #[serde(default)]
    pub commit_text: String,

    #[serde(default)]
    pub signature: String,
    pub source: SkillSource,
    #[serde(default)]
    pub protected: bool,
    #[serde(default)]
    pub requirements: SkillRequirements,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub emoji: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSkillEntry {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_channels")]
    pub channels: Vec<String>,
    pub source: SkillSource,
    #[serde(default)]
    pub config: serde_json::Map<String, serde_json::Value>,
    pub metadata: SkillMetadata,
    #[serde(default)]
    pub requirements: SkillRequirements,
    #[serde(default)]
    pub updated_at: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_channels() -> Vec<String> {
    vec!["all".to_string()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    #[serde(default = "default_workspace_schema_version")]
    pub schema_version: String,
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub skills: BTreeMap<String, WorkspaceSkillEntry>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_workspace_schema_version() -> String {
    WORKSPACE_SCHEMA_VERSION.to_string()
}

impl Default for WorkspaceManifest {
    fn default() -> Self {
        Self {
            schema_version: WORKSPACE_SCHEMA_VERSION.to_string(),
            version: 0,
            skills: BTreeMap::new(),
            extra: serde_json::Map::new(),
        }
    }
}

impl WorkspaceManifest {
    pub fn default_payload() -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert(
            "schema_version".to_string(),
            serde_json::Value::String(WORKSPACE_SCHEMA_VERSION.to_string()),
        );
        m.insert(
            "version".to_string(),
            serde_json::Value::Number(0u64.into()),
        );
        m.insert(
            "skills".to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
        m
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolSkillEntry {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version_text: String,
    #[serde(default)]
    pub commit_text: String,
    #[serde(default)]
    pub signature: String,
    pub source: SkillSource,
    #[serde(default)]
    pub protected: bool,
    #[serde(default)]
    pub requirements: SkillRequirements,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub emoji: String,

    #[serde(default)]
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolManifest {
    #[serde(default = "default_pool_schema_version")]
    pub schema_version: String,
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub builtin_skill_names: Vec<String>,
    #[serde(default)]
    pub skills: BTreeMap<String, PoolSkillEntry>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_pool_schema_version() -> String {
    POOL_SCHEMA_VERSION.to_string()
}

impl Default for PoolManifest {
    fn default() -> Self {
        Self {
            schema_version: POOL_SCHEMA_VERSION.to_string(),
            version: 0,
            builtin_skill_names: Vec::new(),
            skills: BTreeMap::new(),
            extra: serde_json::Map::new(),
        }
    }
}

impl PoolManifest {
    pub fn default_payload() -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        m.insert(
            "schema_version".to_string(),
            serde_json::Value::String(POOL_SCHEMA_VERSION.to_string()),
        );
        m.insert(
            "version".to_string(),
            serde_json::Value::Number(0u64.into()),
        );
        m.insert(
            "skills".to_string(),
            serde_json::Value::Object(serde_json::Map::new()),
        );
        m.insert(
            "builtin_skill_names".to_string(),
            serde_json::Value::Array(Vec::new()),
        );
        m
    }
}
