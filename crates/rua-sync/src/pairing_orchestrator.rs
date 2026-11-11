use std::path::Path;

use iroh::Endpoint;
use serde::{Deserialize, Serialize};

use crate::device_list::{DeviceEntry, DeviceList};
use crate::identity::{DeviceIdentity, NetworkSecret};
use crate::pairing::NetworkState;
use crate::wormhole::WormholeChannel;
use crate::SyncError;

#[derive(Debug, Serialize, Deserialize)]
struct PairingPayload {
    network_state: NetworkState,
    device_entry: DeviceEntry,
}

#[derive(Debug, Serialize, Deserialize)]
struct DocTicketMsg {
    namespace_secret_bytes: [u8; 32],
}

#[derive(Debug, PartialEq, Eq)]
enum PairingAction {
    CreateNetwork,
    ReceiveNetwork,
    ShareNetwork,
    JoinNetwork,
    AddPeers,
    MergeNeeded,
}

pub enum PairingOutcome {
    Joined {
        network_secret: Box<NetworkSecret>,
        device_list: DeviceList,
    },

    Shared {
        device_list: DeviceList,
    },

    PeerAdded {
        device_list: DeviceList,
    },

    MergeNeeded {
        device_list: DeviceList,
        peer_namespace_secret: Box<NetworkSecret>,
    },
}

fn determine_action(
    my_state: &NetworkState,
    peer_state: &NetworkState,
    is_listener: bool,
) -> PairingAction {
    match (my_state.has_network, peer_state.has_network) {
        (false, false) => {
            if is_listener {
                PairingAction::CreateNetwork
            } else {
                PairingAction::ReceiveNetwork
            }
        }
        (true, false) => PairingAction::ShareNetwork,
        (false, true) => PairingAction::JoinNetwork,
        (true, true) => {
            if my_state.namespace_id == peer_state.namespace_id {
                PairingAction::AddPeers
            } else {
                PairingAction::MergeNeeded
            }
        }
    }
}

fn build_device_entry(device: &DeviceIdentity) -> DeviceEntry {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    DeviceEntry {
        device_id: device.device_id_short(),
        public_key: device.public_key().to_bytes(),
        label: String::new(),
        friendly_name: crate::device_list::default_friendly_name(
            std::env::consts::OS,
            &device.public_key().to_bytes(),
        ),
        added_at_ms: now_ms,
        last_seen_ms: now_ms,
        direct_addrs: Vec::new(),
    }
}

fn build_device_list(
    current_list: &DeviceList,
    my_entry: &DeviceEntry,
    peer_entry: &DeviceEntry,
) -> DeviceList {
    let mut list = current_list.clone();
    if !list.contains_device(&my_entry.device_id) {
        list.add_device(my_entry.clone());
    }
    list.add_device(peer_entry.clone());
    list
}

pub async fn run_pairing_listener(
    pin_code: &str,
    network_secret: Option<&NetworkSecret>,
    current_device: &DeviceIdentity,
    current_device_list: &DeviceList,
    sync_dir: &Path,
    created_at_ms: Option<u64>,
) -> Result<PairingOutcome, SyncError> {
    const MAX_ATTEMPTS: u32 = 3;

    let endpoint = WormholeChannel::create_initiator_endpoint(pin_code).await?;

    tracing::debug!(
        node_id = %endpoint.id().fmt_short(),
        "pairing listener endpoint created, waiting for connector"
    );

    let my_entry = current_device_list
        .devices
        .iter()
        .find(|entry| entry.device_id == current_device.device_id_short())
        .cloned()
        .unwrap_or_else(|| build_device_entry(current_device));
    let my_state = build_network_state(network_secret, created_at_ms);

    let mut last_err = None;
    for attempt in 1..=MAX_ATTEMPTS {
        match try_pairing_listener_exchange(
            &endpoint,
            pin_code,
            &my_state,
            &my_entry,
            network_secret,
            current_device_list,
            sync_dir,
        )
        .await
        {
            Ok(outcome) => {
                endpoint.close().await;
                return Ok(outcome);
            }
            Err(e) => {
                if attempt < MAX_ATTEMPTS {
                    tracing::warn!(
                        attempt,
                        max = MAX_ATTEMPTS,
                        "[pairing] listener attempt {attempt}/{MAX_ATTEMPTS} failed: {e}, retrying"
                    );
                } else {
                    tracing::error!(
                        "[pairing] listener attempt {attempt}/{MAX_ATTEMPTS} failed: {e}, giving up"
                    );
                }
                last_err = Some(e);
            }
        }
    }

    endpoint.close().await;
    Err(last_err.unwrap())
}

async fn try_pairing_listener_exchange(
    endpoint: &Endpoint,
    pin_code: &str,
    my_state: &NetworkState,
    my_entry: &DeviceEntry,
    network_secret: Option<&NetworkSecret>,
    current_device_list: &DeviceList,
    sync_dir: &Path,
) -> Result<PairingOutcome, SyncError> {
    let mut channel = WormholeChannel::accept_and_handshake(endpoint, pin_code).await?;

    let outcome = symmetric_exchange(
        &mut channel,
        true,
        my_state,
        my_entry,
        network_secret,
        current_device_list,
        sync_dir,
    )
    .await;

    let _ = channel.close().await;
    outcome
}

pub async fn run_pairing_connector(
    pin_code: &str,
    network_secret: Option<&NetworkSecret>,
    current_device: &DeviceIdentity,
    current_device_list: &DeviceList,
    sync_dir: &Path,
    created_at_ms: Option<u64>,
) -> Result<PairingOutcome, SyncError> {
    const MAX_ATTEMPTS: u32 = 3;

    let my_entry = current_device_list
        .devices
        .iter()
        .find(|entry| entry.device_id == current_device.device_id_short())
        .cloned()
        .unwrap_or_else(|| build_device_entry(current_device));
    let my_state = build_network_state(network_secret, created_at_ms);

    let mut last_err = None;
    for attempt in 1..=MAX_ATTEMPTS {
        match try_pairing_connector_exchange(
            pin_code,
            &my_state,
            &my_entry,
            network_secret,
            current_device_list,
            sync_dir,
        )
        .await
        {
            Ok(outcome) => return Ok(outcome),
            Err(e) => {
                if attempt < MAX_ATTEMPTS {
                    tracing::warn!(
                        attempt,
                        max = MAX_ATTEMPTS,
                        "[pairing] connector attempt {attempt}/{MAX_ATTEMPTS} failed: {e}, retrying in 2s"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                } else {
                    tracing::error!(
                        "[pairing] connector attempt {attempt}/{MAX_ATTEMPTS} failed: {e}, giving up"
                    );
                }
                last_err = Some(e);
            }
        }
    }

    Err(last_err.unwrap())
}

async fn try_pairing_connector_exchange(
    pin_code: &str,
    my_state: &NetworkState,
    my_entry: &DeviceEntry,
    network_secret: Option<&NetworkSecret>,
    current_device_list: &DeviceList,
    sync_dir: &Path,
) -> Result<PairingOutcome, SyncError> {
    let mut channel = WormholeChannel::connect_to_listener(pin_code).await?;

    let outcome = symmetric_exchange(
        &mut channel,
        false,
        my_state,
        my_entry,
        network_secret,
        current_device_list,
        sync_dir,
    )
    .await;

    let _ = channel.close().await;
    outcome
}

fn build_network_state(
    network_secret: Option<&NetworkSecret>,
    created_at_ms: Option<u64>,
) -> NetworkState {
    match network_secret {
        Some(ns) => NetworkState {
            has_network: true,
            namespace_id: Some(*ns.namespace_id().as_bytes()),
            created_at_ms,
        },
        None => NetworkState {
            has_network: false,
            namespace_id: None,
            created_at_ms: None,
        },
    }
}

async fn symmetric_exchange(
    channel: &mut WormholeChannel,
    is_listener: bool,
    my_state: &NetworkState,
    my_entry: &DeviceEntry,
    network_secret: Option<&NetworkSecret>,
    current_device_list: &DeviceList,
    sync_dir: &Path,
) -> Result<PairingOutcome, SyncError> {
    let my_payload = PairingPayload {
        network_state: my_state.clone(),
        device_entry: my_entry.clone(),
    };
    let my_bytes = serde_json::to_vec(&my_payload)
        .map_err(|e| SyncError::Pairing(format!("serialize payload: {e}")))?;

    let peer_payload: PairingPayload = if is_listener {
        channel.send_encrypted(&my_bytes).await?;
        let peer_bytes = channel.recv_encrypted().await?;
        serde_json::from_slice(&peer_bytes)
            .map_err(|e| SyncError::Pairing(format!("deserialize peer payload: {e}")))?
    } else {
        let peer_bytes = channel.recv_encrypted().await?;
        channel.send_encrypted(&my_bytes).await?;
        serde_json::from_slice(&peer_bytes)
            .map_err(|e| SyncError::Pairing(format!("deserialize peer payload: {e}")))?
    };

    let action = determine_action(my_state, &peer_payload.network_state, is_listener);
    tracing::info!("[pairing] determined action: {action:?}");

    let device_list = build_device_list(current_device_list, my_entry, &peer_payload.device_entry);

    match action {
        PairingAction::CreateNetwork => {
            let new_ns = NetworkSecret::generate();
            let doc_ticket = DocTicketMsg {
                namespace_secret_bytes: new_ns.to_bytes(),
            };
            let ticket_bytes = serde_json::to_vec(&doc_ticket)
                .map_err(|e| SyncError::Pairing(format!("serialize DocTicket: {e}")))?;
            channel.send_encrypted(&ticket_bytes).await?;
            new_ns.save(sync_dir)?;
            save_network_created_at(sync_dir);
            Ok(PairingOutcome::Joined {
                network_secret: Box::new(new_ns),
                device_list,
            })
        }
        PairingAction::ShareNetwork => {
            let ns = network_secret.expect("ShareNetwork requires network_secret");
            let doc_ticket = DocTicketMsg {
                namespace_secret_bytes: ns.to_bytes(),
            };
            let ticket_bytes = serde_json::to_vec(&doc_ticket)
                .map_err(|e| SyncError::Pairing(format!("serialize DocTicket: {e}")))?;
            channel.send_encrypted(&ticket_bytes).await?;
            Ok(PairingOutcome::Shared { device_list })
        }
        PairingAction::ReceiveNetwork | PairingAction::JoinNetwork => {
            let ticket_bytes = channel.recv_encrypted().await?;
            let doc_ticket: DocTicketMsg = serde_json::from_slice(&ticket_bytes)
                .map_err(|e| SyncError::Pairing(format!("deserialize DocTicket: {e}")))?;
            let ns = NetworkSecret::from_bytes(&doc_ticket.namespace_secret_bytes);
            ns.save(sync_dir)?;
            Ok(PairingOutcome::Joined {
                network_secret: Box::new(ns),
                device_list,
            })
        }
        PairingAction::AddPeers => Ok(PairingOutcome::PeerAdded { device_list }),
        PairingAction::MergeNeeded => {
            let ns = network_secret.expect("MergeNeeded requires network_secret");
            let doc_ticket = DocTicketMsg {
                namespace_secret_bytes: ns.to_bytes(),
            };
            let ticket_bytes = serde_json::to_vec(&doc_ticket)
                .map_err(|e| SyncError::Pairing(format!("serialize DocTicket: {e}")))?;

            let peer_ticket: DocTicketMsg = if is_listener {
                channel.send_encrypted(&ticket_bytes).await?;
                let peer_bytes = channel.recv_encrypted().await?;
                serde_json::from_slice(&peer_bytes)
                    .map_err(|e| SyncError::Pairing(format!("deserialize peer DocTicket: {e}")))?
            } else {
                let peer_bytes = channel.recv_encrypted().await?;
                channel.send_encrypted(&ticket_bytes).await?;
                serde_json::from_slice(&peer_bytes)
                    .map_err(|e| SyncError::Pairing(format!("deserialize peer DocTicket: {e}")))?
            };

            let peer_ns = NetworkSecret::from_bytes(&peer_ticket.namespace_secret_bytes);
            Ok(PairingOutcome::MergeNeeded {
                device_list,
                peer_namespace_secret: Box::new(peer_ns),
            })
        }
    }
}

fn save_network_created_at(sync_dir: &Path) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let meta = serde_json::json!({ "created_at_ms": now_ms });
    let path = sync_dir.join("network_meta.json");
    if let Err(error) = crate::control_store::write_json_file(&path, &meta) {
        tracing::warn!(error = %error, "[pairing] failed to save network creation metadata");
    }
}

pub fn load_network_created_at(sync_dir: &Path) -> Option<u64> {
    let path = sync_dir.join("network_meta.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get("created_at_ms").and_then(|v| v.as_u64())
}
