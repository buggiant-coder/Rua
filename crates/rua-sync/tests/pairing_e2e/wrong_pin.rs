use iroh::endpoint::presets;
use iroh::Endpoint;
use rua_sync::identity::NetworkSecret;
use rua_sync::pairing::PairingPin;
use rua_sync::wormhole;

const PAIRING_ALPN: &[u8] = b"rua-pairing/1";

#[tokio::test]
async fn wrong_pin_prevents_connection() {
    let mut pin = PairingPin::generate();
    let correct_pin = pin.code().to_string();
    pin.consume().unwrap();
    let wrong_pin = "99999999";

    let correct_secret = wormhole::derive_rendezvous_secret(&correct_pin);
    let init_ep = Endpoint::builder(presets::Minimal)
        .secret_key(correct_secret)
        .alpns(vec![PAIRING_ALPN.to_vec()])
        .bind()
        .await
        .unwrap();

    let init_addrs = init_ep.bound_sockets();
    let init_node_id = init_ep.id();

    let wrong_secret = wormhole::derive_rendezvous_secret(wrong_pin);
    let wrong_node_id = wrong_secret.public();
    assert_ne!(init_node_id, wrong_node_id);

    let join_ep = Endpoint::builder(presets::Minimal)
        .alpns(vec![PAIRING_ALPN.to_vec()])
        .bind()
        .await
        .unwrap();

    let mut node_addr = iroh::EndpointAddr::new(wrong_node_id);
    for sock in init_addrs {
        node_addr.addrs.insert(iroh::TransportAddr::Ip(sock));
    }

    let connect_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        join_ep.connect(node_addr, PAIRING_ALPN),
    )
    .await;

    assert!(
        connect_result.is_err() || connect_result.as_ref().unwrap().is_err(),
        "wrong PIN should prevent connection"
    );

    let tmp = tempfile::tempdir().unwrap();
    assert!(NetworkSecret::load(tmp.path()).unwrap().is_none());

    init_ep.close().await;
    join_ep.close().await;
}
