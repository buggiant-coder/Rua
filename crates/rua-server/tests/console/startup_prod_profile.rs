use rua_sync::identity::NetworkSecret;

#[test]
fn network_secret_file_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let sync_dir = dir.path().join("sync");

    assert!(NetworkSecret::load(&sync_dir).unwrap().is_none());

    let ns = NetworkSecret::generate();
    ns.save(&sync_dir).unwrap();

    let loaded = NetworkSecret::load(&sync_dir).unwrap().unwrap();
    assert_eq!(loaded.to_bytes(), ns.to_bytes());
}
