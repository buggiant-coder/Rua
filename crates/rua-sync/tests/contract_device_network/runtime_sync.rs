use std::sync::Arc;
use std::time::Duration;

use bao_tree::{io::outboard::PreOrderMemOutboard, ChunkRanges};
use iroh_blobs::api::blobs::BlobStatus;
use iroh_blobs::protocol::ChunkRangesExt;
use iroh_blobs::store::IROH_BLOCK_SIZE;
use rua_core::{ChatAddress, ChatScopeStore};
use rua_sync::chat_adapter::ChatIndexSyncAdapter;
use rua_sync::endpoint::SyncNode;
use rua_sync::hlc::HlcTimestamp;
use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use rua_sync::inbound_inbox::InboundInbox;
use rua_sync::merge_store::MergeStore;
use rua_sync::opaque_replica_log::{
    read_encrypted_blob_with_peer_download, OpaqueReplicaLog, SyncChange,
};
use rua_sync::sync_registry::{RegistryApplyStatus, SyncRegistry};
use tokio::io::AsyncReadExt;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_paired_peers_resume_live_sync_without_manual_cycles() {
    use rua_sync::device_list::{DeviceEntry, DeviceList};

    let network = NetworkSecret::generate();
    let left_dir = tempfile::tempdir().unwrap();
    let right_dir = tempfile::tempdir().unwrap();
    let left_device = DeviceIdentity::generate();
    let right_device = DeviceIdentity::generate();
    let left = SyncNode::bind_local(left_dir.path(), network.namespace_secret(), &left_device)
        .await
        .unwrap();
    let left_addr = left.local_peer_addr();
    let offline_device = DeviceIdentity::generate();
    let offline_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    rua_sync::control_store::write_device_list_file(
        right_dir.path().join("device_list.json"),
        &DeviceList {
            devices: vec![
                DeviceEntry {
                    device_id: offline_device.device_id_short(),
                    public_key: *offline_device.public_key().as_bytes(),
                    label: String::new(),
                    friendly_name: "offline peer".into(),
                    added_at_ms: 1,
                    last_seen_ms: 0,
                    direct_addrs: vec![offline_socket.local_addr().unwrap().to_string()],
                },
                DeviceEntry {
                    device_id: left_device.device_id_short(),
                    public_key: *left.node_id().as_bytes(),
                    label: String::new(),
                    friendly_name: "paired peer".into(),
                    added_at_ms: 1,
                    last_seen_ms: 0,
                    direct_addrs: left_addr.ip_addrs().map(ToString::to_string).collect(),
                },
            ],
            ..Default::default()
        },
    )
    .unwrap();

    // The paired Device List must be sufficient after every process restart.
    // Do not register peers or invoke the application's single-peer Sync Cycle.
    for round in 0..2 {
        let right = tokio::time::timeout(
            Duration::from_secs(1),
            SyncNode::bind_local(right_dir.path(), network.namespace_secret(), &right_device),
        )
        .await
        .expect("an offline paired peer must not delay startup")
        .unwrap();
        let key = format!("after-start-{round}");
        for (source, destination, device) in [
            (&left, &right, &left_device),
            (&right, &left, &right_device),
        ] {
            source
                .doc()
                .set_bytes(
                    device.to_iroh_author().id(),
                    key.clone(),
                    b"paired update".as_slice(),
                )
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if destination
                        .doc()
                        .get_exact(device.to_iroh_author().id(), &key, false)
                        .await
                        .unwrap()
                        .is_some()
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("saved paired peers must exchange writes without manual sync");
        }
        right.shutdown().await;
    }
    left.shutdown().await;
}

fn seed_chat(data_dir: &std::path::Path, address: &ChatAddress) {
    let scope = ChatScopeStore::resolve(data_dir, address.clone());
    rua_session::chat_index::upsert_sync(
        scope.root(),
        &rua_session::chat_index::ChatIndexRecord {
            id: address.chat_id().to_string(),
            name: address.chat_id().to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            meta: serde_json::json!({}),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .expect("seed Chat Index");
}

fn chat_exists(data_dir: &std::path::Path, address: &ChatAddress) -> bool {
    let scope = ChatScopeStore::resolve(data_dir, address.clone());
    rua_session::chat_index::get_sync(scope.root(), address.chat_id().as_str())
        .expect("read Chat Index")
        .is_some()
}

fn chat_registry(data_dir: &std::path::Path, device_id: &str) -> SyncRegistry {
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(ChatIndexSyncAdapter::new(
            data_dir.to_path_buf(),
            device_id.to_string(),
        )))
        .expect("register Chat adapter");
    registry
}

async fn apply_pending_changes(
    inbox: &InboundInbox,
    log: &OpaqueReplicaLog,
    node: &SyncNode,
    source_node_id: iroh::PublicKey,
    registry: &SyncRegistry,
    merge_store: &MergeStore,
) -> Vec<(SyncChange, RegistryApplyStatus)> {
    inbox
        .repair_scan_doc_entries(node.doc())
        .await
        .expect("scan replicated entries");
    let mut applied = Vec::new();
    for entry in inbox.pending_ready(u64::MAX).expect("read pending entries") {
        let change = log
            .read_change_with_peer_download(
                entry.author,
                &entry.key,
                node.blob_downloader(),
                vec![source_node_id],
            )
            .await
            .expect("read replicated Chat change")
            .expect("replicated Chat tombstone content");
        let result = registry
            .apply_change(&change, merge_store)
            .await
            .expect("apply replicated Chat change");
        inbox
            .mark_applied(entry.author, &entry.key)
            .expect("mark replicated change applied");
        applied.push((change, result.status));
    }
    applied
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_agent_and_project_chat_deletes_converge_after_reconnect() {
    let network = NetworkSecret::generate();
    let dir_a = tempfile::tempdir().expect("node A tempdir");
    let dir_b = tempfile::tempdir().expect("node B tempdir");
    let inbox_dir = tempfile::tempdir().expect("inbox tempdir");
    let merge_dir = tempfile::tempdir().expect("merge tempdir");
    let device_a = DeviceIdentity::generate();
    let device_b = DeviceIdentity::generate();
    let node_a = SyncNode::bind_local(dir_a.path(), network.namespace_secret(), &device_a)
        .await
        .expect("bind node A");
    let node_b = SyncNode::bind_local(dir_b.path(), network.namespace_secret(), &device_b)
        .await
        .expect("bind node B");
    let sync_key = network.derive_sync_key();
    let log_a = OpaqueReplicaLog::new(
        node_a.doc().clone(),
        device_a.to_iroh_author(),
        sync_key.clone(),
        node_a.blobs_store().clone(),
    );
    let log_b = OpaqueReplicaLog::new(
        node_b.doc().clone(),
        device_b.to_iroh_author(),
        sync_key,
        node_b.blobs_store().clone(),
    );
    let inbox = InboundInbox::open(inbox_dir.path()).expect("open inbox");
    let merge_store = MergeStore::open(merge_dir.path()).expect("open Merge Store");
    let registry = chat_registry(dir_b.path(), &hex::encode(device_b.device_id_short()));
    let addresses = [
        ChatAddress::agent("agent-offline", "chat-agent-delete"),
        ChatAddress::project("project-offline", "chat-project-delete"),
    ];
    for address in &addresses {
        seed_chat(dir_b.path(), address);
        assert!(chat_exists(dir_b.path(), address));
        log_a
            .append_change(&ChatIndexSyncAdapter::delete_change(
                format!("delete-{}", address.identity_key()),
                address,
                hex::encode(device_a.device_id_short()),
                HlcTimestamp {
                    wall_ms: 100,
                    counter: 0,
                    device_id: device_a.device_id_short(),
                },
            ))
            .await
            .expect("append offline Chat tombstone");
    }

    assert!(addresses
        .iter()
        .all(|address| chat_exists(dir_b.path(), address)));

    rua_sync::transport::sync_now(&node_b, node_a.local_peer_addr(), Duration::from_secs(10))
        .await
        .expect("reconnect and sync local peers");
    let first_apply = apply_pending_changes(
        &inbox,
        &log_b,
        &node_b,
        node_a.node_id(),
        &registry,
        &merge_store,
    )
    .await;
    assert_eq!(first_apply.len(), 2);
    assert!(first_apply
        .iter()
        .all(|(_, status)| *status == RegistryApplyStatus::Applied));
    assert!(addresses
        .iter()
        .all(|address| !chat_exists(dir_b.path(), address)));

    rua_sync::transport::sync_now(&node_b, node_a.local_peer_addr(), Duration::from_secs(10))
        .await
        .expect("repeat reconnect sync");
    assert!(apply_pending_changes(
        &inbox,
        &log_b,
        &node_b,
        node_a.node_id(),
        &registry,
        &merge_store,
    )
    .await
    .is_empty());
    for (change, _) in first_apply {
        assert_eq!(
            registry
                .apply_change(&change, &merge_store)
                .await
                .expect("replay Chat tombstone")
                .status,
            RegistryApplyStatus::AlreadyApplied
        );
    }

    node_a.shutdown().await;
    node_b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_network_runtime_preserves_sync_blob_fetch_and_shutdown() {
    let network = NetworkSecret::generate();
    let dir_a = tempfile::tempdir().expect("node A tempdir");
    let dir_b = tempfile::tempdir().expect("node B tempdir");
    let dir_c = tempfile::tempdir().expect("node C tempdir");
    let inbox_dir = tempfile::tempdir().expect("inbox tempdir");
    let device_a = DeviceIdentity::generate();
    let device_b = DeviceIdentity::generate();
    let device_c = DeviceIdentity::generate();
    let node_a = SyncNode::bind_local(dir_a.path(), network.namespace_secret(), &device_a)
        .await
        .expect("bind node A");
    let node_b = SyncNode::bind_local(dir_b.path(), network.namespace_secret(), &device_b)
        .await
        .expect("bind node B");
    let node_c = SyncNode::bind_local(dir_c.path(), network.namespace_secret(), &device_c)
        .await
        .expect("bind node C");
    let sync_key = network.derive_sync_key();

    assert_eq!(
        node_a.network_runtime().diagnostics().local_device_id,
        hex::encode(device_a.device_id_short())
    );
    assert_eq!(
        node_a.network_runtime().diagnostics().network_id,
        hex::encode(network.namespace_id().as_bytes())
    );

    let log_a = OpaqueReplicaLog::new(
        node_a.doc().clone(),
        device_a.to_iroh_author(),
        sync_key.clone(),
        node_a.blobs_store().clone(),
    );
    let log_b = OpaqueReplicaLog::new(
        node_b.doc().clone(),
        device_b.to_iroh_author(),
        sync_key.clone(),
        node_b.blobs_store().clone(),
    );
    let change = SyncChange::new(
        "device-network-runtime-op",
        "device_network_runtime",
        "record-1",
        "upsert",
        hex::encode(device_a.device_id_short()),
        HlcTimestamp {
            wall_ms: 42,
            counter: 0,
            device_id: device_a.device_id_short(),
        },
        serde_json::json!({"value": "replicated through shared runtime"}),
    );
    log_a.append_change(&change).await.expect("append change");

    rua_sync::transport::sync_now(&node_b, node_a.local_peer_addr(), Duration::from_secs(10))
        .await
        .expect("sync local peers");
    rua_sync::transport::sync_now(&node_b, node_c.local_peer_addr(), Duration::from_secs(10))
        .await
        .expect("establish route to empty provider");

    let inbox = InboundInbox::open(inbox_dir.path()).expect("open inbox");
    let repair = inbox
        .repair_scan_doc_entries(node_b.doc())
        .await
        .expect("scan replicated entries");
    assert_eq!(repair.recorded, 1);
    let entry = inbox
        .pending_ready(u64::MAX)
        .expect("read pending entries")
        .into_iter()
        .next()
        .expect("replicated entry");
    let replicated = log_b
        .read_change_with_peer_download(
            entry.author,
            &entry.key,
            node_b.blob_downloader(),
            vec![node_a.node_id()],
        )
        .await
        .expect("fetch and decrypt replicated blob")
        .expect("replicated change");
    assert_eq!(replicated.op_id, change.op_id);
    assert_eq!(replicated.payload, change.payload);

    let first_payload = b"first retained downloader payload";
    let first_hash = log_a
        .put_encrypted_blob(first_payload)
        .await
        .expect("store first standalone encrypted blob");
    let downloaded_first = read_encrypted_blob_with_peer_download(
        node_b.blobs_store(),
        &sync_key,
        &first_hash,
        node_b.blob_downloader(),
        vec![node_c.node_id(), node_a.node_id()],
    )
    .await
    .expect("download first standalone encrypted blob")
    .expect("first standalone encrypted blob");
    assert_eq!(downloaded_first, first_payload);
    let first_download = node_b.blob_downloader().diagnostics();
    assert_eq!(first_download.requests, 1);
    assert_eq!(first_download.successes, 1);
    assert_eq!(first_download.failures, 0);
    assert_eq!(
        first_download
            .last
            .as_ref()
            .expect("first Blob diagnostics")
            .provider_order,
        vec![node_c.node_id().to_string(), node_a.node_id().to_string()]
    );
    let first_attempt = first_download.last.expect("first Blob diagnostics");
    assert_eq!(
        first_attempt.tried_providers,
        vec![node_c.node_id().to_string(), node_a.node_id().to_string()]
    );
    assert_eq!(
        first_attempt.failed_providers,
        vec![node_c.node_id().to_string()]
    );
    assert_eq!(
        first_attempt.selected_provider.as_deref(),
        Some(node_a.node_id().to_string().as_str())
    );

    let second_payload = b"second retained downloader payload";
    let second_hash = log_a
        .put_encrypted_blob(second_payload)
        .await
        .expect("store second standalone encrypted blob");
    let downloaded_second = read_encrypted_blob_with_peer_download(
        node_b.blobs_store(),
        &sync_key,
        &second_hash,
        node_b.blob_downloader(),
        vec![node_a.node_id()],
    )
    .await
    .expect("download second standalone encrypted blob")
    .expect("second standalone encrypted blob");
    assert_eq!(downloaded_second, second_payload);
    let second_download = node_b.blob_downloader().diagnostics();
    assert_eq!(second_download.requests, 2);
    assert_eq!(second_download.successes, 2);
    assert_eq!(second_download.failures, 0);

    let partial_payload = vec![0x5A; 16 * 1024];
    let partial_hash_hex = log_a
        .put_encrypted_blob(&partial_payload)
        .await
        .expect("store encrypted blob for continuation");
    let partial_hash: iroh_blobs::Hash = partial_hash_hex.parse().expect("parse continuation hash");
    let mut encrypted_reader = node_a.blobs_store().reader(partial_hash);
    let mut encrypted = Vec::new();
    encrypted_reader
        .read_to_end(&mut encrypted)
        .await
        .expect("read source ciphertext");
    let partial_ranges = ChunkRanges::chunk(0);
    let outboard = PreOrderMemOutboard::create(&encrypted, IROH_BLOCK_SIZE);
    assert_eq!(iroh_blobs::Hash::from(outboard.root), partial_hash);
    let mut partial_bao = Vec::new();
    partial_bao.extend_from_slice(&(encrypted.len() as u64).to_le_bytes());
    bao_tree::io::sync::encode_ranges_validated(
        &encrypted,
        &outboard,
        &partial_ranges,
        &mut partial_bao,
    )
    .expect("encode partial Bao");
    node_b
        .blobs_store()
        .blobs()
        .import_bao_bytes(partial_hash, partial_ranges, partial_bao)
        .await
        .expect("import partial ciphertext");
    assert!(matches!(
        node_b.blobs_store().status(partial_hash).await,
        Ok(BlobStatus::Partial { .. })
    ));

    let continued = read_encrypted_blob_with_peer_download(
        node_b.blobs_store(),
        &sync_key,
        &partial_hash_hex,
        node_b.blob_downloader(),
        vec![node_a.node_id()],
    )
    .await
    .expect("continue partial encrypted blob")
    .expect("continued encrypted blob");
    assert_eq!(continued, partial_payload);
    let continued_download = node_b.blob_downloader().diagnostics();
    assert_eq!(continued_download.requests, 3);
    assert_eq!(continued_download.successes, 3);
    assert_eq!(continued_download.failures, 0);

    let endpoint_a = node_a.endpoint().clone();
    let endpoint_b = node_b.endpoint().clone();
    let endpoint_c = node_c.endpoint().clone();
    node_a.shutdown().await;
    node_b.shutdown().await;
    node_c.shutdown().await;
    assert!(endpoint_a.is_closed());
    assert!(endpoint_b.is_closed());
    assert!(endpoint_c.is_closed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_now_waits_for_a_busy_peer_before_reporting_completion() {
    let network = NetworkSecret::generate();
    let source_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let source_device = DeviceIdentity::generate();
    let source = SyncNode::bind_local(
        source_dir.path(),
        network.namespace_secret(),
        &source_device,
    )
    .await
    .unwrap();
    let target = SyncNode::bind_local(
        target_dir.path(),
        network.namespace_secret(),
        &DeviceIdentity::generate(),
    )
    .await
    .unwrap();
    source
        .doc()
        .set_bytes(
            source_device.to_iroh_author().id(),
            bytes::Bytes::from_static(b"busy-peer-record"),
            bytes::Bytes::from_static(b"latest value"),
        )
        .await
        .unwrap();
    let peer = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .alpns(vec![iroh_docs::ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let mut peer_addr = iroh::EndpointAddr::new(peer.id());
    for mut socket in peer.bound_sockets() {
        if socket.ip().is_unspecified() {
            socket.set_ip(if socket.is_ipv4() {
                std::net::Ipv4Addr::LOCALHOST.into()
            } else {
                std::net::Ipv6Addr::LOCALHOST.into()
            });
        }
        peer_addr.addrs.insert(iroh::TransportAddr::Ip(socket));
    }
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let client = async {
        let result =
            rua_sync::transport::sync_now(&target, peer_addr, Duration::from_secs(10)).await;
        let _ = done_tx.send(());
        result
    };
    let server = async {
        let mut attempts = 0;
        loop {
            let incoming = tokio::select! {
                incoming = peer.accept() => incoming.unwrap(),
                _ = &mut done_rx => break,
            };
            // Reconciliation can also prompt a gossip connection; this peer serves docs only.
            let Ok(connection) = incoming.await else {
                continue;
            };
            attempts += 1;
            let busy = attempts == 1;
            let result = iroh_docs::net::handle_connection(
                source.sync_handle().clone(),
                connection,
                move |_, _| async move {
                    if busy {
                        iroh_docs::net::AcceptOutcome::Reject(
                            iroh_docs::net::AbortReason::AlreadySyncing,
                        )
                    } else {
                        iroh_docs::net::AcceptOutcome::Allow
                    }
                },
                None,
            )
            .await;
            if busy {
                assert!(matches!(
                    result,
                    Err(iroh_docs::net::AcceptError::Abort {
                        reason: iroh_docs::net::AbortReason::AlreadySyncing,
                        ..
                    })
                ));
            } else {
                result.unwrap();
            }
        }
        attempts
    };
    let (result, attempts) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(client, server)
    })
    .await
    .unwrap();
    assert!(
        result.is_ok(),
        "a busy peer must complete reconciliation before success: {result:?}, attempts={attempts}"
    );
    assert_eq!(attempts, 2);
    let hashes = rua_sync::transport::query_all_entry_hashes(&target)
        .await
        .unwrap();
    assert_eq!(
        hashes.len(),
        1,
        "the successful cycle must include the source record"
    );
    peer.close().await;
    source.shutdown().await;
    target.shutdown().await;
}

#[tokio::test]
async fn concurrent_sync_requests_exchange_both_writes() {
    let network = NetworkSecret::generate();
    let left_dir = tempfile::tempdir().unwrap();
    let right_dir = tempfile::tempdir().unwrap();
    let left_device = DeviceIdentity::generate();
    let right_device = DeviceIdentity::generate();
    let left = SyncNode::bind_local(left_dir.path(), network.namespace_secret(), &left_device)
        .await
        .unwrap();
    let right = SyncNode::bind_local(right_dir.path(), network.namespace_secret(), &right_device)
        .await
        .unwrap();
    for (node, device, key) in [
        (&left, &left_device, "left"),
        (&right, &right_device, "right"),
    ] {
        node.doc()
            .set_bytes(device.to_iroh_author().id(), key, key)
            .await
            .unwrap();
    }
    let (a, b) = tokio::join!(
        rua_sync::transport::sync_now(&left, right.local_peer_addr(), Duration::from_secs(5)),
        rua_sync::transport::sync_now(&right, left.local_peer_addr(), Duration::from_secs(5)),
    );
    for result in [a.unwrap(), b.unwrap()] {
        assert!(result.warnings.is_empty());
        assert_eq!(
            result.path_observation.route,
            rua_sync::transport::SyncRouteKind::Direct
        );
    }
    for node in [&left, &right] {
        assert_eq!(
            rua_sync::transport::query_all_entry_hashes(node)
                .await
                .unwrap()
                .len(),
            2
        );
    }
    left.shutdown().await;
    right.shutdown().await;
}

#[tokio::test]
async fn rejected_namespace_preserves_protocol_error_over_stream_close() {
    let left_dir = tempfile::tempdir().unwrap();
    let right_dir = tempfile::tempdir().unwrap();
    let left = SyncNode::bind_local(
        left_dir.path(),
        NetworkSecret::generate().namespace_secret(),
        &DeviceIdentity::generate(),
    )
    .await
    .unwrap();
    let right = SyncNode::bind_local(
        right_dir.path(),
        NetworkSecret::generate().namespace_secret(),
        &DeviceIdentity::generate(),
    )
    .await
    .unwrap();
    let error = iroh_docs::net::connect_and_sync(
        left.endpoint(),
        left.sync_handle(),
        left.namespace_id(),
        right.local_peer_addr(),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            error,
            iroh_docs::net::ConnectError::RemoteAbort(iroh_docs::net::AbortReason::NotFound)
        ),
        "protocol rejection was masked: {error:?}"
    );
    left.shutdown().await;
    right.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blob_download_survives_a_handshake_longer_than_one_second() {
    use iroh::protocol::ProtocolHandler;
    let dir = tempfile::tempdir().unwrap();
    let network = NetworkSecret::generate();
    let node = SyncNode::bind_local(
        dir.path(),
        network.namespace_secret(),
        &DeviceIdentity::generate(),
    )
    .await
    .unwrap();
    let provider = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .unwrap()
        .alpns(vec![iroh_blobs::ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let lookup = iroh::address_lookup::memory::MemoryLookup::new();
    lookup.add_endpoint_info(
        iroh::EndpointAddr::new(provider.id()).with_ip_addr(provider.bound_sockets()[0]),
    );
    node.endpoint().address_lookup().unwrap().add(lookup);
    let store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::default().into();
    let data = b"handshake progress must survive the pool's one-second default";
    let tag = store.blobs().add_bytes(data.as_slice()).await.unwrap();
    let protocol = iroh_blobs::BlobsProtocol::new(&store, None);
    let transfer = node.blob_downloader().download(
        tag.hash,
        vec![provider.id()],
        Duration::from_secs(5),
        "slow handshake",
    );
    let serve = async {
        let incoming = provider.accept().await.unwrap();
        // Model a recovering relay or mobile link before the first handshake response.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        if let Ok(conn) = incoming.await {
            protocol.accept(conn).await.ok();
        }
    };
    tokio::pin!(serve);
    let result = tokio::select! {
        result = transfer => result,
        _ = &mut serve => panic!("provider closed before the download completed"),
    };
    let downloaded = node.blobs_store().blobs().get_bytes(tag.hash).await;
    node.shutdown().await;
    provider.close().await;
    result.expect("a viable handshake must not be aborted after one second");
    assert_eq!(downloaded.unwrap().as_ref(), data);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blob_download_after_peer_restart_does_not_reuse_an_idle_dead_connection() {
    let key = iroh::SecretKey::generate();
    let old_key = key.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel();
    let old_host = std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")
                .unwrap()
                .secret_key(old_key)
                .bind()
                .await
                .unwrap();
            let store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::default().into();
            let tag = store
                .blobs()
                .add_bytes(b"before restart".as_slice())
                .await
                .unwrap();
            let router = iroh::protocol::Router::builder(endpoint.clone())
                .accept(
                    iroh_blobs::ALPN,
                    iroh_blobs::BlobsProtocol::new(&store, None),
                )
                .spawn();
            ready_tx
                .send((endpoint.bound_sockets()[0], tag.hash))
                .unwrap();
            kill_rx.await.unwrap();
            // Shut the runtime down before dropping handles: simulate process death,
            // with no graceful CONNECTION_CLOSE reaching the downloader.
            (endpoint, router, store, tag)
        })
    });
    let (old_addr, old_hash) = ready_rx.await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let network = NetworkSecret::generate();
    let node = SyncNode::bind_local(
        dir.path(),
        network.namespace_secret(),
        &DeviceIdentity::generate(),
    )
    .await
    .unwrap();
    let lookup = iroh::address_lookup::memory::MemoryLookup::new();
    lookup.add_endpoint_info(iroh::EndpointAddr::new(key.public()).with_ip_addr(old_addr));
    node.endpoint()
        .address_lookup()
        .unwrap()
        .add(lookup.clone());
    node.blob_downloader()
        .download(
            old_hash,
            vec![key.public()],
            Duration::from_secs(2),
            "before restart",
        )
        .await
        .unwrap();
    kill_tx.send(()).unwrap();
    let abandoned = tokio::task::spawn_blocking(move || old_host.join().unwrap())
        .await
        .unwrap();
    let provider = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .unwrap()
        .secret_key(key)
        .bind()
        .await
        .unwrap();
    let store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::default().into();
    let data = b"after restart";
    let tag = store.blobs().add_bytes(data.as_slice()).await.unwrap();
    let router = iroh::protocol::Router::builder(provider.clone())
        .accept(
            iroh_blobs::ALPN,
            iroh_blobs::BlobsProtocol::new(&store, None),
        )
        .spawn();
    lookup.add_endpoint_info(
        iroh::EndpointAddr::new(provider.id()).with_ip_addr(provider.bound_sockets()[0]),
    );
    let result = node
        .blob_downloader()
        .download(
            tag.hash,
            vec![provider.id()],
            Duration::from_secs(2),
            "after restart",
        )
        .await;
    let downloaded = node.blobs_store().blobs().get_bytes(tag.hash).await;
    drop(abandoned);
    router.shutdown().await.unwrap();
    node.shutdown().await;
    result.expect("an idle connection from the killed process must not stall fresh content");
    assert_eq!(downloaded.unwrap().as_ref(), data);
}
