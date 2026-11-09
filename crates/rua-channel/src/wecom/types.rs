use serde::{Deserialize, Serialize};

pub const DEFAULT_WS_URL: &str = "wss://openws.work.weixin.qq.com";

pub mod cmd {
    pub const SUBSCRIBE: &str = "aibot_subscribe";
    pub const HEARTBEAT: &str = "ping";
    pub const RESPONSE: &str = "aibot_respond_msg";
    pub const RESPONSE_WELCOME: &str = "aibot_respond_welcome_msg";
    pub const SEND_MSG: &str = "aibot_send_msg";
    pub const CALLBACK: &str = "aibot_msg_callback";
    pub const EVENT_CALLBACK: &str = "aibot_event_callback";
    pub const UPLOAD_INIT: &str = "aibot_upload_media_init";
    pub const UPLOAD_CHUNK: &str = "aibot_upload_media_chunk";
    pub const UPLOAD_FINISH: &str = "aibot_upload_media_finish";
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WsFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    #[serde(default)]
    pub headers: WsHeaders,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errcode: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errmsg: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WsHeaders {
    #[serde(default)]
    pub req_id: String,
}

#[derive(Debug, Clone)]
pub struct WecomBotConfig {
    pub bot_id: String,
    pub secret: String,
    pub ws_url: String,
}

impl WecomBotConfig {
    pub fn new(bot_id: String, secret: String) -> Self {
        Self {
            bot_id,
            secret,
            ws_url: DEFAULT_WS_URL.to_string(),
        }
    }
}

pub fn generate_req_id(prefix: &str) -> String {
    format!("{}_{}", prefix, uuid::Uuid::new_v4())
}
