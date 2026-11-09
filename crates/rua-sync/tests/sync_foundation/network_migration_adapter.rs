use std::sync::Arc;

use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::network_migration_adapter::{
    NetworkMigrationPayload, NetworkMigrationSyncAdapter, NETWORK_MIGRATION_DOMAIN,
    NETWORK_MIGRATION_KIND_SNAPSHOT, NETWORK_MIGRATION_RECORD_PENDING,
};
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

#[tokio::test]
async fn marker_change_writes_pending_migration_file() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(NetworkMigrationSyncAdapter::new(data.path().to_path_buf()));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let secret_bytes: Vec<u8> = (0u8..32).collect();
    let change = NetworkMigrationSyncAdapter::marker_change(
        "op-network-migration",
        "device-a",
        hlc(10),
        NetworkMigrationPayload {
            target_namespace_id: "winner-namespace".to_string(),
            target_namespace_secret_bytes: secret_bytes.clone(),
            migrated_at_ms: 1_700_000_000_000,
        },
    );

    assert_eq!(change.domain, NETWORK_MIGRATION_DOMAIN);
    assert_eq!(change.record_id, NETWORK_MIGRATION_RECORD_PENDING);
    assert_eq!(change.kind, NETWORK_MIGRATION_KIND_SNAPSHOT);
    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let file = data.path().join("sync").join("pending_migration.json");
    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    assert_eq!(content["target_namespace_id"], "winner-namespace");
    assert_eq!(
        content["target_namespace_secret_bytes"],
        serde_json::json!(secret_bytes)
    );
}

#[tokio::test]
async fn stale_marker_does_not_overwrite_newer_pending_migration() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(NetworkMigrationSyncAdapter::new(data.path().to_path_buf()));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let newer = NetworkMigrationSyncAdapter::marker_change(
        "op-network-migration-newer",
        "device-a",
        hlc(200),
        NetworkMigrationPayload {
            target_namespace_id: "newer-namespace".to_string(),
            target_namespace_secret_bytes: vec![2; 32],
            migrated_at_ms: 2_000,
        },
    );
    registry.apply_change(&newer, &merge_store).await.unwrap();

    let stale = NetworkMigrationSyncAdapter::marker_change(
        "op-network-migration-stale",
        "device-a",
        hlc(100),
        NetworkMigrationPayload {
            target_namespace_id: "old-namespace".to_string(),
            target_namespace_secret_bytes: vec![1; 32],
            migrated_at_ms: 1_000,
        },
    );
    let result = registry.apply_change(&stale, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Skipped);
    let file = data.path().join("sync").join("pending_migration.json");
    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    assert_eq!(content["target_namespace_id"], "newer-namespace");
    assert_eq!(
        content["target_namespace_secret_bytes"],
        serde_json::json!(vec![2; 32])
    );
}

#[tokio::test]
async fn export_is_empty_because_marker_is_control_plane_only() {
    let data = tempfile::tempdir().unwrap();
    let adapter = NetworkMigrationSyncAdapter::new(data.path().to_path_buf());

    let changes = adapter.export_records().await.unwrap();

    assert!(changes.is_empty());
}
