use rua_config::secrets::{
    is_redacted, is_secret_field, merge_with_redaction_safety, redact_secrets,
};
use serde_json::{json, Map, Value};

fn object(value: Value) -> Map<String, Value> {
    value.as_object().expect("object").clone()
}

#[test]
fn secret_field_catalog_covers_provider_channel_auth_and_proxy_fields() {
    for field in [
        "api_key",
        "client_secret",
        "bot_token",
        "access_token",
        "auth_token",
        "secret",
        "app_secret",
        "password",
        "encrypt_key",
        "verification_token",
        "http_proxy_auth",
        "twilio_auth_token",
    ] {
        assert!(
            is_secret_field(field),
            "{field} should be treated as secret"
        );
    }

    for field in ["name", "base_url", "enabled", "workspace_dir"] {
        assert!(
            !is_secret_field(field),
            "{field} should remain a normal config field"
        );
    }
}

#[test]
fn redaction_sentinel_detection_accepts_qwenpaw_and_rust_mask_shapes() {
    for value in [
        "***",
        " ****** ",
        "REDACTED",
        "sk-***REDACTED***",
        "ab****cdef",
    ] {
        assert!(is_redacted(value), "{value:?} should preserve old secret");
    }

    for value in [
        "",
        "sk-plain-secret",
        "redacted-lowercase",
        "asterisk * only",
    ] {
        assert!(
            !is_redacted(value),
            "{value:?} should be accepted as a normal value"
        );
    }
}

#[test]
fn merge_with_redaction_safety_preserves_old_secret_for_mask_or_null() {
    let existing = object(json!({
        "api_key": "sk-existing",
        "client_secret": "client-existing",
        "base_url": "https://old.example.test",
        "enabled": true
    }));
    let incoming = object(json!({
        "api_key": "***",
        "client_secret": null,
        "base_url": "https://new.example.test",
        "enabled": null
    }));

    let merged = merge_with_redaction_safety(&existing, &incoming);

    assert_eq!(merged["api_key"], "sk-existing");
    assert_eq!(merged["client_secret"], "client-existing");
    assert_eq!(merged["base_url"], "https://new.example.test");
    assert!(
        merged["enabled"].is_null(),
        "non-secret null remains a real update"
    );
}

#[test]
fn merge_with_redaction_safety_recurses_into_nested_secret_objects() {
    let existing = object(json!({
        "oauth": {
            "client_secret": "oauth-existing",
            "scopes": ["read"]
        },
        "headers": {
            "Authorization": "Bearer old"
        }
    }));
    let incoming = object(json!({
        "oauth": {
            "client_secret": "******",
            "scopes": ["read", "write"]
        },
        "headers": {
            "Authorization": "Bearer new"
        }
    }));

    let merged = merge_with_redaction_safety(&existing, &incoming);

    assert_eq!(merged["oauth"]["client_secret"], "oauth-existing");
    assert_eq!(merged["oauth"]["scopes"], json!(["read", "write"]));
    assert_eq!(merged["headers"]["Authorization"], "Bearer new");
}

#[test]
fn redact_secrets_masks_non_empty_secret_fields_recursively() {
    let raw = json!({
        "api_key": "sk-secret",
        "empty_secret": "",
        "name": "provider",
        "nested": {
            "client_secret": "oauth-secret",
            "base_url": "https://api.example.test"
        },
        "servers": [
            {"bot_token": "bot-secret", "enabled": true},
            {"password": "", "enabled": false}
        ]
    });

    let redacted = redact_secrets(&raw);

    assert_eq!(redacted["api_key"], "***");
    assert_eq!(redacted["empty_secret"], "");
    assert_eq!(redacted["name"], "provider");
    assert_eq!(redacted["nested"]["client_secret"], "***");
    assert_eq!(redacted["nested"]["base_url"], "https://api.example.test");
    assert_eq!(redacted["servers"][0]["bot_token"], "***");
    assert_eq!(redacted["servers"][0]["enabled"], true);
    assert_eq!(redacted["servers"][1]["password"], "");
}
