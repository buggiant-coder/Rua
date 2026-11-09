use rua_sync::identity::NetworkSecret;

#[test]
fn network_secret_file_store_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let sync_dir = dir.path();

    let ns = NetworkSecret::generate();
    let original_bytes = ns.to_bytes();
    ns.save(sync_dir).unwrap();

    let loaded = NetworkSecret::load(sync_dir).unwrap().unwrap();
    assert_eq!(loaded.to_bytes(), original_bytes);
}

#[test]
fn network_secret_delete() {
    let dir = tempfile::tempdir().unwrap();
    let sync_dir = dir.path();

    let ns = NetworkSecret::generate();
    ns.save(sync_dir).unwrap();
    assert!(sync_dir.join("network_secret.key").exists());

    NetworkSecret::delete(sync_dir).unwrap();
    assert!(!sync_dir.join("network_secret.key").exists());
    assert!(NetworkSecret::load(sync_dir).unwrap().is_none());
}

#[test]
fn network_secret_delete_nonexistent_is_ok() {
    let dir = tempfile::tempdir().unwrap();
    NetworkSecret::delete(dir.path()).unwrap();
}
