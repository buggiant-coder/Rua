use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackConfig {
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub app_token: String,
    #[serde(default)]
    pub signing_secret: String,
    #[serde(default)]
    pub bot_user_id: String,
    #[serde(default)]
    pub media_dir: String,
}
