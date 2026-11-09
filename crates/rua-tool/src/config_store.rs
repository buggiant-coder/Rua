use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use rua_config::types::tools::{default_builtin_tools, BuiltinToolConfig};

use crate::agent_tools_cells::AgentToolsCells;
use crate::tool_config_secret_io;

#[derive(Debug, thiserror::Error)]
pub enum ToolConfigError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("config parse error at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("config error: {0}")]
    Config(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub async_execution: bool,
    /// UI grouping key resolved from the catalog. Empty string when the tool
    /// has no explicit group and the console falls back to the `other` bucket.
    #[serde(default)]
    pub display_group: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone)]
pub struct ToolConfigStore {
    cells: Arc<AgentToolsCells>,
    device_key: Option<[u8; 32]>,
    catalog: serde_json::Map<String, serde_json::Value>,
}

impl ToolConfigStore {
    pub fn new(cells: Arc<AgentToolsCells>) -> Self {
        Self {
            cells,
            device_key: None,
            catalog: default_builtin_tools(),
        }
    }

    pub fn with_device_key(mut self, device_key: [u8; 32]) -> Self {
        self.device_key = Some(device_key);
        self
    }

    pub fn with_catalog(mut self, catalog: serde_json::Map<String, serde_json::Value>) -> Self {
        self.catalog = catalog;
        self
    }

    fn device_key(&self) -> Option<&[u8; 32]> {
        self.device_key.as_ref()
    }

    fn decrypt_config_value(
        &self,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolConfigError> {
        tool_config_secret_io::decrypt_config_value(value, self.device_key()).map_err(|e| {
            ToolConfigError::Config(format!("failed to decrypt tool config secret fields: {e}"))
        })
    }

    fn encrypt_config_value(
        &self,
        agent_id: &str,
        tool_name: &str,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolConfigError> {
        let scope = format!("agent:{agent_id}:tools:{tool_name}:config");
        tool_config_secret_io::encrypt_config_value(value, self.device_key(), scope).map_err(|e| {
            ToolConfigError::Config(format!("failed to encrypt tool config secret fields: {e}"))
        })
    }

    fn agent_id_or_default(agent_id: Option<&str>) -> &str {
        agent_id.unwrap_or("default")
    }

    async fn read_agent_tools(
        &self,
        agent_id: Option<&str>,
    ) -> Result<Option<serde_json::Value>, ToolConfigError> {
        self.cells
            .cell(Self::agent_id_or_default(agent_id))
            .read_value()
            .await
    }

    fn extract_builtin_tools(
        &self,
        value: &serde_json::Value,
    ) -> Result<serde_json::Map<String, serde_json::Value>, ToolConfigError> {
        let section = value
            .as_object()
            .ok_or_else(|| ToolConfigError::Config("tools must be an object".into()))?;
        let Some(builtin) = section.get("builtin_tools") else {
            return Ok(Default::default());
        };
        let builtin = builtin
            .as_object()
            .ok_or_else(|| ToolConfigError::Config("builtin_tools must be an object".into()))?;
        Ok(builtin
            .iter()
            .filter(|(name, _)| self.catalog.contains_key(*name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect())
    }

    fn map_to_dto_list(
        &self,
        builtin_tools: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Vec<ToolInfo>, ToolConfigError> {
        let mut tools = builtin_tools
            .iter()
            .map(|(name, value)| {
                let cfg: BuiltinToolConfig = serde_json::from_value(value.clone())
                    .map_err(|e| ToolConfigError::Config(format!("invalid tool {name}: {e}")))?;
                let definition = &self.catalog[name];
                Ok(ToolInfo {
                    name: name.clone(),
                    enabled: cfg.enabled,
                    description: definition
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&cfg.description)
                        .to_string(),
                    async_execution: cfg.async_execution,
                    display_group: definition
                        .get("display_group")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    extra: cfg.extra,
                })
            })
            .collect::<Result<Vec<_>, ToolConfigError>>()?;
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(tools)
    }

    pub async fn list_tools(
        &self,
        agent_id: Option<&str>,
    ) -> Result<Vec<ToolInfo>, ToolConfigError> {
        let tools_value = self.read_agent_tools(agent_id).await?;
        let builtin_tools = match tools_value {
            Some(ref v) => self.extract_builtin_tools(v)?,
            None => Default::default(),
        };
        self.map_to_dto_list(&builtin_tools)
    }

    pub async fn get_config(
        &self,
        agent_id: Option<&str>,
        tool_name: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>, ToolConfigError> {
        let tools_value = self.read_agent_tools(agent_id).await?;
        let builtin_tools = match tools_value {
            Some(ref v) => self.extract_builtin_tools(v)?,
            None => Default::default(),
        };
        let Some(entry) = builtin_tools.get(tool_name) else {
            return Err(ToolConfigError::NotFound(tool_name.to_string()));
        };

        match entry.get("config") {
            Some(serde_json::Value::Object(_)) => {
                let mut decrypted = self.decrypt_config_value(&entry["config"])?;
                tool_config_secret_io::mask_config_secrets(&mut decrypted);
                decrypted.as_object().cloned().ok_or_else(|| {
                    ToolConfigError::Config(format!("tool '{tool_name}' config must be an object"))
                })
            }
            Some(serde_json::Value::Null) | None => Ok(serde_json::Map::new()),
            Some(other) => Err(ToolConfigError::Config(format!(
                "tool '{tool_name}' config must be an object, got {other}"
            ))),
        }
    }

    pub async fn set_config(
        &self,
        agent_id: Option<&str>,
        tool_name: &str,
        mut config: serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Map<String, serde_json::Value>, ToolConfigError> {
        let id = Self::agent_id_or_default(agent_id);

        let current_config = self.get_config_unmasked(Some(id), tool_name).await?;
        let mut incoming_value = serde_json::Value::Object(config);
        let existing_value = serde_json::Value::Object(current_config);
        tool_config_secret_io::restore_masked_config_values(&mut incoming_value, &existing_value);

        let encrypted_config = self.encrypt_config_value(id, tool_name, &incoming_value)?;

        let cell = self.cells.cell(id);
        cell.update(|current| {
            let mut tools_value = current.unwrap_or_else(|| serde_json::json!({}));
            if let Some(map) = tools_value
                .as_object_mut()
                .and_then(|m| m.get_mut("builtin_tools"))
                .and_then(|bt| bt.as_object_mut())
                .and_then(|bt| bt.get_mut(tool_name))
                .and_then(|e| e.as_object_mut())
            {
                map.insert("config".to_string(), encrypted_config);
            }
            tools_value
        })
        .await?;

        config = self.get_config_unmasked(Some(id), tool_name).await?;
        let mut masked = serde_json::Value::Object(config);
        tool_config_secret_io::mask_config_secrets(&mut masked);
        Ok(masked.as_object().cloned().unwrap_or_default())
    }

    pub async fn get_config_unmasked(
        &self,
        agent_id: Option<&str>,
        tool_name: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>, ToolConfigError> {
        let tools_value = self.read_agent_tools(agent_id).await?;
        let builtin_tools = match tools_value {
            Some(ref v) => self.extract_builtin_tools(v)?,
            None => Default::default(),
        };
        let Some(entry) = builtin_tools.get(tool_name) else {
            return Err(ToolConfigError::NotFound(tool_name.to_string()));
        };

        match entry.get("config") {
            Some(serde_json::Value::Object(_)) => self
                .decrypt_config_value(&entry["config"])?
                .as_object()
                .cloned()
                .ok_or_else(|| {
                    ToolConfigError::Config(format!("tool '{tool_name}' config must be an object"))
                }),
            Some(serde_json::Value::Null) | None => Ok(serde_json::Map::new()),
            Some(other) => Err(ToolConfigError::Config(format!(
                "tool '{tool_name}' config must be an object, got {other}"
            ))),
        }
    }

    pub async fn list_configs_unmasked(
        &self,
        agent_id: Option<&str>,
    ) -> Result<serde_json::Map<String, serde_json::Value>, ToolConfigError> {
        let tools_value = self.read_agent_tools(agent_id).await?;
        let builtin_tools = match tools_value {
            Some(ref v) => self.extract_builtin_tools(v)?,
            None => Default::default(),
        };

        let mut configs = serde_json::Map::new();
        for (tool_name, entry) in builtin_tools {
            let Some(config_value) = entry.get("config") else {
                continue;
            };
            if config_value.is_null() {
                continue;
            }
            if !config_value.is_object() {
                return Err(ToolConfigError::Config(format!(
                    "tool '{tool_name}' config must be an object, got {config_value}"
                )));
            }

            let decrypted = self.decrypt_config_value(config_value)?;
            let Some(config) = decrypted.as_object() else {
                return Err(ToolConfigError::Config(format!(
                    "tool '{tool_name}' config must be an object"
                )));
            };
            if !config.is_empty() {
                configs.insert(tool_name, serde_json::Value::Object(config.clone()));
            }
        }

        Ok(configs)
    }

    pub async fn set_enabled(
        &self,
        agent_id: Option<&str>,
        tool_name: &str,
        enabled: bool,
    ) -> Result<ToolInfo, ToolConfigError> {
        let id = Self::agent_id_or_default(agent_id);
        self.get_config(Some(id), tool_name).await?;

        let cell = self.cells.cell(id);
        let new_value = cell
            .update(|current| {
                let mut tools_value = current.unwrap_or_else(|| serde_json::json!({}));

                if let Some(map) = tools_value
                    .as_object_mut()
                    .and_then(|m| m.get_mut("builtin_tools"))
                    .and_then(|bt| bt.as_object_mut())
                    .and_then(|bt| bt.get_mut(tool_name))
                    .and_then(|e| e.as_object_mut())
                {
                    map.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
                }
                tools_value
            })
            .await?;

        let builtin_tools = self.extract_builtin_tools(&new_value)?;
        let canonical_group = self
            .catalog
            .get(tool_name)
            .and_then(|entry| entry.get("display_group"))
            .and_then(|group| group.as_str())
            .unwrap_or_default()
            .to_string();
        builtin_tools
            .get(tool_name)
            .ok_or_else(|| ToolConfigError::NotFound(tool_name.to_string()))
            .and_then(|entry| {
                serde_json::from_value::<BuiltinToolConfig>(entry.clone())
                    .map(|cfg| ToolInfo {
                        name: cfg.name,
                        description: cfg.description,
                        enabled: cfg.enabled,
                        async_execution: cfg.async_execution,
                        display_group: canonical_group.clone(),
                        extra: cfg.extra,
                    })
                    .map_err(|e| {
                        ToolConfigError::Config(format!("Failed to parse updated tool: {e}"))
                    })
            })
    }

    pub async fn set_async_execution(
        &self,
        agent_id: Option<&str>,
        tool_name: &str,
        async_execution: bool,
    ) -> Result<ToolInfo, ToolConfigError> {
        let id = Self::agent_id_or_default(agent_id);
        self.get_config(Some(id), tool_name).await?;

        let cell = self.cells.cell(id);
        let new_value = cell
            .update(|current| {
                let mut tools_value = current.unwrap_or_else(|| serde_json::json!({}));
                if let Some(map) = tools_value
                    .as_object_mut()
                    .and_then(|m| m.get_mut("builtin_tools"))
                    .and_then(|bt| bt.as_object_mut())
                    .and_then(|bt| bt.get_mut(tool_name))
                    .and_then(|e| e.as_object_mut())
                {
                    map.insert(
                        "async_execution".to_string(),
                        serde_json::Value::Bool(async_execution),
                    );
                }
                tools_value
            })
            .await?;

        let builtin_tools = self.extract_builtin_tools(&new_value)?;
        let canonical_group = self
            .catalog
            .get(tool_name)
            .and_then(|entry| entry.get("display_group"))
            .and_then(|group| group.as_str())
            .unwrap_or_default()
            .to_string();
        builtin_tools
            .get(tool_name)
            .ok_or_else(|| ToolConfigError::NotFound(tool_name.to_string()))
            .and_then(|entry| {
                serde_json::from_value::<BuiltinToolConfig>(entry.clone())
                    .map(|cfg| ToolInfo {
                        name: cfg.name,
                        description: cfg.description,
                        enabled: cfg.enabled,
                        async_execution: cfg.async_execution,
                        display_group: canonical_group.clone(),
                        extra: cfg.extra,
                    })
                    .map_err(|e| {
                        ToolConfigError::Config(format!("Failed to parse updated tool: {e}"))
                    })
            })
    }
}
