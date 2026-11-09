use rua_sync::export::{decrypt_export, encrypt_for_export};
use rua_sync::identity::NetworkSecret;

#[test]
fn encrypt_decrypt_roundtrip() {
    let ns = NetworkSecret::generate();
    let key_bytes = ns.to_bytes();
    let passphrase = "test-pass-phrase-123";

    let armored = encrypt_for_export(&key_bytes, passphrase).unwrap();

    assert!(armored.contains("-----BEGIN AGE ENCRYPTED FILE-----"));
    assert!(armored.contains("-----END AGE ENCRYPTED FILE-----"));

    let recovered = decrypt_export(&armored, passphrase).unwrap();
    assert_eq!(key_bytes, recovered);
}

#[test]
fn wrong_passphrase_fails() {
    let ns = NetworkSecret::generate();
    let key_bytes = ns.to_bytes();

    let armored = encrypt_for_export(&key_bytes, "correct-pass").unwrap();
    let result = decrypt_export(&armored, "wrong-pass");
    assert!(result.is_err());
}

#[test]
fn short_passphrase_still_works_at_crypto_level() {
    let key_bytes = [42u8; 32];
    let armored = encrypt_for_export(&key_bytes, "ab").unwrap();
    let recovered = decrypt_export(&armored, "ab").unwrap();
    assert_eq!(key_bytes, recovered);
}
