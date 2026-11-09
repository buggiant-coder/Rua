use std::fmt;

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum SkillSource {
    Builtin,
    #[default]
    Customized,
    Plugin(String),
}

impl SkillSource {
    pub fn as_str(&self) -> &str {
        match self {
            SkillSource::Builtin => "builtin",
            SkillSource::Customized => "customized",
            SkillSource::Plugin(id) => id.as_str(),
        }
    }

    pub fn display_string(&self) -> String {
        format!("{self}")
    }
}

impl fmt::Display for SkillSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkillSource::Builtin => f.write_str("builtin"),
            SkillSource::Customized => f.write_str("customized"),
            SkillSource::Plugin(id) => write!(f, "plugin:{id}"),
        }
    }
}

impl Serialize for SkillSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("{self}"))
    }
}

impl<'de> Deserialize<'de> for SkillSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "builtin" => Ok(SkillSource::Builtin),
            "customized" => Ok(SkillSource::Customized),
            other => {
                if let Some(plugin_id) = other.strip_prefix("plugin:") {
                    if plugin_id.is_empty() {
                        return Err(de::Error::custom("plugin source requires non-empty id"));
                    }
                    Ok(SkillSource::Plugin(plugin_id.to_string()))
                } else {
                    Err(de::Error::custom(format!("unknown skill source: {other}")))
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRequirements {
    #[serde(default)]
    pub require_bins: Vec<String>,
    #[serde(default)]
    pub require_envs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version_text: String,
    pub content: String,
    pub source: SkillSource,

    #[serde(default)]
    pub references: serde_json::Value,

    #[serde(default)]
    pub scripts: serde_json::Value,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub emoji: String,
}
