use std::sync::Arc;

use rua_sync::device_list::{DeviceEntry, DeviceList};
use rua_sync::device_list_adapter::{
    DeviceListSyncAdapter, DEVICE_LIST_DOMAIN, DEVICE_LIST_KIND_REMOVE, DEVICE_LIST_KIND_RENAME,
    DEVICE_LIST_KIND_SNAPSHOT,
};
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::sidecar::RecordState;
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn sample_device(id_byte: u8, name: &str) -> DeviceEntry {
    DeviceEntry {
        device_id: [id_byte; 8],
        public_key: [id_byte; 32],
        label: name.to_string(),
        friendly_name: name.to_string(),
        added_at_ms: 1_000,
        last_seen_ms: 1_000,
        direct_addrs: Vec::new(),
    }
}

fn write_device_list(data_dir: &std::path::Path, devices: Vec<DeviceEntry>) {
    let path = data_dir.join("sync").join("device_list.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&DeviceList {
            devices,
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
}

fn read_device_list(data_dir: &std::path::Path) -> DeviceList {
    serde_json::from_str(
        &std::fs::read_to_string(data_dir.join("sync").join("device_list.json")).unwrap(),
    )
    .unwrap()
}

fn write_pre_upgrade_device_list(data_dir: &std::path::Path, devices: Vec<DeviceEntry>) {
    let path = data_dir.join("sync/device_list.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        serde_json::to_vec(&serde_json::json!({
            "devices": devices,
            "future_saved_metadata": {"preserved": true},
        }))
        .unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn legacy_saved_phone_name_beats_phone_default_received_first() {
    let mac = tempfile::tempdir().unwrap();
    let windows = tempfile::tempdir().unwrap();
    let mut phone = sample_device(2, "razr");
    phone.label = "android".into();
    phone.public_key = [9; 32];
    write_pre_upgrade_device_list(mac.path(), vec![phone.clone()]);
    let (registry, store) = registry_with_adapter(windows.path()).await;
    phone.friendly_name = "android-02020202".into();
    let first = DeviceListSyncAdapter::snapshot_change(
        "phone-default-first",
        phone.clone(),
        "0202020202020202",
        hlc(1_000),
    );
    registry.apply_change(&first, &store).await.unwrap();
    let exporter = DeviceListSyncAdapter::new(mac.path().to_path_buf(), "mac");
    for change in exporter.export_records().await.unwrap() {
        registry.apply_change(&change, &store).await.unwrap();
    }
    assert_eq!(
        read_device_list(windows.path()).devices[0].friendly_name,
        "razr"
    );
    let migrated = read_device_list(mac.path());
    let record = migrated
        .name_records
        .get("0202020202020202")
        .expect("saved name must gain durable provenance");
    assert_eq!(
        HlcTimestamp::from_sortable_bytes(&record.revision).wall_ms,
        0
    );
    assert_eq!(migrated.extra["future_saved_metadata"]["preserved"], true);
    let later_default = DeviceListSyncAdapter::snapshot_change(
        "phone-default-again",
        phone,
        "0202020202020202",
        hlc(2_000),
    );
    registry.apply_change(&later_default, &store).await.unwrap();
    assert_eq!(
        read_device_list(windows.path()).devices[0].friendly_name,
        "razr"
    );
}

#[tokio::test]
async fn legacy_generated_default_with_main_cursor_accepts_saved_name_baseline() {
    let mac = tempfile::tempdir().unwrap();
    let windows = tempfile::tempdir().unwrap();
    let mut phone = sample_device(2, "razr");
    phone.label = "android".into();
    phone.public_key = [9; 32];
    write_pre_upgrade_device_list(mac.path(), vec![phone.clone()]);
    phone.friendly_name = "android-02020202".into();
    write_pre_upgrade_device_list(windows.path(), vec![phone]);
    let (registry, store) = registry_with_adapter(windows.path()).await;
    store
        .set_record_cursor(
            "device_list:0202020202020202",
            &rua_sync::merge_store::live_cursor(hlc(200), None),
        )
        .unwrap();
    let exporter = DeviceListSyncAdapter::new(mac.path().to_path_buf(), "mac");
    for change in exporter.export_records().await.unwrap() {
        registry.apply_change(&change, &store).await.unwrap();
    }
    assert_eq!(
        read_device_list(windows.path()).devices[0].friendly_name,
        "razr"
    );
    let explicit = DeviceListSyncAdapter::rename_change(
        "explicit-name",
        "0202020202020202",
        "My Phone",
        "windows",
        hlc(300),
    );
    registry.apply_change(&explicit, &store).await.unwrap();
    for change in exporter.export_records().await.unwrap() {
        registry.apply_change(&change, &store).await.unwrap();
    }
    assert_eq!(
        read_device_list(windows.path()).devices[0].friendly_name,
        "My Phone"
    );
}

#[tokio::test]
async fn legacy_saved_custom_name_retains_frontier_and_does_not_gain_export_time() {
    let data = tempfile::tempdir().unwrap();
    write_pre_upgrade_device_list(data.path(), vec![sample_device(1, "My Mac")]);
    let (registry, store) = registry_with_adapter(data.path()).await;
    store
        .set_record_cursor(
            "device_list:0101010101010101",
            &rua_sync::merge_store::live_cursor(hlc(200), None),
        )
        .unwrap();
    let exporter = DeviceListSyncAdapter::new(data.path().to_path_buf(), "mac");
    let first = exporter.export_records().await.unwrap();
    let baseline = read_device_list(data.path()).name_records["0101010101010101"].clone();
    let delayed = DeviceListSyncAdapter::rename_change(
        "legacy-delayed",
        "0101010101010101",
        "old name",
        "peer",
        hlc(100),
    );
    assert_eq!(
        registry
            .apply_change(&delayed, &store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    let control = rua_sync::control_store::SyncControlStore::from_data_dir(data.path());
    let mut list = control.read_device_list().unwrap();
    list.devices[0].last_seen_ms += 100;
    control.write_device_list(&list).unwrap();
    let second = exporter.export_records().await.unwrap();
    assert_eq!(
        first[0].payload["name_record"],
        second[0].payload["name_record"]
    );
    assert_eq!(
        read_device_list(data.path()).name_records["0101010101010101"],
        baseline
    );
    assert_eq!(
        read_device_list(data.path()).devices[0].friendly_name,
        "My Mac"
    );
}

#[tokio::test]
async fn conflicting_legacy_custom_baselines_do_not_overwrite_saved_local_name() {
    let sender = tempfile::tempdir().unwrap();
    let receiver = tempfile::tempdir().unwrap();
    write_pre_upgrade_device_list(sender.path(), vec![sample_device(1, "Old Hostname")]);
    write_pre_upgrade_device_list(receiver.path(), vec![sample_device(1, "My Mac")]);
    let (registry, store) = registry_with_adapter(receiver.path()).await;
    let exporter = DeviceListSyncAdapter::new(sender.path().to_path_buf(), "observer");
    for change in exporter.export_records().await.unwrap() {
        registry.apply_change(&change, &store).await.unwrap();
    }
    assert_eq!(
        read_device_list(receiver.path()).devices[0].friendly_name,
        "My Mac"
    );
}

async fn registry_with_adapter(data_dir: &std::path::Path) -> (SyncRegistry, MergeStore) {
    let merge_dir = data_dir.join("merge");
    let merge_store = MergeStore::open(&merge_dir).unwrap();
    let adapter = Arc::new(DeviceListSyncAdapter::new(
        data_dir.to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    (registry, merge_store)
}

#[tokio::test]
async fn export_reads_device_list_as_snapshots() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(
        data.path(),
        vec![sample_device(2, "tablet"), sample_device(1, "phone")],
    );
    let adapter = DeviceListSyncAdapter::new(data.path().to_path_buf(), "device-a");

    let exported = adapter.export_records().await.unwrap();
    let ids: Vec<_> = exported
        .iter()
        .map(|change| change.record_id.as_str())
        .collect();

    assert_eq!(ids, vec!["0101010101010101", "0202020202020202"]);
    assert!(exported
        .iter()
        .all(|change| change.domain == DEVICE_LIST_DOMAIN));
    assert!(exported
        .iter()
        .all(|change| change.kind == DEVICE_LIST_KIND_SNAPSHOT));
}

#[tokio::test]
async fn remove_change_deletes_device_and_commits_tombstone() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(
        data.path(),
        vec![sample_device(1, "phone"), sample_device(2, "tablet")],
    );
    let (registry, merge_store) = registry_with_adapter(data.path()).await;
    let change =
        DeviceListSyncAdapter::remove_change("op-remove", "0202020202020202", "device-a", hlc(100));

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let list = read_device_list(data.path());
    assert_eq!(list.devices.len(), 1);
    assert!(list.contains_device(&[1; 8]));
    assert!(!list.contains_device(&[2; 8]));
    let cursor = merge_store
        .get_record_cursor("device_list:0202020202020202")
        .unwrap()
        .unwrap();
    assert_eq!(cursor.state, RecordState::Tombstoned);
}

#[tokio::test]
async fn snapshot_change_adds_or_updates_device_presence() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(data.path(), vec![sample_device(1, "phone")]);
    let (registry, merge_store) = registry_with_adapter(data.path()).await;
    let mut remote = sample_device(2, "tablet");
    remote.last_seen_ms = 2_000;
    remote.direct_addrs = vec!["127.0.0.1:1234".parse().unwrap()];
    let change =
        DeviceListSyncAdapter::snapshot_change("op-snapshot", remote, "device-a", hlc(100));

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let list = read_device_list(data.path());
    assert_eq!(list.devices.len(), 2);
    let tablet = list
        .devices
        .iter()
        .find(|entry| entry.device_id == [2; 8])
        .unwrap();
    assert_eq!(tablet.friendly_name, "tablet");
    assert_eq!(tablet.last_seen_ms, 2_000);
    assert_eq!(tablet.direct_addrs.len(), 1);
}

#[tokio::test]
async fn rename_change_updates_friendly_name() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(data.path(), vec![sample_device(1, "phone")]);
    let (registry, merge_store) = registry_with_adapter(data.path()).await;
    let change = DeviceListSyncAdapter::rename_change(
        "op-rename",
        "0101010101010101",
        "daily phone",
        "device-a",
        hlc(100),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let list = read_device_list(data.path());
    assert_eq!(list.devices[0].friendly_name, "daily phone");
}

#[tokio::test]
async fn stale_remove_is_skipped_by_lww() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(data.path(), vec![sample_device(1, "phone")]);
    let (registry, merge_store) = registry_with_adapter(data.path()).await;
    let fresh = DeviceListSyncAdapter::rename_change(
        "op-fresh",
        "0101010101010101",
        "fresh name",
        "device-a",
        hlc(200),
    );
    registry.apply_change(&fresh, &merge_store).await.unwrap();

    let stale =
        DeviceListSyncAdapter::remove_change("op-stale", "0101010101010101", "device-a", hlc(100));
    let result = registry.apply_change(&stale, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Skipped);
    let list = read_device_list(data.path());
    assert!(list.contains_device(&[1; 8]));
}

#[tokio::test]
async fn stale_snapshot_does_not_rollback_newer_device_rename() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(data.path(), vec![sample_device(1, "phone")]);
    let (registry, merge_store) = registry_with_adapter(data.path()).await;
    let fresh = DeviceListSyncAdapter::rename_change(
        "op-fresh",
        "0101010101010101",
        "fresh name",
        "device-a",
        hlc(200),
    );
    registry.apply_change(&fresh, &merge_store).await.unwrap();

    let stale = DeviceListSyncAdapter::snapshot_change(
        "op-stale",
        sample_device(1, "old phone"),
        "device-a",
        hlc(100),
    );
    let result = registry.apply_change(&stale, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Skipped);
    let list = read_device_list(data.path());
    assert_eq!(list.devices[0].friendly_name, "fresh name");
}

#[test]
fn remove_change_uses_opaque_op_id_not_device_id() {
    let change = DeviceListSyncAdapter::remove_change(
        DeviceListSyncAdapter::new_op_id(),
        "0101010101010101",
        "device-a",
        hlc(100),
    );

    assert_eq!(change.kind, DEVICE_LIST_KIND_REMOVE);
    assert!(!change.op_id.contains("0101010101010101"));
}

#[tokio::test]
async fn observer_snapshot_does_not_overwrite_a_custom_device_name() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(data.path(), vec![sample_device(1, "My Mac")]);
    let (registry, merge_store) = registry_with_adapter(data.path()).await;
    let mut observed = sample_device(1, "default-hostname");
    observed.last_seen_ms = 9_000;
    let change = DeviceListSyncAdapter::snapshot_change(
        "op-observer",
        observed,
        "new-windows-device",
        hlc(300),
    );
    registry.apply_change(&change, &merge_store).await.unwrap();
    let list = read_device_list(data.path());
    assert_eq!(list.devices[0].friendly_name, "My Mac");
    assert_eq!(list.devices[0].last_seen_ms, 9_000);
    // An explicit rename remains effective after a presence update.
    let rename = DeviceListSyncAdapter::rename_change(
        "op-user-rename",
        "0101010101010101",
        "Renamed Mac",
        "device-a",
        hlc(200),
    );
    registry.apply_change(&rename, &merge_store).await.unwrap();
    assert_eq!(
        read_device_list(data.path()).devices[0].friendly_name,
        "Renamed Mac"
    );
}

#[tokio::test]
async fn first_sync_adds_third_device_without_replacing_known_names() {
    let mac = tempfile::tempdir().unwrap();
    let windows = tempfile::tempdir().unwrap();
    write_device_list(
        mac.path(),
        vec![
            sample_device(1, "My Mac"),
            sample_device(2, "My Phone"),
            sample_device(3, "Windows"),
        ],
    );
    write_device_list(
        windows.path(),
        vec![sample_device(1, "My Mac"), sample_device(3, "My PC")],
    );
    let adapter = DeviceListSyncAdapter::new(mac.path().to_path_buf(), "0101010101010101");
    let (registry, merge_store) = registry_with_adapter(windows.path()).await;
    for change in adapter.export_records().await.unwrap() {
        registry.apply_change(&change, &merge_store).await.unwrap();
    }
    let list = read_device_list(windows.path());
    assert_eq!(list.devices.len(), 3);
    assert_eq!(
        list.devices
            .iter()
            .find(|d| d.device_id == [2; 8])
            .unwrap()
            .friendly_name,
        "My Phone"
    );
    assert_eq!(
        list.devices
            .iter()
            .find(|d| d.device_id == [3; 8])
            .unwrap()
            .friendly_name,
        "My PC"
    );
}

#[tokio::test]
async fn device_name_version_is_independent_of_observer_presence_and_respects_removal() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(data.path(), vec![sample_device(1, "Default Mac")]);
    let (registry, merge_store) = registry_with_adapter(data.path()).await;
    let observed = DeviceListSyncAdapter::snapshot_change(
        "observer",
        sample_device(1, "stale name"),
        "windows",
        hlc(300),
    );
    registry
        .apply_change(&observed, &merge_store)
        .await
        .unwrap();
    let mut owner = DeviceListSyncAdapter::snapshot_change(
        "owner",
        sample_device(1, "Custom Mac"),
        "0101010101010101",
        hlc(200),
    );
    owner.payload["name_record"] = serde_json::to_value(rua_sync::device_list::DeviceNameRecord {
        friendly_name: "Custom Mac".into(),
        revision: hlc(200).to_sortable_bytes(),
        ..Default::default()
    })
    .unwrap();
    registry.apply_change(&owner, &merge_store).await.unwrap();
    assert_eq!(
        read_device_list(data.path()).devices[0].friendly_name,
        "Custom Mac"
    );
    let stale = DeviceListSyncAdapter::rename_change(
        "stale-rename",
        "0101010101010101",
        "old name",
        "phone",
        hlc(100),
    );
    assert_eq!(
        registry
            .apply_change(&stale, &merge_store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    let remove =
        DeviceListSyncAdapter::remove_change("remove", "0101010101010101", "phone", hlc(400));
    registry.apply_change(&remove, &merge_store).await.unwrap();
    let old_owner = DeviceListSyncAdapter::snapshot_change(
        "old-owner",
        sample_device(1, "Custom Mac"),
        "0101010101010101",
        hlc(350),
    );
    assert_eq!(
        registry
            .apply_change(&old_owner, &merge_store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    assert!(read_device_list(data.path()).devices.is_empty());
}

#[test]
fn rename_change_uses_device_list_domain() {
    let change = DeviceListSyncAdapter::rename_change(
        "op-rename",
        "0101010101010101",
        "daily phone",
        "device-a",
        hlc(100),
    );

    assert_eq!(change.domain, DEVICE_LIST_DOMAIN);
    assert_eq!(change.kind, DEVICE_LIST_KIND_RENAME);
    assert_eq!(change.record_id, "0101010101010101");
}

#[tokio::test]
async fn full_export_does_not_turn_a_stale_default_name_into_a_rename() {
    let mac = tempfile::tempdir().unwrap();
    let windows = tempfile::tempdir().unwrap();
    // The Mac has not received a rename made on an existing peer yet. Pairing
    // another Device rewrites its list, but is not a new edit of its own name.
    write_device_list(mac.path(), vec![sample_device(1, "default-hostname")]);
    write_device_list(windows.path(), vec![sample_device(1, "default-hostname")]);
    let (registry, merge_store) = registry_with_adapter(windows.path()).await;
    let rename = DeviceListSyncAdapter::rename_change(
        "explicit-user-rename",
        "0101010101010101",
        "My Mac",
        "phone",
        hlc(200),
    );
    registry.apply_change(&rename, &merge_store).await.unwrap();
    let exporter = DeviceListSyncAdapter::new(mac.path().to_path_buf(), "0101010101010101");
    for change in exporter.export_records().await.unwrap() {
        registry.apply_change(&change, &merge_store).await.unwrap();
    }
    assert_eq!(
        read_device_list(windows.path()).devices[0].friendly_name,
        "My Mac"
    );
}

#[tokio::test]
async fn legacy_main_cursor_without_name_cursor_does_not_allow_stale_owner_rename() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(data.path(), vec![sample_device(1, "My Mac")]);
    let (registry, merge_store) = registry_with_adapter(data.path()).await;
    merge_store
        .set_record_cursor(
            "device_list:0101010101010101",
            &rua_sync::merge_store::live_cursor(hlc(200), None),
        )
        .unwrap();
    let stale = DeviceListSyncAdapter::snapshot_change(
        "legacy-stale-owner",
        sample_device(1, "default-hostname"),
        "0101010101010101",
        hlc(100),
    );
    registry.apply_change(&stale, &merge_store).await.unwrap();
    assert_eq!(
        read_device_list(data.path()).devices[0].friendly_name,
        "My Mac"
    );
}

#[tokio::test]
async fn full_export_carries_peer_device_rename_to_an_existing_row() {
    let mac = tempfile::tempdir().unwrap();
    let windows = tempfile::tempdir().unwrap();
    write_device_list(mac.path(), vec![sample_device(2, "default phone")]);
    write_device_list(windows.path(), vec![sample_device(2, "default phone")]);
    let (mac_registry, mac_store) = registry_with_adapter(mac.path()).await;
    let renamed = DeviceListSyncAdapter::rename_change(
        "explicit-peer-rename",
        "0202020202020202",
        "Pocket",
        "mac",
        hlc(200),
    );
    mac_registry
        .apply_change(&renamed, &mac_store)
        .await
        .unwrap();
    let exporter = DeviceListSyncAdapter::new(mac.path().to_path_buf(), "0101010101010101");
    let (registry, store) = registry_with_adapter(windows.path()).await;
    for change in exporter.export_records().await.unwrap() {
        registry.apply_change(&change, &store).await.unwrap();
    }
    assert_eq!(
        read_device_list(windows.path()).devices[0].friendly_name,
        "Pocket"
    );
}

#[tokio::test]
async fn local_name_commit_rejects_delayed_rename_after_sidecar_rebuild() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(data.path(), vec![sample_device(1, "default phone")]);
    let control = rua_sync::control_store::SyncControlStore::from_data_dir(data.path());
    let committed = control
        .commit_device_name(&[1; 8], "My Phone", [2; 8])
        .unwrap()
        .unwrap();
    let (registry, store) = registry_with_adapter(data.path()).await;
    let stale = DeviceListSyncAdapter::rename_change(
        "delayed-rename",
        "0101010101010101",
        "old name",
        "phone",
        hlc(100),
    );
    assert_eq!(
        registry.apply_change(&stale, &store).await.unwrap().status,
        RegistryApplyStatus::Skipped
    );
    assert_eq!(
        read_device_list(data.path()).devices[0].friendly_name,
        "My Phone"
    );
    let exported = DeviceListSyncAdapter::new(data.path().to_path_buf(), "windows")
        .export_records()
        .await
        .unwrap();
    let record: rua_sync::device_list::DeviceNameRecord =
        serde_json::from_value(exported[0].payload["name_record"].clone()).unwrap();
    assert_eq!(record, committed);
}

#[tokio::test]
async fn concurrent_local_renames_converge_using_committed_revisions() {
    use rua_sync::control_store::SyncControlStore;
    use rua_sync::device_list::DeviceNameRecord;

    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let baseline = DeviceNameRecord {
        friendly_name: "baseline".into(),
        revision: HlcTimestamp {
            wall_ms: u64::MAX - 100,
            counter: 0,
            device_id: [0; 8],
        }
        .to_sortable_bytes(),
        ..Default::default()
    };
    for data in [a.path(), b.path()] {
        let mut list = DeviceList {
            devices: vec![sample_device(1, "baseline")],
            ..Default::default()
        };
        list.name_records
            .insert("0101010101010101".into(), baseline.clone());
        SyncControlStore::from_data_dir(data)
            .write_device_list(&list)
            .unwrap();
    }
    let a_name = SyncControlStore::from_data_dir(a.path())
        .commit_device_name(&[1; 8], "A name", [2; 8])
        .unwrap()
        .unwrap();
    let b_name = SyncControlStore::from_data_dir(b.path())
        .commit_device_name(&[1; 8], "B name", [3; 8])
        .unwrap()
        .unwrap();
    assert!(b_name.revision > a_name.revision);
    let (a_registry, a_store) = registry_with_adapter(a.path()).await;
    let (b_registry, b_store) = registry_with_adapter(b.path()).await;
    a_registry
        .apply_change(
            &DeviceListSyncAdapter::rename_record_change("b-name", "0101010101010101", b_name, "b"),
            &a_store,
        )
        .await
        .unwrap();
    b_registry
        .apply_change(
            &DeviceListSyncAdapter::rename_record_change("a-name", "0101010101010101", a_name, "a"),
            &b_store,
        )
        .await
        .unwrap();
    assert_eq!(
        read_device_list(a.path()).devices[0].friendly_name,
        "B name"
    );
    assert_eq!(
        read_device_list(b.path()).devices[0].friendly_name,
        "B name"
    );
}

#[tokio::test]
async fn legacy_name_frontier_rejects_delayed_rename_then_accepts_new_commit() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(data.path(), vec![sample_device(1, "My Mac")]);
    let (registry, store) = registry_with_adapter(data.path()).await;
    store
        .set_record_cursor(
            "device_list:0101010101010101",
            &rua_sync::merge_store::live_cursor(hlc(200), None),
        )
        .unwrap();
    // A legacy rename has no embedded name record.
    let mut stale = DeviceListSyncAdapter::rename_change(
        "legacy-rename",
        "0101010101010101",
        "old name",
        "peer",
        hlc(100),
    );
    stale.payload.as_object_mut().unwrap().remove("name_record");
    assert_eq!(
        registry.apply_change(&stale, &store).await.unwrap().status,
        RegistryApplyStatus::Skipped
    );
    assert_eq!(
        read_device_list(data.path()).devices[0].friendly_name,
        "My Mac"
    );
    let fresh = DeviceListSyncAdapter::rename_change(
        "new-commit",
        "0101010101010101",
        "New Mac",
        "peer",
        hlc(250),
    );
    registry.apply_change(&fresh, &store).await.unwrap();
    assert_eq!(
        read_device_list(data.path()).devices[0].friendly_name,
        "New Mac"
    );
}

#[tokio::test]
async fn removal_keeps_name_fact_without_exporting_membership_and_repair_restores_name() {
    let data = tempfile::tempdir().unwrap();
    write_device_list(data.path(), vec![sample_device(1, "default phone")]);
    let (registry, store) = registry_with_adapter(data.path()).await;
    let rename = DeviceListSyncAdapter::rename_change(
        "name",
        "0101010101010101",
        "My Phone",
        "peer",
        hlc(200),
    );
    registry.apply_change(&rename, &store).await.unwrap();
    let remove =
        DeviceListSyncAdapter::remove_change("remove", "0101010101010101", "peer", hlc(300));
    registry.apply_change(&remove, &store).await.unwrap();
    let exporter = DeviceListSyncAdapter::new(data.path().to_path_buf(), "observer");
    assert!(exporter.export_records().await.unwrap().is_empty());
    assert!(read_device_list(data.path())
        .name_records
        .contains_key("0101010101010101"));
    let readd = DeviceListSyncAdapter::snapshot_change(
        "re-paired",
        sample_device(1, "default phone"),
        "peer",
        hlc(400),
    );
    registry.apply_change(&readd, &store).await.unwrap();
    assert_eq!(
        read_device_list(data.path()).devices[0].friendly_name,
        "My Phone"
    );
}
