use rua_sync::pairing::PairingPin;

#[test]
fn pin_is_8_digits() {
    let pin = PairingPin::generate();
    assert_eq!(pin.code().len(), 8);
    assert!(pin.code().chars().all(|c| c.is_ascii_digit()));
}

#[test]
fn pin_not_expired_immediately() {
    let pin = PairingPin::generate();
    assert!(!pin.is_expired());
    assert!(!pin.is_consumed());
}

#[test]
fn pin_matches_own_code() {
    let pin = PairingPin::generate();
    let code = pin.code().to_string();
    assert!(pin.matches(&code));
    assert!(!pin.matches("00000000"));
}

#[test]
fn pin_single_use() {
    let mut pin = PairingPin::generate();
    pin.consume().unwrap();
    assert!(pin.is_consumed());
    let result = pin.consume();
    assert!(result.is_err());
}

#[test]
fn pin_unique_per_generation() {
    let pins: Vec<String> = (0..100)
        .map(|_| PairingPin::generate().code().to_string())
        .collect();
    let unique: std::collections::HashSet<&str> = pins.iter().map(|s| s.as_str()).collect();
    assert!(unique.len() > 90);
}
