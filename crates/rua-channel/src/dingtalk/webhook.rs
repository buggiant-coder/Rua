use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DingTalkWebhookPayload {
    #[serde(default)]
    pub msg_type: String,

    #[serde(default)]
    pub msg_id: String,

    #[serde(default)]
    pub conversation_id: String,

    #[serde(default)]
    pub conversation_type: String,

    #[serde(default)]
    pub sender_id: String,

    #[serde(default)]
    pub sender_nick: String,

    #[serde(default)]
    pub sender_staff_id: String,

    #[serde(default)]
    pub robot_code: String,

    #[serde(default)]
    pub is_in_at_list: bool,

    #[serde(default)]
    pub session_webhook: String,

    #[serde(default)]
    pub session_webhook_expired_time: i64,

    #[serde(default)]
    pub text: Option<TextContent>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextContent {
    #[serde(default)]
    pub content: String,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl DingTalkWebhookPayload {
    pub fn is_group(&self) -> bool {
        self.conversation_type == "2"
    }

    pub fn text_content(&self) -> String {
        self.text
            .as_ref()
            .map(|t| t.content.trim().to_string())
            .unwrap_or_default()
    }
}
