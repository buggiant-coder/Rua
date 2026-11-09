use serde::{Deserialize, Serialize};

pub const DEFAULT_WS_URL: &str = "wss://hag.cloud.huawei.com/openclaw/v1/ws/link";
pub const DEFAULT_WS_URL_BACKUP: &str = "wss://116.63.174.231/openclaw/v1/ws/link";
pub const HEARTBEAT_INTERVAL_SECS: u64 = 30;
pub const TEXT_CHUNK_LIMIT: usize = 4000;

pub(crate) const INITIAL_BACKOFF_SECS: u64 = 1;
pub(crate) const MAX_BACKOFF_SECS: u64 = 60;

fn default_ws_url() -> String {
    DEFAULT_WS_URL.into()
}

fn default_backup_ws_url() -> String {
    DEFAULT_WS_URL_BACKUP.into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XiaoyiConfig {
    #[serde(alias = "ak")]
    pub access_key: String,
    #[serde(alias = "sk")]
    pub secret_key: String,
    pub agent_id: String,
    #[serde(default = "default_ws_url")]
    pub ws_url: String,
    #[serde(default = "default_backup_ws_url")]
    pub backup_ws_url: String,
    #[serde(default)]
    pub media_dir: String,
}
