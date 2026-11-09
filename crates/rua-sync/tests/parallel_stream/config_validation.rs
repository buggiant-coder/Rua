use rua_sync::sync_blob_store::SyncBlobFetchConfig;

#[test]
fn valid_parallelism_values() {
    for v in [1, 2, 4, 8, 16] {
        let cfg = SyncBlobFetchConfig::new(v).unwrap();
        assert_eq!(cfg.parallel_streams, v);
    }
}

#[test]
fn invalid_parallelism_values_rejected() {
    for v in [0, 3, 5, 6, 7, 9, 10, 15, 17, 32, 64, 255] {
        assert!(SyncBlobFetchConfig::new(v).is_err());
    }
}

#[test]
fn default_parallelism_is_4() {
    let cfg = SyncBlobFetchConfig::default();
    assert_eq!(cfg.parallel_streams, 4);
}
