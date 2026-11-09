use rua_channel::qq::types::*;

#[test]
fn op_codes_match_protocol_spec() {
    assert_eq!(OP_DISPATCH, 0);
    assert_eq!(OP_HEARTBEAT, 1);
    assert_eq!(OP_IDENTIFY, 2);
    assert_eq!(OP_RESUME, 6);
    assert_eq!(OP_RECONNECT, 7);
    assert_eq!(OP_INVALID_SESSION, 9);
    assert_eq!(OP_HELLO, 10);
    assert_eq!(OP_HEARTBEAT_ACK, 11);
}

#[test]
fn gateway_payload_parses_hello() {
    let raw = r#"{"op":10,"d":{"heartbeat_interval":45000}}"#;
    let payload: GatewayPayload = serde_json::from_str(raw).unwrap();
    assert_eq!(payload.op, OP_HELLO);
    let data: HelloData = serde_json::from_value(payload.d.unwrap()).unwrap();
    assert_eq!(data.heartbeat_interval, 45000);
}

#[test]
fn gateway_payload_parses_dispatch_with_seq() {
    let raw = r#"{"op":0,"s":42,"t":"C2C_MESSAGE_CREATE","d":{"content":"hi"}}"#;
    let payload: GatewayPayload = serde_json::from_str(raw).unwrap();
    assert_eq!(payload.op, OP_DISPATCH);
    assert_eq!(payload.s, Some(42));
    assert_eq!(payload.t.as_deref(), Some("C2C_MESSAGE_CREATE"));
}

#[test]
fn gateway_payload_parses_heartbeat_ack() {
    let raw = r#"{"op":11}"#;
    let payload: GatewayPayload = serde_json::from_str(raw).unwrap();
    assert_eq!(payload.op, OP_HEARTBEAT_ACK);
    assert!(payload.d.is_none());
    assert!(payload.s.is_none());
}

#[test]
fn event_spec_c2c() {
    let spec = get_event_spec("C2C_MESSAGE_CREATE").unwrap();
    assert_eq!(spec.message_type, "c2c");
    assert_eq!(spec.sender_keys, &["user_openid", "id"]);
    assert!(spec.extra_meta_keys.is_empty());
}

#[test]
fn event_spec_group() {
    let spec = get_event_spec("GROUP_AT_MESSAGE_CREATE").unwrap();
    assert_eq!(spec.message_type, "group");
    assert_eq!(spec.extra_meta_keys, &["group_openid"]);
}

#[test]
fn event_spec_guild() {
    let spec = get_event_spec("AT_MESSAGE_CREATE").unwrap();
    assert_eq!(spec.message_type, "guild");
    assert_eq!(spec.extra_meta_keys, &["channel_id", "guild_id"]);
}

#[test]
fn event_spec_dm() {
    let spec = get_event_spec("DIRECT_MESSAGE_CREATE").unwrap();
    assert_eq!(spec.message_type, "dm");
}

#[test]
fn event_spec_unknown_returns_none() {
    assert!(get_event_spec("UNKNOWN_EVENT").is_none());
}

#[test]
fn token_response_parses_numeric_expires() {
    let raw = r#"{"access_token":"tok123","expires_in":7200}"#;
    let resp: TokenResponse = serde_json::from_str(raw).unwrap();
    assert_eq!(resp.access_token, "tok123");
    assert_eq!(resp.expires_in, 7200);
}

#[test]
fn token_response_parses_string_expires() {
    let raw = r#"{"access_token":"tok456","expires_in":"3600"}"#;
    let resp: TokenResponse = serde_json::from_str(raw).unwrap();
    assert_eq!(resp.access_token, "tok456");
    assert_eq!(resp.expires_in, 3600);
}

#[test]
fn outgoing_payload_serializes() {
    let payload = OutgoingPayload {
        op: OP_HEARTBEAT,
        d: serde_json::json!(null),
    };
    let json = serde_json::to_string(&payload).unwrap();
    assert!(json.contains("\"op\":1"));
}

#[test]
fn api_endpoints_are_correct() {
    assert_eq!(DEFAULT_API_BASE, "https://api.sgroup.qq.com");
    assert_eq!(SANDBOX_API_BASE, "https://sandbox.api.sgroup.qq.com");
    assert_eq!(TOKEN_URL, "https://bots.qq.com/app/getAppAccessToken");
}

#[test]
fn intents_are_correct_bit_positions() {
    assert_eq!(INTENT_PUBLIC_GUILD_MESSAGES, 1 << 30);
    assert_eq!(INTENT_DIRECT_MESSAGE, 1 << 12);
    assert_eq!(INTENT_GROUP_AND_C2C, 1 << 25);
    assert_eq!(INTENT_GUILD_MEMBERS, 1 << 1);
    assert_eq!(INTENT_INTERACTION, 1 << 26);
}
