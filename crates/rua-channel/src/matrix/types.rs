use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatrixConfig {
    #[serde(default)]
    pub homeserver_url: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub default_room_id: String,
    #[serde(default)]
    pub encryption_enabled: bool,
    #[serde(default)]
    pub store_path: Option<String>,
    #[serde(default)]
    pub media_dir: String,
}

/// QwenPaw room policy and rendering options, independent of login credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MatrixOptions {
    pub group_allow_from: Vec<String>,
    pub groups: std::collections::BTreeMap<String, serde_json::Value>,
    pub vision_enabled: bool,
    pub history_limit: usize,
    pub sync_timeout_ms: u64,
    pub mention_pill_in_body: bool,
    pub outbound_structured_mentions: bool,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for MatrixOptions {
    fn default() -> Self {
        Self {
            group_allow_from: Vec::new(),
            groups: Default::default(),
            vision_enabled: true,
            history_limit: 50,
            sync_timeout_ms: 30_000,
            mention_pill_in_body: false,
            outbound_structured_mentions: true,
            extra: Default::default(),
        }
    }
}

impl MatrixOptions {
    pub(crate) fn require_mention(&self, room_id: &str) -> bool {
        let room = self.groups.get(room_id).or_else(|| self.groups.get("*"));
        if room
            .and_then(|v| v.get("autoReply"))
            .and_then(|v| v.as_bool())
            == Some(true)
        {
            return false;
        }
        room.and_then(|v| v.get("requireMention"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }
}
