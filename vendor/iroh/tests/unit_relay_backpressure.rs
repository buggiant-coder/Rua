//! Regression for upstream iroh #4511: a full relay must not block peer control.
use std::{collections::BTreeSet, num::NonZeroUsize, pin::Pin, time::Duration};

use iroh_base::{EndpointAddr, EndpointId, SecretKey};
use n0_watcher::Watchable;
use tokio::sync::oneshot;

use super::*;
use crate::{
    address_lookup::{AddressLookup, AddressLookupServices, Error, Item},
    metrics::EndpointMetrics,
    socket::{
        remote_map::{RemoteMap, RemoteStateMessage},
        transports::{IpConfig, IpTransports, OwnedTransmit, TransportsSender},
    },
};

struct Harness {
    _remotes: RemoteMap,
    _direct_addrs: Watchable<BTreeSet<crate::socket::DirectAddr>>,
    _cancel_guard: tokio_util::sync::DropGuard,
    cancel: CancellationToken,
    actor: mpsc::Sender<RemoteStateMessage>,
    relay_rx: mpsc::Receiver<RelaySendItem>,
}

async fn congested_peer(count: usize, direct: Option<std::net::SocketAddr>) -> Harness {
    let metrics = EndpointMetrics::default();
    let direct_addrs = Watchable::new(BTreeSet::new());
    let cancel = CancellationToken::new();
    let guard = cancel.clone().drop_guard();
    let peer = SecretKey::from_bytes(&[1; 32]).public();
    let lookup = AddressLookupServices::default();
    if let Some(addr) = direct {
        lookup.add(DelayedAddress(EndpointAddr::new(peer).with_ip_addr(addr)));
    }
    let mut remotes = RemoteMap::new(
        metrics.socket.clone(),
        direct_addrs.watch(),
        lookup,
        cancel.clone(),
        Default::default(),
        tracing::Span::none(),
    );
    let relay: RelayUrl = "https://relay.example.invalid".parse().unwrap();
    let (resolve_tx, resolve_rx) = oneshot::channel();
    remotes
        .resolve_remote(
            EndpointAddr::new(peer).with_relay_url(relay.clone()),
            resolve_tx,
        )
        .await
        .unwrap();
    resolve_rx.await.unwrap().unwrap();

    // Use the real bounded relay sender, holding its receiver to apply backpressure.
    let (tx, rx) = mpsc::channel(256);
    let mut sender = TransportsSender {
        ip: IpTransports::bind(
            [IpConfig::V4 {
                ip_net: "127.0.0.1/32".parse().unwrap(),
                port: 0,
                is_required: true,
                is_default: true,
            }]
            .into_iter(),
            &metrics,
        )
        .unwrap()
        .create_sender(),
        relay: vec![RelaySender {
            sender: PollSender::new(tx),
        }],
        custom: Vec::new(),
        max_transmit_segments: NonZeroUsize::new(1).unwrap(),
    };
    let addr = super::super::Addr::Relay(relay, peer);
    for _ in 0..256 {
        std::future::poll_fn(|cx| {
            Pin::new(&mut sender).poll_send(
                cx,
                &addr,
                None,
                &Transmit {
                    ecn: None,
                    contents: b"queued",
                    segment_size: None,
                },
            )
        })
        .await
        .unwrap();
    }
    let actor = remotes.remote_state_actor(peer);
    for _ in 0..count {
        actor
            .send(RemoteStateMessage::SendDatagram(
                Box::new(sender.clone()),
                OwnedTransmit {
                    ecn: None,
                    contents: Bytes::from_static(b"initial"),
                    segment_size: None,
                },
            ))
            .await
            .unwrap();
    }
    Harness {
        _remotes: remotes,
        _direct_addrs: direct_addrs,
        _cancel_guard: guard,
        cancel,
        actor,
        relay_rx: rx,
    }
}

#[derive(Debug)]
struct DelayedAddress(EndpointAddr);

impl AddressLookup for DelayedAddress {
    fn resolve(&self, _: EndpointId) -> Option<n0_future::boxed::BoxStream<Result<Item, Error>>> {
        let addr = self.0.clone();
        Some(Box::pin(futures_util::stream::once(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(Item::new(addr.into(), "delayed_direct", None))
        })))
    }
}

#[tokio::test]
async fn full_relay_queue_does_not_block_remote_info() {
    let mut h = congested_peer(1, None).await;
    let (info_tx, mut info_rx) = oneshot::channel();
    h.actor
        .send(RemoteStateMessage::RemoteInfo(info_tx))
        .await
        .unwrap();

    let responsive = tokio::time::timeout(Duration::from_secs(1), &mut info_rx).await;
    if responsive.is_err() {
        // Demonstrate causality: freeing one queue slot releases the blocked query.
        h.relay_rx.recv().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), info_rx)
            .await
            .expect("query did not recover after draining relay")
            .unwrap();
    }
    assert!(
        responsive.is_ok(),
        "full relay send queue blocked RemoteInfo; draining one slot released it"
    );
}

#[tokio::test]
async fn discovered_direct_path_sends_while_relay_stays_full() {
    let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let h = congested_peer(1, Some(receiver.local_addr().unwrap())).await;
    let mut buf = [0; 64];
    let len = tokio::time::timeout(Duration::from_secs(1), receiver.recv(&mut buf))
        .await
        .expect("new direct path blocked behind relay")
        .unwrap();
    assert_eq!(&buf[..len], b"initial");
    assert_eq!(h.relay_rx.len(), 256);
}

#[tokio::test]
async fn full_relay_queue_does_not_block_shutdown() {
    let mut h = congested_peer(1, None).await;
    let (tx, rx) = oneshot::channel();
    h.actor
        .send(RemoteStateMessage::RemoteInfo(tx))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .unwrap()
        .unwrap();
    h.cancel.cancel();
    tokio::time::timeout(Duration::from_secs(1), async {
        let mut count = 0;
        // Wait for actor termination before releasing queue capacity.
        h.actor.closed().await;
        while h.relay_rx.recv().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 256);
    })
    .await
    .expect("blocked sends retained by cancelled actor");
}

#[tokio::test(start_paused = true)]
async fn blocked_handshakes_expire_without_late_delivery() {
    let mut h = congested_peer(1, None).await;
    let (tx, rx) = oneshot::channel();
    h.actor
        .send(RemoteStateMessage::RemoteInfo(tx))
        .await
        .unwrap();
    rx.await.unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    for _ in 0..256 {
        h.relay_rx.recv().await.unwrap();
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(1), h.relay_rx.recv())
            .await
            .is_err()
    );
}

#[tokio::test(start_paused = true)]
async fn congested_handshakes_do_not_replay_after_capacity_returns() {
    let mut h = congested_peer(32, None).await;
    let (tx, rx) = oneshot::channel();
    h.actor
        .send(RemoteStateMessage::RemoteInfo(tx))
        .await
        .unwrap();
    rx.await.unwrap();
    for _ in 0..256 {
        h.relay_rx.recv().await.unwrap();
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(1), h.relay_rx.recv())
            .await
            .is_err()
    );
}
