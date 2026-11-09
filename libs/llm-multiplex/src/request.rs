use std::collections::HashSet;

use agent_types::{ContentPart, ModelDialogueItem, Role, StructuredOutput, ToolCall, ToolResult};

const MISSING_TOOL_RESULT: &str = "Tool execution was interrupted before producing a result.";

pub fn create_format_tool(structured: &StructuredOutput) -> serde_json::Value {
    let mut schema = structured.schema.clone();
    remove_title_field(&mut schema);

    serde_json::json!({
        "type": "function",
        "function": {
            "name": structured.tool_name(),
            "description": "Generate the required structured output with this function",
            "parameters": schema,
        }
    })
}

pub fn create_anthropic_format_tool(structured: &StructuredOutput) -> serde_json::Value {
    let mut schema = structured.schema.clone();
    remove_title_field(&mut schema);

    serde_json::json!({
        "name": structured.tool_name(),
        "description": "Generate the required structured output with this function",
        "input_schema": schema,
    })
}

pub fn remove_title_field_public(value: &mut serde_json::Value) {
    remove_title_field(value);
}

fn remove_title_field(value: &mut serde_json::Value) {
    if let Some(obj) = value.as_object_mut() {
        obj.remove("title");
        for v in obj.values_mut() {
            remove_title_field(v);
        }
    } else if let Some(arr) = value.as_array_mut() {
        for v in arr.iter_mut() {
            remove_title_field(v);
        }
    }
}

pub fn sanitize_boolean_schemas(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(obj) => {
            if obj.get("additionalProperties") == Some(&serde_json::Value::Bool(true)) {
                obj.insert(
                    "additionalProperties".to_string(),
                    serde_json::Value::Object(serde_json::Map::new()),
                );
            }
            if obj.get("items") == Some(&serde_json::Value::Bool(true)) {
                obj.insert(
                    "items".to_string(),
                    serde_json::Value::Object(serde_json::Map::new()),
                );
            }
            if matches!(obj.get("required"), Some(serde_json::Value::Bool(_))) {
                obj.remove("required");
            }
            for v in obj.values_mut() {
                sanitize_boolean_schemas(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                sanitize_boolean_schemas(v);
            }
        }
        _ => {}
    }
}

pub(crate) fn dedupe_tool_uses_in_message(message: &ModelDialogueItem) -> ModelDialogueItem {
    let mut seen = HashSet::new();
    let content = message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::ToolUse { call } if call.has_valid_identity() => {
                if seen.insert(call.id.clone()) {
                    Some(part.clone())
                } else {
                    None
                }
            }
            ContentPart::ToolUse { .. } => None,
            _ => Some(part.clone()),
        })
        .collect();

    ModelDialogueItem {
        content,
        ..message.clone()
    }
}

/// Close model-input Tool groups before measuring or serializing a projection.
/// Runtime observations may arrive while a real call is pending; they belong
/// after that call's results, not at a new model-call boundary.
pub fn normalize_tool_history(messages: &[ModelDialogueItem]) -> Vec<ModelDialogueItem> {
    let messages = defer_runtime_observations(messages);
    let messages: Vec<_> = messages
        .into_iter()
        .map(dedupe_tool_uses_in_message)
        .collect();
    let mut normalized = Vec::new();
    let mut index = 0;

    while index < messages.len() {
        let message = &messages[index];
        let calls: Vec<ToolCall> = message
            .content
            .iter()
            .filter_map(|part| match part {
                ContentPart::ToolUse { call } => Some(call.clone()),
                _ => None,
            })
            .collect();

        if calls.is_empty() {
            if let Some(message) = without_tool_results(message) {
                normalized.push(message);
            }
            index += 1;
            continue;
        }

        if let Some(call_message) = without_tool_results(message) {
            normalized.push(call_message);
        }

        let mut available_results = valid_tool_results(message);
        let mut residual_messages = Vec::new();
        let mut next = index + 1;
        while next < messages.len() {
            let following = &messages[next];
            if following
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::ToolUse { .. }))
            {
                break;
            }

            let results = valid_tool_results(following);
            if results.is_empty() {
                break;
            }
            available_results.extend(results);
            if let Some(residual) = without_tool_results(following) {
                residual_messages.push(residual);
            }
            next += 1;
        }

        for call in calls {
            let aliases = [Some(call.id.as_str()), call.provider_call_id.as_deref()];
            let result_index = available_results.iter().position(|message| {
                message.tool_results().iter().any(|result| {
                    aliases
                        .iter()
                        .flatten()
                        .any(|alias| *alias == result.call_id)
                })
            });
            let wire_call_id = call
                .provider_call_id
                .clone()
                .unwrap_or_else(|| call.id.clone());
            let mut result_message = match result_index {
                Some(result_index) => available_results.remove(result_index),
                None => ModelDialogueItem {
                    content: vec![ContentPart::ToolResult {
                        result: ToolResult::error(wire_call_id.clone(), MISSING_TOOL_RESULT),
                    }],
                    ..ModelDialogueItem::text(Role::Tool, "")
                },
            };
            if let ContentPart::ToolResult { result } = &mut result_message.content[0] {
                result.call_id = wire_call_id;
                if result.name.is_none() {
                    result.name = Some(call.name.clone());
                }
                result_message.name = result.name.clone();
            }
            normalized.push(result_message);
        }

        normalized.extend(residual_messages);
        index = next;
    }

    normalized
}

fn defer_runtime_observations(messages: &[ModelDialogueItem]) -> Vec<&ModelDialogueItem> {
    let mut ordered = Vec::with_capacity(messages.len());
    let mut deferred = Vec::new();
    let mut pending: Vec<&ToolCall> = Vec::new();
    for message in messages {
        if !pending.is_empty()
            && message
                .metadata
                .get("runtime_generated")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        {
            deferred.push(message);
            continue;
        }
        // A genuine new assistant/user/guidance boundary still closes an
        // incomplete group. Never search a later turn for a reused call id.
        if message.has_tool_calls() || message.tool_results().is_empty() {
            ordered.append(&mut deferred);
            pending.clear();
        }
        pending.extend(message.tool_calls());
        for result in message.tool_results() {
            pending.retain(|call| {
                call.id != result.call_id
                    && call.provider_call_id.as_deref() != Some(result.call_id.as_str())
            });
        }
        ordered.push(message);
        if pending.is_empty() {
            ordered.append(&mut deferred);
        }
    }
    ordered.append(&mut deferred);
    ordered
}

fn valid_tool_results(message: &ModelDialogueItem) -> Vec<ModelDialogueItem> {
    message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::ToolResult { result } if result.has_valid_call_id() => {
                Some(ModelDialogueItem {
                    role: Role::Tool,
                    content: vec![part.clone()],
                    ..message.clone()
                })
            }
            _ => None,
        })
        .collect()
}

fn without_tool_results(message: &ModelDialogueItem) -> Option<ModelDialogueItem> {
    let content: Vec<_> = message
        .content
        .iter()
        .filter(|part| !matches!(part, ContentPart::ToolResult { .. }))
        .cloned()
        .collect();
    (!content.is_empty()).then(|| ModelDialogueItem {
        content,
        ..message.clone()
    })
}
