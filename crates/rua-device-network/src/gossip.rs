use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::StreamExt;
use iroh::address_lookup::memory::MemoryLookup;
use iroh::EndpointId;
use iroh_gossip::api::Event;
use iroh_gossip::net::Gossip;
use iroh_gossip::proto::TopicId;
use sha2::{Digest, Sha256};

use crate::peer_directory::{endpoint_addr_from_entry, DevicePeerDirectory};

const GOSSIP_MESSAGE_LIMIT: usize = 16 * 1024;
const GOSSIP_QUEUE_CAPACITY: usize = 128;

#[derive(Debug, thiserror::Error)]
pub enum DeviceNetworkGossipError {
    #[error("Device Network Gossip payload exceeds {GOSSIP_MESSAGE_LIMIT} bytes")]
    PayloadTooLarge,
    #[error("Device Network Gossip is unavailable")]
    Unavailable,
}

#[derive(Clone)]
pub struct DeviceNetworkGossipBus {
    outgoing: tokio::sync::mpsc::Sender<Vec<u8>>,
    incoming: tokio::sync::broadcast::Sender<Vec<u8>>,
    shutdown: tokio::sync::watch::Sender<bool>,
    neighbors: Arc<AtomicUsize>,
}

impl std::fmt::Debug for DeviceNetworkGossipBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceNetworkGossipBus").finish()
    }
}

impl DeviceNetworkGossipBus {
    pub(crate) fn spawn(
        gossip: Gossip,
        network_id: [u8; 32],
        local_endpoint_id: EndpointId,
        peer_directory: DevicePeerDirectory,
        peer_lookup: MemoryLookup,
    ) -> Self {
        let (outgoing, outgoing_rx) = tokio::sync::mpsc::channel(GOSSIP_QUEUE_CAPACITY);
        let (incoming, _) = tokio::sync::broadcast::channel(GOSSIP_QUEUE_CAPACITY);
        let (shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
        let neighbors = Arc::new(AtomicUsize::new(0));
        tokio::spawn(
            GossipBusActor {
                gossip,
                topic_id: TopicId::from_bytes(gossip_topic_id(network_id)),
                local_endpoint_id,
                peer_directory,
                peer_lookup,
                outgoing: outgoing_rx,
                incoming: incoming.clone(),
                shutdown: shutdown_rx,
                neighbors: Arc::clone(&neighbors),
            }
            .run(),
        );
        Self {
            outgoing,
            incoming,
            shutdown,
            neighbors,
        }
    }

    pub async fn publish(&self, payload: Vec<u8>) -> Result<(), DeviceNetworkGossipError> {
        if payload.len() > GOSSIP_MESSAGE_LIMIT {
            return Err(DeviceNetworkGossipError::PayloadTooLarge);
        }
        self.outgoing
            .send(payload)
            .await
            .map_err(|_| DeviceNetworkGossipError::Unavailable)
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Vec<u8>> {
        self.incoming.subscribe()
    }

    pub fn shutdown_receiver(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub fn neighbor_count(&self) -> usize {
        self.neighbors.load(Ordering::SeqCst)
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.shutdown.send(true);
    }
}

struct GossipBusActor {
    gossip: Gossip,
    topic_id: TopicId,
    local_endpoint_id: EndpointId,
    peer_directory: DevicePeerDirectory,
    peer_lookup: MemoryLookup,
    outgoing: tokio::sync::mpsc::Receiver<Vec<u8>>,
    incoming: tokio::sync::broadcast::Sender<Vec<u8>>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    neighbors: Arc<AtomicUsize>,
}

impl GossipBusActor {
    async fn run(mut self) {
        let bootstrap = refresh_peer_directory(
            &self.peer_directory,
            &self.peer_lookup,
            self.local_endpoint_id,
        );
        let topic = match self.gossip.subscribe(self.topic_id, bootstrap).await {
            Ok(topic) => topic,
            Err(error) => {
                tracing::warn!(error = %error, "failed to join Device Network Gossip topic");
                return;
            }
        };
        let (sender, mut receiver) = topic.split();

        loop {
            tokio::select! {
                changed = self.shutdown.changed() => {
                    if changed.is_err() || *self.shutdown.borrow() {
                        return;
                    }
                }
                payload = self.outgoing.recv() => {
                    let Some(payload) = payload else {
                        return;
                    };
                    let peers = refresh_peer_directory(
                        &self.peer_directory,
                        &self.peer_lookup,
                        self.local_endpoint_id,
                    );
                    if !peers.is_empty() {
                        let _ = sender.join_peers(peers).await;
                    }
                    if let Err(error) = sender.broadcast(payload.into()).await {
                        tracing::debug!(error = %error, "Device Network Gossip broadcast failed");
                    }
                }
                event = receiver.next() => {
                    match event {
                        Some(Ok(Event::Received(message)))
                            if message.content.len() <= GOSSIP_MESSAGE_LIMIT =>
                        {
                            let _ = self.incoming.send(message.content.to_vec());
                        }
                        Some(Ok(Event::Received(_))) => {
                            tracing::warn!("ignored oversized Device Network Gossip payload");
                        }
                        Some(Ok(Event::Lagged)) => {
                            tracing::warn!("Device Network Gossip receiver lagged");
                        }
                        Some(Ok(Event::NeighborUp(_))) => {
                            self.neighbors.fetch_add(1, Ordering::SeqCst);
                        }
                        Some(Ok(Event::NeighborDown(_))) => {
                            self.neighbors.fetch_update(
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                                |count| Some(count.saturating_sub(1)),
                            ).ok();
                        }
                        Some(Err(error)) => {
                            tracing::debug!(error = %error, "Device Network Gossip receiver failed");
                            return;
                        }
                        None => return,
                    }
                }
            }
        }
    }
}

fn gossip_topic_id(network_id: [u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"rua-device-network-wake-v1");
    hasher.update(network_id);
    hasher.finalize().into()
}

fn refresh_peer_directory(
    peer_directory: &DevicePeerDirectory,
    peer_lookup: &MemoryLookup,
    local_endpoint_id: EndpointId,
) -> Vec<EndpointId> {
    let Ok(device_list) = peer_directory.read() else {
        return Vec::new();
    };
    device_list
        .devices
        .into_iter()
        .filter_map(|device| {
            let endpoint_addr = endpoint_addr_from_entry(&device).ok()?;
            let endpoint_id = endpoint_addr.id;
            if endpoint_id == local_endpoint_id {
                return None;
            }
            if !endpoint_addr.addrs.is_empty() {
                peer_lookup.add_endpoint_info(endpoint_addr);
            }
            Some(endpoint_id)
        })
        .collect()
}
