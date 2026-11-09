use std::time::Duration;

use futures::{stream::BoxStream, StreamExt};
use iroh::{
    address_lookup::{AddressLookup, Error, Item},
    endpoint::{presets, QuicTransportConfig},
    Endpoint, EndpointAddr, EndpointId,
};

#[derive(Debug)]
struct DelayedCurrentAddress {
    stale: EndpointAddr,
    current: EndpointAddr,
}

impl AddressLookup for DelayedCurrentAddress {
    fn resolve(&self, id: EndpointId) -> Option<BoxStream<'static, Result<Item, Error>>> {
        if id != self.current.id {
            return None;
        }
        let stale = Item::new(self.stale.clone().into(), "stale_dns", None);
        let current = Item::new(self.current.clone().into(), "current_https", None);
        Some(Box::pin(
            futures::stream::once(async move { Ok(stale) }).chain(futures::stream::once(
                async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok(current)
                },
            )),
        ))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn newly_discovered_peer_path_does_not_wait_for_stale_path_retransmit() {
    assert_discovery_handoff(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn current_peer_path_connects_within_the_handoff_deadline() {
    assert_discovery_handoff(false).await;
}

async fn assert_discovery_handoff(stale_first: bool) {
    const ALPN: &[u8] = b"rua-discovery-handoff-test/1";
    let server = Endpoint::builder(presets::Minimal)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .unwrap()
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    // Keep the obsolete UDP address open but silent, just as a stale relay can
    // accept a TCP connection while no longer routing to the restarted peer.
    let stale = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let current = EndpointAddr::new(server.id()).with_ip_addr(server.bound_sockets()[0]);
    let client = Endpoint::builder(presets::Minimal)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .unwrap()
        .transport_config(
            QuicTransportConfig::builder()
                .initial_rtt(Duration::from_secs(2))
                .build(),
        )
        .address_lookup(DelayedCurrentAddress {
            stale: if stale_first {
                EndpointAddr::new(server.id()).with_ip_addr(stale.local_addr().unwrap())
            } else {
                current.clone()
            },
            current,
        })
        .bind()
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(client.connect(server.id(), ALPN), async {
            server.accept().await.unwrap().await
        })
    })
    .await;
    let _ = tokio::time::timeout(Duration::from_millis(100), async {
        tokio::join!(client.close(), server.close());
    })
    .await;
    let (dialed, accepted) = outcome.expect("fresh discovery must immediately reach the peer, without waiting for the stale path's QUIC backoff");
    dialed.expect("dial new path");
    accepted.expect("accept new path");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restarted_peer_can_use_new_path_while_old_connection_is_still_open() {
    assert_restarted_peer(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restarted_peer_is_rediscovered_while_old_connection_is_still_open() {
    assert_restarted_peer(false).await;
}

async fn assert_restarted_peer(explicit_address: bool) {
    const ALPN: &[u8] = b"rua-restarted-path-test/1";
    let key = iroh::SecretKey::generate();
    let old_key = key.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel();
    // Drop an isolated runtime without Endpoint::close, like Android killing
    // the resident process before it can notify its peer.
    let old_host = std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let endpoint = Endpoint::builder(presets::Minimal)
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")
                .unwrap()
                .secret_key(old_key)
                .alpns(vec![ALPN.to_vec()])
                .bind()
                .await
                .unwrap();
            ready_tx.send(endpoint.bound_sockets()[0]).unwrap();
            let connection = endpoint.accept().await.unwrap().await.unwrap();
            let _ = kill_rx.await;
            // Avoid graceful Drop sending CONNECTION_CLOSE before the runtime
            // is stopped. Dropping these handles after shutdown cannot send it.
            (endpoint, connection)
        })
    });
    let old_addr = ready_rx.await.unwrap();
    let lookup = iroh::address_lookup::memory::MemoryLookup::new();
    let client = Endpoint::builder(presets::Minimal)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .unwrap()
        .address_lookup(lookup.clone())
        .bind()
        .await
        .unwrap();
    let old_connection = client
        .connect(EndpointAddr::new(key.public()).with_ip_addr(old_addr), ALPN)
        .await
        .unwrap();
    kill_tx.send(()).unwrap();
    let abandoned = tokio::task::spawn_blocking(move || old_host.join().unwrap())
        .await
        .unwrap();
    assert!(old_connection.close_reason().is_none());
    let server = Endpoint::builder(presets::Minimal)
        .clear_ip_transports()
        .bind_addr("127.0.0.1:0")
        .unwrap()
        .secret_key(key)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    assert_ne!(server.bound_sockets()[0], old_addr);
    let current_addr = EndpointAddr::new(server.id()).with_ip_addr(server.bound_sockets()[0]);
    lookup.add_endpoint_info(current_addr.clone());
    let dial_addr = if explicit_address {
        current_addr
    } else {
        EndpointAddr::new(server.id())
    };
    let result = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(client.connect(dial_addr, ALPN), async {
            server.accept().await.unwrap().await
        })
    })
    .await;
    drop(abandoned);
    let _ = tokio::time::timeout(Duration::from_millis(100), async {
        tokio::join!(client.close(), server.close());
    })
    .await;
    let (connected, accepted) =
        result.expect("a previous connection must not pin a new handshake to a dead path");
    connected.unwrap();
    accepted.unwrap();
}
