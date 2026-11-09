use serde_json::{Map, Value};

pub fn is_builtin_channel_key(key: &str) -> bool {
    rua_config::channel_catalog::is_builtin_channel(key)
}

pub fn channel_types() -> Vec<String> {
    rua_config::channel_catalog::channel_types()
}

pub fn inject_is_builtin(mut map: Map<String, Value>) -> Map<String, Value> {
    for (key, value) in map.iter_mut() {
        if let Value::Object(obj) = value {
            obj.insert(
                "isBuiltin".to_string(),
                Value::Bool(is_builtin_channel_key(key)),
            );
        }
    }
    map
}

pub fn backfill_builtin_defaults(mut map: Map<String, Value>) -> Map<String, Value> {
    for key in channel_types() {
        if !map.contains_key(&key) {
            map.insert(
                key.clone(),
                serde_json::json!({ "enabled": key == "console" }),
            );
        }
    }
    map
}
