use std::time::Duration;

use iroh::EndpointAddr;
use iroh_docs::net::{self, ObservedSyncFinished};
use iroh_docs::store::Query;
use tracing::{debug, info, warn};

use crate::endpoint::SyncNode;
use crate::SyncError;

pub use iroh_docs::net::{SyncPathKind, SyncPathObservation, SyncPathStat, SyncRouteKind};

#[derive(Debug, Clone, Default)]
pub struct SyncCycleResult {
    pub events_received: usize,

    pub events_sent: usize,

    pub device_revoked: bool,

    pub warnings: Vec<String>,

    pub peer_node_id: String,

    pub conflicts: Vec<crate::conflict::ConflictRecord>,

    pub connect_ms: u64,

    pub process_ms: u64,

    pub path_observation: SyncPathObservation,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("peer unreachable: {0}")]
    PeerUnreachable(String),

    #[error("signature verification failed: {0}")]
    SignatureVerification(String),

    #[error("decryption failed: {0}")]
    DecryptionFailed(String),

    #[error("sync timeout after {0:?}")]
    Timeout(Duration),

    #[error("sync: {0}")]
    Sync(String),
}

impl From<TransportError> for SyncError {
    fn from(e: TransportError) -> Self {
        SyncError::Store(e.to_string())
    }
}

pub async fn sync_now(
    node: &SyncNode,
    peer_addr: EndpointAddr,
    timeout: Duration,
) -> Result<SyncCycleResult, TransportError> {
    let peer_id = peer_addr.id;
    match query_all_entry_hashes(node).await {
        Ok(hashes) => {
            let count = hashes.len();
            info!("[sync] pre-RBSR entry count: {count}");
            if count > 0 {
                let preview: Vec<_> = hashes
                    .iter()
                    .take(5)
                    .map(|h| format!("{h}")[..8].to_string())
                    .collect();
                info!("[sync] pre-RBSR first hashes: {preview:?}");
            }
        }
        Err(e) => {
            warn!(error = %e, "[sync] pre-RBSR entry query failed");
        }
    }

    // The peer may still be finishing its preceding reconciliation. A busy reply
    // is neither completion nor a permanent failure; keep the same peer and deadline.
    let reconcile = async {
        loop {
            match do_connect_and_sync(node, peer_addr.clone()).await {
                Err(net::ConnectError::RemoteAbort(net::AbortReason::AlreadySyncing)) => {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                result => break result,
            }
        }
    };
    let finished = match tokio::time::timeout(timeout, reconcile)
        .await
        .map_err(|_| TransportError::Timeout(timeout))?
    {
        Ok(finished) => finished,
        Err(e) if is_post_reconcile_close_error(&e) => {
            let warning = format!("RBSR close failed after sync protocol: {e}");
            warn!(
                peer = %peer_id.fmt_short(),
                error = %e,
                "[sync] RBSR close failed after sync protocol; treating peer as healthy"
            );
            return Ok(SyncCycleResult {
                warnings: vec![warning],
                ..Default::default()
            });
        }
        Err(e) => return Err(TransportError::PeerUnreachable(e.to_string())),
    };

    info!(
        peer = %finished.sync.peer.fmt_short(),
        sent = finished.sync.outcome.num_sent,
        recv = finished.sync.outcome.num_recv,
        connect_ms = finished.sync.timings.connect.as_millis(),
        process_ms = finished.sync.timings.process.as_millis(),
        route = ?finished.observation.route,
        "RBSR sync complete"
    );

    Ok(SyncCycleResult {
        events_received: finished.sync.outcome.num_recv,
        events_sent: finished.sync.outcome.num_sent,
        connect_ms: finished.sync.timings.connect.as_millis() as u64,
        process_ms: finished.sync.timings.process.as_millis() as u64,
        path_observation: finished.observation,
        ..Default::default()
    })
}

pub fn is_post_reconcile_close_error(error: &net::ConnectError) -> bool {
    matches!(error, net::ConnectError::Close { .. })
}

async fn do_connect_and_sync(
    node: &SyncNode,
    peer_addr: EndpointAddr,
) -> Result<ObservedSyncFinished, net::ConnectError> {
    let result = net::connect_and_sync_observed(
        node.endpoint(),
        node.sync_handle(),
        node.namespace_id(),
        peer_addr,
        None,
    )
    .await;
    if let Err(ref e) = result {
        debug!("connect_and_sync failed: {e:?}");
    }
    result
}

pub async fn query_all_entry_hashes(
    node: &SyncNode,
) -> Result<Vec<iroh_blobs::Hash>, TransportError> {
    let query = Query::all().build();
    let (tx, mut rx) = irpc::channel::mpsc::channel(256);
    node.sync_handle()
        .get_many(node.namespace_id(), query, tx)
        .await
        .map_err(|e| TransportError::Sync(format!("query entries: {e}")))?;

    let mut hashes = Vec::new();
    while let Ok(Some(result)) = rx.recv().await {
        match result {
            Ok(entry) => hashes.push(entry.content_hash()),
            Err(e) => {
                warn!(error = %e, "[sync] iroh entry error during query");
            }
        }
    }
    Ok(hashes)
}
