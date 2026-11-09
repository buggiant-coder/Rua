use crate::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolCallRecord {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub tool_use_id: String,

    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub tool_name: String,

    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub timestamp: String,

    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub input: serde_json::Value,

    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub output: serde_json::Value,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
