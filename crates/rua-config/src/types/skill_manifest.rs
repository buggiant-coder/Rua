use crate::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifestFile {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub schema_version: String,

    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub version: u64,

    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub skills: HashMap<String, SkillEntry>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for SkillManifestFile {
    fn default() -> Self {
        Self {
            schema_version: String::new(),
            version: 0,
            skills: HashMap::new(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillEntry {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub description: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub version_text: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub protected: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub channels: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub source: String,
    #[serde(default)]
    pub metadata: Option<SkillMetadata>,
    #[serde(default)]
    pub requirements: Option<SkillRequirements>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub updated_at: String,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillMetadata {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub description: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub version_text: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub commit_text: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub signature: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub source: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub protected: bool,
    #[serde(default)]
    pub requirements: Option<SkillRequirements>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub updated_at: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub tags: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub emoji: String,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillRequirements {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub require_bins: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub require_envs: Vec<String>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
