use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use rua_sync::device_list::DeviceList;
use rua_sync::transport::SyncCycleResult;
use serde::{Deserialize, Serialize};
use tracing::warn;

pub const PEER_HEALTH_FILE: &str = "peer_health.json";
const TRANSPORT_STATS_FILE: &str = "transport_stats.json";
static TRANSPORT_STATS_WRITE_LOCK: Mutex<()> = Mutex::new(());
const PEER_FAILURE_COOLDOWN_BASE_MS: u64 = 60_000;
const PEER_FAILURE_COOLDOWN_MAX_MS: u64 = 5 * 60_000;
const PEER_SYNC_TIMEOUT: Duration = Duration::from_secs(30);
const UNPROVEN_FAILED_PEER_SYNC_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerHealthState {
    #[serde(default)]
    pub peers: BTreeMap<String, PeerHealthEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerHealthEntry {
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct PeerCandidate {
    pub node_id: iroh::PublicKey,
    pub node_id_string: String,
    pub sync_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetPeerSelectionError {
    InvalidDeviceId(String),
    TargetNotFound(String),
    TargetIsSelf(String),
}

impl std::fmt::Display for TargetPeerSelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDeviceId(id) => write!(f, "invalid target device id: {id}"),
            Self::TargetNotFound(id) => write!(f, "target device not found: {id}"),
            Self::TargetIsSelf(id) => write!(f, "target device is this device: {id}"),
        }
    }
}

impl std::error::Error for TargetPeerSelectionError {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PeerHealthDiagnostics {
    pub total_peers: usize,
    pub selectable_peers: usize,
    pub peers_in_cooldown: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_peer_node_id: Option<String>,
    pub peers: Vec<PeerHealthDiagnosticEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeerHealthDiagnosticEntry {
    pub node_id: String,
    pub device_id_hex: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub friendly_name: String,
    pub last_seen_or_added_ms: u64,
    pub selectable: bool,
    pub consecutive_failures: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_until_ms: Option<u64>,
    pub sync_timeout_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_route: Option<rua_sync::transport::SyncRouteKind>,
    #[serde(default)]
    pub last_direct_bytes: u64,
    #[serde(default)]
    pub last_relay_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_rtt_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncTransportStatsState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_cycle: Option<SyncTransportActiveCycle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cycle: Option<SyncTransportCycleStats>,
    #[serde(default)]
    pub rolling: SyncTransportRollingStats,
    #[serde(default)]
    pub peers: BTreeMap<String, SyncPeerTransportStats>,
}

// Keep storage provenance separate from the stable HTTP transport DTO.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredTransportStats {
    #[serde(flatten)]
    raw: SyncTransportStatsState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legacy_unscoped: Option<SyncTransportStatsState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    networks: BTreeMap<String, NetworkTransportStats>,
    #[serde(default, flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct NetworkTransportStats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_cycle: Option<SyncTransportActiveCycle>,
    #[serde(default)]
    peers: BTreeMap<String, NetworkPeerTransportStats>,
    #[serde(default, flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct NetworkPeerTransportStats {
    #[serde(default)]
    rolling: SyncTransportRollingStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_cycle: Option<SyncTransportCycleStats>,
    #[serde(default, flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncTransportActiveCycle {
    pub started_at_ms: u64,
    pub peer_node_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncTransportCycleStats {
    pub completed_at_ms: u64,
    pub peer_node_id: String,
    pub status: SyncTransportCycleStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<SyncTransportFailureKind>,
    pub events_received: usize,
    pub events_sent: usize,
    pub connect_ms: u64,
    pub process_ms: u64,
    pub duration_ms: u64,
    pub route: rua_sync::transport::SyncRouteKind,
    pub direct_tx_bytes: u64,
    pub direct_rx_bytes: u64,
    pub relay_tx_bytes: u64,
    pub relay_rx_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_rtt_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_rtt_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl SyncTransportCycleStats {
    pub fn direct_bytes(&self) -> u64 {
        self.direct_tx_bytes.saturating_add(self.direct_rx_bytes)
    }

    pub fn relay_bytes(&self) -> u64 {
        self.relay_tx_bytes.saturating_add(self.relay_rx_bytes)
    }

    pub fn best_rtt_ms(&self) -> Option<u64> {
        match (self.direct_rtt_ms, self.relay_rtt_ms) {
            (Some(direct), Some(relay)) => Some(direct.min(relay)),
            (Some(direct), None) => Some(direct),
            (None, Some(relay)) => Some(relay),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTransportCycleStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTransportFailureKind {
    PeerUnavailable,
    Internal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncTransportRollingStats {
    #[serde(default)]
    pub cycles: u64,
    #[serde(default)]
    pub failures: u64,
    #[serde(default)]
    pub events_received: u64,
    #[serde(default)]
    pub events_sent: u64,
    #[serde(default)]
    pub direct_tx_bytes: u64,
    #[serde(default)]
    pub direct_rx_bytes: u64,
    #[serde(default)]
    pub relay_tx_bytes: u64,
    #[serde(default)]
    pub relay_rx_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPeerTransportStats {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cycle: Option<SyncTransportCycleStats>,
    #[serde(default)]
    pub direct_bytes: u64,
    #[serde(default)]
    pub relay_bytes: u64,
    #[serde(default)]
    pub cycles: u64,
    #[serde(default)]
    pub failures: u64,
}

#[derive(Debug, Clone)]
struct CandidateWithSort {
    candidate: PeerCandidate,
    last_seen_or_added_ms: u64,
    last_success_ms: u64,
    cooldown_until_ms: Option<u64>,
}

pub fn load_device_list(sync_dir: &Path) -> DeviceList {
    let device_list_path = sync_dir.join("device_list.json");
    std::fs::read_to_string(&device_list_path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn load_peer_health(sync_dir: &Path) -> PeerHealthState {
    load_peer_health_file(&sync_dir.join(PEER_HEALTH_FILE))
}

pub fn save_peer_health(sync_dir: &Path, peer_health: &PeerHealthState) -> Result<(), String> {
    save_peer_health_file(&sync_dir.join(PEER_HEALTH_FILE), peer_health)
}

pub fn ordered_peer_candidates_for_cycle(
    device_list: &DeviceList,
    self_pubkey_bytes: &[u8; 32],
    peer_health: &PeerHealthState,
    now_ms: u64,
) -> Vec<PeerCandidate> {
    select_peer_candidates_for_cycle(device_list, self_pubkey_bytes, peer_health, now_ms)
        .into_iter()
        .map(|candidate| candidate.candidate)
        .collect()
}

pub fn ordered_available_peer_node_ids_for_cycle(
    device_list: &DeviceList,
    self_pubkey_bytes: &[u8; 32],
    peer_health: &PeerHealthState,
    now_ms: u64,
) -> Vec<String> {
    ordered_peer_candidates_for_cycle(device_list, self_pubkey_bytes, peer_health, now_ms)
        .into_iter()
        .map(|candidate| candidate.node_id_string)
        .collect()
}

pub fn all_peer_candidates_for_device_list_fanout(
    device_list: &DeviceList,
    self_pubkey_bytes: &[u8; 32],
    peer_health: &PeerHealthState,
) -> Vec<PeerCandidate> {
    let mut candidates =
        peer_candidates_for_all_devices(device_list, self_pubkey_bytes, peer_health);
    candidates.sort_by_key(|candidate| {
        let health = peer_health
            .peers
            .get(&candidate.node_id_string)
            .cloned()
            .unwrap_or_default();
        (
            Reverse(health.last_success_ms.unwrap_or(0)),
            candidate.node_id_string.clone(),
        )
    });
    candidates
}

pub fn peer_candidate_for_target_device_id(
    device_list: &DeviceList,
    self_pubkey_bytes: &[u8; 32],
    peer_health: &PeerHealthState,
    target_device_id_hex: &str,
) -> Result<PeerCandidate, TargetPeerSelectionError> {
    let target_device_id = parse_device_id_hex(target_device_id_hex)?;
    let Some(entry) = device_list
        .devices
        .iter()
        .find(|device| device.device_id == target_device_id)
    else {
        return Err(TargetPeerSelectionError::TargetNotFound(
            target_device_id_hex.to_string(),
        ));
    };
    if &entry.public_key == self_pubkey_bytes {
        return Err(TargetPeerSelectionError::TargetIsSelf(
            target_device_id_hex.to_string(),
        ));
    }
    let node_id = iroh::PublicKey::from_bytes(&entry.public_key)
        .map_err(|_| TargetPeerSelectionError::InvalidDeviceId(target_device_id_hex.to_string()))?;
    let node_id_string = node_id.to_string();
    let health = peer_health
        .peers
        .get(&node_id_string)
        .cloned()
        .unwrap_or_default();
    Ok(PeerCandidate {
        node_id,
        node_id_string,
        sync_timeout: candidate_sync_timeout(&health),
    })
}

pub fn peer_health_diagnostics_for_cycle(
    device_list: &DeviceList,
    self_pubkey_bytes: &[u8; 32],
    peer_health: &PeerHealthState,
    now_ms: u64,
) -> PeerHealthDiagnostics {
    let ordered_selectable =
        select_peer_candidates_for_cycle(device_list, self_pubkey_bytes, peer_health, now_ms);
    let next_peer_node_id = ordered_selectable
        .first()
        .map(|candidate| candidate.candidate.node_id_string.clone());

    let mut peers = Vec::new();
    for entry in device_list
        .devices
        .iter()
        .filter(|device| &device.public_key != self_pubkey_bytes)
    {
        let node_id = match iroh::PublicKey::from_bytes(&entry.public_key) {
            Ok(pk) => pk,
            Err(e) => {
                warn!(error = %e, "invalid peer public key in device_list diagnostics, skipping");
                continue;
            }
        };
        let node_id_string = node_id.to_string();
        let health = peer_health
            .peers
            .get(&node_id_string)
            .cloned()
            .unwrap_or_default();
        let cooldown_until_ms = peer_cooldown_until_ms(&health, now_ms);
        peers.push(PeerHealthDiagnosticEntry {
            node_id: node_id_string,
            device_id_hex: hex::encode(entry.device_id),
            friendly_name: entry.friendly_name.clone(),
            last_seen_or_added_ms: entry.last_seen_ms.max(entry.added_at_ms),
            selectable: cooldown_until_ms.is_none(),
            consecutive_failures: health.consecutive_failures,
            last_failure_ms: health.last_failure_ms,
            last_success_ms: health.last_success_ms,
            cooldown_until_ms,
            sync_timeout_ms: candidate_sync_timeout(&health).as_millis() as u64,
            last_route: None,
            last_direct_bytes: 0,
            last_relay_bytes: 0,
            last_rtt_ms: None,
        });
    }
    peers.sort_by_key(|peer| {
        (
            !peer.selectable,
            Reverse(peer.last_success_ms.unwrap_or(0)),
            Reverse(peer.last_seen_or_added_ms),
        )
    });

    PeerHealthDiagnostics {
        total_peers: peers.len(),
        selectable_peers: peers.iter().filter(|peer| peer.selectable).count(),
        peers_in_cooldown: peers.iter().filter(|peer| !peer.selectable).count(),
        next_peer_node_id,
        peers,
    }
}

pub fn load_peer_health_diagnostics(
    sync_dir: &Path,
    self_pubkey_bytes: &[u8; 32],
) -> PeerHealthDiagnostics {
    let transport_stats = load_transport_stats(sync_dir);
    load_peer_health_diagnostics_with_transport(sync_dir, self_pubkey_bytes, &transport_stats)
}

pub fn load_peer_health_diagnostics_with_transport(
    sync_dir: &Path,
    self_pubkey_bytes: &[u8; 32],
    transport_stats: &SyncTransportStatsState,
) -> PeerHealthDiagnostics {
    let device_list = load_device_list(sync_dir);
    let peer_health = load_peer_health(sync_dir);
    let mut diagnostics =
        peer_health_diagnostics_for_cycle(&device_list, self_pubkey_bytes, &peer_health, now_ms());
    for peer in &mut diagnostics.peers {
        if let Some(stats) = transport_stats.peers.get(&peer.node_id) {
            if let Some(last_cycle) = &stats.last_cycle {
                peer.last_route = Some(last_cycle.route);
                peer.last_direct_bytes = last_cycle.direct_bytes();
                peer.last_relay_bytes = last_cycle.relay_bytes();
                peer.last_rtt_ms = last_cycle.best_rtt_ms();
            }
        }
    }
    diagnostics
}

pub fn mark_peer_success(peer_health: &mut PeerHealthState, peer_node_id: &str, now_ms: u64) {
    let entry = peer_health
        .peers
        .entry(peer_node_id.to_string())
        .or_default();
    entry.consecutive_failures = 0;
    entry.last_success_ms = Some(now_ms);
}

pub fn mark_peer_failure(peer_health: &mut PeerHealthState, peer_node_id: &str, now_ms: u64) {
    let entry = peer_health
        .peers
        .entry(peer_node_id.to_string())
        .or_default();
    entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
    entry.last_failure_ms = Some(now_ms);
}

pub fn record_transport_success(
    sync_dir: &Path,
    namespace_id: &str,
    peer_node_id: &str,
    started_at_ms: u64,
    completed_at_ms: u64,
    result: &SyncCycleResult,
) {
    let observation = &result.path_observation;
    let cycle = SyncTransportCycleStats {
        completed_at_ms,
        peer_node_id: peer_node_id.to_string(),
        status: SyncTransportCycleStatus::Success,
        error: None,
        failure_kind: None,
        events_received: result.events_received,
        events_sent: result.events_sent,
        connect_ms: result.connect_ms,
        process_ms: result.process_ms,
        duration_ms: completed_at_ms.saturating_sub(started_at_ms),
        route: observation.route,
        direct_tx_bytes: observation.direct_tx_bytes,
        direct_rx_bytes: observation.direct_rx_bytes,
        relay_tx_bytes: observation.relay_tx_bytes,
        relay_rx_bytes: observation.relay_rx_bytes,
        direct_rtt_ms: observation.direct_rtt_ms,
        relay_rtt_ms: observation.relay_rtt_ms,
        warnings: result.warnings.clone(),
    };
    record_transport_cycle_for_network(sync_dir, namespace_id, cycle);
}

#[allow(clippy::too_many_arguments)]
pub fn record_transport_failure(
    sync_dir: &Path,
    namespace_id: &str,
    peer_node_id: &str,
    started_at_ms: u64,
    completed_at_ms: u64,
    failure_kind: SyncTransportFailureKind,
    error: &str,
) {
    let cycle = SyncTransportCycleStats {
        completed_at_ms,
        peer_node_id: peer_node_id.to_string(),
        status: SyncTransportCycleStatus::Failure,
        error: Some(error.to_string()),
        failure_kind: Some(failure_kind),
        events_received: 0,
        events_sent: 0,
        connect_ms: 0,
        process_ms: 0,
        duration_ms: completed_at_ms.saturating_sub(started_at_ms),
        route: rua_sync::transport::SyncRouteKind::Unknown,
        direct_tx_bytes: 0,
        direct_rx_bytes: 0,
        relay_tx_bytes: 0,
        relay_rx_bytes: 0,
        direct_rtt_ms: None,
        relay_rtt_ms: None,
        warnings: Vec::new(),
    };
    record_transport_cycle_for_network(sync_dir, namespace_id, cycle);
}

pub fn record_transport_active(sync_dir: &Path, peer_node_id: &str, started_at_ms: u64) {
    let namespace_id = disk_namespace_id(sync_dir);
    record_transport_active_in_namespace(
        sync_dir,
        namespace_id.as_deref(),
        peer_node_id,
        started_at_ms,
    );
}

pub fn record_transport_active_for_network(
    sync_dir: &Path,
    namespace_id: &str,
    peer_node_id: &str,
    started_at_ms: u64,
) {
    record_transport_active_in_namespace(sync_dir, Some(namespace_id), peer_node_id, started_at_ms);
}

fn record_transport_active_in_namespace(
    sync_dir: &Path,
    namespace_id: Option<&str>,
    peer_node_id: &str,
    started_at_ms: u64,
) {
    update_transport_stats(sync_dir, |stored| {
        let active = SyncTransportActiveCycle {
            started_at_ms,
            peer_node_id: peer_node_id.to_owned(),
        };
        stored.raw.active_cycle = Some(active.clone());
        if let Some(namespace_id) = namespace_id {
            stored
                .networks
                .entry(namespace_id.to_owned())
                .or_default()
                .active_cycle = Some(active);
        }
        true
    });
}

pub fn clear_transport_active(sync_dir: &Path, peer_node_id: &str) {
    let namespace_id = disk_namespace_id(sync_dir);
    clear_transport_active_in_namespace(sync_dir, namespace_id.as_deref(), peer_node_id);
}

pub fn clear_transport_active_for_network(sync_dir: &Path, namespace_id: &str, peer_node_id: &str) {
    clear_transport_active_in_namespace(sync_dir, Some(namespace_id), peer_node_id);
}

fn clear_transport_active_in_namespace(
    sync_dir: &Path,
    namespace_id: Option<&str>,
    peer_node_id: &str,
) {
    update_transport_stats(sync_dir, |stored| {
        let mut changed = false;
        if stored
            .raw
            .active_cycle
            .as_ref()
            .is_some_and(|active| active.peer_node_id == peer_node_id)
        {
            stored.raw.active_cycle = None;
            changed = true;
        }
        if let Some(namespace_id) = namespace_id {
            let newly_scoped = !stored.networks.contains_key(namespace_id);
            let network = stored.networks.entry(namespace_id.to_owned()).or_default();
            if network
                .active_cycle
                .as_ref()
                .is_some_and(|active| active.peer_node_id == peer_node_id)
            {
                network.active_cycle = None;
                changed = true;
            }
            changed |= newly_scoped;
        }
        changed
    });
}

pub fn load_transport_stats(sync_dir: &Path) -> SyncTransportStatsState {
    load_stored_transport_stats(sync_dir).raw
}

fn load_stored_transport_stats(sync_dir: &Path) -> StoredTransportStats {
    let path = sync_dir.join(TRANSPORT_STATS_FILE);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn record_transport_cycle(sync_dir: &Path, cycle: SyncTransportCycleStats) {
    let namespace_id = disk_namespace_id(sync_dir);
    record_transport_cycle_in_namespace(sync_dir, namespace_id.as_deref(), cycle);
}

pub fn record_transport_cycle_for_network(
    sync_dir: &Path,
    namespace_id: &str,
    cycle: SyncTransportCycleStats,
) {
    record_transport_cycle_in_namespace(sync_dir, Some(namespace_id), cycle);
}

fn record_transport_cycle_in_namespace(
    sync_dir: &Path,
    namespace_id: Option<&str>,
    cycle: SyncTransportCycleStats,
) {
    update_transport_stats(sync_dir, |stored| {
        if let Some(namespace_id) = namespace_id {
            let network = stored.networks.entry(namespace_id.to_owned()).or_default();
            network.active_cycle = None;
            let peer = network.peers.entry(cycle.peer_node_id.clone()).or_default();
            add_cycle_totals(&mut peer.rolling, &cycle);
            peer.last_cycle = Some(cycle.clone());
        }
        let state = &mut stored.raw;
        state.active_cycle = None;
        add_cycle_totals(&mut state.rolling, &cycle);
        let peer = state.peers.entry(cycle.peer_node_id.clone()).or_default();
        peer.cycles = peer.cycles.saturating_add(1);
        if cycle.status == SyncTransportCycleStatus::Failure {
            peer.failures = peer.failures.saturating_add(1);
        }
        peer.direct_bytes = peer.direct_bytes.saturating_add(cycle.direct_bytes());
        peer.relay_bytes = peer.relay_bytes.saturating_add(cycle.relay_bytes());
        peer.last_cycle = Some(cycle.clone());
        state.last_cycle = Some(cycle);
        true
    });
}

fn disk_namespace_id(sync_dir: &Path) -> Option<String> {
    rua_sync::identity::NetworkSecret::load(sync_dir)
        .ok()
        .flatten()
        .map(|secret| hex::encode(secret.namespace_id().as_bytes()))
}

fn update_transport_stats(sync_dir: &Path, update: impl FnOnce(&mut StoredTransportStats) -> bool) {
    let _guard = match TRANSPORT_STATS_WRITE_LOCK.lock() {
        Ok(guard) => guard,
        Err(error) => {
            warn!(%error, "[sync] transport stats write lock poisoned");
            return;
        }
    };
    let mut stored = load_stored_transport_stats(sync_dir);
    if stored.legacy_unscoped.is_none() {
        stored.legacy_unscoped = Some(stored.raw.clone());
    }
    if !update(&mut stored) {
        return;
    }
    let bytes = match serde_json::to_vec_pretty(&stored) {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!(%error, "[sync] serialize transport stats failed");
            return;
        }
    };
    if let Err(error) = atomic_write(&sync_dir.join(TRANSPORT_STATS_FILE), &bytes) {
        warn!(%error, "[sync] save transport stats failed");
    }
}

fn add_cycle_totals(rolling: &mut SyncTransportRollingStats, cycle: &SyncTransportCycleStats) {
    rolling.cycles = rolling.cycles.saturating_add(1);
    if cycle.status == SyncTransportCycleStatus::Failure {
        rolling.failures = rolling.failures.saturating_add(1);
    }
    rolling.events_received = rolling
        .events_received
        .saturating_add(cycle.events_received as u64);
    rolling.events_sent = rolling.events_sent.saturating_add(cycle.events_sent as u64);
    rolling.direct_tx_bytes = rolling
        .direct_tx_bytes
        .saturating_add(cycle.direct_tx_bytes);
    rolling.direct_rx_bytes = rolling
        .direct_rx_bytes
        .saturating_add(cycle.direct_rx_bytes);
    rolling.relay_tx_bytes = rolling.relay_tx_bytes.saturating_add(cycle.relay_tx_bytes);
    rolling.relay_rx_bytes = rolling.relay_rx_bytes.saturating_add(cycle.relay_rx_bytes);
}

fn add_rolling_totals(total: &mut SyncTransportRollingStats, other: &SyncTransportRollingStats) {
    total.cycles = total.cycles.saturating_add(other.cycles);
    total.failures = total.failures.saturating_add(other.failures);
    total.events_received = total.events_received.saturating_add(other.events_received);
    total.events_sent = total.events_sent.saturating_add(other.events_sent);
    total.direct_tx_bytes = total.direct_tx_bytes.saturating_add(other.direct_tx_bytes);
    total.direct_rx_bytes = total.direct_rx_bytes.saturating_add(other.direct_rx_bytes);
    total.relay_tx_bytes = total.relay_tx_bytes.saturating_add(other.relay_tx_bytes);
    total.relay_rx_bytes = total.relay_rx_bytes.saturating_add(other.relay_rx_bytes);
}

/// Project local transport history onto the bound namespace and its current members.
/// Legacy files have no namespace or per-peer event/directional-byte provenance.
pub fn load_transport_stats_for_network(
    sync_dir: &Path,
    namespace_id: Option<&str>,
    self_public_key: &[u8; 32],
) -> SyncTransportStatsState {
    let Some(namespace_id) = namespace_id else {
        return SyncTransportStatsState::default();
    };
    let members: BTreeSet<_> = load_device_list(sync_dir)
        .devices
        .into_iter()
        .filter(|entry| &entry.public_key != self_public_key)
        .map(|entry| hex::encode(entry.public_key))
        .collect();
    let stored = load_stored_transport_stats(sync_dir);
    let legacy = stored.legacy_unscoped.as_ref().unwrap_or(&stored.raw);
    let complete_legacy_membership =
        !legacy.peers.is_empty() && legacy.peers.keys().all(|peer| members.contains(peer));
    let mut projected = SyncTransportStatsState::default();
    if complete_legacy_membership {
        projected.rolling = legacy.rolling.clone();
    }
    for (id, peer) in &legacy.peers {
        if members.contains(id) {
            projected.peers.insert(id.clone(), peer.clone());
            if !complete_legacy_membership {
                projected.rolling.cycles = projected.rolling.cycles.saturating_add(peer.cycles);
                projected.rolling.failures =
                    projected.rolling.failures.saturating_add(peer.failures);
            }
            take_latest_cycle(&mut projected.last_cycle, peer.last_cycle.as_ref());
        }
    }
    take_latest_cycle(
        &mut projected.last_cycle,
        legacy
            .last_cycle
            .as_ref()
            .filter(|cycle| members.contains(&cycle.peer_node_id)),
    );
    if let Some(network) = stored.networks.get(namespace_id) {
        projected.active_cycle = network
            .active_cycle
            .as_ref()
            .filter(|cycle| members.contains(&cycle.peer_node_id))
            .cloned();
        for (id, scoped_peer) in &network.peers {
            if !members.contains(id) {
                continue;
            }
            add_rolling_totals(&mut projected.rolling, &scoped_peer.rolling);
            let peer = projected.peers.entry(id.clone()).or_default();
            peer.cycles = peer.cycles.saturating_add(scoped_peer.rolling.cycles);
            peer.failures = peer.failures.saturating_add(scoped_peer.rolling.failures);
            peer.direct_bytes = peer
                .direct_bytes
                .saturating_add(scoped_peer.rolling.direct_tx_bytes)
                .saturating_add(scoped_peer.rolling.direct_rx_bytes);
            peer.relay_bytes = peer
                .relay_bytes
                .saturating_add(scoped_peer.rolling.relay_tx_bytes)
                .saturating_add(scoped_peer.rolling.relay_rx_bytes);
            take_latest_cycle(&mut peer.last_cycle, scoped_peer.last_cycle.as_ref());
            take_latest_cycle(&mut projected.last_cycle, scoped_peer.last_cycle.as_ref());
        }
    } else {
        projected.active_cycle = legacy
            .active_cycle
            .as_ref()
            .filter(|cycle| members.contains(&cycle.peer_node_id))
            .cloned();
    }
    projected
}

fn take_latest_cycle(
    latest: &mut Option<SyncTransportCycleStats>,
    candidate: Option<&SyncTransportCycleStats>,
) {
    let Some(candidate) = candidate else { return };
    if latest
        .as_ref()
        .is_none_or(|old| candidate.completed_at_ms > old.completed_at_ms)
    {
        *latest = Some(candidate.clone());
    }
}

fn select_peer_candidates_for_cycle(
    device_list: &DeviceList,
    self_pubkey_bytes: &[u8; 32],
    peer_health: &PeerHealthState,
    now_ms: u64,
) -> Vec<CandidateWithSort> {
    let mut candidates = Vec::new();
    for entry in device_list
        .devices
        .iter()
        .filter(|device| &device.public_key != self_pubkey_bytes)
    {
        let node_id = match iroh::PublicKey::from_bytes(&entry.public_key) {
            Ok(pk) => pk,
            Err(e) => {
                warn!(error = %e, "invalid peer public key in device_list, skipping");
                continue;
            }
        };
        let node_id_string = node_id.to_string();
        let health = peer_health
            .peers
            .get(&node_id_string)
            .cloned()
            .unwrap_or_default();
        candidates.push(CandidateWithSort {
            candidate: PeerCandidate {
                node_id,
                node_id_string,
                sync_timeout: candidate_sync_timeout(&health),
            },
            last_seen_or_added_ms: entry.last_seen_ms.max(entry.added_at_ms),
            last_success_ms: health.last_success_ms.unwrap_or(0),
            cooldown_until_ms: peer_cooldown_until_ms(&health, now_ms),
        });
    }

    candidates.sort_by_key(|candidate| {
        (
            candidate.cooldown_until_ms.is_some(),
            Reverse(candidate.last_success_ms),
            Reverse(candidate.last_seen_or_added_ms),
        )
    });

    let available = candidates
        .iter()
        .filter(|candidate| candidate.cooldown_until_ms.is_none())
        .cloned()
        .collect::<Vec<_>>();
    if !available.is_empty() {
        return available;
    }

    candidates
}

fn peer_candidates_for_all_devices(
    device_list: &DeviceList,
    self_pubkey_bytes: &[u8; 32],
    peer_health: &PeerHealthState,
) -> Vec<PeerCandidate> {
    let mut candidates = Vec::new();
    for entry in device_list
        .devices
        .iter()
        .filter(|device| &device.public_key != self_pubkey_bytes)
    {
        let node_id = match iroh::PublicKey::from_bytes(&entry.public_key) {
            Ok(pk) => pk,
            Err(e) => {
                warn!(error = %e, "invalid peer public key in device_list fanout, skipping");
                continue;
            }
        };
        let node_id_string = node_id.to_string();
        let health = peer_health
            .peers
            .get(&node_id_string)
            .cloned()
            .unwrap_or_default();
        candidates.push(PeerCandidate {
            node_id,
            node_id_string,
            sync_timeout: candidate_sync_timeout(&health),
        });
    }
    candidates
}

fn parse_device_id_hex(value: &str) -> Result<[u8; 8], TargetPeerSelectionError> {
    let bytes = hex::decode(value)
        .map_err(|_| TargetPeerSelectionError::InvalidDeviceId(value.to_string()))?;
    bytes
        .try_into()
        .map_err(|_| TargetPeerSelectionError::InvalidDeviceId(value.to_string()))
}

fn peer_cooldown_until_ms(entry: &PeerHealthEntry, now_ms: u64) -> Option<u64> {
    if entry.consecutive_failures == 0 {
        return None;
    }
    let last_failure_ms = entry.last_failure_ms?;
    let exponent = entry.consecutive_failures.saturating_sub(1).min(3);
    let delay_ms = PEER_FAILURE_COOLDOWN_BASE_MS
        .saturating_mul(1_u64 << exponent)
        .min(PEER_FAILURE_COOLDOWN_MAX_MS);
    let cooldown_until = last_failure_ms.saturating_add(delay_ms);
    (cooldown_until > now_ms).then_some(cooldown_until)
}

fn candidate_sync_timeout(entry: &PeerHealthEntry) -> Duration {
    if entry.last_success_ms.is_none() && entry.consecutive_failures > 0 {
        UNPROVEN_FAILED_PEER_SYNC_TIMEOUT
    } else {
        PEER_SYNC_TIMEOUT
    }
}

fn load_peer_health_file(path: &Path) -> PeerHealthState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn save_peer_health_file(path: &Path, peer_health: &PeerHealthState) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(peer_health)
        .map_err(|e| format!("serialize peer health: {e}"))?;
    atomic_write(path, &bytes).map_err(|e| format!("write peer health: {e}"))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp_path = tmp_path_for(path);
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp_path, path)?;
    sync_parent_dir(parent)?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir(parent: &Path) -> std::io::Result<()> {
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_dir(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("peer_health.json");
    path.with_file_name(format!(
        ".{file_name}.tmp.{}.{}",
        std::process::id(),
        now_ms()
    ))
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
