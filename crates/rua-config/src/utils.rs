use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::loader::{load_agent_config, load_config, save_config};

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
    let agent = load_agent_config(agent_id.unwrap_or("default"), wd)?;
    Ok(agent.heartbeat.unwrap_or_else(|| {
        serde_json::json!({
            "every": "6h", "target": "main"
        })
    }))
}

pub fn update_last_dispatch(
    wd: &Path,
    channel: &str,
    user_id: &str,
    session_id: &str,
    agent_id: Option<&str>,
) -> std::io::Result<()> {
    let mut agent = load_agent_config(agent_id.unwrap_or("default"), wd)?;
    agent.last_dispatch = Some(serde_json::json!({
        "channel": channel, "user_id": user_id, "session_id": session_id
    }));
    let workspace = crate::dirs::shellexpand_path(&agent.workspace_dir);
    crate::loader::save_agent_config(&agent, &workspace)
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
