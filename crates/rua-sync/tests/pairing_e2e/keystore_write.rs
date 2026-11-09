use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use rua_sync::pairing::{NetworkState, PairingPin};
use rua_sync::wormhole;

#[derive(serde::Serialize, serde::Deserialize)]
struct PairingPayload {
    network_state: NetworkState,
    device_entry: rua_sync::device_list::DeviceEntry,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DocTicketMsg {
    namespace_secret_bytes: [u8; 32],
}

#[tokio::test]
async fn receiver_writes_network_secret_to_file() {
    let ns = NetworkSecret::generate();

    let mut pin = PairingPin::generate();
    let pin_code = pin.code().to_string();
    pin.consume().unwrap();

    let receiver_sync_dir = tempfile::tempdir().unwrap();
    assert!(NetworkSecret::load(receiver_sync_dir.path())
        .unwrap()
        .is_none());

    let (mut listener_ch, mut connector_ch) =
        wormhole::establish_pair_local(&pin_code).await.unwrap();

    let device = DeviceIdentity::generate();

    let listener_payload = PairingPayload {
        network_state: NetworkState {
            has_network: true,
            namespace_id: Some(*ns.namespace_id().as_bytes()),
            created_at_ms: None,
        },
        device_entry: rua_sync::device_list::DeviceEntry {
            device_id: [0; 8],
            public_key: [0; 32],
            label: String::new(),
            friendly_name: String::new(),
            added_at_ms: 1000,
            last_seen_ms: 0,
            direct_addrs: vec![],
        },
    };
    let connector_payload = PairingPayload {
        network_state: NetworkState {
            has_network: false,
            namespace_id: None,
            created_at_ms: None,
        },
        device_entry: rua_sync::device_list::DeviceEntry {
            device_id: device.device_id_short(),
            public_key: device.public_key().to_bytes(),
            label: String::new(),
            friendly_name: String::new(),
            added_at_ms: 1000,
            last_seen_ms: 0,
            direct_addrs: vec![],
        },
    };

    listener_ch
        .send_encrypted(&serde_json::to_vec(&listener_payload).unwrap())
        .await
        .unwrap();
    let _ = connector_ch.recv_encrypted().await.unwrap();

    connector_ch
        .send_encrypted(&serde_json::to_vec(&connector_payload).unwrap())
        .await
        .unwrap();
    let _ = listener_ch.recv_encrypted().await.unwrap();

    let doc_ticket = DocTicketMsg {
        namespace_secret_bytes: ns.to_bytes(),
    };
    listener_ch
        .send_encrypted(&serde_json::to_vec(&doc_ticket).unwrap())
        .await
        .unwrap();

    let ticket_bytes = connector_ch.recv_encrypted().await.unwrap();
    let recv_ticket: DocTicketMsg = serde_json::from_slice(&ticket_bytes).unwrap();
    let received_ns = NetworkSecret::from_bytes(&recv_ticket.namespace_secret_bytes);
    received_ns.save(receiver_sync_dir.path()).unwrap();

    listener_ch.close().await.unwrap();
    connector_ch.close().await.unwrap();

    let loaded = NetworkSecret::load(receiver_sync_dir.path()).unwrap();
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().to_bytes(), ns.to_bytes());
}
