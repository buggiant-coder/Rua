use rua_config::encrypted_io::EncryptedFieldIO;
use rua_config::secret_projection::{
    contains_encrypted_fields, decrypt_secret_fields, encrypt_secret_fields, field_path,
    SecretFieldPolicy, SecretProjectionKind, SecretProjectionMatch,
};
use serde_json::{json, Value};

const KEY: [u8; 32] = [8; 32];

struct RecursivePolicy;

impl SecretFieldPolicy for RecursivePolicy {
    fn classify(
        &self,
        path: &[String],
        current_key: Option<&str>,
        value: &Value,
    ) -> Option<SecretProjectionMatch> {
        if value.as_str().is_some() && current_key.is_some_and(|key| key.contains("secret")) {
            return Some(SecretProjectionMatch::string(field_path(
                path,
                current_key,
                "secret",
            )));
        }
        None
    }

    fn tool_hint(&self, scope_hint: &str, field: &str) -> String {
        format!("update {field} in {scope_hint}")
    }

    fn missing_key_error(&self, _field: &str) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "missing test key")
    }

    fn unclassified_encrypted_kind(&self) -> Option<SecretProjectionKind> {
        Some(SecretProjectionKind::String)
    }
}

struct JsonPolicy;

impl SecretFieldPolicy for JsonPolicy {
    fn classify(
        &self,
        path: &[String],
        current_key: Option<&str>,
        value: &Value,
    ) -> Option<SecretProjectionMatch> {
        match current_key {
            Some("api_keys") if !value.is_null() => Some(SecretProjectionMatch::json("api_keys")),
            Some("api_key")
                if EncryptedFieldIO::is_encrypted_value(value)
                    || value.as_str().is_some_and(|s| !s.is_empty()) =>
            {
                Some(SecretProjectionMatch::string("api_key"))
            }
            Some(name)
                if path
                    .len()
                    .checked_sub(2)
                    .and_then(|idx| path.get(idx))
                    .is_some_and(|parent| parent == "headers")
                    && name.eq_ignore_ascii_case("authorization")
                    && (value.as_str().is_some()
                        || EncryptedFieldIO::is_encrypted_value(value)) =>
            {
                Some(SecretProjectionMatch::string(format!("headers.{name}")))
            }
            _ => None,
        }
    }

    fn tool_hint(&self, scope_hint: &str, field: &str) -> String {
        format!("update {field} in {scope_hint}")
    }

    fn missing_key_error(&self, field: &str) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{field} encrypted but no key"),
        )
    }
}

#[test]
fn recursive_string_projection_encrypts_and_decrypts_nested_values() {
    let raw = json!({
        "plain": "visible",
        "nested": {
            "client_secret": "hidden",
            "label": "shown"
        },
        "items": [
            { "api_secret": "array-hidden" },
            { "name": "array-shown" }
        ]
    });

    let encrypted =
        encrypt_secret_fields(&raw, Some(&KEY), "scope", &RecursivePolicy).expect("encrypt");

    assert_eq!(encrypted["plain"], "visible");
    assert_eq!(encrypted["nested"]["label"], "shown");
    assert_encrypted(
        &encrypted["nested"]["client_secret"],
        "nested.client_secret",
        "hidden",
    );
    assert_encrypted(
        &encrypted["items"][0]["api_secret"],
        "items.api_secret",
        "array-hidden",
    );
    assert_eq!(encrypted["items"][1]["name"], "array-shown");
    assert!(contains_encrypted_fields(&encrypted));

    let read = decrypt_secret_fields(&encrypted, Some(&KEY), &RecursivePolicy).expect("decrypt");
    assert_eq!(read.value, raw);
    assert_eq!(read.encrypted_fields, 2);
}

#[test]
fn encryption_is_idempotent_for_existing_envelopes() {
    let raw = json!({ "client_secret": "hidden" });
    let encrypted_once =
        encrypt_secret_fields(&raw, Some(&KEY), "scope", &RecursivePolicy).expect("encrypt");
    let envelope = encrypted_once["client_secret"].clone();

    let encrypted_twice =
        encrypt_secret_fields(&encrypted_once, Some(&KEY), "scope", &RecursivePolicy)
            .expect("encrypt twice");

    assert_eq!(encrypted_twice["client_secret"], envelope);
}

#[test]
fn decrypt_plaintext_without_key_succeeds_but_encrypted_without_key_fails() {
    let raw = json!({ "client_secret": "hidden" });
    let plaintext = decrypt_secret_fields(&raw, None, &RecursivePolicy).expect("plaintext");
    assert_eq!(plaintext.value, raw);
    assert_eq!(plaintext.encrypted_fields, 0);

    let encrypted =
        encrypt_secret_fields(&raw, Some(&KEY), "scope", &RecursivePolicy).expect("encrypt");
    let err = decrypt_secret_fields(&encrypted, None, &RecursivePolicy)
        .expect_err("encrypted field without key should fail");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(err.to_string().contains("missing test key"));
}

#[test]
fn decrypts_mixed_plaintext_and_encrypted_values() {
    let raw = json!({
        "client_secret": "hidden",
        "other_secret": "other"
    });
    let encrypted =
        encrypt_secret_fields(&raw, Some(&KEY), "scope", &RecursivePolicy).expect("encrypt");
    let mixed = json!({
        "client_secret": encrypted["client_secret"].clone(),
        "other_secret": "other"
    });

    let read = decrypt_secret_fields(&mixed, Some(&KEY), &RecursivePolicy).expect("decrypt");

    assert_eq!(read.value, raw);
    assert_eq!(read.encrypted_fields, 1);
}

#[test]
fn json_secret_kind_roundtrips_non_string_values() {
    let raw = json!({
        "api_key": "primary",
        "api_keys": [
            { "name": "a", "value": "sk-a" },
            { "name": "b", "value": "sk-b" }
        ],
        "headers": {
            "Authorization": "Bearer token",
            "User-Agent": "rua"
        }
    });

    let encrypted = encrypt_secret_fields(&raw, Some(&KEY), "provider", &JsonPolicy)
        .expect("encrypt json secret");

    assert_encrypted(&encrypted["api_key"], "api_key", "primary");
    assert!(EncryptedFieldIO::is_encrypted_value(&encrypted["api_keys"]));
    assert_encrypted(
        &encrypted["headers"]["Authorization"],
        "headers.Authorization",
        "Bearer token",
    );
    assert_eq!(encrypted["headers"]["User-Agent"], "rua");

    let read = decrypt_secret_fields(&encrypted, Some(&KEY), &JsonPolicy).expect("decrypt");
    assert_eq!(read.value, raw);
    assert_eq!(read.encrypted_fields, 3);
}

#[test]
fn unclassified_envelopes_can_be_preserved() {
    let raw = json!({ "client_secret": "hidden" });
    let encrypted =
        encrypt_secret_fields(&raw, Some(&KEY), "scope", &RecursivePolicy).expect("encrypt");

    let read = decrypt_secret_fields(&encrypted, Some(&KEY), &JsonPolicy).expect("decrypt");

    assert_eq!(read.value, encrypted);
    assert_eq!(read.encrypted_fields, 0);
}

fn assert_encrypted(value: &Value, field: &str, expected_plaintext: &str) {
    assert!(EncryptedFieldIO::is_encrypted_value(value));
    assert_eq!(value["field"], field);
    assert!(!value.to_string().contains(expected_plaintext));
    assert_eq!(
        EncryptedFieldIO::decrypt_string(value, &KEY).expect("decrypt field"),
        expected_plaintext
    );
}
