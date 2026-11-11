//! Packet delay around real iroh/QUIC connections. Native IP and relay
//! transports are disabled so holepunch candidates cannot bypass this network.
use std::{
    collections::HashMap,
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::Duration,
};

use futures::{stream::FuturesUnordered, StreamExt};
use iroh::endpoint::transports::{
    CustomEndpoint, CustomSender, CustomTransport, RecvInfo, Transmit,
};
use iroh_base::CustomAddr;
use tokio::{sync::mpsc, time::Instant};
use tokio_util::sync::CancellationToken;

const TRANSPORT_ID: u64 = 0x727561;
const PACKET_CAPACITY: usize = 512;

#[derive(Debug)]
struct Packet {
    source: CustomAddr,
    data: Vec<u8>,
}

#[derive(Debug)]
struct Outbound {
    packet: Packet,
    destination: mpsc::Sender<Packet>,
    deliver_at: Instant,
}

/// Bounded, lossy datagram delivery with independently delayed packets. QUIC
/// sees the delay in its actual ACK exchanges and computes its own path RTT.
#[derive(Debug)]
pub(super) struct DelayedNetwork {
    destinations: Arc<Mutex<HashMap<CustomAddr, mpsc::Sender<Packet>>>>,
    outgoing: mpsc::Sender<Outbound>,
    one_way_delay: Duration,
    blackhole: Arc<AtomicBool>,
    cancellation: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl DelayedNetwork {
    pub(super) fn new(one_way_delay: Duration) -> Self {
        let (outgoing, mut incoming) = mpsc::channel::<Outbound>(PACKET_CAPACITY);
        let cancellation = CancellationToken::new();
        let blackhole = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn({
            let cancellation = cancellation.clone();
            let blackhole = Arc::clone(&blackhole);
            async move {
                let mut pending = FuturesUnordered::new();
                loop {
                    tokio::select! {
                        _ = cancellation.cancelled() => break,
                        Some(()) = pending.next(), if !pending.is_empty() => {},
                        outbound = incoming.recv() => {
                            let Some(outbound) = outbound else { break };
                            if blackhole.load(Ordering::SeqCst) || pending.len() >= PACKET_CAPACITY {
                                continue;
                            }
                            let blackhole = Arc::clone(&blackhole);
                            pending.push(async move {
                                tokio::time::sleep_until(outbound.deliver_at).await;
                                if !blackhole.load(Ordering::SeqCst) {
                                    // A full datagram receive queue loses the packet, as UDP does.
                                    let _ = outbound.destination.try_send(outbound.packet);
                                }
                            });
                        }
                    }
                }
            }
        });
        Self {
            destinations: Arc::new(Mutex::new(HashMap::new())),
            outgoing,
            one_way_delay,
            blackhole,
            cancellation,
            task: Some(task),
        }
    }

    pub(super) fn transport(&self, endpoint: iroh::EndpointId) -> Arc<DelayedTransport> {
        let addr = Self::addr(endpoint);
        let (send, recv) = mpsc::channel(PACKET_CAPACITY);
        assert!(self
            .destinations
            .lock()
            .unwrap()
            .insert(addr.clone(), send)
            .is_none());
        Arc::new(DelayedTransport {
            addr: n0_watcher::Watchable::new(vec![addr]),
            receiver: Mutex::new(Some(recv)),
            destinations: Arc::clone(&self.destinations),
            outgoing: self.outgoing.clone(),
            one_way_delay: self.one_way_delay,
        })
    }

    pub(super) fn endpoint_addr(endpoint: iroh::EndpointId) -> iroh::EndpointAddr {
        iroh::EndpointAddr::from_parts(
            endpoint,
            [iroh::TransportAddr::Custom(Self::addr(endpoint))],
        )
    }

    fn addr(endpoint: iroh::EndpointId) -> CustomAddr {
        CustomAddr::from_parts(TRANSPORT_ID, endpoint.as_bytes())
    }

    pub(super) fn set_blackhole(&self, enabled: bool) {
        self.blackhole.store(enabled, Ordering::SeqCst);
    }

    pub(super) async fn shutdown(mut self) {
        self.cancellation.cancel();
        self.task
            .take()
            .expect("network task")
            .await
            .expect("delayed network task");
    }
}

impl Drop for DelayedNetwork {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Debug)]
pub(super) struct DelayedTransport {
    addr: n0_watcher::Watchable<Vec<CustomAddr>>,
    receiver: Mutex<Option<mpsc::Receiver<Packet>>>,
    destinations: Arc<Mutex<HashMap<CustomAddr, mpsc::Sender<Packet>>>>,
    outgoing: mpsc::Sender<Outbound>,
    one_way_delay: Duration,
}

impl CustomTransport for DelayedTransport {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
        Ok(Box::new(DelayedEndpoint {
            addr: self.addr.clone(),
            receiver: self.receiver.lock().unwrap().take().expect("bind once"),
            sender: Arc::new(DelayedSender {
                source: self.addr.get()[0].clone(),
                destinations: Arc::clone(&self.destinations),
                outgoing: self.outgoing.clone(),
                one_way_delay: self.one_way_delay,
            }),
        }))
    }
}

#[derive(Debug)]
struct DelayedEndpoint {
    addr: n0_watcher::Watchable<Vec<CustomAddr>>,
    receiver: mpsc::Receiver<Packet>,
    sender: Arc<DelayedSender>,
}

impl CustomEndpoint for DelayedEndpoint {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
        self.addr.watch()
    }

    fn create_sender(&self) -> Arc<dyn CustomSender> {
        self.sender.clone()
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
        metas: &mut [noq_udp::RecvMeta],
        recv_infos: &mut [RecvInfo],
    ) -> Poll<io::Result<usize>> {
        assert_eq!(bufs.len(), metas.len());
        assert_eq!(bufs.len(), recv_infos.len());
        if bufs.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut packets = Vec::new();
        let count = match self.receiver.poll_recv_many(cx, &mut packets, bufs.len()) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(0) => return Poll::Ready(Err(io::Error::other("network closed"))),
            Poll::Ready(count) => count,
        };
        for (index, packet) in packets.into_iter().enumerate() {
            if packet.data.len() > bufs[index].len() {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "datagram exceeds receive buffer",
                )));
            }
            bufs[index][..packet.data.len()].copy_from_slice(&packet.data);
            metas[index].len = packet.data.len();
            metas[index].stride = packet.data.len();
            recv_infos[index] = RecvInfo::new(packet.source, Some(self.sender.source.clone()));
        }
        Poll::Ready(Ok(count))
    }
}

#[derive(Debug)]
struct DelayedSender {
    source: CustomAddr,
    destinations: Arc<Mutex<HashMap<CustomAddr, mpsc::Sender<Packet>>>>,
    outgoing: mpsc::Sender<Outbound>,
    one_way_delay: Duration,
}

impl CustomSender for DelayedSender {
    fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
        addr.id() == TRANSPORT_ID
    }

    fn poll_send(
        &self,
        _cx: &mut Context<'_>,
        dst: &CustomAddr,
        transmit: &Transmit<'_>,
    ) -> Poll<io::Result<()>> {
        if transmit.contents.is_empty() {
            return Poll::Ready(Ok(()));
        }
        let destinations = self.destinations.lock().unwrap();
        let destination = destinations
            .get(dst)
            .ok_or_else(|| io::Error::other("unknown endpoint"))?;
        for data in transmit
            .contents
            .chunks(transmit.segment_size.unwrap_or(transmit.contents.len()))
        {
            let _ = self.outgoing.try_send(Outbound {
                packet: Packet {
                    source: self.source.clone(),
                    data: data.to_vec(),
                },
                destination: destination.clone(),
                deliver_at: Instant::now() + self.one_way_delay,
            });
        }
        Poll::Ready(Ok(()))
    }
}
