use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OneBotConfig {
    #[serde(default)]
    pub http_url: String,
    #[serde(default = "default_ws_host")]
    pub ws_host: String,
    #[serde(default = "default_ws_port")]
    pub ws_port: u16,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub media_dir: String,
}

fn default_ws_host() -> String {
    "127.0.0.1".to_string()
}

fn default_ws_port() -> u16 {
    6199
}

impl Default for OneBotConfig {
    fn default() -> Self {
        Self {
            http_url: String::new(),
            ws_host: default_ws_host(),
            ws_port: default_ws_port(),
            access_token: None,
            media_dir: String::new(),
        }
    }
}
