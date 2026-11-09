use serde::{Deserialize, Serialize};

use crate::error::AgentError;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelSlotRef {
    #[serde(default)]
    pub provider_id: String,

    #[serde(default)]
    pub model: String,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub id: String,

    pub name: String,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(default)]
    pub system_prompt: String,

    #[serde(default)]
    pub model_slot: Option<ModelSlotRef>,

    #[serde(default)]
    pub enabled_tools: Vec<String>,

    #[serde(default)]
    pub enabled_mcp_servers: Vec<String>,

    #[serde(default)]
    pub workspace_dir: String,

    #[serde(default)]
    pub language: String,

    #[serde(default)]
    pub system_prompt_files: Vec<String>,

    #[serde(default)]
    pub template_id: Option<String>,

    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub metadata: serde_json::Value,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl AgentProfile {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        let id = id.into();
        let name = name.into();
        Self {
            id,
            name,
            description: None,
            system_prompt: String::new(),
            model_slot: None,
            enabled_tools: Vec::new(),
            enabled_mcp_servers: Vec::new(),
            workspace_dir: String::new(),
            language: String::new(),
            system_prompt_files: Vec::new(),
            template_id: None,
            enabled: true,
            metadata: serde_json::Value::Null,
            extra: serde_json::Map::new(),
        }
    }

    pub fn validate(&self) -> Result<(), AgentError> {
        if self.id.is_empty() {
            return Err(AgentError::config("Agent profile ID cannot be empty"));
        }
        if self.name.is_empty() {
            return Err(AgentError::config("Agent profile name cannot be empty"));
        }
        Ok(())
    }

    pub fn from_config(config: &rua_config::types::AgentProfileConfig) -> Self {
        let enabled_tools = config
            .tools
            .as_ref()
            .and_then(|tools| tools.get("builtin_tools"))
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flat_map(|tools| tools.iter())
            .filter(|(_, value)| {
                value
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true)
            })
            .map(|(name, _)| name.clone())
            .collect();
        let enabled_mcp_servers = config
            .mcp
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .into_iter()
            .flat_map(|servers| servers.iter())
            .filter(|(_, value)| {
                value
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true)
            })
            .map(|(name, _)| name.clone())
            .collect();
        let mut extra = config.extra.clone();
        if let Some(tools) = config.tools.clone() {
            extra.insert("tools".to_string(), tools);
        }
        if let Some(mcp) = config.mcp.clone() {
            extra.insert("mcp".to_string(), mcp);
        }
        Self {
            id: config.id.clone(),
            name: config.name.clone(),
            description: if config.description.is_empty() {
                None
            } else {
                Some(config.description.clone())
            },
            system_prompt: String::new(),
            model_slot: config.default_model.as_ref().map(|m| ModelSlotRef {
                provider_id: m.provider_id.clone(),
                model: m.model.clone(),
                extra: m.extra.clone(),
            }),
            enabled_tools,
            enabled_mcp_servers,
            workspace_dir: config.workspace_dir.clone(),
            language: config.language.clone(),
            system_prompt_files: config.system_prompt_files.clone(),
            template_id: config.template_id.clone(),
            enabled: true,
            metadata: serde_json::Value::Null,
            extra,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    pub url: String,

    pub name: String,

    #[serde(default)]
    pub mime_type: Option<String>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct SecurityContext {
    pub user_id: String,

    pub channel: String,

    pub interactive: bool,
}
