use rua_config::encrypted_io::{EncryptedFieldIO, EncryptedFileIO};
use serde_json::json;

#[test]
fn encrypted_file_write_uses_binary_format() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secret.json");
    let key = [3_u8; 32];

    EncryptedFileIO::write_json(&path, &json!({ "api_key": "sk-test" }), Some(&key))
        .expect("write encrypted");

    let bytes = std::fs::read(&path).expect("read encrypted file");
    assert_eq!(bytes.first().copied(), Some(0x01));
    assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_err());
    assert!(!String::from_utf8_lossy(&bytes).contains("sk-test"));
    assert!(EncryptedFileIO::is_encrypted_file(&path).expect("detect"));

    let decrypted: serde_json::Value =
        EncryptedFileIO::read_json(&path, Some(&key)).expect("decrypt");
    assert_eq!(decrypted["api_key"], "sk-test");
}

#[test]
fn plaintext_read_still_works_with_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("plain.json");
    std::fs::write(&path, r#"{ "api_key": "legacy" }"#).expect("write plaintext");

    let value: serde_json::Value =
        EncryptedFileIO::read_json(&path, Some(&[9_u8; 32])).expect("read plaintext");
    assert_eq!(value["api_key"], "legacy");
    assert!(!EncryptedFileIO::is_encrypted_file(&path).expect("detect"));
}

#[test]
fn encrypted_file_without_key_fails_before_json_parse() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secret.json");

    EncryptedFileIO::write_json(&path, &json!({ "api_key": "sk-test" }), Some(&[3_u8; 32]))
        .expect("write encrypted");

    let err = EncryptedFileIO::read_json::<serde_json::Value>(&path, None)
        .expect_err("encrypted files require a key");

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        err.to_string().contains("no key"),
        "unexpected missing-key error: {err}"
    );
}

#[test]
fn corrupted_encrypted_file_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secret.json");
    let key = [3_u8; 32];

    EncryptedFileIO::write_json(&path, &json!({ "api_key": "sk-test" }), Some(&key))
        .expect("write encrypted");
    let mut bytes = std::fs::read(&path).expect("read encrypted file");
    let last = bytes.last_mut().expect("encrypted payload has bytes");
    *last ^= 0x01;
    std::fs::write(&path, bytes).expect("write corrupted encrypted file");

    let err = EncryptedFileIO::read_json::<serde_json::Value>(&path, Some(&key))
        .expect_err("corrupted encrypted file must not decrypt");
    assert!(
        err.to_string().contains("decrypt"),
        "expected decrypt error for corrupted ciphertext, got {err}"
    );
}

#[test]
fn encrypted_file_with_wrong_key_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secret.json");

    EncryptedFileIO::write_json(&path, &json!({ "api_key": "sk-test" }), Some(&[3_u8; 32]))
        .expect("write encrypted");

    let err = EncryptedFileIO::read_json::<serde_json::Value>(&path, Some(&[4_u8; 32]))
        .expect_err("wrong key must not produce plaintext");
    assert!(
        err.to_string().contains("decrypt"),
        "expected decrypt error for wrong key, got {err}"
    );
}

#[test]
fn encrypted_field_is_readable_json_without_plaintext_secret() {
    let key = [4_u8; 32];
    let field = EncryptedFieldIO::encrypt_string(
        "sk-field-secret",
        &key,
        "api_key",
        "Use `rua providers set-api-key demo` to update this API key.",
    )
    .expect("encrypt field");

    assert_eq!(field["rua_encrypted"], true);
    assert_eq!(field["field"], "api_key");
    assert!(field["notice"]
        .as_str()
        .expect("notice")
        .contains("Other JSON fields may be edited normally"));
    assert!(!field.to_string().contains("sk-field-secret"));
    assert_eq!(
        EncryptedFieldIO::decrypt_string(&field, &key).expect("decrypt field"),
        "sk-field-secret"
    );
}

#[test]
fn encrypted_field_roundtrips_unicode_without_plaintext_leakage() {
    let key = [7_u8; 32];
    let secret = "你好世界";

    let field = EncryptedFieldIO::encrypt_string(
        secret,
        &key,
        "api_key",
        "Use `rua providers set-api-key demo` to update this API key.",
    )
    .expect("encrypt field");

    assert!(EncryptedFieldIO::is_encrypted_value(&field));
    assert!(
        !field.to_string().contains(secret),
        "encrypted field envelope must not contain plaintext unicode secret"
    );
    assert_eq!(
        EncryptedFieldIO::decrypt_string(&field, &key).expect("decrypt field"),
        secret
    );
}

#[test]
fn encrypted_field_with_wrong_key_fails_closed() {
    let field = EncryptedFieldIO::encrypt_string(
        "sk-field-secret",
        &[4_u8; 32],
        "api_key",
        "Use `rua providers set-api-key demo` to update this API key.",
    )
    .expect("encrypt field");

    let err = EncryptedFieldIO::decrypt_string(&field, &[5_u8; 32])
        .expect_err("wrong key must not decrypt field");
    assert!(
        err.to_string().contains("decrypt"),
        "expected decrypt error for wrong field key, got {err}"
    );
}

#[test]
fn malformed_encrypted_field_fails_closed() {
    let field = json!({
        "rua_encrypted": true,
        "version": 1,
        "nonce": "not-base64",
        "ciphertext": "also-not-base64"
    });

    let err = EncryptedFieldIO::decrypt_string(&field, &[4_u8; 32])
        .expect_err("malformed encrypted field must not decrypt");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}
