//! QwenPaw channel declarations with explicit Rua protocol adaptations.
//! Snapshot source and license are recorded in data/qwenpaw-channels.json.
use std::sync::OnceLock;

use serde_json::{Map, Value};

pub const QWENPAW_CHANNELS_JSON: &str = include_str!("../data/qwenpaw-channels.json");
const LOCAL_CHANNELS_JSON: &str = include_str!("../data/local-channels.json");

fn catalog() -> &'static Map<String, Value> {
    static CATALOG: OnceLock<Map<String, Value>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let upstream: Value =
            serde_json::from_str(QWENPAW_CHANNELS_JSON).expect("valid channel snapshot");
        let local: Value =
            serde_json::from_str(LOCAL_CHANNELS_JSON).expect("valid channel adaptations");
        upstream["channels"]
            .as_object()
            .expect("channel snapshot map")
            .iter()
            .map(|(id, entry)| {
                let adaptation = &local["channels"][id];
                let mut fields = Map::new();
                for field in entry["fields"].as_array().expect("channel fields") {
                    let name = field["name"].as_str().expect("channel field name");
                    if [&local["common_omit"], &adaptation["omit"]]
                        .into_iter()
                        .any(|omit| {
                            omit.as_array()
                                .is_some_and(|omit| omit.iter().any(|value| value == name))
                        })
                    {
                        continue;
                    }
                    let renamed = adaptation["rename"][name].as_str().unwrap_or(name);
                    let mut field = field.clone();
                    field["name"] = renamed.into();
                    fields.insert(renamed.to_string(), field);
                }
                for overrides in [&local["common"], &adaptation["fields"]] {
                    if let Some(overrides) = overrides.as_object() {
                        for (name, override_value) in overrides {
                            if adaptation["omit"]
                                .as_array()
                                .is_some_and(|omit| omit.iter().any(|value| value == name))
                            {
                                continue;
                            }
                            let field = fields.entry(name.clone()).or_insert_with(|| {
                                serde_json::json!({
                                    "name": name, "kind": "text", "secret": false,
                                    "default": "", "options": []
                                })
                            });
                            field.as_object_mut().expect("field object").extend(
                                override_value.as_object().expect("field override").clone(),
                            );
                        }
                    }
                }
                let defaults = fields
                    .iter()
                    .map(|(name, field)| (name.clone(), field["default"].clone()))
                    .collect::<Map<_, _>>();
                (
                    id.clone(),
                    serde_json::json!({
                        "fields": fields.into_values().collect::<Vec<_>>(),
                        "defaults": defaults,
                        "order": local["order"].as_array()
                            .and_then(|order| order.iter().position(|value| value == id))
                            .unwrap_or(usize::MAX),
                    }),
                )
            })
            .collect()
    })
}

pub fn channel_types() -> Vec<String> {
    let mut keys: Vec<_> = catalog().keys().cloned().collect();
    keys.sort_by_key(|id| catalog()[id]["order"].as_u64().unwrap_or(u64::MAX));
    keys
}

pub fn canonical_channel_key(key: &str) -> &str {
    if key == "weixin" {
        "wechat"
    } else {
        key
    }
}

pub fn is_builtin_channel(key: &str) -> bool {
    catalog().contains_key(canonical_channel_key(key))
}

pub fn channel_defaults(key: &str) -> Value {
    catalog()
        .get(canonical_channel_key(key))
        .map(|entry| entry["defaults"].clone())
        .unwrap_or_else(|| serde_json::json!({}))
}

pub fn channel_fields(key: &str) -> Vec<Value> {
    catalog()
        .get(canonical_channel_key(key))
        .and_then(|entry| entry["fields"].as_array())
        .cloned()
        .unwrap_or_default()
}

/// Apply upstream defaults only to absent fields. Existing credentials and
/// unknown plugin/vendor fields retain their values.
pub fn resolve_channel_config(key: &str, config: &Value) -> Result<Value, String> {
    validate_channel_fields(key, config)?;
    let Some(saved) = config.as_object() else {
        return Ok(config.clone());
    };
    let mut resolved = channel_defaults(key)
        .as_object()
        .cloned()
        .unwrap_or_default();
    resolved.extend(saved.clone());
    if key == "console" {
        resolved.insert("enabled".into(), Value::Bool(true));
    }
    Ok(Value::Object(resolved))
}

/// Validate current channel configuration constraints.
pub fn validate_channel_fields(key: &str, config: &Value) -> Result<(), String> {
    if let Some(entry) = catalog().get(canonical_channel_key(key)) {
        if let Some(defaults) = entry["defaults"].as_object() {
            for (name, default) in defaults {
                if default.is_boolean() && config.get(name).is_some_and(|value| !value.is_boolean())
                {
                    return Err(format!("{key}.{name} must be a bool"));
                }
            }
        }
    }
    if key == "mqtt" {
        if let Some(port) = config.get("port") {
            serde_json::from_value::<u16>(port.clone())
                .map_err(|_| "mqtt.port must be an integer between 0 and 65535".to_string())?;
        }
    }
    if key == "sip"
        && config
            .get("bind_addr")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        && config
            .get("sip_mode")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("sip.sip_mode must be explicit when bind_addr is configured".into());
    }
    Ok(())
}
