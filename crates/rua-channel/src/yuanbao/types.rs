use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YuanbaoConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_secret: String,
    #[serde(default = "default_api_domain")]
    pub api_domain: String,
    #[serde(default)]
    pub media_dir: String,
}

fn default_api_domain() -> String {
    DEFAULT_API_DOMAIN.to_string()
}

impl Default for YuanbaoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            app_id: String::new(),
            app_secret: String::new(),
            api_domain: DEFAULT_API_DOMAIN.to_string(),
            media_dir: String::new(),
        }
    }
}

pub const DEFAULT_WS_URL: &str = "wss://bot-wss.yuanbao.tencent.com/wss/connection";
pub const DEFAULT_API_DOMAIN: &str = "bot.yuanbao.tencent.com";
pub const SIGN_TOKEN_PATH: &str = "/api/v5/robotLogic/sign-token";

pub const RETRYABLE_SIGN_CODE: i64 = 10099;
pub const SIGN_MAX_RETRIES: u32 = 3;
pub const SIGN_RETRY_DELAY_MS: u64 = 1000;

pub const TOKEN_REFRESH_MARGIN_SECS: u64 = 300;
pub const HEARTBEAT_INTERVAL_SECS: u64 = 5;
pub const HEARTBEAT_TIMEOUT_THRESHOLD: u32 = 2;
pub const SEND_TIMEOUT_SECS: u64 = 30;
pub const TEXT_CHUNK_LIMIT: usize = 2800;
pub const TYPING_KEEPALIVE_INTERVAL_SECS: u64 = 3;
pub const HEARTBEAT_RUNNING: u32 = 1;
pub const HEARTBEAT_FINISH: u32 = 2;

pub const RECONNECT_DELAYS: &[u64] = &[1, 2, 5, 10, 30, 60];
pub const MAX_RECONNECT_ATTEMPTS: u32 = 100;

pub const NO_RECONNECT_CLOSE_CODES: &[u16] = &[4012, 4013, 4014, 4018, 4019, 4021];
pub const AUTH_FAILED_CODES: &[u32] = &[41103, 41104, 41108];
pub const AUTH_ALREADY_CODE: u32 = 41101;
