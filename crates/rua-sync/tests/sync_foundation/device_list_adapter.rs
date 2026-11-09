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
        serde_json::to_vec_pretty(&DeviceList { devices }).unwrap(),
    )
    .unwrap();
}

fn read_device_list(data_dir: &std::path::Path) -> DeviceList {
    serde_json::from_str(
        &std::fs::read_to_string(data_dir.join("sync").join("device_list.json")).unwrap(),
    )
    .unwrap()
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
