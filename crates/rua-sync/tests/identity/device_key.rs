use rua_sync::crypto::DeviceKey;

#[test]
fn generate_or_load_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let secret_dir = dir.path().join("secret");

    let key = DeviceKey::generate_or_load(&secret_dir).unwrap();
    let path = secret_dir.join("device_key.key");
    assert!(path.exists());

    let hex_str = std::fs::read_to_string(&path).unwrap();
    let decoded = hex::decode(hex_str.trim()).unwrap();
    assert_eq!(decoded.len(), 32);
    assert_eq!(decoded.as_slice(), key.as_bytes().as_slice());
}

#[test]
fn generate_or_load_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let secret_dir = dir.path().join("secret");

    let key1 = DeviceKey::generate_or_load(&secret_dir).unwrap();
    let key2 = DeviceKey::generate_or_load(&secret_dir).unwrap();
    assert_eq!(key1.as_bytes(), key2.as_bytes());
}

#[test]
fn generate_produces_different_keys() {
    let k1 = DeviceKey::generate();
    let k2 = DeviceKey::generate();
    assert_ne!(k1.as_bytes(), k2.as_bytes());
}

#[test]
fn from_bytes_roundtrip() {
    let original = DeviceKey::generate();
    let bytes = *original.as_bytes();
    let restored = DeviceKey::from_bytes(bytes);
    assert_eq!(original.as_bytes(), restored.as_bytes());
}

#[cfg(unix)]
#[test]
fn file_permissions_are_600() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let secret_dir = dir.path().join("secret");

    let _key = DeviceKey::generate_or_load(&secret_dir).unwrap();
    let path = secret_dir.join("device_key.key");

    let perms = std::fs::metadata(&path).unwrap().permissions();
    assert_eq!(perms.mode() & 0o777, 0o600);
}
