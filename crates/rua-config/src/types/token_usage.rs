use crate::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type TokenUsageFile = HashMap<String, HashMap<String, TokenUsageEntry>>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsageEntry {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub provider_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub model_name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub prompt_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub completion_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub call_count: u64,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
