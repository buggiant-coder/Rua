pub fn redact_provider_secret(secret: &str) -> String {
    if secret.is_empty() {
        return "***".to_string();
    }
    if secret.len() <= 10 {
        return "***".to_string();
    }
    let prefix = &secret[..4];
    let suffix = &secret[secret.len() - 4..];
    format!("{prefix}...{suffix}")
}

pub fn is_redaction_sentinel(value: &str) -> bool {
    value == "***"
        || value == "****"
        || value == "***REDACTED***"
        || value.chars().all(|c| c == '*')
}

pub fn is_secret_field(field_name: &str) -> bool {
    let lower = field_name.to_lowercase();
    lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("authorization")
}

pub fn redact_json_secrets(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if is_secret_field(key) {
                    if let serde_json::Value::String(s) = val {
                        *s = redact_provider_secret(s);
                    }
                } else {
                    redact_json_secrets(val);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                redact_json_secrets(item);
            }
        }
        _ => {}
    }
}
