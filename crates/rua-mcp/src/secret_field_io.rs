use rua_config::secret_projection::{
    contains_encrypted_fields as contains_projected_encrypted_fields, decrypt_secret_fields,
    encrypt_secret_fields, field_path, SecretFieldPolicy, SecretProjectionKind,
    SecretProjectionMatch,
};

const MISSING_KEY_ERROR: &str =
    "MCP config contains encrypted secret fields but no device key provided";

pub fn encrypt_client_value(
    value: &serde_json::Value,
    key: Option<&[u8; 32]>,
    scope_hint: impl AsRef<str>,
) -> std::io::Result<serde_json::Value> {
    encrypt_secret_fields(value, key, scope_hint.as_ref(), &McpSecretFieldPolicy)
}

pub fn decrypt_client_value(
    value: &serde_json::Value,
    key: Option<&[u8; 32]>,
) -> std::io::Result<serde_json::Value> {
    decrypt_secret_fields(value, key, &McpSecretFieldPolicy).map(|read| read.value)
}

pub fn contains_encrypted_fields(value: &serde_json::Value) -> bool {
    contains_projected_encrypted_fields(value)
}

struct McpSecretFieldPolicy;

impl SecretFieldPolicy for McpSecretFieldPolicy {
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
                "mcp_secret",
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

pub fn should_protect_string(path: &[String], current_key: Option<&str>) -> bool {
    let Some(key) = current_key else {
        return false;
    };

    let parent_key = path
        .len()
        .checked_sub(2)
        .and_then(|idx| path.get(idx))
        .map(String::as_str);

    if parent_key == Some("env") {
        return true;
    }

    if parent_key == Some("headers") {
        return is_sensitive_header(key);
    }

    if parent_key == Some("oauth") {
        return is_sensitive_oauth_field(key);
    }

    is_sensitive_raw_field(key)
}

pub fn is_sensitive_header(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "x-api-key"
    ) || lower.contains("auth")
        || lower.contains("key")
        || lower.contains("token")
        || lower.contains("secret")
}

pub fn is_sensitive_oauth_field(key: &str) -> bool {
    matches!(key, "access_token" | "refresh_token" | "client_secret")
}

pub fn is_sensitive_raw_field(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    [
        "client_secret",
        "password",
        "api_key",
        "access_key",
        "secret",
        "token",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn tool_hint(scope_hint: &str, field: &str) -> String {
    format!(
        "Use `rua mcp set-secret --client-key <client-key> --path {field} --value <secret>` to update this MCP secret in `{scope_hint}`. Use `--shared` for shared MCP configs. Do not edit encrypted envelopes by hand."
    )
}
