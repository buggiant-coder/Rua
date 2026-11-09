use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionPromptFileSnapshot {
    pub id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,

    pub included: bool,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionPendingActionSnapshot {
    pub action_type: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub questions: Option<serde_json::Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,

    pub status: String,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
