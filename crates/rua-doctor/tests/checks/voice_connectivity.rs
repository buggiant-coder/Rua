use rua_doctor::connectivity::is_reachable_status;

#[test]
fn twilio_401_is_reachable() {
    assert!(is_reachable_status(401));
}

#[test]
fn connectivity_treats_success_and_auth_errors_as_reachable() {
    assert!(is_reachable_status(200));
    assert!(is_reachable_status(401));
    assert!(is_reachable_status(403));
}

#[test]
fn connectivity_treats_missing_or_server_error_statuses_as_unreachable() {
    assert!(!is_reachable_status(404));
    assert!(!is_reachable_status(500));
}

#[test]
fn voice_creds_present_in_extra() {
    use rua_config::types::ChannelConfig;

    let json = serde_json::json!({
        "voice": {
            "enabled": true,
            "twilio_account_sid": "ACtest123",
            "twilio_auth_token": "token123",
            "phone_number": "+15551234567"
        }
    });
    let config: ChannelConfig = serde_json::from_value(json).unwrap();
    assert!(config.voice.enabled);
    assert_eq!(
        config
            .voice
            .extra
            .get("twilio_account_sid")
            .and_then(|v| v.as_str()),
        Some("ACtest123")
    );
    assert_eq!(
        config
            .voice
            .extra
            .get("twilio_auth_token")
            .and_then(|v| v.as_str()),
        Some("token123")
    );
    assert_eq!(
        config
            .voice
            .extra
            .get("phone_number")
            .and_then(|v| v.as_str()),
        Some("+15551234567")
    );
}

#[test]
fn voice_creds_missing_when_not_set() {
    use rua_config::types::ChannelConfig;

    let json = serde_json::json!({
        "voice": {
            "enabled": true
        }
    });
    let config: ChannelConfig = serde_json::from_value(json).unwrap();
    assert!(config.voice.enabled);
    assert!(config.voice.extra.get("twilio_account_sid").is_none());
    assert!(config.voice.extra.get("twilio_auth_token").is_none());
    assert!(config.voice.extra.get("phone_number").is_none());
}

#[test]
fn voice_disabled_no_creds_needed() {
    use rua_config::types::ChannelConfig;

    let json = serde_json::json!({
        "voice": {
            "enabled": false
        }
    });
    let config: ChannelConfig = serde_json::from_value(json).unwrap();
    assert!(!config.voice.enabled);
}
