use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub async_execution: bool,
    #[serde(default)]
    pub display_group: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolToggleRequest {
    #[serde(default)]
    pub enabled: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAsyncExecutionRequest {
    pub async_execution: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfigUpdateRequest {
    #[serde(default)]
    pub config: serde_json::Map<String, serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl From<rua_tool::config_store::ToolInfo> for ToolInfo {
    fn from(value: rua_tool::config_store::ToolInfo) -> Self {
        Self {
            name: value.name,
            description: value.description,
            enabled: value.enabled,
            async_execution: value.async_execution,
            display_group: value.display_group,
            extra: value.extra,
        }
    }
}
