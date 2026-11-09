use std::path::PathBuf;
use std::sync::Arc;

use cell_store::{CellStore, CellStoreError, FileSystemCellStore};
use mcp_manager::error::{McpError, McpResult};
use mcp_manager::types::{McpConfig, McpServerConfig, McpServerId};

use crate::secret_field_io;

pub struct SharedMcpCells {
    store: Arc<FileSystemCellStore>,
    file: PathBuf,
    device_key: Option<[u8; 32]>,
}

impl SharedMcpCells {
    pub fn new(store: Arc<FileSystemCellStore>, file: PathBuf) -> Self {
        Self {
            store,
            file,
            device_key: None,
        }
    }

    pub fn with_key(store: Arc<FileSystemCellStore>, file: PathBuf, device_key: [u8; 32]) -> Self {
        Self::new(store, file).with_device_key(device_key)
    }

    pub fn with_device_key(mut self, device_key: [u8; 32]) -> Self {
        self.device_key = Some(device_key);
        self
    }

    pub fn cell<'a>(&'a self, server_id: &McpServerId) -> SharedMcpCell<'a> {
        SharedMcpCell {
            cells: self,
            server_id: server_id.clone(),
        }
    }

    pub async fn list(
        &self,
    ) -> McpResult<std::collections::BTreeMap<McpServerId, McpServerConfig>> {
        let raw = match self
            .store
            .read::<serde_json::Value>(&self.file, "$")
            .await
            .map_err(cell_err_to_mcp)?
        {
            Some(value) => value,
            None => return Ok(std::collections::BTreeMap::new()),
        };
        let decrypted = self.decrypt_value(&raw)?;
        let cfg: McpConfig = serde_json::from_value(decrypted).map_err(|e| McpError::Internal {
            message: format!("failed to deserialize decrypted shared MCP config: {e}"),
        })?;
        Ok(cfg.clients)
    }

    fn json_path_for(server_id: &McpServerId) -> String {
        format!("$.clients.{}", server_id.as_str())
    }

    fn device_key(&self) -> Option<&[u8; 32]> {
        self.device_key.as_ref()
    }

    fn decrypt_value(&self, value: &serde_json::Value) -> McpResult<serde_json::Value> {
        secret_field_io::decrypt_client_value(value, self.device_key()).map_err(secret_err_to_mcp)
    }

    fn encrypt_value(
        &self,
        server_id: &McpServerId,
        value: &serde_json::Value,
    ) -> McpResult<serde_json::Value> {
        let scope = format!("shared:mcp:{}", server_id.as_str());
        secret_field_io::encrypt_client_value(value, self.device_key(), scope)
            .map_err(secret_err_to_mcp)
    }
}

pub struct SharedMcpCell<'a> {
    cells: &'a SharedMcpCells,
    server_id: McpServerId,
}

impl<'a> SharedMcpCell<'a> {
    pub async fn read(&self) -> McpResult<Option<McpServerConfig>> {
        let Some(raw) = self
            .cells
            .store
            .read::<serde_json::Value>(
                &self.cells.file,
                &SharedMcpCells::json_path_for(&self.server_id),
            )
            .await
            .map_err(cell_err_to_mcp)?
        else {
            return Ok(None);
        };
        let decrypted = self.cells.decrypt_value(&raw)?;
        serde_json::from_value(decrypted)
            .map(Some)
            .map_err(|e| McpError::Internal {
                message: format!(
                    "failed to deserialize decrypted shared MCP server '{}': {e}",
                    self.server_id.as_str()
                ),
            })
    }

    pub async fn write(&self, config: McpServerConfig) -> McpResult<()> {
        let raw = serde_json::to_value(config).map_err(|e| McpError::Internal {
            message: format!(
                "failed to serialize shared MCP server '{}': {e}",
                self.server_id.as_str()
            ),
        })?;
        let encrypted = self.cells.encrypt_value(&self.server_id, &raw)?;
        self.cells
            .store
            .write(
                &self.cells.file,
                &SharedMcpCells::json_path_for(&self.server_id),
                encrypted,
            )
            .await
            .map_err(cell_err_to_mcp)
    }

    pub async fn update<F>(&self, mutator: F) -> McpResult<McpServerConfig>
    where
        F: FnOnce(Option<McpServerConfig>) -> McpServerConfig + Send,
    {
        let mut captured_error: Option<McpError> = None;
        let server_id = self.server_id.clone();
        let device_key = self.cells.device_key;
        let updated_raw = self
            .cells
            .store
            .update::<serde_json::Value, _>(
                &self.cells.file,
                &SharedMcpCells::json_path_for(&self.server_id),
                |current| {
                    let fallback = current.clone().unwrap_or(serde_json::Value::Null);
                    let current_config = match current {
                        Some(ref raw) => match decrypt_shared_value(raw, device_key.as_ref())
                            .and_then(|decrypted| {
                                serde_json::from_value::<McpServerConfig>(decrypted).map_err(
                                    |e| McpError::Internal {
                                        message: format!(
                                            "failed to deserialize decrypted shared MCP server '{}': {e}",
                                            server_id.as_str()
                                        ),
                                    },
                                )
                            }) {
                                Ok(config) => Some(config),
                                Err(e) => {
                                    captured_error = Some(e);
                                    return fallback;
                                }
                            },
                        None => None,
                    };

                    let next = mutator(current_config);
                    let next_raw = match serde_json::to_value(&next).map_err(|e| {
                        McpError::Internal {
                            message: format!(
                                "failed to serialize shared MCP server '{}': {e}",
                                server_id.as_str()
                            ),
                        }
                    }) {
                        Ok(value) => value,
                        Err(e) => {
                            captured_error = Some(e);
                            return fallback;
                        }
                    };
                    let scope = format!("shared:mcp:{}", server_id.as_str());
                    match encrypt_shared_value(&next_raw, device_key.as_ref(), scope) {
                        Ok(value) => value,
                        Err(e) => {
                            captured_error = Some(e);
                            fallback
                        }
                    }
                },
            )
            .await
            .map_err(cell_err_to_mcp)?;

        if let Some(err) = captured_error {
            return Err(err);
        }
        let decrypted = self.cells.decrypt_value(&updated_raw)?;
        serde_json::from_value(decrypted).map_err(|e| McpError::Internal {
            message: format!(
                "failed to deserialize updated shared MCP server '{}': {e}",
                self.server_id.as_str()
            ),
        })
    }

    pub async fn delete(&self) -> McpResult<()> {
        self.cells
            .store
            .delete(
                &self.cells.file,
                &SharedMcpCells::json_path_for(&self.server_id),
            )
            .await
            .map_err(cell_err_to_mcp)
    }
}

fn cell_err_to_mcp(err: CellStoreError) -> McpError {
    McpError::Internal {
        message: format!("shared MCP cell store error: {err}"),
    }
}

fn secret_err_to_mcp(err: std::io::Error) -> McpError {
    McpError::Internal {
        message: format!("shared MCP secret field IO error: {err}"),
    }
}

fn decrypt_shared_value(
    value: &serde_json::Value,
    device_key: Option<&[u8; 32]>,
) -> McpResult<serde_json::Value> {
    secret_field_io::decrypt_client_value(value, device_key).map_err(secret_err_to_mcp)
}

fn encrypt_shared_value(
    value: &serde_json::Value,
    device_key: Option<&[u8; 32]>,
    scope_hint: impl AsRef<str>,
) -> McpResult<serde_json::Value> {
    secret_field_io::encrypt_client_value(value, device_key, scope_hint).map_err(secret_err_to_mcp)
}
