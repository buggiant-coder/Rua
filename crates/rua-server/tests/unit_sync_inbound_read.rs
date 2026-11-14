use super::*;
use rua_sync::{
    device_list::{DeviceEntry, DeviceList},
    endpoint::SyncNode,
    hlc::HlcTimestamp,
    identity::{DeviceIdentity, NetworkSecret},
    inbound_inbox::{InboxEntry, InboxState},
    opaque_replica_log::{OpaqueReplicaLog, SyncChange},
};

#[tokio::test]
async fn local_inbound_content_does_not_wait_for_an_offline_device_session() {
    let tmp = tempfile::tempdir().unwrap();
    let network = NetworkSecret::generate();
    let device = DeviceIdentity::generate();
    let offline = DeviceIdentity::generate();
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    rua_sync::control_store::write_device_list_file(
        tmp.path().join("device_list.json"),
        &DeviceList {
            devices: vec![DeviceEntry {
                device_id: offline.device_id_short(),
                public_key: *offline.public_key().as_bytes(),
                label: String::new(),
                friendly_name: "offline".into(),
                added_at_ms: 1,
                last_seen_ms: 0,
                direct_addrs: vec![socket.local_addr().unwrap().to_string()],
            }],
            ..Default::default()
        },
    )
    .unwrap();
    let node = SyncNode::bind(tmp.path(), network.namespace_secret(), &device, None)
        .await
        .unwrap();
    let log = OpaqueReplicaLog::new(
        node.doc().clone(),
        device.to_iroh_author(),
        network.derive_sync_key(),
        node.blobs_store().clone(),
    );
    let change = SyncChange::new(
        "local-read",
        "read-test",
        "record",
        "upsert",
        "remote",
        HlcTimestamp {
            wall_ms: 42,
            counter: 0,
            device_id: device.device_id_short(),
        },
        serde_json::json!({"value": "already downloaded"}),
    );
    let key = log.append_change(&change).await.unwrap();
    let author = device.to_iroh_author().id();
    let hash = node
        .doc()
        .get_exact(author, &key, false)
        .await
        .unwrap()
        .unwrap()
        .content_hash();
    let entry = InboxEntry {
        author,
        key,
        content_hash: hash.to_string(),
        state: InboxState::Pending,
        attempts: 0,
        seen_at_ms: 1,
        last_error: None,
    };
    let download = ActiveBlobDownload {
        downloader: node.blob_downloader().clone(),
        providers: vec![iroh::EndpointId::from_bytes(offline.public_key().as_bytes()).unwrap()],
    };
    let recorder = rua_sync::stage_diagnostics::SyncStageRecorder::default();
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        read_inbound_frame_fast(&log, Some(&download), &recorder, entry),
    )
    .await;
    node.shutdown().await;
    let result = result.expect("local content waited for a network session");
    match result {
        InboundReadResult::Readable(read) => {
            assert_eq!(read.changes, vec![change]);
            assert!(!read.read_diagnostics.used_peer_download);
        }
        _ => panic!("local content was not read"),
    }
}

#[derive(Debug)]
struct StalledDeviceSession(tokio_util::sync::CancellationToken);

impl iroh::protocol::ProtocolHandler for StalledDeviceSession {
    async fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> Result<(), iroh::protocol::AcceptError> {
        self.0.cancelled().await;
        drop(connection);
        Ok(())
    }
}

#[tokio::test]
async fn missing_inbound_content_uses_blobs_without_waiting_for_a_device_session() {
    let tmp = tempfile::tempdir().unwrap();
    let network = NetworkSecret::generate();
    let device = DeviceIdentity::generate();
    let provider = DeviceIdentity::generate();
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(iroh::SecretKey::from_bytes(
            &provider.signing_key().to_bytes(),
        ))
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .unwrap()
        .bind()
        .await
        .unwrap();
    let store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::default().into();
    let cancel = tokio_util::sync::CancellationToken::new();
    let guard = cancel.clone().drop_guard();
    let router = iroh::protocol::Router::builder(endpoint.clone())
        .accept(
            iroh_blobs::ALPN,
            iroh_blobs::BlobsProtocol::new(&store, None),
        )
        .accept(
            rua_device_network::DEVICE_SESSION_ALPN,
            StalledDeviceSession(cancel.clone()),
        )
        .spawn();
    rua_sync::control_store::write_device_list_file(
        tmp.path().join("device_list.json"),
        &DeviceList {
            devices: vec![DeviceEntry {
                device_id: provider.device_id_short(),
                public_key: *provider.public_key().as_bytes(),
                label: String::new(),
                friendly_name: "blob-provider".into(),
                added_at_ms: 1,
                last_seen_ms: 0,
                direct_addrs: vec![endpoint.bound_sockets()[0].to_string()],
            }],
            ..Default::default()
        },
    )
    .unwrap();
    let node = SyncNode::bind(tmp.path(), network.namespace_secret(), &device, None)
        .await
        .unwrap();
    node.network_runtime()
        .resolve_peer_addr(endpoint.id())
        .await
        .unwrap();
    let log = OpaqueReplicaLog::new(
        node.doc().clone(),
        device.to_iroh_author(),
        network.derive_sync_key(),
        node.blobs_store().clone(),
    );
    let change = SyncChange::new(
        "remote-read",
        "read-test",
        "record",
        "upsert",
        "remote",
        HlcTimestamp {
            wall_ms: 42,
            counter: 0,
            device_id: provider.device_id_short(),
        },
        serde_json::json!({"value": "download via blobs"}),
    );
    let hash = rua_sync::opaque_replica_log::put_encrypted_blob(
        &store,
        &network.derive_sync_key(),
        &serde_json::to_vec(&change).unwrap(),
    )
    .await
    .unwrap();
    let entry = InboxEntry {
        author: provider.to_iroh_author().id(),
        key: log.record_entry_key(&change.domain, &change.record_id),
        content_hash: hash,
        state: InboxState::Pending,
        attempts: 0,
        seen_at_ms: 1,
        last_error: None,
    };
    let download = ActiveBlobDownload {
        downloader: node.blob_downloader().clone(),
        providers: vec![endpoint.id()],
    };
    let recorder = rua_sync::stage_diagnostics::SyncStageRecorder::default();
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        read_inbound_frame_fast(&log, Some(&download), &recorder, entry),
    )
    .await;
    drop(guard);
    router.shutdown().await.unwrap();
    node.shutdown().await;
    match result.expect("available Blob transfer waited for Device Session admission") {
        InboundReadResult::Readable(read) => {
            assert_eq!(read.changes, vec![change]);
            assert!(read.read_diagnostics.used_peer_download);
        }
        _ => panic!("available content was not downloaded"),
    }
}
