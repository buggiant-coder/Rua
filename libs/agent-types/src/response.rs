use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::finish::FinishReason;
use crate::model_dialogue_item::ModelDialogueItem;
use crate::usage::TokenUsage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub message: ModelDialogueItem,

    #[serde(default)]
    pub usage: Option<TokenUsage>,

    #[serde(default)]
    pub model: Option<String>,

    pub finish_reason: FinishReason,

    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl ChatResponse {
    pub fn has_tool_calls(&self) -> bool {
        self.finish_reason.is_tool_use() || self.message.has_tool_calls()
    }

    pub fn is_complete(&self) -> bool {
        self.finish_reason.is_stop()
    }

    pub fn text_content(&self) -> String {
        self.message.text_content()
    }
}
