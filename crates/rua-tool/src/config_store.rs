use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use rua_config::types::tools::{
    default_builtin_tools, BuiltinToolConfig, ToolIdentity, ToolSelectionPolicy,
};
use sha2::{Digest, Sha256};

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
    #[error("configuration changed: {0}")]
    Conflict(String),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSelectionPolicyPreview {
    pub expected: Option<serde_json::Value>,
    pub current: ToolSelectionPolicy,
    pub candidate: ToolSelectionPolicy,
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

    fn policy_from_value(
        value: Option<&serde_json::Value>,
    ) -> Result<ToolSelectionPolicy, ToolConfigError> {
        let value = value
            .and_then(|value| value.get("selection_policy"))
            .ok_or_else(|| ToolConfigError::Config("tools.selection_policy is required".into()))?;
        let policy: ToolSelectionPolicy =
            serde_json::from_value(value.clone()).map_err(|error| {
                ToolConfigError::Config(format!("Invalid Tool selection policy: {error}"))
            })?;
        policy.validate().map_err(ToolConfigError::Config)?;
        Ok(policy)
    }

    fn revision(value: Option<&serde_json::Value>) -> Option<serde_json::Value> {
        value.map(|value| {
            serde_json::Value::String(format!(
                "sha256:{:x}",
                Sha256::digest(value.to_string().as_bytes())
            ))
        })
    }

    fn map_to_dto_list(
        &self,
        builtin_tools: &serde_json::Map<String, serde_json::Value>,
        policy: &ToolSelectionPolicy,
    ) -> Result<Vec<ToolInfo>, ToolConfigError> {
        let mut tools = self
            .catalog
            .iter()
            .map(|(name, definition)| {
                let configured = builtin_tools.get(name);
                let value = configured.unwrap_or(definition);
                let cfg: BuiltinToolConfig =
                    serde_json::from_value(value.clone()).map_err(|error| {
                        ToolConfigError::Config(format!("invalid tool {name}: {error}"))
                    })?;
                let identity = ToolIdentity::builtin(name);
                let capability = definition
                    .get("capability")
                    .and_then(|value| value.as_str());
                let enabled = policy.selects(&identity, capability);
                let mut extra = cfg.extra;
                extra.remove("enabled");
                extra.remove("capability");
                extra.remove("capability_enabled");
                extra.insert(
                    "configured".into(),
                    serde_json::Value::Bool(configured.is_some()),
                );
                extra.insert(
                    "identity".into(),
                    serde_json::to_value(&identity).expect("Tool identity serializes"),
                );
                if let Some(capability) = capability {
                    extra.insert(
                        "capability".into(),
                        serde_json::Value::String(capability.into()),
                    );
                    let capability_enabled = match policy {
                        ToolSelectionPolicy::DefaultEnabled {
                            disabled_capabilities,
                            ..
                        } => !disabled_capabilities
                            .iter()
                            .any(|disabled| disabled == capability),
                        _ => true,
                    };
                    extra.insert("capability_enabled".into(), capability_enabled.into());
                }
                Ok(ToolInfo {
                    name: name.clone(),
                    enabled,
                    description: definition
                        .get("description")
                        .and_then(|value| value.as_str())
                        .unwrap_or(&cfg.description)
                        .to_string(),
                    async_execution: cfg.async_execution,
                    display_group: definition
                        .get("display_group")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    extra,
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
        let builtin_tools = match tools_value.as_ref() {
            Some(value) => self.extract_builtin_tools(value)?,
            None => Default::default(),
        };
        let policy = Self::policy_from_value(tools_value.as_ref())?;
        self.map_to_dto_list(&builtin_tools, &policy)
    }

    pub async fn selection_policy(
        &self,
        agent_id: Option<&str>,
    ) -> Result<ToolSelectionPolicy, ToolConfigError> {
        Self::policy_from_value(self.read_agent_tools(agent_id).await?.as_ref())
    }

    fn preview_policy(
        &self,
        document: Option<&serde_json::Value>,
        candidate: ToolSelectionPolicy,
    ) -> Result<ToolSelectionPolicyPreview, ToolConfigError> {
        candidate.validate().map_err(ToolConfigError::Config)?;
        let current = Self::policy_from_value(document.and_then(|value| value.get("tools")))?;
        Ok(ToolSelectionPolicyPreview {
            expected: Self::revision(document),
            current,
            candidate,
        })
    }

    pub async fn preview_selection_policy(
        &self,
        agent_id: Option<&str>,
        candidate: ToolSelectionPolicy,
    ) -> Result<ToolSelectionPolicyPreview, ToolConfigError> {
        let document = self
            .cells
            .cell(Self::agent_id_or_default(agent_id))
            .read_document()
            .await?;
        self.preview_policy(document.as_ref(), candidate)
    }

    pub async fn set_selection_policy(
        &self,
        agent_id: Option<&str>,
        expected: Option<serde_json::Value>,
        candidate: ToolSelectionPolicy,
    ) -> Result<ToolSelectionPolicy, ToolConfigError> {
        let cell = self.cells.cell(Self::agent_id_or_default(agent_id));
        let current = cell
            .read_document()
            .await?
            .ok_or_else(|| ToolConfigError::NotFound(Self::agent_id_or_default(agent_id).into()))?;
        let current = Some(current);
        if Self::revision(current.as_ref()) != expected {
            return Err(ToolConfigError::Conflict(
                "Tool configuration changed; preview it again".into(),
            ));
        }
        let preview = self.preview_policy(current.as_ref(), candidate)?;
        let mut replacement = current.clone().unwrap_or_else(|| serde_json::json!({}));
        let document = replacement.as_object_mut().ok_or_else(|| {
            ToolConfigError::Config("Agent configuration must be an object".into())
        })?;
        document
            .entry("tools")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or_else(|| ToolConfigError::Config("tools must be an object".into()))?
            .insert(
                "selection_policy".into(),
                serde_json::to_value(&preview.candidate).expect("Tool policy serializes"),
            );
        if !cell.compare_exchange_document(current, replacement).await? {
            return Err(ToolConfigError::Conflict(
                "Tool configuration changed; preview it again".into(),
            ));
        }
        Ok(preview.candidate)
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
        let entry = builtin_tools
            .get(tool_name)
            .or_else(|| self.catalog.get(tool_name))
            .ok_or_else(|| ToolConfigError::NotFound(tool_name.to_string()))?;
        if !entry.is_object() {
            return Err(ToolConfigError::Config(format!("invalid tool {tool_name}")));
        }

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

        let definition = self
            .catalog
            .get(tool_name)
            .cloned()
            .ok_or_else(|| ToolConfigError::NotFound(tool_name.into()))?;
        let cell = self.cells.cell(id);
        cell.update(|current| {
            let mut tools_value = current.unwrap_or_else(|| serde_json::json!({}));
            let builtin = tools_value
                .as_object_mut()
                .expect("validated tools object")
                .entry("builtin_tools")
                .or_insert_with(|| serde_json::json!({}));
            let entry = builtin
                .as_object_mut()
                .expect("validated builtin_tools object")
                .entry(tool_name)
                .or_insert(definition);
            entry
                .as_object_mut()
                .expect("validated tool entry")
                .insert("config".into(), encrypted_config);
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
        let entry = builtin_tools
            .get(tool_name)
            .or_else(|| self.catalog.get(tool_name))
            .ok_or_else(|| ToolConfigError::NotFound(tool_name.to_string()))?;
        if !entry.is_object() {
            return Err(ToolConfigError::Config(format!("invalid tool {tool_name}")));
        }

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

    pub async fn set_identity_enabled(
        &self,
        agent_id: Option<&str>,
        identity: ToolIdentity,
        enabled: bool,
    ) -> Result<ToolSelectionPolicy, ToolConfigError> {
        identity.validate().map_err(ToolConfigError::Config)?;
        let current = self.read_agent_tools(agent_id).await?;
        let mut policy = Self::policy_from_value(current.as_ref())?;
        policy.set_tool_enabled(identity, enabled);
        let mut replacement = current
            .clone()
            .expect("explicit policy has a tools section");
        replacement
            .as_object_mut()
            .ok_or_else(|| ToolConfigError::Config("tools must be an object".into()))?
            .insert(
                "selection_policy".into(),
                serde_json::to_value(&policy).expect("Tool policy serializes"),
            );
        if !self
            .cells
            .cell(Self::agent_id_or_default(agent_id))
            .compare_exchange(current, replacement)
            .await?
        {
            return Err(ToolConfigError::Conflict(
                "Tool configuration changed; retry the edit".into(),
            ));
        }
        Ok(policy)
    }

    pub async fn set_enabled(
        &self,
        agent_id: Option<&str>,
        tool_name: &str,
        enabled: bool,
    ) -> Result<ToolInfo, ToolConfigError> {
        self.update_tool(agent_id, tool_name, Some(enabled), None)
            .await
    }

    pub async fn set_async_execution(
        &self,
        agent_id: Option<&str>,
        tool_name: &str,
        async_execution: bool,
    ) -> Result<ToolInfo, ToolConfigError> {
        self.update_tool(agent_id, tool_name, None, Some(async_execution))
            .await
    }

    async fn update_tool(
        &self,
        agent_id: Option<&str>,
        tool_name: &str,
        enabled: Option<bool>,
        async_execution: Option<bool>,
    ) -> Result<ToolInfo, ToolConfigError> {
        let definition = self
            .catalog
            .get(tool_name)
            .cloned()
            .ok_or_else(|| ToolConfigError::NotFound(tool_name.into()))?;
        let current = self.read_agent_tools(agent_id).await?;
        let mut policy = Self::policy_from_value(current.as_ref())?;
        let mut replacement = current
            .clone()
            .ok_or_else(|| ToolConfigError::Config("tools.selection_policy is required".into()))?;
        self.extract_builtin_tools(&replacement)?;
        let tools = replacement
            .as_object_mut()
            .ok_or_else(|| ToolConfigError::Config("tools must be an object".into()))?;
        if let Some(enabled) = enabled {
            policy.set_tool_enabled(ToolIdentity::builtin(tool_name), enabled);
            tools.insert(
                "selection_policy".into(),
                serde_json::to_value(&policy).expect("Tool policy serializes"),
            );
        }
        let builtin = tools
            .entry("builtin_tools")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .expect("validated builtin_tools");
        let entry = builtin
            .entry(tool_name)
            .or_insert(definition)
            .as_object_mut()
            .ok_or_else(|| ToolConfigError::Config(format!("invalid tool {tool_name}")))?;
        if let Some(async_execution) = async_execution {
            entry.insert(
                "async_execution".into(),
                serde_json::Value::Bool(async_execution),
            );
        }
        let builtin = self.extract_builtin_tools(&replacement)?;
        let info = self
            .map_to_dto_list(&builtin, &policy)?
            .into_iter()
            .find(|tool| tool.name == tool_name)
            .ok_or_else(|| ToolConfigError::NotFound(tool_name.into()))?;
        if !self
            .cells
            .cell(Self::agent_id_or_default(agent_id))
            .compare_exchange(current, replacement)
            .await?
        {
            return Err(ToolConfigError::Conflict(
                "Tool configuration changed; retry the edit".into(),
            ));
        }
        Ok(info)
    }
}
