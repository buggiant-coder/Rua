use std::path::Path;

use llm_multiplex::LlmError;
use rua_config::encrypted_io::EncryptedFieldIO;
use rua_config::secret_projection::{SecretFieldPolicy, SecretProjectionMatch};

use crate::config_loader::ProviderConfig;

pub(crate) struct ProviderRead {
    pub config: ProviderConfig,
    pub needs_rewrite: bool,
}

pub(crate) fn read_provider_config(
    path: &Path,
    key: Option<&[u8; 32]>,
) -> std::io::Result<ProviderRead> {
    let bytes = std::fs::read(path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    let mut needs_rewrite = key.is_some()
        && contains_plaintext_secret_fields(
            &value,
            &ProviderSecretFieldPolicy,
            &mut Vec::new(),
            None,
        );
    let read = decrypt_secret_fields(value, key)?;
    let mut config: ProviderConfig =
        serde_json::from_value(read.value).map_err(std::io::Error::other)?;
    needs_rewrite |= config.apply_model_defaults();
    Ok(ProviderRead {
        config,
        needs_rewrite,
    })
}

pub(crate) fn write_provider_config(
    path: &Path,
    config: &ProviderConfig,
    key: Option<&[u8; 32]>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut config = config.clone();
    config.apply_model_defaults();
    let value = serde_json::to_value(&config).map_err(std::io::Error::other)?;
    let value = encrypt_secret_fields(&value, key, &config.id)?;
    rua_config::atomic_write_json(path, &value)?;
    chmod_600(path)?;
    Ok(())
}

pub(crate) fn llm_config_error(action: &str, path: &Path, err: std::io::Error) -> LlmError {
    LlmError::Config {
        message: format!("{action} at {} failed: {err}", path.display()),
    }
}

fn decrypt_secret_fields(
    value: serde_json::Value,
    key: Option<&[u8; 32]>,
) -> std::io::Result<rua_config::secret_projection::SecretProjectionRead> {
    rua_config::secret_projection::decrypt_secret_fields(&value, key, &ProviderSecretFieldPolicy)
}

fn encrypt_secret_fields(
    value: &serde_json::Value,
    key: Option<&[u8; 32]>,
    provider_id: &str,
) -> std::io::Result<serde_json::Value> {
    rua_config::secret_projection::encrypt_secret_fields(
        value,
        key,
        provider_id,
        &ProviderSecretFieldPolicy,
    )
}

fn contains_plaintext_secret_fields(
    value: &serde_json::Value,
    policy: &dyn SecretFieldPolicy,
    path: &mut Vec<String>,
    current_key: Option<&str>,
) -> bool {
    if EncryptedFieldIO::is_encrypted_value(value) {
        return false;
    }
    if policy.classify(path, current_key, value).is_some() {
        return true;
    }

    match value {
        serde_json::Value::Array(items) => items
            .iter()
            .any(|item| contains_plaintext_secret_fields(item, policy, path, None)),
        serde_json::Value::Object(obj) => obj.iter().any(|(field, field_value)| {
            path.push(field.clone());
            let contains = contains_plaintext_secret_fields(field_value, policy, path, Some(field));
            path.pop();
            contains
        }),
        _ => false,
    }
}

struct ProviderSecretFieldPolicy;

impl SecretFieldPolicy for ProviderSecretFieldPolicy {
    fn classify(
        &self,
        path: &[String],
        current_key: Option<&str>,
        value: &serde_json::Value,
    ) -> Option<SecretProjectionMatch> {
        match current_key {
            Some("api_key") if is_encryptable_string_secret(value) => {
                Some(SecretProjectionMatch::string("api_key"))
            }
            Some("api_keys") if is_encryptable_json_secret(value) => {
                Some(SecretProjectionMatch::json("api_keys"))
            }
            Some(name) if is_provider_secret_header(path, name, value) => {
                let map_field = path.get(path.len().saturating_sub(2))?;
                Some(SecretProjectionMatch::string(format!("{map_field}.{name}")))
            }
            _ => None,
        }
    }

    fn tool_hint(&self, provider_id: &str, field: &str) -> String {
        provider_secret_tool_hint(provider_id, field)
    }

    fn missing_key_error(&self, field: &str) -> std::io::Error {
        encrypted_without_key_error(field)
    }
}

fn is_encryptable_string_secret(value: &serde_json::Value) -> bool {
    EncryptedFieldIO::is_encrypted_value(value) || value.as_str().is_some_and(|s| !s.is_empty())
}

fn is_encryptable_json_secret(value: &serde_json::Value) -> bool {
    if EncryptedFieldIO::is_encrypted_value(value) {
        return true;
    }
    if value.is_null() {
        return false;
    }
    !value.as_array().is_some_and(|items| items.is_empty())
}

fn is_provider_secret_header(path: &[String], name: &str, value: &serde_json::Value) -> bool {
    let parent_key = path
        .len()
        .checked_sub(2)
        .and_then(|idx| path.get(idx))
        .map(String::as_str);
    matches!(parent_key, Some("headers" | "custom_headers"))
        && is_secret_header_name(name)
        && is_encryptable_string_secret(value)
}

fn encrypted_without_key_error(field: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("provider {field} is encrypted but no key provided"),
    )
}

fn provider_secret_tool_hint(provider_id: &str, field: &str) -> String {
    format!(
        "Use `rua providers get {provider_id}` to read this provider and provider update commands to update `{field}`. The provider id is also the filename without `.json`."
    )
}

fn is_secret_header_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "authorization"
        || lower.contains("api-key")
        || lower.contains("apikey")
        || lower.contains("api_key")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
}

fn chmod_600(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
