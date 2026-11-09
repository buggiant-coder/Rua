use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::loader::{load_agent_config, load_config, save_config};
use crate::types::LastDispatchConfig;

pub fn get_config_path(wd: &Path) -> PathBuf {
    let file = std::env::var("RUA_CONFIG_FILE").unwrap_or_else(|_| "config.json".to_string());
    wd.join(file)
}

pub fn get_heartbeat_query_path(wd: &Path) -> PathBuf {
    let file = std::env::var("RUA_HEARTBEAT_FILE").unwrap_or_else(|_| "HEARTBEAT.md".to_string());
    get_config_path(wd)
        .parent()
        .unwrap_or(Path::new("."))
        .join(file)
}

pub fn get_heartbeat_config(
    wd: &Path,
    agent_id: Option<&str>,
) -> std::io::Result<serde_json::Value> {
    if let Some(id) = agent_id {
        let agent_config = load_agent_config(id, wd)?;
        {
            if let Some(hb) = agent_config.heartbeat {
                return Ok(hb);
            }
        }
        return Ok(serde_json::json!({
            "every": "6h",
            "target": "main"
        }));
    }

    let config = load_config(&get_config_path(wd), wd)?;
    if let Some(defaults) = &config.agents.defaults {
        if let Some(hb) = defaults.get("heartbeat") {
            if !hb.is_null() {
                return Ok(hb.clone());
            }
        }
    }
    Ok(serde_json::json!({
        "every": "6h",
        "target": "main"
    }))
}

pub fn update_last_dispatch(
    wd: &Path,
    channel: &str,
    user_id: &str,
    session_id: &str,
    agent_id: Option<&str>,
) -> std::io::Result<()> {
    let config_path = get_config_path(wd);

    if let Some(id) = agent_id {
        let mut agent_config = load_agent_config(id, wd)?;
        {
            let ld = serde_json::json!({
                "channel": channel,
                "user_id": user_id,
                "session_id": session_id,
            });
            agent_config.last_dispatch = Some(ld);
            let ws = crate::dirs::shellexpand_path(&agent_config.workspace_dir);
            return crate::loader::save_agent_config(&agent_config, &ws);
        }
    }

    let mut config = load_config(&config_path, wd)?;
    config.last_dispatch = Some(LastDispatchConfig {
        channel: channel.to_string(),
        user_id: user_id.to_string(),
        session_id: session_id.to_string(),
        extra: serde_json::Map::new(),
    });
    save_config(&config, &config_path)
}

pub fn read_last_api(wd: &Path) -> std::io::Result<Option<(String, u64)>> {
    let config = load_config(&get_config_path(wd), wd)?;
    let Some(la) = config.extra.get("last_api") else {
        return Ok(None);
    };
    let host = la
        .get("host")
        .and_then(|v| v.as_str())
        .ok_or_else(|| std::io::Error::other("last_api.host must be a string"))?
        .to_string();
    let port = la
        .get("port")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| std::io::Error::other("last_api.port must be an unsigned integer"))?;
    if host.is_empty() {
        return Err(std::io::Error::other("last_api.host must not be empty"));
    }
    Ok(Some((host, port)))
}

pub fn write_last_api(wd: &Path, host: &str, port: u64) -> std::io::Result<()> {
    let config_path = get_config_path(wd);
    let mut config = load_config(&config_path, wd)?;
    config.extra.insert(
        "last_api".to_string(),
        serde_json::json!({ "host": host, "port": port }),
    );
    save_config(&config, &config_path)
}

pub fn get_jobs_path(wd: &Path) -> PathBuf {
    let file = std::env::var("RUA_JOBS_FILE").unwrap_or_else(|_| "jobs.json".to_string());
    wd.join(file)
}

pub fn get_plugins_dir(wd: &Path) -> PathBuf {
    wd.join("plugins")
}

pub fn is_rua_running(wd: &Path) -> bool {
    let path = get_heartbeat_query_path(wd);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let mtime = match meta.modified() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let elapsed = SystemTime::now().duration_since(mtime).unwrap_or_default();
    elapsed.as_secs() < 60
}
