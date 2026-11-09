use serde::{Deserialize, Serialize};

pub const OP_DISPATCH: u8 = 0;
pub const OP_HEARTBEAT: u8 = 1;
pub const OP_IDENTIFY: u8 = 2;
pub const OP_RESUME: u8 = 6;
pub const OP_RECONNECT: u8 = 7;
pub const OP_INVALID_SESSION: u8 = 9;
pub const OP_HELLO: u8 = 10;
pub const OP_HEARTBEAT_ACK: u8 = 11;

pub const INTENT_PUBLIC_GUILD_MESSAGES: u32 = 1 << 30;
pub const INTENT_DIRECT_MESSAGE: u32 = 1 << 12;
pub const INTENT_GROUP_AND_C2C: u32 = 1 << 25;
pub const INTENT_GUILD_MEMBERS: u32 = 1 << 1;
pub const INTENT_INTERACTION: u32 = 1 << 26;

pub const DEFAULT_API_BASE: &str = "https://api.sgroup.qq.com";
pub const SANDBOX_API_BASE: &str = "https://sandbox.api.sgroup.qq.com";
pub const TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";

#[derive(Debug, Deserialize)]
pub struct GatewayPayload {
    pub op: u8,
    pub d: Option<serde_json::Value>,
    pub s: Option<u64>,
    pub t: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OutgoingPayload {
    pub op: u8,
    pub d: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct HelloData {
    pub heartbeat_interval: u64,
}

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(deserialize_with = "deserialize_expires_in")]
    pub expires_in: u64,
}

fn deserialize_expires_in<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        String(String),
        Number(u64),
    }
    match StringOrNumber::deserialize(deserializer)? {
        StringOrNumber::String(s) => s.parse().map_err(serde::de::Error::custom),
        StringOrNumber::Number(n) => Ok(n),
    }
}

#[derive(Debug, Deserialize)]
pub struct GatewayUrlResponse {
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct MessageEventSpec {
    pub message_type: &'static str,
    pub sender_keys: &'static [&'static str],
    pub extra_meta_keys: &'static [&'static str],
}

pub fn get_event_spec(event_type: &str) -> Option<&'static MessageEventSpec> {
    match event_type {
        "C2C_MESSAGE_CREATE" => Some(&C2C_SPEC),
        "AT_MESSAGE_CREATE" => Some(&GUILD_SPEC),
        "DIRECT_MESSAGE_CREATE" => Some(&DM_SPEC),
        "GROUP_AT_MESSAGE_CREATE" => Some(&GROUP_SPEC),
        _ => None,
    }
}

static C2C_SPEC: MessageEventSpec = MessageEventSpec {
    message_type: "c2c",
    sender_keys: &["user_openid", "id"],
    extra_meta_keys: &[],
};

static GUILD_SPEC: MessageEventSpec = MessageEventSpec {
    message_type: "guild",
    sender_keys: &["id", "username"],
    extra_meta_keys: &["channel_id", "guild_id"],
};

static DM_SPEC: MessageEventSpec = MessageEventSpec {
    message_type: "dm",
    sender_keys: &["id", "username"],
    extra_meta_keys: &["channel_id", "guild_id"],
};

static GROUP_SPEC: MessageEventSpec = MessageEventSpec {
    message_type: "group",
    sender_keys: &["member_openid", "id"],
    extra_meta_keys: &["group_openid"],
};

pub const RECONNECT_DELAYS: &[u64] = &[1, 2, 5, 10, 30, 60];
pub const MAX_RECONNECT_ATTEMPTS: usize = 100;
