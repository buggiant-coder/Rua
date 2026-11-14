use std::path::{Path, PathBuf};
use std::sync::Arc;

use rua_config::AgentProfileConfig;
use rua_core::{Result, RuaError};
use tracing::warn;

use crate::agent_io::load_agent_config_raw;
use crate::agent_io::save_agent_config;
use crate::agent_mcp_cells::AgentMcpCells;
use crate::secret_field_io;
use crate::secret_mask;
use crate::shared_store::SharedMcpStore;
use crate::store_dto::{McpClientConfig, McpClientCreate, McpClientUpdate, McpOAuthConfig};

#[derive(Clone)]
pub struct McpClientStore {
    data_dir: PathBuf,
    agent_mcp_cells: Option<Arc<AgentMcpCells>>,
    device_key: Option<[u8; 32]>,
}

struct LoadedMcpClients {
    agent_config: Option<AgentProfileConfig>,
    original_mcp_value: serde_json::Value,
    mcp_value: serde_json::Value,
    clients: serde_json::Map<String, serde_json::Value>,
}

impl McpClientStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            agent_mcp_cells: None,
            device_key: None,
        }
    }

    pub fn with_agent_mcp_cells(data_dir: PathBuf, agent_mcp_cells: Arc<AgentMcpCells>) -> Self {
        Self {
            data_dir,
            agent_mcp_cells: Some(agent_mcp_cells),
            device_key: None,
        }
    }

    pub fn with_device_key(mut self, device_key: [u8; 32]) -> Self {
        self.device_key = Some(device_key);
        self
    }

    pub fn with_agent_mcp_cells_and_key(
        data_dir: PathBuf,
        agent_mcp_cells: Arc<AgentMcpCells>,
        device_key: [u8; 32],
    ) -> Self {
        Self::with_agent_mcp_cells(data_dir, agent_mcp_cells).with_device_key(device_key)
    }

    pub fn decrypt_mcp_value(
        value: &serde_json::Value,
        device_key: Option<&[u8; 32]>,
    ) -> Result<serde_json::Value> {
        secret_field_io::decrypt_client_value(value, device_key).map_err(secret_io_error)
    }

    pub fn encrypt_mcp_value(
        value: &serde_json::Value,
        device_key: Option<&[u8; 32]>,
        scope_hint: impl AsRef<str>,
    ) -> Result<serde_json::Value> {
        secret_field_io::encrypt_client_value(value, device_key, scope_hint)
            .map_err(secret_io_error)
    }

    fn device_key(&self) -> Option<&[u8; 32]> {
        self.device_key.as_ref()
    }

    fn decrypt_for_load(&self, value: &serde_json::Value) -> Result<serde_json::Value> {
        Self::decrypt_mcp_value(value, self.device_key())
    }

    fn encrypt_for_save(
        &self,
        agent_id: Option<&str>,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let scope = agent_id
            .map(|id| format!("agent:{id}:mcp"))
            .unwrap_or_else(|| "agent:mcp".to_string());
        Self::encrypt_mcp_value(value, self.device_key(), scope)
    }

    pub fn mcp_config_from_decrypted_value(
        value: serde_json::Value,
    ) -> Result<mcp_manager::types::McpConfig> {
        serde_json::from_value(value).map_err(|e| RuaError::Config {
            message: format!("failed to deserialize decrypted MCP config: {e}"),
        })
    }

    pub fn decrypt_mcp_config(
        value: &serde_json::Value,
        device_key: Option<&[u8; 32]>,
    ) -> Result<mcp_manager::types::McpConfig> {
        Self::mcp_config_from_decrypted_value(Self::decrypt_mcp_value(value, device_key)?)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    async fn load(&self, agent_id: Option<&str>) -> Result<LoadedMcpClients> {
        if let (Some(cells), Some(agent_id)) = (&self.agent_mcp_cells, agent_id) {
            let mcp_value = cells
                .cell(agent_id)
                .read_value()
                .await
                .map_err(|e| RuaError::Mcp {
                    server: agent_id.to_string(),
                    message: e.to_string(),
                })?
                .unwrap_or_else(|| serde_json::json!({"clients": {}}));
            let mcp_value = self.decrypt_for_load(&mcp_value)?;
            let clients = clients_from_mcp_value(&mcp_value);
            return Ok(LoadedMcpClients {
                agent_config: None,
                original_mcp_value: mcp_value.clone(),
                mcp_value,
                clients,
            });
        }

        let agent_config = load_agent_config_raw(&self.data_dir, agent_id)?;
        let mcp_value = agent_config
            .mcp
            .clone()
            .unwrap_or_else(|| serde_json::json!({"clients": {}}));
        let mcp_value = self.decrypt_for_load(&mcp_value)?;
        let clients = clients_from_mcp_value(&mcp_value);
        Ok(LoadedMcpClients {
            agent_config: Some(agent_config),
            original_mcp_value: mcp_value.clone(),
            mcp_value,
            clients,
        })
    }

    async fn save(&self, agent_id: Option<&str>, mut loaded: LoadedMcpClients) -> Result<()> {
        set_clients_in_mcp_value(&mut loaded.mcp_value, loaded.clients);
        if loaded.mcp_value == loaded.original_mcp_value {
            return Ok(());
        }
        let encrypted_mcp_value = self.encrypt_for_save(agent_id, &loaded.mcp_value)?;

        if let (Some(cells), Some(agent_id)) = (&self.agent_mcp_cells, agent_id) {
            cells
                .cell(agent_id)
                .write_value(encrypted_mcp_value)
                .await
                .map_err(|e| RuaError::Mcp {
                    server: agent_id.to_string(),
                    message: e.to_string(),
                })?;
            return Ok(());
        }

        let mut agent_config = loaded.agent_config.ok_or_else(|| RuaError::Config {
            message: "missing agent config for MCP client store save".into(),
        })?;
        agent_config.mcp = Some(encrypted_mcp_value);
        save_agent_config(&self.data_dir, agent_id, &agent_config)?;
        Ok(())
    }

    fn value_to_dto_masked(key: &str, value: &serde_json::Value) -> Result<McpClientConfig> {
        let mut dto_result = SharedMcpStore::value_to_dto(key, value)?;
        secret_mask::mask_client_secrets(&mut dto_result);
        Ok(dto_result)
    }

    pub async fn list_clients(&self, agent_id: Option<&str>) -> Result<Vec<McpClientConfig>> {
        let loaded = self.load(agent_id).await?;
        let mut result = Vec::new();
        for (key, value) in &loaded.clients {
            match Self::value_to_dto_masked(key, value) {
                Ok(dto) => result.push(dto),
                Err(e) => warn!("Failed to parse MCP client '{key}': {e}"),
            }
        }
        Ok(result)
    }

    pub async fn get_client(
        &self,
        agent_id: Option<&str>,
        client_key: &str,
    ) -> Result<Option<McpClientConfig>> {
        let loaded = self.load(agent_id).await?;
        match loaded.clients.get(client_key) {
            Some(value) => Ok(Some(Self::value_to_dto_masked(client_key, value)?)),
            None => Ok(None),
        }
    }

    pub async fn get_client_for_execution(
        &self,
        agent_id: Option<&str>,
        client_key: &str,
    ) -> Result<Option<McpClientConfig>> {
        let loaded = self.load(agent_id).await?;
        match loaded.clients.get(client_key) {
            Some(value) => Ok(Some(SharedMcpStore::value_to_dto(client_key, value)?)),
            None => Ok(None),
        }
    }

    pub async fn create_client(
        &self,
        agent_id: Option<&str>,
        config_create: McpClientCreate,
    ) -> Result<McpClientConfig> {
        let mut loaded = self.load(agent_id).await?;
        if loaded.clients.contains_key(&config_create.key) {
            return Err(RuaError::Validation {
                message: format!("MCP client '{}' already exists", config_create.key),
            });
        }
        let key = config_create.key.clone();
        let value = SharedMcpStore::create_to_value(&config_create)?;
        loaded.clients.insert(key.clone(), value.clone());
        self.save(agent_id, loaded).await?;
        Self::value_to_dto_masked(&key, &value)
    }

    pub async fn update_client(
        &self,
        agent_id: Option<&str>,
        client_key: &str,
        mut update: McpClientUpdate,
    ) -> Result<McpClientConfig> {
        let mut loaded = self.load(agent_id).await?;
        let entry = loaded
            .clients
            .get_mut(client_key)
            .ok_or_else(|| RuaError::NotFound {
                resource: "mcp_client".into(),
                id: client_key.into(),
            })?;
        secret_mask::restore_masked_values(&mut update, entry);
        SharedMcpStore::apply_update(entry, &update);
        let updated_value = entry.clone();
        self.save(agent_id, loaded).await?;
        Self::value_to_dto_masked(client_key, &updated_value)
    }

    pub async fn toggle_client(
        &self,
        agent_id: Option<&str>,
        client_key: &str,
        enabled: bool,
    ) -> Result<McpClientConfig> {
        let mut loaded = self.load(agent_id).await?;
        let entry = loaded
            .clients
            .get_mut(client_key)
            .ok_or_else(|| RuaError::NotFound {
                resource: "mcp_client".into(),
                id: client_key.into(),
            })?;
        if let serde_json::Value::Object(ref mut map) = entry {
            map.insert("enabled".into(), serde_json::Value::Bool(enabled));
        }
        let updated_value = entry.clone();
        self.save(agent_id, loaded).await?;
        Self::value_to_dto_masked(client_key, &updated_value)
    }

    pub async fn set_oauth(
        &self,
        agent_id: Option<&str>,
        client_key: &str,
        oauth: Option<McpOAuthConfig>,
    ) -> Result<McpClientConfig> {
        let mut loaded = self.load(agent_id).await?;
        let entry = loaded
            .clients
            .get_mut(client_key)
            .ok_or_else(|| RuaError::NotFound {
                resource: "mcp_client".into(),
                id: client_key.into(),
            })?;
        if let serde_json::Value::Object(ref mut map) = entry {
            match oauth {
                Some(value) => {
                    map.insert(
                        "oauth".into(),
                        serde_json::to_value(value).map_err(|e| RuaError::Config {
                            message: format!("Failed to serialize MCP oauth config: {e}"),
                        })?,
                    );
                }
                None => {
                    map.remove("oauth");
                }
            }
        }
        let updated_value = entry.clone();
        self.save(agent_id, loaded).await?;
        Self::value_to_dto_masked(client_key, &updated_value)
    }

    pub async fn delete_client(&self, agent_id: Option<&str>, client_key: &str) -> Result<()> {
        let mut loaded = self.load(agent_id).await?;
        if loaded.clients.remove(client_key).is_none() {
            return Err(RuaError::NotFound {
                resource: "mcp_client".into(),
                id: client_key.into(),
            });
        }
        self.save(agent_id, loaded).await?;
        Ok(())
    }
}

fn secret_io_error(err: std::io::Error) -> RuaError {
    RuaError::Config {
        message: format!("failed to process encrypted MCP secret fields: {err}"),
    }
}

fn clients_from_mcp_value(
    mcp_value: &serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    mcp_value["clients"]
        .as_object()
        .cloned()
        .unwrap_or_default()
}

fn set_clients_in_mcp_value(
    mcp_value: &mut serde_json::Value,
    clients: serde_json::Map<String, serde_json::Value>,
) {
    match mcp_value.as_object_mut() {
        Some(obj) => {
            obj.insert("clients".to_string(), serde_json::Value::Object(clients));
        }
        None => {
            *mcp_value = serde_json::json!({"clients": clients});
        }
    }
}
