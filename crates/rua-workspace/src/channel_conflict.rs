use rua_agent::AgentCells;
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize)]
pub struct ConflictInfo {
    pub conflict: bool,
    pub conflicting_agent_id: String,
    pub conflicting_agent_name: String,
    pub channel_type: String,
    pub identifier: String,
}

pub fn extract_identifier(channel_type: &str, config: &Map<String, Value>) -> Option<String> {
    let str_field = |key: &str| {
        config
            .get(key)?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from)
    };

    let port_str = |key: &str| {
        config.get(key).and_then(|v| {
            v.as_str()
                .filter(|s| !s.is_empty())
                .map(String::from)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
                .or_else(|| v.as_f64().map(|n| (n as u64).to_string()))
        })
    };

    match channel_type {
        "dingtalk" => str_field("client_id"),
        "telegram" => str_field("bot_token"),
        "feishu" => str_field("app_id"),
        "discord" => str_field("bot_token"),
        "qq" => str_field("app_id"),
        "wecom" => str_field("bot_id"),
        "onebot" => {
            let host = str_field("ws_host")?;
            let port = port_str("ws_port")?;
            Some(format!("{host}:{port}"))
        }
        "matrix" => {
            let user_id = str_field("user_id")?;
            let homeserver = str_field("homeserver")?;
            Some(format!("{user_id}@{homeserver}"))
        }
        "mqtt" => {
            let host = str_field("host")?;
            let port = port_str("port")?;
            Some(format!("{host}:{port}"))
        }
        "xiaoyi" => str_field("access_key"),
        "yuanbao" => str_field("app_id"),
        _ => None,
    }
}

pub async fn check_channel_conflict(
    agent_cells: &AgentCells,
    channel_type: &str,
    identifier: &str,
    current_agent_id: &str,
) -> Result<Option<ConflictInfo>, String> {
    let agent_ids = agent_cells
        .list()
        .await
        .map_err(|e| format!("list agents: {e}"))?;

    for aid in &agent_ids {
        if aid == current_agent_id {
            continue;
        }

        let cell = agent_cells.section::<Value>(aid, "channels");
        let channels = match cell.read().await {
            Ok(Some(Value::Object(map))) => map,
            _ => continue,
        };

        let ch_config = match channels.get(channel_type) {
            Some(Value::Object(m)) => m,
            _ => continue,
        };

        let enabled = ch_config
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !enabled {
            continue;
        }

        if let Some(other_id) = extract_identifier(channel_type, ch_config) {
            if other_id == identifier {
                let agent_name = agent_cells
                    .full(aid)
                    .read_value()
                    .await
                    .ok()
                    .flatten()
                    .and_then(|v| v.get("name")?.as_str().map(String::from))
                    .unwrap_or_else(|| aid.clone());

                return Ok(Some(ConflictInfo {
                    conflict: true,
                    conflicting_agent_id: aid.clone(),
                    conflicting_agent_name: agent_name,
                    channel_type: channel_type.to_string(),
                    identifier: identifier.to_string(),
                }));
            }
        }
    }

    Ok(None)
}

pub async fn disable_conflicting_channel(
    agent_cells: &AgentCells,
    conflicting_agent_id: &str,
    channel_type: &str,
) -> Result<(), String> {
    let cell = agent_cells.section::<Value>(conflicting_agent_id, "channels");
    cell.update(|existing| {
        let mut map = match existing {
            Some(Value::Object(m)) => m,
            _ => return existing.unwrap_or(Value::Null),
        };
        if let Some(Value::Object(ch)) = map.get_mut(channel_type) {
            ch.insert("enabled".to_string(), Value::Bool(false));
        }
        Value::Object(map)
    })
    .await
    .map_err(|e| format!("disable conflicting channel: {e}"))?;
    Ok(())
}
