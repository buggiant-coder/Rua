use rua_sync::pairing::PairingPin;

#[tokio::test]
async fn expired_pin_cannot_be_consumed() {
    let mut pin = PairingPin::generate();

    assert!(!pin.is_expired());
    assert!(pin.consume().is_ok());

    let err = pin.consume();
    assert!(err.is_err());
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("already consumed"));
}

#[tokio::test]
async fn pin_remaining_secs_decreases() {
    let pin = PairingPin::generate();
    let secs = pin.remaining_secs();

    assert!(secs > 295 && secs <= 300);
}
