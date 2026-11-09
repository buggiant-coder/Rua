use mcp_manager::types::{McpCallResult, McpContent, McpServerId, McpToolInfo};
use rua_core::tool::{ToolResult, ToolSchema};
use rua_core::ContentPart;

pub struct McpToolAdapter;

impl McpToolAdapter {
    pub fn to_tool_schema(info: &McpToolInfo) -> ToolSchema {
        ToolSchema {
            name: info.name.clone(),
            description: info.description.clone(),
            parameters: info.input_schema.clone(),
        }
    }

    pub fn to_prefixed_tool_schema(info: &McpToolInfo, prefix: Option<&str>) -> ToolSchema {
        let name = if let Some(prefix) = prefix {
            format!("{prefix}_{}", info.name)
        } else {
            info.name.clone()
        };

        ToolSchema {
            name,
            description: info.description.clone(),
            parameters: info.input_schema.clone(),
        }
    }

    pub fn validate_args(
        schema: &serde_json::Value,
        args: &serde_json::Value,
    ) -> Result<(), String> {
        if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
            if let Some(obj) = args.as_object() {
                for field in required {
                    if let Some(field_name) = field.as_str() {
                        if !obj.contains_key(field_name) {
                            return Err(format!("missing required field: {field_name}"));
                        }
                    }
                }
            } else if !required.is_empty() {
                return Err("arguments must be an object".to_string());
            }
        }

        if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
            if let Some(args_obj) = args.as_object() {
                for (name, prop_schema) in properties {
                    if let Some(value) = args_obj.get(name) {
                        if let Some(expected_type) =
                            prop_schema.get("type").and_then(|t| t.as_str())
                        {
                            let type_ok = match expected_type {
                                "string" => value.is_string(),
                                "number" => value.is_number(),
                                "integer" => value.is_i64() || value.is_u64(),
                                "boolean" => value.is_boolean(),
                                "array" => value.is_array(),
                                "object" => value.is_object(),
                                "null" => value.is_null(),
                                _ => true,
                            };
                            if !type_ok {
                                return Err(format!(
                                    "field '{name}' expected type '{expected_type}', \
                                     got {}",
                                    value_type_name(value)
                                ));
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn to_core_schema(info: &McpToolInfo, canonical_name: &str) -> ToolSchema {
        ToolSchema {
            name: canonical_name.to_string(),
            description: info.description.clone(),
            parameters: info.input_schema.clone(),
        }
    }

    pub fn canonical_name(server_id: &McpServerId, tool_name: &str) -> String {
        format!("mcp__{}__{}", server_id, tool_name)
    }

    pub fn map_call_result(mcp_result: &McpCallResult) -> ToolResult {
        let mut parts: Vec<ContentPart> = Vec::new();
        for item in &mcp_result.content {
            match item {
                McpContent::Text { text } => {
                    parts.push(ContentPart::Text { text: text.clone() });
                }
                McpContent::Image { data, mime_type } => {
                    let url = format!("data:{mime_type};base64,{data}");
                    parts.push(ContentPart::Image {
                        url,
                        media_type: Some(mime_type.clone()),
                        alt: None,
                    });
                }
                McpContent::Resource { uri, text } => {
                    if let Some(text) = text {
                        parts.push(ContentPart::Text { text: text.clone() });
                    } else {
                        parts.push(ContentPart::File {
                            url: uri.clone(),
                            name: uri.clone(),
                            media_type: None,
                            size: None,
                        });
                    }
                }
            }
        }
        let mut metadata = serde_json::Map::new();
        if let Some(structured) = &mcp_result.structured_content {
            metadata.insert("mcp_structured_content".into(), structured.clone());
            // MCP structured results contain actionable ids and snapshot tokens that
            // the human-readable summary need not repeat. Preserve them for the model.
            let text = structured.to_string();
            if !mcp_result.content.iter().any(|part| {
                matches!(part,
                McpContent::Text { text: existing } if existing == &text)
            }) {
                parts.push(ContentPart::Text { text });
            }
        }
        if parts.is_empty() {
            parts.push(ContentPart::Text {
                text: String::new(),
            });
        }
        ToolResult {
            call_id: "mcp".to_string(),
            name: None,
            content: parts,
            is_error: mcp_result.is_error,
            duration_ms: None,
            attachments: None,
            truncation: None,
            metadata,
        }
    }
}

fn value_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}
