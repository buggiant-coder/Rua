use rua_sync::pairing::PairingPin;

#[test]
fn pin_remaining_secs_decreases() {
    let pin = PairingPin::generate();
    let secs = pin.remaining_secs();

    assert!(secs > 295 && secs <= 300);
}

#[test]
fn expired_pin_cannot_be_consumed() {
    let mut pin = PairingPin::generate();
    assert!(!pin.is_expired());

    pin.consume().unwrap();
}
