use std::path::{Path, PathBuf};

use rua_config::AgentProfileConfig;
use rua_core::{Result, RuaError};

use crate::agent_io::{load_agent_config, read_json_file, save_agent_config};
use crate::secret_field_io;
use crate::store_dto::{
    self, SharedMcpAgentResult, SharedMcpBroadcastRequest, SharedMcpBroadcastResponse,
    SharedMcpClientConfig, SharedMcpClientCreate, SharedMcpClientUpdate, SharedMcpSyncRequest,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct SharedMcpFile {
    #[serde(default)]
    pub clients: serde_json::Map<String, serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct SharedMcpStore {
    data_dir: PathBuf,
    device_key: Option<[u8; 32]>,
}

impl SharedMcpStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            device_key: None,
        }
    }

    pub fn with_device_key(mut self, device_key: [u8; 32]) -> Self {
        self.device_key = Some(device_key);
        self
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn shared_mcp_path(&self) -> PathBuf {
        self.data_dir.join("shared_mcp.json")
    }

    fn device_key(&self) -> Option<&[u8; 32]> {
        self.device_key.as_ref()
    }

    fn decrypt_mcp_value(&self, value: &serde_json::Value) -> Result<serde_json::Value> {
        secret_field_io::decrypt_client_value(value, self.device_key()).map_err(secret_io_error)
    }

    fn encrypt_mcp_value(
        &self,
        value: &serde_json::Value,
        scope_hint: impl AsRef<str>,
    ) -> Result<serde_json::Value> {
        secret_field_io::encrypt_client_value(value, self.device_key(), scope_hint)
            .map_err(secret_io_error)
    }

    fn load_shared_mcp(&self) -> Result<SharedMcpFile> {
        let Some(raw) = read_json_file::<serde_json::Value>(&self.shared_mcp_path())? else {
            return Ok(SharedMcpFile::default());
        };
        let decrypted = self.decrypt_mcp_value(&raw)?;
        serde_json::from_value(decrypted).map_err(|e| RuaError::Config {
            message: format!("Failed to parse decrypted shared_mcp.json: {e}"),
        })
    }

    fn save_shared_mcp(&self, file: &SharedMcpFile) -> Result<()> {
        let path = self.shared_mcp_path();
        let value = serde_json::to_value(file).map_err(|e| RuaError::Config {
            message: format!("Failed to serialize shared_mcp.json: {e}"),
        })?;
        let value = self.encrypt_mcp_value(&value, "shared:mcp")?;
        rua_config::atomic_write_json(&path, &value).map_err(|e| RuaError::Io {
            message: format!("Failed to write shared_mcp.json: {e}"),
        })?;
        Ok(())
    }

    pub fn get_agent_mcp_clients(
        agent_config: &AgentProfileConfig,
    ) -> serde_json::Map<String, serde_json::Value> {
        match &agent_config.mcp {
            Some(mcp_value) => mcp_value["clients"]
                .as_object()
                .cloned()
                .unwrap_or_default(),
            None => serde_json::Map::new(),
        }
    }

    fn decrypt_agent_mcp_clients(
        &self,
        agent_config: &AgentProfileConfig,
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        let Some(mcp_value) = &agent_config.mcp else {
            return Ok(serde_json::Map::new());
        };
        let decrypted = self.decrypt_mcp_value(mcp_value)?;
        Ok(decrypted["clients"]
            .as_object()
            .cloned()
            .unwrap_or_default())
    }

    pub fn set_agent_mcp_clients(
        agent_config: &mut AgentProfileConfig,
        clients: serde_json::Map<String, serde_json::Value>,
    ) {
        match agent_config.mcp.as_mut() {
            Some(mcp_value) => {
                if let Some(mcp_obj) = mcp_value.as_object_mut() {
                    mcp_obj.insert("clients".to_string(), serde_json::Value::Object(clients));
                } else {
                    *mcp_value = serde_json::json!({"clients": clients});
                }
            }
            None => {
                agent_config.mcp = Some(serde_json::json!({"clients": clients}));
            }
        }
    }

    fn encrypt_agent_mcp_for_save(
        &self,
        agent_id: &str,
        agent_config: &mut AgentProfileConfig,
    ) -> Result<()> {
        let Some(mcp_value) = agent_config.mcp.as_ref() else {
            return Ok(());
        };
        let encrypted = self.encrypt_mcp_value(mcp_value, format!("agent:{agent_id}:mcp"))?;
        agent_config.mcp = Some(encrypted);
        Ok(())
    }

    pub fn list_agent_ids(&self) -> Result<Vec<String>> {
        let ws_dir = self.data_dir.join("workspaces");
        if !ws_dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        let entries = std::fs::read_dir(&ws_dir).map_err(|e| RuaError::Io {
            message: format!("Failed to read workspaces dir: {e}"),
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| RuaError::Io {
                message: format!("Failed to read dir entry: {e}"),
            })?;
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if entry.path().join("agent.json").exists() {
                        ids.push(name.to_string());
                    }
                }
            }
        }
        Ok(ids)
    }

    pub fn value_to_dto(key: &str, value: &serde_json::Value) -> Result<SharedMcpClientConfig> {
        let normalized = store_dto::normalize_legacy_json(value);
        let server: store_dto::McpServerConfig =
            serde_json::from_value(normalized).map_err(|e| RuaError::Config {
                message: format!("Failed to parse MCP client '{key}': {e}"),
            })?;
        Ok(SharedMcpClientConfig {
            key: key.to_string(),
            server,
        })
    }

    pub fn create_to_value(create: &SharedMcpClientCreate) -> Result<serde_json::Value> {
        let mut map = serde_json::Map::new();
        map.insert(
            "name".into(),
            serde_json::Value::String(create.name.clone()),
        );
        map.insert("enabled".into(), serde_json::Value::Bool(create.enabled));
        map.insert(
            "transport".into(),
            serde_json::Value::String(if create.transport.is_empty() {
                "stdio".to_string()
            } else {
                create.transport.clone()
            }),
        );
        map.insert(
            "command".into(),
            serde_json::Value::String(create.command.clone().unwrap_or_default()),
        );
        map.insert(
            "args".into(),
            serde_json::to_value(&create.args).unwrap_or(serde_json::Value::Array(vec![])),
        );
        map.insert(
            "env".into(),
            create
                .env
                .clone()
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
        );
        map.insert(
            "url".into(),
            serde_json::Value::String(create.url.clone().unwrap_or_default()),
        );
        map.insert(
            "headers".into(),
            create
                .headers
                .clone()
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
        );
        map.insert(
            "description".into(),
            serde_json::Value::String(String::new()),
        );
        map.insert("cwd".into(), serde_json::Value::String(String::new()));
        if let Some(oauth) = &create.oauth {
            map.insert(
                "oauth".into(),
                serde_json::to_value(oauth).unwrap_or(serde_json::Value::Null),
            );
        }
        if !create.disabled_tools.is_empty() {
            map.insert(
                "disabled_tools".into(),
                serde_json::to_value(&create.disabled_tools)
                    .unwrap_or(serde_json::Value::Array(vec![])),
            );
        }
        for (k, v) in &create.extra {
            map.insert(k.clone(), v.clone());
        }
        Ok(serde_json::Value::Object(map))
    }

    pub fn apply_update(existing: &mut serde_json::Value, update: &SharedMcpClientUpdate) {
        if let serde_json::Value::Object(ref mut map) = existing {
            if let Some(ref name) = update.name {
                map.insert("name".into(), serde_json::Value::String(name.clone()));
            }
            if let Some(enabled) = update.enabled {
                map.insert("enabled".into(), serde_json::Value::Bool(enabled));
            }
            if let Some(ref transport) = update.transport {
                map.insert(
                    "transport".into(),
                    serde_json::Value::String(transport.clone()),
                );
            }
            if let Some(ref command) = update.command {
                map.insert("command".into(), serde_json::Value::String(command.clone()));
            }
            if let Some(ref args) = update.args {
                if let Ok(args_val) = serde_json::to_value(args) {
                    map.insert("args".into(), args_val);
                }
            }
            if let Some(ref env) = update.env {
                map.insert("env".into(), env.clone());
            }
            if let Some(ref url) = update.url {
                map.insert("url".into(), serde_json::Value::String(url.clone()));
            }
            if let Some(ref headers) = update.headers {
                map.insert("headers".into(), headers.clone());
            }
            if let Some(ref oauth) = update.oauth {
                map.insert(
                    "oauth".into(),
                    serde_json::to_value(oauth).unwrap_or(serde_json::Value::Null),
                );
            }
            if let Some(ref disabled_tools) = update.disabled_tools {
                if let Ok(value) = serde_json::to_value(disabled_tools) {
                    map.insert("disabled_tools".into(), value);
                }
            }
            for (k, v) in &update.extra {
                map.insert(k.clone(), v.clone());
            }
        }
    }

    pub async fn broadcast(
        &self,
        req: SharedMcpBroadcastRequest,
    ) -> Result<SharedMcpBroadcastResponse> {
        let shared_file = self.load_shared_mcp()?;

        let client_keys: Vec<String> = if req.client_keys.is_empty() {
            shared_file.clients.keys().cloned().collect()
        } else {
            req.client_keys.clone()
        };

        let agent_ids = if req.agent_ids.is_empty() {
            self.list_agent_ids()?
        } else {
            req.agent_ids.clone()
        };

        let mut results = Vec::new();
        for agent_id_str in &agent_ids {
            let agent_id_opt = Some(agent_id_str.as_str());
            match load_agent_config(&self.data_dir, agent_id_opt) {
                Ok(mut agent_config) => {
                    let mut mcp_clients = self.decrypt_agent_mcp_clients(&agent_config)?;
                    let mut changed = false;

                    for key in &client_keys {
                        if let Some(value) = shared_file.clients.get(key) {
                            let exists = mcp_clients.contains_key(key);
                            if exists {
                                let conflict_mode = req
                                    .extra
                                    .get("conflict_mode")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("ask");
                                match conflict_mode {
                                    "skip" => {
                                        results.push(SharedMcpAgentResult {
                                            agent_id: agent_id_str.clone(),
                                            success: true,
                                            error: Some("skipped".into()),
                                            extra: serde_json::Map::new(),
                                        });
                                        continue;
                                    }
                                    "ask" => {
                                        results.push(SharedMcpAgentResult {
                                            agent_id: agent_id_str.clone(),
                                            success: false,
                                            error: Some("conflict".into()),
                                            extra: serde_json::Map::new(),
                                        });
                                        continue;
                                    }
                                    _ => {}
                                }
                            }
                            if mcp_clients.get(key) != Some(value) {
                                mcp_clients.insert(key.clone(), value.clone());
                                changed = true;
                            }
                        }
                    }

                    if changed {
                        Self::set_agent_mcp_clients(&mut agent_config, mcp_clients);
                        self.encrypt_agent_mcp_for_save(agent_id_str, &mut agent_config)?;
                        if let Err(e) =
                            save_agent_config(&self.data_dir, agent_id_opt, &agent_config)
                        {
                            results.push(SharedMcpAgentResult {
                                agent_id: agent_id_str.clone(),
                                success: false,
                                error: Some(format!("failed to save: {e}")),
                                extra: serde_json::Map::new(),
                            });
                            continue;
                        }
                    }

                    results.push(SharedMcpAgentResult {
                        agent_id: agent_id_str.clone(),
                        success: true,
                        error: None,
                        extra: serde_json::Map::new(),
                    });
                }
                Err(e) => {
                    results.push(SharedMcpAgentResult {
                        agent_id: agent_id_str.clone(),
                        success: false,
                        error: Some(format!("failed to load agent config: {e}")),
                        extra: serde_json::Map::new(),
                    });
                }
            }
        }

        Ok(SharedMcpBroadcastResponse {
            results,
            extra: serde_json::Map::new(),
        })
    }

    pub async fn sync_from_agent(
        &self,
        req: SharedMcpSyncRequest,
    ) -> Result<SharedMcpBroadcastResponse> {
        let agent_config = load_agent_config(&self.data_dir, Some(req.agent_id.as_str()))?;
        let agent_mcp_clients = self.decrypt_agent_mcp_clients(&agent_config)?;

        let client_keys: Vec<String> = if req.client_keys.is_empty() {
            agent_mcp_clients.keys().cloned().collect()
        } else {
            req.client_keys.clone()
        };

        let mut shared_file = self.load_shared_mcp()?;

        let mut changed = false;
        let mut results = Vec::new();
        let conflict_mode = req
            .extra
            .get("conflict_mode")
            .and_then(|value| value.as_str())
            .unwrap_or("ask");
        for key in &client_keys {
            let Some(value) = agent_mcp_clients.get(key) else {
                results.push(SharedMcpAgentResult {
                    agent_id: req.agent_id.clone(),
                    success: false,
                    error: Some(format!("MCP client '{key}' not found in source agent")),
                    extra: serde_json::Map::from_iter([
                        ("client_key".into(), serde_json::Value::String(key.clone())),
                        ("status".into(), serde_json::Value::String("error".into())),
                    ]),
                });
                continue;
            };

            let existed = shared_file.clients.contains_key(key);
            match shared_file.clients.get(key) {
                Some(existing) if existing == value => {
                    results.push(SharedMcpAgentResult {
                        agent_id: req.agent_id.clone(),
                        success: true,
                        error: None,
                        extra: serde_json::Map::from_iter([
                            ("client_key".into(), serde_json::Value::String(key.clone())),
                            (
                                "status".into(),
                                serde_json::Value::String("unchanged".into()),
                            ),
                        ]),
                    });
                    continue;
                }
                Some(_) => match conflict_mode {
                    "skip" => {
                        results.push(SharedMcpAgentResult {
                            agent_id: req.agent_id.clone(),
                            success: true,
                            error: Some("skipped".into()),
                            extra: serde_json::Map::from_iter([
                                ("client_key".into(), serde_json::Value::String(key.clone())),
                                ("status".into(), serde_json::Value::String("skipped".into())),
                            ]),
                        });
                        continue;
                    }
                    "overwrite" => {}
                    _ => {
                        results.push(SharedMcpAgentResult {
                            agent_id: req.agent_id.clone(),
                            success: false,
                            error: Some("conflict".into()),
                            extra: serde_json::Map::from_iter([
                                ("client_key".into(), serde_json::Value::String(key.clone())),
                                (
                                    "status".into(),
                                    serde_json::Value::String("conflict".into()),
                                ),
                            ]),
                        });
                        continue;
                    }
                },
                None => {}
            }

            shared_file.clients.insert(key.clone(), value.clone());
            changed = true;
            results.push(SharedMcpAgentResult {
                agent_id: req.agent_id.clone(),
                success: true,
                error: None,
                extra: serde_json::Map::from_iter([
                    ("client_key".into(), serde_json::Value::String(key.clone())),
                    (
                        "status".into(),
                        serde_json::Value::String(
                            if existed { "overwritten" } else { "created" }.into(),
                        ),
                    ),
                ]),
            });
        }

        if changed {
            self.save_shared_mcp(&shared_file)?;
        }

        Ok(SharedMcpBroadcastResponse {
            results,
            extra: serde_json::Map::new(),
        })
    }
}

fn secret_io_error(err: std::io::Error) -> RuaError {
    RuaError::Config {
        message: format!("MCP secret field IO failed: {err}"),
    }
}
