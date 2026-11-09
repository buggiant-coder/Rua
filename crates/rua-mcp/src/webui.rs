use mcp_manager::redaction;
use mcp_manager::types::McpServerConfig;

pub fn redact_server_for_api(config: &McpServerConfig) -> serde_json::Value {
    let mut value = serde_json::to_value(config).unwrap_or_default();

    if let Some(env) = value.get_mut("env").and_then(|e| e.as_object_mut()) {
        for v in env.values_mut() {
            if let Some(s) = v.as_str() {
                *v = serde_json::Value::String(redaction::mask_secret(s));
            }
        }
    }

    if let Some(headers) = value.get_mut("headers").and_then(|h| h.as_object_mut()) {
        for (k, v) in headers.iter_mut() {
            let lower = k.to_lowercase();
            if lower.contains("auth")
                || lower.contains("key")
                || lower.contains("token")
                || lower.contains("secret")
            {
                if let Some(s) = v.as_str() {
                    *v = serde_json::Value::String(redaction::mask_secret(s));
                }
            }
        }
    }

    value
}
