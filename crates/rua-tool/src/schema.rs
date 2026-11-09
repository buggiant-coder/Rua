use agent_types::ToolDefinition;
use serde::{Deserialize, Serialize};

use crate::annotations::ExtendedToolAnnotations;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,

    pub description: String,

    pub parameters: serde_json::Value,

    pub annotations: ExtendedToolAnnotations,

    #[serde(default = "default_namespace")]
    pub namespace: String,
}

fn default_namespace() -> String {
    "builtin".into()
}

impl ToolSpec {
    pub fn to_tool_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
            strict: None,
            annotations: self.annotations.to_tool_annotations(),
            metadata: self.annotations.to_tool_metadata(),
        }
    }
}

pub fn validate_args(
    tool_name: &str,
    args: &serde_json::Value,
    schema: &serde_json::Value,
) -> Result<(), crate::error::ToolError> {
    validate_value(args, schema, "$", tool_name)
}

fn validate_value(
    value: &serde_json::Value,
    schema: &serde_json::Value,
    path: &str,
    tool_name: &str,
) -> Result<(), crate::error::ToolError> {
    if let Some(expected) = schema.get("type").and_then(serde_json::Value::as_str) {
        let matches = match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "null" => value.is_null(),
            _ => true,
        };
        if !matches {
            return schema_error(
                tool_name,
                if path == "$" {
                    format!("expected {expected}, got {}", json_type_name(value))
                } else {
                    format!("{path}: expected {expected}, got {}", json_type_name(value))
                },
            );
        }
    }

    if let Some(allowed) = schema.get("enum").and_then(serde_json::Value::as_array) {
        if !allowed.contains(value) {
            return schema_error(
                tool_name,
                format!("{path}: value is not in the allowed enum"),
            );
        }
    }

    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
            for field in required.iter().filter_map(serde_json::Value::as_str) {
                if !object.contains_key(field) {
                    return schema_error(tool_name, format!("missing required field: {field}"));
                }
            }
        }
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object);
        for (field, field_value) in object {
            match properties.and_then(|properties| properties.get(field)) {
                Some(field_schema) => {
                    validate_value(
                        field_value,
                        field_schema,
                        &format!("{path}.{field}"),
                        tool_name,
                    )?;
                }
                None if schema
                    .get("additionalProperties")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false) =>
                {
                    return schema_error(tool_name, format!("unknown field at {path}.{field}"));
                }
                None => {}
            }
        }
    }

    if let (Some(items), Some(values)) = (schema.get("items"), value.as_array()) {
        for (index, item) in values.iter().enumerate() {
            validate_value(item, items, &format!("{path}[{index}]"), tool_name)?;
        }
    }
    Ok(())
}

fn schema_error<T>(tool_name: &str, message: String) -> Result<T, crate::error::ToolError> {
    Err(crate::error::ToolError::SchemaValidation {
        tool: tool_name.to_string(),
        message,
    })
}

fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}
