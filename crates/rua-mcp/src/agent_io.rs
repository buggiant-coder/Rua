use std::path::{Path, PathBuf};

use rua_config::AgentProfileConfig;
use rua_core::{Result, RuaError};

pub fn resolve_agent_id(agent_id: Option<&str>) -> &str {
    agent_id.unwrap_or("default")
}

pub fn agent_json_path(data_dir: &Path, agent_id: Option<&str>) -> PathBuf {
    let id = resolve_agent_id(agent_id);
    data_dir.join("workspaces").join(id).join("agent.json")
}

pub fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let parsed = serde_json::from_str(&content).map_err(|e| RuaError::Config {
                message: format!("Failed to parse {}: {e}", path.display()),
            })?;
            Ok(Some(parsed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(RuaError::Io {
            message: format!("Failed to read {}: {e}", path.display()),
        }),
    }
}

pub fn load_agent_config_raw(
    data_dir: &Path,
    agent_id: Option<&str>,
) -> Result<AgentProfileConfig> {
    let id = resolve_agent_id(agent_id).to_string();
    let path = agent_json_path(data_dir, Some(&id));
    let empty_profile = || empty_profile(&id);
    read_json_file(&path).map(|opt| opt.unwrap_or_else(empty_profile))
}

pub fn load_agent_config(data_dir: &Path, agent_id: Option<&str>) -> Result<AgentProfileConfig> {
    let id = resolve_agent_id(agent_id);

    let config = if id == "default" {
        let path = agent_json_path(data_dir, Some("default"));
        read_json_file(&path)?.unwrap_or_else(|| empty_profile(id))
    } else {
        let default_path = agent_json_path(data_dir, Some("default"));
        let mut base: serde_json::Value = read_json_file::<serde_json::Value>(&default_path)?
            .unwrap_or_else(|| serde_json::json!({}));

        let target_path = agent_json_path(data_dir, Some(id));
        let target_opt: Option<serde_json::Value> = read_json_file(&target_path)?;

        if let Some(base) = base.as_object_mut() {
            base.remove("tools");
        }

        let target_name: String = target_opt
            .as_ref()
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(id)
            .to_string();

        if let Some(target) = target_opt {
            if let (Some(base_obj), Some(target_obj)) = (base.as_object_mut(), target.as_object()) {
                for (key, value) in target_obj {
                    if !value.is_null() {
                        base_obj.insert(key.clone(), value.clone());
                    }
                }
            }
        }

        if let Some(obj) = base.as_object_mut() {
            obj.insert("id".to_string(), serde_json::Value::String(id.to_string()));
            obj.insert("name".to_string(), serde_json::Value::String(target_name));
        }

        serde_json::from_value(base).map_err(|e| RuaError::Config {
            message: format!("Failed to parse merged agent config for '{id}': {e}"),
        })?
    };

    Ok(config)
}

pub fn save_agent_config(
    data_dir: &Path,
    agent_id: Option<&str>,
    config: &AgentProfileConfig,
) -> Result<()> {
    let path = agent_json_path(data_dir, agent_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| RuaError::Io {
            message: format!("Failed to create directory {}: {e}", parent.display()),
        })?;
    }
    let value = serde_json::to_value(config).map_err(|e| RuaError::Config {
        message: format!("Failed to serialize agent config: {e}"),
    })?;
    rua_config::atomic_write_json(&path, &value).map_err(|e| RuaError::Io {
        message: format!("Failed to write {}: {e}", path.display()),
    })?;
    Ok(())
}

fn empty_profile(id: &str) -> AgentProfileConfig {
    AgentProfileConfig {
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        workspace_dir: String::new(),
        template_id: None,
        language: String::new(),
        system_prompt_files: Vec::new(),
        running: Default::default(),
        default_model: None,
        channels: None,
        mcp: None,
        heartbeat: None,
        last_dispatch: None,
        tools: None,
        extra: serde_json::Map::new(),
    }
}
