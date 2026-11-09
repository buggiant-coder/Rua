use rua_channel::yuanbao::codec::{
    decode_auth_bind_rsp, decode_conn_msg, decode_kickout_msg, decode_ping_rsp, encode_conn_msg,
    ConnHead, CMD_TYPE_PUSH, MODULE_BIZ,
};

#[test]
fn decode_ping_rsp_with_heart_interval() {
    let data = vec![0x08, 10];
    let rsp = decode_ping_rsp(&data);
    assert_eq!(rsp.heart_interval, 10);
}

#[test]
fn decode_ping_rsp_empty() {
    let rsp = decode_ping_rsp(&[]);
    assert_eq!(rsp.heart_interval, 0);
}

#[test]
fn decode_auth_bind_rsp_success() {
    let mut data = Vec::new();

    data.push(0x1a);
    let s = b"conn-abc";
    data.push(s.len() as u8);
    data.extend_from_slice(s);

    let rsp = decode_auth_bind_rsp(&data);
    assert_eq!(rsp.code, 0);
    assert_eq!(rsp.connect_id, "conn-abc");
}

#[test]
fn decode_auth_bind_rsp_error() {
    let mut data = Vec::new();

    data.push(0x08);

    let mut val = 41103u64;
    loop {
        let byte = (val & 0x7F) as u8;
        val >>= 7;
        if val == 0 {
            data.push(byte);
            break;
        }
        data.push(byte | 0x80);
    }

    data.push(0x12);
    let s = b"auth failed";
    data.push(s.len() as u8);
    data.extend_from_slice(s);

    let rsp = decode_auth_bind_rsp(&data);
    assert_eq!(rsp.code, 41103);
    assert_eq!(rsp.message, "auth failed");
}

#[test]
fn decode_kickout_msg_reason() {
    let mut data = Vec::new();
    data.push(0x0a);
    let s = b"version ban";
    data.push(s.len() as u8);
    data.extend_from_slice(s);

    let msg = decode_kickout_msg(&data);
    assert_eq!(msg.reason, "version ban");
}

#[test]
fn decode_push_frame_with_json_data() {
    let json_payload = r#"{"callback_command":"C2C.CallbackAfterSendMsg","from_account":"user1","msg_body":[{"msg_type":"TIMTextElem","msg_content":{"text":"hello"}}]}"#;

    let head = ConnHead {
        cmd_type: CMD_TYPE_PUSH,
        cmd: "push".to_string(),
        seq_no: 1,
        msg_id: "push-1".to_string(),
        module: MODULE_BIZ.to_string(),
        need_ack: true,
        status: 0,
    };
    let raw = encode_conn_msg(&head, json_payload.as_bytes());
    let decoded = decode_conn_msg(&raw).unwrap();

    assert_eq!(decoded.head.cmd_type, CMD_TYPE_PUSH);
    assert!(decoded.head.need_ack);

    let json_str = std::str::from_utf8(&decoded.data).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap();
    assert_eq!(parsed["callback_command"], "C2C.CallbackAfterSendMsg");
    assert_eq!(parsed["from_account"], "user1");
}
