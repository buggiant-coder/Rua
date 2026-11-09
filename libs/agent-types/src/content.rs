use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::tool::{ToolCall, ToolResult};

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ContentPart {
    Text {
        text: String,
    },

    Thinking {
        text: String,

        signature: Option<String>,
    },

    Image {
        url: String,

        media_type: Option<String>,

        alt: Option<String>,
    },

    Audio {
        url: String,

        media_type: Option<String>,
    },

    Video {
        url: String,

        media_type: Option<String>,
    },

    File {
        url: String,

        name: String,

        media_type: Option<String>,

        size: Option<u64>,
    },

    ToolUse {
        call: ToolCall,
    },

    ToolResult {
        result: ToolResult,
    },

    Data {
        value: serde_json::Value,
    },
}

impl Serialize for ContentPart {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        match self {
            ContentPart::Text { text } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "text")?;
                map.serialize_entry("text", text)?;
                map.end()
            }
            ContentPart::Thinking { text, signature } => {
                let count = if signature.is_some() { 3 } else { 2 };
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("type", "thinking")?;
                map.serialize_entry("thinking", text)?;
                if let Some(sig) = signature {
                    map.serialize_entry("signature", sig)?;
                }
                map.end()
            }
            ContentPart::Image {
                url,
                media_type,
                alt,
            } => {
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("type", "image")?;
                map.serialize_entry("url", url)?;
                map.serialize_entry("media_type", media_type)?;
                map.serialize_entry("alt", alt)?;
                map.end()
            }
            ContentPart::Audio { url, media_type } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("type", "audio")?;
                map.serialize_entry("url", url)?;
                map.serialize_entry("media_type", media_type)?;
                map.end()
            }
            ContentPart::Video { url, media_type } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("type", "video")?;
                map.serialize_entry("url", url)?;
                map.serialize_entry("media_type", media_type)?;
                map.end()
            }
            ContentPart::File {
                url,
                name,
                media_type,
                size,
            } => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("type", "file")?;
                map.serialize_entry("url", url)?;
                map.serialize_entry("name", name)?;
                map.serialize_entry("media_type", media_type)?;
                map.serialize_entry("size", size)?;
                map.end()
            }
            ContentPart::ToolUse { call } => {
                let extra_content = call.metadata.get("extra_content");
                let mut metadata = call.metadata.clone();
                metadata.remove("extra_content");
                let count = 4
                    + usize::from(call.raw_arguments.is_some())
                    + usize::from(extra_content.is_some())
                    + usize::from(!metadata.is_empty());
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("type", "tool_use")?;
                map.serialize_entry("id", &call.id)?;
                map.serialize_entry("name", &call.name)?;
                map.serialize_entry("input", &call.arguments)?;
                if let Some(raw) = &call.raw_arguments {
                    map.serialize_entry("raw_input", raw)?;
                }
                if let Some(extra) = extra_content {
                    map.serialize_entry("extra_content", extra)?;
                }
                if !metadata.is_empty() {
                    map.serialize_entry("metadata", &metadata)?;
                }
                map.end()
            }
            ContentPart::ToolResult { result } => {
                let count = 4
                    + usize::from(result.is_error)
                    + usize::from(result.duration_ms.is_some())
                    + usize::from(result.attachments.is_some())
                    + usize::from(result.truncation.is_some())
                    + usize::from(!result.metadata.is_empty());
                let mut map = serializer.serialize_map(Some(count))?;
                map.serialize_entry("type", "tool_result")?;
                map.serialize_entry("id", &result.call_id)?;
                map.serialize_entry("name", result.name.as_deref().unwrap_or(""))?;
                map.serialize_entry("output", &result.content)?;
                if result.is_error {
                    map.serialize_entry("is_error", &result.is_error)?;
                }
                if let Some(duration_ms) = result.duration_ms {
                    map.serialize_entry("duration_ms", &duration_ms)?;
                }
                if let Some(attachments) = &result.attachments {
                    map.serialize_entry("attachments", attachments)?;
                }
                if let Some(truncation) = &result.truncation {
                    map.serialize_entry("truncation", truncation)?;
                }
                if !result.metadata.is_empty() {
                    map.serialize_entry("metadata", &result.metadata)?;
                }
                map.end()
            }
            ContentPart::Data { value } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "data")?;
                map.serialize_entry("value", value)?;
                map.end()
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentPartDe {
    Text {
        text: String,
    },
    Thinking {
        #[serde(alias = "thinking")]
        text: String,
        #[serde(default)]
        signature: Option<String>,
    },
    Image {
        url: String,
        #[serde(default)]
        media_type: Option<String>,
        #[serde(default)]
        alt: Option<String>,
    },
    Audio {
        url: String,
        #[serde(default)]
        media_type: Option<String>,
    },
    Video {
        url: String,
        #[serde(default)]
        media_type: Option<String>,
    },
    File {
        url: String,
        name: String,
        #[serde(default)]
        media_type: Option<String>,
        #[serde(default)]
        size: Option<u64>,
    },
    ToolUse {
        #[serde(default)]
        call: Option<ToolCall>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        input: Option<serde_json::Value>,
        #[serde(default)]
        raw_input: Option<String>,
        #[serde(default)]
        extra_content: Option<serde_json::Value>,
        #[serde(default)]
        metadata: serde_json::Map<String, serde_json::Value>,
    },
    ToolResult {
        #[serde(default)]
        result: Option<Box<crate::tool::ToolResult>>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default, alias = "output")]
        content: Option<serde_json::Value>,
        #[serde(default)]
        is_error: Option<bool>,
        #[serde(default)]
        duration_ms: Option<u64>,
        #[serde(default)]
        attachments: Option<Vec<crate::tool::ToolAttachment>>,
        #[serde(default)]
        truncation: Option<crate::tool::TruncationMeta>,
        #[serde(default)]
        metadata: serde_json::Map<String, serde_json::Value>,
    },
    Data {
        value: serde_json::Value,
    },
}

impl<'de> Deserialize<'de> for ContentPart {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value =
            serde_json::Value::deserialize(deserializer).map_err(serde::de::Error::custom)?;
        Ok(content_part_from_value(value))
    }
}

fn convert_content_part_de(raw: ContentPartDe) -> ContentPart {
    match raw {
        ContentPartDe::Text { text } => ContentPart::Text { text },
        ContentPartDe::Thinking { text, signature } => ContentPart::Thinking { text, signature },
        ContentPartDe::Image {
            url,
            media_type,
            alt,
        } => ContentPart::Image {
            url,
            media_type,
            alt,
        },
        ContentPartDe::Audio { url, media_type } => ContentPart::Audio { url, media_type },
        ContentPartDe::Video { url, media_type } => ContentPart::Video { url, media_type },
        ContentPartDe::File {
            url,
            name,
            media_type,
            size,
        } => ContentPart::File {
            url,
            name,
            media_type,
            size,
        },
        ContentPartDe::ToolUse {
            call,
            id,
            name,
            input,
            raw_input,
            extra_content,
            metadata,
        } => {
            let mut call = if let Some(call) = call {
                call
            } else {
                let mut call = ToolCall::new(
                    id.unwrap_or_default(),
                    name.unwrap_or_default(),
                    input.unwrap_or(serde_json::Value::Object(Default::default())),
                );
                call.raw_arguments = raw_input;
                call
            };
            for (key, value) in metadata {
                call.metadata.insert(key, value);
            }
            if let Some(extra_content) = extra_content {
                call.metadata.insert("extra_content".into(), extra_content);
            }
            ContentPart::ToolUse { call }
        }
        ContentPartDe::ToolResult {
            result,
            id,
            name,
            content,
            is_error,
            duration_ms,
            attachments,
            truncation,
            metadata,
        } => {
            let result = if let Some(result) = result {
                *result
            } else {
                let content_parts = match content {
                    Some(serde_json::Value::String(s)) => {
                        vec![ContentPart::Text { text: s }]
                    }
                    Some(serde_json::Value::Array(arr)) => arr
                        .into_iter()
                        .filter_map(|v| serde_json::from_value(v).ok())
                        .collect(),
                    Some(other) => {
                        vec![ContentPart::Text {
                            text: other.to_string(),
                        }]
                    }
                    None => vec![],
                };
                crate::tool::ToolResult {
                    call_id: id.unwrap_or_default(),
                    name: name.filter(|n| !n.is_empty()),
                    content: content_parts,
                    is_error: is_error.unwrap_or(false),
                    duration_ms,
                    attachments,
                    truncation,
                    metadata,
                }
            };
            ContentPart::ToolResult { result }
        }
        ContentPartDe::Data { value } => ContentPart::Data { value },
    }
}

pub fn content_part_from_value(value: serde_json::Value) -> ContentPart {
    if let Some(s) = value.as_str() {
        return ContentPart::Text {
            text: s.to_string(),
        };
    }
    if let Some(part) = legacy_source_media_part(&value) {
        return part;
    }
    if let Some(part) = managed_media_part(&value) {
        return part;
    }
    match serde_json::from_value::<ContentPartDe>(value.clone()) {
        Ok(raw) => convert_content_part_de(raw),
        Err(_) => ContentPart::Data { value },
    }
}

fn managed_media_part(value: &serde_json::Value) -> Option<ContentPart> {
    let object = value.as_object()?;
    let part_type = object.get("type")?.as_str()?;
    let media_ref = object.get("media_ref")?.as_str()?;
    if !is_managed_media_ref(media_ref) {
        return None;
    }
    let url = media_ref.to_string();
    match part_type {
        "image" => Some(ContentPart::Image {
            url,
            media_type: None,
            alt: None,
        }),
        "audio" => Some(ContentPart::Audio {
            url,
            media_type: None,
        }),
        "video" => Some(ContentPart::Video {
            url,
            media_type: None,
        }),
        "file" => Some(ContentPart::File {
            url,
            name: String::new(),
            media_type: None,
            size: None,
        }),
        _ => None,
    }
}

pub fn is_managed_media_ref(value: &str) -> bool {
    let Some(filename) = value.strip_prefix("media/") else {
        return false;
    };
    !filename.is_empty()
        && !filename.contains('/')
        && !filename.contains('\\')
        && !filename.contains("..")
        && filename != "."
}

pub fn content_part_to_stored_value(part: &ContentPart) -> serde_json::Value {
    let managed = match part {
        ContentPart::Image { url, .. } => Some(("image", url)),
        ContentPart::Audio { url, .. } => Some(("audio", url)),
        ContentPart::Video { url, .. } => Some(("video", url)),
        ContentPart::File { url, .. } => Some(("file", url)),
        _ => None,
    };
    if let Some((part_type, media_ref)) = managed {
        if is_managed_media_ref(media_ref) {
            return serde_json::json!({
                "type": part_type,
                "media_ref": media_ref,
            });
        }
    }
    serde_json::to_value(part).unwrap_or(serde_json::Value::Null)
}

fn legacy_source_media_part(value: &serde_json::Value) -> Option<ContentPart> {
    let object = value.as_object()?;
    let part_type = object.get("type")?.as_str()?;
    let source = object.get("source")?.as_object()?;
    let url = source.get("url")?.as_str()?.to_string();
    let media_type = source
        .get("media_type")
        .or_else(|| source.get("mime_type"))
        .or_else(|| object.get("media_type"))
        .and_then(|value| value.as_str())
        .map(str::to_string);

    match part_type {
        "image" => Some(ContentPart::Image {
            url,
            media_type,
            alt: object
                .get("alt")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        }),
        "audio" => Some(ContentPart::Audio { url, media_type }),
        "video" => Some(ContentPart::Video { url, media_type }),
        _ => None,
    }
}

impl ContentPart {
    pub fn is_text(&self) -> bool {
        matches!(self, ContentPart::Text { .. })
    }

    pub fn is_thinking(&self) -> bool {
        matches!(self, ContentPart::Thinking { .. })
    }

    pub fn is_tool_use(&self) -> bool {
        matches!(self, ContentPart::ToolUse { .. })
    }

    pub fn is_tool_result(&self) -> bool {
        matches!(self, ContentPart::ToolResult { .. })
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentPart::Text { text } => Some(text),
            _ => None,
        }
    }

    pub fn as_thinking(&self) -> Option<&str> {
        match self {
            ContentPart::Thinking { text, .. } => Some(text),
            _ => None,
        }
    }

    pub fn as_tool_call(&self) -> Option<&ToolCall> {
        match self {
            ContentPart::ToolUse { call } => Some(call),
            _ => None,
        }
    }

    pub fn as_tool_result(&self) -> Option<&crate::tool::ToolResult> {
        match self {
            ContentPart::ToolResult { result } => Some(result),
            _ => None,
        }
    }
}
