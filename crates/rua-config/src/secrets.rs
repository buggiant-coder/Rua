use serde_json::Value;

const REDACTION_SENTINELS: &[&str] = &["***", "REDACTED", "******", "sk-***REDACTED***"];

pub fn is_redacted(value: &str) -> bool {
    let trimmed = value.trim();
    REDACTION_SENTINELS.contains(&trimmed) || trimmed.contains("***") || trimmed == "REDACTED"
}

const SECRET_FIELD_NAMES: &[&str] = &[
    "api_key",
    "client_secret",
    "bot_token",
    "access_token",
    "auth_token",
    "secret",
    "app_secret",
    "sk",
    "password",
    "encrypt_key",
    "verification_token",
    "http_proxy_auth",
    "twilio_auth_token",
    "app_token",
    "signing_secret",
    "secret_key",
    "livekit_api_key",
    "livekit_api_secret",
    "sip_password",
    "dashscope_api_key",
];

pub fn is_secret_field(field_name: &str) -> bool {
    SECRET_FIELD_NAMES.contains(&field_name)
}

pub fn merge_with_redaction_safety(
    existing: &serde_json::Map<String, Value>,
    incoming: &serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut result = existing.clone();

    for (key, new_val) in incoming {
        match new_val {
            Value::Null if is_secret_field(key) => {}
            Value::String(s) if is_redacted(s) => {}
            Value::Object(new_obj) => {
                if let Some(Value::Object(old_obj)) = existing.get(key) {
                    let merged = merge_with_redaction_safety(old_obj, new_obj);
                    result.insert(key.clone(), Value::Object(merged));
                } else {
                    result.insert(key.clone(), new_val.clone());
                }
            }
            _ => {
                result.insert(key.clone(), new_val.clone());
            }
        }
    }

    result
}

pub fn redact_secrets(value: &Value) -> Value {
    match value {
        Value::Object(obj) => {
            let mut redacted = serde_json::Map::new();
            for (key, val) in obj {
                if is_secret_field(key) {
                    if let Value::String(s) = val {
                        if !s.is_empty() {
                            redacted.insert(key.clone(), Value::String("***".into()));
                        } else {
                            redacted.insert(key.clone(), val.clone());
                        }
                    } else {
                        redacted.insert(key.clone(), val.clone());
                    }
                } else {
                    redacted.insert(key.clone(), redact_secrets(val));
                }
            }
            Value::Object(redacted)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(redact_secrets).collect()),
        _ => value.clone(),
    }
}

pub fn redact_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    if chars.len() <= 8 {
        "***".to_string()
    } else {
        let prefix: String = chars[..4].iter().collect();
        let suffix: String = chars[chars.len() - 4..].iter().collect();
        format!("{prefix}...{suffix}")
    }
}

pub fn mask_env_value(value: &str) -> String {
    if value.is_empty() {
        return value.to_string();
    }
    let length = value.len();
    if length <= 8 {
        return "*".repeat(length);
    }
    let chars: Vec<char> = value.chars().collect();
    let prefix_len = if chars.len() > 2 && chars[2] == '-' {
        3
    } else {
        2
    };
    let prefix: String = chars[..prefix_len].iter().collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    let masked_len = std::cmp::max(length - prefix_len - 4, 4);
    format!("{prefix}{}{suffix}", "*".repeat(masked_len))
}

pub fn is_masked_value(value: &str) -> bool {
    value.contains("****")
}
