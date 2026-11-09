use rua_sync::identity::{DeviceIdentity, NetworkSecret};

#[test]
fn network_secret_roundtrip_file() {
    let dir = tempfile::tempdir().unwrap();
    let sync_dir = dir.path();

    let ns = NetworkSecret::generate();
    ns.save(sync_dir).unwrap();

    let ns2 = NetworkSecret::load(sync_dir).unwrap().unwrap();
    assert_eq!(ns.to_bytes(), ns2.to_bytes());
}

#[test]
fn network_secret_generates_different_keys() {
    let n1 = NetworkSecret::generate();
    let n2 = NetworkSecret::generate();
    assert_ne!(n1.to_bytes(), n2.to_bytes());
}

#[test]
fn network_secret_from_bytes_roundtrip() {
    let original = NetworkSecret::generate();
    let bytes = original.to_bytes();
    let restored = NetworkSecret::from_bytes(&bytes);
    assert_eq!(original.to_bytes(), restored.to_bytes());
}

#[test]
fn network_secret_load_absent_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(NetworkSecret::load(dir.path()).unwrap().is_none());
}

#[test]
fn network_secret_derive_sync_key_deterministic() {
    let ns = NetworkSecret::generate();
    let k1 = ns.derive_sync_key();
    let k2 = ns.derive_sync_key();
    assert_eq!(k1.as_bytes(), k2.as_bytes());
}

#[test]
fn network_secret_derive_sync_key_preserves_persisted_protocol() {
    let ns = NetworkSecret::from_bytes(&std::array::from_fn(|i| i as u8));
    assert_eq!(
        hex::encode(ns.derive_sync_key().as_bytes()),
        "16081d43d5baa6257016f4305f8f7880bd00e853ccac9ec40f15f54973ce85bb"
    );
}

#[test]
fn device_identity_roundtrip() {
    let dir = tempfile::tempdir().unwrap();

    let device = DeviceIdentity::generate_or_load(dir.path()).unwrap();
    let pk1 = device.public_key();
    let id1 = device.device_id_short();
    std::fs::create_dir_all(dir.path().join("sync")).unwrap();
    std::fs::write(dir.path().join("sync/network_secret.key"), b"network state").unwrap();
    std::fs::remove_dir_all(dir.path().join("sync")).unwrap();

    let device2 = DeviceIdentity::generate_or_load(dir.path()).unwrap();
    assert_eq!(pk1.as_bytes(), device2.public_key().as_bytes());
    assert_eq!(id1, device2.device_id_short());
    assert!(dir.path().join("device_identity.json").is_file());
    assert!(!dir.path().join("sync/device_identity.json").exists());
}

#[test]
fn device_identity_short_id_is_8_bytes() {
    let device = DeviceIdentity::generate();
    let id = device.device_id_short();
    assert_eq!(id.len(), 8);
}

#[test]
fn device_to_iroh_author() {
    let device = DeviceIdentity::generate();
    let author = device.to_iroh_author();
    assert_eq!(author.to_bytes(), device.signing_key().to_bytes());
}
