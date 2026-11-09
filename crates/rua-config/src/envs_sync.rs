use std::path::Path;

pub fn set_env_from_envs_json(rua_root: &Path) {
    let envs_path = rua_root.join("envs.json");

    if !envs_path.is_file() {
        return;
    }

    let content = match std::fs::read_to_string(&envs_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %envs_path.display(), "Failed to read envs.json: {e}");
            return;
        }
    };

    let map: serde_json::Map<String, serde_json::Value> = match serde_json::from_str(&content) {
        Ok(serde_json::Value::Object(m)) => m,
        Ok(_) => {
            tracing::warn!(path = %envs_path.display(), "envs.json is not a JSON object");
            return;
        }
        Err(e) => {
            tracing::warn!(path = %envs_path.display(), "Failed to parse envs.json: {e}");
            return;
        }
    };

    for (key, value) in &map {
        let val_str = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        std::env::set_var(key, &val_str);
    }

    tracing::debug!(count = map.len(), "Loaded env vars from envs.json");
}
