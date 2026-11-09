use rua_channel::yuanbao::codec::{
    build_heartbeat_msg, build_send_c2c_msg, build_send_group_msg, decode_conn_msg,
};

fn read_varint(bytes: &[u8], offset: &mut usize) -> u64 {
    let mut shift = 0;
    let mut value = 0_u64;
    loop {
        let byte = bytes[*offset];
        *offset += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

fn read_len_delimited<'a>(bytes: &'a [u8], offset: &mut usize) -> &'a [u8] {
    let len = read_varint(bytes, offset) as usize;
    let start = *offset;
    *offset += len;
    &bytes[start..start + len]
}

fn protobuf_fields(bytes: &[u8]) -> Vec<(u64, Vec<u8>)> {
    let mut offset = 0;
    let mut fields = Vec::new();
    while offset < bytes.len() {
        let key = read_varint(bytes, &mut offset);
        let field = key >> 3;
        let wire = key & 0x07;
        let value = match wire {
            0 => read_varint(bytes, &mut offset).to_le_bytes().to_vec(),
            2 => read_len_delimited(bytes, &mut offset).to_vec(),
            other => panic!("unsupported wire type {other}"),
        };
        fields.push((field, value));
    }
    fields
}

fn string_field(bytes: &[u8], field: u64) -> String {
    protobuf_fields(bytes)
        .into_iter()
        .find(|(number, _)| *number == field)
        .map(|(_, value)| String::from_utf8(value).unwrap())
        .unwrap()
}

fn u32_field(bytes: &[u8], field: u64) -> u32 {
    let raw = protobuf_fields(bytes)
        .into_iter()
        .find(|(number, _)| *number == field)
        .map(|(_, value)| value)
        .unwrap();
    u64::from_le_bytes(raw.try_into().unwrap()) as u32
}

fn repeated_message_fields(bytes: &[u8], field: u64) -> Vec<Vec<u8>> {
    protobuf_fields(bytes)
        .into_iter()
        .filter(|(number, _)| *number == field)
        .map(|(_, value)| value)
        .collect()
}

#[test]
fn send_c2c_text_body_format() {
    let body = vec![(
        "TIMTextElem".to_string(),
        serde_json::json!({"text": "test message"}),
    )];
    let (raw, msg_id) = build_send_c2c_msg("target_user", "bot_account", &body);

    assert!(!msg_id.is_empty());
    assert!(msg_id.len() == 32);

    let decoded = decode_conn_msg(&raw).unwrap();
    assert_eq!(decoded.head.cmd, "send_c2c_message");
    assert_eq!(decoded.head.module, "yuanbao_openclaw_proxy");
    assert!(!decoded.data.is_empty());
    assert_eq!(string_field(&decoded.data, 1), "target_user");
    assert_eq!(string_field(&decoded.data, 2), "bot_account");
    let msg_body = repeated_message_fields(&decoded.data, 4);
    assert_eq!(msg_body.len(), 1);
    assert_eq!(string_field(&msg_body[0], 1), "TIMTextElem");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&string_field(&msg_body[0], 2)).unwrap(),
        serde_json::json!({"text": "test message"})
    );
}

#[test]
fn send_group_text_body_format() {
    let body = vec![(
        "TIMTextElem".to_string(),
        serde_json::json!({"text": "group message"}),
    )];
    let (raw, msg_id) = build_send_group_msg("group_code_123", "bot_account", &body);

    assert!(!msg_id.is_empty());
    assert!(msg_id.len() == 32);

    let decoded = decode_conn_msg(&raw).unwrap();
    assert_eq!(decoded.head.cmd, "send_group_message");
    assert_eq!(decoded.head.module, "yuanbao_openclaw_proxy");
    assert!(!decoded.data.is_empty());
}

#[test]
fn send_c2c_multiple_elements() {
    let body = vec![
        (
            "TIMTextElem".to_string(),
            serde_json::json!({"text": "line 1"}),
        ),
        (
            "TIMTextElem".to_string(),
            serde_json::json!({"text": "line 2"}),
        ),
    ];
    let (raw, _) = build_send_c2c_msg("user", "bot", &body);
    let decoded = decode_conn_msg(&raw).unwrap();

    assert!(decoded.data.len() > 30);
}

#[test]
fn private_typing_heartbeat_body_format() {
    let (raw, msg_id) = build_heartbeat_msg("bot_account", "target_user", 1, None);

    assert!(!msg_id.is_empty());
    assert!(msg_id.len() == 32);

    let decoded = decode_conn_msg(&raw).unwrap();
    assert_eq!(decoded.head.cmd, "send_private_heartbeat");
    assert_eq!(decoded.head.module, "yuanbao_openclaw_proxy");
    assert!(!decoded.data.is_empty());
    assert_eq!(string_field(&decoded.data, 1), "bot_account");
    assert_eq!(string_field(&decoded.data, 2), "target_user");
    assert_eq!(u32_field(&decoded.data, 3), 1);
}

#[test]
fn group_typing_heartbeat_body_format() {
    let (raw, msg_id) = build_heartbeat_msg("bot_account", "target_user", 2, Some("group_123"));

    assert!(!msg_id.is_empty());
    assert!(msg_id.len() == 32);

    let decoded = decode_conn_msg(&raw).unwrap();
    assert_eq!(decoded.head.cmd, "send_group_heartbeat");
    assert_eq!(decoded.head.module, "yuanbao_openclaw_proxy");
    assert!(!decoded.data.is_empty());
    assert_eq!(string_field(&decoded.data, 1), "bot_account");
    assert_eq!(string_field(&decoded.data, 2), "target_user");
    assert_eq!(string_field(&decoded.data, 3), "group_123");
    assert_eq!(u32_field(&decoded.data, 5), 2);
}
