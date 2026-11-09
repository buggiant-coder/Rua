use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    #[serde(default)]
    pub webhook_url: String,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub bot_user_id: Option<String>,
    #[serde(default)]
    pub media_dir: String,
}
