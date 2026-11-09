use rua_channel::yuanbao::auth::compute_hmac_signature;

#[test]
fn sign_token_body_contains_required_fields() {
    let nonce = "abc123";
    let timestamp = "2026-06-01T12:00:00+08:00";
    let app_id = "my_app_id";
    let app_secret = "my_secret";

    let signature = compute_hmac_signature(nonce, timestamp, app_id, app_secret);

    let body = serde_json::json!({
        "app_key": app_id,
        "nonce": nonce,
        "signature": signature,
        "timestamp": timestamp,
    });

    assert_eq!(body["app_key"], "my_app_id");
    assert_eq!(body["nonce"], "abc123");
    assert_eq!(body["timestamp"], "2026-06-01T12:00:00+08:00");
    assert_eq!(body["signature"].as_str().unwrap().len(), 64);
}
