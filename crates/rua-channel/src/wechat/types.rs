use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WechatConfig {
    pub bot_token: Option<String>,
    pub base_url: Option<String>,
}
