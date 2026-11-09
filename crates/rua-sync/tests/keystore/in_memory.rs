use rua_sync::identity::NetworkSecret;

#[test]
fn network_secret_from_bytes_roundtrip() {
    let ns = NetworkSecret::generate();
    let bytes = ns.to_bytes();
    let restored = NetworkSecret::from_bytes(&bytes);
    assert_eq!(restored.to_bytes(), bytes);
}

#[test]
fn network_secret_namespace_id_deterministic() {
    let ns = NetworkSecret::generate();
    let id1 = ns.namespace_id();
    let id2 = ns.namespace_id();
    assert_eq!(id1, id2);
}

#[test]
fn network_secret_different_keys_different_ids() {
    let n1 = NetworkSecret::generate();
    let n2 = NetworkSecret::generate();
    assert_ne!(n1.namespace_id(), n2.namespace_id());
}
