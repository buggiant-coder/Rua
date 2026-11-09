use crate::secret_field_io;
use crate::store_dto::{McpClientConfig, McpClientUpdate, McpTransportConfig};

pub use rua_config::secrets::{is_masked_value, mask_env_value, redact_api_key};

pub fn mask_json_object_values(val: &mut serde_json::Value) {
    if let Some(obj) = val.as_object_mut() {
        for v in obj.values_mut() {
            if let Some(s) = v.as_str() {
                *v = serde_json::Value::String(mask_env_value(s));
            }
        }
    }
}

pub fn mask_client_secrets(client: &mut McpClientConfig) {
    match &mut client.server.transport {
        McpTransportConfig::Stdio { ref mut env, .. } => {
            for v in env.values_mut() {
                *v = mask_env_value(v);
            }
        }
        McpTransportConfig::StreamableHttp {
            ref mut headers, ..
        }
        | McpTransportConfig::Sse {
            ref mut headers, ..
        } => {
            for (k, v) in headers.iter_mut() {
                if secret_field_io::is_sensitive_header(k) {
                    *v = mask_env_value(v);
                }
            }
        }
    }
    if let Some(oauth) = client.server.oauth.as_mut() {
        oauth.access_token = mask_env_value(&oauth.access_token);
        oauth.refresh_token = mask_env_value(&oauth.refresh_token);
        for (field, value) in oauth.extra.iter_mut() {
            let mut path = vec!["oauth".to_string(), field.clone()];
            mask_protected_json_value(value, &mut path, Some(field));
        }
    }
    for (field, value) in client.server.extra.iter_mut() {
        if field == "bearer_token_env_var" {
            continue;
        }
        let mut path = vec![field.clone()];
        mask_protected_json_value(value, &mut path, Some(field));
    }
}

fn mask_protected_json_value(
    value: &mut serde_json::Value,
    path: &mut Vec<String>,
    current_key: Option<&str>,
) {
    if let Some(s) = value.as_str() {
        if secret_field_io::should_protect_string(path, current_key) {
            *value = serde_json::Value::String(mask_env_value(s));
        }
        return;
    }

    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                mask_protected_json_value(item, path, None);
            }
        }
        serde_json::Value::Object(obj) => {
            for (field, field_value) in obj {
                path.push(field.clone());
                mask_protected_json_value(field_value, path, Some(field));
                path.pop();
            }
        }
        _ => {}
    }
}

pub fn mask_client_sensitive_fields(client: &mut McpClientConfig) {
    mask_client_secrets(client);
}

pub fn restore_masked_values(update: &mut McpClientUpdate, existing_value: &serde_json::Value) {
    if let Some(ref mut env_val) = update.env {
        if let (Some(incoming_obj), Some(existing_env)) = (
            env_val.as_object(),
            existing_value.get("env").and_then(|e| e.as_object()),
        ) {
            let mut restored = serde_json::Map::new();
            for (k, v) in incoming_obj {
                if let (Some(incoming_str), Some(existing_val)) = (v.as_str(), existing_env.get(k))
                {
                    if let Some(existing_str) = existing_val.as_str() {
                        if is_masked_value(incoming_str)
                            && incoming_str == mask_env_value(existing_str)
                        {
                            restored.insert(k.clone(), existing_val.clone());
                            continue;
                        }
                    }
                }
                restored.insert(k.clone(), v.clone());
            }
            *env_val = serde_json::Value::Object(restored);
        }
    }
    if let Some(ref mut headers_val) = update.headers {
        if let (Some(incoming_obj), Some(existing_headers)) = (
            headers_val.as_object(),
            existing_value.get("headers").and_then(|h| h.as_object()),
        ) {
            let mut restored = serde_json::Map::new();
            for (k, v) in incoming_obj {
                if let (Some(incoming_str), Some(existing_val)) =
                    (v.as_str(), existing_headers.get(k))
                {
                    if let Some(existing_str) = existing_val.as_str() {
                        if is_masked_value(incoming_str)
                            && incoming_str == mask_env_value(existing_str)
                        {
                            restored.insert(k.clone(), existing_val.clone());
                            continue;
                        }
                    }
                }
                restored.insert(k.clone(), v.clone());
            }
            *headers_val = serde_json::Value::Object(restored);
        }
    }
    if let Some(ref mut oauth) = update.oauth {
        if let Some(existing_oauth) = existing_value.get("oauth") {
            restore_masked_oauth(oauth, existing_oauth);
        }
    }
    if let Some(oauth_val) = update.extra.get_mut("oauth") {
        if let Some(existing_oauth) = existing_value.get("oauth") {
            restore_masked_oauth_value(oauth_val, existing_oauth);
        }
    }
    for (field, incoming_value) in update.extra.iter_mut() {
        let Some(existing_field) = existing_value.get(field) else {
            continue;
        };
        let mut path = vec![field.clone()];
        restore_masked_json_value(incoming_value, existing_field, &mut path, Some(field));
    }
}

fn restore_masked_oauth(
    incoming: &mut mcp_manager::types::McpOAuthConfig,
    existing_value: &serde_json::Value,
) {
    if let Some(existing_obj) = existing_value.as_object() {
        if let Some(existing) = existing_obj.get("access_token").and_then(|v| v.as_str()) {
            if is_masked_value(&incoming.access_token)
                && incoming.access_token == mask_env_value(existing)
            {
                incoming.access_token = existing.to_string();
            }
        }
        if let Some(existing) = existing_obj.get("refresh_token").and_then(|v| v.as_str()) {
            if is_masked_value(&incoming.refresh_token)
                && incoming.refresh_token == mask_env_value(existing)
            {
                incoming.refresh_token = existing.to_string();
            }
        }
        for (field, incoming_value) in incoming.extra.iter_mut() {
            let Some(existing_field) = existing_obj.get(field) else {
                continue;
            };
            let mut path = vec!["oauth".to_string(), field.clone()];
            restore_masked_json_value(incoming_value, existing_field, &mut path, Some(field));
        }
    }
}

fn restore_masked_oauth_value(
    incoming_value: &mut serde_json::Value,
    existing_value: &serde_json::Value,
) {
    let (Some(incoming), Some(existing)) =
        (incoming_value.as_object_mut(), existing_value.as_object())
    else {
        return;
    };
    for field in ["access_token", "refresh_token", "client_secret"] {
        let Some(incoming_str) = incoming.get(field).and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(existing_val) = existing.get(field) else {
            continue;
        };
        let Some(existing_str) = existing_val.as_str() else {
            continue;
        };
        if is_masked_value(incoming_str) && incoming_str == mask_env_value(existing_str) {
            incoming.insert(field.to_string(), existing_val.clone());
        }
    }
    for (field, incoming_value) in incoming.iter_mut() {
        let Some(existing_field) = existing.get(field) else {
            continue;
        };
        let mut path = vec!["oauth".to_string(), field.clone()];
        restore_masked_json_value(incoming_value, existing_field, &mut path, Some(field));
    }
}

fn restore_masked_json_value(
    incoming_value: &mut serde_json::Value,
    existing_value: &serde_json::Value,
    path: &mut Vec<String>,
    current_key: Option<&str>,
) {
    if let (Some(incoming_str), Some(existing_str)) =
        (incoming_value.as_str(), existing_value.as_str())
    {
        if secret_field_io::should_protect_string(path, current_key)
            && is_masked_value(incoming_str)
            && incoming_str == mask_env_value(existing_str)
        {
            *incoming_value = existing_value.clone();
        }
        return;
    }

    let (Some(incoming), Some(existing)) =
        (incoming_value.as_object_mut(), existing_value.as_object())
    else {
        return;
    };

    for (field, field_value) in incoming.iter_mut() {
        let Some(existing_field) = existing.get(field) else {
            continue;
        };
        path.push(field.clone());
        restore_masked_json_value(field_value, existing_field, path, Some(field));
        path.pop();
    }
}
