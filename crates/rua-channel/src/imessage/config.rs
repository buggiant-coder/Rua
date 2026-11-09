use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IMessageConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default = "default_poll_sec")]
    pub poll_sec: f64,
    #[serde(default)]
    pub bot_prefix: String,
    #[serde(default)]
    pub allowed_senders: Vec<String>,
    #[serde(default)]
    pub media_dir: String,
    #[serde(default = "default_max_decoded_size")]
    pub max_decoded_size: usize,
}

impl Default for IMessageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            db_path: default_db_path(),
            poll_sec: default_poll_sec(),
            bot_prefix: String::new(),
            allowed_senders: Vec::new(),
            media_dir: String::new(),
            max_decoded_size: default_max_decoded_size(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_db_path() -> String {
    "~/Library/Messages/chat.db".into()
}

fn default_poll_sec() -> f64 {
    1.0
}

fn default_max_decoded_size() -> usize {
    10 * 1024 * 1024
}

impl IMessageConfig {
    pub fn filter_by_prefix(&self, text: &str) -> Option<String> {
        if self.bot_prefix.is_empty() {
            return Some(text.to_string());
        }
        text.strip_prefix(self.bot_prefix.as_str())
            .map(|rest| rest.trim().to_string())
    }
}
