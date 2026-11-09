use rua_channel::yuanbao::auth::{beijing_timestamp, compute_hmac_signature};

#[test]
fn signature_is_deterministic() {
    let sig1 = compute_hmac_signature("nonce1", "2026-01-01T00:00:00+08:00", "app123", "secret");
    let sig2 = compute_hmac_signature("nonce1", "2026-01-01T00:00:00+08:00", "app123", "secret");
    assert_eq!(sig1, sig2);
    assert_eq!(sig1.len(), 64);
}

#[test]
fn signature_changes_with_nonce() {
    let sig1 = compute_hmac_signature("nonce1", "ts", "app", "secret");
    let sig2 = compute_hmac_signature("nonce2", "ts", "app", "secret");
    assert_ne!(sig1, sig2);
}

#[test]
fn signature_changes_with_timestamp() {
    let sig1 = compute_hmac_signature("n", "ts1", "app", "secret");
    let sig2 = compute_hmac_signature("n", "ts2", "app", "secret");
    assert_ne!(sig1, sig2);
}

#[test]
fn signature_uses_app_secret_as_key() {
    let sig1 = compute_hmac_signature("n", "ts", "app", "secret1");
    let sig2 = compute_hmac_signature("n", "ts", "app", "secret2");
    assert_ne!(sig1, sig2);
}

#[test]
fn beijing_timestamp_format() {
    let ts = beijing_timestamp();
    assert!(ts.ends_with("+08:00"));
    assert!(ts.contains('T'));
}
