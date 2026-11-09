use serde::{Deserialize, Serialize};

use crate::content::ContentPart;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,

    pub description: String,

    pub parameters: serde_json::Value,

    #[serde(default)]
    pub strict: Option<bool>,

    #[serde(default)]
    pub annotations: ToolAnnotations,

    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolAnnotations {
    #[serde(default)]
    pub title: Option<String>,

    #[serde(default)]
    pub read_only: Option<bool>,

    #[serde(default)]
    pub destructive: Option<bool>,

    #[serde(default)]
    pub idempotent: Option<bool>,

    #[serde(default)]
    pub open_world: Option<bool>,

    #[serde(default)]
    pub requires_approval: Option<bool>,

    #[serde(default)]
    pub async_execution: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,

    pub name: String,

    pub arguments: serde_json::Value,

    #[serde(default)]
    pub raw_arguments: Option<String>,

    #[serde(default)]
    pub provider_call_id: Option<String>,

    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            raw_arguments: None,
            provider_call_id: None,
            metadata: serde_json::Map::new(),
        }
    }

    pub fn effective_arguments(&self) -> serde_json::Value {
        if !arguments_need_raw_repair(&self.arguments) {
            return self.arguments.clone();
        }

        self.raw_arguments
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .filter(|value| !arguments_need_raw_repair(value))
            .unwrap_or_else(|| self.arguments.clone())
    }

    pub fn has_valid_identity(&self) -> bool {
        !self.id.trim().is_empty() && !self.name.trim().is_empty()
    }
}

fn arguments_need_raw_repair(value: &serde_json::Value) -> bool {
    value.is_null() || value.as_object().is_some_and(serde_json::Map::is_empty)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolAttachment {
    pub name: String,
    pub mime_type: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TruncationMeta {
    pub tool: String,

    #[serde(default)]
    pub total_lines: Option<u64>,

    #[serde(default)]
    pub total_bytes: Option<u64>,

    #[serde(default)]
    pub est_tokens: Option<u64>,

    #[serde(default)]
    pub shown_start: Option<u64>,

    #[serde(default)]
    pub shown_end: Option<u64>,

    #[serde(default)]
    pub continuation_hint: Option<String>,

    #[serde(default)]
    pub recall_key: Option<String>,

    #[serde(default)]
    pub retry_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,

    #[serde(default)]
    pub name: Option<String>,

    pub content: Vec<ContentPart>,

    #[serde(default)]
    pub is_error: bool,

    #[serde(default)]
    pub duration_ms: Option<u64>,

    #[serde(default)]
    pub attachments: Option<Vec<ToolAttachment>>,

    #[serde(default)]
    pub truncation: Option<TruncationMeta>,

    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl ToolResult {
    pub fn text(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            name: None,
            content: vec![ContentPart::Text { text: text.into() }],
            is_error: false,
            duration_ms: None,
            attachments: None,
            truncation: None,
            metadata: serde_json::Map::new(),
        }
    }

    pub fn error(call_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            name: None,
            content: vec![ContentPart::Text {
                text: message.into(),
            }],
            is_error: true,
            duration_ms: None,
            attachments: None,
            truncation: None,
            metadata: serde_json::Map::new(),
        }
    }

    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn has_valid_call_id(&self) -> bool {
        !self.call_id.trim().is_empty()
    }
}
