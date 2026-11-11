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
            ..Default::default()
        })
        .unwrap();
    assert_eq!(store.read_device_list().unwrap().devices.len(), 1);
    store.remove_device_list().unwrap();
    assert!(store.read_device_list().unwrap().devices.is_empty());
}

#[test]
fn committed_name_succeeds_observed_revision_after_restart_and_clock_skew() {
    use rua_sync::device_list::DeviceNameRecord;
    use rua_sync::hlc::HlcTimestamp;

    let tmp = tempfile::tempdir().unwrap();
    let store = SyncControlStore::from_data_dir(tmp.path());
    let observed = HlcTimestamp {
        wall_ms: u64::MAX - 1,
        counter: u32::MAX,
        device_id: [9; 8],
    };
    let mut list = DeviceList {
        devices: vec![sample_device()],
        ..Default::default()
    };
    list.extra.insert(
        "future_list_metadata".into(),
        serde_json::json!({"kept": true}),
    );
    let mut previous = DeviceNameRecord {
        friendly_name: "remote name".into(),
        revision: observed.to_sortable_bytes(),
        ..Default::default()
    };
    previous
        .extra
        .insert("future_name_metadata".into(), serde_json::json!(42));
    list.name_records.insert(hex::encode([1; 8]), previous);
    store.write_device_list(&list).unwrap();

    let restarted = SyncControlStore::from_data_dir(tmp.path());
    let committed = restarted
        .commit_device_name(&[1; 8], "local name", [2; 8])
        .unwrap()
        .unwrap();
    assert!(HlcTimestamp::from_sortable_bytes(&committed.revision) > observed);
    assert_eq!(committed.extra["future_name_metadata"], 42);
    let stored = restarted.read_device_list().unwrap();
    assert_eq!(stored.devices[0].friendly_name, "local name");
    assert_eq!(stored.name_records[&hex::encode([1; 8])], committed);
    assert_eq!(stored.extra["future_list_metadata"]["kept"], true);
}

#[test]
fn pairing_merges_latest_name_facts_without_restoring_membership() {
    use rua_sync::device_list::DeviceNameRecord;
    use rua_sync::hlc::HlcTimestamp;

    let mut captured = DeviceList {
        devices: vec![sample_device()],
        ..Default::default()
    };
    let mut current = captured.clone();
    current.devices[0].friendly_name = "latest edit".into();
    let record = DeviceNameRecord {
        friendly_name: "latest edit".into(),
        revision: HlcTimestamp {
            wall_ms: 200,
            counter: 0,
            device_id: [1; 8],
        }
        .to_sortable_bytes(),
        ..Default::default()
    };
    current
        .name_records
        .insert(hex::encode([1; 8]), record.clone());
    let mut excluded = sample_device();
    excluded.device_id = [2; 8];
    current.devices.push(excluded);
    current
        .name_records
        .insert(hex::encode([2; 8]), record.clone());

    captured.merge_name_facts_from(&current);
    assert_eq!(captured.devices.len(), 1);
    assert_eq!(captured.devices[0].friendly_name, "latest edit");
    assert_eq!(captured.name_records[&hex::encode([1; 8])], record);
    assert!(!captured.name_records.contains_key(&hex::encode([2; 8])));

    // A later pairing result also retains the newest fact it already carries.
    let old = current.clone();
    captured
        .name_records
        .get_mut(&hex::encode([1; 8]))
        .unwrap()
        .revision[7] = 201;
    captured
        .name_records
        .get_mut(&hex::encode([1; 8]))
        .unwrap()
        .friendly_name = "newer result".into();
    captured.merge_name_facts_from(&old);
    assert_eq!(captured.devices[0].friendly_name, "newer result");
}

#[test]
fn pairing_commit_imports_legacy_saved_name_before_replacing_observed_default() {
    let temp = tempfile::tempdir().unwrap();
    let store = SyncControlStore::from_data_dir(temp.path());
    std::fs::create_dir_all(store.device_list_path().parent().unwrap()).unwrap();
    let mut saved = sample_device();
    saved.friendly_name = "razr".into();
    std::fs::write(
        store.device_list_path(),
        serde_json::to_vec(&serde_json::json!({
            "devices": [saved],
            "future_pairing_metadata": true,
        }))
        .unwrap(),
    )
    .unwrap();
    let mut observed = sample_device();
    observed.friendly_name = "android-01010101".into();
    let mut pairing_result = DeviceList {
        devices: vec![observed],
        ..Default::default()
    };

    let current = store.read_device_list().unwrap();
    pairing_result.merge_name_facts_from(&current);
    store.write_device_list(&pairing_result).unwrap();

    let committed = store.read_device_list().unwrap();
    assert_eq!(committed.devices[0].friendly_name, "razr");
    assert_eq!(
        committed.name_records["0101010101010101"].friendly_name,
        "razr"
    );
    assert_eq!(committed.name_schema_version, 1);
    assert_eq!(committed.extra["future_pairing_metadata"], true);
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
