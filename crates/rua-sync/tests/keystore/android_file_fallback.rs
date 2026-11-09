use rua_sync::identity::NetworkSecret;
use tempfile::TempDir;

#[test]
fn network_secret_file_persistence_roundtrip() {
    let dir = TempDir::new().unwrap();
    let sync_dir = dir.path();

    assert!(NetworkSecret::load(sync_dir).unwrap().is_none());

    let ns = NetworkSecret::generate();
    let original_bytes = ns.to_bytes();
    ns.save(sync_dir).unwrap();

    let key_path = sync_dir.join("network_secret.key");
    assert!(key_path.exists());
    let content = std::fs::read_to_string(&key_path).unwrap();
    assert_eq!(content.len(), 64);

    let loaded = NetworkSecret::load(sync_dir).unwrap().expect("should load");
    assert_eq!(loaded.to_bytes(), original_bytes);
}

#[test]
fn network_secret_saves_to_nested_dir() {
    let dir = TempDir::new().unwrap();
    let sync_dir = dir.path().join("nested").join("sync");

    let ns = NetworkSecret::generate();
    ns.save(&sync_dir).unwrap();
    assert!(sync_dir.join("network_secret.key").exists());
}
