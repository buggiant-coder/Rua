use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MattermostConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub media_dir: String,
}
