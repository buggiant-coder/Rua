use rua_channel::yuanbao::codec::{
    build_auth_bind_msg, build_ping_msg, build_push_ack, build_send_c2c_msg, build_send_group_msg,
    decode_conn_msg, ConnHead, CMD_AUTH_BIND, CMD_PING, CMD_TYPE_PUSH, CMD_TYPE_PUSH_ACK,
    CMD_TYPE_REQUEST, MODULE_BIZ, MODULE_CONN_ACCESS,
};

#[test]
fn ping_msg_roundtrip() {
    let raw = build_ping_msg();
    assert!(!raw.is_empty());
    let decoded = decode_conn_msg(&raw).unwrap();
    assert_eq!(decoded.head.cmd, CMD_PING);
    assert_eq!(decoded.head.cmd_type, CMD_TYPE_REQUEST);
    assert_eq!(decoded.head.module, MODULE_CONN_ACCESS);
}

#[test]
fn auth_bind_msg_roundtrip() {
    let raw = build_auth_bind_msg("ybBot", "bot123", "bot", "token-value");
    assert!(!raw.is_empty());
    let decoded = decode_conn_msg(&raw).unwrap();
    assert_eq!(decoded.head.cmd, CMD_AUTH_BIND);
    assert_eq!(decoded.head.cmd_type, CMD_TYPE_REQUEST);
    assert_eq!(decoded.head.module, MODULE_CONN_ACCESS);
    assert!(!decoded.data.is_empty());
}

#[test]
fn push_ack_roundtrip() {
    let original = ConnHead {
        cmd_type: CMD_TYPE_PUSH,
        cmd: "biz_cmd".to_string(),
        seq_no: 5,
        msg_id: "msg-123".to_string(),
        module: MODULE_BIZ.to_string(),
        need_ack: true,
        status: 0,
    };
    let ack_raw = build_push_ack(&original);
    let decoded = decode_conn_msg(&ack_raw).unwrap();
    assert_eq!(decoded.head.cmd_type, CMD_TYPE_PUSH_ACK);
    assert_eq!(decoded.head.cmd, "biz_cmd");
    assert_eq!(decoded.head.msg_id, "msg-123");
    assert_eq!(decoded.head.module, MODULE_BIZ);
}

#[test]
fn send_c2c_roundtrip() {
    let body = vec![(
        "TIMTextElem".to_string(),
        serde_json::json!({"text": "hello"}),
    )];
    let (raw, msg_id) = build_send_c2c_msg("user123", "bot456", &body);
    assert!(!msg_id.is_empty());
    assert!(!raw.is_empty());
    let decoded = decode_conn_msg(&raw).unwrap();
    assert_eq!(decoded.head.cmd, "send_c2c_message");
    assert_eq!(decoded.head.module, MODULE_BIZ);
    assert!(!decoded.data.is_empty());
}

#[test]
fn send_group_roundtrip() {
    let body = vec![(
        "TIMTextElem".to_string(),
        serde_json::json!({"text": "group msg"}),
    )];
    let (raw, msg_id) = build_send_group_msg("grp789", "bot456", &body);
    assert!(!msg_id.is_empty());
    assert!(!raw.is_empty());
    let decoded = decode_conn_msg(&raw).unwrap();
    assert_eq!(decoded.head.cmd, "send_group_message");
    assert_eq!(decoded.head.module, MODULE_BIZ);
}

#[test]
fn empty_data_roundtrip() {
    let raw = build_ping_msg();
    let decoded = decode_conn_msg(&raw).unwrap();

    assert!(decoded.data.is_empty());
}
