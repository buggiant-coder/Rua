use std::sync::Arc;

use rua_server::sync_migration::{apply_pending_network_migration, NetworkMigrationApplyOutcome};
use rua_server::sync_network::{BoundSyncNetwork, BoundSyncNetworkError};
use rua_server::{AppState, ServerProfile};
use rua_sync::endpoint::SyncNode;
use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use rua_sync::network_migration_adapter::{
    pending_migration_path, write_pending_migration_file, NetworkMigrationPayload,
};

async fn bind_node(
    sync_dir: &std::path::Path,
    network_secret: &NetworkSecret,
    device: &DeviceIdentity,
) -> SyncNode {
    SyncNode::bind(sync_dir, network_secret.namespace_secret(), device, None)
        .await
        .expect("bind sync node")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_sync_network_accepts_consistent_runtime_identity() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    let network_secret = Arc::new(NetworkSecret::generate());
    network_secret.save(&sync_dir).expect("save network secret");
    let device = Arc::new(DeviceIdentity::generate());
    let node = bind_node(&sync_dir, &network_secret, &device).await;

    let bound = BoundSyncNetwork::from_parts(
        &sync_dir,
        &node,
        Arc::clone(&network_secret),
        Arc::clone(&device),
    )
    .expect("bound network");

    assert_eq!(bound.namespace_id(), network_secret.namespace_id());
    assert_eq!(
        bound.local_device_id_hex(),
        hex::encode(device.device_id_short())
    );
    assert_eq!(bound.local_author(), device.to_iroh_author().id());
    assert_eq!(
        bound.sync_key().as_bytes(),
        network_secret.derive_sync_key().as_bytes()
    );
    let _log = bound.opaque_replica_log();

    node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_sync_network_rejects_runtime_namespace_mismatch() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    let node_network_secret = NetworkSecret::generate();
    let memory_network_secret = Arc::new(NetworkSecret::generate());
    memory_network_secret
        .save(&sync_dir)
        .expect("save memory network secret");
    let device = Arc::new(DeviceIdentity::generate());
    let node = bind_node(&sync_dir, &node_network_secret, &device).await;

    let err = BoundSyncNetwork::from_parts(
        &sync_dir,
        &node,
        Arc::clone(&memory_network_secret),
        Arc::clone(&device),
    )
    .expect_err("namespace mismatch must be rejected");

    assert!(matches!(
        err,
        BoundSyncNetworkError::IdentityMismatch { .. }
    ));

    node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_sync_network_rejects_disk_secret_mismatch() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    let runtime_network_secret = Arc::new(NetworkSecret::generate());
    let disk_network_secret = NetworkSecret::generate();
    disk_network_secret
        .save(&sync_dir)
        .expect("save different disk network secret");
    let device = Arc::new(DeviceIdentity::generate());
    let node = bind_node(&sync_dir, &runtime_network_secret, &device).await;

    let err = BoundSyncNetwork::from_parts(
        &sync_dir,
        &node,
        Arc::clone(&runtime_network_secret),
        Arc::clone(&device),
    )
    .expect_err("disk mismatch must be rejected");

    assert!(matches!(
        err,
        BoundSyncNetworkError::DiskSecretMismatch { .. }
    ));

    node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_sync_network_rejects_runtime_device_identity_mismatch() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    let network_secret = Arc::new(NetworkSecret::generate());
    network_secret.save(&sync_dir).expect("save network secret");
    let runtime_device = Arc::new(DeviceIdentity::generate());
    let state_device = Arc::new(DeviceIdentity::generate());
    let node = bind_node(&sync_dir, &network_secret, &runtime_device).await;

    let err = BoundSyncNetwork::from_parts(
        &sync_dir,
        &node,
        Arc::clone(&network_secret),
        Arc::clone(&state_device),
    )
    .expect_err("Device identity mismatch must be rejected");

    assert!(matches!(
        err,
        BoundSyncNetworkError::DeviceIdentityMismatch { .. }
    ));

    node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_sync_network_reports_not_paired_when_runtime_identity_is_incomplete() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    let network_secret = Arc::new(NetworkSecret::generate());
    let device = Arc::new(DeviceIdentity::generate());

    let err = BoundSyncNetwork::from_optional_parts(
        &sync_dir,
        None,
        Some(Arc::clone(&network_secret)),
        Some(Arc::clone(&device)),
    )
    .expect_err("missing node is not paired");

    assert_eq!(err, BoundSyncNetworkError::NotPaired);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_sync_network_reports_corrupt_disk_secret() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    let network_secret = Arc::new(NetworkSecret::generate());
    network_secret.save(&sync_dir).expect("save network secret");
    let device = Arc::new(DeviceIdentity::generate());
    let node = bind_node(&sync_dir, &network_secret, &device).await;
    std::fs::write(sync_dir.join("network_secret.key"), "not-hex").expect("corrupt secret");

    let err = BoundSyncNetwork::from_parts(
        &sync_dir,
        &node,
        Arc::clone(&network_secret),
        Arc::clone(&device),
    )
    .expect_err("corrupt disk secret must be rejected");

    assert!(matches!(err, BoundSyncNetworkError::CorruptDiskSecret(_)));

    node.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_network_migration_switches_secret_rebinds_runtime_and_clears_marker() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed dir");
    std::fs::create_dir_all(tmp.path().join("prompt_library")).expect("prompt dir");
    std::fs::write(
        tmp.path().join("prompt_library/pre-migration.md"),
        "pre-migration local state",
    )
    .expect("write pre-migration prompt");
    let sync_dir = tmp.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(sync_dir.join("enabled"), "true").expect("enable sync");
    let old_secret = NetworkSecret::generate();
    old_secret.save(&sync_dir).expect("save old secret");
    let target_secret = NetworkSecret::generate();
    let marker_path = pending_migration_path(tmp.path());
    write_pending_migration_file(
        &marker_path,
        &NetworkMigrationPayload {
            target_namespace_id: hex::encode(target_secret.namespace_id().as_bytes()),
            target_namespace_secret_bytes: target_secret.to_bytes().to_vec(),
            migrated_at_ms: 42,
        },
    )
    .expect("write migration marker");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Desktop);
    let old_endpoint = state
        .device_network
        .lock()
        .await
        .as_ref()
        .expect("old Device Network Runtime")
        .endpoint()
        .clone();

    let outcome = apply_pending_network_migration(&state)
        .await
        .expect("apply migration");

    assert_eq!(outcome, NetworkMigrationApplyOutcome::Migrated);
    let loaded = NetworkSecret::load(&sync_dir)
        .expect("load migrated secret")
        .expect("migrated secret exists");
    assert_eq!(loaded.to_bytes(), target_secret.to_bytes());
    let bound = state
        .bound_sync_network()
        .await
        .expect("runtime rebound to migrated namespace");
    assert_eq!(bound.namespace_id(), target_secret.namespace_id());
    let log = bound.opaque_replica_log();
    let key = log.record_entry_key("prompts", "pre-migration.md");
    let exported = log
        .read_change(bound.local_author(), &key)
        .await
        .expect("read migrated Full Export")
        .expect("pre-migration state was published");
    assert_eq!(exported.payload["content"], "pre-migration local state");
    assert!(old_endpoint.is_closed());
    let migrated_runtime = state
        .device_network
        .lock()
        .await
        .as_ref()
        .cloned()
        .expect("migrated Device Network Runtime");
    assert_eq!(
        migrated_runtime.diagnostics().network_id,
        hex::encode(target_secret.namespace_id().as_bytes())
    );
    assert!(!migrated_runtime.endpoint().is_closed());
    assert!(!marker_path.exists(), "migration marker should be cleared");

    state.clear_sync_node().await;
}
