use chrono::{DateTime, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::content::{content_part_from_value, ContentPart};
use crate::role::Role;
use crate::tool::{ToolCall, ToolResult};

/// Per-message text boundaries inside a provider-neutral assistant item.
/// The text stays in `content`; only its original message metadata is indexed here.
pub const ASSISTANT_MESSAGE_PARTS_METADATA_KEY: &str = "rua_assistant_message_parts";
pub const ASSISTANT_PHASE_METADATA_KEY: &str = "phase";
pub const PROVIDER_MESSAGE_ID_METADATA_KEY: &str = "provider_message_id";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessagePart {
    /// Index among Text parts, unaffected by Tool/Thinking protocol transforms.
    pub text_index: usize,
    #[serde(flatten)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

fn deserialize_flexible_datetime<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(s) => {
            if let Ok(dt) = s.parse::<DateTime<Utc>>() {
                return Ok(Some(dt));
            }
            if let Ok(dt) = DateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f %z") {
                return Ok(Some(dt.with_timezone(&Utc)));
            }
            if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f") {
                if let Some(local_dt) = Local.from_local_datetime(&naive).single() {
                    return Ok(Some(local_dt.with_timezone(&Utc)));
                }
            }
            Err(serde::de::Error::custom(format!("invalid timestamp: {s}")))
        }
    }
}

fn deserialize_content_parts<'de, D>(deserializer: D) -> Result<Vec<ContentPart>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::String(text) => Ok(vec![ContentPart::Text { text }]),
        serde_json::Value::Array(parts) => {
            Ok(parts.into_iter().map(content_part_from_value).collect())
        }
        other => Ok(vec![content_part_from_value(other)]),
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelDialogueItem {
    #[serde(default)]
    pub id: Option<String>,

    pub role: Role,

    #[serde(default, deserialize_with = "deserialize_content_parts")]
    pub content: Vec<ContentPart>,

    #[serde(default)]
    pub name: Option<String>,

    #[serde(
        default,
        alias = "timestamp",
        deserialize_with = "deserialize_flexible_datetime"
    )]
    pub created_at: Option<DateTime<Utc>>,

    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl Serialize for ModelDialogueItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(6))?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("role", &self.role)?;
        map.serialize_entry("content", &self.content)?;
        map.serialize_entry("metadata", &self.metadata)?;
        let ts = self
            .created_at
            .unwrap_or_else(Utc::now)
            .with_timezone(&Local);
        let formatted = ts.format("%Y-%m-%d %H:%M:%S%.3f").to_string();
        map.serialize_entry("timestamp", &formatted)?;
        map.end()
    }
}

impl ModelDialogueItem {
    /// Restore the assistant message boundaries carried by a model response.
    /// Ordinary history, including history written before this metadata existed,
    /// remains one item. Persistence and provider encoding use the same split.
    pub fn split_assistant_messages(&self) -> Vec<Self> {
        let parts = self
            .metadata
            .get(ASSISTANT_MESSAGE_PARTS_METADATA_KEY)
            .and_then(|value| {
                serde_json::from_value::<Vec<AssistantMessagePart>>(value.clone()).ok()
            })
            .unwrap_or_default();
        if self.role != Role::Assistant || parts.is_empty() {
            return vec![self.clone()];
        }
        let mut base = self.clone();
        base.content.clear();
        base.metadata.remove(ASSISTANT_MESSAGE_PARTS_METADATA_KEY);
        base.metadata.remove(ASSISTANT_PHASE_METADATA_KEY);
        base.metadata.remove(PROVIDER_MESSAGE_ID_METADATA_KEY);
        let mut pending = base.clone();
        let mut messages = Vec::new();
        let mut text_index = 0;
        for content in &self.content {
            let message_part = if content.is_text() {
                let part = parts.iter().find(|part| part.text_index == text_index);
                text_index += 1;
                part
            } else {
                None
            };
            if let Some(part) = message_part {
                if !pending.content.is_empty() {
                    messages.push(pending);
                    pending = base.clone();
                }
                let mut message = base.clone();
                message.content.push(content.clone());
                message.metadata.extend(part.metadata.clone());
                messages.push(message);
            } else {
                pending.content.push(content.clone());
            }
        }
        if !pending.content.is_empty() {
            messages.push(pending);
        }
        messages
    }

    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            id: None,
            role,
            content: vec![ContentPart::Text { text: text.into() }],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        }
    }

    pub fn text_with_id(role: Role, text: impl Into<String>) -> Self {
        Self {
            id: Some(uuid::Uuid::new_v4().to_string()),
            role,
            content: vec![ContentPart::Text { text: text.into() }],
            name: None,
            created_at: Some(Utc::now()),
            metadata: serde_json::Map::new(),
        }
    }

    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|p| p.as_text())
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn thinking_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|p| p.as_thinking())
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_calls(&self) -> Vec<&ToolCall> {
        self.content
            .iter()
            .filter_map(|p| p.as_tool_call())
            .filter(|call| call.has_valid_identity())
            .collect()
    }

    pub fn tool_results(&self) -> Vec<&ToolResult> {
        self.content
            .iter()
            .filter_map(|p| p.as_tool_result())
            .filter(|result| result.has_valid_call_id())
            .collect()
    }

    pub fn has_text(&self) -> bool {
        self.content.iter().any(|p| p.is_text())
    }

    pub fn has_thinking(&self) -> bool {
        self.content.iter().any(|p| p.is_thinking())
    }

    pub fn has_tool_calls(&self) -> bool {
        self.content
            .iter()
            .filter_map(|p| p.as_tool_call())
            .any(|call| call.has_valid_identity())
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}
