use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::store::{CheckpointModelIdentity, CheckpointSequenceRange, MessageLogEntry};

pub const HISTORY_REPLACEMENT_FORMAT_VERSION: u32 = 3;
pub const HISTORY_REPLACEMENT_PROMPT_VERSION: u32 = 2;
pub const HISTORY_REPLACEMENT_MIN_SOURCE_TOKENS: u64 = 16_000;
pub const HISTORY_REPLACEMENT_MIN_SAVED_TOKENS: u64 = 8_000;
pub const HISTORY_REPLACEMENT_MIN_REDUCTION_PERCENT: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryReplacementContentAnchor {
    pub message_id: String,
    pub content_part_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryReplacementMutationEvidence {
    pub message_id: String,
    pub content_part_index: usize,
    pub before: Value,
    pub after: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryReplacementApplicationEvidence {
    pub record_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub mutations: Vec<HistoryReplacementMutationEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest_carrier: Option<HistoryReplacementContentAnchor>,
    pub removed_message_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryReplacementSource {
    pub turn_id: String,
    pub message_ids: Vec<String>,
    pub start_anchor: HistoryReplacementContentAnchor,
    pub end_anchor: HistoryReplacementContentAnchor,
    pub sequence_range: CheckpointSequenceRange,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_references: Vec<HistoryReplacementToolReference>,
    pub projection_lineage: String,
    pub token_estimate: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryReplacementToolReference {
    pub tool_call_id: String,
    pub tool_name: String,
    pub call_sequence: u64,
    pub result_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recall_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionTraceBlock {
    pub record_id: String,
    pub source: HistoryReplacementSource,
    pub source_content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryReplacementRejectionReason {
    EmptyOutput,
    OutputLimitReached,
    InsufficientSavings,
    InsufficientReduction,
    UnsupportedFinishReason,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryReplacementRejectedGeneration {
    pub model: CheckpointModelIdentity,
    pub reason: HistoryReplacementRejectionReason,
    pub replacement_token_estimate: u64,
    pub saved_token_estimate: u64,
    pub rejected_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HistoryReplacementRecordState {
    Ready {
        digest: String,
        replacement_token_estimate: u64,
        model: CheckpointModelIdentity,
        generated_at: String,
    },
    Rejected {
        #[serde(default)]
        generations: Vec<HistoryReplacementRejectedGeneration>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryReplacementRecord {
    pub record_id: String,
    pub chat_address: rua_core::ChatAddress,
    pub format_version: u32,
    pub prompt_version: u32,
    pub source: HistoryReplacementSource,
    pub state: HistoryReplacementRecordState,
    pub created_at: String,
    pub updated_at: String,
    pub checksum: String,
}

impl HistoryReplacementRecord {
    pub fn ready_digest(&self) -> Option<&str> {
        match &self.state {
            HistoryReplacementRecordState::Ready { digest, .. } => Some(digest),
            HistoryReplacementRecordState::Rejected { .. } => None,
        }
    }

    pub fn replacement_token_estimate(&self) -> Option<u64> {
        match self.state {
            HistoryReplacementRecordState::Ready {
                replacement_token_estimate,
                ..
            } => Some(replacement_token_estimate),
            HistoryReplacementRecordState::Rejected { .. } => None,
        }
    }

    pub fn is_rejected_for_model(&self, model: &CheckpointModelIdentity) -> bool {
        matches!(
            &self.state,
            HistoryReplacementRecordState::Rejected { generations }
                if generations
                    .iter()
                    .any(|generation| generation.model == *model)
        )
    }

    pub fn allows_generation_for_model(&self, model: &CheckpointModelIdentity) -> bool {
        matches!(&self.state, HistoryReplacementRecordState::Rejected { .. })
            && !self.is_rejected_for_model(model)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryReplacementReadyDraft {
    pub record_id: String,
    pub source: HistoryReplacementSource,
    pub digest: String,
    pub replacement_token_estimate: u64,
    pub model: CheckpointModelIdentity,
    pub generated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryReplacementRejectedDraft {
    pub record_id: String,
    pub source: HistoryReplacementSource,
    pub model: CheckpointModelIdentity,
    pub reason: HistoryReplacementRejectionReason,
    pub replacement_token_estimate: u64,
    pub saved_token_estimate: u64,
    pub rejected_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryReplacementCandidateDecision {
    Ready {
        replacement_token_estimate: u64,
        saved_token_estimate: u64,
    },
    Rejected {
        reason: HistoryReplacementRejectionReason,
        replacement_token_estimate: u64,
        saved_token_estimate: u64,
    },
    Retryable {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartKind {
    Process,
    VisibleAssistantText,
    Other,
}

struct PartRef<'a> {
    message: &'a MessageLogEntry,
    message_index: usize,
    part_index: usize,
    value: &'a Value,
    kind: PartKind,
}

pub fn derive_history_replacement_record_id(
    chat_id: &str,
    source: &HistoryReplacementSource,
) -> String {
    #[derive(Serialize)]
    struct HistoryReplacementIdentity<'a> {
        chat_id: &'a str,
        message_ids: &'a [String],
        start_anchor: &'a HistoryReplacementContentAnchor,
        end_anchor: &'a HistoryReplacementContentAnchor,
        format_version: u32,
        prompt_version: u32,
    }

    let identity = HistoryReplacementIdentity {
        chat_id,
        message_ids: &source.message_ids,
        start_anchor: &source.start_anchor,
        end_anchor: &source.end_anchor,
        format_version: HISTORY_REPLACEMENT_FORMAT_VERSION,
        prompt_version: HISTORY_REPLACEMENT_PROMPT_VERSION,
    };
    let digest = Sha256::digest(
        serde_json::to_vec(&identity).expect("history replacement identity is serializable"),
    );
    format!("hr-{}", hex_digest(&digest))
}

pub fn history_replacement_locator(source: &HistoryReplacementSource) -> String {
    let mut marker = format!(
        "[History Replacement; source: conversation_read_span(start_sequence={}, end_sequence={})]",
        source.sequence_range.start_sequence, source.sequence_range.end_sequence
    );
    if !source.tool_references.is_empty() {
        let references =
            serde_json::to_string(&source.tool_references).unwrap_or_else(|_| "[]".to_string());
        marker.pop();
        marker.push_str("; tool_references: ");
        marker.push_str(&references);
        marker.push(']');
    }
    marker
}

pub fn history_replacement_version_is_superseded(value: &Value) -> bool {
    let Some(format_version) = value.get("format_version").and_then(Value::as_u64) else {
        return false;
    };
    let Some(prompt_version) = value.get("prompt_version").and_then(Value::as_u64) else {
        return false;
    };
    format_version != HISTORY_REPLACEMENT_FORMAT_VERSION as u64
        || prompt_version != HISTORY_REPLACEMENT_PROMPT_VERSION as u64
}

pub(crate) fn history_replacement_checksum(
    record: &HistoryReplacementRecord,
) -> rua_core::Result<String> {
    let mut payload = record.clone();
    payload.checksum.clear();
    let bytes = serde_json::to_vec(&payload).map_err(|error| rua_core::RuaError::Io {
        message: format!(
            "failed to serialize history replacement {} for checksum: {error}",
            record.record_id
        ),
    })?;
    Ok(format!("sha256:{}", hex_digest(&Sha256::digest(bytes))))
}

pub fn validate_history_replacement_record(
    address: &rua_core::ChatAddress,
    record_id: &str,
    record: &HistoryReplacementRecord,
) -> rua_core::Result<()> {
    if record.chat_address != *address || record.record_id != record_id {
        return Err(rua_core::RuaError::Io {
            message: format!("history replacement identity mismatch for {address}/{record_id}"),
        });
    }
    if record.format_version != HISTORY_REPLACEMENT_FORMAT_VERSION
        || record.prompt_version != HISTORY_REPLACEMENT_PROMPT_VERSION
    {
        return Err(rua_core::RuaError::Io {
            message: format!("unsupported history replacement version for {address}/{record_id}"),
        });
    }
    if derive_history_replacement_record_id(address.chat_id().as_str(), &record.source)
        != record.record_id
    {
        return Err(rua_core::RuaError::Io {
            message: format!(
                "history replacement source identity mismatch for {address}/{record_id}"
            ),
        });
    }
    if history_replacement_checksum(record)? != record.checksum {
        return Err(rua_core::RuaError::Io {
            message: format!("history replacement checksum mismatch for {address}/{record_id}"),
        });
    }
    Ok(())
}

pub fn detect_execution_trace_blocks(
    chat_id: &str,
    turn_id: &str,
    messages: &[MessageLogEntry],
    projection_lineage: String,
    token_estimate_divisor: f64,
    variant_terminal: bool,
) -> Vec<ExecutionTraceBlock> {
    let mut parts = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        for (part_index, value) in content_parts(&message.content).iter().enumerate() {
            let kind = classify_part(message, value);
            if is_ignorable_empty_text(value) {
                continue;
            }
            parts.push(PartRef {
                message,
                message_index,
                part_index,
                value,
                kind,
            });
        }
    }

    let mut process_runs = Vec::new();
    let mut cursor = 0usize;
    while cursor < parts.len() {
        if parts[cursor].kind != PartKind::Process {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor + 1 < parts.len() && parts[cursor + 1].kind == PartKind::Process {
            cursor += 1;
        }
        process_runs.push((start, cursor));
        cursor += 1;
    }
    process_runs
        .into_iter()
        .filter_map(|(start, end)| {
            let following_visible_text = parts.get(end + 1).and_then(|part| {
                (part.kind == PartKind::VisibleAssistantText)
                    .then(|| part_text(part.value))
                    .flatten()
            });
            let closes_at_variant_end = variant_terminal && end + 1 == parts.len();
            if following_visible_text.is_none() && !closes_at_variant_end {
                return None;
            }
            let process_parts = &parts[start..=end];
            if !tool_protocol_is_complete(process_parts) {
                return None;
            }
            let source_start = start
                .checked_sub(1)
                .filter(|previous| parts[*previous].kind == PartKind::VisibleAssistantText)
                .unwrap_or(start);
            let source_parts = &parts[source_start..=end];
            build_execution_trace_block(
                chat_id,
                turn_id,
                source_parts,
                process_parts,
                projection_lineage.clone(),
                token_estimate_divisor,
            )
        })
        .collect()
}

pub(crate) fn apply_ready_history_replacements_with_evidence(
    messages: &mut Vec<MessageLogEntry>,
    records: &[HistoryReplacementRecord],
) -> Vec<Option<HistoryReplacementApplicationEvidence>> {
    let message_indexes = messages
        .iter()
        .enumerate()
        .map(|(index, message)| (message.message_id.clone(), index))
        .collect::<HashMap<_, _>>();
    let reports = records
        .iter()
        .map(|record| {
            apply_ready_history_replacement_with_index(messages, record, &message_indexes)
        })
        .collect();
    messages.retain(|message| {
        !message
            .content
            .as_array()
            .is_some_and(|parts| parts.is_empty())
    });
    reports
}

fn apply_ready_history_replacement_with_index(
    messages: &mut [MessageLogEntry],
    record: &HistoryReplacementRecord,
    message_indexes: &HashMap<String, usize>,
) -> Option<HistoryReplacementApplicationEvidence> {
    let digest = record.ready_digest()?;
    let source = &record.source;
    let start_message = source.start_anchor.message_id.as_str();
    let end_message = source.end_anchor.message_id.as_str();
    let start_message_index = *message_indexes.get(start_message)?;
    let end_message_index = *message_indexes.get(end_message)?;
    if start_message_index > end_message_index {
        return None;
    }
    let start_entry = messages.get(start_message_index)?;
    let end_entry = messages.get(end_message_index)?;
    if start_entry.sequence < source.sequence_range.start_sequence
        || end_entry.sequence > source.sequence_range.end_sequence
        || source.start_anchor.content_part_index >= content_part_count(&start_entry.content)
        || source.end_anchor.content_part_index >= content_part_count(&end_entry.content)
    {
        return None;
    }
    let start_position = (start_message_index, source.start_anchor.content_part_index);
    let end_position = (end_message_index, source.end_anchor.content_part_index);
    if start_position > end_position {
        return None;
    }

    let mut mutations = Vec::new();
    let replacement_text = format!(
        "{}\n\n{}",
        history_replacement_locator(source),
        digest.trim()
    );
    let replacement_part = serde_json::json!({
        "type": "text",
        "text": replacement_text,
    });
    for (message_index, message) in messages
        .iter_mut()
        .enumerate()
        .take(end_position.0 + 1)
        .skip(start_position.0)
    {
        let original_content = std::mem::replace(&mut message.content, Value::Null);
        let original_parts = into_content_parts(original_content);
        let mut projected_parts = Vec::with_capacity(original_parts.len());
        for (part_index, part) in original_parts.into_iter().enumerate() {
            let position = (message_index, part_index);
            if position < start_position || position > end_position {
                projected_parts.push(part);
                continue;
            }
            if position == start_position {
                projected_parts.push(replacement_part.clone());
                mutations.push(HistoryReplacementMutationEvidence {
                    message_id: message.message_id.clone(),
                    content_part_index: part_index,
                    before: part,
                    after: Some(replacement_part.clone()),
                });
            } else {
                mutations.push(HistoryReplacementMutationEvidence {
                    message_id: message.message_id.clone(),
                    content_part_index: part_index,
                    before: part,
                    after: None,
                });
            }
        }
        message
            .metadata
            .insert("hidden_from_ui".to_string(), serde_json::Value::Bool(true));
        message
            .metadata
            .insert("model_visible".to_string(), serde_json::Value::Bool(true));
        message.metadata.insert(
            "history_replacement_record_id".to_string(),
            serde_json::Value::String(record.record_id.clone()),
        );
        message.content = Value::Array(projected_parts);
    }
    let removed_message_ids = messages
        .iter()
        .take(end_position.0 + 1)
        .skip(start_position.0)
        .filter(|message| message.content.as_array().is_some_and(Vec::is_empty))
        .map(|message| message.message_id.clone())
        .collect();
    Some(HistoryReplacementApplicationEvidence {
        record_id: record.record_id.clone(),
        status: "applied".to_string(),
        reason: None,
        mutations,
        digest_carrier: Some(source.start_anchor.clone()),
        removed_message_ids,
    })
}

fn content_part_count(content: &Value) -> usize {
    content.as_array().map_or(1, Vec::len)
}

fn into_content_parts(content: Value) -> Vec<Value> {
    match content {
        Value::Array(parts) => parts,
        other => vec![other],
    }
}

pub fn estimate_text_tokens(text: &str, divisor: f64) -> u64 {
    ((text.len() as f64 / divisor.max(1.0)).ceil() as u64).max(1)
}

pub fn evaluate_history_replacement_candidate(
    source_token_estimate: u64,
    digest: &str,
    rendered_replacement: &str,
    finish_reason: &str,
    divisor: f64,
) -> HistoryReplacementCandidateDecision {
    if matches!(finish_reason, "cancelled" | "canceled" | "error") {
        return HistoryReplacementCandidateDecision::Retryable {
            reason: finish_reason.to_string(),
        };
    }
    let replacement_token_estimate = estimate_text_tokens(rendered_replacement, divisor);
    let saved_token_estimate = source_token_estimate.saturating_sub(replacement_token_estimate);
    let rejected = |reason| HistoryReplacementCandidateDecision::Rejected {
        reason,
        replacement_token_estimate,
        saved_token_estimate,
    };
    if finish_reason == "length" {
        return rejected(HistoryReplacementRejectionReason::OutputLimitReached);
    }
    if finish_reason != "stop" {
        return rejected(HistoryReplacementRejectionReason::UnsupportedFinishReason);
    }
    if digest.trim().is_empty() {
        return rejected(HistoryReplacementRejectionReason::EmptyOutput);
    }
    if saved_token_estimate < HISTORY_REPLACEMENT_MIN_SAVED_TOKENS {
        return rejected(HistoryReplacementRejectionReason::InsufficientSavings);
    }
    if saved_token_estimate.saturating_mul(100)
        < source_token_estimate.saturating_mul(HISTORY_REPLACEMENT_MIN_REDUCTION_PERCENT)
    {
        return rejected(HistoryReplacementRejectionReason::InsufficientReduction);
    }
    HistoryReplacementCandidateDecision::Ready {
        replacement_token_estimate,
        saved_token_estimate,
    }
}

fn estimate_values_tokens<'a>(values: impl Iterator<Item = &'a Value>, divisor: f64) -> u64 {
    values
        .map(|value| {
            serde_json::to_vec(value)
                .map(|bytes| ((bytes.len() as f64 / divisor.max(1.0)).ceil() as u64).max(1))
                .unwrap_or(1)
        })
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn build_execution_trace_block(
    chat_id: &str,
    turn_id: &str,
    source_parts: &[PartRef<'_>],
    process_parts: &[PartRef<'_>],
    projection_lineage: String,
    token_estimate_divisor: f64,
) -> Option<ExecutionTraceBlock> {
    let first = source_parts.first()?;
    let last = source_parts.last()?;
    let mut seen_message_ids = HashSet::new();
    let message_ids = source_parts
        .iter()
        .filter(|part| seen_message_ids.insert(part.message.message_id.clone()))
        .map(|part| part.message.message_id.clone())
        .collect::<Vec<_>>();
    let source = HistoryReplacementSource {
        turn_id: turn_id.to_string(),
        message_ids,
        start_anchor: HistoryReplacementContentAnchor {
            message_id: first.message.message_id.clone(),
            content_part_index: first.part_index,
        },
        end_anchor: HistoryReplacementContentAnchor {
            message_id: last.message.message_id.clone(),
            content_part_index: last.part_index,
        },
        sequence_range: CheckpointSequenceRange {
            start_sequence: first.message.sequence,
            end_sequence: last.message.sequence,
        },
        tool_references: extract_tool_references(process_parts)?,
        projection_lineage,
        token_estimate: estimate_values_tokens(
            source_parts.iter().map(|part| part.value),
            token_estimate_divisor,
        ),
    };
    Some(ExecutionTraceBlock {
        record_id: derive_history_replacement_record_id(chat_id, &source),
        source_content: render_source_content(source_parts),
        source,
    })
}

fn extract_tool_references(parts: &[PartRef<'_>]) -> Option<Vec<HistoryReplacementToolReference>> {
    let results = parts
        .iter()
        .filter_map(|part| {
            if part_type(part.value) != Some("tool_result") {
                return None;
            }
            Some((
                tool_result_call_id(part.value)?,
                (part.message.sequence, tool_result_recall_key(part.value)),
            ))
        })
        .collect::<HashMap<_, _>>();

    parts
        .iter()
        .filter(|part| part_type(part.value) == Some("tool_use"))
        .map(|part| {
            let tool_call_id = tool_call_id(part.value)?;
            let (result_sequence, recall_key) = results.get(tool_call_id)?.clone();
            Some(HistoryReplacementToolReference {
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name(part.value).unwrap_or("unknown").to_string(),
                call_sequence: part.message.sequence,
                result_sequence,
                recall_key,
            })
        })
        .collect()
}

fn tool_protocol_is_complete(parts: &[PartRef<'_>]) -> bool {
    let mut pending = HashSet::new();
    let mut seen_calls = HashSet::new();
    for part in parts {
        match part_type(part.value) {
            Some("tool_use") => {
                let Some(call_id) = tool_call_id(part.value) else {
                    return false;
                };
                if !seen_calls.insert(call_id.to_string()) {
                    return false;
                }
                pending.insert(call_id.to_string());
            }
            Some("tool_result") => {
                let Some(call_id) = tool_result_call_id(part.value) else {
                    return false;
                };
                if !pending.remove(call_id) {
                    return false;
                }
            }
            Some("thinking") => {}
            _ => return false,
        }
    }
    pending.is_empty()
}

fn render_source_content(parts: &[PartRef<'_>]) -> String {
    let mut rendered = String::new();
    let mut previous_message_index = None;
    for part in parts {
        if previous_message_index != Some(part.message_index) {
            if !rendered.is_empty() {
                rendered.push('\n');
            }
            rendered.push_str(&format!(
                "[role={} sequence={} message_id={}]\n",
                part.message.role, part.message.sequence, part.message.message_id
            ));
            previous_message_index = Some(part.message_index);
        }
        rendered
            .push_str(&serde_json::to_string(part.value).unwrap_or_else(|_| "null".to_string()));
        rendered.push('\n');
    }
    rendered
}

fn content_parts(content: &Value) -> Vec<&Value> {
    match content {
        Value::Array(parts) => parts.iter().collect(),
        other => vec![other],
    }
}

fn classify_part(message: &MessageLogEntry, part: &Value) -> PartKind {
    match part_type(part) {
        Some("thinking" | "tool_use" | "tool_result") => PartKind::Process,
        Some("text")
            if message.role == "assistant"
                && message.status == "completed"
                && part_text(part).is_some_and(|text| !text.trim().is_empty()) =>
        {
            PartKind::VisibleAssistantText
        }
        _ => PartKind::Other,
    }
}

fn part_type(part: &Value) -> Option<&str> {
    part.get("type").and_then(Value::as_str)
}

fn tool_call_id(part: &Value) -> Option<&str> {
    part.get("id")
        .and_then(Value::as_str)
        .or_else(|| {
            part.get("call")
                .and_then(|call| call.get("id"))
                .and_then(Value::as_str)
        })
        .filter(|call_id| !call_id.trim().is_empty())
}

fn tool_result_call_id(part: &Value) -> Option<&str> {
    part.get("id")
        .and_then(Value::as_str)
        .or_else(|| part.get("call_id").and_then(Value::as_str))
        .or_else(|| {
            part.get("result")
                .and_then(|result| result.get("call_id"))
                .and_then(Value::as_str)
        })
        .filter(|call_id| !call_id.trim().is_empty())
}

fn tool_name(part: &Value) -> Option<&str> {
    part.get("name")
        .and_then(Value::as_str)
        .or_else(|| {
            part.get("call")
                .and_then(|call| call.get("name"))
                .and_then(Value::as_str)
        })
        .filter(|name| !name.trim().is_empty())
}

fn tool_result_recall_key(part: &Value) -> Option<String> {
    let nested = part.get("result");
    [Some(part), nested]
        .into_iter()
        .flatten()
        .find_map(|value| {
            value
                .get("truncation")
                .and_then(|truncation| truncation.get("recall_key"))
                .and_then(Value::as_str)
                .or_else(|| {
                    value
                        .get("metadata")
                        .and_then(|metadata| metadata.get("tool_output"))
                        .and_then(|tool_output| {
                            tool_output
                                .get("recall_key")
                                .or_else(|| tool_output.get("archive_key"))
                        })
                        .and_then(Value::as_str)
                })
                .filter(|key| !key.trim().is_empty())
                .map(str::to_string)
        })
}

fn part_text(part: &Value) -> Option<String> {
    part.get("text").and_then(Value::as_str).map(str::to_string)
}

fn is_ignorable_empty_text(part: &Value) -> bool {
    part_type(part) == Some("text")
        && part
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.trim().is_empty())
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(&mut out, "{byte:02x}");
            out
        })
}
