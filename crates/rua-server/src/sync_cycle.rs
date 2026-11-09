use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rua_sync::endpoint::SyncNode;
use rua_sync::transport::SyncCycleResult;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing::{info, warn};

static SYNC_CYCLE_GATE: Mutex<()> = Mutex::const_new(());
const FIRST_SYNC_ENDPOINT_ONLINE_WAIT: Duration = Duration::from_secs(5);
const FIRST_SYNC_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(15);
const FIRST_SYNC_MAX_ATTEMPTS: usize = 3;
const FIRST_SYNC_RETRY_DELAY: Duration = Duration::from_secs(1);

/// End-to-end budget for one single-peer Sync Cycle: waiting for the cycle gate,
/// resolving the peer address, and the transport attempt. A Sync Now or targeted
/// cycle queued behind a running cycle must not stack its own timeout on top of
/// that cycle's timeout.
const SINGLE_PEER_CYCLE_BUDGET: Duration = Duration::from_secs(30);

/// Error code for a cycle that gave up waiting for the previous Sync Cycle.
pub const SYNC_CYCLE_QUEUE_TIMEOUT: &str = "sync_cycle_queue_timeout";

/// Error code for a cycle whose budget ran out before transport could finish.
/// Unlike a transport timeout this is not attributed to the peer, because queueing
/// may have consumed the budget.
pub const SYNC_CYCLE_DEADLINE_TIMEOUT: &str = "sync_cycle_deadline_timeout";
#[cfg(debug_assertions)]
const TEST_SYNC_CYCLE_DELAY_MAX_MS: u64 = 60_000;

pub use crate::sync_peer_directory::{
    all_peer_candidates_for_device_list_fanout, clear_transport_active,
    load_peer_health_diagnostics, load_transport_stats, ordered_available_peer_node_ids_for_cycle,
    peer_candidate_for_target_device_id, peer_health_diagnostics_for_cycle,
    record_transport_active, record_transport_cycle, PeerCandidate, PeerHealthDiagnostics,
    PeerHealthEntry, PeerHealthState, SyncTransportActiveCycle, SyncTransportCycleStats,
    SyncTransportCycleStatus, SyncTransportRollingStats, SyncTransportStatsState,
    TargetPeerSelectionError,
};

#[derive(Debug, Clone, Default)]
pub struct AllPeerSyncCycleResult {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub events_received: usize,
    pub events_sent: usize,
    pub already_syncing_retries: usize,
    pub failures: Vec<String>,
}

/// Wall-clock budget shared by the queue and transport phases of one Sync Cycle.
#[derive(Debug, Clone, Copy)]
pub struct SyncCycleDeadline {
    expires_at: Instant,
}

impl SyncCycleDeadline {
    pub fn starting_now(budget: Duration) -> Self {
        Self {
            expires_at: Instant::now() + budget,
        }
    }

    /// Budget left for the remaining phases; zero once the deadline has passed.
    pub fn remaining(&self) -> Duration {
        self.expires_at.saturating_duration_since(Instant::now())
    }
}

/// Acquire the Sync Cycle gate without waiting past `deadline`.
///
/// The gate serialises Sync Cycles process-wide, so waiting for it must stay
/// inside the same end-to-end budget as the transport attempt.
pub async fn acquire_sync_cycle_gate<'a>(
    gate: &'a Mutex<()>,
    deadline: SyncCycleDeadline,
) -> Result<tokio::sync::MutexGuard<'a, ()>, String> {
    let queue_budget = deadline.remaining();
    if queue_budget.is_zero() {
        return Err(format!(
            "{SYNC_CYCLE_QUEUE_TIMEOUT}: cycle budget exhausted before acquiring the gate"
        ));
    }
    match tokio::time::timeout(queue_budget, gate.lock()).await {
        Ok(guard) => Ok(guard),
        Err(_) => Err(format!(
            "{SYNC_CYCLE_QUEUE_TIMEOUT}: previous Sync Cycle still holds the gate after {queue_budget:?}"
        )),
    }
}

async fn clone_sync_node(sync_node: &Arc<Mutex<Option<SyncNode>>>) -> Result<SyncNode, String> {
    sync_node
        .lock()
        .await
        .as_ref()
        .cloned()
        .ok_or_else(|| "not_paired: no sync node available".to_string())
}

pub async fn run_sync_cycle(
    sync_node: &Arc<Mutex<Option<SyncNode>>>,
    sync_dir: &Path,
) -> Result<SyncCycleResult, String> {
    fail_sync_cycle_if_injected().await?;
    let deadline = SyncCycleDeadline::starting_now(SINGLE_PEER_CYCLE_BUDGET);
    let _cycle_guard = acquire_sync_cycle_gate(&SYNC_CYCLE_GATE, deadline).await?;
    let node = tokio::time::timeout(deadline.remaining(), clone_sync_node(sync_node))
        .await
        .map_err(|_| format!("{SYNC_CYCLE_QUEUE_TIMEOUT}: waiting for the Sync Node"))??;

    let device_list = crate::sync_peer_directory::load_device_list(sync_dir);

    let self_pubkey = node.node_id();
    let self_pubkey_bytes = self_pubkey.as_bytes();

    let mut peer_health = crate::sync_peer_directory::load_peer_health(sync_dir);
    let cycle_started_ms = crate::sync_peer_directory::now_ms();
    let peer_candidates = crate::sync_peer_directory::ordered_peer_candidates_for_cycle(
        &device_list,
        self_pubkey_bytes,
        &peer_health,
        cycle_started_ms,
    );

    if device_list
        .devices
        .iter()
        .all(|d| &d.public_key == self_pubkey_bytes)
    {
        return Err("no_eligible_peer: no remote peers in device_list".to_string());
    }

    if peer_candidates.is_empty() {
        return Err("no_eligible_peer: all peers are in failure cooldown".to_string());
    }

    let candidate = peer_candidates
        .into_iter()
        .next()
        .expect("checked non-empty peer candidates");
    run_sync_cycle_candidate(
        &node,
        sync_dir,
        &mut peer_health,
        cycle_started_ms,
        &candidate,
        true,
        deadline,
    )
    .await
    .map_err(|error| format!("peer {}: {error}", candidate.node_id.fmt_short()))
}

pub async fn run_sync_cycle_to_device_id(
    sync_node: &Arc<Mutex<Option<SyncNode>>>,
    sync_dir: &Path,
    target_device_id_hex: &str,
) -> Result<SyncCycleResult, String> {
    fail_sync_cycle_if_injected().await?;
    let deadline = SyncCycleDeadline::starting_now(SINGLE_PEER_CYCLE_BUDGET);
    let _cycle_guard = acquire_sync_cycle_gate(&SYNC_CYCLE_GATE, deadline).await?;
    let node = tokio::time::timeout(deadline.remaining(), clone_sync_node(sync_node))
        .await
        .map_err(|_| format!("{SYNC_CYCLE_QUEUE_TIMEOUT}: waiting for the Sync Node"))??;

    let device_list = crate::sync_peer_directory::load_device_list(sync_dir);
    let self_pubkey = node.node_id();
    let self_pubkey_bytes = self_pubkey.as_bytes();

    let mut peer_health = crate::sync_peer_directory::load_peer_health(sync_dir);
    let cycle_started_ms = crate::sync_peer_directory::now_ms();
    let candidate = crate::sync_peer_directory::peer_candidate_for_target_device_id(
        &device_list,
        self_pubkey_bytes,
        &peer_health,
        target_device_id_hex,
    )
    .map_err(|e| e.to_string())?;

    run_sync_cycle_candidate(
        &node,
        sync_dir,
        &mut peer_health,
        cycle_started_ms,
        &candidate,
        true,
        deadline,
    )
    .await
    .map_err(|error| format!("peer {}: {error}", candidate.node_id.fmt_short()))
}

pub async fn run_sync_cycle_to_all_device_list_peers(
    sync_node: &Arc<Mutex<Option<SyncNode>>>,
    sync_dir: &Path,
) -> Result<AllPeerSyncCycleResult, String> {
    fail_sync_cycle_if_injected().await?;
    let deadline = SyncCycleDeadline::starting_now(SINGLE_PEER_CYCLE_BUDGET);
    let _cycle_guard = acquire_sync_cycle_gate(&SYNC_CYCLE_GATE, deadline).await?;
    let node = tokio::time::timeout(deadline.remaining(), clone_sync_node(sync_node))
        .await
        .map_err(|_| format!("{SYNC_CYCLE_QUEUE_TIMEOUT}: waiting for the Sync Node"))??;
    run_sync_cycle_to_all_device_list_peers_locked(
        &node,
        sync_dir,
        AllPeerRetryMode::AlreadySyncingOnly,
    )
    .await
}

pub async fn run_first_sync_cycle_to_all_device_list_peers(
    sync_node: &Arc<Mutex<Option<SyncNode>>>,
    sync_dir: &Path,
) -> Result<AllPeerSyncCycleResult, String> {
    fail_sync_cycle_if_injected().await?;
    let deadline = SyncCycleDeadline::starting_now(SINGLE_PEER_CYCLE_BUDGET);
    let _cycle_guard = acquire_sync_cycle_gate(&SYNC_CYCLE_GATE, deadline).await?;
    let node = tokio::time::timeout(deadline.remaining(), clone_sync_node(sync_node))
        .await
        .map_err(|_| format!("{SYNC_CYCLE_QUEUE_TIMEOUT}: waiting for the Sync Node"))??;
    if tokio::time::timeout(FIRST_SYNC_ENDPOINT_ONLINE_WAIT, node.endpoint().online())
        .await
        .is_err()
    {
        warn!(
            wait_ms = FIRST_SYNC_ENDPOINT_ONLINE_WAIT.as_millis(),
            "[sync] First Sync endpoint did not become Relay-online before peer discovery"
        );
    }
    run_sync_cycle_to_all_device_list_peers_locked(
        &node,
        sync_dir,
        AllPeerRetryMode::FirstSyncReadiness,
    )
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AllPeerRetryMode {
    AlreadySyncingOnly,
    FirstSyncReadiness,
}

#[cfg(debug_assertions)]
async fn fail_sync_cycle_if_injected() -> Result<(), String> {
    let enabled = std::env::var("RUA_TEST_FAIL_SYNC_CYCLES")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes"));
    if !enabled {
        return Ok(());
    }
    let delay_ms = std::env::var("RUA_TEST_SYNC_CYCLE_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
        .min(TEST_SYNC_CYCLE_DELAY_MAX_MS);
    if delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    Err("injected_sync_cycle_failure: stable Sync Cycle unavailable".to_string())
}

#[cfg(not(debug_assertions))]
async fn fail_sync_cycle_if_injected() -> Result<(), String> {
    Ok(())
}

async fn run_sync_cycle_to_all_device_list_peers_locked(
    node: &SyncNode,
    sync_dir: &Path,
    retry_mode: AllPeerRetryMode,
) -> Result<AllPeerSyncCycleResult, String> {
    let device_list = crate::sync_peer_directory::load_device_list(sync_dir);
    let self_pubkey = node.node_id();
    let self_pubkey_bytes = self_pubkey.as_bytes();
    if device_list
        .devices
        .iter()
        .all(|d| &d.public_key == self_pubkey_bytes)
    {
        return Err("not_paired: no peers in device_list".to_string());
    }

    let mut peer_health = crate::sync_peer_directory::load_peer_health(sync_dir);
    let peer_candidates = crate::sync_peer_directory::all_peer_candidates_for_device_list_fanout(
        &device_list,
        self_pubkey_bytes,
        &peer_health,
    );
    if peer_candidates.is_empty() {
        return Err("no reachable peer candidates in device_list".to_string());
    }

    let mut result = AllPeerSyncCycleResult {
        attempted: peer_candidates.len(),
        ..Default::default()
    };

    for candidate in &peer_candidates {
        let mut candidate = candidate.clone();
        let max_attempts = match retry_mode {
            AllPeerRetryMode::AlreadySyncingOnly => 20,
            AllPeerRetryMode::FirstSyncReadiness => {
                candidate.sync_timeout = candidate.sync_timeout.min(FIRST_SYNC_ATTEMPT_TIMEOUT);
                FIRST_SYNC_MAX_ATTEMPTS
            }
        };
        match run_sync_cycle_candidate_with_retry(
            node,
            sync_dir,
            &mut peer_health,
            &candidate,
            max_attempts,
            retry_mode,
        )
        .await
        {
            Ok(peer_result) => {
                result.succeeded += 1;
                result.events_received = result
                    .events_received
                    .saturating_add(peer_result.result.events_received);
                result.events_sent = result
                    .events_sent
                    .saturating_add(peer_result.result.events_sent);
                result.already_syncing_retries = result
                    .already_syncing_retries
                    .saturating_add(peer_result.already_syncing_retries);
            }
            Err(error) => {
                result.failed += 1;
                result
                    .failures
                    .push(format!("peer {}: {error}", candidate.node_id.fmt_short()));
            }
        }
    }

    Ok(result)
}

async fn run_sync_cycle_candidate(
    node: &SyncNode,
    sync_dir: &Path,
    peer_health: &mut crate::sync_peer_directory::PeerHealthState,
    cycle_started_ms: u64,
    candidate: &PeerCandidate,
    already_syncing_as_success: bool,
    deadline: SyncCycleDeadline,
) -> Result<SyncCycleResult, String> {
    let peer_addr = resolve_peer_addr_within_budget(node, candidate, deadline).await?;
    let transport_budget = deadline.remaining().min(candidate.sync_timeout);
    if transport_budget.is_zero() {
        return Err(format!(
            "{SYNC_CYCLE_DEADLINE_TIMEOUT}: cycle budget exhausted before transport"
        ));
    }

    crate::sync_peer_directory::record_transport_active(
        sync_dir,
        &candidate.node_id_string,
        cycle_started_ms,
    );
    match rua_sync::transport::sync_now(node, peer_addr, transport_budget).await {
        Ok(mut result) => {
            let completed_at_ms = crate::sync_peer_directory::now_ms();
            crate::sync_peer_directory::mark_peer_success(
                peer_health,
                &candidate.node_id_string,
                completed_at_ms,
            );
            if let Err(e) = crate::sync_peer_directory::save_peer_health(sync_dir, peer_health) {
                warn!(error = %e, "[sync] save peer health after success failed");
            }
            result.peer_node_id = candidate.node_id_string.clone();
            crate::sync_peer_directory::record_transport_success(
                sync_dir,
                &candidate.node_id_string,
                cycle_started_ms,
                completed_at_ms,
                &result,
            );
            Ok(result)
        }
        Err(e) => {
            if e.to_string().contains("AlreadySyncing") {
                if !already_syncing_as_success {
                    crate::sync_peer_directory::clear_transport_active(
                        sync_dir,
                        &candidate.node_id_string,
                    );
                    return Err("already syncing".to_string());
                }
                info!(
                    peer = %candidate.node_id.fmt_short(),
                    "sync already in progress with peer, treating as success"
                );
                crate::sync_peer_directory::mark_peer_success(
                    peer_health,
                    &candidate.node_id_string,
                    crate::sync_peer_directory::now_ms(),
                );
                if let Err(e) = crate::sync_peer_directory::save_peer_health(sync_dir, peer_health)
                {
                    warn!(error = %e, "[sync] save peer health after already-syncing failed");
                }
                crate::sync_peer_directory::clear_transport_active(
                    sync_dir,
                    &candidate.node_id_string,
                );
                return Ok(SyncCycleResult {
                    peer_node_id: candidate.node_id_string.clone(),
                    ..Default::default()
                });
            }
            let failed_at_ms = crate::sync_peer_directory::now_ms();
            crate::sync_peer_directory::mark_peer_failure(
                peer_health,
                &candidate.node_id_string,
                failed_at_ms,
            );
            if let Err(save_err) =
                crate::sync_peer_directory::save_peer_health(sync_dir, peer_health)
            {
                warn!(error = %save_err, "[sync] save peer health after failure failed");
            }
            crate::sync_peer_directory::record_transport_failure(
                sync_dir,
                &candidate.node_id_string,
                cycle_started_ms,
                failed_at_ms,
                match &e {
                    rua_sync::transport::TransportError::PeerUnreachable(_) => {
                        crate::sync_peer_directory::SyncTransportFailureKind::PeerUnavailable
                    }
                    _ => crate::sync_peer_directory::SyncTransportFailureKind::Internal,
                },
                &e.to_string(),
            );
            warn!(
                peer = %candidate.node_id.fmt_short(),
                error = %e,
                "sync with peer failed"
            );
            Err(e.to_string())
        }
    }
}

async fn resolve_peer_addr_within_budget(
    node: &SyncNode,
    candidate: &PeerCandidate,
    deadline: SyncCycleDeadline,
) -> Result<iroh::EndpointAddr, String> {
    let resolve_budget = deadline.remaining();
    if resolve_budget.is_zero() {
        return Err(format!(
            "{SYNC_CYCLE_DEADLINE_TIMEOUT}: cycle budget exhausted before resolving the peer address"
        ));
    }
    match tokio::time::timeout(
        resolve_budget,
        node.network_runtime().resolve_peer_addr(candidate.node_id),
    )
    .await
    {
        Ok(Ok(Some(peer_addr))) => Ok(peer_addr),
        Ok(Ok(None)) => Err("resolve peer address: paired peer is missing".to_string()),
        Ok(Err(error)) => Err(format!("resolve peer address: {error}")),
        Err(_) => Err(format!(
            "{SYNC_CYCLE_DEADLINE_TIMEOUT}: peer address resolution exceeded the cycle budget"
        )),
    }
}

#[derive(Debug, Clone)]
struct RetriedSyncCycleResult {
    result: SyncCycleResult,
    already_syncing_retries: usize,
}

async fn run_sync_cycle_candidate_with_retry(
    node: &SyncNode,
    sync_dir: &Path,
    peer_health: &mut crate::sync_peer_directory::PeerHealthState,
    candidate: &PeerCandidate,
    max_attempts: usize,
    retry_mode: AllPeerRetryMode,
) -> Result<RetriedSyncCycleResult, String> {
    let mut last_error = String::new();
    let mut already_syncing_retries = 0usize;
    for attempt in 1..=max_attempts.max(1) {
        let cycle_started_ms = crate::sync_peer_directory::now_ms();
        match run_sync_cycle_candidate(
            node,
            sync_dir,
            peer_health,
            cycle_started_ms,
            candidate,
            false,
            SyncCycleDeadline::starting_now(candidate.sync_timeout),
        )
        .await
        {
            Ok(result) => {
                return Ok(RetriedSyncCycleResult {
                    result,
                    already_syncing_retries,
                });
            }
            Err(error)
                if attempt < max_attempts
                    && (is_already_syncing_retry_error(&error)
                        || retry_mode == AllPeerRetryMode::FirstSyncReadiness) =>
            {
                let already_syncing = is_already_syncing_retry_error(&error);
                if already_syncing {
                    already_syncing_retries = already_syncing_retries.saturating_add(1);
                }
                last_error = error;
                let delay = if already_syncing {
                    Duration::from_millis(250)
                } else {
                    FIRST_SYNC_RETRY_DELAY
                };
                warn!(
                    peer = %candidate.node_id.fmt_short(),
                    attempt,
                    max_attempts,
                    retry_delay_ms = delay.as_millis(),
                    error = %last_error,
                    "[sync] First Sync peer is not ready; retrying within the same Cycle"
                );
                tokio::time::sleep(delay).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

fn is_already_syncing_retry_error(error: &str) -> bool {
    error.to_ascii_lowercase().contains("already syncing")
}
