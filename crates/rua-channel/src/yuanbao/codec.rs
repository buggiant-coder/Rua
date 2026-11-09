use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

fn encode_varint(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            buf.push(byte);
            break;
        }
        buf.push(byte | 0x80);
    }
}

fn decode_varint(data: &[u8], offset: &mut usize) -> Option<u64> {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        if *offset >= data.len() {
            return None;
        }
        let byte = data[*offset];
        *offset += 1;
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

fn encode_tag(buf: &mut Vec<u8>, field_number: u32, wire_type: u8) {
    encode_varint(buf, ((field_number as u64) << 3) | wire_type as u64);
}

fn encode_string_field(buf: &mut Vec<u8>, field_number: u32, value: &str) {
    if value.is_empty() {
        return;
    }
    encode_tag(buf, field_number, 2);
    encode_varint(buf, value.len() as u64);
    buf.extend_from_slice(value.as_bytes());
}

fn encode_bytes_field(buf: &mut Vec<u8>, field_number: u32, value: &[u8]) {
    if value.is_empty() {
        return;
    }
    encode_tag(buf, field_number, 2);
    encode_varint(buf, value.len() as u64);
    buf.extend_from_slice(value);
}

fn encode_uint32_field(buf: &mut Vec<u8>, field_number: u32, value: u32) {
    if value == 0 {
        return;
    }
    encode_tag(buf, field_number, 0);
    encode_varint(buf, value as u64);
}

fn encode_bool_field(buf: &mut Vec<u8>, field_number: u32, value: bool) {
    if !value {
        return;
    }
    encode_tag(buf, field_number, 0);
    buf.push(1);
}

fn encode_message_field(buf: &mut Vec<u8>, field_number: u32, inner: &[u8]) {
    if inner.is_empty() {
        return;
    }
    encode_tag(buf, field_number, 2);
    encode_varint(buf, inner.len() as u64);
    buf.extend_from_slice(inner);
}

struct FieldIter<'a> {
    data: &'a [u8],
    offset: usize,
}

#[derive(Debug)]
struct Field<'a> {
    number: u32,
    #[allow(dead_code)]
    wire_type: u8,
    varint_val: u64,
    bytes_val: &'a [u8],
}

impl<'a> FieldIter<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }
}

impl<'a> Iterator for FieldIter<'a> {
    type Item = Field<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.data.len() {
            return None;
        }
        let tag = decode_varint(self.data, &mut self.offset)?;
        let wire_type = (tag & 0x7) as u8;
        let number = (tag >> 3) as u32;

        match wire_type {
            0 => {
                let val = decode_varint(self.data, &mut self.offset)?;
                Some(Field {
                    number,
                    wire_type,
                    varint_val: val,
                    bytes_val: &[],
                })
            }
            2 => {
                let len = decode_varint(self.data, &mut self.offset)? as usize;
                if self.offset + len > self.data.len() {
                    return None;
                }
                let val = &self.data[self.offset..self.offset + len];
                self.offset += len;
                Some(Field {
                    number,
                    wire_type,
                    varint_val: 0,
                    bytes_val: val,
                })
            }
            1 => {
                if self.offset + 8 > self.data.len() {
                    return None;
                }
                self.offset += 8;
                Some(Field {
                    number,
                    wire_type,
                    varint_val: 0,
                    bytes_val: &[],
                })
            }
            5 => {
                if self.offset + 4 > self.data.len() {
                    return None;
                }
                self.offset += 4;
                Some(Field {
                    number,
                    wire_type,
                    varint_val: 0,
                    bytes_val: &[],
                })
            }
            _ => None,
        }
    }
}

pub const CMD_TYPE_REQUEST: u32 = 0;
pub const CMD_TYPE_RESPONSE: u32 = 1;
pub const CMD_TYPE_PUSH: u32 = 2;
pub const CMD_TYPE_PUSH_ACK: u32 = 3;

pub const CMD_AUTH_BIND: &str = "auth-bind";
pub const CMD_PING: &str = "ping";
pub const CMD_KICKOUT: &str = "kickout";

pub const MODULE_CONN_ACCESS: &str = "conn_access";
pub const MODULE_BIZ: &str = "yuanbao_openclaw_proxy";

pub const BIZ_CMD_SEND_C2C: &str = "send_c2c_message";
pub const BIZ_CMD_SEND_GROUP: &str = "send_group_message";
pub const BIZ_CMD_PRIVATE_HB: &str = "send_private_heartbeat";
pub const BIZ_CMD_GROUP_HB: &str = "send_group_heartbeat";

#[derive(Debug, Clone, Default)]
pub struct ConnHead {
    pub cmd_type: u32,
    pub cmd: String,
    pub seq_no: u32,
    pub msg_id: String,
    pub module: String,
    pub need_ack: bool,
    pub status: u32,
}

#[derive(Debug, Clone)]
pub struct ConnMsg {
    pub head: ConnHead,
    pub data: Vec<u8>,
}

fn encode_conn_head(head: &ConnHead) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    encode_uint32_field(&mut buf, 1, head.cmd_type);
    encode_string_field(&mut buf, 2, &head.cmd);
    encode_uint32_field(&mut buf, 3, head.seq_no);
    encode_string_field(&mut buf, 4, &head.msg_id);
    encode_string_field(&mut buf, 5, &head.module);
    encode_bool_field(&mut buf, 6, head.need_ack);
    encode_uint32_field(&mut buf, 10, head.status);
    buf
}

fn decode_conn_head(data: &[u8]) -> ConnHead {
    let mut head = ConnHead::default();
    for field in FieldIter::new(data) {
        match field.number {
            1 => head.cmd_type = field.varint_val as u32,
            2 => {
                head.cmd = String::from_utf8_lossy(field.bytes_val).to_string();
            }
            3 => head.seq_no = field.varint_val as u32,
            4 => {
                head.msg_id = String::from_utf8_lossy(field.bytes_val).to_string();
            }
            5 => {
                head.module = String::from_utf8_lossy(field.bytes_val).to_string();
            }
            6 => head.need_ack = field.varint_val != 0,
            10 => head.status = field.varint_val as u32,
            _ => {}
        }
    }
    head
}

pub fn encode_conn_msg(head: &ConnHead, inner_data: &[u8]) -> Vec<u8> {
    let head_bytes = encode_conn_head(head);
    let mut buf = Vec::with_capacity(head_bytes.len() + inner_data.len() + 16);
    encode_message_field(&mut buf, 1, &head_bytes);
    encode_bytes_field(&mut buf, 2, inner_data);
    buf
}

pub fn decode_conn_msg(raw: &[u8]) -> Option<ConnMsg> {
    let mut head_bytes: &[u8] = &[];
    let mut data_bytes: &[u8] = &[];

    for field in FieldIter::new(raw) {
        match field.number {
            1 => head_bytes = field.bytes_val,
            2 => data_bytes = field.bytes_val,
            _ => {}
        }
    }

    if head_bytes.is_empty() {
        return None;
    }

    Some(ConnMsg {
        head: decode_conn_head(head_bytes),
        data: data_bytes.to_vec(),
    })
}

fn encode_auth_info(uid: &str, source: &str, token: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    encode_string_field(&mut buf, 1, uid);
    encode_string_field(&mut buf, 2, source);
    encode_string_field(&mut buf, 3, token);
    buf
}

fn encode_device_info() -> Vec<u8> {
    let mut buf = Vec::with_capacity(8);
    encode_string_field(&mut buf, 1, "16");
    buf
}

fn encode_auth_bind_req(biz_id: &str, uid: &str, source: &str, token: &str) -> Vec<u8> {
    let auth_info = encode_auth_info(uid, source, token);
    let device_info = encode_device_info();

    let mut buf = Vec::with_capacity(128);
    encode_string_field(&mut buf, 1, biz_id);
    encode_message_field(&mut buf, 2, &auth_info);
    encode_message_field(&mut buf, 3, &device_info);
    buf
}

#[derive(Debug, Clone, Default)]
pub struct AuthBindRsp {
    pub code: u32,
    pub message: String,
    pub connect_id: String,
}

pub fn decode_auth_bind_rsp(data: &[u8]) -> AuthBindRsp {
    let mut rsp = AuthBindRsp::default();
    for field in FieldIter::new(data) {
        match field.number {
            1 => rsp.code = field.varint_val as u32,
            2 => rsp.message = String::from_utf8_lossy(field.bytes_val).to_string(),
            3 => rsp.connect_id = String::from_utf8_lossy(field.bytes_val).to_string(),
            _ => {}
        }
    }
    rsp
}

#[derive(Debug, Clone, Default)]
pub struct PingRsp {
    pub heart_interval: u32,
}

pub fn decode_ping_rsp(data: &[u8]) -> PingRsp {
    let mut rsp = PingRsp::default();
    for field in FieldIter::new(data) {
        if field.number == 1 {
            rsp.heart_interval = field.varint_val as u32;
        }
    }
    rsp
}

#[derive(Debug, Clone, Default)]
pub struct KickoutMsg {
    pub reason: String,
}

pub fn decode_kickout_msg(data: &[u8]) -> KickoutMsg {
    let mut msg = KickoutMsg::default();
    for field in FieldIter::new(data) {
        if field.number == 1 {
            msg.reason = String::from_utf8_lossy(field.bytes_val).to_string();
        }
    }
    msg
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InboundMessagePush {
    #[serde(default, rename = "callbackCommand")]
    pub callback_command: String,
    #[serde(default, rename = "fromAccount")]
    pub from_account: String,
    #[serde(default, rename = "toAccount")]
    pub to_account: String,
    #[serde(default, rename = "senderNickname")]
    pub sender_nickname: String,
    #[serde(default, rename = "groupCode")]
    pub group_code: String,
    #[serde(default, rename = "groupName")]
    pub group_name: String,
    #[serde(default, rename = "msgSeq")]
    pub msg_seq: u64,
    #[serde(default, rename = "msgTime")]
    pub msg_time: u64,
    #[serde(default, rename = "msgKey")]
    pub msg_key: String,
    #[serde(default, rename = "msgId")]
    pub msg_id: String,
    #[serde(default, rename = "msgBody")]
    pub msg_body: Vec<MsgBodyElem>,
    #[serde(default, rename = "botOwnerId")]
    pub bot_owner_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MsgBodyElem {
    #[serde(default, rename = "msgType")]
    pub msg_type: String,
    #[serde(default, rename = "msgContent")]
    pub msg_content: serde_json::Value,
}

fn encode_msg_body_elem(msg_type: &str, msg_content: &serde_json::Value) -> Vec<u8> {
    let content_str = serde_json::to_string(msg_content).unwrap_or_default();
    let mut buf = Vec::with_capacity(64);
    encode_string_field(&mut buf, 1, msg_type);
    encode_string_field(&mut buf, 2, &content_str);
    buf
}

fn encode_send_c2c_req(
    to_account: &str,
    from_account: &str,
    msg_random: u32,
    msg_body: &[(String, serde_json::Value)],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);
    encode_string_field(&mut buf, 1, to_account);
    encode_string_field(&mut buf, 2, from_account);
    encode_uint32_field(&mut buf, 3, msg_random);
    for (msg_type, content) in msg_body {
        let elem = encode_msg_body_elem(msg_type, content);
        encode_message_field(&mut buf, 4, &elem);
    }
    buf
}

fn encode_send_group_req(
    group_code: &str,
    from_account: &str,
    random: &str,
    msg_body: &[(String, serde_json::Value)],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);
    encode_string_field(&mut buf, 1, group_code);
    encode_string_field(&mut buf, 2, from_account);
    encode_string_field(&mut buf, 3, random);
    for (msg_type, content) in msg_body {
        let elem = encode_msg_body_elem(msg_type, content);
        encode_message_field(&mut buf, 4, &elem);
    }
    buf
}

fn encode_private_heartbeat_req(from_account: &str, to_account: &str, heartbeat: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(96);
    encode_string_field(&mut buf, 1, from_account);
    encode_string_field(&mut buf, 2, to_account);
    encode_uint32_field(&mut buf, 3, heartbeat);
    buf
}

fn encode_group_heartbeat_req(
    from_account: &str,
    to_account: &str,
    group_code: &str,
    send_time: u32,
    heartbeat: u32,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128);
    encode_string_field(&mut buf, 1, from_account);
    encode_string_field(&mut buf, 2, to_account);
    encode_string_field(&mut buf, 3, group_code);
    encode_uint32_field(&mut buf, 4, send_time);
    encode_uint32_field(&mut buf, 5, heartbeat);
    buf
}

static SEQ_COUNTER: AtomicU32 = AtomicU32::new(0);

fn next_seq() -> u32 {
    let val = SEQ_COUNTER.fetch_add(1, Ordering::Relaxed);
    if val >= u32::MAX / 2 {
        SEQ_COUNTER.store(0, Ordering::Relaxed);
    }
    val
}

fn generate_msg_id() -> String {
    uuid::Uuid::new_v4().as_simple().to_string()
}

pub fn build_auth_bind_msg(biz_id: &str, uid: &str, source: &str, token: &str) -> Vec<u8> {
    let auth_data = encode_auth_bind_req(biz_id, uid, source, token);
    let head = ConnHead {
        cmd_type: CMD_TYPE_REQUEST,
        cmd: CMD_AUTH_BIND.to_string(),
        seq_no: next_seq(),
        msg_id: generate_msg_id(),
        module: MODULE_CONN_ACCESS.to_string(),
        ..Default::default()
    };
    encode_conn_msg(&head, &auth_data)
}

pub fn build_ping_msg() -> Vec<u8> {
    let head = ConnHead {
        cmd_type: CMD_TYPE_REQUEST,
        cmd: CMD_PING.to_string(),
        seq_no: next_seq(),
        msg_id: generate_msg_id(),
        module: MODULE_CONN_ACCESS.to_string(),
        ..Default::default()
    };
    encode_conn_msg(&head, &[])
}

pub fn build_push_ack(original_head: &ConnHead) -> Vec<u8> {
    let head = ConnHead {
        cmd_type: CMD_TYPE_PUSH_ACK,
        cmd: original_head.cmd.clone(),
        seq_no: next_seq(),
        msg_id: original_head.msg_id.clone(),
        module: original_head.module.clone(),
        ..Default::default()
    };
    encode_conn_msg(&head, &[])
}

pub fn build_send_c2c_msg(
    to_account: &str,
    from_account: &str,
    msg_body: &[(String, serde_json::Value)],
) -> (Vec<u8>, String) {
    let msg_random = rand::random::<u32>();
    let biz_data = encode_send_c2c_req(to_account, from_account, msg_random, msg_body);
    let msg_id = generate_msg_id();
    let head = ConnHead {
        cmd_type: CMD_TYPE_REQUEST,
        cmd: BIZ_CMD_SEND_C2C.to_string(),
        seq_no: next_seq(),
        msg_id: msg_id.clone(),
        module: MODULE_BIZ.to_string(),
        ..Default::default()
    };
    (encode_conn_msg(&head, &biz_data), msg_id)
}

pub fn build_send_group_msg(
    group_code: &str,
    from_account: &str,
    msg_body: &[(String, serde_json::Value)],
) -> (Vec<u8>, String) {
    let random_str = format!("{}", rand::random::<u32>());
    let biz_data = encode_send_group_req(group_code, from_account, &random_str, msg_body);
    let msg_id = generate_msg_id();
    let head = ConnHead {
        cmd_type: CMD_TYPE_REQUEST,
        cmd: BIZ_CMD_SEND_GROUP.to_string(),
        seq_no: next_seq(),
        msg_id: msg_id.clone(),
        module: MODULE_BIZ.to_string(),
        ..Default::default()
    };
    (encode_conn_msg(&head, &biz_data), msg_id)
}

pub fn build_heartbeat_msg(
    from_account: &str,
    to_account: &str,
    heartbeat: u32,
    group_code: Option<&str>,
) -> (Vec<u8>, String) {
    let (cmd, biz_data) = if let Some(group_code) = group_code.filter(|value| !value.is_empty()) {
        (
            BIZ_CMD_GROUP_HB,
            encode_group_heartbeat_req(from_account, to_account, group_code, 0, heartbeat),
        )
    } else {
        (
            BIZ_CMD_PRIVATE_HB,
            encode_private_heartbeat_req(from_account, to_account, heartbeat),
        )
    };
    let msg_id = generate_msg_id();
    let head = ConnHead {
        cmd_type: CMD_TYPE_REQUEST,
        cmd: cmd.to_string(),
        seq_no: next_seq(),
        msg_id: msg_id.clone(),
        module: MODULE_BIZ.to_string(),
        ..Default::default()
    };
    (encode_conn_msg(&head, &biz_data), msg_id)
}
