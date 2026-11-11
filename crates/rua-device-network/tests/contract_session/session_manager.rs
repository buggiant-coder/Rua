use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use rua_device_network::device_list::{DeviceEntry, DeviceList};
use rua_device_network::{
    BoundDeviceNetworkMembership, DeviceNetworkBootstrap, DeviceNetworkIdentity,
    DeviceNetworkPreset, DeviceNetworkRuntime, DevicePeerDirectory, DeviceSessionBulkStream,
    DeviceSessionBulkTransferHandler, DeviceSessionCommandHandler, DeviceSessionConfig,
    DeviceSessionError, DeviceSessionFrameStream, DeviceSessionLiveActivityHandler,
    DeviceSessionManager, DeviceSessionOrigin, DeviceSessionState, DEVICE_SESSION_PROTOCOL_VERSION,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Barrier, Notify};

#[cfg(feature = "test-util")]
#[path = "delayed_transport.rs"]
mod delayed_transport;
#[cfg(feature = "test-util")]
use delayed_transport::DelayedNetwork;

#[derive(Debug, Default)]
struct EchoHandler {
    calls: AtomicUsize,
    hang_next: AtomicBool,
    hang_release: Notify,
    reject_next: AtomicBool,
}

#[async_trait]
impl DeviceSessionCommandHandler for EchoHandler {
    async fn handle_command(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.hang_next.swap(false, Ordering::SeqCst) {
            self.hang_release.notified().await;
        }
        if self.reject_next.swap(false, Ordering::SeqCst) {
            return Err("injected business rejection".to_string());
        }
        Ok(payload.to_vec())
    }
}

#[derive(Debug, Default)]
struct EchoLiveActivityHandler {
    calls: AtomicUsize,
    hang_next: AtomicBool,
    hang_release: Notify,
}

#[async_trait]
impl DeviceSessionLiveActivityHandler for EchoLiveActivityHandler {
    async fn subscribe_live_activity(
        &self,
        payload: &[u8],
    ) -> Result<DeviceSessionFrameStream, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.hang_next.swap(false, Ordering::SeqCst) {
            self.hang_release.notified().await;
        }
        let payload = payload.to_vec();
        Ok(Box::pin(
            futures::stream::once(async move { payload }).chain(futures::stream::pending()),
        ))
    }
}

#[derive(Debug, Default)]
struct EchoBulkTransferHandler {
    calls: AtomicUsize,
    fail_next: AtomicBool,
}

#[async_trait]
impl DeviceSessionBulkTransferHandler for EchoBulkTransferHandler {
    async fn handle_bulk_transfer(
        &self,
        mut stream: DeviceSessionBulkStream,
    ) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let payload = {
            let recv = stream.recv_mut();
            let len = recv.read_u32().await.map_err(|error| error.to_string())? as usize;
            let mut payload = vec![0; len];
            recv.read_exact(&mut payload)
                .await
                .map_err(|error| error.to_string())?;
            payload
        };
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err("injected Bulk Transfer failure".to_string());
        }
        {
            let send = stream.send_mut();
            send.write_u32(payload.len() as u32)
                .await
                .map_err(|error| error.to_string())?;
            send.write_all(&payload)
                .await
                .map_err(|error| error.to_string())?;
        }
        stream.finish().map_err(|error| error.to_string())
    }
}

#[derive(Debug, Default)]
struct SustainedBulkTransferHandler;

#[async_trait]
impl DeviceSessionBulkTransferHandler for SustainedBulkTransferHandler {
    async fn handle_bulk_transfer(
        &self,
        mut stream: DeviceSessionBulkStream,
    ) -> Result<(), String> {
        loop {
            let len = stream
                .recv_mut()
                .read_u32()
                .await
                .map_err(|error| error.to_string())? as usize;
            if len == 0 {
                break;
            }
            let mut payload = vec![0; len];
            stream
                .recv_mut()
                .read_exact(&mut payload)
                .await
                .map_err(|error| error.to_string())?;
            tokio::time::sleep(Duration::from_millis(2)).await;
            stream
                .send_mut()
                .write_u8(1)
                .await
                .map_err(|error| error.to_string())?;
        }
        stream.finish().map_err(|error| error.to_string())
    }
}

struct TestRuntime {
    _dir: tempfile::TempDir,
    sync_dir: std::path::PathBuf,
    runtime: DeviceNetworkRuntime,
    device_id: String,
    addr: iroh::EndpointAddr,
    handler: Arc<EchoHandler>,
}

async fn build_runtime(
    secret_byte: u8,
    network_secret: [u8; 32],
    peers: Vec<DeviceEntry>,
    config: DeviceSessionConfig,
) -> TestRuntime {
    let dir = tempfile::tempdir().expect("tempdir");
    let sync_dir = dir.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(
        sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&DeviceList {
            devices: peers,
            ..Default::default()
        })
        .expect("serialize peers"),
    )
    .expect("write peers");
    let secret = iroh::SecretKey::from_bytes(&[secret_byte; 32]);
    let identity = DeviceNetworkIdentity::new(secret);
    let device_id = hex::encode(identity.local_device_id());
    let bootstrap = DeviceNetworkBootstrap::bind(
        DeviceNetworkPreset::Local,
        identity,
        BoundDeviceNetworkMembership::new(network_secret),
        DevicePeerDirectory::from_sync_dir(&sync_dir),
    )
    .await
    .expect("bind runtime");
    let handler = Arc::new(EchoHandler::default());
    let runtime = bootstrap.spawn_with_session_config(Vec::new(), Some(handler.clone()), config);
    let mut addr = iroh::EndpointAddr::new(runtime.endpoint().id());
    for socket in runtime.endpoint().bound_sockets() {
        let socket = match socket {
            std::net::SocketAddr::V4(socket) if socket.ip().is_unspecified() => {
                (std::net::Ipv4Addr::LOCALHOST, socket.port()).into()
            }
            std::net::SocketAddr::V6(socket) if socket.ip().is_unspecified() => {
                (std::net::Ipv6Addr::LOCALHOST, socket.port()).into()
            }
            socket => socket,
        };
        addr.addrs.insert(iroh::TransportAddr::Ip(socket));
    }
    TestRuntime {
        _dir: dir,
        sync_dir,
        runtime,
        device_id,
        addr,
        handler,
    }
}

#[cfg(feature = "test-util")]
async fn build_runtime_on_delayed_network(
    secret_byte: u8,
    network_secret: [u8; 32],
    peers: Vec<DeviceEntry>,
    config: DeviceSessionConfig,
    network: &DelayedNetwork,
) -> TestRuntime {
    let dir = tempfile::tempdir().expect("tempdir");
    let sync_dir = dir.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(
        sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&DeviceList {
            devices: peers,
            ..Default::default()
        })
        .expect("serialize peers"),
    )
    .expect("write peers");
    let secret = iroh::SecretKey::from_bytes(&[secret_byte; 32]);
    let identity = DeviceNetworkIdentity::new(secret);
    let device_id = hex::encode(identity.local_device_id());
    let transport = network.transport(identity.endpoint_secret().public());
    let builder = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .clear_ip_transports()
        .clear_relay_transports()
        .add_custom_transport(transport);
    let bootstrap = DeviceNetworkBootstrap::bind_with_endpoint_builder(
        builder,
        identity,
        BoundDeviceNetworkMembership::new(network_secret),
        DevicePeerDirectory::from_sync_dir(&sync_dir),
    )
    .await
    .expect("bind isolated delayed runtime");
    let handler = Arc::new(EchoHandler::default());
    let runtime = bootstrap.spawn_with_session_config(Vec::new(), Some(handler.clone()), config);
    let addr = DelayedNetwork::endpoint_addr(runtime.endpoint().id());
    TestRuntime {
        _dir: dir,
        sync_dir,
        runtime,
        device_id,
        addr,
        handler,
    }
}

#[cfg(feature = "test-util")]
async fn slow_pair() -> (TestRuntime, TestRuntime, DelayedNetwork) {
    let network_secret = [101; 32];
    let config = DeviceSessionConfig {
        connect_timeout: Duration::from_secs(10),
        handshake_timeout: Duration::from_secs(10),
        ..test_config()
    };
    let network = DelayedNetwork::new(Duration::from_millis(650));
    let b = build_runtime_on_delayed_network(
        102,
        network_secret,
        vec![peer_entry(101)],
        config.clone(),
        &network,
    )
    .await;
    let a = build_runtime_on_delayed_network(
        101,
        network_secret,
        vec![peer_entry(102)],
        config,
        &network,
    )
    .await;
    (a, b, network)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_directory_reconcile_closes_removed_session_and_rejects_new_streams() {
    let network_secret = [41; 32];
    let b = build_runtime(42, network_secret, vec![peer_entry(41)], test_config()).await;
    let a = build_runtime(
        41,
        network_secret,
        vec![peer_entry_with_direct_addrs(42, direct_addrs(&b.addr))],
        test_config(),
    )
    .await;

    a.runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"before removal",
            Duration::from_secs(5),
        )
        .await
        .expect("initial command");
    std::fs::write(
        a.sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&DeviceList::default()).expect("serialize empty Device List"),
    )
    .expect("remove peer from Device List");

    let removed = a
        .runtime
        .session_manager()
        .reconcile_peer_directory()
        .expect("reconcile Device Sessions");
    assert_eq!(removed, vec![b.device_id.clone()]);
    let error = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"after removal",
            Duration::from_secs(2),
        )
        .await
        .expect_err("removed Device must not receive a new logical stream");
    assert!(matches!(error, DeviceSessionError::PeerNotPaired(_)));

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

fn peer_entry(secret_byte: u8) -> DeviceEntry {
    peer_entry_with_direct_addrs(secret_byte, Vec::new())
}

fn write_peer_list(sync_dir: &std::path::Path, peer: DeviceEntry) {
    std::fs::write(
        sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&DeviceList {
            devices: vec![peer],
            ..Default::default()
        })
        .expect("serialize peer list"),
    )
    .expect("write peer list");
}

fn peer_entry_with_direct_addrs(secret_byte: u8, direct_addrs: Vec<String>) -> DeviceEntry {
    let secret = iroh::SecretKey::from_bytes(&[secret_byte; 32]);
    let public_key = *secret.public().as_bytes();
    let mut device_id = [0; 8];
    device_id.copy_from_slice(&public_key[..8]);
    DeviceEntry {
        device_id,
        public_key,
        label: format!("device-{secret_byte}"),
        friendly_name: format!("Device {secret_byte}"),
        added_at_ms: 1,
        last_seen_ms: 1,
        direct_addrs,
    }
}

fn direct_addrs(addr: &iroh::EndpointAddr) -> Vec<String> {
    addr.addrs
        .iter()
        .filter_map(|addr| match addr {
            iroh::TransportAddr::Ip(addr) => Some(addr.to_string()),
            _ => None,
        })
        .collect()
}

fn test_config() -> DeviceSessionConfig {
    DeviceSessionConfig {
        connect_timeout: Duration::from_secs(2),
        handshake_timeout: Duration::from_secs(2),
        reconnect_attempts: 1,
        ..Default::default()
    }
}

#[test]
fn peer_directory_keeps_bare_identity_for_address_lookup() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sync_dir = dir.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(
        sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&DeviceList {
            devices: vec![peer_entry(82)],
            ..Default::default()
        })
        .expect("serialize Device List"),
    )
    .expect("write Device List");
    let peer_id = iroh::SecretKey::from_bytes(&[82; 32]).public();
    let mut device_id = [0; 8];
    device_id.copy_from_slice(&peer_id.as_bytes()[..8]);

    let addr = DevicePeerDirectory::from_sync_dir(&sync_dir)
        .endpoint_addr_by_device_id(&device_id)
        .expect("read peer")
        .expect("paired peer");

    assert_eq!(addr.id, peer_id);
    assert!(
        addr.addrs.is_empty(),
        "Address Lookup, not the local home relay, resolves peer transport addresses"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_only_command_resolves_peer_address_inside_session_manager() {
    let network_secret = [8; 32];
    let b = build_runtime(82, network_secret, vec![peer_entry(81)], test_config()).await;
    let a = build_runtime(
        81,
        network_secret,
        vec![peer_entry_with_direct_addrs(82, direct_addrs(&b.addr))],
        test_config(),
    )
    .await;

    let response = a
        .runtime
        .session_manager()
        .command_to(&b.device_id, b"resolved", Duration::from_secs(5))
        .await
        .expect("target-only command");

    assert_eq!(response, b"resolved");
    assert_eq!(
        a.runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        1
    );
    a.runtime.shutdown().await.expect("a shutdown");
    b.runtime.shutdown().await.expect("b shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_direct_session_seeds_shared_address_resolution_without_new_dial() {
    let network_secret = [18; 32];
    let b = build_runtime(92, network_secret, vec![peer_entry(91)], test_config()).await;
    let a = build_runtime(
        91,
        network_secret,
        vec![peer_entry_with_direct_addrs(92, direct_addrs(&b.addr))],
        test_config(),
    )
    .await;

    a.runtime
        .session_manager()
        .command_to(&b.device_id, b"prove direct path", Duration::from_secs(5))
        .await
        .expect("establish direct Device Session");
    let dials_before = a
        .runtime
        .session_manager()
        .diagnostics()
        .physical_dial_attempts;
    write_peer_list(&a.sync_dir, peer_entry(92));

    let resolved = a
        .runtime
        .resolve_peer_addr(b.runtime.endpoint().id())
        .await
        .expect("resolve peer")
        .expect("paired peer");

    assert!(
        resolved
            .addrs
            .iter()
            .any(|addr| matches!(addr, iroh::TransportAddr::Ip(_))),
        "active direct Device Session path must remain available to Docs/Blob resolution"
    );
    assert_eq!(
        a.runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        dials_before,
        "address resolution must not open another Device Session"
    );

    a.runtime.shutdown().await.expect("a shutdown");
    b.runtime.shutdown().await.expect("b shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_is_lazy_and_repeated_commands_reuse_one_generation() {
    let network_secret = [9; 32];
    let a = build_runtime(1, network_secret, vec![peer_entry(2)], test_config()).await;
    let b = build_runtime(2, network_secret, vec![peer_entry(1)], test_config()).await;

    let initial = a.runtime.session_manager().diagnostics();
    assert_eq!(initial.sessions.len(), 1);
    assert_eq!(initial.sessions[0].state, DeviceSessionState::NotConnected);
    assert_eq!(initial.sessions[0].generation, None);
    assert_eq!(
        a.runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        0
    );

    let first = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"first",
            Duration::from_secs(5),
        )
        .await
        .expect("first command");
    let first_generation = a.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("generation");
    let second = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"second",
            Duration::from_secs(5),
        )
        .await
        .expect("second command");
    let diagnostics = a.runtime.session_manager().diagnostics();

    assert_eq!(first, b"first");
    assert_eq!(second, b"second");
    assert_eq!(b.handler.calls.load(Ordering::SeqCst), 2);
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions.len(), 1);
    assert_eq!(diagnostics.sessions[0].generation, Some(first_generation));
    assert_eq!(diagnostics.sessions[0].state, DeviceSessionState::Connected);
    assert_eq!(diagnostics.sessions[0].path_class, "direct");
    assert!(diagnostics.sessions[0].rtt_ms.is_some());
    assert!(diagnostics.sessions[0].tx_bytes > 0);
    assert!(diagnostics.sessions[0].rx_bytes > 0);
    assert!(diagnostics.sessions[0].state_changed_at_ms.is_some());

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_change_notification_probes_and_keeps_a_healthy_generation() {
    let network_secret = [45; 32];
    let a = build_runtime(45, network_secret, vec![peer_entry(46)], test_config()).await;
    let b = build_runtime(46, network_secret, vec![peer_entry(45)], test_config()).await;

    a.runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"before-network-change",
            Duration::from_secs(5),
        )
        .await
        .expect("initial command");
    let generation = a.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("generation");

    let report = a.runtime.session_manager().notify_network_change().await;
    let diagnostics = a.runtime.session_manager().diagnostics();

    assert_eq!(report.probed_sessions, 1);
    assert_eq!(report.invalidated_sessions, 0);
    assert_eq!(diagnostics.network_change_notifications, 1);
    assert_eq!(diagnostics.network_change_probe_failures, 0);
    assert_eq!(diagnostics.sessions[0].generation, Some(generation));

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connectivity_changes_wake_on_session_transitions_not_business_rejection() {
    let network_secret = [46; 32];
    let a = build_runtime(46, network_secret, vec![peer_entry(47)], test_config()).await;
    let b = build_runtime(47, network_secret, vec![peer_entry(46)], test_config()).await;
    let manager = a.runtime.session_manager();
    let mut changes = manager.subscribe_connectivity_changes();

    assert!(
        tokio::time::timeout(Duration::from_millis(50), changes.changed())
            .await
            .is_err(),
        "subscribing must not synthesize a change"
    );

    manager
        .command(
            &b.device_id,
            b.addr.clone(),
            b"connect",
            Duration::from_secs(5),
        )
        .await
        .expect("establish session");
    tokio::time::timeout(Duration::from_secs(1), changes.changed())
        .await
        .expect("connected change timeout")
        .expect("connected change");
    let connected_revision = *changes.borrow_and_update();
    assert!(connected_revision > 0);

    b.handler.reject_next.store(true, Ordering::SeqCst);
    manager
        .command(
            &b.device_id,
            b.addr.clone(),
            b"reject",
            Duration::from_secs(5),
        )
        .await
        .expect_err("business rejection");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), changes.changed())
            .await
            .is_err(),
        "business rejection must not publish a connectivity change"
    );

    b.runtime.shutdown().await.expect("shutdown B");
    tokio::time::timeout(Duration::from_secs(2), changes.changed())
        .await
        .expect("disconnect change timeout")
        .expect("disconnect change");
    assert_eq!(
        manager.connectivity_state(&b.device_id).unwrap(),
        DeviceSessionState::NotConnected
    );

    a.runtime.shutdown().await.expect("shutdown A");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connectivity_projection_is_ordered_and_includes_idle_paired_devices() {
    let network_secret = [31; 32];
    let runtime = build_runtime(
        31,
        network_secret,
        vec![peer_entry(33), peer_entry(32)],
        test_config(),
    )
    .await;

    let connectivity = runtime.runtime.connectivity();
    assert_eq!(connectivity.len(), 2);
    assert!(connectivity[0].peer_device_id < connectivity[1].peer_device_id);
    assert!(connectivity
        .iter()
        .all(|peer| peer.state == DeviceSessionState::NotConnected));

    runtime.runtime.shutdown().await.expect("shutdown runtime");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_business_rejection_keeps_the_shared_session_connected() {
    let network_secret = [34; 32];
    let a = build_runtime(34, network_secret, vec![peer_entry(35)], test_config()).await;
    let b = build_runtime(35, network_secret, vec![peer_entry(34)], test_config()).await;

    a.runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"before",
            Duration::from_secs(5),
        )
        .await
        .expect("initial command");
    let generation = a.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("generation");
    b.handler.reject_next.store(true, Ordering::SeqCst);

    let error = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"reject",
            Duration::from_secs(5),
        )
        .await
        .expect_err("business rejection");
    assert!(matches!(error, DeviceSessionError::CommandRejected(_)));

    let diagnostics = a.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions[0].generation, Some(generation));
    assert_eq!(diagnostics.sessions[0].state, DeviceSessionState::Connected);

    let response = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"after",
            Duration::from_secs(5),
        )
        .await
        .expect("command after business rejection");
    assert_eq!(response, b"after");

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_activity_switch_reuses_the_command_session_generation() {
    let network_secret = [13; 32];
    let a = build_runtime(13, network_secret, vec![peer_entry(14)], test_config()).await;
    let b = build_runtime(14, network_secret, vec![peer_entry(13)], test_config()).await;
    let live_handler = Arc::new(EchoLiveActivityHandler::default());
    b.runtime
        .session_manager()
        .set_live_activity_handler(live_handler.clone());

    let command = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"command",
            Duration::from_secs(5),
        )
        .await
        .expect("command");
    assert_eq!(command, b"command");
    let generation = a.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("session generation");

    let mut first = a
        .runtime
        .session_manager()
        .open_live_activity(
            &b.device_id,
            b.addr.clone(),
            b"chat-a",
            Duration::from_secs(5),
        )
        .await
        .expect("first Live Activity stream");
    assert_eq!(
        first.next_frame().await.expect("first frame"),
        Some(b"chat-a".to_vec())
    );
    drop(first);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if b.runtime.session_manager().diagnostics().sessions[0].active_logical_streams == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first Live Activity stream should close");

    let mut second = a
        .runtime
        .session_manager()
        .open_live_activity(
            &b.device_id,
            b.addr.clone(),
            b"chat-b",
            Duration::from_secs(5),
        )
        .await
        .expect("second Live Activity stream");
    assert_eq!(
        second.next_frame().await.expect("second frame"),
        Some(b"chat-b".to_vec())
    );

    let diagnostics = a.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions[0].generation, Some(generation));
    assert_eq!(live_handler.calls.load(Ordering::SeqCst), 2);

    drop(second);
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_transfer_reuses_command_and_live_activity_session_generation() {
    let network_secret = [15; 32];
    let a = build_runtime(15, network_secret, vec![peer_entry(16)], test_config()).await;
    let b = build_runtime(16, network_secret, vec![peer_entry(15)], test_config()).await;
    let live_handler = Arc::new(EchoLiveActivityHandler::default());
    let bulk_handler = Arc::new(EchoBulkTransferHandler::default());
    b.runtime
        .session_manager()
        .set_live_activity_handler(live_handler);
    b.runtime
        .session_manager()
        .set_bulk_transfer_handler(bulk_handler.clone());

    a.runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"command",
            Duration::from_secs(5),
        )
        .await
        .expect("command");
    let generation = a.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("generation");

    let mut bulk = a
        .runtime
        .session_manager()
        .open_bulk_transfer(&b.device_id, b.addr.clone(), Duration::from_secs(5))
        .await
        .expect("Bulk Transfer stream");
    assert_eq!(bulk.generation(), generation);
    assert_eq!(bulk.send_mut().priority().expect("Bulk priority"), -1);
    bulk.send_mut().write_u32(4).await.expect("bulk length");
    bulk.send_mut()
        .write_all(b"bulk")
        .await
        .expect("bulk payload");
    assert_eq!(bulk.recv_mut().read_u32().await.expect("echo length"), 4);
    let mut echoed = [0; 4];
    bulk.recv_mut()
        .read_exact(&mut echoed)
        .await
        .expect("echo payload");
    assert_eq!(&echoed, b"bulk");
    drop(bulk);

    let mut live = a
        .runtime
        .session_manager()
        .open_live_activity(
            &b.device_id,
            b.addr.clone(),
            b"live",
            Duration::from_secs(5),
        )
        .await
        .expect("Live Activity stream");
    assert_eq!(
        live.next_frame().await.expect("Live Activity frame"),
        Some(b"live".to_vec())
    );
    let diagnostics = a.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions[0].generation, Some(generation));
    assert_eq!(bulk_handler.calls.load(Ordering::SeqCst), 1);

    drop(live);
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_transfer_failure_does_not_replace_session_or_break_command() {
    let network_secret = [17; 32];
    let a = build_runtime(17, network_secret, vec![peer_entry(18)], test_config()).await;
    let b = build_runtime(18, network_secret, vec![peer_entry(17)], test_config()).await;
    let bulk_handler = Arc::new(EchoBulkTransferHandler::default());
    bulk_handler.fail_next.store(true, Ordering::SeqCst);
    b.runtime
        .session_manager()
        .set_bulk_transfer_handler(bulk_handler);

    let mut bulk = a
        .runtime
        .session_manager()
        .open_bulk_transfer(&b.device_id, b.addr.clone(), Duration::from_secs(5))
        .await
        .expect("Bulk Transfer stream");
    let generation = bulk.generation();
    bulk.send_mut().write_u32(4).await.expect("bulk length");
    bulk.send_mut()
        .write_all(b"fail")
        .await
        .expect("bulk payload");
    bulk.recv_mut()
        .read_u32()
        .await
        .expect_err("failed stream should close");
    drop(bulk);

    let response = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"still-usable",
            Duration::from_secs(5),
        )
        .await
        .expect("command after Bulk failure");
    assert_eq!(response, b"still-usable");
    let diagnostics = a.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions[0].generation, Some(generation));

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sustained_bulk_preserves_command_and_live_activity_latency_budget() {
    let network_secret = [19; 32];
    let a = build_runtime(19, network_secret, vec![peer_entry(20)], test_config()).await;
    let b = build_runtime(20, network_secret, vec![peer_entry(19)], test_config()).await;
    b.runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(EchoLiveActivityHandler::default()));
    b.runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(SustainedBulkTransferHandler));

    let manager = a.runtime.session_manager().clone();
    let peer_device_id = b.device_id.clone();
    let peer_addr = b.addr.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let transfer = tokio::spawn(async move {
        let mut stream = manager
            .open_bulk_transfer(&peer_device_id, peer_addr, Duration::from_secs(5))
            .await
            .expect("Bulk Transfer stream");
        let generation = stream.generation();
        let payload = vec![7u8; 64 * 1024];
        let mut started_tx = Some(started_tx);
        for _ in 0..256 {
            stream
                .send_mut()
                .write_u32(payload.len() as u32)
                .await
                .expect("chunk length");
            stream
                .send_mut()
                .write_all(&payload)
                .await
                .expect("chunk payload");
            assert_eq!(stream.recv_mut().read_u8().await.expect("chunk ack"), 1);
            if let Some(started_tx) = started_tx.take() {
                let _ = started_tx.send(());
            }
        }
        stream.send_mut().write_u32(0).await.expect("finish marker");
        stream.finish().expect("finish Bulk stream");
        generation
    });
    started_rx.await.expect("Bulk Transfer started");
    let generation = a.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("generation");

    let command_started = Instant::now();
    let command = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"interactive-command",
            Duration::from_secs(2),
        )
        .await
        .expect("Command during Bulk Transfer");
    let command_latency = command_started.elapsed();
    assert_eq!(command, b"interactive-command");

    let live_started = Instant::now();
    let mut live = a
        .runtime
        .session_manager()
        .open_live_activity(
            &b.device_id,
            b.addr.clone(),
            b"interactive-live",
            Duration::from_secs(2),
        )
        .await
        .expect("Live Activity during Bulk Transfer");
    assert_eq!(
        live.next_frame().await.expect("Live Activity frame"),
        Some(b"interactive-live".to_vec())
    );
    let live_latency = live_started.elapsed();
    drop(live);

    assert!(
        command_latency < Duration::from_millis(500),
        "Command latency during Bulk Transfer was {command_latency:?}"
    );
    assert!(
        live_latency < Duration::from_millis(500),
        "Live Activity latency during Bulk Transfer was {live_latency:?}"
    );
    assert_eq!(transfer.await.expect("Bulk Transfer task"), generation);
    let diagnostics = a.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions[0].generation, Some(generation));

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnect_reconnects_with_a_new_generation() {
    let network_secret = [8; 32];
    let a = build_runtime(3, network_secret, vec![peer_entry(4)], test_config()).await;
    let b = build_runtime(4, network_secret, vec![peer_entry(3)], test_config()).await;

    a.runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"before",
            Duration::from_secs(5),
        )
        .await
        .expect("initial command");
    let first_generation = a.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("first generation");
    a.runtime
        .session_manager()
        .disconnect_peer(&b.device_id)
        .await
        .expect("disconnect");

    let response = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"after",
            Duration::from_secs(5),
        )
        .await
        .expect("reconnected command");
    let diagnostics = a.runtime.session_manager().diagnostics();
    let second_generation = diagnostics.sessions[0]
        .generation
        .expect("second generation");

    assert_eq!(response, b"after");
    assert!(second_generation > first_generation);
    assert_eq!(diagnostics.physical_dial_attempts, 2);

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[cfg(feature = "test-util")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_command_keeps_a_healthy_generation_with_rtt_over_one_second() {
    let (a, b, network) = health_probe::warm_slow_pair().await;
    let manager = a.runtime.session_manager();
    let before = manager.diagnostics();
    let generation = before.sessions[0].generation.expect("connected generation");
    assert!(
        before.sessions[0].rtt_ms.expect("measured QUIC RTT") > 1000,
        "slow path diagnostics: {:?}",
        before.sessions[0]
    );

    b.handler.hang_next.store(true, Ordering::SeqCst);
    let started = Instant::now();
    let error = manager
        .command_once_to(&b.device_id, b"mutation", Duration::from_millis(100))
        .await
        .expect_err("caller deadline");
    assert!(matches!(
        error,
        DeviceSessionError::Timeout {
            stage: "command exchange",
            ..
        }
    ));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "health recovery must not extend the caller deadline"
    );
    // The independent health stream needs a real 1.3s round trip. A 1s fixed
    // health timer closes this otherwise healthy physical connection first.
    tokio::time::sleep(Duration::from_millis(1800)).await;
    let after = manager.diagnostics();
    assert_eq!(
        after.sessions[0].generation,
        Some(generation),
        "a probe slower than 1s must not retire a healthy WAN generation"
    );
    assert_eq!(after.sessions[0].state, DeviceSessionState::Connected);
    assert_eq!(after.physical_dial_attempts, 1);
    assert_eq!(
        b.handler.calls.load(Ordering::SeqCst),
        2,
        "a timed-out mutation must not be replayed"
    );
    b.handler.hang_release.notify_waiters();
    assert_eq!(
        manager
            .command_to(&b.device_id, b"after", Duration::from_secs(5))
            .await
            .expect("reuse healthy generation"),
        b"after"
    );
    assert_eq!(
        manager.diagnostics().sessions[0].generation,
        Some(generation)
    );
    assert_eq!(manager.diagnostics().physical_dial_attempts, 1);

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
    network.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_logical_operation_keeps_a_responsive_generation() {
    let network_secret = [48; 32];
    let a = build_runtime(48, network_secret, vec![peer_entry(49)], test_config()).await;
    let b = build_runtime(49, network_secret, vec![peer_entry(48)], test_config()).await;

    a.runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"before-timeout",
            Duration::from_secs(5),
        )
        .await
        .expect("initial command");
    let first_generation = a.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("first generation");

    b.handler.hang_next.store(true, Ordering::SeqCst);
    let error = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"timeout",
            Duration::from_millis(100),
        )
        .await
        .expect_err("hung operation must time out");
    assert!(
        matches!(error, DeviceSessionError::Timeout { stage: "command exchange", budget_ms } if budget_ms > 0)
    );
    assert!(error.to_string().contains("command exchange"));
    let timed_out = a.runtime.session_manager().diagnostics();
    assert_eq!(timed_out.sessions[0].generation, Some(first_generation));
    assert_eq!(timed_out.sessions[0].state, DeviceSessionState::Connected);

    b.handler.hang_release.notify_waiters();
    let response = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"after-timeout",
            Duration::from_secs(5),
        )
        .await
        .expect("next command reconnects");
    let reconnected = a.runtime.session_manager().diagnostics();
    assert_eq!(response, b"after-timeout");
    assert_eq!(reconnected.sessions[0].generation, Some(first_generation));
    assert_eq!(reconnected.physical_dial_attempts, 1);

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_live_activity_keeps_a_responsive_generation() {
    let network_secret = [50; 32];
    let a = build_runtime(50, network_secret, vec![peer_entry(51)], test_config()).await;
    let b = build_runtime(51, network_secret, vec![peer_entry(50)], test_config()).await;
    let live_handler = Arc::new(EchoLiveActivityHandler::default());
    b.runtime
        .session_manager()
        .set_live_activity_handler(live_handler.clone());

    a.runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"before-timeout",
            Duration::from_secs(5),
        )
        .await
        .expect("initial command");
    let first_generation = a.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("first generation");

    live_handler.hang_next.store(true, Ordering::SeqCst);
    let error = a
        .runtime
        .session_manager()
        .open_live_activity(
            &b.device_id,
            b.addr.clone(),
            b"timeout",
            Duration::from_millis(100),
        )
        .await
        .expect_err("hung Live Activity subscription must time out");
    assert!(
        matches!(error, DeviceSessionError::Timeout { stage: "Live Activity subscription", budget_ms } if budget_ms > 0)
    );
    assert!(error.to_string().contains("Live Activity subscription"));
    let timed_out = a.runtime.session_manager().diagnostics();
    assert_eq!(timed_out.sessions[0].generation, Some(first_generation));
    assert_eq!(timed_out.sessions[0].state, DeviceSessionState::Connected);

    live_handler.hang_release.notify_waiters();
    let mut live = a
        .runtime
        .session_manager()
        .open_live_activity(
            &b.device_id,
            b.addr.clone(),
            b"after-timeout",
            Duration::from_secs(5),
        )
        .await
        .expect("next Live Activity reuses the responsive generation");
    assert_eq!(
        live.next_frame().await.expect("Live Activity frame"),
        Some(b"after-timeout".to_vec())
    );
    let recovered = a.runtime.session_manager().diagnostics();
    assert_eq!(recovered.sessions[0].generation, Some(first_generation));
    assert_eq!(recovered.physical_dial_attempts, 1);

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restarted_peer_with_same_identity_replaces_its_previous_connection() {
    let network_secret = [38; 32];
    let a_before = build_runtime(38, network_secret, vec![peer_entry(39)], test_config()).await;
    let b = build_runtime(39, network_secret, vec![peer_entry(38)], test_config()).await;

    a_before
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"before-restart",
            Duration::from_secs(5),
        )
        .await
        .expect("initial command");
    let first_generation = b.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("first generation");

    let a_after = build_runtime(38, network_secret, vec![peer_entry(39)], test_config()).await;
    let response = a_after
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"after-restart",
            Duration::from_secs(5),
        )
        .await
        .expect("restarted peer command");
    let diagnostics = b.runtime.session_manager().diagnostics();
    let second_generation = diagnostics.sessions[0]
        .generation
        .expect("second generation");

    assert_eq!(response, b"after-restart");
    assert!(second_generation > first_generation);
    assert_eq!(diagnostics.session_replacements, 1);

    a_after.runtime.shutdown().await.expect("shutdown A after");
    a_before
        .runtime
        .shutdown()
        .await
        .expect("shutdown A before");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replacement_connection_terminates_old_live_stream_after_bounded_drain() {
    let network_secret = [50; 32];
    let a_before = build_runtime(50, network_secret, vec![peer_entry(51)], test_config()).await;
    let b = build_runtime(51, network_secret, vec![peer_entry(50)], test_config()).await;
    b.runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(EchoLiveActivityHandler::default()));

    let mut old_stream = a_before
        .runtime
        .session_manager()
        .open_live_activity(
            &b.device_id,
            b.addr.clone(),
            b"old-live-stream",
            Duration::from_secs(5),
        )
        .await
        .expect("old live stream");
    assert_eq!(
        old_stream.next_frame().await.expect("first frame"),
        Some(b"old-live-stream".to_vec())
    );

    let a_after = build_runtime(50, network_secret, vec![peer_entry(51)], test_config()).await;
    a_after
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"replacement",
            Duration::from_secs(5),
        )
        .await
        .expect("replacement command");

    tokio::time::timeout(Duration::from_secs(2), old_stream.next_frame())
        .await
        .expect("old Live Activity stream must not drain forever")
        .expect_err("replacement closes the old physical connection");

    a_after.runtime.shutdown().await.expect("shutdown A after");
    a_before
        .runtime
        .shutdown()
        .await
        .expect("shutdown A before");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_shutdown_closes_sessions_and_rejects_new_streams() {
    let network_secret = [36; 32];
    let a = build_runtime(36, network_secret, vec![peer_entry(37)], test_config()).await;
    let b = build_runtime(37, network_secret, vec![peer_entry(36)], test_config()).await;
    let manager = a.runtime.session_manager().clone();

    manager
        .command(
            &b.device_id,
            b.addr.clone(),
            b"before-shutdown",
            Duration::from_secs(5),
        )
        .await
        .expect("establish session");
    a.runtime.shutdown().await.expect("shutdown A");

    let error = manager
        .command(
            &b.device_id,
            b.addr.clone(),
            b"after-shutdown",
            Duration::from_secs(1),
        )
        .await
        .expect_err("shut down manager must reject new streams");
    assert!(matches!(error, DeviceSessionError::Closed(_)));
    assert_eq!(
        manager.diagnostics().sessions[0].state,
        DeviceSessionState::NotConnected
    );

    b.runtime
        .session_manager()
        .command(
            &a.device_id,
            a.addr.clone(),
            b"inbound-after-shutdown",
            Duration::from_secs(1),
        )
        .await
        .expect_err("shut down runtime must reject inbound reconnects");

    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_reconnect_attempt_is_counted_once() {
    let network_secret = [10; 32];
    let a = build_runtime(11, network_secret, vec![peer_entry(12)], test_config()).await;
    let b = build_runtime(12, network_secret, vec![peer_entry(11)], test_config()).await;

    a.runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"before",
            Duration::from_secs(5),
        )
        .await
        .expect("initial command");
    b.runtime.shutdown().await.expect("shutdown B");

    a.runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"after",
            Duration::from_secs(5),
        )
        .await
        .expect_err("peer is offline");
    let diagnostics = a.runtime.session_manager().diagnostics();

    assert_eq!(diagnostics.sessions.len(), 1);
    assert_eq!(diagnostics.sessions[0].reconnects, 1);
    assert_eq!(
        diagnostics.sessions[0].state,
        DeviceSessionState::NotConnected
    );

    a.runtime.shutdown().await.expect("shutdown A");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_shot_command_does_not_retry_an_offline_peer() {
    let network_secret = [53; 32];
    let a = build_runtime(53, network_secret, vec![peer_entry(54)], test_config()).await;
    let b = build_runtime(54, network_secret, vec![peer_entry(53)], test_config()).await;

    a.runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"before",
            Duration::from_secs(5),
        )
        .await
        .expect("initial command");
    let dial_attempts_before = a
        .runtime
        .session_manager()
        .diagnostics()
        .physical_dial_attempts;
    b.runtime.shutdown().await.expect("shutdown B");

    a.runtime
        .session_manager()
        .command_once_to(&b.device_id, b"best-effort", Duration::from_millis(500))
        .await
        .expect_err("offline peer");

    assert_eq!(
        a.runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        dial_attempts_before + 1
    );
    a.runtime.shutdown().await.expect("shutdown A");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhausted_request_budget_reports_dial_wait_without_attempting_connection() {
    let a = build_runtime(11, [10; 32], vec![peer_entry(12)], test_config()).await;
    let b = build_runtime(12, [10; 32], vec![peer_entry(11)], test_config()).await;
    let error = a
        .runtime
        .session_manager()
        .command(&b.device_id, b.addr.clone(), b"expired", Duration::ZERO)
        .await
        .expect_err("expired request");
    assert!(matches!(
        error,
        DeviceSessionError::Timeout {
            stage: "Device Session admission",
            budget_ms: 0
        }
    ));
    assert!(error.to_string().contains("remaining budget: 0 ms"));
    assert_eq!(
        a.runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        0
    );
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_dial_converges_on_the_deterministic_connection() {
    let network_secret = [7; 32];
    let a = build_runtime(5, network_secret, vec![peer_entry(6)], test_config()).await;
    let b = build_runtime(6, network_secret, vec![peer_entry(5)], test_config()).await;

    let (from_a, from_b) = tokio::join!(
        a.runtime.session_manager().command(
            &b.device_id,
            b.addr.clone(),
            b"from-a",
            Duration::from_secs(5)
        ),
        b.runtime.session_manager().command(
            &a.device_id,
            a.addr.clone(),
            b"from-b",
            Duration::from_secs(5)
        )
    );
    assert_eq!(from_a.expect("A command"), b"from-a");
    assert_eq!(from_b.expect("B command"), b"from-b");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let a_diagnostics = a.runtime.session_manager().diagnostics();
    let b_diagnostics = b.runtime.session_manager().diagnostics();
    assert_eq!(a_diagnostics.sessions.len(), 1);
    assert_eq!(b_diagnostics.sessions.len(), 1);
    let a_is_lower = a.runtime.endpoint().id().as_bytes() < b.runtime.endpoint().id().as_bytes();
    assert_eq!(
        a_diagnostics.sessions[0].origin,
        Some(if a_is_lower {
            DeviceSessionOrigin::Outgoing
        } else {
            DeviceSessionOrigin::Incoming
        })
    );
    assert_eq!(
        b_diagnostics.sessions[0].origin,
        Some(if a_is_lower {
            DeviceSessionOrigin::Incoming
        } else {
            DeviceSessionOrigin::Outgoing
        })
    );

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incompatible_version_is_explicit_and_does_not_fallback() {
    assert_eq!(DEVICE_SESSION_PROTOCOL_VERSION, 18);
    let network_secret = [6; 32];
    let a = build_runtime(7, network_secret, vec![peer_entry(8)], test_config()).await;
    let mut incompatible = test_config();
    incompatible.protocol_version = DEVICE_SESSION_PROTOCOL_VERSION - 1;
    let b = build_runtime(8, network_secret, vec![peer_entry(7)], incompatible).await;

    let error = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"read",
            Duration::from_secs(5),
        )
        .await
        .expect_err("version mismatch");

    let DeviceSessionError::VersionIncompatible {
        local_version,
        remote_version,
    } = error
    else {
        panic!("unexpected error: {error:?}");
    };
    assert_eq!(local_version, DEVICE_SESSION_PROTOCOL_VERSION);
    assert_eq!(remote_version, DEVICE_SESSION_PROTOCOL_VERSION - 1);
    let diagnostics = a.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(
        diagnostics.sessions[0].state,
        DeviceSessionState::NotConnected
    );
    let second = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"read-again",
            Duration::from_secs(5),
        )
        .await
        .expect_err("stable version mismatch");
    assert!(matches!(
        second,
        DeviceSessionError::VersionIncompatible { .. }
    ));
    assert_eq!(
        a.runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        1,
        "stable incompatibility must not repeatedly dial"
    );
    assert_eq!(
        b.runtime.session_manager().diagnostics().sessions[0].state,
        DeviceSessionState::NotConnected
    );

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn different_bound_network_membership_is_rejected() {
    let a = build_runtime(9, [5; 32], vec![peer_entry(10)], test_config()).await;
    let b = build_runtime(10, [4; 32], vec![peer_entry(9)], test_config()).await;

    let error = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"read",
            Duration::from_secs(5),
        )
        .await
        .expect_err("membership mismatch");
    assert!(
        matches!(&error, DeviceSessionError::MembershipRejected),
        "unexpected error: {error:?}"
    );

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

/// Stage reported when a request is queued behind another request's in-flight
/// physical dial for the same peer.
const DIAL_LOCK_STAGE: &str = "Device Session admission";

/// Point a paired peer's real `EndpointId` at a UDP socket that is bound but
/// never read and never answered. Pairing validation only ties the Device id to
/// the peer public key, so the dial passes validation and then stays inside
/// `Endpoint::connect` until `connect_timeout` expires, holding the per-peer
/// dial lock for the whole window. The returned socket must be kept alive.
fn stalling_peer_addr(peer: &iroh::EndpointAddr) -> (iroh::EndpointAddr, std::net::UdpSocket) {
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind stalled peer socket");
    let port = socket.local_addr().expect("stalled socket addr").port();
    let mut addr = iroh::EndpointAddr::new(peer.id);
    addr.addrs.insert(iroh::TransportAddr::Ip(
        (std::net::Ipv4Addr::LOCALHOST, port).into(),
    ));
    (addr, socket)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn incoming_session_completes_original_waiter_without_waiting_for_outgoing_dial() {
    let a = build_runtime(61, [61; 32], vec![peer_entry(62)], stall_config()).await;
    let b = build_runtime(62, [61; 32], vec![peer_entry(61)], test_config()).await;
    let (addr, _socket) = stalling_peer_addr(&b.addr);
    let manager = a.runtime.session_manager().clone();
    let peer = b.device_id.clone();
    let outgoing = tokio::spawn(async move {
        manager
            .command(&peer, addr, b"old-dial", Duration::from_millis(800))
            .await
    });
    wait_for_physical_dials(a.runtime.session_manager(), 1).await;
    b.runtime
        .session_manager()
        .command(
            &a.device_id,
            a.addr.clone(),
            b"new-session",
            Duration::from_secs(5),
        )
        .await
        .expect("incoming session established");
    let generation = a.runtime.session_manager().diagnostics().sessions[0].generation;
    assert_eq!(
        join_bounded(outgoing, Duration::from_millis(250), "original waiter")
            .await
            .expect("incoming session admits original waiter"),
        b"old-dial"
    );
    let diagnostics = a.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.sessions[0].state, DeviceSessionState::Connected);
    assert_eq!(diagnostics.sessions[0].generation, generation);
    assert!(diagnostics.sessions[0].last_error.is_none());
    a.runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"still-usable",
            Duration::from_secs(5),
        )
        .await
        .expect("reuse healthy session");
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn incoming_session_wakes_requests_queued_behind_a_stalled_dial() {
    let a = build_runtime(61, [61; 32], vec![peer_entry(62)], stall_config()).await;
    let b = build_runtime(62, [61; 32], vec![peer_entry(61)], test_config()).await;
    let (addr, _socket) = stalling_peer_addr(&b.addr);
    let manager = a.runtime.session_manager().clone();
    let owner = tokio::spawn({
        let manager = manager.clone();
        let peer = b.device_id.clone();
        async move {
            manager
                .command(&peer, addr, b"stalled-owner", Duration::from_secs(10))
                .await
        }
    });
    wait_for_physical_dials(&manager, 1).await;

    let mut queued = Box::pin(manager.command(
        &b.device_id,
        b.addr.clone(),
        b"queued-command",
        Duration::from_secs(2),
    ));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(queued.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;

    b.runtime
        .session_manager()
        .command(
            &a.device_id,
            a.addr.clone(),
            b"incoming-session",
            Duration::from_secs(2),
        )
        .await
        .expect("reverse dial establishes an authenticated session");
    let response = tokio::time::timeout(Duration::from_millis(250), queued).await;
    assert_eq!(
        join_bounded(owner, Duration::from_millis(250), "original waiter")
            .await
            .expect("original waiter also wakes"),
        b"stalled-owner"
    );
    let diagnostics = manager.diagnostics();
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");

    assert_eq!(
        response
            .expect("queued request must wake without waiting for the old dial")
            .expect("queued request must reuse the incoming session"),
        b"queued-command"
    );
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions[0].state, DeviceSessionState::Connected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn caller_deadline_reports_admission_expiry_without_extending_budget() {
    let a = build_runtime(63, [63; 32], vec![peer_entry(64)], test_config()).await;
    let b = build_runtime(64, [63; 32], vec![peer_entry(63)], test_config()).await;
    let (addr, _socket) = stalling_peer_addr(&b.addr);
    let error = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            addr,
            b"prepare-media",
            Duration::from_millis(100),
        )
        .await
        .expect_err("connection timeout");
    assert!(
        matches!(error, DeviceSessionError::Timeout { stage: "Device Session admission", budget_ms } if budget_ms > 0),
        "{error:?}"
    );
    assert_eq!(
        a.runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        1
    );
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

/// Wait for the manager to report `expected` physical dial attempts. The
/// condition is diagnostics-driven; the poll interval only bounds the cadence.
async fn wait_for_physical_dials(manager: &DeviceSessionManager, expected: u64) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if manager.diagnostics().physical_dial_attempts >= expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("manager never reported {expected} physical dial attempt(s)"));
}

fn is_dial_lock_wait(error: &DeviceSessionError) -> bool {
    matches!(
        error,
        DeviceSessionError::Timeout { stage, .. } if *stage == DIAL_LOCK_STAGE
    )
}

/// Config for tests that deliberately park the per-peer dial lock inside
/// `Endpoint::connect`: no retries so dial counts are attributable, and a
/// connect budget wide enough that the observation windows below sit well
/// inside the stall instead of racing its expiry.
fn stall_config() -> DeviceSessionConfig {
    DeviceSessionConfig {
        connect_timeout: Duration::from_secs(4),
        handshake_timeout: Duration::from_secs(4),
        reconnect_attempts: 0,
        ..test_config()
    }
}

/// Bounded join so a hung task can never stall the test past its budget.
async fn join_bounded<T>(task: tokio::task::JoinHandle<T>, budget: Duration, label: &str) -> T {
    tokio::time::timeout(budget, task)
        .await
        .unwrap_or_else(|_| panic!("{label} did not finish within {budget:?}"))
        .unwrap_or_else(|error| panic!("{label} task failed: {error}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_cold_commands_share_one_physical_dial_attempt() {
    let network_secret = [55; 32];
    let b = build_runtime(56, network_secret, vec![peer_entry(55)], test_config()).await;
    let a = build_runtime(55, network_secret, vec![peer_entry(56)], test_config()).await;
    let manager = a.runtime.session_manager().clone();
    let barrier = Arc::new(Barrier::new(9));

    let mut tasks = Vec::new();
    for index in 0..8u8 {
        let manager = manager.clone();
        let barrier = Arc::clone(&barrier);
        let peer_device_id = b.device_id.clone();
        let peer_addr = b.addr.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            let payload = vec![index; 32];
            let response = manager
                .command(
                    &peer_device_id,
                    peer_addr,
                    &payload,
                    Duration::from_secs(10),
                )
                .await;
            (payload, response)
        }));
    }
    barrier.wait().await;

    for task in tasks {
        let (payload, response) = join_bounded(task, Duration::from_secs(15), "cold Command").await;
        assert_eq!(
            response.expect("cold Command on a shared first dial"),
            payload,
            "each concurrent cold Command must receive its own response"
        );
    }

    let diagnostics = manager.diagnostics();
    assert_eq!(
        diagnostics.physical_dial_attempts, 1,
        "8 concurrent cold Commands must share exactly one physical dial attempt"
    );
    assert_eq!(b.handler.calls.load(Ordering::SeqCst), 8);
    assert_eq!(diagnostics.sessions.len(), 1);
    assert_eq!(diagnostics.sessions[0].state, DeviceSessionState::Connected);
    assert!(diagnostics.sessions[0].generation.is_some());

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_dial_lock_waiter_leaves_dial_owner_and_peer_recovery_intact() {
    let network_secret = [57; 32];
    let b = build_runtime(58, network_secret, vec![peer_entry(57)], test_config()).await;
    let a = build_runtime(57, network_secret, vec![peer_entry(58)], stall_config()).await;
    let manager = a.runtime.session_manager().clone();
    let (stalling_addr, _stalled_socket) = stalling_peer_addr(&b.addr);

    let owner = tokio::spawn({
        let manager = manager.clone();
        let peer_device_id = b.device_id.clone();
        let peer_addr = stalling_addr.clone();
        async move {
            manager
                .command(
                    &peer_device_id,
                    peer_addr,
                    b"dial-owner",
                    Duration::from_secs(10),
                )
                .await
        }
    });
    wait_for_physical_dials(&manager, 1).await;

    // Cancellation by dropping the request future when its own budget expires.
    let dropped_waiter = tokio::time::timeout(
        Duration::from_millis(250),
        manager.command(
            &b.device_id,
            stalling_addr.clone(),
            b"cancelled-by-budget",
            Duration::from_secs(4),
        ),
    )
    .await;
    assert!(
        dropped_waiter.is_err(),
        "a queued request must not complete while another request owns the dial"
    );
    assert_eq!(
        manager.diagnostics().physical_dial_attempts,
        1,
        "a request cancelled while waiting for the dial lock must not open its own dial"
    );

    // Cancellation by aborting the owning task while it is queued on the lock.
    let mut aborted_waiter = tokio::spawn({
        let manager = manager.clone();
        let peer_device_id = b.device_id.clone();
        let peer_addr = stalling_addr.clone();
        async move {
            manager
                .command(
                    &peer_device_id,
                    peer_addr,
                    b"cancelled-by-abort",
                    Duration::from_secs(4),
                )
                .await
        }
    });
    let completed_early =
        tokio::time::timeout(Duration::from_millis(100), &mut aborted_waiter).await;
    assert!(
        completed_early.is_err(),
        "the second waiter must still be queued behind the owner's dial"
    );
    aborted_waiter.abort();
    assert!(aborted_waiter
        .await
        .expect_err("aborted waiter")
        .is_cancelled());
    assert_eq!(
        manager.diagnostics().physical_dial_attempts,
        1,
        "aborting a queued waiter must not open a second dial"
    );

    let owner_error = join_bounded(owner, Duration::from_secs(15), "dial owner")
        .await
        .expect_err(
            "the dial owner must not be cancelled by its waiters and must fail on its own budget",
        );
    assert!(
        matches!(
            &owner_error,
            DeviceSessionError::Timeout {
                stage: "QUIC connection establishment",
                ..
            } | DeviceSessionError::Connect(_)
        ),
        "the dial owner must keep its own outcome, got {owner_error:?}"
    );

    let response = manager
        .command(
            &b.device_id,
            b.addr.clone(),
            b"after-cancellation",
            Duration::from_secs(5),
        )
        .await
        .expect("peer recovers after cancelled waiters");
    assert_eq!(response, b"after-cancellation");
    let diagnostics = manager.diagnostics();
    assert_eq!(
        diagnostics.physical_dial_attempts, 2,
        "cancelled waiters must not add dials: owner dial + one recovery dial"
    );
    assert_eq!(diagnostics.sessions.len(), 1);
    assert_eq!(diagnostics.sessions[0].state, DeviceSessionState::Connected);
    assert!(diagnostics.sessions[0].generation.is_some());

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_first_waiter_preserves_shared_attempt_for_remaining_waiters() {
    let network_secret = [59; 32];
    let b = build_runtime(60, network_secret, vec![peer_entry(59)], test_config()).await;
    let a = build_runtime(59, network_secret, vec![peer_entry(60)], stall_config()).await;
    let manager = a.runtime.session_manager().clone();
    let (stalling_addr, _stalled_socket) = stalling_peer_addr(&b.addr);

    let owner = tokio::spawn({
        let manager = manager.clone();
        let peer_device_id = b.device_id.clone();
        let peer_addr = stalling_addr.clone();
        async move {
            manager
                .command(
                    &peer_device_id,
                    peer_addr,
                    b"dial-owner",
                    Duration::from_secs(10),
                )
                .await
        }
    });
    wait_for_physical_dials(&manager, 1).await;

    // Three queued requests with a short budget prove they are queued behind the
    // owner's dial instead of opening dials of their own.
    let (queued_0, queued_1, queued_2) = tokio::join!(
        manager.command(
            &b.device_id,
            b.addr.clone(),
            b"queued-0",
            Duration::from_millis(250)
        ),
        manager.command(
            &b.device_id,
            b.addr.clone(),
            b"queued-1",
            Duration::from_millis(250)
        ),
        manager.command(
            &b.device_id,
            b.addr.clone(),
            b"queued-2",
            Duration::from_millis(250)
        )
    );
    for queued in [queued_0, queued_1, queued_2] {
        let error = queued.expect_err("queued requests must not bypass the dial lock");
        assert!(
            is_dial_lock_wait(&error),
            "queued request must report {DIAL_LOCK_STAGE}, got {error:?}"
        );
    }
    assert_eq!(
        manager.diagnostics().physical_dial_attempts,
        1,
        "queued requests must not dial while another request owns the dial"
    );

    let dials_before_cancellation = manager.diagnostics().physical_dial_attempts;
    let mut waiting = Box::pin(manager.command(
        &b.device_id,
        b.addr.clone(),
        b"already-waiting",
        Duration::from_secs(5),
    ));
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(waiting.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    owner.abort();
    assert!(owner
        .await
        .expect_err("cancelled dial owner")
        .is_cancelled());
    b.runtime
        .session_manager()
        .command(
            &a.device_id,
            a.addr.clone(),
            b"reverse",
            Duration::from_secs(2),
        )
        .await
        .expect("incoming session admits remaining waiter");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("queued request completion")
            .expect("queued request recovers"),
        b"already-waiting"
    );

    let mut recovery = Vec::new();
    for index in 0..3u8 {
        let manager = manager.clone();
        let peer_device_id = b.device_id.clone();
        let peer_addr = b.addr.clone();
        recovery.push(tokio::spawn(async move {
            let payload = vec![index; 16];
            let response = manager
                .command(&peer_device_id, peer_addr, &payload, Duration::from_secs(5))
                .await;
            (payload, response)
        }));
    }
    for task in recovery {
        let (payload, response) =
            join_bounded(task, Duration::from_secs(15), "recovery Command").await;
        assert_eq!(
            response.expect("requests recover after the dial owner is cancelled"),
            payload
        );
    }

    let diagnostics = manager.diagnostics();
    assert_eq!(
        diagnostics.physical_dial_attempts, dials_before_cancellation,
        "cancelling the first waiter must not replace the shared dial"
    );
    assert_eq!(b.handler.calls.load(Ordering::SeqCst), 4);
    assert_eq!(diagnostics.sessions.len(), 1);
    assert_eq!(diagnostics.sessions[0].state, DeviceSessionState::Connected);
    assert!(diagnostics.sessions[0].generation.is_some());

    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn short_waiter_expires_while_other_streams_complete_one_shared_cold_dial() {
    let b = build_runtime(92, [91; 32], vec![peer_entry(91)], test_config()).await;
    b.runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(EchoLiveActivityHandler::default()));
    b.runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(EchoBulkTransferHandler::default()));
    let remote = b
        .addr
        .addrs
        .iter()
        .find_map(|addr| match addr {
            iroh::TransportAddr::Ip(addr) if addr.is_ipv4() => Some(*addr),
            _ => None,
        })
        .expect("IPv4 endpoint");
    let proxy = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("proxy");
    let a = build_runtime(
        91,
        [91; 32],
        vec![peer_entry_with_direct_addrs(
            92,
            vec![proxy.local_addr().unwrap().to_string()],
        )],
        test_config(),
    )
    .await;
    // Hold the real QUIC Initial longer than each logical stream's budget.
    // All stream classes must await the same bounded cold establishment.
    let forward = async {
        let mut buf = [0; 65536];
        let mut client = None;
        loop {
            let (len, source) = proxy.recv_from(&mut buf).await.expect("receive datagram");
            let target = if source == remote {
                client.expect("client sent Initial")
            } else {
                if client.is_none() {
                    client = Some(source);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                remote
            };
            proxy
                .send_to(&buf[..len], target)
                .await
                .expect("forward datagram");
        }
    };
    let manager = a.runtime.session_manager();
    let operation_timeout = Duration::from_millis(250);
    let requests = async {
        let (command, once, live, bulk) = tokio::join!(
            manager.command_to(&b.device_id, b"command", operation_timeout),
            manager.command_once_to(&b.device_id, b"once", Duration::from_secs(2)),
            manager.open_live_activity_to(&b.device_id, b"live", Duration::from_secs(2)),
            manager.open_bulk_transfer_to(&b.device_id, Duration::from_secs(2)),
        );
        assert!(matches!(
            command,
            Err(DeviceSessionError::Timeout {
                stage: "Device Session admission",
                ..
            })
        ));
        assert_eq!(
            b.handler.calls.load(Ordering::SeqCst),
            1,
            "expired command must never be delivered later"
        );
        assert_eq!(once.expect("cold non-replayed command"), b"once");
        let mut live = live.expect("cold live subscription");
        assert_eq!(
            live.next_frame().await.expect("live frame"),
            Some(b"live".to_vec())
        );
        let mut bulk = bulk.expect("cold bulk stream");
        bulk.send_mut().write_u32(4).await.expect("bulk length");
        bulk.send_mut()
            .write_all(b"bulk")
            .await
            .expect("bulk payload");
        assert_eq!(bulk.recv_mut().read_u32().await.expect("echo length"), 4);
        let mut bytes = [0; 4];
        bulk.recv_mut()
            .read_exact(&mut bytes)
            .await
            .expect("echo payload");
        assert_eq!(&bytes, b"bulk");
        assert_eq!(manager.diagnostics().physical_dial_attempts, 1);
        // Once connected, an unresponsive command still observes its own budget.
        b.handler.hang_next.store(true, Ordering::SeqCst);
        let started = Instant::now();
        let error = manager
            .command_to(&b.device_id, b"hung", operation_timeout)
            .await
            .expect_err("command deadline");
        assert!(matches!(
            error,
            DeviceSessionError::Timeout {
                stage: "command exchange",
                ..
            }
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        b.handler.hang_release.notify_waiters();
    };
    tokio::time::timeout(Duration::from_secs(6), async {
        tokio::select! {
            () = requests => {},
            () = forward => unreachable!(),
        }
    })
    .await
    .expect("bounded cold-start test");
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_session_admission_observes_total_budget_and_zero_never_dials() {
    let blackhole = std::net::UdpSocket::bind("127.0.0.1:0").expect("blackhole");
    let config = DeviceSessionConfig {
        connect_timeout: Duration::from_millis(150),
        handshake_timeout: Duration::from_millis(150),
        ..test_config()
    };
    let a = build_runtime(
        93,
        [93; 32],
        vec![peer_entry_with_direct_addrs(
            94,
            vec![blackhole.local_addr().unwrap().to_string()],
        )],
        config,
    )
    .await;
    let peer = hex::encode(peer_entry(94).device_id);
    let manager = a.runtime.session_manager();
    let zero = manager
        .command_to(&peer, b"zero", Duration::ZERO)
        .await
        .expect_err("zero budget");
    assert!(matches!(zero, DeviceSessionError::Timeout { .. }));
    assert_eq!(manager.diagnostics().physical_dial_attempts, 0);
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        manager.command_to(&peer, b"cold", Duration::from_millis(10)),
    )
    .await
    .expect("bounded preparation")
    .expect_err("unreachable peer");
    assert!(matches!(
        error,
        DeviceSessionError::Timeout {
            stage: "Device Session admission",
            ..
        }
    ));
    assert_eq!(manager.diagnostics().physical_dial_attempts, 1);
    assert_eq!(
        manager.connectivity_state(&peer).unwrap(),
        DeviceSessionState::NotConnected
    );
    a.runtime.shutdown().await.expect("shutdown A");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpaired_sender_knowing_endpoint_and_direct_address_cannot_run_commands() {
    // A knows B's complete public identity, route, and even the network secret.
    // B must still reject A because B has not paired with this public key.
    let a = build_runtime(81, [81; 32], vec![peer_entry(82)], test_config()).await;
    let b = build_runtime(82, [81; 32], vec![], test_config()).await;
    let result = a
        .runtime
        .session_manager()
        .command(
            &b.device_id,
            b.addr.clone(),
            b"private operation",
            Duration::from_secs(2),
        )
        .await;
    assert!(result.is_err(), "knowing the address must not grant access");
    assert!(
        b.runtime
            .session_manager()
            .diagnostics()
            .accepted_connections
            > 0,
        "exercise the remote admission check, not a local address rejection"
    );
    assert_eq!(b.handler.calls.load(Ordering::SeqCst), 0);
    assert!(b
        .runtime
        .session_manager()
        .diagnostics()
        .sessions
        .iter()
        .all(|session| session.generation.is_none()));
    a.runtime.shutdown().await.expect("shutdown A");
    b.runtime.shutdown().await.expect("shutdown B");
}

#[cfg(feature = "test-util")]
#[path = "health_probe.rs"]
mod health_probe;
