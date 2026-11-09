use rua_sync::device_list::{DeviceEntry, DeviceList};
use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use rua_sync::pairing::{NetworkState, PairingPin};
use rua_sync::wormhole;

#[tokio::test]
async fn channel_encrypted_roundtrip() {
    let mut pin = PairingPin::generate();
    let pin_code = pin.code().to_string();
    pin.consume().unwrap();

    let (mut init_ch, mut join_ch) = wormhole::establish_pair_local(&pin_code).await.unwrap();

    init_ch
        .send_encrypted(b"hello from listener")
        .await
        .unwrap();
    let msg = join_ch.recv_encrypted().await.unwrap();
    assert_eq!(msg, b"hello from listener");

    join_ch
        .send_encrypted(b"hello from connector")
        .await
        .unwrap();
    let msg2 = init_ch.recv_encrypted().await.unwrap();
    assert_eq!(msg2, b"hello from connector");

    init_ch.close().await.unwrap();
    join_ch.close().await.unwrap();
}

fn make_entry(device: &DeviceIdentity) -> DeviceEntry {
    DeviceEntry {
        device_id: device.device_id_short(),
        public_key: device.public_key().to_bytes(),
        label: String::new(),
        friendly_name: String::new(),
        added_at_ms: 1000,
        last_seen_ms: 0,
        direct_addrs: vec![],
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PairingPayload {
    network_state: NetworkState,
    device_entry: DeviceEntry,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DocTicketMsg {
    namespace_secret_bytes: [u8; 32],
}

#[tokio::test]
async fn symmetric_both_no_network_exchange() {
    let device_a = DeviceIdentity::generate();
    let device_b = DeviceIdentity::generate();
    let sync_dir_b = tempfile::tempdir().unwrap();

    let mut pin = PairingPin::generate();
    let pin_code = pin.code().to_string();
    pin.consume().unwrap();

    let (mut listener_ch, mut connector_ch) =
        wormhole::establish_pair_local(&pin_code).await.unwrap();

    let state_a = NetworkState {
        has_network: false,
        namespace_id: None,
        created_at_ms: None,
    };
    let state_b = state_a.clone();

    let payload_a = PairingPayload {
        network_state: state_a,
        device_entry: make_entry(&device_a),
    };
    let payload_b = PairingPayload {
        network_state: state_b,
        device_entry: make_entry(&device_b),
    };

    let bytes_a = serde_json::to_vec(&payload_a).unwrap();
    listener_ch.send_encrypted(&bytes_a).await.unwrap();
    let recv_a = connector_ch.recv_encrypted().await.unwrap();
    let _peer_a: PairingPayload = serde_json::from_slice(&recv_a).unwrap();

    let bytes_b = serde_json::to_vec(&payload_b).unwrap();
    connector_ch.send_encrypted(&bytes_b).await.unwrap();
    let recv_b = listener_ch.recv_encrypted().await.unwrap();
    let _peer_b: PairingPayload = serde_json::from_slice(&recv_b).unwrap();

    let new_ns = NetworkSecret::generate();
    let doc_ticket = DocTicketMsg {
        namespace_secret_bytes: new_ns.to_bytes(),
    };
    let ticket_bytes = serde_json::to_vec(&doc_ticket).unwrap();
    listener_ch.send_encrypted(&ticket_bytes).await.unwrap();

    let recv_ticket = connector_ch.recv_encrypted().await.unwrap();
    let recv_doc: DocTicketMsg = serde_json::from_slice(&recv_ticket).unwrap();
    let received_ns = NetworkSecret::from_bytes(&recv_doc.namespace_secret_bytes);
    received_ns.save(sync_dir_b.path()).unwrap();

    listener_ch.close().await.unwrap();
    connector_ch.close().await.unwrap();

    assert_eq!(new_ns.to_bytes(), received_ns.to_bytes());

    let loaded = NetworkSecret::load(sync_dir_b.path()).unwrap();
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().to_bytes(), new_ns.to_bytes());
}

#[tokio::test]
async fn symmetric_one_has_network_exchange() {
    let ns = NetworkSecret::generate();
    let device_a = DeviceIdentity::generate();
    let device_b = DeviceIdentity::generate();
    let sync_dir_b = tempfile::tempdir().unwrap();

    let mut pin = PairingPin::generate();
    let pin_code = pin.code().to_string();
    pin.consume().unwrap();

    let (mut listener_ch, mut connector_ch) =
        wormhole::establish_pair_local(&pin_code).await.unwrap();

    let state_a = NetworkState {
        has_network: true,
        namespace_id: Some(*ns.namespace_id().as_bytes()),
        created_at_ms: None,
    };

    let state_b = NetworkState {
        has_network: false,
        namespace_id: None,
        created_at_ms: None,
    };

    let payload_a = PairingPayload {
        network_state: state_a,
        device_entry: make_entry(&device_a),
    };
    let payload_b = PairingPayload {
        network_state: state_b,
        device_entry: make_entry(&device_b),
    };

    let bytes_a = serde_json::to_vec(&payload_a).unwrap();
    listener_ch.send_encrypted(&bytes_a).await.unwrap();
    let recv_a = connector_ch.recv_encrypted().await.unwrap();
    let _peer_a: PairingPayload = serde_json::from_slice(&recv_a).unwrap();

    let bytes_b = serde_json::to_vec(&payload_b).unwrap();
    connector_ch.send_encrypted(&bytes_b).await.unwrap();
    let recv_b = listener_ch.recv_encrypted().await.unwrap();
    let _peer_b: PairingPayload = serde_json::from_slice(&recv_b).unwrap();

    let doc_ticket = DocTicketMsg {
        namespace_secret_bytes: ns.to_bytes(),
    };
    let ticket_bytes = serde_json::to_vec(&doc_ticket).unwrap();
    listener_ch.send_encrypted(&ticket_bytes).await.unwrap();

    let recv_ticket = connector_ch.recv_encrypted().await.unwrap();
    let recv_doc: DocTicketMsg = serde_json::from_slice(&recv_ticket).unwrap();
    let received_ns = NetworkSecret::from_bytes(&recv_doc.namespace_secret_bytes);
    received_ns.save(sync_dir_b.path()).unwrap();

    listener_ch.close().await.unwrap();
    connector_ch.close().await.unwrap();

    assert_eq!(ns.to_bytes(), received_ns.to_bytes());

    let loaded = NetworkSecret::load(sync_dir_b.path()).unwrap();
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().to_bytes(), ns.to_bytes());
}

#[tokio::test]
async fn symmetric_same_network_adds_peers() {
    let ns = NetworkSecret::generate();
    let device_a = DeviceIdentity::generate();
    let device_b = DeviceIdentity::generate();

    let mut pin = PairingPin::generate();
    let pin_code = pin.code().to_string();
    pin.consume().unwrap();

    let (mut listener_ch, mut connector_ch) =
        wormhole::establish_pair_local(&pin_code).await.unwrap();

    let ns_id = *ns.namespace_id().as_bytes();

    let state_a = NetworkState {
        has_network: true,
        namespace_id: Some(ns_id),
        created_at_ms: None,
    };
    let state_b = NetworkState {
        has_network: true,
        namespace_id: Some(ns_id),
        created_at_ms: None,
    };

    let entry_a = make_entry(&device_a);
    let entry_b = make_entry(&device_b);

    let payload_a = PairingPayload {
        network_state: state_a,
        device_entry: entry_a.clone(),
    };
    let payload_b = PairingPayload {
        network_state: state_b,
        device_entry: entry_b.clone(),
    };

    let bytes_a = serde_json::to_vec(&payload_a).unwrap();
    listener_ch.send_encrypted(&bytes_a).await.unwrap();
    let recv_a = connector_ch.recv_encrypted().await.unwrap();
    let peer_a: PairingPayload = serde_json::from_slice(&recv_a).unwrap();

    let bytes_b = serde_json::to_vec(&payload_b).unwrap();
    connector_ch.send_encrypted(&bytes_b).await.unwrap();
    let recv_b = listener_ch.recv_encrypted().await.unwrap();
    let peer_b: PairingPayload = serde_json::from_slice(&recv_b).unwrap();

    listener_ch.close().await.unwrap();
    connector_ch.close().await.unwrap();

    let mut list_a = DeviceList::new();
    list_a.add_device(entry_a);
    list_a.add_device(peer_b.device_entry);

    let mut list_b = DeviceList::new();
    list_b.add_device(entry_b);
    list_b.add_device(peer_a.device_entry);

    assert_eq!(list_a.devices.len(), 2);
    assert_eq!(list_b.devices.len(), 2);

    let keys_a = list_a.public_keys();
    let keys_b = list_b.public_keys();
    for key in &keys_a {
        assert!(keys_b.contains(key));
    }
}

#[tokio::test]
async fn symmetric_different_networks_returns_merge_needed() {
    let ns_a = NetworkSecret::generate();
    let ns_b = NetworkSecret::generate();

    let id_a = *ns_a.namespace_id().as_bytes();
    let id_b = *ns_b.namespace_id().as_bytes();
    assert_ne!(id_a, id_b);

    let state_a = NetworkState {
        has_network: true,
        namespace_id: Some(id_a),
        created_at_ms: None,
    };
    let state_b = NetworkState {
        has_network: true,
        namespace_id: Some(id_b),
        created_at_ms: None,
    };

    assert!(state_a.has_network);
    assert!(state_b.has_network);
    assert_ne!(state_a.namespace_id, state_b.namespace_id);
}
