use rua_sync::wormhole::connector_listener_addr_for_test;

#[test]
fn pin_pairing_resolves_listener_addresses_instead_of_guessing_a_relay() {
    let addr = connector_listener_addr_for_test("12345678");

    assert!(
        addr.addrs.is_empty(),
        "the PIN carries only the listener identity; Address Lookup must resolve its addresses"
    );
}
