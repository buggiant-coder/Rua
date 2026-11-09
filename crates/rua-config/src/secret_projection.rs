use crate::encrypted_io::EncryptedFieldIO;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretProjectionKind {
    String,

    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretProjectionMatch {
    pub kind: SecretProjectionKind,
    pub field: String,
}

impl SecretProjectionMatch {
    pub fn string(field: impl Into<String>) -> Self {
        Self {
            kind: SecretProjectionKind::String,
            field: field.into(),
        }
    }

    pub fn json(field: impl Into<String>) -> Self {
        Self {
            kind: SecretProjectionKind::Json,
            field: field.into(),
        }
    }
}

pub trait SecretFieldPolicy {
    fn classify(
        &self,
        path: &[String],
        current_key: Option<&str>,
        value: &serde_json::Value,
    ) -> Option<SecretProjectionMatch>;

    fn tool_hint(&self, scope_hint: &str, field: &str) -> String;

    fn missing_key_error(&self, field: &str) -> std::io::Error;

    fn unclassified_encrypted_kind(&self) -> Option<SecretProjectionKind> {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SecretProjectionRead {
    pub value: serde_json::Value,
    pub encrypted_fields: usize,
}

pub fn encrypt_secret_fields(
    value: &serde_json::Value,
    key: Option<&[u8; 32]>,
    scope_hint: &str,
    policy: &dyn SecretFieldPolicy,
) -> std::io::Result<serde_json::Value> {
    let Some(key) = key else {
        return Ok(value.clone());
    };
    encrypt_value(value, key, scope_hint, policy, &mut Vec::new(), None)
}

pub fn decrypt_secret_fields(
    value: &serde_json::Value,
    key: Option<&[u8; 32]>,
    policy: &dyn SecretFieldPolicy,
) -> std::io::Result<SecretProjectionRead> {
    let mut count = 0;
    let value = decrypt_value(value, key, policy, &mut Vec::new(), None, &mut count)?;
    Ok(SecretProjectionRead {
        value,
        encrypted_fields: count,
    })
}

pub fn contains_encrypted_fields(value: &serde_json::Value) -> bool {
    if EncryptedFieldIO::is_encrypted_value(value) {
        return true;
    }

    match value {
        serde_json::Value::Array(items) => items.iter().any(contains_encrypted_fields),
        serde_json::Value::Object(obj) => obj.values().any(contains_encrypted_fields),
        _ => false,
    }
}

pub fn field_path(path: &[String], current_key: Option<&str>, fallback: &str) -> String {
    if path.is_empty() {
        return current_key.unwrap_or(fallback).to_string();
    }
    path.join(".")
}

fn encrypt_value(
    value: &serde_json::Value,
    key: &[u8; 32],
    scope_hint: &str,
    policy: &dyn SecretFieldPolicy,
    path: &mut Vec<String>,
    current_key: Option<&str>,
) -> std::io::Result<serde_json::Value> {
    if EncryptedFieldIO::is_encrypted_value(value) {
        return Ok(value.clone());
    }

    if let Some(secret) = policy.classify(path, current_key, value) {
        return encrypt_matched_value(value, key, scope_hint, policy, &secret);
    }

    match value {
        serde_json::Value::Array(items) => {
            let encrypted = items
                .iter()
                .map(|item| encrypt_value(item, key, scope_hint, policy, path, None))
                .collect::<std::io::Result<Vec<_>>>()?;
            Ok(serde_json::Value::Array(encrypted))
        }
        serde_json::Value::Object(obj) => {
            let mut encrypted = serde_json::Map::with_capacity(obj.len());
            for (field, field_value) in obj {
                path.push(field.clone());
                let new_value =
                    encrypt_value(field_value, key, scope_hint, policy, path, Some(field))?;
                path.pop();
                encrypted.insert(field.clone(), new_value);
            }
            Ok(serde_json::Value::Object(encrypted))
        }
        _ => Ok(value.clone()),
    }
}

fn encrypt_matched_value(
    value: &serde_json::Value,
    key: &[u8; 32],
    scope_hint: &str,
    policy: &dyn SecretFieldPolicy,
    secret: &SecretProjectionMatch,
) -> std::io::Result<serde_json::Value> {
    let plaintext = match secret.kind {
        SecretProjectionKind::String => {
            let Some(plaintext) = value.as_str() else {
                return Ok(value.clone());
            };
            plaintext.to_string()
        }
        SecretProjectionKind::Json => {
            serde_json::to_string(value).map_err(std::io::Error::other)?
        }
    };

    EncryptedFieldIO::encrypt_string(
        &plaintext,
        key,
        &secret.field,
        policy.tool_hint(scope_hint, &secret.field),
    )
}

fn decrypt_value(
    value: &serde_json::Value,
    key: Option<&[u8; 32]>,
    policy: &dyn SecretFieldPolicy,
    path: &mut Vec<String>,
    current_key: Option<&str>,
    encrypted_fields: &mut usize,
) -> std::io::Result<serde_json::Value> {
    if EncryptedFieldIO::is_encrypted_value(value) {
        let classified = policy.classify(path, current_key, value);
        let field = classified
            .as_ref()
            .map(|secret| secret.field.clone())
            .unwrap_or_else(|| field_path(path, current_key, "secret"));
        let Some(kind) = classified
            .as_ref()
            .map(|secret| secret.kind)
            .or_else(|| policy.unclassified_encrypted_kind())
        else {
            return Ok(value.clone());
        };
        let key = key.ok_or_else(|| policy.missing_key_error(&field))?;
        let plaintext = EncryptedFieldIO::decrypt_string(value, key)?;
        *encrypted_fields += 1;
        return match kind {
            SecretProjectionKind::String => Ok(serde_json::Value::String(plaintext)),
            SecretProjectionKind::Json => {
                serde_json::from_str(&plaintext).map_err(std::io::Error::other)
            }
        };
    }

    match value {
        serde_json::Value::Array(items) => {
            let decrypted = items
                .iter()
                .map(|item| decrypt_value(item, key, policy, path, None, encrypted_fields))
                .collect::<std::io::Result<Vec<_>>>()?;
            Ok(serde_json::Value::Array(decrypted))
        }
        serde_json::Value::Object(obj) => {
            let mut decrypted = serde_json::Map::with_capacity(obj.len());
            for (field, field_value) in obj {
                path.push(field.clone());
                let new_value = decrypt_value(
                    field_value,
                    key,
                    policy,
                    path,
                    Some(field),
                    encrypted_fields,
                )?;
                path.pop();
                decrypted.insert(field.clone(), new_value);
            }
            Ok(serde_json::Value::Object(decrypted))
        }
        _ => Ok(value.clone()),
    }
}
