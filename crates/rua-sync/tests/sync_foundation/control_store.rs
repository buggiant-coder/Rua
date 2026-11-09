use rua_sync::control_store::SyncControlStore;
use rua_sync::device_list::{DeviceEntry, DeviceList};
use rua_sync::network_migration_adapter::NetworkMigrationPayload;

fn sample_device() -> DeviceEntry {
    DeviceEntry {
        device_id: [1; 8],
        public_key: [1; 32],
        label: "phone".to_string(),
        friendly_name: "phone".to_string(),
        added_at_ms: 1,
        last_seen_ms: 2,
        direct_addrs: Vec::new(),
    }
}

#[test]
fn enabled_flag_roundtrips_atomically() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SyncControlStore::from_data_dir(tmp.path());

    assert!(!store.read_enabled());
    store.write_enabled(true).unwrap();
    assert!(store.read_enabled());
    store.write_enabled(false).unwrap();
    assert!(!store.read_enabled());
}

#[test]
fn device_list_roundtrips_and_removes() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SyncControlStore::from_data_dir(tmp.path());

    assert!(store.read_device_list().unwrap().devices.is_empty());
    store
        .write_device_list(&DeviceList {
            devices: vec![sample_device()],
        })
        .unwrap();
    assert_eq!(store.read_device_list().unwrap().devices.len(), 1);
    store.remove_device_list().unwrap();
    assert!(store.read_device_list().unwrap().devices.is_empty());
}

#[test]
fn pending_migration_marker_roundtrips_and_removes() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SyncControlStore::from_data_dir(tmp.path());
    store
        .write_pending_migration(&NetworkMigrationPayload {
            target_namespace_id: "target".to_string(),
            target_namespace_secret_bytes: vec![7; 32],
            migrated_at_ms: 42,
        })
        .unwrap();

    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(store.pending_migration_path()).unwrap())
            .unwrap();
    assert_eq!(content["target_namespace_id"], "target");
    assert_eq!(
        content["target_namespace_secret_bytes"],
        serde_json::json!(vec![7; 32])
    );

    store.remove_pending_migration().unwrap();
    assert!(!store.pending_migration_path().exists());
}
