use std::collections::BTreeMap;

use rua_server::sync_cycle::{
    all_peer_candidates_for_device_list_fanout, clear_transport_active, load_transport_stats,
    ordered_available_peer_node_ids_for_cycle, peer_candidate_for_target_device_id,
    peer_health_diagnostics_for_cycle, record_transport_active, record_transport_cycle,
    PeerHealthEntry, PeerHealthState, SyncTransportCycleStats, SyncTransportCycleStatus,
    TargetPeerSelectionError,
};
use rua_sync::device_list::{DeviceEntry, DeviceList};
use tempfile::TempDir;

const DESKTOP: [u8; 32] = [
    26, 124, 149, 21, 50, 46, 9, 21, 27, 245, 69, 23, 251, 139, 45, 17, 14, 28, 42, 239, 140, 7,
    181, 106, 168, 249, 104, 107, 176, 193, 0, 238,
];

#[tokio::test]
async fn sync_cycle_deadline_bounds_queue_wait() {
    use rua_server::sync_cycle::{acquire_sync_cycle_gate, SyncCycleDeadline};
    let gate = tokio::sync::Mutex::new(());
    let owner = gate.lock().await;
    let deadline = SyncCycleDeadline::starting_now(std::time::Duration::from_millis(40));
    let started = tokio::time::Instant::now();
    let error = acquire_sync_cycle_gate(&gate, deadline).await.unwrap_err();
    assert!(error.starts_with("sync_cycle_queue_timeout"));
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    drop(owner);
    assert!(acquire_sync_cycle_gate(&gate, deadline).await.is_err());
}

#[tokio::test]
async fn sync_cycle_deadline_preserves_only_remaining_transport_budget() {
    use rua_server::sync_cycle::{acquire_sync_cycle_gate, SyncCycleDeadline};
    let gate = tokio::sync::Mutex::new(());
    let owner = gate.lock().await;
    let deadline = SyncCycleDeadline::starting_now(std::time::Duration::from_millis(200));
    let release = async {
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        drop(owner);
    };
    let (guard, ()) = tokio::join!(acquire_sync_cycle_gate(&gate, deadline), release);
    let _guard = guard.expect("released gate");
    assert!(deadline.remaining() <= std::time::Duration::from_millis(160));
    assert!(!deadline.remaining().is_zero());
}
const STALE_PEER: [u8; 32] = [
    107, 21, 83, 221, 241, 240, 164, 239, 75, 120, 70, 157, 170, 145, 220, 224, 228, 143, 8, 147,
    207, 32, 122, 140, 203, 67, 106, 57, 176, 37, 31, 245,
];
const EMULATOR_PEER: [u8; 32] = [
    247, 24, 248, 102, 181, 103, 91, 88, 76, 140, 240, 190, 9, 222, 27, 114, 151, 168, 39, 219, 70,
    96, 178, 54, 141, 93, 26, 136, 180, 217, 191, 94,
];

#[test]
fn peer_selection_prefers_newer_peer_without_health() {
    let device_list = device_list();

    let ordered = ordered_available_peer_node_ids_for_cycle(
        &device_list,
        &DESKTOP,
        &PeerHealthState::default(),
        1_000_000,
    );

    assert_eq!(ordered[0], node_id_string(EMULATOR_PEER));
    assert_eq!(ordered[1], node_id_string(STALE_PEER));
}

#[test]
fn peer_selection_skips_peer_in_failure_cooldown() {
    let device_list = device_list();
    let mut health = PeerHealthState {
        peers: BTreeMap::new(),
    };
    health.peers.insert(
        node_id_string(STALE_PEER),
        PeerHealthEntry {
            consecutive_failures: 1,
            last_failure_ms: Some(10_000),
            last_success_ms: Some(9_000),
        },
    );

    let ordered =
        ordered_available_peer_node_ids_for_cycle(&device_list, &DESKTOP, &health, 20_000);

    assert_eq!(ordered, vec![node_id_string(EMULATOR_PEER)]);
}

#[test]
fn peer_selection_falls_back_to_cooldown_peers_when_all_peers_are_cooling_down() {
    let device_list = device_list();
    let mut health = PeerHealthState {
        peers: BTreeMap::new(),
    };
    health.peers.insert(
        node_id_string(STALE_PEER),
        PeerHealthEntry {
            consecutive_failures: 1,
            last_failure_ms: Some(10_000),
            last_success_ms: Some(11_000),
        },
    );
    health.peers.insert(
        node_id_string(EMULATOR_PEER),
        PeerHealthEntry {
            consecutive_failures: 2,
            last_failure_ms: Some(10_000),
            last_success_ms: Some(12_000),
        },
    );

    let ordered =
        ordered_available_peer_node_ids_for_cycle(&device_list, &DESKTOP, &health, 20_000);

    assert_eq!(
        ordered,
        vec![node_id_string(EMULATOR_PEER), node_id_string(STALE_PEER)],
        "manual sync should still try the best known peer when every peer is cooling down"
    );
}

#[test]
fn targeted_peer_selection_uses_requested_device_not_best_available_peer() {
    let device_list = device_list();
    let mut health = PeerHealthState {
        peers: BTreeMap::new(),
    };
    health.peers.insert(
        node_id_string(STALE_PEER),
        PeerHealthEntry {
            consecutive_failures: 2,
            last_failure_ms: Some(10_000),
            last_success_ms: Some(9_000),
        },
    );

    let target_device_id = hex::encode(&STALE_PEER[..8]);
    let candidate =
        peer_candidate_for_target_device_id(&device_list, &DESKTOP, &health, &target_device_id)
            .expect("target candidate");

    assert_eq!(candidate.node_id_string, node_id_string(STALE_PEER));
    assert_eq!(
        candidate.sync_timeout.as_millis(),
        30_000,
        "targeted sync should use the target peer's normal timeout when it has proven success"
    );
}

#[test]
fn targeted_peer_selection_rejects_self_device() {
    let device_list = device_list();
    let target_device_id = hex::encode(&DESKTOP[..8]);
    let err = peer_candidate_for_target_device_id(
        &device_list,
        &DESKTOP,
        &PeerHealthState::default(),
        &target_device_id,
    )
    .expect_err("self target must be rejected");

    assert_eq!(
        err,
        TargetPeerSelectionError::TargetIsSelf(target_device_id)
    );
}

#[test]
fn targeted_peer_selection_rejects_unknown_device() {
    let device_list = device_list();
    let target_device_id = "0001020304050607";
    let err = peer_candidate_for_target_device_id(
        &device_list,
        &DESKTOP,
        &PeerHealthState::default(),
        target_device_id,
    )
    .expect_err("missing target must be rejected");

    assert_eq!(
        err,
        TargetPeerSelectionError::TargetNotFound(target_device_id.to_string())
    );
}

#[test]
fn device_list_fanout_targets_all_known_peers_even_when_some_are_in_cooldown() {
    let device_list = device_list();
    let mut health = PeerHealthState {
        peers: BTreeMap::new(),
    };
    health.peers.insert(
        node_id_string(STALE_PEER),
        PeerHealthEntry {
            consecutive_failures: 2,
            last_failure_ms: Some(10_000),
            last_success_ms: Some(9_000),
        },
    );

    let candidates = all_peer_candidates_for_device_list_fanout(&device_list, &DESKTOP, &health);
    let node_ids = candidates
        .iter()
        .map(|candidate| candidate.node_id_string.clone())
        .collect::<Vec<_>>();

    assert_eq!(node_ids.len(), 2);
    assert!(node_ids.contains(&node_id_string(STALE_PEER)));
    assert!(node_ids.contains(&node_id_string(EMULATOR_PEER)));
}

#[test]
fn peer_diagnostics_exposes_selectable_and_cooldown_peers() {
    let device_list = device_list();
    let mut health = PeerHealthState {
        peers: BTreeMap::new(),
    };
    health.peers.insert(
        node_id_string(STALE_PEER),
        PeerHealthEntry {
            consecutive_failures: 2,
            last_failure_ms: Some(10_000),
            last_success_ms: Some(9_000),
        },
    );
    health.peers.insert(
        node_id_string(EMULATOR_PEER),
        PeerHealthEntry {
            consecutive_failures: 0,
            last_failure_ms: None,
            last_success_ms: Some(12_000),
        },
    );

    let diagnostics = peer_health_diagnostics_for_cycle(&device_list, &DESKTOP, &health, 20_000);

    assert_eq!(diagnostics.total_peers, 2);
    assert_eq!(diagnostics.selectable_peers, 1);
    assert_eq!(diagnostics.peers_in_cooldown, 1);
    assert_eq!(
        diagnostics.next_peer_node_id,
        Some(node_id_string(EMULATOR_PEER))
    );
    assert_eq!(diagnostics.peers[0].node_id, node_id_string(EMULATOR_PEER));
    assert!(diagnostics.peers[0].selectable);
    assert_eq!(diagnostics.peers[0].sync_timeout_ms, 30_000);
    assert_eq!(diagnostics.peers[1].node_id, node_id_string(STALE_PEER));
    assert!(!diagnostics.peers[1].selectable);
    assert_eq!(diagnostics.peers[1].cooldown_until_ms, Some(130_000));
    assert_eq!(diagnostics.peers[1].sync_timeout_ms, 30_000);
}

#[test]
fn peer_diagnostics_keeps_short_timeout_after_cooldown_for_unproven_failed_peer() {
    let device_list = device_list();
    let mut health = PeerHealthState {
        peers: BTreeMap::new(),
    };
    health.peers.insert(
        node_id_string(STALE_PEER),
        PeerHealthEntry {
            consecutive_failures: 3,
            last_failure_ms: Some(10_000),
            last_success_ms: None,
        },
    );

    let diagnostics = peer_health_diagnostics_for_cycle(&device_list, &DESKTOP, &health, 1_000_000);

    let stale = diagnostics
        .peers
        .iter()
        .find(|peer| peer.node_id == node_id_string(STALE_PEER))
        .expect("stale peer diagnostics");
    assert!(
        stale.selectable,
        "cooldown has expired, peer remains a fallback candidate"
    );
    assert_eq!(
        stale.sync_timeout_ms, 5_000,
        "unproven failed peer must not stall a cycle for the full peer timeout"
    );
}

#[test]
fn transport_stats_persist_and_aggregate_route_bytes_by_peer() {
    let tmp = TempDir::new().expect("tempdir");
    let peer = node_id_string(EMULATOR_PEER);

    record_transport_cycle(
        tmp.path(),
        SyncTransportCycleStats {
            completed_at_ms: 10_000,
            peer_node_id: peer.clone(),
            status: SyncTransportCycleStatus::Success,
            error: None,
            failure_kind: None,
            events_received: 7,
            events_sent: 3,
            connect_ms: 12,
            process_ms: 34,
            duration_ms: 46,
            route: rua_sync::transport::SyncRouteKind::Mixed,
            direct_tx_bytes: 100,
            direct_rx_bytes: 200,
            relay_tx_bytes: 30,
            relay_rx_bytes: 40,
            direct_rtt_ms: Some(18),
            relay_rtt_ms: Some(80),
            warnings: Vec::new(),
        },
    );
    record_transport_cycle(
        tmp.path(),
        SyncTransportCycleStats {
            completed_at_ms: 11_000,
            peer_node_id: peer.clone(),
            status: SyncTransportCycleStatus::Failure,
            error: Some("timeout".to_string()),
            failure_kind: Some(rua_server::sync_peer_directory::SyncTransportFailureKind::Internal),
            events_received: 0,
            events_sent: 0,
            connect_ms: 0,
            process_ms: 0,
            duration_ms: 5_000,
            route: rua_sync::transport::SyncRouteKind::Unknown,
            direct_tx_bytes: 0,
            direct_rx_bytes: 0,
            relay_tx_bytes: 0,
            relay_rx_bytes: 0,
            direct_rtt_ms: None,
            relay_rtt_ms: None,
            warnings: Vec::new(),
        },
    );

    let stats = load_transport_stats(tmp.path());
    assert_eq!(stats.active_cycle, None);
    assert_eq!(stats.rolling.cycles, 2);
    assert_eq!(stats.rolling.failures, 1);
    assert_eq!(stats.rolling.events_received, 7);
    assert_eq!(stats.rolling.events_sent, 3);
    assert_eq!(stats.rolling.direct_tx_bytes, 100);
    assert_eq!(stats.rolling.direct_rx_bytes, 200);
    assert_eq!(stats.rolling.relay_tx_bytes, 30);
    assert_eq!(stats.rolling.relay_rx_bytes, 40);
    assert_eq!(
        stats.last_cycle.as_ref().unwrap().status,
        SyncTransportCycleStatus::Failure
    );

    let peer_stats = stats.peers.get(&peer).expect("peer stats");
    assert_eq!(peer_stats.cycles, 2);
    assert_eq!(peer_stats.failures, 1);
    assert_eq!(peer_stats.direct_bytes, 300);
    assert_eq!(peer_stats.relay_bytes, 70);
    assert_eq!(
        peer_stats.last_cycle.as_ref().unwrap().error.as_deref(),
        Some("timeout")
    );
}

#[test]
fn clear_active_transport_preserves_last_measured_cycle() {
    let tmp = TempDir::new().expect("tempdir");
    let peer = node_id_string(EMULATOR_PEER);
    record_transport_cycle(
        tmp.path(),
        SyncTransportCycleStats {
            completed_at_ms: 10_000,
            peer_node_id: peer.clone(),
            status: SyncTransportCycleStatus::Success,
            error: None,
            failure_kind: None,
            events_received: 1,
            events_sent: 2,
            connect_ms: 3,
            process_ms: 4,
            duration_ms: 5,
            route: rua_sync::transport::SyncRouteKind::Direct,
            direct_tx_bytes: 100,
            direct_rx_bytes: 200,
            relay_tx_bytes: 0,
            relay_rx_bytes: 0,
            direct_rtt_ms: Some(12),
            relay_rtt_ms: None,
            warnings: Vec::new(),
        },
    );
    record_transport_active(tmp.path(), &peer, 11_000);

    clear_transport_active(tmp.path(), &peer);

    let stats = load_transport_stats(tmp.path());
    assert_eq!(stats.active_cycle, None);
    assert_eq!(stats.rolling.cycles, 1);
    assert_eq!(
        stats.last_cycle.as_ref().unwrap().route,
        rua_sync::transport::SyncRouteKind::Direct
    );
    assert_eq!(stats.last_cycle.as_ref().unwrap().direct_bytes(), 300);
}

#[test]
fn peer_directory_owns_peer_selection_and_transport_state_types() {
    let sync_cycle_src = include_str!("../../src/sync_cycle.rs");
    let peer_directory_src = include_str!("../../src/sync_peer_directory.rs");

    assert!(
        !sync_cycle_src.contains("pub struct PeerHealthState")
            && !sync_cycle_src.contains("pub struct PeerHealthDiagnostics")
            && !sync_cycle_src.contains("pub struct SyncTransportStatsState")
            && sync_cycle_src.contains("sync_peer_directory"),
        "Sync Cycle should delegate peer directory state and diagnostics"
    );
    assert!(
        peer_directory_src.contains("pub struct PeerHealthState")
            && peer_directory_src.contains("pub struct PeerHealthDiagnostics")
            && peer_directory_src.contains("pub struct SyncTransportStatsState"),
        "Peer Directory should own peer health, diagnostics, and transport stats types"
    );
}

#[test]
fn transport_stats_namespace_and_membership_projection_preserves_all_attributed_metrics() {
    use rua_server::sync_peer_directory::{
        load_peer_health_diagnostics_with_transport, load_transport_stats_for_network,
        record_transport_cycle_for_network,
    };

    let tmp = TempDir::new().unwrap();
    let current = node_id_string(EMULATOR_PEER);
    let foreign = node_id_string(STALE_PEER);
    let control = rua_sync::control_store::SyncControlStore::from_sync_dir(tmp.path());
    let mut members = DeviceList {
        devices: vec![device(DESKTOP, 1), device(EMULATOR_PEER, 2)],
        ..Default::default()
    };
    control.write_device_list(&members).unwrap();
    let first = measured_transport_cycle(&current, 100, [7, 3, 100, 200, 30, 40], false);
    let unrelated = measured_transport_cycle(&foreign, 200, [1_000; 6], false);
    let mut other_network = measured_transport_cycle(&current, 300, [11, 13, 0, 0, 23, 29], true);
    other_network.route = rua_sync::transport::SyncRouteKind::Relay;
    record_transport_cycle_for_network(tmp.path(), "network-a", first.clone());
    record_transport_cycle_for_network(tmp.path(), "network-a", unrelated);
    record_transport_cycle_for_network(tmp.path(), "network-b", other_network.clone());

    let projected = load_transport_stats_for_network(tmp.path(), Some("network-a"), &DESKTOP);
    assert_eq!(projected.rolling.cycles, 1);
    assert_eq!(projected.rolling.failures, 0);
    assert_eq!(projected.rolling.events_received, 7);
    assert_eq!(projected.rolling.events_sent, 3);
    assert_eq!(projected.rolling.direct_tx_bytes, 100);
    assert_eq!(projected.rolling.direct_rx_bytes, 200);
    assert_eq!(projected.rolling.relay_tx_bytes, 30);
    assert_eq!(projected.rolling.relay_rx_bytes, 40);
    assert_eq!(projected.peers.len(), 1);
    assert_eq!(projected.peers[&current].direct_bytes, 300);
    assert_eq!(projected.peers[&current].relay_bytes, 70);
    assert_eq!(projected.last_cycle, Some(first));
    let health = load_peer_health_diagnostics_with_transport(tmp.path(), &DESKTOP, &projected);
    let health_peer = health
        .peers
        .iter()
        .find(|peer| peer.node_id == current)
        .unwrap();
    assert_eq!(
        health_peer.last_route,
        Some(rua_sync::transport::SyncRouteKind::Mixed)
    );
    assert_eq!(health_peer.last_direct_bytes, 300);
    assert_eq!(health_peer.last_relay_bytes, 70);
    let other = load_transport_stats_for_network(tmp.path(), Some("network-b"), &DESKTOP);
    assert_eq!(other.rolling.cycles, 1);
    assert_eq!(other.rolling.failures, 1);
    assert_eq!(other.rolling.events_received, 11);
    assert_eq!(other.rolling.events_sent, 13);
    assert_eq!(other.rolling.direct_tx_bytes, 0);
    assert_eq!(other.rolling.direct_rx_bytes, 0);
    assert_eq!(other.rolling.relay_tx_bytes, 23);
    assert_eq!(other.rolling.relay_rx_bytes, 29);
    assert_eq!(other.last_cycle, Some(other_network));
    assert_eq!(load_transport_stats(tmp.path()).rolling.cycles, 3);

    members
        .devices
        .retain(|entry| entry.public_key != EMULATOR_PEER);
    control.write_device_list(&members).unwrap();
    assert_eq!(
        load_transport_stats_for_network(tmp.path(), Some("network-a"), &DESKTOP)
            .rolling
            .cycles,
        0
    );
    members.devices.push(device(EMULATOR_PEER, 2));
    control.write_device_list(&members).unwrap();
    assert_eq!(
        load_transport_stats_for_network(tmp.path(), Some("network-a"), &DESKTOP)
            .rolling
            .cycles,
        1
    );
    assert_eq!(
        load_transport_stats_for_network(tmp.path(), None, &DESKTOP)
            .rolling
            .cycles,
        0
    );
}

#[test]
fn transport_stats_preserves_legacy_evidence_and_current_peer_history_on_scoped_writes() {
    use rua_server::sync_peer_directory::{
        load_transport_stats_for_network, record_transport_cycle_for_network,
    };

    let tmp = TempDir::new().unwrap();
    let current = node_id_string(EMULATOR_PEER);
    let unrelated = node_id_string(STALE_PEER);
    rua_sync::control_store::SyncControlStore::from_sync_dir(tmp.path())
        .write_device_list(&DeviceList {
            devices: vec![device(DESKTOP, 1), device(EMULATOR_PEER, 2)],
            ..Default::default()
        })
        .unwrap();
    let legacy = serde_json::json!({
        "rolling": {
            "cycles": 26_802, "failures": 13_601,
            "events_received": 900, "events_sent": 800,
            "direct_tx_bytes": 10_000, "direct_rx_bytes": 20_000,
            "relay_tx_bytes": 30_000, "relay_rx_bytes": 40_000
        },
        "peers": {
            current.clone(): {"cycles": 2, "failures": 1, "direct_bytes": 300, "relay_bytes": 30},
            unrelated.clone(): {"cycles": 26_800, "failures": 13_600, "direct_bytes": 29_700, "relay_bytes": 69_970}
        },
        "future_stats_metadata": {"kept": true}
    });
    let path = tmp.path().join("transport_stats.json");
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    record_transport_cycle_for_network(
        tmp.path(),
        "network-a",
        measured_transport_cycle(&current, 100, [7, 3, 100, 200, 30, 40], false),
    );
    let projected = load_transport_stats_for_network(tmp.path(), Some("network-a"), &DESKTOP);
    assert_eq!(projected.rolling.cycles, 3);
    assert_eq!(projected.rolling.failures, 1);
    assert_eq!(projected.rolling.events_received, 7);
    assert_eq!(projected.rolling.events_sent, 3);
    assert_eq!(projected.rolling.direct_tx_bytes, 100);
    assert_eq!(projected.rolling.direct_rx_bytes, 200);
    assert_eq!(projected.rolling.relay_tx_bytes, 30);
    assert_eq!(projected.rolling.relay_rx_bytes, 40);
    assert_eq!(projected.peers[&current].cycles, 3);
    assert_eq!(projected.peers[&current].direct_bytes, 600);
    assert_eq!(projected.peers[&current].relay_bytes, 100);
    let retained: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(retained["rolling"]["cycles"], 26_803);
    assert_eq!(retained["legacy_unscoped"]["rolling"], legacy["rolling"]);
    assert_eq!(
        retained["legacy_unscoped"]["peers"][&unrelated],
        legacy["peers"][&unrelated]
    );
    assert_eq!(
        retained["future_stats_metadata"],
        legacy["future_stats_metadata"]
    );
}

#[test]
fn transport_stats_scoped_clear_does_not_restore_a_legacy_active_cycle() {
    use rua_server::sync_peer_directory::{
        clear_transport_active_for_network, load_transport_stats_for_network,
    };

    let tmp = TempDir::new().unwrap();
    let peer = node_id_string(EMULATOR_PEER);
    rua_sync::control_store::SyncControlStore::from_sync_dir(tmp.path())
        .write_device_list(&DeviceList {
            devices: vec![device(EMULATOR_PEER, 1)],
            ..Default::default()
        })
        .unwrap();
    std::fs::write(
        tmp.path().join("transport_stats.json"),
        serde_json::to_vec(&serde_json::json!({
            "active_cycle": {"started_at_ms": 100, "peer_node_id": peer.clone()}
        }))
        .unwrap(),
    )
    .unwrap();
    clear_transport_active_for_network(tmp.path(), "network-a", &peer);

    assert_eq!(load_transport_stats(tmp.path()).active_cycle, None);
    assert_eq!(
        load_transport_stats_for_network(tmp.path(), Some("network-a"), &DESKTOP).active_cycle,
        None
    );
    let retained: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tmp.path().join("transport_stats.json")).unwrap())
            .unwrap();
    assert_eq!(
        retained["legacy_unscoped"]["active_cycle"]["peer_node_id"],
        peer
    );
}

fn measured_transport_cycle(
    peer: &str,
    at: u64,
    metrics: [u64; 6],
    failed: bool,
) -> SyncTransportCycleStats {
    SyncTransportCycleStats {
        completed_at_ms: at,
        peer_node_id: peer.to_owned(),
        status: if failed {
            SyncTransportCycleStatus::Failure
        } else {
            SyncTransportCycleStatus::Success
        },
        error: failed.then(|| "timeout".to_owned()),
        failure_kind: failed
            .then_some(rua_server::sync_peer_directory::SyncTransportFailureKind::Internal),
        events_received: metrics[0] as usize,
        events_sent: metrics[1] as usize,
        connect_ms: 1,
        process_ms: 2,
        duration_ms: 3,
        route: rua_sync::transport::SyncRouteKind::Mixed,
        direct_tx_bytes: metrics[2],
        direct_rx_bytes: metrics[3],
        relay_tx_bytes: metrics[4],
        relay_rx_bytes: metrics[5],
        direct_rtt_ms: Some(4),
        relay_rtt_ms: Some(5),
        warnings: Vec::new(),
    }
}

fn device_list() -> DeviceList {
    DeviceList {
        devices: vec![
            device(DESKTOP, 3_000),
            device(STALE_PEER, 1_000),
            device(EMULATOR_PEER, 2_000),
        ],
        ..Default::default()
    }
}

fn device(public_key: [u8; 32], last_seen_ms: u64) -> DeviceEntry {
    DeviceEntry {
        device_id: public_key[..8].try_into().unwrap(),
        public_key,
        label: String::new(),
        friendly_name: String::new(),
        added_at_ms: last_seen_ms,
        last_seen_ms,
        direct_addrs: Vec::new(),
    }
}

fn node_id_string(public_key: [u8; 32]) -> String {
    iroh::PublicKey::from_bytes(&public_key)
        .unwrap()
        .to_string()
}
