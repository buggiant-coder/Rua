use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FeishuConfig {
    #[serde(default)]
    pub webhook_url: String,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub app_secret: Option<String>,
    #[serde(default)]
    pub bot_open_id: Option<String>,
    #[serde(default)]
    pub share_session_in_group: bool,
    #[serde(default)]
    pub media_dir: String,
}
