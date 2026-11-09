use agent_types::ToolResult;
use react_agent::tool_disclosure::{
    assemble_tool_definitions, estimate_tool_schema_tokens, ToolDisclosureMode,
};
use rua_config::AgentsRunningConfig;
use rua_contract::agent::AgentInputMessage;
use rua_core::message::ContentPart;
use rua_session::{
    ContextProjection, ContextTargetBand, MessageLogEntry, ProjectionMessageGroupEstimate,
    WorkspaceSessionStore,
};
use sha2::{Digest, Sha256};

pub fn context_target_band_from_running(running: &AgentsRunningConfig) -> ContextTargetBand {
    context_target_band_from_running_max_input(
        running,
        running.max_input_length.max(1) as u64,
        true,
    )
}

pub fn context_target_band_for_model(
    running: &AgentsRunningConfig,
    context_window: u64,
) -> ContextTargetBand {
    context_target_band_from_running_max_input(running, context_window.max(1), true)
}

pub fn blocking_context_target_band_from_running(
    running: &AgentsRunningConfig,
) -> ContextTargetBand {
    context_target_band_from_running_max_input(
        running,
        running.max_input_length.max(1) as u64,
        false,
    )
}

pub fn target_band_version(target_band: &ContextTargetBand) -> String {
    format!(
        "running:{}:{}:{}",
        target_band.upper_target_tokens,
        target_band.emergency_limit_tokens,
        target_band.tail_reserve_tokens,
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemContextAccounting {
    pub system_context_hash: String,
    pub system_context_tokens: u64,
    pub visible_tool_schema_tokens: u64,
    pub capability_index_tokens: u64,
    pub disclosure_mode: ToolDisclosureMode,
    pub disclosure_activated: bool,
}

pub fn build_system_context_accounting(
    system_prompt_fragments: Vec<String>,
    skill_prompt_block: Option<String>,
    toolkit: Option<&react_agent::AgentToolkit>,
    running_config: Option<&AgentsRunningConfig>,
    context_window_tokens: Option<usize>,
) -> SystemContextAccounting {
    let disclosure_config = running_config
        .map(rua_agent::tool_disclosure::react_tool_disclosure_config)
        .unwrap_or_default();
    let context_window_tokens = context_window_tokens
        .or_else(|| running_config.map(|running| running.max_input_length.max(1) as usize));

    let mut hash_parts = serde_json::Map::new();
    let system_fragments: Vec<String> = system_prompt_fragments
        .into_iter()
        .filter(|fragment| !fragment.trim().is_empty())
        .collect();
    let system_fragment_tokens: u64 = system_fragments
        .iter()
        .map(|fragment| estimate_string_tokens(fragment))
        .sum();
    let skill_prompt_tokens = skill_prompt_block
        .as_deref()
        .filter(|block| !block.trim().is_empty())
        .map(estimate_string_tokens)
        .unwrap_or_default();

    hash_parts.insert(
        "system_fragments".to_string(),
        serde_json::json!(system_fragments),
    );
    hash_parts.insert(
        "skill_prompt_block".to_string(),
        serde_json::json!(skill_prompt_block),
    );

    let mut visible_tool_schema_tokens = 0u64;
    let mut capability_index_tokens = 0u64;
    let mut disclosure_activated = false;

    if let Some(toolkit) = toolkit {
        let assembly =
            assemble_tool_definitions(toolkit, &disclosure_config, context_window_tokens);
        disclosure_activated = assembly.activated;
        visible_tool_schema_tokens = assembly
            .tools
            .iter()
            .map(|definition| estimate_tool_schema_tokens(definition) as u64)
            .sum();

        let mut visible_tool_names: Vec<String> = assembly
            .tools
            .iter()
            .map(|definition| definition.name.clone())
            .collect();
        visible_tool_names.sort();

        let mut direct_tool_names = assembly.direct_tool_names;
        direct_tool_names.sort();
        let mut deferred_tool_names = assembly.deferred_tool_names;
        deferred_tool_names.sort();

        let capability_index = if assembly.activated {
            let index = format!(
                "deferred_categories:{};deferred_count:{}",
                deferred_tool_categories(toolkit).join(","),
                deferred_tool_names.len()
            );
            capability_index_tokens = estimate_string_tokens(&index);
            Some(index)
        } else {
            None
        };

        hash_parts.insert(
            "tool_disclosure".to_string(),
            serde_json::json!({
                "mode": disclosure_mode_name(disclosure_config.enabled),
                "activated": assembly.activated,
                "threshold_pct": disclosure_config.threshold_pct,
                "search_default_limit": disclosure_config.search_default_limit,
                "search_max_limit": disclosure_config.search_max_limit,
                "direct_tool_names": direct_tool_names,
                "deferred_tool_names": deferred_tool_names,
                "visible_tool_names": visible_tool_names,
                "visible_tool_schema_tokens": visible_tool_schema_tokens,
                "capability_index": capability_index,
            }),
        );
    } else {
        hash_parts.insert(
            "tool_disclosure".to_string(),
            serde_json::json!({
                "mode": disclosure_mode_name(disclosure_config.enabled),
                "activated": false,
            }),
        );
    }

    let hash_payload = serde_json::Value::Object(hash_parts);
    let hash = stable_hash_value(&hash_payload);
    SystemContextAccounting {
        system_context_hash: hash,
        system_context_tokens: system_fragment_tokens
            .saturating_add(skill_prompt_tokens)
            .saturating_add(visible_tool_schema_tokens)
            .saturating_add(capability_index_tokens),
        visible_tool_schema_tokens,
        capability_index_tokens,
        disclosure_mode: disclosure_config.enabled,
        disclosure_activated,
    }
}

pub fn early_compaction_total_tokens(
    projection: &ContextProjection,
    estimated_projected_conversation_tokens: u64,
    estimated_system_context_tokens: u64,
) -> u64 {
    let estimated_total =
        estimated_projected_conversation_tokens.saturating_add(estimated_system_context_tokens);
    let previous = projection
        .retained_messages
        .iter()
        .chain(projection.tail_messages.iter())
        .rev()
        .find_map(|message| {
            let estimate = message
                .metadata
                .get(react_agent::MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY)
                .cloned()
                .and_then(|value| {
                    serde_json::from_value::<react_agent::ModelInputTokenEstimate>(value).ok()
                })?;
            let prompt_tokens = message
                .metadata
                .get(rua_agent::turn_persistence::RUA_TOKEN_USAGE_METADATA_KEY)
                .and_then(|usage| usage.get("last_call_usage"))
                .and_then(|usage| usage.get("prompt_tokens"))
                .and_then(serde_json::Value::as_u64)?;
            Some((estimate, prompt_tokens))
        });
    let Some((previous, previous_provider_prompt_tokens)) = previous else {
        return estimated_total;
    };
    if estimated_projected_conversation_tokens >= previous.conversation_estimate_tokens {
        previous_provider_prompt_tokens.saturating_add(
            estimated_projected_conversation_tokens - previous.conversation_estimate_tokens,
        )
    } else {
        previous_provider_prompt_tokens.saturating_sub(
            previous.conversation_estimate_tokens - estimated_projected_conversation_tokens,
        )
    }
}

pub async fn projection_message_group_estimates(
    store: &WorkspaceSessionStore,
    chat_id: &str,
) -> rua_core::Result<Vec<ProjectionMessageGroupEstimate>> {
    store.projection_message_group_estimates(chat_id).await
}

fn estimate_string_tokens(text: &str) -> u64 {
    (text.len() as u64 / 4).max(1)
}

fn stable_hash_value(value: &serde_json::Value) -> String {
    let serialized = serde_json::to_vec(value).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(serialized);
    format!("sha256:{:x}", hasher.finalize())
}

fn disclosure_mode_name(mode: ToolDisclosureMode) -> &'static str {
    match mode {
        ToolDisclosureMode::Auto => "auto",
        ToolDisclosureMode::On => "on",
        ToolDisclosureMode::Off => "off",
    }
}

fn deferred_tool_categories(toolkit: &react_agent::AgentToolkit) -> Vec<String> {
    let mut categories: Vec<String> = toolkit
        .disclosure_inventory()
        .into_iter()
        .filter(|entry| {
            entry.disclosure.class == react_agent::toolkit::DisclosureClass::DeferredCandidate
        })
        .filter_map(|entry| entry.disclosure.category)
        .collect();
    categories.sort();
    categories.dedup();
    categories
}

pub fn estimate_log_message_tokens(messages: &[MessageLogEntry]) -> u64 {
    rua_agent::projection_preparation::project_message_log_entries_for_model(messages)
        .iter()
        .map(|message| {
            serde_json::to_string(&message.content)
                .map(|content| (content.len() as u64 / 4).max(1))
                .unwrap_or(1)
        })
        .sum()
}

pub fn build_context_usage_snapshot_from_projection(
    estimated_tokens: u64,
    running_config: Option<&AgentsRunningConfig>,
    system_context: &SystemContextAccounting,
    projection: &ContextProjection,
    current_user_input: Option<&str>,
    used_emergency_bounded_projection: bool,
) -> Option<serde_json::Value> {
    let max_input_length = running_config?.max_input_length.max(1) as u64;
    let pending_user_input_tokens = current_user_input
        .map(estimate_string_tokens)
        .unwrap_or_default();
    let context_usage_ratio = (estimated_tokens as f64 / max_input_length as f64) * 100.0;
    Some(serde_json::json!({
        "estimated_tokens": estimated_tokens,
        "max_input_length": max_input_length,
        "context_usage_ratio": context_usage_ratio,
        "source": "projection",
        "breakdown": {
            "system_context_tokens": system_context.system_context_tokens,
            "visible_tool_schema_tokens": system_context.visible_tool_schema_tokens,
            "capability_index_tokens": system_context.capability_index_tokens,
            "projected_conversation_tokens": projection.stable_token_estimate,
            "pending_user_input_tokens": pending_user_input_tokens,
            "summary_tokens": projection.summary.as_deref().map(estimate_string_tokens).unwrap_or_default(),
            "retained_message_count": projection.retained_messages.len(),
            "tail_message_count": projection.tail_messages.len(),
            "used_emergency_bounded_projection": used_emergency_bounded_projection,
        }
    }))
}

pub fn message_log_entries_to_input_messages(
    messages: &[MessageLogEntry],
) -> Vec<AgentInputMessage> {
    let projected =
        rua_agent::projection_preparation::project_message_log_entries_for_model(messages);
    let role_text_pairs: Vec<(String, String)> = projected
        .iter()
        .map(|message| {
            (
                message.role.to_string(),
                render_content_value(
                    &serde_json::to_value(&message.content).unwrap_or(serde_json::Value::Null),
                ),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str)> = role_text_pairs
        .iter()
        .map(|(role, text)| (role.as_str(), text.as_str()))
        .collect();

    let input_messages = rua_command::agent_input_indices(&borrowed)
        .into_iter()
        .filter_map(|index| projected.get(index))
        .map(|message| AgentInputMessage {
            role: message.role.to_string(),
            content: message.content.clone(),
        })
        .collect::<Vec<_>>();

    repair_aborted_tool_rounds(input_messages)
}

pub fn message_log_entry_to_input_message(message: &MessageLogEntry) -> AgentInputMessage {
    let content = match &message.content {
        serde_json::Value::Array(parts) => parts
            .iter()
            .cloned()
            .map(rua_core::message::content_part_from_value)
            .collect(),
        value => vec![rua_core::message::content_part_from_value(value.clone())],
    };
    AgentInputMessage {
        role: message.role.clone(),
        content,
    }
}

fn is_turn_aborted_input_message(message: &AgentInputMessage) -> bool {
    message.content.iter().any(|part| {
        matches!(
            part,
            ContentPart::Text { text } if text.contains("<turn_aborted>")
        )
    })
}

fn synthetic_interrupted_tool_result(call_id: &str, name: Option<&str>) -> AgentInputMessage {
    let mut result = ToolResult::error(
        call_id.to_string(),
        "Tool execution was interrupted because the user stopped the turn before this tool returned.",
    );
    result.name = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    AgentInputMessage {
        role: "tool".to_string(),
        content: vec![ContentPart::ToolResult { result }],
    }
}

fn repair_aborted_tool_rounds(messages: Vec<AgentInputMessage>) -> Vec<AgentInputMessage> {
    let mut repaired = Vec::with_capacity(messages.len());
    let mut pending_tool_calls: Vec<(String, Option<String>)> = Vec::new();

    for mut message in messages {
        if is_turn_aborted_input_message(&message) {
            repaired.extend(pending_tool_calls.drain(..).map(|(call_id, name)| {
                synthetic_interrupted_tool_result(&call_id, name.as_deref())
            }));

            message.role = "user".to_string();
            repaired.push(message);
            continue;
        }

        for part in &message.content {
            match part {
                ContentPart::ToolUse { call } if call.has_valid_identity() => {
                    pending_tool_calls.push((call.id.clone(), Some(call.name.clone())));
                }
                ContentPart::ToolResult { result } if result.has_valid_call_id() => {
                    pending_tool_calls.retain(|(call_id, _)| call_id != &result.call_id);
                }
                _ => {}
            }
        }
        repaired.push(message);
    }

    repaired
}

pub fn message_log_text(message: &MessageLogEntry) -> String {
    render_content_value(&message.content)
}

fn render_content_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Array(parts) => parts
            .iter()
            .map(render_content_value)
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Object(map) => render_content_object(map),
        other => other.to_string(),
    }
}

fn render_content_object(map: &serde_json::Map<String, serde_json::Value>) -> String {
    if let Some(text) = map.get("text").and_then(|text| text.as_str()) {
        return text.to_string();
    }

    match map.get("type").and_then(|value| value.as_str()) {
        Some("thinking") => map
            .get("thinking")
            .or_else(|| map.get("text"))
            .map(render_content_value)
            .unwrap_or_default(),
        Some("tool_use") => render_tool_use_text(map),
        Some("tool_result") => render_tool_result_text(map),
        Some("data") => map
            .get("value")
            .map(render_content_value)
            .unwrap_or_else(|| serde_json::Value::Object(map.clone()).to_string()),
        _ => serde_json::Value::Object(map.clone()).to_string(),
    }
}

fn field_text<'a>(
    primary: &'a serde_json::Map<String, serde_json::Value>,
    fallback: Option<&'a serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Option<&'a str> {
    primary
        .get(key)
        .and_then(|value| value.as_str())
        .or_else(|| fallback.and_then(|map| map.get(key).and_then(|value| value.as_str())))
}

fn field_value<'a>(
    primary: &'a serde_json::Map<String, serde_json::Value>,
    fallback: Option<&'a serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Option<&'a serde_json::Value> {
    primary
        .get(key)
        .or_else(|| fallback.and_then(|map| map.get(key)))
}

fn render_tool_use_text(map: &serde_json::Map<String, serde_json::Value>) -> String {
    let nested = map.get("call").and_then(|value| value.as_object());
    let id = field_text(map, nested, "id").unwrap_or_default();
    let name = field_text(map, nested, "name").unwrap_or_default();
    let input = field_value(map, nested, "input")
        .or_else(|| field_value(map, nested, "arguments"))
        .map(render_content_value)
        .unwrap_or_default();

    format!("Tool call: {name} id={id} input={input}")
}

fn render_tool_result_text(map: &serde_json::Map<String, serde_json::Value>) -> String {
    let nested = map.get("result").and_then(|value| value.as_object());
    let id = field_text(map, nested, "id")
        .or_else(|| field_text(map, nested, "call_id"))
        .unwrap_or_default();
    let name = field_text(map, nested, "name").unwrap_or_default();
    let output = field_value(map, nested, "output")
        .or_else(|| field_value(map, nested, "content"))
        .map(render_content_value)
        .unwrap_or_default();
    let truncation = field_value(map, nested, "truncation")
        .and_then(render_truncation_hint)
        .unwrap_or_default();

    if truncation.is_empty() {
        format!("Tool result: {name} id={id} output={output}")
    } else {
        format!("Tool result: {name} id={id} output={output} {truncation}")
    }
}

fn render_truncation_hint(value: &serde_json::Value) -> Option<String> {
    let object = value.as_object()?;
    if let Some(recall_key) = object.get("recall_key").and_then(|value| value.as_str()) {
        return Some(format!("recall_key={recall_key}"));
    }
    if let Some(hint) = object
        .get("continuation_hint")
        .and_then(|value| value.as_str())
    {
        return Some(format!("continuation={hint}"));
    }
    object
        .get("retry_hint")
        .and_then(|value| value.as_str())
        .map(|hint| format!("retry={hint}"))
}

fn context_target_band_from_running_max_input(
    running: &AgentsRunningConfig,
    max_input: u64,
    allow_early_compaction_target: bool,
) -> ContextTargetBand {
    rua_session::context_target_band_from_running(running, max_input, allow_early_compaction_target)
}
