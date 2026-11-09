use rua_sync::pairing::PairingPin;

#[tokio::test]
async fn pin_can_only_be_consumed_once() {
    let mut pin = PairingPin::generate();

    assert!(pin.consume().is_ok());
    assert!(pin.is_consumed());

    let err = pin.consume();
    assert!(err.is_err());
    let msg = err.unwrap_err().to_string();
    assert!(msg.contains("already consumed"));
}

#[tokio::test]
async fn pin_matches_validates_code() {
    let pin = PairingPin::generate();
    let code = pin.code().to_string();

    assert!(pin.matches(&code));
    assert!(!pin.matches("00000000"));
    assert!(!pin.matches(""));
}
