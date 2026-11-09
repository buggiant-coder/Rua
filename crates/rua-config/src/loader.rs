use std::fs;
use std::path::{Path, PathBuf};

use crate::migration::atomic::atomic_write;
use crate::types::RootConfig;

pub fn load_config(config_path: &Path, _rua_root: &Path) -> std::io::Result<RootConfig> {
    let bytes = fs::read(config_path).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("read configuration {}: {error}", config_path.display()),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("parse configuration {}: {error}", config_path.display()),
        )
    })
}

pub fn save_config(config: &RootConfig, config_path: &Path) -> std::io::Result<()> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    atomic_write(config_path, json.as_bytes())
}

pub fn load_agent_config(
    agent_id: &str,
    rua_root: &Path,
) -> std::io::Result<crate::types::AgentProfileConfig> {
    let config_path = rua_root.join("config.json");
    let config = load_config(&config_path, rua_root)?;
    let value = config.agents.profiles.get(agent_id).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Agent '{agent_id}' is not registered"),
        )
    })?;
    let reference: crate::types::AgentProfileRef =
        serde_json::from_value(value.clone()).map_err(std::io::Error::other)?;
    let path = PathBuf::from(&reference.workspace_dir).join("agent.json");
    let bytes = fs::read(&path).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("read Agent configuration {}: {error}", path.display()),
        )
    })?;
    let profile: crate::types::AgentProfileConfig =
        serde_json::from_slice(&bytes).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("parse Agent configuration {}: {error}", path.display()),
            )
        })?;
    if profile.id != agent_id || profile.workspace_dir != reference.workspace_dir {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Agent '{agent_id}' identity or workspace differs from its registration"),
        ));
    }
    Ok(profile)
}

pub fn save_agent_config(
    config: &crate::types::AgentProfileConfig,
    workspace_dir: &Path,
) -> std::io::Result<()> {
    let agent_json = workspace_dir.join("agent.json");
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    atomic_write(&agent_json, json.as_bytes())
}
