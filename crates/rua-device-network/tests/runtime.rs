use rua_device_network::{
    device_list::{DeviceEntry, DeviceList},
    BoundDeviceNetworkMembership, DeviceNetworkBootstrap, DeviceNetworkIdentity,
    DeviceNetworkPreset,
};

#[tokio::test]
async fn local_runtime_owns_endpoint_router_gossip_and_diagnostics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let endpoint_secret = iroh::SecretKey::from_bytes(&[7; 32]);
    let expected_device_id = endpoint_secret.public().as_bytes()[..8].to_vec();
    let identity = DeviceNetworkIdentity::new(endpoint_secret);
    let network_secret = [11; 32];
    let expected_network_id = *iroh_docs::NamespaceSecret::from_bytes(&network_secret)
        .id()
        .as_bytes();
    let membership = BoundDeviceNetworkMembership::new(network_secret);
    let bootstrap = DeviceNetworkBootstrap::bind(
        DeviceNetworkPreset::Local,
        identity,
        membership,
        rua_device_network::DevicePeerDirectory::from_sync_dir(dir.path()),
    )
    .await
    .expect("bind local Device network");

    let runtime = bootstrap.spawn(Vec::new());
    let endpoint = runtime.endpoint().clone();
    let diagnostics = runtime.diagnostics();
    assert_eq!(diagnostics.state, "active");
    assert_eq!(diagnostics.local_device_id, hex::encode(expected_device_id));
    assert_eq!(diagnostics.network_id, hex::encode(expected_network_id));
    assert!(!diagnostics.mdns_enabled);
    assert!(diagnostics.gossip_running);
    assert!(!diagnostics.bound_sockets.is_empty());

    runtime.shutdown().await.expect("shutdown runtime");
    assert!(endpoint.is_closed());
    assert_eq!(runtime.diagnostics().state, "stopped");
    runtime.shutdown().await.expect("idempotent shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gossip_wake_fans_out_without_opening_device_sessions() {
    let a_dir = tempfile::tempdir().expect("A tempdir");
    let b_dir = tempfile::tempdir().expect("B tempdir");
    let network_secret = [12; 32];
    let a_secret = iroh::SecretKey::from_bytes(&[21; 32]);
    let b_secret = iroh::SecretKey::from_bytes(&[22; 32]);
    let a_bootstrap = DeviceNetworkBootstrap::bind(
        DeviceNetworkPreset::Local,
        DeviceNetworkIdentity::new(a_secret.clone()),
        BoundDeviceNetworkMembership::new(network_secret),
        rua_device_network::DevicePeerDirectory::from_sync_dir(a_dir.path()),
    )
    .await
    .expect("bind A");
    let b_bootstrap = DeviceNetworkBootstrap::bind(
        DeviceNetworkPreset::Local,
        DeviceNetworkIdentity::new(b_secret.clone()),
        BoundDeviceNetworkMembership::new(network_secret),
        rua_device_network::DevicePeerDirectory::from_sync_dir(b_dir.path()),
    )
    .await
    .expect("bind B");

    write_peer_list(
        a_dir.path(),
        peer_entry(&b_secret, direct_addrs(b_bootstrap.endpoint())),
    );
    write_peer_list(
        b_dir.path(),
        peer_entry(&a_secret, direct_addrs(a_bootstrap.endpoint())),
    );

    let a = a_bootstrap.spawn(Vec::new());
    let b = b_bootstrap.spawn(Vec::new());
    let mut wakes = b.gossip_bus().subscribe();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if a.gossip_bus().neighbor_count() > 0 && b.gossip_bus().neighbor_count() > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Gossip neighbors should connect");
    a.gossip_bus()
        .publish(b"chat-live-wake".to_vec())
        .await
        .expect("publish wake");
    let received = tokio::time::timeout(std::time::Duration::from_secs(10), wakes.recv())
        .await
        .expect("wake timeout")
        .expect("wake channel");

    assert_eq!(received, b"chat-live-wake");
    assert_eq!(a.connectivity().len(), 1);
    assert_eq!(b.connectivity().len(), 1);
    assert_eq!(
        a.connectivity()[0].state,
        rua_device_network::DeviceConnectivityState::NotConnected
    );
    assert_eq!(
        b.connectivity()[0].state,
        rua_device_network::DeviceConnectivityState::NotConnected
    );
    assert!(a.diagnostics().gossip_neighbors > 0);
    assert!(b.diagnostics().gossip_neighbors > 0);
    assert_eq!(a.session_manager().diagnostics().physical_dial_attempts, 0);
    assert_eq!(b.session_manager().diagnostics().physical_dial_attempts, 0);

    a.shutdown().await.expect("shutdown A");
    b.shutdown().await.expect("shutdown B");
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

fn peer_entry(secret: &iroh::SecretKey, direct_addrs: Vec<String>) -> DeviceEntry {
    let public_key = *secret.public().as_bytes();
    let mut device_id = [0; 8];
    device_id.copy_from_slice(&public_key[..8]);
    DeviceEntry {
        device_id,
        public_key,
        label: hex::encode(device_id),
        friendly_name: "Test Device".to_string(),
        added_at_ms: 1,
        last_seen_ms: 1,
        direct_addrs,
    }
}

fn direct_addrs(endpoint: &iroh::Endpoint) -> Vec<String> {
    endpoint
        .bound_sockets()
        .into_iter()
        .map(|socket| match socket {
            std::net::SocketAddr::V4(socket) if socket.ip().is_unspecified() => {
                std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, socket.port()))
            }
            std::net::SocketAddr::V6(socket) if socket.ip().is_unspecified() => {
                std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, socket.port()))
            }
            socket => socket,
        })
        .map(|socket| socket.to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_discovery_attempts_https_independently_of_dns_failure() {
    use futures::StreamExt;
    use iroh::dns::{DnsProtocol, DnsResolver};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    // Fail every DNS query locally. No public discovery or relay service is
    // required: the HTTP resolver must independently attempt its hostname even
    // after the peer's TXT lookup has failed.
    let dns = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dns_addr = dns.local_addr().unwrap();
    let cancel = CancellationToken::new();
    let _cancel_on_drop = cancel.clone().drop_guard();
    let responder = tokio::spawn(async move {
        let mut packet = [0u8; 2048];
        loop {
            let received = tokio::select! {
                _ = cancel.cancelled() => break,
                received = dns.recv_from(&mut packet) => received.unwrap(),
            };
            let (len, from) = received;
            assert!(len >= 12);
            let mut end = 12;
            while packet[end] != 0 {
                end += usize::from(packet[end]) + 1;
            }
            end += 5; // root label and QTYPE/QCLASS
            packet[2] = 0x81;
            packet[3] = 0x83; // response, recursion available, NXDOMAIN
            packet[6..12].fill(0);
            dns.send_to(&packet[..end], from).await.unwrap();
        }
    });
    let resolver = DnsResolver::builder()
        .with_nameservers([(dns_addr, DnsProtocol::Udp)])
        .build();
    let endpoint = rua_device_network::n0_endpoint_builder()
        .dns_resolver(resolver)
        .relay_mode(iroh::RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    let mut results = endpoint
        .address_lookup()
        .unwrap()
        .resolve(iroh::SecretKey::from_bytes(&[93; 32]).public());
    let mut dns_failed = false;
    let mut https_attempted = false;
    // Iroh staggers DNS retries up to 3 seconds plus jitter.
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(result) = results.next().await {
            if let Ok(Err(error)) = result {
                let detail = error.to_string();
                dns_failed |= detail.contains("'dns'");
                https_attempted |= detail.contains("'pkarr'");
                if dns_failed && https_attempted {
                    return;
                }
            }
        }
    })
    .await;
    drop(results);
    endpoint.close().await;
    drop(_cancel_on_drop);
    responder.await.unwrap();
    assert!(observed.is_ok() && dns_failed && https_attempted,
        "both DNS and HTTPS discovery must run independently: dns={dns_failed}, https={https_attempted}");
}

#[test]
fn published_addresses_include_public_ipv6_without_private_interface_addresses() {
    use iroh::TransportAddr;
    let relay = TransportAddr::Relay("https://relay.example./".parse().unwrap());
    let public = TransportAddr::Ip("[2606:4700:4700::1111]:11204".parse().unwrap());
    let excluded = [
        "127.0.0.1:11204",
        "192.168.1.2:11204",
        "8.8.8.8:11204",
        "[::]:11204",
        "[::1]:11204",
        "[fe80::1]:11204",
        "[fd00::1]:11204",
        "[ff0e::1]:11204",
        "[2001:db8::1]:11204",
        "[3fff::1]:11204",
    ];
    let mut candidates = vec![relay.clone(), public.clone()];
    candidates.extend(
        excluded
            .into_iter()
            .map(|addr| TransportAddr::Ip(addr.parse().unwrap())),
    );
    assert_eq!(
        rua_device_network::published_device_address_filter()
            .apply(&candidates)
            .as_ref(),
        &vec![relay, public],
        "public discovery must retain the relay fallback and public IPv6 only"
    );
}
