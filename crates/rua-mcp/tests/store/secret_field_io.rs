use rua_config::encrypted_io::EncryptedFieldIO;
use rua_mcp::secret_field_io::{
    contains_encrypted_fields, decrypt_client_value, encrypt_client_value,
};
use serde_json::{json, Value};

const KEY: [u8; 32] = [7; 32];

#[test]
fn encrypts_sensitive_mcp_fields_and_preserves_readable_fields() {
    let raw = sample_raw_client();

    let encrypted =
        encrypt_client_value(&raw, Some(&KEY), "agent default mcp client demo").expect("encrypt");

    assert_eq!(encrypted["name"], "Demo MCP");
    assert_eq!(encrypted["transport"], "streamable_http");
    assert_eq!(encrypted["url"], "https://mcp.example.test");
    assert_eq!(encrypted["command"], "npx");
    assert_eq!(encrypted["args"], json!(["server"]));
    assert_eq!(encrypted["description"], "plain description");
    assert_eq!(encrypted["oauth"]["client_id"], "public-client");
    assert_eq!(encrypted["oauth"]["scope"], "openid profile");
    assert_eq!(
        encrypted["oauth"]["token_endpoint"],
        "https://auth.example.test/token"
    );
    assert_eq!(
        encrypted["oauth"]["auth_endpoint"],
        "https://auth.example.test/auth"
    );

    assert_encrypted_string(&encrypted["env"]["API_TOKEN"], "env.API_TOKEN", "env-token");
    assert_encrypted_string(
        &encrypted["headers"]["Authorization"],
        "headers.Authorization",
        "Bearer header-token",
    );
    assert_encrypted_string(
        &encrypted["headers"]["x-api-key"],
        "headers.x-api-key",
        "header-api-key",
    );
    assert_encrypted_string(
        &encrypted["headers"]["Cookie"],
        "headers.Cookie",
        "session=header-cookie",
    );
    assert_encrypted_string(
        &encrypted["headers"]["Set-Cookie"],
        "headers.Set-Cookie",
        "session=header-set-cookie",
    );
    assert_eq!(encrypted["headers"]["User-Agent"], "rua");
    assert_encrypted_string(
        &encrypted["oauth"]["access_token"],
        "oauth.access_token",
        "oauth-access",
    );
    assert_encrypted_string(
        &encrypted["oauth"]["refresh_token"],
        "oauth.refresh_token",
        "oauth-refresh",
    );
    assert_encrypted_string(
        &encrypted["oauth"]["client_secret"],
        "oauth.client_secret",
        "oauth-client-secret",
    );
    assert_encrypted_string(&encrypted["extra_token"], "extra_token", "extra-token");
    assert_encrypted_string(
        &encrypted["nested"]["database_password"],
        "nested.database_password",
        "db-password",
    );
    assert_eq!(encrypted["nested"]["display_name"], "Nested");
}

#[test]
fn encryption_is_idempotent_for_existing_envelopes() {
    let raw = sample_raw_client();
    let encrypted_once =
        encrypt_client_value(&raw, Some(&KEY), "agent default mcp client demo").expect("encrypt");
    let env_once = encrypted_once["env"]["API_TOKEN"].clone();
    let header_once = encrypted_once["headers"]["Authorization"].clone();

    let encrypted_twice =
        encrypt_client_value(&encrypted_once, Some(&KEY), "agent default mcp client demo")
            .expect("encrypt again");

    assert_eq!(encrypted_twice["env"]["API_TOKEN"], env_once);
    assert_eq!(encrypted_twice["headers"]["Authorization"], header_once);
    assert_eq!(
        decrypt_client_value(&encrypted_twice, Some(&KEY)).expect("decrypt"),
        raw
    );
}

#[test]
fn decrypt_handles_plaintext_encrypted_and_mixed_inputs() {
    let raw = sample_raw_client();
    assert_eq!(
        decrypt_client_value(&raw, None).expect("plaintext without key"),
        raw
    );

    let encrypted =
        encrypt_client_value(&raw, Some(&KEY), "agent default mcp client demo").expect("encrypt");
    assert!(contains_encrypted_fields(&encrypted));
    assert_eq!(
        decrypt_client_value(&encrypted, Some(&KEY)).expect("decrypt encrypted"),
        raw
    );

    let mut mixed = raw.clone();
    mixed["oauth"]["access_token"] = encrypted["oauth"]["access_token"].clone();
    assert_eq!(
        decrypt_client_value(&mixed, Some(&KEY)).expect("decrypt mixed"),
        raw
    );
}

#[test]
fn missing_key_errors_only_when_encrypted_fields_are_present() {
    let raw = sample_raw_client();
    assert!(!contains_encrypted_fields(&raw));
    assert_eq!(
        decrypt_client_value(&raw, None).expect("plaintext without key"),
        raw
    );

    let encrypted =
        encrypt_client_value(&raw, Some(&KEY), "agent default mcp client demo").expect("encrypt");
    let err = decrypt_client_value(&encrypted, None).expect_err("missing key should fail");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err
        .to_string()
        .contains("MCP config contains encrypted secret fields but no device key provided"));
}

fn sample_raw_client() -> Value {
    json!({
        "name": "Demo MCP",
        "description": "plain description",
        "transport": "streamable_http",
        "url": "https://mcp.example.test",
        "command": "npx",
        "args": ["server"],
        "cwd": "/tmp/demo",
        "enabled": true,
        "auto_connect": "on_start",
        "call_timeout_secs": 30,
        "max_concurrency": 2,
        "disabled_tools": ["debug"],
        "env": {
            "API_TOKEN": "env-token",
            "PLAIN_ENV": "env-plain"
        },
        "headers": {
            "Authorization": "Bearer header-token",
            "x-api-key": "header-api-key",
            "Cookie": "session=header-cookie",
            "Set-Cookie": "session=header-set-cookie",
            "User-Agent": "rua"
        },
        "oauth": {
            "client_id": "public-client",
            "scope": "openid profile",
            "access_token": "oauth-access",
            "refresh_token": "oauth-refresh",
            "client_secret": "oauth-client-secret",
            "expires_at": 4102444800.0,
            "token_endpoint": "https://auth.example.test/token",
            "auth_endpoint": "https://auth.example.test/auth"
        },
        "extra_token": "extra-token",
        "nested": {
            "database_password": "db-password",
            "display_name": "Nested"
        }
    })
}

fn assert_encrypted_string(value: &Value, field: &str, expected_plaintext: &str) {
    assert!(EncryptedFieldIO::is_encrypted_value(value));
    assert_eq!(value["field"], field);
    assert!(!value.to_string().contains(expected_plaintext));
    assert_eq!(
        EncryptedFieldIO::decrypt_string(value, &KEY).expect("decrypt field"),
        expected_plaintext
    );
}
