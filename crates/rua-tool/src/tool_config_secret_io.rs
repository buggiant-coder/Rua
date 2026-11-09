use rua_config::secret_projection::{
    decrypt_secret_fields, encrypt_secret_fields, field_path, SecretFieldPolicy,
    SecretProjectionKind, SecretProjectionMatch,
};
use rua_config::secrets::{is_masked_value, mask_env_value};

const MISSING_KEY_ERROR: &str =
    "tool config contains encrypted secret fields but no device key provided";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub struct ToolConfigSnapshot {
    pub values: serde_json::Value,
    pub secret_pointers: Vec<String>,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ToolConfigSnapshot {
    pub fn capture(value: &serde_json::Value) -> Self {
        fn capture(
            value: &mut serde_json::Value,
            path: &mut Vec<String>,
            pointers: &mut Vec<String>,
        ) {
            let secret = should_protect_string(path, path.last().map(String::as_str));
            if secret && !value.is_null() {
                let pointer = path
                    .iter()
                    .map(|part| format!("/{}", part.replace('~', "~0").replace('/', "~1")))
                    .collect::<String>();
                pointers.push(pointer);
                *value = serde_json::Value::Null;
                return;
            }
            match value {
                serde_json::Value::Object(fields) => {
                    for (key, value) in fields {
                        path.push(key.clone());
                        capture(value, path, pointers);
                        path.pop();
                    }
                }
                serde_json::Value::Array(items) => {
                    for (index, item) in items.iter_mut().enumerate() {
                        path.push(index.to_string());
                        capture(item, path, pointers);
                        path.pop();
                    }
                }
                _ => {}
            }
        }
        let mut snapshot = Self {
            values: value.clone(),
            secret_pointers: Vec::new(),
            extra: serde_json::Map::new(),
        };
        capture(
            &mut snapshot.values,
            &mut Vec::new(),
            &mut snapshot.secret_pointers,
        );
        snapshot
    }

    pub fn resolve(&self, runtime_values: &serde_json::Value) -> Result<serde_json::Value, String> {
        let mut values = self.values.clone();
        for pointer in &self.secret_pointers {
            let path = pointer
                .strip_prefix('/')
                .ok_or_else(|| "invalid tool credential pointer".to_string())?
                .split('/')
                .map(|part| part.replace("~1", "/").replace("~0", "~"))
                .collect::<Vec<_>>();
            if !should_protect_string(&path, path.last().map(String::as_str)) {
                return Err(format!(
                    "tool configuration reference is not a credential: {pointer}"
                ));
            }
            let secret = runtime_values
                .pointer(pointer)
                .ok_or_else(|| format!("tool credential unavailable at {pointer}"))?;
            if secret.is_null() {
                return Err(format!("tool credential unavailable at {pointer}"));
            }
            let slot = values
                .pointer_mut(pointer)
                .ok_or_else(|| format!("invalid tool configuration reference {pointer}"))?;
            if !slot.is_null() {
                return Err(format!(
                    "tool credential placeholder must be null: {pointer}"
                ));
            }
            *slot = secret.clone();
        }
        Ok(values)
    }
}

pub fn encrypt_config_value(
    value: &serde_json::Value,
    key: Option<&[u8; 32]>,
    scope_hint: impl AsRef<str>,
) -> std::io::Result<serde_json::Value> {
    encrypt_secret_fields(
        value,
        key,
        scope_hint.as_ref(),
        &ToolConfigSecretFieldPolicy,
    )
}

pub fn decrypt_config_value(
    value: &serde_json::Value,
    key: Option<&[u8; 32]>,
) -> std::io::Result<serde_json::Value> {
    decrypt_secret_fields(value, key, &ToolConfigSecretFieldPolicy).map(|read| read.value)
}

pub fn mask_config_secrets(value: &mut serde_json::Value) {
    mask_json_value(value, &mut Vec::new(), None);
}

pub fn restore_masked_config_values(
    incoming: &mut serde_json::Value,
    existing: &serde_json::Value,
) {
    restore_masked_json_value(incoming, existing, &mut Vec::new(), None);
}

struct ToolConfigSecretFieldPolicy;

impl SecretFieldPolicy for ToolConfigSecretFieldPolicy {
    fn classify(
        &self,
        path: &[String],
        current_key: Option<&str>,
        value: &serde_json::Value,
    ) -> Option<SecretProjectionMatch> {
        if value.as_str().is_some() && should_protect_string(path, current_key) {
            return Some(SecretProjectionMatch::string(field_path(
                path,
                current_key,
                "tool_config_secret",
            )));
        }
        None
    }

    fn tool_hint(&self, scope_hint: &str, field: &str) -> String {
        tool_hint(scope_hint, field)
    }

    fn missing_key_error(&self, _field: &str) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData, MISSING_KEY_ERROR)
    }

    fn unclassified_encrypted_kind(&self) -> Option<SecretProjectionKind> {
        Some(SecretProjectionKind::String)
    }
}

fn mask_json_value(
    value: &mut serde_json::Value,
    path: &mut Vec<String>,
    current_key: Option<&str>,
) {
    if let Some(s) = value.as_str() {
        if should_protect_string(path, current_key) {
            *value = serde_json::Value::String(mask_env_value(s));
        }
        return;
    }

    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                mask_json_value(item, path, None);
            }
        }
        serde_json::Value::Object(obj) => {
            for (field, field_value) in obj {
                path.push(field.clone());
                mask_json_value(field_value, path, Some(field));
                path.pop();
            }
        }
        _ => {}
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
        if should_protect_string(path, current_key)
            && is_masked_value(incoming_str)
            && incoming_str == mask_env_value(existing_str)
        {
            *incoming_value = existing_value.clone();
        }
        return;
    }

    match (incoming_value, existing_value) {
        (serde_json::Value::Array(incoming), serde_json::Value::Array(existing)) => {
            for (index, item) in incoming.iter_mut().enumerate() {
                let Some(existing_item) = existing.get(index) else {
                    continue;
                };
                restore_masked_json_value(item, existing_item, path, None);
            }
        }
        (serde_json::Value::Object(incoming), serde_json::Value::Object(existing)) => {
            for (field, field_value) in incoming.iter_mut() {
                let Some(existing_field) = existing.get(field) else {
                    continue;
                };
                path.push(field.clone());
                restore_masked_json_value(field_value, existing_field, path, Some(field));
                path.pop();
            }
        }
        _ => {}
    }
}

pub fn should_protect_string(path: &[String], current_key: Option<&str>) -> bool {
    let Some(key) = current_key else {
        return false;
    };

    let parent_key = path
        .len()
        .checked_sub(2)
        .and_then(|idx| path.get(idx))
        .map(String::as_str);

    if parent_key == Some("headers") {
        return is_sensitive_header(key);
    }

    is_sensitive_raw_field(key)
}

fn is_sensitive_header(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "x-api-key"
    ) || lower.contains("auth")
        || lower.contains("key")
        || lower.contains("token")
        || lower.contains("secret")
}

fn is_sensitive_raw_field(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    [
        "api_key",
        "access_key",
        "client_secret",
        "password",
        "secret",
        "token",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn tool_hint(scope_hint: &str, field: &str) -> String {
    format!(
        "Use the built-in tool configuration UI to update `{field}` in `{scope_hint}`. Do not edit encrypted envelopes by hand."
    )
}
