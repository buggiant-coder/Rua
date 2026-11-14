//! Network implementation of the iroh-docs protocol

use std::future::Future;

use iroh::{endpoint::PathEvent, Endpoint, EndpointAddr, PublicKey, TransportAddr};
use n0_future::time::{Duration, Instant};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;
use tracing::{debug, error_span, trace, Instrument};

use crate::{
    actor::SyncHandle,
    metrics::Metrics,
    net::codec::{run_alice, BobState},
    NamespaceId, SyncOutcome,
};

/// The ALPN identifier for the iroh-docs protocol
pub const ALPN: &[u8] = b"/iroh-sync/1";

mod codec;

/// Connect to a peer and sync a replica
pub async fn connect_and_sync(
    endpoint: &Endpoint,
    sync: &SyncHandle,
    namespace: NamespaceId,
    peer: EndpointAddr,
    metrics: Option<&Metrics>,
) -> Result<SyncFinished, ConnectError> {
    connect_and_sync_observed(endpoint, sync, namespace, peer, metrics)
        .await
        .map(|finished| finished.sync)
}

/// Connect to a peer and sync a replica, returning observed transport path data.
pub async fn connect_and_sync_observed(
    endpoint: &Endpoint,
    sync: &SyncHandle,
    namespace: NamespaceId,
    peer: EndpointAddr,
    metrics: Option<&Metrics>,
) -> Result<ObservedSyncFinished, ConnectError> {
    let t_start = Instant::now();
    let peer_id = peer.id;
    trace!("connect");
    let connection = endpoint
        .connect(peer, crate::ALPN)
        .await
        .map_err(ConnectError::connect)?;

    let (mut send_stream, mut recv_stream) =
        connection.open_bi().await.map_err(ConnectError::connect)?;

    let t_connect = t_start.elapsed();
    debug!(?t_connect, "connected");

    let mut path_events = connection.path_events();
    let mut observation = SyncPathObservation::default();

    let res = {
        let run_sync = run_alice(&mut send_stream, &mut recv_stream, sync, namespace, peer_id);
        tokio::pin!(run_sync);
        let mut path_events_open = true;
        loop {
            tokio::select! {
                event = path_events.next(), if path_events_open => {
                    match event {
                        Some(event) => observation.observe_path_event(event),
                        None => path_events_open = false,
                    }
                }
                res = &mut run_sync => break res,
            }
        }
    };

    send_stream.finish().map_err(ConnectError::close)?;
    send_stream.stopped().await.map_err(ConnectError::close)?;
    recv_stream
        .read_to_end(0)
        .await
        .map_err(ConnectError::close)?;
    observation.observe_open_paths(&connection);
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(1), path_events.next()).await
    {
        observation.observe_path_event(event);
    }
    observation.finalize_route();

    if let Some(metrics) = metrics {
        if res.is_ok() {
            metrics.sync_via_connect_success.inc();
        } else {
            metrics.sync_via_connect_failure.inc();
        }
    }

    let t_process = t_start.elapsed() - t_connect;
    match &res {
        Ok(res) => {
            debug!(
                ?t_connect,
                ?t_process,
                sent = %res.num_sent,
                recv = %res.num_recv,
                "done, ok"
            );
        }
        Err(err) => {
            debug!(?t_connect, ?t_process, ?err, "done, failed");
        }
    }

    let outcome = res?;

    let timings = Timings {
        connect: t_connect,
        process: t_process,
    };

    let sync = SyncFinished {
        namespace,
        peer: peer_id,
        outcome,
        timings,
    };

    Ok(ObservedSyncFinished { sync, observation })
}

/// Whether we want to accept or reject an incoming sync request.
#[derive(Debug, Clone)]
pub enum AcceptOutcome {
    /// Accept the sync request.
    Allow,
    /// Decline the sync request
    Reject(AbortReason),
}

/// Handle an iroh-docs connection and sync all shared documents in the replica store.
pub async fn handle_connection<F, Fut>(
    sync: SyncHandle,
    connection: iroh::endpoint::Connection,
    accept_cb: F,
    metrics: Option<&Metrics>,
) -> Result<SyncFinished, AcceptError>
where
    F: Fn(NamespaceId, PublicKey) -> Fut,
    Fut: Future<Output = AcceptOutcome>,
{
    let accepted = handle_connection_protocol(sync, connection, accept_cb, metrics).await?;
    accepted.close.await?;
    accepted.result
}

/// Reconciliation and its transport drain have separate lifetimes. The live
/// actor can release the peer as soon as reconciliation stops mutating state,
/// while the original task retains the connection until graceful close ends.
pub(crate) struct AcceptedSync {
    pub result: Result<SyncFinished, AcceptError>,
    pub close: std::pin::Pin<Box<dyn Future<Output = Result<(), AcceptError>> + Send>>,
}

pub(crate) async fn handle_connection_protocol<F, Fut>(
    sync: SyncHandle,
    connection: iroh::endpoint::Connection,
    accept_cb: F,
    metrics: Option<&Metrics>,
) -> Result<AcceptedSync, AcceptError>
where
    F: Fn(NamespaceId, PublicKey) -> Fut,
    Fut: Future<Output = AcceptOutcome>,
{
    let t_start = Instant::now();
    let peer = connection.remote_id();
    let (mut send_stream, mut recv_stream) = connection
        .accept_bi()
        .await
        .map_err(|e| AcceptError::open(peer, e))?;

    let t_connect = t_start.elapsed();
    let span = error_span!("accept", peer = %peer.fmt_short(), namespace = tracing::field::Empty);
    span.in_scope(|| {
        debug!(?t_connect, "connection established");
    });

    let mut state = BobState::new(peer);
    let res = state
        .run(&mut send_stream, &mut recv_stream, sync, accept_cb)
        .instrument(span.clone())
        .await;

    if let Some(metrics) = metrics {
        if res.is_ok() {
            metrics.sync_via_accept_success.inc();
        } else {
            metrics.sync_via_accept_failure.inc();
        }
    }

    let namespace = state.namespace();
    let outcome = state.into_outcome();

    let t_process = t_start.elapsed() - t_connect;
    span.in_scope(|| match &res {
        Ok(_res) => {
            debug!(
                ?t_connect,
                ?t_process,
                sent = %outcome.num_sent,
                recv = %outcome.num_recv,
                "done, ok"
            );
        }
        Err(err) => {
            debug!(?t_connect, ?t_process, ?err, "done, failed");
        }
    });

    let result = res.map(|namespace| SyncFinished {
        namespace,
        outcome,
        peer,
        timings: Timings {
            connect: t_connect,
            process: t_process,
        },
    });
    let close = Box::pin(async move {
        // Keep the connection alive while queued stream data and FIN are drained.
        let _connection = connection;
        send_stream
            .finish()
            .map_err(|error| AcceptError::close(peer, namespace, error))?;
        send_stream
            .stopped()
            .await
            .map_err(|error| AcceptError::close(peer, namespace, error))?;
        recv_stream
            .read_to_end(0)
            .await
            .map_err(|error| AcceptError::close(peer, namespace, error))?;

        Ok(())
    });
    Ok(AcceptedSync { result, close })
}

/// Details of a finished sync operation.
#[derive(Debug, Clone)]
pub struct SyncFinished {
    /// The namespace that was synced.
    pub namespace: NamespaceId,
    /// The peer we syned with.
    pub peer: PublicKey,
    /// The outcome of the sync operation
    pub outcome: SyncOutcome,
    /// The time this operation took
    pub timings: Timings,
}

/// Details of a finished sync operation with transport path observation.
#[derive(Debug, Clone)]
pub struct ObservedSyncFinished {
    /// The ordinary sync result.
    pub sync: SyncFinished,
    /// Best-effort path stats captured from the actual iroh connection.
    pub observation: SyncPathObservation,
}

/// Route classification for a docs sync connection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRouteKind {
    /// No reliable path data was available.
    #[default]
    Unknown,
    /// The observed application path was direct/IP only.
    Direct,
    /// The observed application path was relay only.
    Relay,
    /// Both direct and relay paths were observed during the connection.
    Mixed,
}

/// Transport path kind for a docs sync connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncPathKind {
    /// Direct IP path.
    Direct,
    /// Relay path.
    Relay,
    /// Other transport address kind.
    Other,
}

/// Best-effort path observation for a docs sync connection.
///
/// Byte counters are iroh/QUIC UDP-layer counters and include protocol overhead.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPathObservation {
    /// Connection route classification derived from observed paths.
    pub route: SyncRouteKind,
    /// Whether at least one direct/IP path was observed.
    pub observed_direct: bool,
    /// Whether at least one relay path was observed.
    pub observed_relay: bool,
    /// UDP bytes sent on direct paths.
    pub direct_tx_bytes: u64,
    /// UDP bytes received on direct paths.
    pub direct_rx_bytes: u64,
    /// UDP bytes sent on relay paths.
    pub relay_tx_bytes: u64,
    /// UDP bytes received on relay paths.
    pub relay_rx_bytes: u64,
    /// Smallest observed direct-path RTT in milliseconds.
    pub direct_rtt_ms: Option<u64>,
    /// Smallest observed relay-path RTT in milliseconds.
    pub relay_rtt_ms: Option<u64>,
    /// Stats-bearing paths observed from lifecycle events and final snapshots.
    pub paths: Vec<SyncPathStat>,
}

impl SyncPathObservation {
    fn observe_open_paths(&mut self, connection: &iroh::endpoint::Connection) {
        let paths = connection.paths();
        for path in paths.iter() {
            let kind = if path.is_ip() {
                SyncPathKind::Direct
            } else if path.is_relay() {
                SyncPathKind::Relay
            } else {
                SyncPathKind::Other
            };
            let stats = path.stats();
            self.record_path(SyncPathStat {
                kind,
                selected: path.is_selected(),
                remote_addr: path.remote_addr().to_string(),
                rtt_ms: stats.rtt.as_millis() as u64,
                tx_bytes: stats.udp_tx.bytes,
                rx_bytes: stats.udp_rx.bytes,
                tx_datagrams: stats.udp_tx.datagrams,
                rx_datagrams: stats.udp_rx.datagrams,
                lost_packets: stats.lost_packets,
                lost_bytes: stats.lost_bytes,
            });
        }
    }

    fn observe_path_event(&mut self, event: PathEvent) {
        match event {
            PathEvent::Opened { remote_addr, .. } | PathEvent::Selected { remote_addr, .. } => {
                self.observe_addr(&remote_addr);
            }
            PathEvent::Closed {
                remote_addr,
                last_stats,
                ..
            } => {
                let kind = Self::kind_from_addr(&remote_addr);
                self.record_path(SyncPathStat {
                    kind,
                    selected: false,
                    remote_addr: remote_addr.to_string(),
                    rtt_ms: last_stats.rtt.as_millis() as u64,
                    tx_bytes: last_stats.udp_tx.bytes,
                    rx_bytes: last_stats.udp_rx.bytes,
                    tx_datagrams: last_stats.udp_tx.datagrams,
                    rx_datagrams: last_stats.udp_rx.datagrams,
                    lost_packets: last_stats.lost_packets,
                    lost_bytes: last_stats.lost_bytes,
                });
            }
            PathEvent::Lagged { .. } => {}
            _ => {}
        }
    }

    fn record_path(&mut self, path_stat: SyncPathStat) {
        match path_stat.kind {
            SyncPathKind::Direct => {
                self.observed_direct = true;
                self.direct_tx_bytes = self.direct_tx_bytes.saturating_add(path_stat.tx_bytes);
                self.direct_rx_bytes = self.direct_rx_bytes.saturating_add(path_stat.rx_bytes);
                self.direct_rtt_ms = min_optional(self.direct_rtt_ms, path_stat.rtt_ms);
            }
            SyncPathKind::Relay => {
                self.observed_relay = true;
                self.relay_tx_bytes = self.relay_tx_bytes.saturating_add(path_stat.tx_bytes);
                self.relay_rx_bytes = self.relay_rx_bytes.saturating_add(path_stat.rx_bytes);
                self.relay_rtt_ms = min_optional(self.relay_rtt_ms, path_stat.rtt_ms);
            }
            SyncPathKind::Other => {}
        }
        self.paths.push(path_stat);
    }

    fn observe_addr(&mut self, addr: &TransportAddr) {
        match Self::kind_from_addr(addr) {
            SyncPathKind::Direct => self.observed_direct = true,
            SyncPathKind::Relay => self.observed_relay = true,
            SyncPathKind::Other => {}
        }
    }

    fn kind_from_addr(addr: &TransportAddr) -> SyncPathKind {
        if addr.is_ip() {
            SyncPathKind::Direct
        } else if addr.is_relay() {
            SyncPathKind::Relay
        } else {
            SyncPathKind::Other
        }
    }

    fn finalize_route(&mut self) {
        self.route = match (self.observed_direct, self.observed_relay) {
            (true, true) => SyncRouteKind::Mixed,
            (true, false) => SyncRouteKind::Direct,
            (false, true) => SyncRouteKind::Relay,
            (false, false) => SyncRouteKind::Unknown,
        };
    }
}

fn min_optional(current: Option<u64>, next: u64) -> Option<u64> {
    Some(current.map_or(next, |current| current.min(next)))
}

/// Stats for a single observed transport path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPathStat {
    /// Path kind.
    pub kind: SyncPathKind,
    /// Whether this path was selected when the snapshot was captured.
    pub selected: bool,
    /// Remote transport address, stringified for diagnostics.
    pub remote_addr: String,
    /// RTT estimate in milliseconds.
    pub rtt_ms: u64,
    /// UDP bytes sent on this path.
    pub tx_bytes: u64,
    /// UDP bytes received on this path.
    pub rx_bytes: u64,
    /// UDP datagrams sent on this path.
    pub tx_datagrams: u64,
    /// UDP datagrams received on this path.
    pub rx_datagrams: u64,
    /// Lost packets on this path.
    pub lost_packets: u64,
    /// Lost bytes on this path.
    pub lost_bytes: u64,
}

/// Time a sync operation took
#[derive(Debug, Default, Clone)]
pub struct Timings {
    /// Time to establish connection
    pub connect: Duration,
    /// Time to run sync exchange
    pub process: Duration,
}

/// Errors that may occur on handling incoming sync connections.
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum AcceptError {
    /// Failed to establish connection
    #[error("Failed to establish connection")]
    Connect {
        #[source]
        error: anyhow::Error,
    },
    /// Failed to open replica
    #[error("Failed to open replica with {peer:?}")]
    Open {
        peer: PublicKey,
        #[source]
        error: anyhow::Error,
    },
    /// We aborted the sync request.
    #[error("Aborted sync of {namespace:?} with {peer:?}: {reason:?}")]
    Abort {
        peer: PublicKey,
        namespace: NamespaceId,
        reason: AbortReason,
    },
    /// Failed to run sync
    #[error("Failed to sync {namespace:?} with {peer:?}")]
    Sync {
        peer: PublicKey,
        namespace: Option<NamespaceId>,
        #[source]
        error: anyhow::Error,
    },
    /// Failed to close
    #[error("Failed to close {namespace:?} with {peer:?}")]
    Close {
        peer: PublicKey,
        namespace: Option<NamespaceId>,
        #[source]
        error: anyhow::Error,
    },
}

/// Errors that may occur on outgoing sync requests.
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum ConnectError {
    /// Failed to establish connection
    #[error("Failed to establish connection")]
    Connect {
        #[source]
        error: anyhow::Error,
    },
    /// The remote peer aborted the sync request.
    #[error("Remote peer aborted sync: {0:?}")]
    RemoteAbort(AbortReason),
    /// Failed to run sync
    #[error("Failed to sync")]
    Sync {
        #[source]
        error: anyhow::Error,
    },
    /// Failed to close
    #[error("Failed to close connection1")]
    Close {
        #[source]
        error: anyhow::Error,
    },
}

/// Reason why we aborted an incoming sync request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AbortReason {
    /// Namespace is not available.
    NotFound,
    /// We are already syncing this namespace.
    AlreadySyncing,
    /// We experienced an error while trying to provide the requested resource
    InternalServerError,
}

impl AcceptError {
    fn open(peer: PublicKey, error: impl Into<anyhow::Error>) -> Self {
        Self::Open {
            peer,
            error: error.into(),
        }
    }
    pub(crate) fn sync(
        peer: PublicKey,
        namespace: Option<NamespaceId>,
        error: impl Into<anyhow::Error>,
    ) -> Self {
        Self::Sync {
            peer,
            namespace,
            error: error.into(),
        }
    }
    fn close(
        peer: PublicKey,
        namespace: Option<NamespaceId>,
        error: impl Into<anyhow::Error>,
    ) -> Self {
        Self::Close {
            peer,
            namespace,
            error: error.into(),
        }
    }
    /// Get the peer's node ID (if available)
    pub fn peer(&self) -> Option<PublicKey> {
        match self {
            AcceptError::Connect { .. } => None,
            AcceptError::Open { peer, .. } => Some(*peer),
            AcceptError::Sync { peer, .. } => Some(*peer),
            AcceptError::Close { peer, .. } => Some(*peer),
            AcceptError::Abort { peer, .. } => Some(*peer),
        }
    }

    /// Get the namespace (if available)
    pub fn namespace(&self) -> Option<NamespaceId> {
        match self {
            AcceptError::Connect { .. } => None,
            AcceptError::Open { .. } => None,
            AcceptError::Sync { namespace, .. } => namespace.to_owned(),
            AcceptError::Close { namespace, .. } => namespace.to_owned(),
            AcceptError::Abort { namespace, .. } => Some(*namespace),
        }
    }
}

impl ConnectError {
    fn connect(error: impl Into<anyhow::Error>) -> Self {
        Self::Connect {
            error: error.into(),
        }
    }
    fn close(error: impl Into<anyhow::Error>) -> Self {
        Self::Close {
            error: error.into(),
        }
    }
    pub(crate) fn sync(error: impl Into<anyhow::Error>) -> Self {
        Self::Sync {
            error: error.into(),
        }
    }
    pub(crate) fn remote_abort(reason: AbortReason) -> Self {
        Self::RemoteAbort(reason)
    }
}

#[cfg(test)]
#[path = "../tests/unit_sync_close.rs"]
mod sync_close_tests;
