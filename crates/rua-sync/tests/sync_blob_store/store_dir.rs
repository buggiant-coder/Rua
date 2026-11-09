use rua_sync::sync_blob_store::resolve_sync_blob_store_dir;

#[test]
fn resolves_new_blob_store_dir_by_default() {
    let dir = tempfile::tempdir().unwrap();

    assert_eq!(
        resolve_sync_blob_store_dir(dir.path()),
        dir.path().join("blobs")
    );
}

#[test]
fn resolves_legacy_iroh_blobs_dir_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let legacy_data = dir.path().join("iroh-blobs").join("data");
    std::fs::create_dir_all(&legacy_data).unwrap();

    assert_eq!(
        resolve_sync_blob_store_dir(dir.path()),
        dir.path().join("iroh-blobs")
    );
}
