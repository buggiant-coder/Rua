use agent_types::conversation_message_identity::MODEL_DIALOGUE_ITEM_ID_METADATA_KEY;
use agent_types::model_dialogue_item::{
    AssistantMessagePart, ASSISTANT_MESSAGE_PARTS_METADATA_KEY, ASSISTANT_PHASE_METADATA_KEY,
    PROVIDER_MESSAGE_ID_METADATA_KEY,
};
use agent_types::{ContentPart, ModelDialogueItem, Role, ToolCall, ToolResult};
use react_agent::tool_result_compressor::compact_projected_tool_results;
use rua_config::AgentsRunningConfig;
use rua_session::{MessageLogEntry, RuntimeNote, RuntimeNotePayload, TaskWatchOutputProjection};
use sha2::{Digest, Sha256};

pub(crate) const TOOL_RESULT_TRUNCATION_EVIDENCE_KEY: &str = "rua_llm_debug_tool_result_truncation";
const SOURCE_MESSAGE_ID_METADATA_KEY: &str = "rua_source_message_id";
const MAX_PROVIDER_TOOL_CALL_ID_BYTES: usize = 64;

pub struct ToolResultProjectionHook {
    recent_n: usize,
    old_max_bytes: usize,
    batch_size: usize,
}

impl ToolResultProjectionHook {
    pub fn from_running_config(running: &AgentsRunningConfig) -> Option<Self> {
        let tool_results = &running.tool_result_compact;
        tool_results.projection_truncation_enabled.then_some(Self {
            recent_n: tool_results.recent_n as usize,
            old_max_bytes: tool_results.old_max_bytes as usize,
            batch_size: tool_results.projection_truncation_batch_size as usize,
        })
    }

    fn apply(&self, messages: &mut [ModelDialogueItem]) -> usize {
        compact_projected_tool_results(messages, self.recent_n, self.old_max_bytes, self.batch_size)
    }

    pub fn projected_messages(&self, messages: &[ModelDialogueItem]) -> Vec<ModelDialogueItem> {
        let mut projected = messages.to_vec();
        let before = tool_result_sizes(&projected);
        self.apply(&mut projected);
        attach_truncation_evidence(&mut projected, before);
        projected
    }
}

struct ToolResultSnapshot {
    bytes: usize,
}

fn tool_result_sizes(
    messages: &[ModelDialogueItem],
) -> std::collections::HashMap<String, ToolResultSnapshot> {
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|part| match part {
            ContentPart::ToolResult { result } => Some((
                result.call_id.clone(),
                ToolResultSnapshot {
                    bytes: model_visible_tool_result_bytes(&result.content),
                },
            )),
            _ => None,
        })
        .collect()
}

fn model_visible_tool_result_bytes(content: &[ContentPart]) -> usize {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => text.len(),
            part => serde_json::to_vec(part).map_or(0, |bytes| bytes.len()),
        })
        .sum()
}

pub(crate) fn project_message_log_entries(
    entries: &[MessageLogEntry],
    running: &AgentsRunningConfig,
) -> Vec<ModelDialogueItem> {
    let messages = project_message_log_entries_for_model(entries);
    project_tool_results_for_model(&messages, running)
}

pub fn project_tool_results_for_model(
    messages: &[ModelDialogueItem],
    running: &AgentsRunningConfig,
) -> Vec<ModelDialogueItem> {
    let mut projected = messages.to_vec();
    let before = tool_result_sizes(&projected);
    bound_tool_results(&mut projected, running);
    if let Some(hook) = ToolResultProjectionHook::from_running_config(running) {
        hook.apply(&mut projected);
    }
    attach_truncation_evidence(&mut projected, before);
    projected
}

pub fn project_message_log_entries_for_model(
    entries: &[MessageLogEntry],
) -> Vec<ModelDialogueItem> {
    let mut observed_tasks = std::collections::HashSet::new();
    let mut observed_notifications = std::collections::HashSet::new();
    let mut messages = Vec::new();
    for entry in entries {
        let mut projected = project_message_log_entry_for_model(entry);
        if let Some(note) = RuntimeNote::from_message_log_entry(entry) {
            if let RuntimeNotePayload::TaskAttemptTerminal(task) = &note.payload {
                if !observed_notifications.insert(note.runtime_note_id.clone()) {
                    continue;
                }
                if observed_tasks.contains(&(task.task_id.clone(), u64::from(task.attempt_no))) {
                    for message in &mut projected {
                        for part in &mut message.content {
                            if let ContentPart::ToolResult { result } = part {
                                result.content = vec![ContentPart::Text { text: serde_json::json!({
                                    "status":task.terminal_status, "output_already_observed":true,
                                    "read":{"task_id":task.task_id},
                                }).to_string() }];
                            }
                        }
                    }
                }
            }
        }
        for message in &projected {
            for result in message.tool_results() {
                if result.name.as_deref() != Some("task_read") || result.truncation.is_some() {
                    continue;
                }
                if let Some(observation) = result.metadata.get("task_terminal_observation") {
                    if let (Some(id), Some(attempt)) = (
                        observation["task_id"].as_str(),
                        observation["attempt_no"].as_u64(),
                    ) {
                        observed_tasks.insert((id.to_string(), attempt));
                    }
                }
            }
        }
        messages.extend(projected);
    }
    llm_multiplex::request::normalize_tool_history(&aggregate_model_dialogue_items(messages))
}

fn project_message_log_entry_for_model(message: &MessageLogEntry) -> Vec<ModelDialogueItem> {
    if message.role == "ui"
        || message
            .metadata
            .get("ui_only")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        || message
            .metadata
            .get("model_visible")
            .and_then(serde_json::Value::as_bool)
            .is_some_and(|visible| !visible)
    {
        return Vec::new();
    }
    let Some(note) = RuntimeNote::from_message_log_entry(message) else {
        return vec![message_log_entry_to_chat_message(message)];
    };
    match &note.payload {
        RuntimeNotePayload::CollaborationResult(result) => {
            collaboration_observation_messages(message, &note, result)
        }
        RuntimeNotePayload::TaskWatchOutput(task)
            if task.projection == TaskWatchOutputProjection::ActivityOnly =>
        {
            Vec::new()
        }
        RuntimeNotePayload::TaskWatchOutput(task) => task_observation_messages(
            message,
            &note,
            "watch_output",
            &task.task_id,
            task.attempt_no,
            false,
        ),
        RuntimeNotePayload::TaskAttemptTerminal(task) => task_observation_messages(
            message,
            &note,
            "terminal",
            &task.task_id,
            task.attempt_no,
            task.terminal_status != "completed",
        ),
        _ => vec![message_log_entry_to_chat_message(message)],
    }
}

fn collaboration_observation_messages(
    source: &MessageLogEntry,
    note: &RuntimeNote,
    collaboration: &rua_session::CollaborationResultRuntimeNote,
) -> Vec<ModelDialogueItem> {
    let call_id = runtime_observation_call_id(&note.runtime_note_id);
    let tool_name = "Chat_Notification";
    let mut metadata = source.metadata.clone();
    metadata.insert(
        SOURCE_MESSAGE_ID_METADATA_KEY.to_string(),
        serde_json::Value::String(source.message_id.clone()),
    );
    metadata.insert(
        "runtime_generated".to_string(),
        serde_json::Value::Bool(true),
    );
    let body = if collaboration.results.is_empty() {
        rua_session::collaboration::collaboration_result_page(
            &collaboration.request_id,
            &collaboration.result_id,
            &collaboration.status,
            &collaboration.output,
            collaboration.partial,
            0,
        )
        .expect("initial result offset is valid")
    } else {
        serde_json::json!({"status": collaboration.status, "partial": collaboration.partial, "results": collaboration.results})
    };
    let mut result = ToolResult::text(call_id.clone(), body.to_string());
    result.name = Some(tool_name.to_string());
    result.is_error = collaboration.status != "completed";
    let created_at = chrono::DateTime::parse_from_rfc3339(&source.created_at)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc));
    vec![
        ModelDialogueItem {
            id: Some(format!("{}:tool-use", source.message_id)),
            role: Role::Assistant,
            content: vec![ContentPart::ToolUse {
                call: ToolCall::new(
                    call_id.clone(),
                    tool_name,
                    serde_json::json!({
                        "event": "result",
                        "request_id": collaboration.request_id,
                        "result_id": collaboration.result_id,
                        "chat": collaboration.target,
                    }),
                ),
            }],
            name: None,
            created_at,
            metadata: metadata.clone(),
        },
        ModelDialogueItem {
            id: Some(format!("{}:tool-result", source.message_id)),
            role: Role::Tool,
            content: vec![ContentPart::ToolResult { result }],
            name: Some(tool_name.to_string()),
            created_at,
            metadata,
        },
    ]
}

fn task_observation_messages(
    source: &MessageLogEntry,
    note: &RuntimeNote,
    event: &str,
    task_id: &str,
    attempt_no: u32,
    is_error: bool,
) -> Vec<ModelDialogueItem> {
    let call_id = runtime_observation_call_id(&note.runtime_note_id);
    let tool_name = "Task_Notification";
    let mut metadata = source.metadata.clone();
    metadata.insert(
        SOURCE_MESSAGE_ID_METADATA_KEY.to_string(),
        serde_json::Value::String(source.message_id.clone()),
    );
    metadata.insert(
        "runtime_generated".to_string(),
        serde_json::Value::Bool(true),
    );
    let text = match &note.payload {
        RuntimeNotePayload::TaskAttemptTerminal(task) => serde_json::json!({
            "status": task.terminal_status,
            "preview": task.output.chars().take(2000).collect::<String>(),
            "read": {"task_id": task.task_id},
        })
        .to_string(),
        _ => note.render_for_model(),
    };
    let mut result = ToolResult::text(call_id.clone(), text);
    result.name = Some(tool_name.to_string());
    result.is_error = is_error;
    vec![
        ModelDialogueItem {
            id: Some(format!("{}:tool-use", source.message_id)),
            role: Role::Assistant,
            content: vec![ContentPart::ToolUse {
                call: ToolCall::new(
                    call_id.clone(),
                    tool_name,
                    serde_json::json!({
                        "event": event,
                        "task_id": task_id,
                        "attempt_no": attempt_no,
                    }),
                ),
            }],
            name: None,
            created_at: chrono::DateTime::parse_from_rfc3339(&source.created_at)
                .ok()
                .map(|value| value.with_timezone(&chrono::Utc)),
            metadata: metadata.clone(),
        },
        ModelDialogueItem {
            id: Some(format!("{}:tool-result", source.message_id)),
            role: Role::Tool,
            content: vec![ContentPart::ToolResult { result }],
            name: Some(tool_name.to_string()),
            created_at: chrono::DateTime::parse_from_rfc3339(&source.created_at)
                .ok()
                .map(|value| value.with_timezone(&chrono::Utc)),
            metadata,
        },
    ]
}

fn runtime_observation_call_id(runtime_note_id: &str) -> String {
    let readable = format!("runtime-note:{runtime_note_id}");
    if readable.len() <= MAX_PROVIDER_TOOL_CALL_ID_BYTES {
        return readable;
    }

    let digest = format!("{:x}", Sha256::digest(readable.as_bytes()));
    let digest_len = MAX_PROVIDER_TOOL_CALL_ID_BYTES - "runtime-note:".len();
    format!("runtime-note:{}", &digest[..digest_len])
}

fn bound_tool_results(messages: &mut [ModelDialogueItem], running: &AgentsRunningConfig) {
    let configured_max_tokens = running.tool_result_compact.execution_output_max_tokens as usize;
    let max_tokens = if configured_max_tokens > 0 {
        configured_max_tokens
    } else {
        rua_core::tool::DEFAULT_TOOL_EXECUTION_OUTPUT_MAX_TOKENS
    };
    let divisor = running.context_compact.token_count_estimate_divisor;
    for message in messages {
        for part in &mut message.content {
            let ContentPart::ToolResult { result } = part else {
                continue;
            };
            if result.truncation.is_some() {
                continue;
            }
            let text = result.text_content();
            let tool_name = result.name.as_deref().unwrap_or("Tool");
            let (compressed, truncation) = rua_tool::result_store::compress_output_tokens(
                &text, max_tokens, tool_name, None, divisor,
            );
            let Some(truncation) = truncation else {
                continue;
            };
            let mut content = vec![ContentPart::Text { text: compressed }];
            content.extend(
                result
                    .content
                    .iter()
                    .filter(|part| !part.is_text())
                    .cloned(),
            );
            result.content = content;
            result.truncation = Some(truncation);
        }
    }
}

fn attach_truncation_evidence(
    projected: &mut [ModelDialogueItem],
    before: std::collections::HashMap<String, ToolResultSnapshot>,
) {
    let after = tool_result_sizes(projected);
    let mut evidence = before
        .into_iter()
        .filter_map(|(call_id, before)| {
            let after = after.get(&call_id)?;
            (before.bytes != after.bytes).then(|| {
                serde_json::json!({
                    "call_id": call_id,
                    "before_bytes": before.bytes,
                    "after_bytes": after.bytes,
                    "kind": "truncated",
                })
            })
        })
        .collect::<Vec<_>>();
    evidence.sort_by(|left, right| left["call_id"].as_str().cmp(&right["call_id"].as_str()));
    if !evidence.is_empty() {
        if let Some(first) = projected.first_mut() {
            first.metadata.insert(
                TOOL_RESULT_TRUNCATION_EVIDENCE_KEY.into(),
                serde_json::Value::Array(evidence),
            );
        }
    }
}

pub(crate) fn aggregate_model_dialogue_items(
    messages: Vec<ModelDialogueItem>,
) -> Vec<ModelDialogueItem> {
    let mut aggregated: Vec<ModelDialogueItem> = Vec::with_capacity(messages.len());
    for message in messages {
        let dialogue_item_id = message
            .metadata
            .get(MODEL_DIALOGUE_ITEM_ID_METADATA_KEY)
            .and_then(serde_json::Value::as_str);
        let merge_with_previous = dialogue_item_id.is_some_and(|dialogue_item_id| {
            aggregated.last().is_some_and(|previous| {
                previous.role == Role::Assistant
                    && message.role == Role::Assistant
                    && previous
                        .metadata
                        .get(MODEL_DIALOGUE_ITEM_ID_METADATA_KEY)
                        .and_then(serde_json::Value::as_str)
                        == Some(dialogue_item_id)
            })
        });
        if merge_with_previous {
            let previous = aggregated
                .last_mut()
                .expect("previous item exists when aggregation is selected");
            let mut parts = assistant_message_parts(previous);
            let offset = previous
                .content
                .iter()
                .filter(|part| part.is_text())
                .count();
            parts.extend(
                assistant_message_parts(&message)
                    .into_iter()
                    .map(|mut part| {
                        part.text_index += offset;
                        part
                    }),
            );
            previous.content.extend(message.content);
            if !parts.is_empty() {
                previous.metadata.remove(ASSISTANT_PHASE_METADATA_KEY);
                previous.metadata.remove(PROVIDER_MESSAGE_ID_METADATA_KEY);
                previous.metadata.insert(
                    ASSISTANT_MESSAGE_PARTS_METADATA_KEY.into(),
                    serde_json::to_value(parts).expect("assistant message metadata"),
                );
            }
            if let Some(usage) = message
                .metadata
                .get(crate::turn_persistence::RUA_TOKEN_USAGE_METADATA_KEY)
            {
                previous.metadata.insert(
                    crate::turn_persistence::RUA_TOKEN_USAGE_METADATA_KEY.to_string(),
                    usage.clone(),
                );
            }
        } else {
            aggregated.push(message);
        }
    }
    aggregated
}

fn assistant_message_parts(message: &ModelDialogueItem) -> Vec<AssistantMessagePart> {
    if let Some(parts) = message.metadata.get(ASSISTANT_MESSAGE_PARTS_METADATA_KEY) {
        return serde_json::from_value(parts.clone()).unwrap_or_default();
    }
    if !message.metadata.contains_key(ASSISTANT_PHASE_METADATA_KEY)
        && !message
            .metadata
            .contains_key(PROVIDER_MESSAGE_ID_METADATA_KEY)
    {
        return Vec::new();
    }
    message
        .content
        .iter()
        .filter(|part| part.is_text())
        .enumerate()
        .map(|(text_index, _)| AssistantMessagePart {
            text_index,
            metadata: message.metadata.clone(),
        })
        .collect()
}

pub(crate) fn message_log_entry_to_chat_message(message: &MessageLogEntry) -> ModelDialogueItem {
    let content = match &message.content {
        serde_json::Value::Array(parts) => parts
            .iter()
            .cloned()
            .map(rua_core::message::content_part_from_value)
            .collect(),
        value => vec![rua_core::message::content_part_from_value(value.clone())],
    };
    let role = match message.role.as_str() {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        "developer" => Role::Developer,
        "system" if is_legacy_turn_aborted_marker(message) => Role::User,
        _ => Role::System,
    };
    let mut metadata = message.metadata.clone();
    metadata.insert(
        SOURCE_MESSAGE_ID_METADATA_KEY.to_string(),
        serde_json::Value::String(message.message_id.clone()),
    );
    ModelDialogueItem {
        id: Some(message.message_id.clone()),
        role,
        content,
        name: message
            .metadata
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        created_at: chrono::DateTime::parse_from_rfc3339(&message.created_at)
            .ok()
            .map(|value| value.with_timezone(&chrono::Utc)),
        metadata,
    }
}

fn is_legacy_turn_aborted_marker(message: &MessageLogEntry) -> bool {
    message
        .metadata
        .get("model_visible_marker")
        .and_then(serde_json::Value::as_str)
        == Some("turn_aborted")
        || message.content.as_array().is_some_and(|parts| {
            parts.iter().any(|part| {
                part.get("text")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| text.contains("<turn_aborted>"))
            })
        })
}
