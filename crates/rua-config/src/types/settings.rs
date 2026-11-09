use crate::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SettingsFile {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub language: String,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
