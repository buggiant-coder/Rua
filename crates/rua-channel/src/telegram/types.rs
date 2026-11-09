use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramConfig {
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub chat_id: String,
    #[serde(default)]
    pub allowed_users: Vec<String>,
    #[serde(default)]
    pub bot_username: String,
    #[serde(default)]
    pub media_dir: String,
}
