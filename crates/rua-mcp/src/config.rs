use std::collections::BTreeMap;
use std::path::Path;

use mcp_manager::types::{McpConfig, McpServerConfig, McpServerId};
use mcp_manager::McpError;

pub struct McpConfigLoader;

impl McpConfigLoader {
    pub fn load_from_file(path: &Path) -> Result<McpConfig, McpError> {
        let content = std::fs::read_to_string(path).map_err(|e| McpError::ConfigInvalid {
            message: format!("failed to read MCP config from {}: {e}", path.display()),
        })?;
        Self::load_from_str(&content)
    }

    pub fn load_from_str(json: &str) -> Result<McpConfig, McpError> {
        let config: McpConfig =
            serde_json::from_str(json).map_err(|e| McpError::ConfigInvalid {
                message: format!("failed to parse MCP config: {e}"),
            })?;
        Self::validate(&config)?;
        Ok(config)
    }

    pub fn load_from_value(value: serde_json::Value) -> Result<McpConfig, McpError> {
        let config: McpConfig =
            serde_json::from_value(value).map_err(|e| McpError::ConfigInvalid {
                message: format!("failed to parse MCP config value: {e}"),
            })?;
        Self::validate(&config)?;
        Ok(config)
    }

    fn validate(config: &McpConfig) -> Result<(), McpError> {
        for (id, server) in &config.clients {
            Self::validate_server(id, server)?;
        }
        Ok(())
    }

    fn validate_server(id: &McpServerId, server: &McpServerConfig) -> Result<(), McpError> {
        if server.name.is_empty() {
            return Err(McpError::ConfigInvalid {
                message: format!("server '{id}' has empty name"),
            });
        }

        match &server.transport {
            mcp_manager::types::McpTransportConfig::Stdio { command, .. } => {
                if command.is_empty() {
                    return Err(McpError::ConfigInvalid {
                        message: format!("server '{id}' stdio transport has empty command"),
                    });
                }
            }
            mcp_manager::types::McpTransportConfig::StreamableHttp { url, .. }
            | mcp_manager::types::McpTransportConfig::Sse { url, .. } => {
                if url.is_empty() {
                    return Err(McpError::ConfigInvalid {
                        message: format!("server '{id}' HTTP/SSE transport has empty URL"),
                    });
                }

                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(McpError::ConfigInvalid {
                        message: format!("server '{id}' URL must start with http:// or https://"),
                    });
                }
            }
        }

        Ok(())
    }

    pub fn config_hash(config: &McpConfig) -> u64 {
        use std::hash::{Hash, Hasher};
        let json = serde_json::to_string(config).unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        json.hash(&mut hasher);
        hasher.finish()
    }

    pub fn diff_configs(
        old: &McpConfig,
        new: &McpConfig,
    ) -> (
        Vec<McpServerId>,
        Vec<McpServerId>,
        Vec<McpServerId>,
        Vec<McpServerId>,
    ) {
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut updated = Vec::new();
        let mut unchanged = Vec::new();

        for (id, new_config) in &new.clients {
            match old.clients.get(id) {
                Some(old_config) => {
                    if old_config != new_config {
                        updated.push(id.clone());
                    } else {
                        unchanged.push(id.clone());
                    }
                }
                None => {
                    added.push(id.clone());
                }
            }
        }

        for id in old.clients.keys() {
            if !new.clients.contains_key(id) {
                removed.push(id.clone());
            }
        }

        (added, removed, updated, unchanged)
    }

    pub fn enabled_servers(config: &McpConfig) -> BTreeMap<McpServerId, McpServerConfig> {
        config
            .clients
            .iter()
            .filter(|(_, cfg)| cfg.enabled)
            .map(|(id, cfg)| (id.clone(), cfg.clone()))
            .collect()
    }
}
