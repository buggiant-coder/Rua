use bytes::Bytes;
use iroh_blobs::api::blobs::BlobStatus;
use iroh_blobs::api::Store as BlobsApiStore;
use iroh_blobs::store::fs::options::Options as BlobsFsOptions;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::store::GcConfig;
use iroh_blobs::HashAndFormat;
use rua_sync::crypto::SyncKey;
use rua_sync::endpoint::SyncNode;
use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use rua_sync::opaque_replica_log::{put_encrypted_blob, read_encrypted_blob};
use rua_sync::sync_blob_store::{SyncBlobFetchConfig, SyncBlobStore};
use std::time::Duration;

#[tokio::test]
async fn network_rebind_preserves_same_network_queue_and_excludes_previous_network() {
    use rua_sync::inbound_inbox::{InboundInbox, InboxState};
    let dir = tempfile::tempdir().unwrap();
    let device = DeviceIdentity::generate();
    let first = NetworkSecret::generate();
    let second = NetworkSecret::generate();
    let inbox = InboundInbox::open(dir.path()).unwrap();
    let node = SyncNode::bind(dir.path(), first.namespace_secret(), &device, None)
        .await
        .unwrap();
    let versions = rua_sync::merge_store::MergeStore::open(dir.path()).unwrap();
    versions.mark_applied_op("old-op").unwrap();
    let cursor = rua_sync::merge_store::tombstone_cursor(network_hlc(42), None);
    versions
        .set_record_cursor("device_list:removed", &cursor)
        .unwrap();
    let author = device.to_iroh_author().id();
    inbox
        .record_remote_insert(
            author,
            b"old-network-entry",
            iroh_blobs::Hash::new(b"old ciphertext"),
        )
        .unwrap();
    inbox
        .mark_failed(author, b"old-network-entry", "old network decrypt failure")
        .unwrap();
    node.shutdown().await;
    drop(node);
    let node = SyncNode::bind(dir.path(), first.namespace_secret(), &device, None)
        .await
        .unwrap();
    assert_eq!(inbox.count_by_state(InboxState::Failed).unwrap(), 1);
    assert!(versions.has_applied_op("old-op").unwrap());
    assert_eq!(
        versions.get_record_cursor("device_list:removed").unwrap(),
        Some(cursor)
    );
    node.shutdown().await;
    drop(node);
    let node = SyncNode::bind(dir.path(), second.namespace_secret(), &device, None)
        .await
        .unwrap();
    assert_eq!(
        inbox.count_by_state(InboxState::Failed).unwrap(),
        0,
        "old-network ciphertext must never be read with the new network key"
    );
    assert!(!versions.has_applied_op("old-op").unwrap());
    assert!(versions
        .get_record_cursor("device_list:removed")
        .unwrap()
        .is_none());
    node.shutdown().await;
}

#[tokio::test]
async fn network_binding_upgrade_quarantines_unknown_versions_and_replays_current_removal() {
    use rua_sync::control_store::write_device_list_file;
    use rua_sync::device_list::DeviceList;
    use rua_sync::device_list_adapter::DeviceListSyncAdapter;
    use rua_sync::merge_store::{tombstone_cursor, MergeStore};
    use rua_sync::opaque_replica_log::OpaqueReplicaLog;
    use rua_sync::sidecar::RecordState;
    use rua_sync::sync_registry::{RegistryApplyStatus, SyncRegistry};
    use std::sync::Arc;

    let data = tempfile::tempdir().unwrap();
    let sync_dir = data.path().join("sync");
    let device = DeviceIdentity::generate();
    let phone = DeviceIdentity::generate();
    let device_id = hex::encode(device.device_id_short());
    let phone_id = hex::encode(phone.device_id_short());
    let projection_path = sync_dir.join("device_list.json");
    write_device_list_file(
        &projection_path,
        &DeviceList {
            devices: vec![
                network_device_entry(&device, "Windows"),
                network_device_entry(&phone, "Phone"),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    let versions = MergeStore::open(&sync_dir).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(DeviceListSyncAdapter::new(
            data.path().to_path_buf(),
            device_id.clone(),
        )))
        .unwrap();
    let remove = DeviceListSyncAdapter::remove_change(
        "legacy-removal",
        &phone_id,
        &device_id,
        network_hlc(300),
    );
    let record_key = format!("{}:{}", remove.domain, remove.record_id);
    registry.apply_change(&remove, &versions).await.unwrap();
    let fingerprint = versions
        .applied_op_fingerprint(&remove.op_id)
        .unwrap()
        .unwrap()
        .unwrap();
    let snapshot = b"retained legacy base snapshot";
    let snapshot_hash = versions.put_base_snapshot(snapshot).unwrap();
    let cursor = tombstone_cursor(remove.hlc, Some(snapshot_hash.clone()));
    versions.set_record_cursor(&record_key, &cursor).unwrap();
    let projection_before_binding = std::fs::read(&projection_path).unwrap();

    let network = NetworkSecret::generate();
    let node = SyncNode::bind(&sync_dir, network.namespace_secret(), &device, None)
        .await
        .unwrap();
    assert!(!versions.has_applied_op(&remove.op_id).unwrap());
    assert!(versions.get_record_cursor(&record_key).unwrap().is_none());
    assert_eq!(
        std::fs::read(&projection_path).unwrap(),
        projection_before_binding,
        "binding must leave canonical projections intact"
    );
    assert_eq!(
        versions.get_base_snapshot(&snapshot_hash).unwrap().unwrap(),
        snapshot
    );
    let connection = rusqlite::Connection::open(sync_dir.join("sync-state.sqlite")).unwrap();
    let archived_op: (String, String, String, String) = connection
        .query_row(
            "SELECT op_id, domain, record_id, change_fingerprint
             FROM sync_legacy_unbound_applied_ops",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        archived_op,
        (
            remove.op_id.clone(),
            remove.domain.clone(),
            record_key.clone(),
            fingerprint
        )
    );
    let archived_cursor: (String, Vec<u8>, String, String) = connection
        .query_row(
            "SELECT record_id, hlc, state, base_blob_hash
             FROM sync_legacy_unbound_record_cursors",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        archived_cursor,
        (
            record_key.clone(),
            cursor.hlc.to_sortable_bytes().to_vec(),
            "tombstoned".into(),
            snapshot_hash
        )
    );

    let current_phone = DeviceListSyncAdapter::snapshot_change(
        "current-phone-snapshot",
        network_device_entry(&phone, "Phone"),
        &device_id,
        network_hlc(200),
    );
    assert_eq!(
        registry
            .apply_change(&current_phone, &versions)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Applied
    );
    let log = OpaqueReplicaLog::new(
        node.doc().clone(),
        device.to_iroh_author(),
        network.derive_sync_key(),
        node.blobs_store().clone(),
    );
    let key = log.append_change(&remove).await.unwrap();
    let replay = log
        .read_change(device.to_iroh_author().id(), &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        registry
            .apply_change(&replay, &versions)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Applied,
        "current-network log replay must restore a legitimate removal"
    );
    assert!(versions.has_applied_op(&remove.op_id).unwrap());
    assert_eq!(
        versions
            .get_record_cursor(&record_key)
            .unwrap()
            .unwrap()
            .state,
        RecordState::Tombstoned
    );
    let projection: DeviceList =
        serde_json::from_slice(&std::fs::read(&projection_path).unwrap()).unwrap();
    assert!(!projection.contains_device(&phone.device_id_short()));
    assert!(projection.contains_device(&device.device_id_short()));
    node.shutdown().await;
}

#[tokio::test]
async fn network_binding_upgrade_does_not_let_foreign_phone_tombstone_hide_current_phone() {
    use rua_sync::control_store::write_device_list_file;
    use rua_sync::device_list::DeviceList;
    use rua_sync::device_list_adapter::DeviceListSyncAdapter;
    use rua_sync::merge_store::MergeStore;
    use rua_sync::sync_registry::{RegistryApplyStatus, SyncRegistry};
    use std::sync::Arc;

    let data = tempfile::tempdir().unwrap();
    let sync_dir = data.path().join("sync");
    let windows = DeviceIdentity::generate();
    let mac = DeviceIdentity::generate();
    let phone = DeviceIdentity::generate();
    let phone_id = hex::encode(phone.device_id_short());
    let mac_id = hex::encode(mac.device_id_short());
    write_device_list_file(
        sync_dir.join("device_list.json"),
        &DeviceList {
            devices: vec![
                network_device_entry(&windows, "Windows"),
                network_device_entry(&mac, "Mac"),
            ],
            ..Default::default()
        },
    )
    .unwrap();
    let versions = MergeStore::open(&sync_dir).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(DeviceListSyncAdapter::new(
            data.path().to_path_buf(),
            hex::encode(windows.device_id_short()),
        )))
        .unwrap();
    let old_remove = DeviceListSyncAdapter::remove_change(
        "foreign-phone-removal",
        &phone_id,
        &mac_id,
        network_hlc(300),
    );
    registry.apply_change(&old_remove, &versions).await.unwrap();
    let network = NetworkSecret::generate();
    let node = SyncNode::bind(&sync_dir, network.namespace_secret(), &windows, None)
        .await
        .unwrap();
    let current_phone = DeviceListSyncAdapter::snapshot_change(
        "current-phone-snapshot",
        network_device_entry(&phone, "Existing Phone"),
        &mac_id,
        network_hlc(200),
    );
    let result = registry
        .apply_change(&current_phone, &versions)
        .await
        .unwrap();
    assert_eq!(
        result.status,
        RegistryApplyStatus::Applied,
        "a version without network provenance must not suppress current-network records"
    );
    let list: DeviceList =
        serde_json::from_slice(&std::fs::read(sync_dir.join("device_list.json")).unwrap()).unwrap();
    assert!(list.contains_device(&phone.device_id_short()));
    assert_eq!(list.devices.len(), 3);
    node.shutdown().await;
}

#[tokio::test]
async fn network_binding_upgrade_rolls_back_quarantine_and_reset_together_on_failure() {
    use rua_sync::inbound_inbox::{InboundInbox, InboxState};
    use rua_sync::merge_store::{tombstone_cursor, MergeStore};

    let dir = tempfile::tempdir().unwrap();
    let device = DeviceIdentity::generate();
    let network = NetworkSecret::generate();
    let versions = MergeStore::open(dir.path()).unwrap();
    versions.mark_applied_op("legacy-op").unwrap();
    let cursor = tombstone_cursor(network_hlc(300), None);
    versions
        .set_record_cursor("device_list:removed", &cursor)
        .unwrap();
    let inbox = InboundInbox::open(dir.path()).unwrap();
    inbox
        .record_remote_insert(
            device.to_iroh_author().id(),
            b"legacy-entry",
            iroh_blobs::Hash::new(b"legacy ciphertext"),
        )
        .unwrap();
    let connection = rusqlite::Connection::open(dir.path().join("sync-state.sqlite")).unwrap();
    connection
        .execute_batch(
            "INSERT INTO sync_metadata(key, integer_value)
                 VALUES('last_successful_repair_scan_ms', 123);
             CREATE TRIGGER reject_network_reset BEFORE DELETE ON sync_metadata
                 BEGIN SELECT RAISE(ABORT, 'injected network reset failure'); END;",
        )
        .unwrap();

    let failure = SyncNode::bind(dir.path(), network.namespace_secret(), &device, None).await;
    assert!(failure.is_err());
    assert!(versions.has_applied_op("legacy-op").unwrap());
    assert_eq!(
        versions.get_record_cursor("device_list:removed").unwrap(),
        Some(cursor)
    );
    assert_eq!(inbox.count_by_state(InboxState::Pending).unwrap(), 1);
    assert_eq!(inbox.last_successful_repair_scan_ms().unwrap(), Some(123));
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE name IN ('sync_network_binding',
                     'sync_legacy_unbound_applied_ops',
                     'sync_legacy_unbound_record_cursors')",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0,
        "a failed migration must not leave binding or quarantine half committed"
    );

    connection
        .execute_batch("DROP TRIGGER reject_network_reset;")
        .unwrap();
    let node = SyncNode::bind(dir.path(), network.namespace_secret(), &device, None)
        .await
        .unwrap();
    assert!(!versions.has_applied_op("legacy-op").unwrap());
    assert!(versions
        .get_record_cursor("device_list:removed")
        .unwrap()
        .is_none());
    assert_eq!(inbox.count_by_state(InboxState::Pending).unwrap(), 0);
    assert_eq!(inbox.last_successful_repair_scan_ms().unwrap(), None);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sync_legacy_unbound_applied_ops",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        1
    );
    node.shutdown().await;
}

#[tokio::test]
async fn network_binding_upgrade_restores_only_proven_own_removals_before_remote_apply() {
    use rua_sync::control_store::write_device_list_file;
    use rua_sync::device_list::DeviceList;
    use rua_sync::device_list_adapter::DeviceListSyncAdapter;
    use rua_sync::merge_store::{tombstone_cursor, MergeStore};
    use rua_sync::opaque_replica_log::OpaqueReplicaLog;
    use rua_sync::sync_registry::{sync_cursor_record_id, RegistryApplyStatus, SyncRegistry};
    use std::sync::Arc;

    let data = tempfile::tempdir().unwrap();
    let sync_dir = data.path().join("sync");
    let device = DeviceIdentity::generate();
    let removed_phone = DeviceIdentity::generate();
    let foreign_phone = DeviceIdentity::generate();
    let snapshot_phone = DeviceIdentity::generate();
    let device_id = hex::encode(device.device_id_short());
    let projection_path = sync_dir.join("device_list.json");
    write_device_list_file(
        &projection_path,
        &DeviceList {
            devices: vec![network_device_entry(&device, "Windows")],
            ..Default::default()
        },
    )
    .unwrap();
    let network = NetworkSecret::generate();
    let node = SyncNode::bind(&sync_dir, network.namespace_secret(), &device, None)
        .await
        .unwrap();
    let versions = MergeStore::open(&sync_dir).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(DeviceListSyncAdapter::new(
            data.path().to_path_buf(),
            device_id.clone(),
        )))
        .unwrap();
    let removal = DeviceListSyncAdapter::remove_change(
        "own-current-network-removal",
        hex::encode(removed_phone.device_id_short()),
        &device_id,
        network_hlc(300),
    );
    let foreign_removal = DeviceListSyncAdapter::remove_change(
        "foreign-network-removal",
        hex::encode(foreign_phone.device_id_short()),
        &device_id,
        network_hlc(300),
    );
    let own_snapshot = DeviceListSyncAdapter::snapshot_change(
        "own-current-network-snapshot",
        network_device_entry(&snapshot_phone, "Snapshot Phone"),
        &device_id,
        network_hlc(300),
    );
    for change in [&removal, &foreign_removal, &own_snapshot] {
        registry.apply_change(change, &versions).await.unwrap();
    }
    // A coincident record/HLC alone cannot link an old tombstone to this
    // namespace's snapshot operation, even when that operation is proven.
    let snapshot_cursor_id = sync_cursor_record_id(&own_snapshot.domain, &own_snapshot.record_id);
    versions
        .set_record_cursor(
            &snapshot_cursor_id,
            &tombstone_cursor(own_snapshot.hlc, None),
        )
        .unwrap();
    let log = OpaqueReplicaLog::new(
        node.doc().clone(),
        device.to_iroh_author(),
        network.derive_sync_key(),
        node.blobs_store().clone(),
    );
    log.append_change(&removal).await.unwrap();
    log.append_change(&own_snapshot).await.unwrap();
    node.shutdown().await;
    drop(log);
    drop(node);
    let connection = rusqlite::Connection::open(sync_dir.join("sync-state.sqlite")).unwrap();
    connection
        .execute("DELETE FROM sync_network_binding", [])
        .unwrap();
    let canonical_before_binding = std::fs::read(&projection_path).unwrap();

    let node = SyncNode::bind(&sync_dir, network.namespace_secret(), &device, None)
        .await
        .unwrap();
    let removal_cursor_id = sync_cursor_record_id(&removal.domain, &removal.record_id);
    assert_eq!(
        versions.get_record_cursor(&removal_cursor_id).unwrap(),
        Some(tombstone_cursor(removal.hlc, None)),
        "bind must recover proven own removals before self inbox rows are skipped"
    );
    assert!(versions.has_applied_op(&removal.op_id).unwrap());
    assert!(!versions.has_applied_op(&foreign_removal.op_id).unwrap());
    assert!(versions
        .get_record_cursor(&sync_cursor_record_id(
            &foreign_removal.domain,
            &foreign_removal.record_id
        ))
        .unwrap()
        .is_none());
    assert!(versions
        .get_record_cursor(&snapshot_cursor_id)
        .unwrap()
        .is_none());
    assert!(!versions.has_applied_op(&own_snapshot.op_id).unwrap());
    assert_eq!(
        std::fs::read(&projection_path).unwrap(),
        canonical_before_binding,
        "proof recovery must not replay self-authored canonical snapshots"
    );

    for (phone, expected) in [
        (&removed_phone, RegistryApplyStatus::Skipped),
        (&foreign_phone, RegistryApplyStatus::Applied),
    ] {
        let snapshot = DeviceListSyncAdapter::snapshot_change(
            format!("remote-snapshot-{}", hex::encode(phone.device_id_short())),
            network_device_entry(phone, "Phone"),
            "remote-author",
            network_hlc(200),
        );
        assert_eq!(
            registry
                .apply_change(&snapshot, &versions)
                .await
                .unwrap()
                .status,
            expected
        );
    }
    node.shutdown().await;
}

fn network_hlc(wall_ms: u64) -> rua_sync::hlc::HlcTimestamp {
    rua_sync::hlc::HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [1; 8],
    }
}

fn network_device_entry(device: &DeviceIdentity, name: &str) -> rua_sync::device_list::DeviceEntry {
    rua_sync::device_list::DeviceEntry {
        device_id: device.device_id_short(),
        public_key: device.public_key().to_bytes(),
        label: name.into(),
        friendly_name: name.into(),
        added_at_ms: 1,
        last_seen_ms: 1,
        direct_addrs: Vec::new(),
    }
}

#[tokio::test]
async fn put_get_roundtrip_1mb() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");

    let sync_key = SyncKey::from_bytes([0u8; 32]);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let api_store = store.api_store();

    let plaintext: Vec<u8> = (0..1_048_576).map(|i| (i % 256) as u8).collect();

    let hash_ref = put_encrypted_blob(&api_store, &sync_key, &plaintext)
        .await
        .unwrap();

    assert!(!hash_ref.is_empty());

    let recovered = read_encrypted_blob(&api_store, &sync_key, &hash_ref)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered, plaintext);
}

#[tokio::test]
async fn put_stores_only_ciphertext() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");

    let sync_key = SyncKey::from_bytes([0u8; 32]);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let api_store = store.api_store();

    let plaintext = b"secret sync content";
    let hash_ref = put_encrypted_blob(&api_store, &sync_key, plaintext)
        .await
        .unwrap();

    let plaintext_hash = blake3::hash(plaintext).to_hex().to_string();
    assert_ne!(
        hash_ref, plaintext_hash,
        "hash should be of ciphertext, not plaintext"
    );
}

#[tokio::test]
async fn has_blob_returns_true_after_put() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");

    let sync_key = SyncKey::from_bytes([0u8; 32]);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let api_store = store.api_store();

    let hash_ref = put_encrypted_blob(&api_store, &sync_key, b"hello")
        .await
        .unwrap();

    assert!(store.has_blob(&hash_ref).await.unwrap());
}

#[tokio::test]
async fn put_encrypted_blob_uses_temporary_tag_reclaimed_by_gc() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");
    std::fs::create_dir_all(&blobs_dir).unwrap();

    let mut opts = BlobsFsOptions::new(&blobs_dir);
    opts.gc = Some(GcConfig {
        interval: Duration::from_secs(1),
        add_protected: None,
    });
    let store: BlobsApiStore = FsStore::load_with_opts(blobs_dir.join("blobs.db"), opts)
        .await
        .unwrap()
        .into();
    let sync_key = SyncKey::from_bytes([9u8; 32]);

    std::env::set_var("RUA_SYNC_TEMP_BLOB_RETENTION_SECS", "1");
    let hash_ref = put_encrypted_blob(&store, &sync_key, &vec![0x5Au8; 1024 * 1024])
        .await
        .unwrap();
    std::env::remove_var("RUA_SYNC_TEMP_BLOB_RETENTION_SECS");
    let hash_bytes: [u8; 32] = hex::decode(&hash_ref).unwrap().try_into().unwrap();
    let hash = iroh_blobs::Hash::from_bytes(hash_bytes);

    assert!(
        matches!(
            store.status(hash).await.unwrap(),
            BlobStatus::Complete { .. }
        ),
        "blob must be present while the retained TempTag is alive"
    );

    tokio::time::sleep(Duration::from_secs(5)).await;

    let status = store.status(hash).await.unwrap();
    assert!(
        matches!(status, BlobStatus::NotFound),
        "temporary encrypted blob should be reclaimed after TTL + GC, got {status:?}"
    );
}

#[tokio::test]
async fn gc_preserves_current_media_and_skill_payloads_after_restart_and_reclaims_old_payloads() {
    use rua_sync::media_adapter::{MediaOwner, MediaSyncAdapter};
    use rua_sync::opaque_replica_log::OpaqueReplicaLog;
    use rua_sync::skill_adapter::SkillBlobManifestSyncAdapter;

    let data = tempfile::tempdir().unwrap();
    let blobs_dir = data.path().join("blobs");
    let sync_dir = data.path().join("sync");
    let device = DeviceIdentity::generate();
    let network = NetworkSecret::generate();
    let device_id = hex::encode(device.device_id_short());
    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let node = SyncNode::bind_with_gc_interval(
        &sync_dir,
        network.namespace_secret(),
        &device,
        Some(store.api_store()),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    let log = OpaqueReplicaLog::new(
        node.doc().clone(),
        device.to_iroh_author(),
        network.derive_sync_key(),
        store.api_store(),
    );
    let media = MediaSyncAdapter::new(data.path().to_path_buf(), &device_id)
        .with_blob_store(store.api_store(), network.derive_sync_key());
    let path = data.path().join("image.bin");
    std::fs::write(&path, vec![b'a'; 300 * 1024]).unwrap();
    let old = media
        .snapshot_change_for_media_path(
            &MediaOwner::Agent("default".into()),
            "7f705ce4-77ce-4303-9c78-55545538b29b",
            &path,
            "old-media",
            &device_id,
            network_hlc(10),
        )
        .await
        .unwrap();
    let old_hash = old.payload["hash_ref"]
        .as_str()
        .unwrap()
        .parse::<iroh_blobs::Hash>()
        .unwrap();
    log.append_change(&old).await.unwrap();
    std::fs::write(&path, vec![b'b'; 300 * 1024]).unwrap();
    let current = media
        .snapshot_change_for_media_path(
            &MediaOwner::Agent("default".into()),
            "7f705ce4-77ce-4303-9c78-55545538b29b",
            &path,
            "current-media",
            &device_id,
            network_hlc(20),
        )
        .await
        .unwrap();
    let media_hash = current.payload["hash_ref"]
        .as_str()
        .unwrap()
        .parse::<iroh_blobs::Hash>()
        .unwrap();
    log.append_change(&current).await.unwrap();
    let skill_dir = data.path().join("skill_pool").join("writer");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("resource.bin"), vec![b'c'; 300 * 1024]).unwrap();
    let skills = SkillBlobManifestSyncAdapter::new(data.path().to_path_buf(), &device_id)
        .with_blob_store(store.api_store(), network.derive_sync_key());
    let skill = skills
        .snapshot_change_for_skill("writer", "current-skill", &device_id, network_hlc(20))
        .await
        .unwrap();
    let skill_hash = skill.payload["files"][0]["hash_ref"]
        .as_str()
        .unwrap()
        .parse::<iroh_blobs::Hash>()
        .unwrap();
    log.append_change(&skill).await.unwrap();
    let orphan = store
        .api_store()
        .add_bytes(Bytes::from_static(b"unreferenced payload"))
        .temp_tag()
        .await
        .unwrap();
    let orphan_hash = orphan.hash();
    drop(orphan);
    node.shutdown().await;
    drop((log, media, skills, node, store));

    // Restart loses the process-local import TempTags, exactly as retention
    // expiry does. Persistent current entries must supply payload reachability.
    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let node = SyncNode::bind_with_gc_interval(
        &sync_dir,
        network.namespace_secret(),
        &device,
        Some(store.api_store()),
        Duration::from_millis(20),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if matches!(
                store.api_store().status(orphan_hash).await.unwrap(),
                BlobStatus::NotFound
            ) && matches!(
                store.api_store().status(old_hash).await.unwrap(),
                BlobStatus::NotFound
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("orphan and overwritten payloads must be reclaimed");
    for (label, hash) in [("media", media_hash), ("skill", skill_hash)] {
        assert!(
            matches!(
                store.api_store().status(hash).await.unwrap(),
                BlobStatus::Complete { .. }
            ),
            "current {label} payload must survive restart and GC"
        );
    }
    node.shutdown().await;
}

#[tokio::test]
async fn gc_aborts_sweep_until_current_envelope_reachability_can_be_read() {
    use rua_sync::opaque_replica_log::OpaqueReplicaLog;
    use rua_sync::prompt_adapter::PromptLibrarySyncAdapter;
    let data = tempfile::tempdir().unwrap();
    let blobs_dir = data.path().join("blobs");
    let sync_dir = data.path().join("sync");
    let device = DeviceIdentity::generate();
    let network = NetworkSecret::generate();
    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let node = SyncNode::bind_with_gc_interval(
        &sync_dir,
        network.namespace_secret(),
        &device,
        Some(store.api_store()),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    let log = OpaqueReplicaLog::new(
        node.doc().clone(),
        device.to_iroh_author(),
        network.derive_sync_key(),
        store.api_store(),
    );
    let bad = store
        .api_store()
        .add_bytes(Bytes::from_static(b"unreadable encrypted envelope"))
        .temp_tag()
        .await
        .unwrap();
    node.doc()
        .set_hash(
            device.to_iroh_author().id(),
            log.record_entry_key("prompts", "valid.md"),
            bad.hash(),
            29,
        )
        .await
        .unwrap();
    drop(bad);
    let orphan = store
        .api_store()
        .add_bytes(Bytes::from_static(b"possibly referenced payload"))
        .temp_tag()
        .await
        .unwrap();
    let orphan_hash = orphan.hash();
    drop(orphan);
    node.shutdown().await;
    drop((node, store, log));

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let node = SyncNode::bind_with_gc_interval(
        &sync_dir,
        network.namespace_secret(),
        &device,
        Some(store.api_store()),
        Duration::from_millis(20),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        matches!(
            store.api_store().status(orphan_hash).await.unwrap(),
            BlobStatus::Complete { .. }
        ),
        "unknown reachability must abort sweep instead of deleting payloads"
    );
    let log = OpaqueReplicaLog::new(
        node.doc().clone(),
        device.to_iroh_author(),
        network.derive_sync_key(),
        store.api_store(),
    );
    let repaired = PromptLibrarySyncAdapter::upsert_change(
        "repaired-envelope",
        "valid.md",
        "valid content",
        hex::encode(device.device_id_short()),
        network_hlc(30),
    );
    log.append_change(&repaired).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !matches!(
            store.api_store().status(orphan_hash).await.unwrap(),
            BlobStatus::NotFound
        ) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("successful reachability proof must allow orphan collection again");
    node.shutdown().await;
}

#[tokio::test]
async fn production_external_store_starts_background_gc() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");
    let node_dir = dir.path().join("node");
    let device = DeviceIdentity::generate();
    let network = NetworkSecret::generate();

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let old_node = SyncNode::bind_with_gc_interval(
        &node_dir,
        network.namespace_secret(),
        &device,
        Some(store.api_store()),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    let log = rua_sync::opaque_replica_log::OpaqueReplicaLog::new(
        old_node.doc().clone(),
        device.to_iroh_author(),
        network.derive_sync_key(),
        store.api_store(),
    );
    let current = rua_sync::prompt_adapter::PromptLibrarySyncAdapter::upsert_change(
        "current-prompt",
        "current.md",
        "current conversation state",
        hex::encode(device.device_id_short()),
        network_hlc(10),
    );
    let key = log.append_change(&current).await.unwrap();
    let current_hash = old_node
        .doc()
        .get_exact(device.to_iroh_author().id(), &key, false)
        .await
        .unwrap()
        .unwrap()
        .content_hash();
    let orphan = store
        .api_store()
        .blobs()
        .add_slice("orphaned sync blob")
        .temp_tag()
        .await
        .unwrap();
    let orphan_hash = orphan.hash();
    drop(orphan);
    let current_auto_tag = store
        .api_store()
        .tags()
        .create(HashAndFormat::raw(current_hash))
        .await
        .unwrap();
    let orphan_auto_tag = store
        .api_store()
        .tags()
        .create(HashAndFormat::raw(orphan_hash))
        .await
        .unwrap();
    store
        .api_store()
        .tags()
        .set("explicit-sync-tag", HashAndFormat::raw(orphan_hash))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(3), old_node.shutdown())
        .await
        .expect("old SyncNode should shut down before reopening its stores");
    drop(old_node);
    drop(log);
    drop(store);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let node = SyncNode::bind_with_gc_interval(
        &node_dir,
        network.namespace_secret(),
        &device,
        Some(store.api_store()),
        Duration::from_millis(20),
    )
    .await
    .unwrap();

    assert!(store
        .api_store()
        .tags()
        .get(current_auto_tag)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .api_store()
        .tags()
        .get(orphan_auto_tag)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .api_store()
        .tags()
        .get("explicit-sync-tag")
        .await
        .unwrap()
        .is_some());
    store
        .api_store()
        .tags()
        .delete("explicit-sync-tag")
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if matches!(
                store.api_store().status(orphan_hash).await.unwrap(),
                BlobStatus::NotFound
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("external SyncBlobStore should run background GC");

    assert!(matches!(
        store.api_store().status(current_hash).await.unwrap(),
        BlobStatus::Complete { .. }
    ));

    tokio::time::timeout(Duration::from_secs(3), node.shutdown())
        .await
        .expect("SyncNode shutdown should stop background GC");
}

#[tokio::test]
async fn read_encrypted_blob_returns_none_for_missing() {
    let dir = tempfile::tempdir().unwrap();
    let blobs_dir = dir.path().join("sync-blobs");

    let sync_key = SyncKey::from_bytes([0u8; 32]);

    let store = SyncBlobStore::open(&blobs_dir, SyncBlobFetchConfig::default())
        .await
        .unwrap();
    let api_store = store.api_store();

    let result = read_encrypted_blob(&api_store, &sync_key, &"00".repeat(32))
        .await
        .unwrap();
    assert!(result.is_none());
}
