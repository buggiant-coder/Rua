use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use futures::stream::StreamExt;
use rua_core::{AgentCollaborationMode, ChatAddress};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, error, warn};

use rua_agent::context_manager::ContextManager;
use rua_agent::memory_signal::MemorySignal;
use rua_agent::memory_signal_note::{
    detect_memory_signal_for_user_input, memory_signal_runtime_note,
};
use rua_config::AgentsRunningConfig;
use rua_contract::agent::{AgentInputMessage, AgentRuntime};
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;
use rua_core::message::ContentPart;
use rua_core::runtime_layout::RuntimeLayout;
use rua_session::{
    early_background_compaction_coordinator, ChatContextSnapshot, ChatSessionHeader,
    CheckpointModelIdentity, ContextProjection, ContextProjectionRequest,
    EarlyBackgroundCompactionDecision, EarlyBackgroundCompactionInput,
    EarlyBackgroundCompactionLease, EarlyBackgroundCompactionPolicy, NewConversationMessage,
    NewConversationTurn, ProjectionEntryMode, ProjectionPlanner, ProjectionPlanningInput,
    TurnLifecycle, TurnLifecycleStatus, TurnWriter, WorkspaceSessionStore,
};

use crate::endpoints::dto;
use crate::endpoints::messages::PushMessageStore;
use crate::services::agent_manager::AgentManager;
use crate::services::chat_device_activity::{
    apply_chat_activity, copy_chat_activity_to_header_extra, ChatDeviceActivityUpdate,
};
use crate::services::chat_index;
use crate::services::chat_metadata_store::{
    materialize_targeted_chat_metadata, ChatMetadataStoreError,
};
use crate::services::context_projection::{
    build_system_context_accounting, context_target_band_from_running,
    early_compaction_total_tokens, message_log_entries_to_input_messages,
};
use crate::services::conversation_authority::{
    ConversationAuthority, ConversationInputPayload, ConversationTurnBinding,
    ConversationTurnTerminalFact,
};
use crate::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacementError,
};
use crate::services::conversation_authority_router::{
    ConversationAuthorityRouter, ConversationControlRoute, ConversationControlRouteRequest,
};
use crate::services::conversation_authority_target::chat_execution_runtime_metadata;
use crate::services::conversation_input_intake::{ConversationInputIntake, TaskWatchWakeReceipts};
use crate::services::conversation_live_activity::ConversationLiveActivity;
use crate::services::device_command_bus::{
    DeviceCommandDeliveryError, RemoteChatControlAction, RemoteConversationInputRequest,
};
use crate::services::remote_conversation_input::{
    RemoteConversationControlParams, RemoteConversationInputDelivery,
};
use crate::streaming::{ActiveStreamRegistry, Priority, StreamKey};
use crate::sync_publication::{
    publish_sync_fact_with_runtime, PublicationStatus, SyncPublicationFact, SyncPublicationRuntime,
};

const RUA_TOKEN_USAGE_META_KEY: &str = "rua_token_usage";
pub const TURN_ABORTED_MARKER_TEXT: &str = "<turn_aborted>\nThe user interrupted the previous turn on purpose. Any running commands or tools may still be running in the background. If any tools/commands were aborted, they may have partially executed.\n</turn_aborted>";
pub const TURN_INTERJECTED_MARKER_TEXT: &str = "<turn_interjected>\nThe user inserted a new message, so the previous turn was ended before it could continue.\n</turn_interjected>";

use rua_channel::core::console::{ConsoleChannelService, ProcessRequest, SessionPersistence};

pub(crate) fn prompt_file_fragment_for_entries(
    ws_dir: &std::path::Path,
    entries: &[String],
) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    rua_agent::prompt::build_system_prompt_from_working_dir(ws_dir, Some(entries))
        .ok()
        .filter(|prompt| !prompt.trim().is_empty())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub role: String,
    #[serde(deserialize_with = "deserialize_content")]
    pub content: Vec<serde_json::Value>,
    pub name: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn deserialize_content<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer).map_err(serde::de::Error::custom)?;
    match value {
        serde_json::Value::Array(arr) => Ok(arr),
        serde_json::Value::String(s) => Ok(vec![serde_json::json!({"type": "text", "text": s})]),
        other => Ok(vec![other]),
    }
}

impl StoredMessage {
    pub fn from_input_message(msg: &AgentInputMessage, agent_id: &str, device_id: &str) -> Self {
        let content_parts: Vec<serde_json::Value> = msg
            .content
            .iter()
            .map(agent_types::content::content_part_to_stored_value)
            .collect();
        let name = match msg.role.as_str() {
            "user" => "user".to_string(),
            "assistant" => agent_id.to_string(),
            _ => "system".to_string(),
        };

        let timestamp = chrono::Utc::now().to_rfc3339();
        StoredMessage {
            id: format!("msg-{}", uuid::Uuid::new_v4()),
            role: msg.role.clone(),
            content: content_parts,
            name,
            timestamp,
            device_id: device_id.to_string(),
            metadata: serde_json::json!({}),
            extra: serde_json::Map::new(),
        }
    }

    pub fn to_input_message(&self) -> Option<AgentInputMessage> {
        let parts: Vec<ContentPart> = self
            .content
            .iter()
            .map(|v| rua_core::message::content_part_from_value(v.clone()))
            .collect();
        if parts.is_empty() {
            return None;
        }
        Some(AgentInputMessage {
            role: self.role.clone(),
            content: parts,
        })
    }
}

fn stamp_acp_origin(stored: &mut StoredMessage, runner: &str) {
    let map = match stored.metadata.as_object_mut() {
        Some(m) => m,
        None => {
            stored.metadata = serde_json::json!({});
            stored
                .metadata
                .as_object_mut()
                .expect("metadata just set to object")
        }
    };
    map.insert(
        "origin".to_string(),
        serde_json::json!({ "kind": "acp", "runner": runner }),
    );
}

fn is_acp_passthrough_input(text: &str) -> bool {
    rua_command::parse_command(text)
        .map(|(name, _)| !rua_command::is_acp_reserved_command(name))
        .unwrap_or(true)
}

async fn persisted_acp_origin_runner(
    store: &WorkspaceSessionStore,
    chat_id: &str,
) -> Option<String> {
    let header = match store.load_chat_header(chat_id).await {
        Ok(Some(header)) => header,
        Ok(None) => return None,
        Err(error) => {
            warn!(
                chat_id,
                error = %error,
                "failed to load chat header while resolving persisted ACP origin"
            );
            return None;
        }
    };
    header
        .extra
        .get("acp_binding")
        .and_then(|value| value.as_object())
        .and_then(|binding| binding.get("runner"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

async fn persisted_chat_model_slot(
    store: &WorkspaceSessionStore,
    chat_id: &str,
) -> Option<serde_json::Value> {
    let header = match store.load_chat_header(chat_id).await {
        Ok(Some(header)) => header,
        Ok(None) => return None,
        Err(error) => {
            warn!(
                chat_id,
                error = %error,
                "failed to load chat header while resolving chat model slot"
            );
            return None;
        }
    };
    rua_chat::model_slot_from_header(&header)
}

fn model_id_from_slot(slot: &serde_json::Value) -> Option<String> {
    slot.get("model")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn chat_model_slot_from_extra(
    extra: &serde_json::Map<String, serde_json::Value>,
) -> Option<serde_json::Value> {
    let slot = if let Some(turn_model) = extra.get(rua_chat::TURN_MODEL_KEY) {
        let mut slot = turn_model.as_object()?.clone();
        if let Some(thinking) = extra
            .get(rua_chat::TURN_THINKING_KEY)
            .and_then(serde_json::Value::as_object)
        {
            slot.extend(thinking.clone());
        }
        serde_json::Value::Object(slot)
    } else {
        extra.get("chat_model_slot")?.clone()
    };
    let _provider_id = slot
        .get("provider_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let _model = slot
        .get("model")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(slot)
}

fn chat_model_slot_from_active_turn_detail(
    detail: &serde_json::Value,
) -> Option<serde_json::Value> {
    chat_model_slot_from_extra(detail.get("metadata")?.as_object()?)
}

async fn chat_model_slot_from_conversation_authority(
    authority: Option<&Arc<ConversationAuthority>>,
    agent_id: &str,
    chat_id: &str,
) -> Option<serde_json::Value> {
    let active_turn = authority?
        .active_turn(&ChatAddress::agent(agent_id, chat_id))
        .await?;
    chat_model_slot_from_active_turn_detail(active_turn.driver_detail.as_ref()?)
}

#[derive(Debug, Clone)]
pub(crate) struct DeviceDeliveryMetadata {
    device_delivery_id: String,
    device_delivery_sequence: u64,
    source_device_id: String,
    target_device_id: String,
}

impl DeviceDeliveryMetadata {
    pub(crate) fn from_extra(extra: &serde_json::Map<String, serde_json::Value>) -> Option<Self> {
        let value = extra.get("device_delivery")?.as_object()?;
        let device_delivery_id = value.get("device_delivery_id")?.as_str()?.to_string();
        let device_delivery_sequence = value.get("device_delivery_sequence")?.as_u64()?;
        let source_device_id = value.get("source_device_id")?.as_str()?.to_string();
        let target_device_id = value.get("target_device_id")?.as_str()?.to_string();
        Some(Self {
            device_delivery_id,
            device_delivery_sequence,
            source_device_id,
            target_device_id,
        })
    }

    fn to_message_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "device_delivery_id": self.device_delivery_id,
            "device_delivery_sequence": self.device_delivery_sequence,
            "source_device_id": self.source_device_id,
            "target_device_id": self.target_device_id,
            "executed_on_device_id": self.target_device_id,
        })
    }
}

fn compact_summary_intro(model: Option<&CheckpointModelIdentity>) -> &'static str {
    match model {
        Some(model) if model.provider == "manual" && model.model == "compact" => {
            "Earlier conversation was manually compacted with /compact."
        }
        Some(model) if model.provider == "runtime" => {
            "Earlier conversation was compacted by the runtime context manager."
        }
        Some(_) => "Earlier conversation was automatically compacted in the background.",
        None => "Earlier conversation was compacted.",
    }
}

const CONVERSATION_RECALL_TOOL_NAMES: &[&str] = &["conversation_search", "conversation_read_span"];

#[derive(Debug)]
struct EvergreenProjectionInput {
    messages: Vec<AgentInputMessage>,
    conversation_recall_available: bool,
}

fn disable_conversation_recall_tools(disabled_tools: &mut Vec<String>) {
    for tool in CONVERSATION_RECALL_TOOL_NAMES {
        if !disabled_tools.iter().any(|disabled| disabled == tool) {
            disabled_tools.push((*tool).to_string());
        }
    }
}

fn projection_allows_conversation_recall(projection: &ContextProjection) -> bool {
    projection
        .summary
        .as_ref()
        .is_some_and(|summary| !summary.trim().is_empty())
        || projection
            .retained_messages
            .iter()
            .chain(projection.tail_messages.iter())
            .any(|message| {
                message
                    .metadata
                    .contains_key("history_replacement_record_id")
            })
}

fn context_projection_to_input_messages(projection: &ContextProjection) -> Vec<AgentInputMessage> {
    let mut messages = Vec::new();
    if let Some(summary) = projection
        .summary
        .as_ref()
        .map(|summary| summary.trim())
        .filter(|summary| !summary.is_empty())
    {
        messages.push(AgentInputMessage {
            role: "system".to_string(),
            content: vec![ContentPart::Text {
                text: format!(
                    "# Compact Summary of Previous Conversation\n\
{}\n\
The raw transcript remains stored in this chat. The active model context now \
contains this summary plus the recent messages that follow it. If the user asks \
about earlier omitted details and this summary plus recent context is \
insufficient, use conversation_search to find the raw transcript span, then \
conversation_read_span to expand it when needed.\n\n{summary}",
                    compact_summary_intro(projection.summary_model.as_ref())
                ),
            }],
        });
    }
    messages.extend(message_log_entries_to_input_messages(
        &projection.retained_messages,
    ));
    messages.extend(message_log_entries_to_input_messages(
        &projection.tail_messages,
    ));
    messages
}

fn projection_tokens_after_tool_truncation(
    projection: &ContextProjection,
    running: &AgentsRunningConfig,
) -> u64 {
    rua_agent::projection_preparation::final_projection_token_estimate(projection, running)
}

fn attach_token_usage_to_closing_assistant(
    messages: &mut [NewConversationMessage],
    usage: Option<serde_json::Value>,
    last_call_usage: Option<serde_json::Value>,
    max_input_length: Option<u64>,
) {
    if usage.is_none() && last_call_usage.is_none() {
        return;
    }
    let Some(message) = messages
        .iter_mut()
        .rev()
        .find(|message| message.role == "assistant")
    else {
        return;
    };
    message.metadata.insert(
        RUA_TOKEN_USAGE_META_KEY.to_string(),
        serde_json::json!({
            "usage": usage,
            "last_call_usage": last_call_usage,
            "max_input_length": max_input_length,
        }),
    );
}

async fn load_evergreen_projection_messages(
    store: &rua_session::WorkspaceSessionStore,
    chat_address: &ChatAddress,
    running_config: Option<&AgentsRunningConfig>,
) -> Option<EvergreenProjectionInput> {
    let started = Instant::now();
    let chat_id = chat_address.chat_id().as_str();
    store.load_chat_header(chat_id).await.ok().flatten()?;
    let request = ContextProjectionRequest {
        current_user_input: None,
    };
    let projection = match running_config {
        Some(running) => store
            .build_effective_context_projection_for_address(
                chat_address,
                request,
                running.context_compact.history_replacement_enabled,
                running.context_compact.token_count_estimate_divisor,
            )
            .await
            .ok()?,
        None => store
            .build_context_projection(chat_id, request)
            .await
            .ok()?,
    };
    let conversation_recall_available = projection_allows_conversation_recall(&projection);
    let message_conversion_started = Instant::now();
    let messages = context_projection_to_input_messages(&projection);
    let message_conversion_elapsed = message_conversion_started.elapsed();
    let projected_conversation_tokens = running_config
        .map(|running| projection_tokens_after_tool_truncation(&projection, running))
        .unwrap_or(projection.stable_token_estimate);
    debug!(
        chat_id,
        message_count = messages.len(),
        tail_message_count = projection.tail_messages.len(),
        has_summary = projection.summary.is_some(),
        stable_token_estimate = projected_conversation_tokens,
        message_conversion_elapsed_ms = message_conversion_elapsed.as_millis(),
        elapsed_ms = started.elapsed().as_millis(),
        "assembled evergreen projection input messages"
    );
    Some(EvergreenProjectionInput {
        messages,
        conversation_recall_available,
    })
}

async fn load_evergreen_replay_messages_before_turn(
    store: &rua_session::WorkspaceSessionStore,
    chat_id: &str,
    turn_id: &str,
) -> rua_core::Result<Option<Vec<AgentInputMessage>>> {
    let Some(turn) = store
        .read_turn_range(chat_id, 1, usize::MAX)
        .await?
        .into_iter()
        .find(|turn| turn.turn_id == turn_id)
    else {
        return Ok(None);
    };
    let limit = turn.user_message_sequence.saturating_sub(1) as usize;
    let messages = store.read_message_range(chat_id, 1, limit).await?;
    Ok(Some(message_log_entries_to_input_messages(&messages)))
}

fn conversation_terminal_runtime_note(
    agent_id: &str,
    chat_id: &str,
    turn_id: &str,
    terminal_status: &str,
    reason: &str,
    summary: String,
) -> rua_session::RuntimeNote {
    rua_session::RuntimeNote {
        runtime_note_id: format!("runtime-note:conversation:{chat_id}:{turn_id}:{reason}"),
        created_at: chrono::Utc::now().to_rfc3339(),
        payload: rua_session::RuntimeNotePayload::ConversationRuntimeTerminal(
            rua_session::ConversationRuntimeTerminalRuntimeNote {
                agent_id: agent_id.to_string(),
                chat_id: chat_id.to_string(),
                turn_id: turn_id.to_string(),
                terminal_status: terminal_status.to_string(),
                reason: reason.to_string(),
                summary,
            },
        ),
    }
}

struct ConversationTerminalFact<'a> {
    terminal_status: &'static str,
    lifecycle_status: &'a str,
    reason: &'a str,
    summary: String,
    write_runtime_note: bool,
    marker: Option<ConversationTerminalMarker>,
}

enum ConversationTerminalMarker {
    TurnAborted,
    TurnInterjected,
}

fn turn_aborted_marker_message(turn_id: &str, created_at: String) -> NewConversationMessage {
    NewConversationMessage {
        message_id: format!("turn-aborted-{turn_id}"),
        role: "user".to_string(),
        created_at,
        status: "completed".to_string(),
        content: serde_json::json!([{
            "type": "text",
            "text": TURN_ABORTED_MARKER_TEXT,
        }]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::from_iter([
            (
                "model_visible_marker".to_string(),
                serde_json::Value::String("turn_aborted".to_string()),
            ),
            (
                "reason".to_string(),
                serde_json::Value::String("user_stop".to_string()),
            ),
        ]),
    }
}

fn turn_interjected_marker_message(turn_id: &str, created_at: String) -> NewConversationMessage {
    NewConversationMessage {
        message_id: format!("turn-interjected-{turn_id}"),
        role: "system".to_string(),
        created_at,
        status: "completed".to_string(),
        content: serde_json::json!([{
            "type": "text",
            "text": TURN_INTERJECTED_MARKER_TEXT,
        }]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::from_iter([
            (
                "model_visible_marker".to_string(),
                serde_json::Value::String("turn_interjected".to_string()),
            ),
            (
                "reason".to_string(),
                serde_json::Value::String("active_interjection".to_string()),
            ),
        ]),
    }
}

fn memory_signal_marker_message(
    turn_id: &str,
    created_at: String,
    signal: &MemorySignal,
) -> rua_core::Result<NewConversationMessage> {
    memory_signal_runtime_note(format!("memory-signal-{turn_id}"), created_at, signal)
        .into_new_conversation_message()
}

async fn persist_conversation_terminal_to_session(
    workspace_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    turn_id: &str,
    fact: ConversationTerminalFact<'_>,
    session_sync: Option<&SessionSyncEmitter>,
) -> rua_core::Result<bool> {
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let turns = store.read_turn_range(chat_id, 1, usize::MAX).await?;
    let Some(turn) = turns.iter().find(|turn| turn.turn_id == turn_id) else {
        return Ok(false);
    };
    let can_write_terminal = turn.lifecycle.status == "in_progress"
        || (matches!(
            fact.reason,
            "user_stop" | "active_interjection" | "runtime_failure"
        ) && turn.lifecycle.status == "completed");
    if !can_write_terminal {
        return Ok(false);
    }

    let now = chrono::Utc::now().to_rfc3339();
    let mut lifecycle = turn.lifecycle.clone();
    lifecycle.assistant_started_at = lifecycle.assistant_started_at.or_else(|| Some(now.clone()));
    lifecycle.assistant_completed_at = Some(now.clone());
    lifecycle.status = fact.lifecycle_status.to_string();

    if let Some(marker) = fact.marker {
        let (variant_id, message) = match marker {
            ConversationTerminalMarker::TurnAborted => (
                format!("turn-aborted-{}", turn.turn_id),
                turn_aborted_marker_message(&turn.turn_id, now),
            ),
            ConversationTerminalMarker::TurnInterjected => (
                format!("turn-interjected-{}", turn.turn_id),
                turn_interjected_marker_message(&turn.turn_id, now),
            ),
        };
        store
            .append_messages_to_turn(
                chat_id,
                &turn.turn_id,
                lifecycle,
                Some(variant_id),
                vec![message],
            )
            .await?;
    } else {
        store
            .set_turn_lifecycle(chat_id, &turn.turn_id, lifecycle)
            .await?;
    }

    if fact.write_runtime_note {
        let note = conversation_terminal_runtime_note(
            agent_id,
            chat_id,
            &turn.turn_id,
            fact.terminal_status,
            fact.reason,
            fact.summary,
        );
        store
            .append_runtime_note(chat_id, &turn.turn_id, note)
            .await?;
    }

    if let Some(emitter) = session_sync {
        let address = rua_core::ChatAddress::agent(agent_id, chat_id);
        emitter
            .emit_evergreen_chat_snapshot(workspace_dir, &address)
            .await;
    }
    Ok(true)
}

pub async fn persist_conversation_user_stop_to_session(
    workspace_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    turn_id: &str,
    session_sync: Option<&SessionSyncEmitter>,
) -> rua_core::Result<bool> {
    persist_conversation_terminal_to_session(
        workspace_dir,
        agent_id,
        chat_id,
        turn_id,
        ConversationTerminalFact {
            terminal_status: "interrupted",
            lifecycle_status: "interrupted",
            reason: "user_stop",
            summary: "conversation interrupted by user stop".to_string(),
            write_runtime_note: false,
            marker: Some(ConversationTerminalMarker::TurnAborted),
        },
        session_sync,
    )
    .await
}

pub async fn persist_conversation_interjected_to_session(
    workspace_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    turn_id: &str,
    session_sync: Option<&SessionSyncEmitter>,
) -> rua_core::Result<bool> {
    persist_conversation_terminal_to_session(
        workspace_dir,
        agent_id,
        chat_id,
        turn_id,
        ConversationTerminalFact {
            terminal_status: "interrupted",
            lifecycle_status: "interjected",
            reason: "active_interjection",
            summary: "conversation ended because the user inserted a new message".to_string(),
            write_runtime_note: false,
            marker: Some(ConversationTerminalMarker::TurnInterjected),
        },
        session_sync,
    )
    .await
}

pub async fn persist_conversation_runtime_failure_to_session(
    workspace_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    turn_id: &str,
    error: &str,
    session_sync: Option<&SessionSyncEmitter>,
) -> rua_core::Result<bool> {
    persist_conversation_terminal_to_session(
        workspace_dir,
        agent_id,
        chat_id,
        turn_id,
        ConversationTerminalFact {
            terminal_status: "failed",
            lifecycle_status: "failed",
            reason: "runtime_failure",
            summary: format!("conversation failed: {error}"),
            write_runtime_note: true,
            marker: None,
        },
        session_sync,
    )
    .await
}

pub async fn reconcile_orphan_in_progress_turns(
    workspace_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    has_active_turn: bool,
    session_sync: Option<&SessionSyncEmitter>,
) -> rua_core::Result<usize> {
    if has_active_turn {
        return Ok(0);
    }

    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let turns = store.read_turn_range(chat_id, 1, usize::MAX).await?;
    let mut reconciled = 0usize;
    for turn in turns {
        if turn.lifecycle.status != "in_progress" {
            continue;
        }
        let messages = store.read_turn_messages(chat_id, &turn.turn_id).await?;
        let is_device_delivery = messages.iter().any(|message| {
            message
                .metadata
                .get("device_delivery_id")
                .and_then(|value| value.as_str())
                .is_some()
        });
        if is_device_delivery {
            continue;
        }
        let wrote = persist_conversation_terminal_to_session(
            workspace_dir,
            agent_id,
            chat_id,
            &turn.turn_id,
            ConversationTerminalFact {
                terminal_status: "interrupted",
                lifecycle_status: "interrupted",
                reason: "runtime_restart",
                summary: "conversation interrupted by runtime restart".to_string(),
                write_runtime_note: true,
                marker: None,
            },
            session_sync,
        )
        .await?;
        if wrote {
            reconciled += 1;
        }
    }
    Ok(reconciled)
}

#[derive(Debug, Clone, Default)]
pub struct SessionContextSnapshot {
    pub sys_prompt: Option<String>,
    pub active_prompt_files: Vec<rua_config::SessionPromptFileSnapshot>,
    pub pending_action: Option<rua_config::SessionPendingActionSnapshot>,
}

pub fn build_prompt_file_snapshots(
    workspace_dir: &Path,
    active_prompt_files: &[String],
) -> Vec<rua_config::SessionPromptFileSnapshot> {
    let prompt_dir = workspace_dir.join("prompts");
    active_prompt_files
        .iter()
        .map(|id| {
            let path = prompt_dir.join(id);
            let content_hash = std::fs::read(&path).ok().map(|bytes| {
                let digest = Sha256::digest(&bytes);
                hex::encode(digest)
            });
            rua_config::SessionPromptFileSnapshot {
                id: id.clone(),
                name: Path::new(id)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string),
                included: content_hash.is_some(),
                content_hash,
                extra: serde_json::Map::new(),
            }
        })
        .collect()
}

fn chat_context_snapshot_from_session(snapshot: &SessionContextSnapshot) -> ChatContextSnapshot {
    ChatContextSnapshot {
        sys_prompt: snapshot.sys_prompt.clone(),
        active_prompt_files: snapshot
            .active_prompt_files
            .iter()
            .map(|item| serde_json::to_value(item).unwrap_or_default())
            .collect(),
        pending_action: snapshot
            .pending_action
            .as_ref()
            .filter(|pending| pending.action_type == "ask_user")
            .map(|pending| serde_json::to_value(pending).unwrap_or_default()),
    }
}

fn is_conversation_owned_pending_action(value: &serde_json::Value) -> bool {
    value
        .get("action_type")
        .and_then(|action_type| action_type.as_str())
        == Some("ask_user")
}

async fn save_session_context_snapshot_to_chat_header(
    store: &rua_session::WorkspaceSessionStore,
    chat_id: &str,
    snapshot: &SessionContextSnapshot,
) -> rua_core::Result<()> {
    let updated = store
        .update_chat_header(chat_id, |header| {
            let existing_pending_action = header
                .context_snapshot
                .pending_action
                .clone()
                .filter(is_conversation_owned_pending_action);
            header.context_snapshot = chat_context_snapshot_from_session(snapshot);
            if header.context_snapshot.pending_action.is_none() {
                header.context_snapshot.pending_action = existing_pending_action;
            }
        })
        .await?;
    if !updated {
        return Err(missing_chat(chat_id));
    }
    Ok(())
}

fn pending_action_from_runtime_event(
    event: &RuntimeEvent,
) -> Option<rua_config::SessionPendingActionSnapshot> {
    match event {
        RuntimeEvent::AskUser {
            request_id,
            questions,
        } => Some(rua_config::SessionPendingActionSnapshot {
            action_type: "ask_user".to_string(),
            request_id: Some(request_id.clone()),
            call_id: None,
            name: None,
            prompt: questions.first().map(|q| q.question.clone()),
            questions: Some(serde_json::to_value(questions).unwrap_or_default()),
            arguments: None,
            status: "waiting".to_string(),
            extra: serde_json::Map::new(),
        }),
        _ => None,
    }
}

fn header_has_waiting_pending_action(header: Option<&ChatSessionHeader>) -> bool {
    header
        .and_then(|header| header.context_snapshot.pending_action.as_ref())
        .and_then(|pending| pending.as_object())
        .is_some_and(|pending| {
            pending.get("action_type").and_then(|value| value.as_str()) == Some("ask_user")
                && pending.get("status").and_then(|value| value.as_str()) == Some("waiting")
        })
}

async fn persist_pending_action_snapshot(
    workspace_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    pending_action: rua_config::SessionPendingActionSnapshot,
    session_sync: Option<&SessionSyncEmitter>,
) -> rua_core::Result<()> {
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let value =
        serde_json::to_value(&pending_action).map_err(|error| rua_core::RuaError::Validation {
            message: format!("invalid pending action snapshot: {error}"),
        })?;
    let updated = store
        .update_chat_header(chat_id, |header| {
            header.context_snapshot.pending_action = Some(value);
        })
        .await?;
    if !updated {
        return Err(missing_chat(chat_id));
    }
    if let Some(emitter) = session_sync {
        let address = rua_core::ChatAddress::agent(agent_id, chat_id);
        emitter
            .emit_evergreen_chat_snapshot(workspace_dir, &address)
            .await;
    }
    Ok(())
}

pub async fn clear_pending_action_snapshot(
    workspace_dir: &Path,
    chat_id: &str,
) -> rua_core::Result<()> {
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let updated = store
        .update_chat_header(chat_id, |header| {
            header.context_snapshot.pending_action = None;
        })
        .await?;
    if !updated {
        return Err(missing_chat(chat_id));
    }
    Ok(())
}

pub struct ConsoleChatService {
    streams: Arc<ActiveStreamRegistry>,
    push_store: Option<PushMessageStore>,
    agent_service: Option<Arc<AgentManager>>,
    registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
    sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    host_profile: String,
    session_sync: Option<SessionSyncEmitter>,

    acp_chat_registry: Option<Arc<crate::services::acp_chat_registry::AcpChatRegistry>>,
    tool_config_cells: Option<Arc<rua_tool::AgentToolsCells>>,
    tool_config_device_key: Option<[u8; 32]>,
    tool_catalog: Option<Arc<serde_json::Map<String, serde_json::Value>>>,

    provider_manager: Option<Arc<rua_provider::manager::ProviderManager>>,
    conversation_authority: Option<Arc<ConversationAuthority>>,
    project_bindings: Option<Arc<rua_project::ProjectResourceBindingStore>>,
}

#[derive(Clone)]
pub struct SessionSyncEmitter {
    data_dir: PathBuf,
    sync_enabled: Arc<AtomicBool>,
    sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    sync_network_secret: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::NetworkSecret>>>>,
    sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    sync_hlc_clock: crate::state::SyncHlcClockCell,
    sync_trigger: Arc<tokio::sync::RwLock<Option<rua_sync::trigger::SyncTriggerHandle>>>,
    device_key: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConversationPublicationBoundary {
    ModelCall,
    Terminal,
}

impl SessionSyncEmitter {
    pub(crate) fn from_app_state(state: &crate::AppState) -> Self {
        Self::new(
            state.rua_root.clone(),
            Arc::clone(&state.sync_enabled),
            Arc::clone(&state.sync_node),
            Arc::clone(&state.sync_network_secret),
            Arc::clone(&state.sync_device),
            Arc::clone(&state.sync_hlc_clock),
            Arc::clone(&state.sync_trigger),
        )
        .with_device_key(*state.device_key.as_bytes())
    }

    pub fn new(
        data_dir: PathBuf,
        sync_enabled: Arc<AtomicBool>,
        sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
        sync_network_secret: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::NetworkSecret>>>>,
        sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
        sync_hlc_clock: crate::state::SyncHlcClockCell,
        sync_trigger: Arc<tokio::sync::RwLock<Option<rua_sync::trigger::SyncTriggerHandle>>>,
    ) -> Self {
        Self {
            data_dir,
            sync_enabled,
            sync_node,
            sync_network_secret,
            sync_device,
            sync_hlc_clock,
            sync_trigger,
            device_key: None,
        }
    }

    pub fn with_device_key(mut self, device_key: [u8; 32]) -> Self {
        self.device_key = Some(device_key);
        self
    }

    pub(crate) async fn emit_evergreen_chat_snapshot(
        &self,
        scope_root: &Path,
        address: &rua_core::ChatAddress,
    ) {
        let runtime = SyncPublicationRuntime::new(
            self.data_dir.join("sync"),
            Arc::clone(&self.sync_enabled),
            Arc::clone(&self.sync_node),
            Arc::clone(&self.sync_network_secret),
            Arc::clone(&self.sync_device),
            Some(Arc::clone(&self.sync_hlc_clock)),
            Arc::clone(&self.sync_trigger),
        );
        let _ = publish_sync_fact_with_runtime(
            &runtime,
            SyncPublicationFact::StableConversationState {
                scope_root: scope_root.to_path_buf(),
                address: address.clone(),
            },
        )
        .await;
    }

    async fn publish_conversation_boundary(
        &self,
        scope_root: &Path,
        address: &rua_core::ChatAddress,
        _boundary: ConversationPublicationBoundary,
    ) -> rua_core::Result<()> {
        let runtime = SyncPublicationRuntime::new(
            self.data_dir.join("sync"),
            Arc::clone(&self.sync_enabled),
            Arc::clone(&self.sync_node),
            Arc::clone(&self.sync_network_secret),
            Arc::clone(&self.sync_device),
            Some(Arc::clone(&self.sync_hlc_clock)),
            Arc::clone(&self.sync_trigger),
        );
        let result = publish_sync_fact_with_runtime(
            &runtime,
            SyncPublicationFact::StableConversationState {
                scope_root: scope_root.to_path_buf(),
                address: address.clone(),
            },
        )
        .await;
        if result.status != PublicationStatus::Published {
            return Ok(());
        }

        let manifest =
            crate::services::conversation_manifest::load_conversation_manifest(scope_root, address)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!(
                        "published Conversation snapshot has no readable manifest for {address}"
                    ),
                })?;
        let watermarks =
            crate::services::durable_publication_watermark::DurablePublicationWatermarkStore::new(
                self.data_dir.join("sync"),
            )?;
        watermarks.advance(address, &manifest).await?;
        Ok(())
    }

    pub(crate) async fn emit_chat_index_upsert_json(
        &self,
        agent_id: &str,
        chat_id: &str,
        chat: serde_json::Value,
    ) {
        self.emit_chat_index_upsert_for(&rua_core::ChatAddress::agent(agent_id, chat_id), chat)
            .await;
    }

    pub(crate) async fn emit_chat_index_upsert_for(
        &self,
        address: &rua_core::ChatAddress,
        chat: serde_json::Value,
    ) {
        let runtime = SyncPublicationRuntime::new(
            self.data_dir.join("sync"),
            Arc::clone(&self.sync_enabled),
            Arc::clone(&self.sync_node),
            Arc::clone(&self.sync_network_secret),
            Arc::clone(&self.sync_device),
            Some(Arc::clone(&self.sync_hlc_clock)),
            Arc::clone(&self.sync_trigger),
        );
        let _ = publish_sync_fact_with_runtime(
            &runtime,
            SyncPublicationFact::ChatIndexUpserted {
                address: address.clone(),
                chat,
            },
        )
        .await;
    }

    pub(crate) fn spawn_chat_state_publication_for(
        &self,
        scope_root: &Path,
        address: &rua_core::ChatAddress,
        chat: Option<serde_json::Value>,
    ) {
        if !self.sync_enabled.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let emitter = self.clone();
        let scope_root = scope_root.to_path_buf();
        let address = address.clone();
        tokio::spawn(async move {
            if let Some(chat) = chat {
                emitter.emit_chat_index_upsert_for(&address, chat).await;
            }
            emitter
                .emit_evergreen_chat_snapshot(&scope_root, &address)
                .await;
        });
    }

    pub(crate) async fn emit_chat_deleted_for_checked(
        &self,
        scope_root: &Path,
        address: &rua_core::ChatAddress,
    ) -> Result<(), String> {
        let runtime = SyncPublicationRuntime::new(
            self.data_dir.join("sync"),
            Arc::clone(&self.sync_enabled),
            Arc::clone(&self.sync_node),
            Arc::clone(&self.sync_network_secret),
            Arc::clone(&self.sync_device),
            Some(Arc::clone(&self.sync_hlc_clock)),
            Arc::clone(&self.sync_trigger),
        );
        require_delete_publication(
            publish_sync_fact_with_runtime(
                &runtime,
                SyncPublicationFact::ChatDeleted {
                    scope_root: scope_root.to_path_buf(),
                    address: address.clone(),
                },
            )
            .await,
            "Chat",
        )
    }

    pub(crate) async fn emit_media_deleted_for_checked(
        &self,
        owner: rua_sync::media_adapter::MediaOwner,
        media_id: &str,
    ) -> Result<(), String> {
        let runtime = SyncPublicationRuntime::new(
            self.data_dir.join("sync"),
            Arc::clone(&self.sync_enabled),
            Arc::clone(&self.sync_node),
            Arc::clone(&self.sync_network_secret),
            Arc::clone(&self.sync_device),
            Some(Arc::clone(&self.sync_hlc_clock)),
            Arc::clone(&self.sync_trigger),
        );
        require_delete_publication(
            publish_sync_fact_with_runtime(
                &runtime,
                SyncPublicationFact::MediaDeleted {
                    owner,
                    media_id: media_id.to_string(),
                },
            )
            .await,
            "Media",
        )
    }

    pub(crate) async fn emit_agent_config_committed(
        &self,
        agent_id: &str,
        config: serde_json::Value,
    ) {
        let Some(device_key) = self.device_key.as_ref() else {
            tracing::warn!(
                agent_id,
                "skip authority Agent config publication without Device Key"
            );
            return;
        };
        let portable = match crate::sync_publication::normalize_agent_config_for_publication(
            &config, device_key,
        ) {
            Ok(portable) => portable,
            Err(error) => {
                tracing::warn!(
                    agent_id,
                    error = %error,
                    "skip authority Agent config publication normalization"
                );
                return;
            }
        };
        let runtime = SyncPublicationRuntime::new(
            self.data_dir.join("sync"),
            Arc::clone(&self.sync_enabled),
            Arc::clone(&self.sync_node),
            Arc::clone(&self.sync_network_secret),
            Arc::clone(&self.sync_device),
            Some(Arc::clone(&self.sync_hlc_clock)),
            Arc::clone(&self.sync_trigger),
        );
        let _ = publish_sync_fact_with_runtime(
            &runtime,
            SyncPublicationFact::AgentConfigCommitted {
                agent_id: agent_id.to_string(),
                config: portable,
            },
        )
        .await;
    }
}

fn require_delete_publication(
    result: crate::sync_publication::PublicationResult,
    label: &str,
) -> Result<(), String> {
    match result.status {
        PublicationStatus::Published | PublicationStatus::SkippedDisabled => Ok(()),
        PublicationStatus::SkippedUnbound => {
            Err(format!("{label} delete Sync publication is unavailable"))
        }
        PublicationStatus::AppendFailed => Err(format!(
            "{label} delete Sync publication failed: {}",
            result.errors.join("; ")
        )),
    }
}

impl ConsoleChatService {
    pub fn new(
        registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
        streams: Arc<ActiveStreamRegistry>,
        sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
        host_profile: impl Into<String>,
    ) -> Self {
        Self {
            streams,
            push_store: None,
            agent_service: None,
            registry,
            sync_device,
            host_profile: host_profile.into(),
            session_sync: None,
            acp_chat_registry: None,
            tool_config_cells: None,
            tool_config_device_key: None,
            tool_catalog: None,
            provider_manager: None,
            conversation_authority: None,
            project_bindings: None,
        }
    }

    pub fn with_project_bindings(
        mut self,
        bindings: Arc<rua_project::ProjectResourceBindingStore>,
    ) -> Self {
        self.project_bindings = Some(bindings);
        self
    }

    pub fn with_provider_manager(
        mut self,
        provider_manager: Arc<rua_provider::manager::ProviderManager>,
    ) -> Self {
        self.provider_manager = Some(provider_manager);
        self
    }

    pub fn with_conversation_authority(mut self, authority: Arc<ConversationAuthority>) -> Self {
        self.conversation_authority = Some(authority);
        self
    }

    pub fn with_acp_chat_registry(
        mut self,
        registry: Arc<crate::services::acp_chat_registry::AcpChatRegistry>,
    ) -> Self {
        self.acp_chat_registry = Some(registry);
        self
    }

    pub fn with_push_store(mut self, store: PushMessageStore) -> Self {
        self.push_store = Some(store);
        self
    }

    pub fn with_agent_service(mut self, svc: Arc<AgentManager>) -> Self {
        self.agent_service = Some(svc);
        self
    }

    pub fn with_session_sync_emitter(mut self, emitter: SessionSyncEmitter) -> Self {
        self.session_sync = Some(emitter);
        self
    }

    pub fn with_tool_config_context(
        mut self,
        cells: Arc<rua_tool::AgentToolsCells>,
        device_key: [u8; 32],
        catalog: Arc<serde_json::Map<String, serde_json::Value>>,
    ) -> Self {
        self.tool_config_cells = Some(cells);
        self.tool_config_device_key = Some(device_key);
        self.tool_catalog = Some(catalog);
        self
    }
}

async fn load_tool_configs_for_runtime(
    cells: Option<Arc<rua_tool::AgentToolsCells>>,
    device_key: Option<[u8; 32]>,
    catalog: Option<Arc<serde_json::Map<String, serde_json::Value>>>,
    agent_id: &str,
) -> Option<serde_json::Value> {
    let cells = cells?;
    let mut store = match device_key {
        Some(key) => rua_tool::ToolConfigStore::new(cells).with_device_key(key),
        None => rua_tool::ToolConfigStore::new(cells),
    };
    if let Some(catalog) = catalog {
        store = store.with_catalog((*catalog).clone());
    }
    match store.list_configs_unmasked(Some(agent_id)).await {
        Ok(configs) if configs.is_empty() => None,
        Ok(configs) => Some(serde_json::Value::Object(configs)),
        Err(error) => {
            warn!(agent_id, error = %error, "failed to load tool configs for runtime metadata");
            None
        }
    }
}

pub(crate) fn snapshot_tool_configuration(
    tools: Option<&serde_json::Value>,
) -> rua_tool::tool_config_secret_io::ToolConfigSnapshot {
    let mut configs = serde_json::Map::new();
    if let Some(entries) = tools
        .and_then(|tools| tools.get("builtin_tools"))
        .and_then(serde_json::Value::as_object)
    {
        for (name, entry) in entries {
            if let Some(config) = entry.get("config").filter(|config| config.is_object()) {
                configs.insert(name.clone(), config.clone());
            }
        }
    }
    rua_tool::tool_config_secret_io::ToolConfigSnapshot::capture(&serde_json::Value::Object(
        configs,
    ))
}

pub fn map_runtime_event_to_chat_events(
    event: RuntimeEvent,
    chat_id: &str,
    agent_id: &str,
) -> Vec<dto::ChatEvent> {
    match event {
        RuntimeEvent::Start { .. } => vec![dto::ChatEvent::Start {
            chat_id: chat_id.to_string(),
            agent_id: Some(agent_id.to_string()),
        }],
        RuntimeEvent::TextDelta { delta, message_id } => match message_id {
            Some(message_id) => vec![dto::ChatEvent::TextDelta {
                message_id: message_id.to_string(),
                delta,
            }],
            None => vec![dto::ChatEvent::Error {
                detail: "text delta is missing canonical message_id".to_string(),
            }],
        },
        RuntimeEvent::ThinkingDelta {
            delta, message_id, ..
        } => match message_id {
            Some(message_id) => vec![dto::ChatEvent::ThinkingDelta {
                message_id: message_id.to_string(),
                delta,
            }],
            None => vec![dto::ChatEvent::Error {
                detail: "thinking delta is missing canonical message_id".to_string(),
            }],
        },
        RuntimeEvent::ModelCallReset { model_call_id } => {
            vec![dto::ChatEvent::ModelCallReset { model_call_id }]
        }

        RuntimeEvent::ToolCallDelta {
            id,
            index,
            name,
            arguments_delta,
        } => {
            if id.is_empty() {
                vec![dto::ChatEvent::Error {
                    detail: "tool call delta is missing canonical message_id".to_string(),
                }]
            } else {
                vec![dto::ChatEvent::ToolCallDelta {
                    message_id: agent_types::conversation_message_identity::tool_use_message_id(
                        &id,
                    ),
                    id,
                    index,
                    name,
                    arguments_delta,
                }]
            }
        }
        RuntimeEvent::ToolCall { call } => {
            if call.id.is_empty() {
                return vec![dto::ChatEvent::Error {
                    detail: "tool call is missing canonical message_id".to_string(),
                }];
            }
            debug!(
                tool_call_id = %call.id,
                tool_name = %call.name,
                arguments = %call.arguments,
                "map_runtime_event_to_chat_events: ToolCall"
            );
            vec![dto::ChatEvent::ToolCall {
                message_id: agent_types::conversation_message_identity::tool_use_message_id(
                    &call.id,
                ),
                id: call.id.clone(),
                name: call.name,
                arguments: call.arguments.to_string(),
            }]
        }
        RuntimeEvent::ToolResult { result } => {
            if result.call_id.is_empty() {
                return vec![dto::ChatEvent::Error {
                    detail: "tool result is missing canonical message_id".to_string(),
                }];
            }
            let mut events = vec![dto::ChatEvent::ToolResult {
                message_id: agent_types::conversation_message_identity::tool_result_message_id(
                    &result.call_id,
                ),
                id: result.call_id.as_str().to_string(),
                name: result.name.clone(),
                content: content_parts_to_json(&result.content),
            }];

            let file_content_parts = extract_file_content_parts(&result.content);
            if !file_content_parts.is_empty() {
                events.push(dto::ChatEvent::Message {
                    id: agent_types::conversation_message_identity::tool_result_attachment_message_id(
                        &result.call_id,
                    ),
                    object: "message".to_string(),
                    role: "assistant".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::Value::Array(file_content_parts),
                });
            }

            events
        }
        RuntimeEvent::ToolResultDelta {
            id,
            name,
            output_delta,
        } => {
            if id.is_empty() {
                vec![dto::ChatEvent::Error {
                    detail: "tool result delta is missing canonical message_id".to_string(),
                }]
            } else {
                vec![dto::ChatEvent::ToolResultDelta {
                    message_id: agent_types::conversation_message_identity::tool_result_message_id(
                        &id,
                    ),
                    id,
                    name,
                    output_delta,
                }]
            }
        }
        RuntimeEvent::ContextCompaction {
            status,
            mode,
            message,
        } => vec![dto::ChatEvent::ContextCompaction {
            status,
            mode,
            message,
        }],
        RuntimeEvent::Usage { .. } => vec![],
        RuntimeEvent::Error { error } => vec![dto::ChatEvent::Error {
            detail: error.to_string(),
        }],
        RuntimeEvent::End { finish_reason } => vec![dto::ChatEvent::Done { finish_reason }],
        RuntimeEvent::ToolApprovalPending {
            call_id,
            tool_name,
            request_id,
            prompt,
        } => {
            let id = request_id.unwrap_or_else(|| call_id.clone());
            let content = serde_json::json!([{
                "type": "tool_approval_request",
                "data": {
                    "id": &id,
                    "approval_request_id": &id,
                    "call_id": &call_id,
                    "name": &tool_name,
                    "prompt": prompt,
                    "status": "waiting"
                }
            }]);
            vec![dto::ChatEvent::ToolApprovalRequest {
                object: "message".to_string(),
                id,
                role: "assistant".to_string(),
                status: "in_progress".to_string(),
                content,
            }]
        }
        RuntimeEvent::IterationStart { .. } | RuntimeEvent::IterationEnd { .. } => vec![],
        RuntimeEvent::AskUser {
            request_id,
            questions,
        } => {
            let content = serde_json::json!([{
                "type": "ask_user_request",
                "data": {
                    "id": &request_id,
                    "request_id": &request_id,
                    "questions": &questions,
                    "prompt": questions.first().map(|question| question.question.clone()),
                    "status": "waiting"
                }
            }]);
            vec![dto::ChatEvent::AskUserRequest {
                object: "message".to_string(),
                id: request_id,
                role: "assistant".to_string(),
                status: "in_progress".to_string(),
                content,
            }]
        }
        RuntimeEvent::ToolApprovalRequest {
            id,
            name,
            arguments,
        } => {
            let content = serde_json::json!([{
                "type": "tool_approval_request",
                "data": {
                    "id": &id,
                    "name": &name,
                    "arguments": &arguments
                }
            }]);
            vec![dto::ChatEvent::ToolApprovalRequest {
                object: "message".to_string(),
                id,
                role: "assistant".to_string(),
                status: "completed".to_string(),
                content,
            }]
        }
    }
}

fn strip_file_scheme(url: &str) -> &str {
    url.strip_prefix("file://").unwrap_or(url)
}

fn extract_file_content_parts(parts: &[ContentPart]) -> Vec<serde_json::Value> {
    parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::File { url, name, .. } => Some(serde_json::json!({
                "type": "file",
                "file_url": strip_file_scheme(url),
                "filename": name,
            })),
            ContentPart::Image { url, .. } => Some(serde_json::json!({
                "type": "image",
                "image_url": strip_file_scheme(url),
            })),
            ContentPart::Audio { url, .. } => Some(serde_json::json!({
                "type": "audio",
                "audio_url": strip_file_scheme(url),
            })),
            ContentPart::Video { url, .. } => Some(serde_json::json!({
                "type": "video",
                "video_url": strip_file_scheme(url),
            })),
            _ => None,
        })
        .collect()
}

fn content_parts_to_json(parts: &[ContentPart]) -> serde_json::Value {
    if parts
        .iter()
        .all(|part| matches!(part, ContentPart::Text { .. }))
    {
        return serde_json::Value::String(
            parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        );
    }

    serde_json::Value::Array(
        parts
            .iter()
            .map(|p| {
                serde_json::to_value(p).unwrap_or_else(
                    |_| serde_json::json!({"type": "text", "text": format!("{:?}", p)}),
                )
            })
            .collect(),
    )
}

pub(crate) fn extract_text_from_content_json(content: &serde_json::Value) -> String {
    if let Some(arr) = content.as_array() {
        arr.iter()
            .filter_map(|item| {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    item.get("text").and_then(|t| t.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("")
    } else if let Some(s) = content.as_str() {
        s.to_string()
    } else {
        content.to_string()
    }
}

fn missing_chat(chat_id: &str) -> rua_core::RuaError {
    rua_core::RuaError::NotFound {
        resource: "Chat".into(),
        id: chat_id.into(),
    }
}

fn is_placeholder_chat_title(title: &str) -> bool {
    let title = title.trim();
    title.is_empty() || title == "New Chat"
}

async fn save_chat_to_chat_index_with_activity(
    workspace_dir: &Path,
    chat_id: &str,
    chat_name: &str,
    device_activity: Option<&ChatDeviceActivityUpdate>,
) -> rua_core::Result<serde_json::Value> {
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let name = truncate_chat_name(chat_name);
    let now = Utc::now();
    let updated = store
        .update_chat_header(chat_id, |header| {
            if !name.is_empty() && is_placeholder_chat_title(&header.metadata.title) {
                header.metadata.title = name.clone();
            }
            header.metadata.last_activity_at = now.to_rfc3339();
            if let Some(activity) = device_activity {
                let mut meta = serde_json::Value::Object(header.extra.clone());
                apply_chat_activity(&mut meta, activity, false);
                copy_chat_activity_to_header_extra(&meta, &mut header.extra);
            }
        })
        .await?;
    if !updated {
        return Err(missing_chat(chat_id));
    }
    let header = store
        .load_chat_header(chat_id)
        .await?
        .ok_or_else(|| missing_chat(chat_id))?;
    let existing = chat_index::get(workspace_dir, chat_id)
        .await
        .map_err(|message| rua_core::RuaError::Io { message })?;
    let entry = rua_chat::project_chat_header(&header, existing)?;
    chat_index::upsert(workspace_dir, entry.clone())
        .await
        .map_err(|message| rua_core::RuaError::Io { message })?;
    Ok(chat_index_entry_to_json(&entry))
}

#[derive(Debug)]
pub enum TargetedChatMaterializationError {
    InvalidChatId(String),
    InvalidAuthorityPlacement(ConversationAuthorityPlacementError),
    Io(std::io::Error),
    Json(serde_json::Error),
    Core(rua_core::RuaError),
    Metadata(String),
}

impl std::fmt::Display for TargetedChatMaterializationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidChatId(message) => write!(f, "invalid targeted chat id: {message}"),
            Self::InvalidAuthorityPlacement(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "io: {error}"),
            Self::Json(error) => write!(f, "json: {error}"),
            Self::Core(error) => write!(f, "session: {error}"),
            Self::Metadata(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for TargetedChatMaterializationError {}

impl From<std::io::Error> for TargetedChatMaterializationError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for TargetedChatMaterializationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<ConversationAuthorityPlacementError> for TargetedChatMaterializationError {
    fn from(error: ConversationAuthorityPlacementError) -> Self {
        Self::InvalidAuthorityPlacement(error)
    }
}

impl From<rua_core::RuaError> for TargetedChatMaterializationError {
    fn from(error: rua_core::RuaError) -> Self {
        Self::Core(error)
    }
}

impl From<ChatMetadataStoreError> for TargetedChatMaterializationError {
    fn from(error: ChatMetadataStoreError) -> Self {
        match error {
            ChatMetadataStoreError::InvalidAuthorityPlacement(error) => {
                Self::InvalidAuthorityPlacement(error)
            }
            ChatMetadataStoreError::Io(error) => Self::Io(error),
            ChatMetadataStoreError::Json(error) => Self::Json(error),
            ChatMetadataStoreError::Session(error) => Self::Core(error),
            ChatMetadataStoreError::Conflict(message)
            | ChatMetadataStoreError::Internal(message) => Self::Metadata(message),
        }
    }
}

pub async fn materialize_targeted_chat(
    workspace_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    chat_name: &str,
    authority_placement: &crate::services::conversation_authority_placement::ConversationAuthorityPlacement,
) -> std::result::Result<serde_json::Value, TargetedChatMaterializationError> {
    materialize_targeted_chat_with_model(
        workspace_dir,
        agent_id,
        chat_id,
        chat_name,
        authority_placement,
        None,
    )
    .await
}

pub(crate) async fn materialize_targeted_chat_with_model(
    workspace_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    chat_name: &str,
    authority_placement: &crate::services::conversation_authority_placement::ConversationAuthorityPlacement,
    initial_model: Option<serde_json::Value>,
) -> std::result::Result<serde_json::Value, TargetedChatMaterializationError> {
    resolve_chat_id(chat_id).map_err(TargetedChatMaterializationError::InvalidChatId)?;
    let address = rua_core::ChatAddress::agent(agent_id, chat_id);
    let chat = materialize_targeted_chat_metadata(
        None,
        workspace_dir,
        &address,
        agent_id,
        chat_name,
        authority_placement,
        initial_model,
    )
    .await?;

    let scope_store =
        rua_core::ChatScopeStore::for_agent_workspace(workspace_dir, address.clone())?;
    rua_chat::ChatService::for_scope_store(scope_store)
        .complete_agent_scope_materialization(&address)
        .await?;
    Ok(chat.to_json_value())
}

pub async fn append_remote_submitted_user_turn(
    workspace_dir: &Path,
    request: &RemoteConversationInputRequest,
) -> rua_core::Result<AppendedSubmittedUserTurn> {
    materialize_targeted_chat(
        workspace_dir,
        &request.agent_id,
        request.chat_id(),
        &request.text,
        &crate::services::conversation_authority_placement::ConversationAuthorityPlacement {
            device_id: request.target_device_id.clone(),
            authority_epoch: request.authority_epoch,
            base_manifest_hash: request.base_manifest_hash.clone(),
        },
    )
    .await
    .map_err(|e| rua_core::RuaError::Runtime {
        message: format!("materialize targeted chat: {e}"),
    })?;

    let user_message = AgentInputMessage {
        role: "user".to_string(),
        content: vec![ContentPart::Text {
            text: request.text.clone(),
        }],
    };
    let mut stored = StoredMessage::from_input_message(
        &user_message,
        &request.agent_id,
        &request.target_device_id,
    );
    stored.metadata = device_delivery_message_metadata(request);
    let message = stored_message_to_conversation_message(&stored);
    let turn_id = format!("turn-{}", uuid::Uuid::new_v4());
    let user_message = conversation_user_message_payload(&message, &turn_id);
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    store
        .append_conversation_turn(
            request.chat_id(),
            NewConversationTurn {
                turn_id: turn_id.clone(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: message.created_at.clone(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![message],
            },
        )
        .await?;
    Ok(AppendedSubmittedUserTurn {
        turn_id,
        user_message,
    })
}

fn device_delivery_message_metadata(request: &RemoteConversationInputRequest) -> serde_json::Value {
    serde_json::json!({
        "submission_id": request.submission_id,
        "device_delivery_id": request.device_delivery_id,
        "device_delivery_sequence": request.device_delivery_sequence,
        "source_device_id": request.source_device_id,
        "target_device_id": request.target_device_id,
        "executed_on_device_id": request.target_device_id,
        "authority_device_id": request.target_device_id,
        "authority_epoch": request.authority_epoch,
    })
}

pub(crate) fn resolve_chat_id(request_chat_id: &str) -> std::result::Result<String, String> {
    if uuid::Uuid::parse_str(request_chat_id).is_ok() {
        tracing::info!(
            target: "diag.session",
            request_chat_id = %request_chat_id,
            outcome = "uuid_passthrough",
            "[DIAG-session] resolve_chat_id",
        );
        return Ok(request_chat_id.to_string());
    }
    Err(format!("expected UUID chat_id, got: {request_chat_id}"))
}

pub fn truncate_chat_name(msg: &str) -> String {
    let trimmed = msg.trim();
    if trimmed.chars().count() <= 80 {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(77).collect();
        format!("{head}…")
    }
}

pub(crate) fn chat_index_entry_to_json(entry: &chat_index::ChatIndexEntry) -> serde_json::Value {
    serde_json::json!({
        "id": entry.id.clone(),
        "name": entry.name.clone(),
        "user_id": entry.user_id.clone(),
        "channel": entry.channel.clone(),
        "created_at": entry.created_at.to_rfc3339(),
        "updated_at": entry.updated_at.to_rfc3339(),
        "meta": entry.meta.clone(),
        "status": entry.status.clone(),
        "pinned": entry.pinned,
        "pin_order": entry.pin_order,
    })
}

pub struct WorkspaceSessionPersistence {
    workspace_dir: PathBuf,
    address: Option<ChatAddress>,
    loaded_message_count: usize,
    active_turn_id: Option<String>,
    regenerate_turn_id: Option<String>,
    system_prompt: Option<String>,
    session_context_snapshot: SessionContextSnapshot,
    device_id: String,
    authority_device_id: String,
    authority_epoch: Option<ConversationAuthorityEpoch>,
    device_activity: Option<ChatDeviceActivityUpdate>,
    session_sync: Option<SessionSyncEmitter>,
    context_manager: Option<Arc<dyn ContextManager>>,
    running_config: Option<Arc<AgentsRunningConfig>>,
    checkpoint_model: Option<CheckpointModelIdentity>,
    model_call_identity: Option<rua_agent::model_call_context::ModelCallIdentityContext>,
    conversation_authority: Option<Arc<ConversationAuthority>>,

    acp_origin_runner: Option<String>,
    assistant_started_at: Option<String>,
}

impl WorkspaceSessionPersistence {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            workspace_dir,
            address: None,
            loaded_message_count: 0,
            active_turn_id: None,
            regenerate_turn_id: None,
            system_prompt: None,
            session_context_snapshot: SessionContextSnapshot::default(),
            device_id: String::new(),
            authority_device_id: String::new(),
            authority_epoch: None,
            device_activity: None,
            session_sync: None,
            context_manager: None,
            running_config: None,
            checkpoint_model: None,
            model_call_identity: None,
            conversation_authority: None,
            acp_origin_runner: None,
            assistant_started_at: None,
        }
    }

    pub fn with_address(mut self, address: ChatAddress) -> Self {
        self.address = Some(address);
        self
    }

    pub fn with_acp_origin_runner(mut self, runner: String) -> Self {
        self.acp_origin_runner = Some(runner);
        self
    }

    pub fn with_assistant_started_at(mut self, started_at: String) -> Self {
        self.assistant_started_at = Some(started_at);
        self
    }

    pub fn with_loaded_message_count(mut self, count: usize) -> Self {
        self.loaded_message_count = count;
        self
    }

    pub fn with_active_turn_id(mut self, turn_id: String) -> Self {
        self.active_turn_id = Some(turn_id);
        self
    }

    pub fn with_regenerate_turn_id(mut self, turn_id: String) -> Self {
        self.regenerate_turn_id = Some(turn_id);
        self
    }

    pub fn with_system_prompt(mut self, prompt: String) -> Self {
        self.system_prompt = Some(prompt);
        self
    }

    pub fn with_session_context_snapshot(mut self, snapshot: SessionContextSnapshot) -> Self {
        self.session_context_snapshot = snapshot;
        self
    }

    pub fn with_device_id(mut self, device_id: String) -> Self {
        self.device_id = device_id;
        self
    }

    pub fn with_authority(
        mut self,
        device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
    ) -> Self {
        self.authority_device_id = device_id;
        self.authority_epoch = Some(authority_epoch);
        self
    }

    pub fn with_device_activity(mut self, device_activity: ChatDeviceActivityUpdate) -> Self {
        self.device_activity = Some(device_activity);
        self
    }

    pub fn with_session_sync_emitter(mut self, emitter: SessionSyncEmitter) -> Self {
        self.session_sync = Some(emitter);
        self
    }

    pub fn with_context_manager(mut self, manager: Arc<dyn ContextManager>) -> Self {
        self.context_manager = Some(manager);
        self
    }

    pub fn with_running_config(mut self, running: Arc<AgentsRunningConfig>) -> Self {
        self.running_config = Some(running);
        self
    }

    pub fn with_checkpoint_model(mut self, model: CheckpointModelIdentity) -> Self {
        self.checkpoint_model = Some(model);
        self
    }

    pub fn with_model_call_identity(
        mut self,
        context: Option<rua_agent::model_call_context::ModelCallIdentityContext>,
    ) -> Self {
        self.model_call_identity = context;
        self
    }

    pub fn with_conversation_authority(mut self, authority: Arc<ConversationAuthority>) -> Self {
        self.conversation_authority = Some(authority);
        self
    }

    async fn save_messages_inner(
        &self,
        agent_id: &str,
        chat_id: &str,
        messages: &[AgentInputMessage],
        assistant_started_at: Option<&str>,
        usage: Option<serde_json::Value>,
        last_call_usage: Option<serde_json::Value>,
    ) -> rua_core::Result<()> {
        let store = rua_session::WorkspaceSessionStore::new(&self.workspace_dir);
        save_session_context_snapshot_to_chat_header(
            &store,
            chat_id,
            &self.session_context_snapshot,
        )
        .await?;
        let header = store.load_chat_header(chat_id).await?;
        let force_in_progress = header_has_waiting_pending_action(header.as_ref());
        let mut new_messages = messages
            .iter()
            .skip(self.loaded_message_count)
            .map(|message| {
                let mut stored =
                    StoredMessage::from_input_message(message, agent_id, &self.device_id);
                if let Some(runner) = &self.acp_origin_runner {
                    stamp_acp_origin(&mut stored, runner);
                }
                if let Some(authority_epoch) = self.authority_epoch {
                    stored.extra.insert(
                        "authority_device_id".to_string(),
                        serde_json::json!(self.authority_device_id),
                    );
                    stored.extra.insert(
                        "authority_epoch".to_string(),
                        serde_json::json!(authority_epoch),
                    );
                }
                stored_message_to_conversation_message(&stored)
            })
            .collect::<Vec<_>>();
        let max_input_length = self
            .running_config
            .as_ref()
            .map(|config| config.max_input_length.max(1) as u64);
        attach_token_usage_to_closing_assistant(
            &mut new_messages,
            usage,
            last_call_usage,
            max_input_length,
        );
        let publication_boundary = if let Some(turn_id) = self.active_turn_id.as_deref() {
            let writer = TurnWriter::new(&store, chat_id, turn_id);
            writer
                .append_output(new_messages, force_in_progress, assistant_started_at)
                .await?;
            writer
                .is_complete_model_call_boundary()
                .await?
                .then_some(ConversationPublicationBoundary::ModelCall)
        } else {
            let syncable_turn = append_evergreen_messages(
                &store,
                chat_id,
                self.regenerate_turn_id.as_deref(),
                new_messages,
                force_in_progress,
                assistant_started_at,
            )
            .await?;
            syncable_turn.then_some(ConversationPublicationBoundary::ModelCall)
        };
        self.after_turn_write(agent_id, chat_id, publication_boundary)
            .await
    }

    async fn finalize_turn_inner(
        &self,
        agent_id: &str,
        chat_id: &str,
        assistant_started_at: Option<&str>,
    ) -> rua_core::Result<()> {
        let store = rua_session::WorkspaceSessionStore::new(&self.workspace_dir);
        save_session_context_snapshot_to_chat_header(
            &store,
            chat_id,
            &self.session_context_snapshot,
        )
        .await?;
        let header = store.load_chat_header(chat_id).await?;
        let force_in_progress = header_has_waiting_pending_action(header.as_ref());
        let turn_id =
            self.active_turn_id
                .as_deref()
                .ok_or_else(|| rua_core::RuaError::Runtime {
                    message: format!("Turn finalization requires an active Turn for {chat_id}"),
                })?;
        let record = TurnWriter::new(&store, chat_id, turn_id)
            .finalize(force_in_progress, assistant_started_at)
            .await?;
        let publication_boundary = TurnLifecycleStatus::parse(&record.lifecycle.status)
            .is_syncable_conversation_state()
            .then_some(ConversationPublicationBoundary::Terminal);
        self.after_turn_write(agent_id, chat_id, publication_boundary)
            .await
    }

    async fn after_turn_write(
        &self,
        agent_id: &str,
        chat_id: &str,
        publication_boundary: Option<ConversationPublicationBoundary>,
    ) -> rua_core::Result<()> {
        if self.acp_origin_runner.is_none() {
            if let (Some(authority), Some(context_manager), Some(running_config), Some(model)) = (
                self.conversation_authority.as_ref(),
                self.context_manager.as_ref(),
                self.running_config.as_ref(),
                self.checkpoint_model.as_ref(),
            ) {
                authority.schedule_history_replacement(
                    crate::services::history_replacement::HistoryReplacementJobContext {
                        workspace_dir: self.workspace_dir.clone(),
                        chat_address: self
                            .address
                            .clone()
                            .unwrap_or_else(|| rua_core::ChatAddress::agent(agent_id, chat_id)),
                        agent_id: agent_id.to_string(),
                        authority_device_id: self.authority_device_id.clone(),
                        context_manager: Arc::clone(context_manager),
                        running_config: Arc::clone(running_config),
                        model: model.clone(),
                        session_sync: self.session_sync.clone(),
                    },
                );
            }
            maybe_spawn_early_background_compaction(EarlyBackgroundCompactionJobInput {
                store: rua_session::WorkspaceSessionStore::new(&self.workspace_dir),
                workspace_dir: self.workspace_dir.clone(),
                agent_id: agent_id.to_string(),
                chat_id: chat_id.to_string(),
                chat_address: self
                    .address
                    .clone()
                    .unwrap_or_else(|| ChatAddress::agent(agent_id, chat_id)),
                context_manager: self.context_manager.clone(),
                running_config: self.running_config.clone(),
                model: self.checkpoint_model.clone(),
                system_prompt: self.system_prompt.clone(),
                session_sync: self.session_sync.clone(),
                model_call_identity: Some(
                    rua_agent::model_call_context::ModelCallIdentityContext::from_chat_address(
                        &self
                            .address
                            .clone()
                            .unwrap_or_else(|| ChatAddress::agent(agent_id, chat_id)),
                    ),
                ),
            })
            .await;
        }
        if let Some(publication_boundary) = publication_boundary {
            if let Some(emitter) = &self.session_sync {
                let address = self
                    .address
                    .clone()
                    .unwrap_or_else(|| ChatAddress::agent(agent_id, chat_id));
                emitter
                    .publish_conversation_boundary(
                        &self.workspace_dir,
                        &address,
                        publication_boundary,
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl SessionPersistence for WorkspaceSessionPersistence {
    async fn save_messages(
        &self,
        agent_id: &str,
        chat_id: &str,
        messages: &[AgentInputMessage],
    ) -> rua_core::Result<()> {
        self.save_messages_inner(
            agent_id,
            chat_id,
            messages,
            self.assistant_started_at.as_deref(),
            None,
            None,
        )
        .await
    }

    async fn save_messages_with_assistant_started_at(
        &self,
        agent_id: &str,
        chat_id: &str,
        messages: &[AgentInputMessage],
        assistant_started_at: Option<&str>,
    ) -> rua_core::Result<()> {
        self.save_messages_inner(
            agent_id,
            chat_id,
            messages,
            assistant_started_at.or(self.assistant_started_at.as_deref()),
            None,
            None,
        )
        .await
    }

    async fn save_messages_with_token_usage(
        &self,
        agent_id: &str,
        chat_id: &str,
        messages: &[AgentInputMessage],
        assistant_started_at: Option<&str>,
        usage: Option<serde_json::Value>,
        last_call_usage: Option<serde_json::Value>,
    ) -> rua_core::Result<()> {
        self.save_messages_inner(
            agent_id,
            chat_id,
            messages,
            assistant_started_at.or(self.assistant_started_at.as_deref()),
            usage,
            last_call_usage,
        )
        .await
    }

    async fn finalize_turn(
        &self,
        agent_id: &str,
        chat_id: &str,
        assistant_started_at: Option<&str>,
        _usage: Option<serde_json::Value>,
        _last_call_usage: Option<serde_json::Value>,
    ) -> rua_core::Result<()> {
        self.finalize_turn_inner(
            agent_id,
            chat_id,
            assistant_started_at.or(self.assistant_started_at.as_deref()),
        )
        .await
    }

    async fn save_turn_aborted(&self, agent_id: &str, chat_id: &str) -> rua_core::Result<()> {
        let Some(turn_id) = self
            .regenerate_turn_id
            .as_deref()
            .or(self.active_turn_id.as_deref())
        else {
            return Ok(());
        };
        persist_conversation_user_stop_to_session(
            &self.workspace_dir,
            agent_id,
            chat_id,
            turn_id,
            self.session_sync.as_ref(),
        )
        .await?;
        Ok(())
    }

    async fn save_turn_interjected(&self, agent_id: &str, chat_id: &str) -> rua_core::Result<()> {
        let Some(turn_id) = self
            .regenerate_turn_id
            .as_deref()
            .or(self.active_turn_id.as_deref())
        else {
            return Ok(());
        };
        persist_conversation_interjected_to_session(
            &self.workspace_dir,
            agent_id,
            chat_id,
            turn_id,
            self.session_sync.as_ref(),
        )
        .await?;
        Ok(())
    }

    async fn save_chat_entry(
        &self,
        agent_id: &str,
        chat_id: &str,
        chat_name: &str,
    ) -> rua_core::Result<()> {
        let updated = save_chat_to_chat_index_with_activity(
            &self.workspace_dir,
            chat_id,
            chat_name,
            self.device_activity.as_ref(),
        )
        .await?;
        if let Some(emitter) = &self.session_sync {
            let chat = updated;
            if let Some(address) = self.address.as_ref() {
                emitter.emit_chat_index_upsert_for(address, chat).await;
            } else {
                emitter
                    .emit_chat_index_upsert_json(agent_id, chat_id, chat)
                    .await;
            }
        }
        Ok(())
    }
}

struct EarlyBackgroundCompactionJobInput {
    store: WorkspaceSessionStore,
    workspace_dir: PathBuf,
    agent_id: String,
    chat_id: String,
    chat_address: ChatAddress,
    context_manager: Option<Arc<dyn ContextManager>>,
    running_config: Option<Arc<AgentsRunningConfig>>,
    model: Option<CheckpointModelIdentity>,
    system_prompt: Option<String>,
    session_sync: Option<SessionSyncEmitter>,
    model_call_identity: Option<rua_agent::model_call_context::ModelCallIdentityContext>,
}

async fn maybe_spawn_early_background_compaction(input: EarlyBackgroundCompactionJobInput) {
    let EarlyBackgroundCompactionJobInput {
        store,
        workspace_dir,
        agent_id,
        chat_id,
        chat_address,
        context_manager,
        running_config,
        model,
        system_prompt,
        session_sync,
        model_call_identity,
    } = input;
    let (Some(context_manager), Some(running_config), Some(model)) =
        (context_manager, running_config, model)
    else {
        return;
    };
    if !running_config.context_compact.context_compact_enabled {
        return;
    }
    if !running_config
        .context_compact
        .early_background_compaction_enabled
    {
        return;
    }

    let target_band = context_target_band_from_running(&running_config);
    let system_context = build_system_context_accounting(
        system_prompt.into_iter().collect(),
        None,
        None,
        Some(&running_config),
        Some(running_config.max_input_length.max(1) as usize),
    );
    let projection = match store
        .build_effective_context_projection_for_address(
            &chat_address,
            ContextProjectionRequest {
                current_user_input: None,
            },
            running_config.context_compact.history_replacement_enabled,
            running_config.context_compact.token_count_estimate_divisor,
        )
        .await
    {
        Ok(projection) => projection,
        Err(e) => {
            warn!(error = %e, chat_id, "early background compaction projection failed");
            return;
        }
    };
    let checkpoint = store
        .load_active_summary_checkpoint(&chat_id)
        .await
        .ok()
        .flatten();
    let tail_start_sequence = checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.frontier.end_sequence + 1)
        .unwrap_or(1);
    let message_groups = match store
        .projection_message_group_estimates_effective_for_address(
            &chat_address,
            running_config.context_compact.history_replacement_enabled,
            running_config.context_compact.token_count_estimate_divisor,
        )
        .await
    {
        Ok(message_groups) => message_groups,
        Err(e) => {
            warn!(error = %e, chat_id, "early background compaction message-group estimate failed");
            return;
        }
    };
    let mut message_groups: Vec<_> = message_groups
        .into_iter()
        .filter(|group| group.last_sequence >= tail_start_sequence)
        .collect();
    rua_agent::projection_preparation::finalize_projection_message_group_estimates(
        &projection,
        &mut message_groups,
        &running_config,
    );
    let checkpoint_tokens = checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.token_estimate)
        .unwrap_or_default();
    let planner = ProjectionPlanner::new(target_band);
    let projected_conversation_tokens =
        projection_tokens_after_tool_truncation(&projection, &running_config);
    let plan = planner.plan(ProjectionPlanningInput {
        entry_mode: ProjectionEntryMode::EarlyBackgroundCompaction,
        current_projection_tokens: early_compaction_total_tokens(
            &projection,
            projected_conversation_tokens,
            system_context.system_context_tokens,
        ),
        checkpoint_tokens,
        system_context_tokens: system_context.system_context_tokens,
        pending_user_input_tokens: 0,
        message_groups,
    });
    let policy = EarlyBackgroundCompactionPolicy;
    let EarlyBackgroundCompactionDecision::ScheduleCheckpoint { .. } =
        policy.evaluate(EarlyBackgroundCompactionInput { plan: plan.clone() })
    else {
        return;
    };
    let Some(frontier) = plan.fold_frontier else {
        return;
    };

    let early_compaction_key = early_background_compaction_key(&workspace_dir, &chat_id);
    let Some(lease) = early_background_compaction_coordinator().try_start_job(early_compaction_key)
    else {
        return;
    };
    let finish_lease = lease.clone();
    tokio::spawn(async move {
        if let Err(e) = create_summary_checkpoint(SummaryCheckpointJob {
            store,
            chat_id: chat_id.clone(),
            chat_address,
            agent_id: agent_id.clone(),
            workspace_dir: workspace_dir.clone(),
            context_manager,
            running_config,
            model,
            frontier,
            lease: Some(lease),
            session_sync,
            model_call_identity,
        })
        .await
        {
            warn!(error = %e, chat_id, "early background compaction checkpoint failed");
        }
        early_background_compaction_coordinator().finish_job(&finish_lease);
    });
}

struct SummaryCheckpointJob {
    store: WorkspaceSessionStore,
    chat_id: String,
    chat_address: ChatAddress,
    agent_id: String,
    workspace_dir: PathBuf,
    context_manager: Arc<dyn ContextManager>,
    running_config: Arc<AgentsRunningConfig>,
    model: CheckpointModelIdentity,
    frontier: rua_session::ProjectionFoldFrontier,
    lease: Option<EarlyBackgroundCompactionLease>,
    session_sync: Option<SessionSyncEmitter>,
    model_call_identity: Option<rua_agent::model_call_context::ModelCallIdentityContext>,
}

async fn create_summary_checkpoint(job: SummaryCheckpointJob) -> rua_core::Result<bool> {
    let SummaryCheckpointJob {
        store,
        chat_id,
        chat_address,
        agent_id,
        workspace_dir,
        context_manager,
        running_config,
        model,
        frontier,
        lease,
        session_sync,
        model_call_identity,
    } = job;
    let replication_hook = session_sync.map(|emitter| {
        crate::services::session_replication::SyncSessionReplicationHook::new(emitter)
            as Arc<dyn rua_contract::agent::SessionReplicationHook>
    });
    let summary_source = rua_agent::summary_checkpoint::build_summary_projection_slice(
        &store,
        &chat_address,
        &running_config,
        frontier.end_sequence,
    )
    .await?;
    let publisher = rua_agent::summary_checkpoint::SummaryCheckpointPublisher::new(
        rua_agent::summary_checkpoint::SummaryCheckpointPublisherInput {
            store,
            chat_id: chat_id.clone(),
            chat_address,
            agent_id: agent_id.clone(),
            context_manager: Some(context_manager),
            running_config,
            model,
            workspace_dir: workspace_dir.clone(),
            replication_hook,
            model_call_identity,
        },
    );
    let publish = publisher.publish(&frontier, summary_source, || {
        lease.as_ref().is_none_or(|lease| {
            !lease.is_cancelled() && early_background_compaction_coordinator().can_publish(lease)
        })
    });
    let published = if let Some(active_lease) = lease.as_ref() {
        tokio::select! {
            biased;
            _ = active_lease.cancelled() => return Ok(false),
            result = publish => result?,
        }
    } else {
        publish.await?
    };
    Ok(published.is_some())
}

fn early_background_compaction_key(workspace_dir: &Path, chat_id: &str) -> String {
    format!("{}::{chat_id}", workspace_dir.display())
}

async fn append_evergreen_messages(
    store: &rua_session::WorkspaceSessionStore,
    chat_id: &str,
    regenerate_turn_id: Option<&str>,
    new_messages: Vec<NewConversationMessage>,
    force_in_progress: bool,
    assistant_started_at_override: Option<&str>,
) -> rua_core::Result<bool> {
    if new_messages.is_empty() {
        return Ok(false);
    }

    if let Some(turn_id) = regenerate_turn_id {
        let variant_messages = new_messages
            .into_iter()
            .filter(|message| message.role != "user")
            .collect::<Vec<_>>();
        if variant_messages.is_empty() {
            return Ok(false);
        }
        let assistant_started_at =
            assistant_started_at_override
                .map(str::to_string)
                .or_else(|| {
                    variant_messages
                        .iter()
                        .find(|message| message.role == "assistant")
                        .map(|message| message.created_at.clone())
                });
        let assistant_completed_at =
            if force_in_progress || has_unmatched_tool_use(&variant_messages) {
                None
            } else {
                variant_messages
                    .iter()
                    .rev()
                    .find(|message| message.role == "assistant")
                    .map(|message| message.created_at.clone())
            };
        let status = if assistant_completed_at.is_some() {
            "completed"
        } else {
            "in_progress"
        }
        .to_string();
        let selected_variant_id = variant_messages
            .iter()
            .rev()
            .find(|message| message.role == "assistant")
            .map(|message| message.message_id.clone());
        let turns = store.read_turn_range(chat_id, 1, usize::MAX).await?;
        let Some(existing_turn) = turns.iter().find(|turn| turn.turn_id == turn_id) else {
            return Ok(false);
        };
        let record = store
            .append_messages_to_turn_as_new_variant(
                chat_id,
                turn_id,
                TurnLifecycle {
                    user_submitted_at: existing_turn.lifecycle.user_submitted_at.clone(),
                    assistant_started_at: assistant_started_at
                        .or_else(|| existing_turn.lifecycle.assistant_started_at.clone()),
                    assistant_completed_at,
                    status,
                },
                selected_variant_id,
                variant_messages,
            )
            .await?;
        return Ok(
            TurnLifecycleStatus::parse(&record.lifecycle.status).is_syncable_conversation_state()
        );
    }

    if new_messages
        .first()
        .is_none_or(|message| message.role != "user")
    {
        return Err(rua_core::RuaError::Runtime {
            message: format!(
                "cannot append non-user conversation output for {chat_id} without active_turn_id"
            ),
        });
    }

    let now = chrono::Utc::now().to_rfc3339();
    let user_submitted_at = new_messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| message.created_at.clone())
        .unwrap_or_else(|| now.clone());
    let assistant_started_at = assistant_started_at_override
        .map(str::to_string)
        .or_else(|| {
            new_messages
                .iter()
                .find(|message| message.role == "assistant")
                .map(|message| message.created_at.clone())
        });
    let has_open_tool_call = has_unmatched_tool_use(&new_messages);
    let assistant_completed_at = if force_in_progress || has_open_tool_call {
        None
    } else {
        new_messages
            .iter()
            .rev()
            .find(|message| message.role == "assistant")
            .map(|message| message.created_at.clone())
    };
    let status = if assistant_completed_at.is_some() {
        "completed"
    } else {
        "in_progress"
    };
    let selected_variant_id = new_messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|assistant| assistant.message_id.clone());

    let record = store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: format!("turn-{}", uuid::Uuid::new_v4()),
                lifecycle: TurnLifecycle {
                    user_submitted_at,
                    assistant_started_at,
                    assistant_completed_at,
                    status: status.to_string(),
                },
                selected_variant_id,
                messages: new_messages,
            },
        )
        .await?;
    Ok(TurnLifecycleStatus::parse(&record.lifecycle.status).is_syncable_conversation_state())
}

fn has_unmatched_tool_use(messages: &[NewConversationMessage]) -> bool {
    let mut open_tool_call_ids = std::collections::HashSet::new();
    for message in messages {
        let Some(parts) = message.content.as_array() else {
            continue;
        };
        for part in parts {
            let part_type = part.get("type").and_then(|value| value.as_str());
            match part_type {
                Some("tool_use") => {
                    if let Some(id) = part.get("id").and_then(|value| value.as_str()) {
                        open_tool_call_ids.insert(id.to_string());
                    }
                }
                Some("tool_result") => {
                    if let Some(id) = part.get("id").and_then(|value| value.as_str()) {
                        open_tool_call_ids.remove(id);
                    } else if let Some(id) = part.get("call_id").and_then(|value| value.as_str()) {
                        open_tool_call_ids.remove(id);
                    }
                }
                _ => {}
            }
        }
    }
    !open_tool_call_ids.is_empty()
}

pub(crate) struct SubmittedUserTurnMetadata<'a> {
    pub turn_id: Option<&'a str>,
    pub input_id: Option<&'a str>,
    pub acceptance_mode: Option<&'a str>,
    pub submission_id: Option<&'a str>,
    pub device_delivery: Option<&'a DeviceDeliveryMetadata>,
    pub acp_origin_runner: Option<&'a str>,
    pub memory_signal: Option<&'a MemorySignal>,
    pub turn_agent_assignment: Option<&'a serde_json::Value>,
    pub turn_model: Option<&'a serde_json::Value>,
    pub turn_thinking: Option<&'a serde_json::Value>,
    pub collaboration_mode: AgentCollaborationMode,
    pub authority_device_id: &'a str,
    pub authority_epoch: ConversationAuthorityEpoch,
}

#[derive(Debug, Clone)]
pub struct AppendedSubmittedUserTurn {
    pub turn_id: String,
    pub user_message: serde_json::Value,
}

async fn append_evergreen_submitted_user_turn(
    workspace_dir: &Path,
    agent_id: &str,
    chat_address: &ChatAddress,
    user_message: &AgentInputMessage,
    device_id: &str,
    metadata: SubmittedUserTurnMetadata<'_>,
) -> rua_core::Result<Option<AppendedSubmittedUserTurn>> {
    let Some((turn, user_message_payload)) =
        build_submitted_user_turn(agent_id, user_message, device_id, metadata)?
    else {
        return Ok(None);
    };

    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let turn_id = turn.turn_id.clone();
    store
        .append_conversation_turn_for_address(chat_address, turn)
        .await
        .map(|_| {
            Some(AppendedSubmittedUserTurn {
                turn_id,
                user_message: user_message_payload,
            })
        })
}

pub(crate) fn build_submitted_user_turn(
    agent_id: &str,
    user_message: &AgentInputMessage,
    device_id: &str,
    metadata: SubmittedUserTurnMetadata<'_>,
) -> rua_core::Result<Option<(NewConversationTurn, serde_json::Value)>> {
    let is_empty_user = user_message.content.is_empty()
        || user_message
            .content
            .iter()
            .all(|part| matches!(part, ContentPart::Text { text } if text.trim().is_empty()));
    if user_message.role != "user" || is_empty_user {
        return Ok(None);
    }

    let mut stored = StoredMessage::from_input_message(user_message, agent_id, device_id);
    if let Some(device_delivery) = metadata.device_delivery {
        stored.metadata = device_delivery.to_message_metadata();
    }
    let stored_metadata = stored
        .metadata
        .as_object_mut()
        .expect("StoredMessage metadata is an object");
    if let Some(submission_id) = metadata.submission_id {
        stored_metadata.insert(
            crate::services::conversation_authority::SUBMISSION_ID_KEY.to_string(),
            serde_json::json!(submission_id),
        );
    }
    if let Some(input_id) = metadata.input_id {
        stored_metadata.insert(
            crate::services::conversation_authority::INPUT_ID_KEY.to_string(),
            serde_json::json!(input_id),
        );
    }
    if let Some(acceptance_mode) = metadata.acceptance_mode {
        stored_metadata.insert(
            crate::services::conversation_authority::ACCEPTANCE_MODE_KEY.to_string(),
            serde_json::json!(acceptance_mode),
        );
    }
    if let Some(assignment) = metadata.turn_agent_assignment {
        stored_metadata.insert(
            rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
            assignment.clone(),
        );
    }
    if let Some(model) = metadata.turn_model {
        stored_metadata.insert(rua_chat::TURN_MODEL_KEY.to_string(), model.clone());
    }
    if let Some(thinking) = metadata.turn_thinking {
        stored_metadata.insert(rua_chat::TURN_THINKING_KEY.to_string(), thinking.clone());
    }
    stored_metadata.insert(
        rua_core::CHAT_COLLABORATION_MODE_META_KEY.to_string(),
        serde_json::json!(metadata.collaboration_mode),
    );
    stored_metadata.insert(
        "authority_device_id".to_string(),
        serde_json::json!(metadata.authority_device_id),
    );
    stored_metadata.insert(
        "authority_epoch".to_string(),
        serde_json::json!(metadata.authority_epoch),
    );
    if let Some(runner) = metadata.acp_origin_runner {
        stamp_acp_origin(&mut stored, runner);
    }
    let message = stored_message_to_conversation_message(&stored);
    let user_submitted_at = message.created_at.clone();
    let turn_id = metadata
        .turn_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("turn-{}", uuid::Uuid::new_v4()));
    let user_message_payload = conversation_user_message_payload(&message, &turn_id);
    let mut messages = vec![message];
    if let Some(signal) = metadata.memory_signal {
        messages.push(memory_signal_marker_message(
            &turn_id,
            user_submitted_at.clone(),
            signal,
        )?);
    }
    Ok(Some((
        NewConversationTurn {
            turn_id,
            lifecycle: TurnLifecycle {
                user_submitted_at,
                assistant_started_at: None,
                assistant_completed_at: None,
                status: "in_progress".to_string(),
            },
            selected_variant_id: None,
            messages,
        },
        user_message_payload,
    )))
}

async fn load_persisted_submitted_user_turn(
    scope_root: &Path,
    chat_id: &str,
    turn_id: &str,
) -> rua_core::Result<Option<AppendedSubmittedUserTurn>> {
    let store = rua_session::WorkspaceSessionStore::new(scope_root);
    let message = store
        .read_turn_messages(chat_id, turn_id)
        .await?
        .into_iter()
        .find(|message| message.role == "user");
    Ok(message.map(|message| {
        let message = NewConversationMessage {
            message_id: message.message_id,
            role: message.role,
            created_at: message.created_at,
            status: message.status,
            content: message.content,
            provider: message.provider,
            tool: message.tool,
            metadata: message.metadata,
        };
        AppendedSubmittedUserTurn {
            turn_id: turn_id.to_string(),
            user_message: conversation_user_message_payload(&message, turn_id),
        }
    }))
}

fn stored_message_to_conversation_message(message: &StoredMessage) -> NewConversationMessage {
    let created_at = if message.timestamp.trim().is_empty() {
        chrono::Utc::now().to_rfc3339()
    } else {
        message.timestamp.clone()
    };
    let mut metadata = serde_json::Map::new();
    if !message.name.is_empty() {
        metadata.insert("name".to_string(), serde_json::json!(message.name));
    }
    if !message.device_id.is_empty() {
        metadata.insert(
            "device_id".to_string(),
            serde_json::json!(message.device_id),
        );
    }
    if let Some(object) = message.metadata.as_object() {
        for (key, value) in object {
            metadata.insert(key.clone(), value.clone());
        }
    } else if !message.metadata.is_null() {
        metadata.insert("legacy_metadata".to_string(), message.metadata.clone());
    }
    for (key, value) in &message.extra {
        metadata.insert(key.clone(), value.clone());
    }

    NewConversationMessage {
        message_id: message.id.clone(),
        role: message.role.clone(),
        created_at,
        status: "completed".to_string(),
        content: serde_json::Value::Array(message.content.clone()),
        provider: None,
        tool: None,
        metadata,
    }
}

fn conversation_user_message_payload(
    message: &NewConversationMessage,
    turn_id: &str,
) -> serde_json::Value {
    let original_name = message
        .metadata
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or(&message.role);
    let mut metadata = serde_json::json!({
        "original_id": message.message_id,
        "original_name": original_name,
        "metadata": message.metadata,
        "created_at": message.created_at,
    });
    if let Some(origin) = message.metadata.get("origin").cloned() {
        metadata["origin"] = origin;
    }

    serde_json::json!({
        "id": message.message_id,
        "role": message.role,
        "type": "message",
        "status": message.status,
        "content": message.content,
        "metadata": metadata,
        "created_at": message.created_at,
        "turn_id": turn_id,
    })
}

impl ConsoleChatService {
    pub fn chat_stream(
        &self,
        agent_id: &str,
        runtime: Arc<rua_workspace::WorkspaceRuntime>,
        req: dto::ChatRequest,
        cancel_rx: tokio::sync::watch::Receiver<rua_core::RunControl>,
    ) -> Pin<
        Box<
            dyn futures::Stream<Item = std::result::Result<dto::ChatEvent, String>>
                + Send
                + 'static,
        >,
    > {
        let workspace_dir = runtime.workspace_dir();
        let chat_id = req
            .effective_chat_id()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        self.chat_stream_in_context(
            ConversationExecutionContext {
                address: ChatAddress::agent(agent_id, chat_id),
                execution_agent_id: agent_id.to_string(),
                scope_root: workspace_dir.clone(),
                working_dir: workspace_dir,
            },
            runtime,
            req,
            cancel_rx,
        )
    }

    pub fn chat_stream_in_context(
        &self,
        execution: ConversationExecutionContext,
        runtime: Arc<rua_workspace::WorkspaceRuntime>,
        mut req: dto::ChatRequest,
        cancel_rx: tokio::sync::watch::Receiver<rua_core::RunControl>,
    ) -> Pin<
        Box<
            dyn futures::Stream<Item = std::result::Result<dto::ChatEvent, String>>
                + Send
                + 'static,
        >,
    > {
        let request_chat_id = req
            .effective_chat_id()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let agent_id = execution.execution_agent_id;
        let address = execution.address;
        let scope_root = execution.scope_root;
        let working_dir = execution.working_dir;
        let streams = Arc::clone(&self.streams);
        let user_message = req.effective_message();
        let requested_regenerate_turn_id = req
            .extra
            .get("regenerate_turn_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let authority_turn_id = req
            .extra
            .get("turn_id")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let model_override = req
            .model
            .as_ref()
            .map(dto::ChatModelInput::model_id)
            .map(str::to_string);
        let agent_service = self.agent_service.clone();
        let registry = Arc::clone(&self.registry);
        let provider_manager = self.provider_manager.clone();
        let conversation_authority = self.conversation_authority.clone();
        let acp_chat_registry = self.acp_chat_registry.clone();
        let ordered_user_content = req.ordered_user_content_parts();
        let device_delivery = DeviceDeliveryMetadata::from_extra(&req.extra);
        let submission_id = req
            .extra
            .get(crate::services::conversation_authority::SUBMISSION_ID_KEY)
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let authority_input_id = req
            .extra
            .get(crate::services::conversation_authority::INPUT_ID_KEY)
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let submitted_user_turn_persisted = req
            .extra
            .get(crate::services::conversation_authority::SUBMITTED_USER_TURN_PERSISTED_KEY)
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        let hook_continuation = req
            .extra
            .get(rua_agent::runtime::HOOK_CONTINUATION_CONTEXT_METADATA_KEY)
            .and_then(serde_json::Value::as_array)
            .is_some_and(|contexts| !contexts.is_empty());
        let task_watch_notification_turn = req
            .extra
            .get(TASK_WATCH_NOTIFICATION_TURN_META_KEY)
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        let acceptance_mode = req
            .extra
            .get(crate::services::conversation_authority::ACCEPTANCE_MODE_KEY)
            .and_then(|value| value.as_str())
            .map(str::to_string);

        let agent_workspace_dir = runtime.workspace_dir();

        let sync_device = Arc::clone(&self.sync_device);
        let host_profile = self.host_profile.clone();
        let session_sync = self.session_sync.clone();
        let device_context_data_dir = session_sync.as_ref().map(|sync| sync.data_dir.clone());
        let project_bindings = self.project_bindings.clone();
        let tool_config_cells = self.tool_config_cells.clone();
        let tool_config_device_key = self.tool_config_device_key;
        let tool_catalog = self.tool_catalog.clone();

        let (tx, rx) =
            tokio::sync::mpsc::channel::<std::result::Result<dto::ChatEvent, String>>(32);
        let panic_tx = tx.clone();
        let panic_agent_id = agent_id.clone();
        let panic_request_chat_id = request_chat_id.clone();

        let chat_stream_task = tokio::spawn(async move {
            tracing::info!(
                target: "diag.session",
                agent_id = %agent_id,
                request_chat_id = %request_chat_id,
                "[DIAG-session] chat_stream task spawned",
            );
            let captured_configuration = match req
                .extra
                .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
                .cloned()
                .map(
                    serde_json::from_value::<crate::services::product_hooks::ChatHookConfiguration>,
                )
                .transpose()
            {
                Ok(configuration) => configuration,
                Err(error) => {
                    let _ = tx
                        .send(Err(format!("invalid captured Chat configuration: {error}")))
                        .await;
                    return;
                }
            };
            if let Some(configuration) = captured_configuration.as_ref() {
                if let Err(error) = configuration.validate() {
                    let _ = tx.send(Err(error)).await;
                    return;
                }
                if let Some(assignment) = configuration.agent_assignment.as_ref() {
                    req.extra.insert(
                        rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
                        serde_json::to_value(assignment).expect("Agent assignment serializes"),
                    );
                }
            }
            if !req
                .extra
                .contains_key(rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY)
            {
                let assignment = match resolve_turn_agent_assignment(
                    &agent_service,
                    &agent_id,
                    address.agent_id().is_some(),
                )
                .await
                {
                    Ok(assignment) => assignment,
                    Err(error) => {
                        let _ = tx.send(Err(error)).await;
                        return;
                    }
                };
                req.extra.insert(
                    rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
                    serde_json::to_value(assignment)
                        .expect("Turn Agent Assignment is serializable"),
                );
            }
            let turn_agent = turn_agent_context_from_extra(&req.extra, &agent_id);
            let resolved = match turn_agent {
                Some(context) => context,
                None => resolve_agent_context(&agent_service, &agent_id).await,
            };
            let system_prompt = resolved.system_prompt;
            let active_prompt_files = resolved.active_prompt_files;
            let mut disabled_tools = resolved.disabled_tools;

            let chat_id = match resolve_chat_id(&request_chat_id) {
                Ok(id) => id,
                Err(err) => {
                    tracing::error!(
                        target: "diag.session",
                        agent_id = %agent_id,
                        request_chat_id = %request_chat_id,
                        error = %err,
                        "[DIAG-session] chat_stream rejected non-UUID chat_id",
                    );
                    let _ = tx.send(Err(format!("invalid chat_id: {err}"))).await;
                    return;
                }
            };
            let collaboration_mode_override = match req
                .extra
                .remove(rua_core::TURN_COLLABORATION_MODE_OVERRIDE_KEY)
                .map(serde_json::from_value::<AgentCollaborationMode>)
                .transpose()
            {
                Ok(mode) => mode,
                Err(error) => {
                    let _ = tx
                        .send(Err(format!("invalid Agent Collaboration Mode: {error}")))
                        .await;
                    return;
                }
            };
            let chat_default_collaboration_mode =
                if let Some(configuration) = captured_configuration.as_ref() {
                    match serde_json::from_value::<AgentCollaborationMode>(
                        configuration.collaboration_mode.clone(),
                    ) {
                        Ok(mode) => mode,
                        Err(error) => {
                            let _ = tx
                                .send(Err(format!("invalid captured collaboration mode: {error}")))
                                .await;
                            return;
                        }
                    }
                } else {
                    match WorkspaceSessionStore::new(&scope_root)
                        .load_chat_header(&chat_id)
                        .await
                    {
                        Ok(Some(header)) => match header
                            .extra
                            .get(rua_core::CHAT_COLLABORATION_MODE_META_KEY)
                            .cloned()
                            .map(serde_json::from_value::<AgentCollaborationMode>)
                            .transpose()
                        {
                            Ok(mode) => mode.unwrap_or_default(),
                            Err(error) => {
                                let _ = tx
                                    .send(Err(format!(
                                        "invalid persisted Agent Collaboration Mode: {error}"
                                    )))
                                    .await;
                                return;
                            }
                        },
                        Ok(None) => AgentCollaborationMode::default(),
                        Err(error) => {
                            let _ = tx.send(Err(error.to_string())).await;
                            return;
                        }
                    }
                };
            let effective_collaboration_mode =
                collaboration_mode_override.unwrap_or(chat_default_collaboration_mode);
            tracing::info!(
                target: "diag.session",
                agent_id = %agent_id,
                request_chat_id = %request_chat_id,
                chat_id = %chat_id,
                "[DIAG-session] chat_stream chat_id resolved (will be sent on first SSE frame)",
            );
            let mut model_override = model_override;
            let mut chat_model_slot = chat_model_slot_from_extra(&req.extra);
            if model_override.is_none() {
                if let Some(model_id) = chat_model_slot.as_ref().and_then(model_id_from_slot) {
                    model_override = Some(model_id);
                }
            }
            if chat_model_slot.is_none() {
                let store = WorkspaceSessionStore::new(&scope_root);
                if let Some(model_slot) = persisted_chat_model_slot(&store, &chat_id).await {
                    if model_override.is_none() {
                        model_override = model_id_from_slot(&model_slot);
                    }
                    chat_model_slot = Some(model_slot);
                }
            }
            let active_chat_model_identity = if let Some(slot) = chat_model_slot.as_ref() {
                let provider = slot
                    .get("provider_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                let model = slot
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                match (provider, model) {
                    (Some(provider), Some(model)) => Some(CheckpointModelIdentity {
                        provider: provider.to_string(),
                        model: model.to_string(),
                    }),
                    _ => None,
                }
            } else if let Some(pm) = provider_manager.as_ref() {
                pm.default_model()
                    .await
                    .map(|model| CheckpointModelIdentity {
                        provider: model.provider_id,
                        model: model.model,
                    })
            } else {
                None
            };
            let mut history_messages = Vec::new();
            let loaded_message_count;
            let mut regenerate_turn_id = None;
            let mut conversation_recall_available = false;
            let mut memory_signal = None;
            let device_id_str = sync_device
                .read()
                .ok()
                .and_then(|guard| {
                    guard
                        .as_ref()
                        .map(|device| hex::encode(device.device_id_short()))
                })
                .unwrap_or_default();
            let device_name_snapshot = device_context_data_dir.as_ref().and_then(|data_dir| {
                crate::services::device_context::device_name_snapshot(data_dir, &device_id_str)
            });
            let mut running_config = if let Some(configuration) = captured_configuration.as_ref() {
                match configuration.running_config.resolve() {
                    Ok(running) => Some(running),
                    Err(error) => {
                        let _ = tx.send(Err(error)).await;
                        return;
                    }
                }
            } else {
                let store = WorkspaceSessionStore::new(&scope_root);
                let chat_running_config = match store.load_chat_running_config(&chat_id).await {
                    Ok(Some(running_config)) => running_config,
                    Ok(None) => {
                        let _ = tx.send(Err("Chat running configuration is missing; update the Chat configuration explicitly".to_string())).await;
                        return;
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error.to_string())).await;
                        return;
                    }
                };
                match chat_running_config.resolve() {
                    Ok(running_config) => Some(running_config),
                    Err(error) => {
                        let _ = tx
                            .send(Err(format!("invalid Chat Running Config: {error}")))
                            .await;
                        return;
                    }
                }
            };

            if let (Some(running), Some(pm)) = (running_config.as_mut(), provider_manager.as_ref())
            {
                let global_max = running.max_input_length.max(1) as u64;
                let effective = match active_chat_model_identity.as_ref() {
                    Some(model) => pm
                        .get_known_model_context_window(&model.provider, &model.model)
                        .await
                        .map(|window| window.min(global_max))
                        .unwrap_or(global_max),
                    None => global_max,
                };
                running.max_input_length = effective.min(u32::MAX as u64) as u32;
            }
            let running_config = running_config;
            let runtime_context_manager = runtime.context_manager().await;
            let ws_context = rua_agent::context::load_workspace_context(&agent_workspace_dir).await;
            let execution_environment_prompt =
                Some(rua_agent::prompt::build_execution_environment_prompt(
                    &RuntimeLayout::from_current_process(Some(&host_profile)),
                    &working_dir,
                    &host_profile,
                    (!device_id_str.is_empty()).then_some(device_id_str.as_str()),
                    device_name_snapshot.as_deref(),
                ));
            let project_id = address.project_id().cloned();
            let project_resources = match project_id.as_ref() {
                Some(project_id) => {
                    let binding = project_bindings
                        .as_ref()
                        .ok_or_else(|| "Project Resource Binding Store is unavailable".to_string())
                        .and_then(|bindings| {
                            bindings.get(project_id).map_err(|error| error.to_string())
                        })
                        .and_then(|binding| {
                            binding.ok_or_else(|| "Project Resource Binding is missing".to_string())
                        });
                    match binding {
                        Ok(binding) => Some(binding.resources),
                        Err(error) => {
                            let _ = tx.send(Err(error)).await;
                            return;
                        }
                    }
                }
                None => None,
            };
            let mut project_instructions = match project_id.as_ref() {
                Some(project_id) => {
                    crate::services::project_instructions::load_project_instructions(
                        &working_dir,
                        project_id,
                    )
                    .await
                    .map(|instructions| instructions.model_instruction())
                }
                None => None,
            };
            if let Some(resources) = project_resources.as_ref() {
                let paths = resources
                    .directories
                    .iter()
                    .map(|directory| &directory.resource_root)
                    .collect::<Vec<_>>();
                let inventory = format!("Project resource directories (JSON paths): {}. Default working directory: {}. Use an explicit path for another directory; attachment does not activate its configuration or Hooks.", serde_json::json!(paths), serde_json::json!(working_dir));
                let instructions = project_instructions.get_or_insert_with(String::new);
                instructions.push_str("\n\n");
                instructions.push_str(&inventory);
            }
            let agent_prompt_files = match captured_configuration.as_ref() {
                Some(configuration) => configuration.prompt_file_content.clone(),
                None => {
                    prompt_file_fragment_for_entries(&agent_workspace_dir, &active_prompt_files)
                }
            };
            let workspace_context =
                (!ws_context.is_empty()).then(|| ws_context.to_injection_text());
            let acp_passthrough_runner = if is_acp_passthrough_input(&user_message) {
                let active_runner = if let Some(handle) = acp_chat_registry
                    .as_ref()
                    .and_then(|registry| registry.get(&agent_id))
                {
                    handle
                        .manager
                        .get(&chat_id)
                        .await
                        .map(|session| session.runner)
                } else {
                    None
                };
                match active_runner {
                    Some(runner) => Some(runner),
                    None => {
                        let store = rua_session::WorkspaceSessionStore::new(&scope_root);
                        persisted_acp_origin_runner(&store, &chat_id).await
                    }
                }
            } else {
                None
            };
            {
                let store = rua_session::WorkspaceSessionStore::new(&scope_root);
                if acp_passthrough_runner.is_none() && !task_watch_notification_turn {
                    let memory_write_available = runtime.memory_manager().await.is_some()
                        && !disabled_tools.iter().any(|tool| tool == "memory_write");
                    match detect_memory_signal_for_user_input(
                        &store,
                        &chat_id,
                        &user_message,
                        memory_write_available,
                    )
                    .await
                    {
                        Ok(signal) => memory_signal = signal,
                        Err(e) => {
                            warn!(error = %e, chat_id, "failed to detect memory signal");
                        }
                    }
                }
                if let (Some(turn_id), Some(selected_variant_id)) = (
                    req.extra.get("turn_id").and_then(|v| v.as_str()),
                    req.extra
                        .get("selected_variant_id")
                        .and_then(|v| v.as_str()),
                ) {
                    if let Err(e) = store
                        .set_turn_selected_variant(
                            &chat_id,
                            turn_id,
                            selected_variant_id.to_string(),
                        )
                        .await
                    {
                        warn!(error = %e, chat_id, turn_id, "failed to select Evergreen assistant variant");
                    }
                }
                if acp_passthrough_runner.is_none() {
                    let evergreen_history = load_evergreen_projection_messages(
                        &store,
                        &address,
                        running_config.as_ref(),
                    )
                    .await;
                    if let Some(history) = evergreen_history {
                        conversation_recall_available = history.conversation_recall_available;
                        history_messages = history.messages;
                        if submitted_user_turn_persisted
                            && history_messages
                                .last()
                                .is_some_and(|message| message.role == "user")
                        {
                            history_messages.pop();
                        }
                    }
                }
                if let Some(turn_id) = requested_regenerate_turn_id.as_deref() {
                    match load_evergreen_replay_messages_before_turn(&store, &chat_id, turn_id)
                        .await
                    {
                        Ok(Some(replay)) => {
                            debug!(
                                "Explicit regenerate requested for session {}: appending assistant variant to {}",
                                request_chat_id, turn_id
                            );
                            history_messages = replay;
                            regenerate_turn_id = Some(turn_id.to_string());
                        }
                        Ok(None) => {
                            warn!(
                                "Explicit regenerate ignored for session {}: unknown turn_id {}",
                                request_chat_id, turn_id
                            );
                        }
                        Err(e) => {
                            warn!(error = %e, chat_id, turn_id, "failed to load Evergreen regenerate replay");
                        }
                    }
                }
                // The persistence boundary indexes this exact projected array. Model-only
                // summaries and History Replacements must therefore count toward the boundary,
                // even though they are not themselves durable conversation messages.
                loaded_message_count = history_messages.len();
            }
            if !conversation_recall_available {
                disable_conversation_recall_tools(&mut disabled_tools);
            }
            let mut all_messages = history_messages;

            let submitted_user_message = if task_watch_notification_turn || hook_continuation {
                None
            } else {
                let mut user_content = if ordered_user_content.is_empty() {
                    vec![ContentPart::Text {
                        text: user_message.clone(),
                    }]
                } else {
                    ordered_user_content
                };
                let submitted_user_message = AgentInputMessage {
                    role: "user".into(),
                    content: user_content.clone(),
                };

                inject_file_path_hints(&mut user_content);
                all_messages.push(AgentInputMessage {
                    role: "user".into(),
                    content: user_content,
                });
                Some(submitted_user_message)
            };

            let persisted_sys_prompt = {
                let parts = [
                    agent_prompt_files.clone(),
                    system_prompt.clone(),
                    workspace_context.clone(),
                ]
                .into_iter()
                .flatten()
                .filter(|part| !part.trim().is_empty())
                .collect::<Vec<_>>();
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join("\n\n"))
                }
            };
            let accounting_sys_prompt = [
                persisted_sys_prompt.clone(),
                project_instructions.clone(),
                Some(rua_agent::collaboration_mode::section(effective_collaboration_mode).content),
                execution_environment_prompt.clone(),
            ]
            .into_iter()
            .flatten()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>();
            let accounting_sys_prompt =
                (!accounting_sys_prompt.is_empty()).then(|| accounting_sys_prompt.join("\n\n"));

            let device_activity = ChatDeviceActivityUpdate {
                device_id: (!device_id_str.is_empty()).then_some(device_id_str.clone()),
                host_profile: host_profile.clone(),
            };
            let authority_placement =
                match crate::services::conversation_authority_target::load_chat_authority_placement_target(
                    &scope_root,
                    &chat_id,
                )
                .await
                {
                    Ok(placement) => placement,
                    Err(error) => {
                    let _ = tx
                        .send(Err(error))
                        .await;
                    return;
                }
                };
            let active_prompt_file_snapshots =
                build_prompt_file_snapshots(&agent_workspace_dir, &active_prompt_files);
            let session_context_snapshot = SessionContextSnapshot {
                sys_prompt: persisted_sys_prompt.clone(),
                active_prompt_files: active_prompt_file_snapshots,
                pending_action: None,
            };
            let mut submitted_user_turn: Option<AppendedSubmittedUserTurn> = None;
            if requested_regenerate_turn_id.is_none() {
                if let Some(submitted_user_message) = submitted_user_message.as_ref() {
                    let persisted = if submitted_user_turn_persisted {
                        match authority_turn_id.as_deref() {
                            Some(turn_id) => {
                                load_persisted_submitted_user_turn(&scope_root, &chat_id, turn_id)
                                    .await
                            }
                            None => Err(rua_core::RuaError::Runtime {
                                message: "pre-persisted Project first Turn is missing its turn_id"
                                    .to_string(),
                            }),
                        }
                    } else {
                        append_evergreen_submitted_user_turn(
                            &scope_root,
                            &agent_id,
                            &address,
                            submitted_user_message,
                            &device_id_str,
                            SubmittedUserTurnMetadata {
                                turn_id: authority_turn_id.as_deref(),
                                input_id: authority_input_id.as_deref(),
                                acceptance_mode: acceptance_mode.as_deref(),
                                submission_id: submission_id.as_deref(),
                                device_delivery: device_delivery.as_ref(),
                                acp_origin_runner: acp_passthrough_runner.as_deref(),
                                memory_signal: memory_signal.as_ref(),
                                turn_agent_assignment: req
                                    .extra
                                    .get(rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY),
                                turn_model: req.extra.get(rua_chat::TURN_MODEL_KEY),
                                turn_thinking: req.extra.get(rua_chat::TURN_THINKING_KEY),
                                collaboration_mode: effective_collaboration_mode,
                                authority_device_id: &authority_placement.device_id,
                                authority_epoch: authority_placement.authority_epoch,
                            },
                        )
                        .await
                    };
                    match persisted {
                        Ok(turn) => submitted_user_turn = turn,
                        Err(e) => {
                            warn!(error = %e, chat_id, "failed to eagerly persist submitted user message");
                            let _ = tx.send(Err(e.to_string())).await;
                            return;
                        }
                    }
                    if submitted_user_turn.is_some() {
                        if let Some(emitter) = session_sync.as_ref() {
                            emitter.spawn_chat_state_publication_for(&scope_root, &address, None);
                        }
                    }
                }
            }
            let submitted_turn_id = submitted_user_turn
                .as_ref()
                .map(|turn| turn.turn_id.clone());
            let active_turn_id = authority_turn_id
                .as_ref()
                .or(regenerate_turn_id.as_ref())
                .or(submitted_turn_id.as_ref())
                .cloned();
            if let Some(ref turn_id) = active_turn_id {
                let _ = streams
                    .set_active_turn_id(CONSOLE_CHANNEL_ID, &address, Priority::Normal, turn_id)
                    .await;
            }
            let persistence: Arc<dyn SessionPersistence> = {
                let mut p = WorkspaceSessionPersistence::new(scope_root.clone())
                    .with_address(address.clone())
                    .with_loaded_message_count(loaded_message_count)
                    .with_device_id(device_id_str.clone())
                    .with_authority(
                        authority_placement.device_id.clone(),
                        authority_placement.authority_epoch,
                    )
                    .with_device_activity(device_activity)
                    .with_session_context_snapshot(session_context_snapshot)
                    .with_model_call_identity(Some(
                        rua_agent::model_call_context::ModelCallIdentityContext::from_chat_address(
                            &address,
                        ),
                    ));
                if let Some(authority) = conversation_authority.clone() {
                    p = p.with_conversation_authority(authority);
                }
                if let Some(emitter) = session_sync.clone() {
                    p = p.with_session_sync_emitter(emitter);
                }
                if let Some(prompt) = accounting_sys_prompt {
                    p = p.with_system_prompt(prompt);
                }
                if let Some(turn_id) = regenerate_turn_id.clone() {
                    p = p.with_regenerate_turn_id(turn_id);
                }
                if let Some(turn_id) = active_turn_id.clone() {
                    p = p.with_active_turn_id(turn_id);
                }
                if let Some(manager) = runtime_context_manager.clone() {
                    p = p.with_context_manager(manager);
                }
                if let Some(running) = running_config.clone() {
                    p = p.with_running_config(Arc::new(running));
                }
                if let Some(model) = active_chat_model_identity.clone() {
                    p = p.with_checkpoint_model(model);
                }
                if let Some(runner) = acp_passthrough_runner.clone() {
                    p = p.with_acp_origin_runner(runner);
                }
                Arc::new(p)
            };

            let workspace_runtime = match registry.get(&agent_id).await {
                Some(runtime) => Ok(runtime),
                None => registry.get_or_create(&agent_id).await,
            };
            let agent_runtime: Arc<dyn AgentRuntime> = match workspace_runtime {
                Ok(ws_rt) => match ws_rt.react_runtime().await {
                    Some(rt) => rt,
                    None => {
                        let _ = tx
                            .send(Err("react_runtime not available for agent".to_string()))
                            .await;
                        return;
                    }
                },
                Err(e) => {
                    let _ = tx.send(Err(format!("workspace unavailable: {e}"))).await;
                    return;
                }
            };
            let channel_service = ConsoleChannelService::new(agent_runtime);

            let locale = rua_config::SettingsStore::new(agent_workspace_dir.clone())
                .get("language")
                .ok()
                .flatten()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .map(|raw| rua_i18n::normalize_lang(&raw))
                .unwrap_or_else(|| "zh".into());

            let mut metadata = std::collections::HashMap::new();
            metadata.insert("channel".into(), serde_json::json!("console"));
            metadata.insert("locale".into(), serde_json::json!(locale));
            metadata.insert(
                rua_agent::collaboration_mode::AGENT_COLLABORATION_MODE_METADATA_KEY.into(),
                serde_json::json!(effective_collaboration_mode),
            );
            if let Some(running) = running_config.as_ref() {
                metadata.insert(
                    rua_agent::runtime::EFFECTIVE_RUNNING_CONFIG_METADATA_KEY.into(),
                    serde_json::to_value(running).unwrap_or_default(),
                );
            }
            metadata.insert(
                "host_profile".into(),
                serde_json::json!(host_profile.clone()),
            );
            for key in [rua_agent::runtime::HOOK_CONTINUATION_CONTEXT_METADATA_KEY] {
                if let Some(value) = req.extra.get(key) {
                    metadata.insert(key.into(), value.clone());
                }
            }
            if let Some(ref prompt) = agent_prompt_files {
                metadata.insert(
                    rua_agent::prompt::AGENT_PROMPT_FILES_METADATA_KEY.into(),
                    serde_json::Value::String(prompt.clone()),
                );
            }
            if let Some(ref prompt) = system_prompt {
                metadata.insert(
                    rua_agent::prompt::AGENT_CUSTOM_PROMPT_METADATA_KEY.into(),
                    serde_json::Value::String(prompt.clone()),
                );
            }
            if let Some(ref context) = workspace_context {
                metadata.insert(
                    rua_agent::prompt::WORKSPACE_CONTEXT_METADATA_KEY.into(),
                    serde_json::Value::String(context.clone()),
                );
            }
            if !device_id_str.is_empty() {
                metadata.insert("device_id".into(), serde_json::json!(device_id_str.clone()));
                if let Some(ref name) = device_name_snapshot {
                    metadata.insert("device_friendly_name".into(), serde_json::json!(name));
                }
            }
            if let Some(ref data_dir) = device_context_data_dir {
                metadata.insert(
                    "known_devices".into(),
                    serde_json::Value::Array(
                        crate::services::device_context::known_devices_for_runtime(
                            data_dir,
                            (!device_id_str.is_empty()).then_some(device_id_str.as_str()),
                        ),
                    ),
                );
            }
            if let Some(ref instructions) = project_instructions {
                metadata.insert(
                    rua_agent::hooks::PROJECT_INSTRUCTIONS_METADATA_KEY.into(),
                    serde_json::Value::String(instructions.clone()),
                );
            }
            if let Some(ref execution_environment) = execution_environment_prompt {
                metadata.insert(
                    rua_agent::runtime::EXECUTION_ENVIRONMENT_METADATA_KEY.into(),
                    serde_json::Value::String(execution_environment.clone()),
                );
            }
            if let Some(resources) = project_resources.as_ref() {
                metadata.insert(
                    "project_resource_roots".into(),
                    serde_json::json!(resources
                        .directories
                        .iter()
                        .map(|directory| &directory.resource_root)
                        .collect::<Vec<_>>()),
                );
                metadata.insert(
                    "project_resource_revision".into(),
                    serde_json::json!(resources.revision),
                );
            }
            if project_id.is_some() {
                metadata.insert(
                    rua_agent::hooks::PROJECT_SKILLS_ROOT_METADATA_KEY.into(),
                    serde_json::Value::String(working_dir.to_string_lossy().into_owned()),
                );
            }
            metadata.insert(
                "chat_execution".into(),
                chat_execution_runtime_metadata(Some(&scope_root), &chat_id, &req.extra).await,
            );
            metadata.insert(
                "rua_workspace_dir".into(),
                serde_json::json!(scope_root.to_string_lossy()),
            );
            metadata.insert(
                rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.into(),
                serde_json::to_value(&address).expect("Chat Address is serializable"),
            );
            if let Some(ref slot) = chat_model_slot {
                metadata.insert("chat_model_slot".into(), slot.clone());
            }
            if let Some(ref turn_id) = active_turn_id {
                metadata.insert("turn_id".into(), serde_json::json!(turn_id));
            }
            if let Some(ref turn_id) = regenerate_turn_id {
                metadata.insert(
                    rua_agent::turn_persistence::REGENERATE_TURN_ID_METADATA_KEY.into(),
                    serde_json::json!(turn_id),
                );
            }
            if let Some(ref runner) = acp_passthrough_runner {
                metadata.insert(
                    rua_agent::turn_persistence::ACP_ORIGIN_RUNNER_METADATA_KEY.into(),
                    serde_json::json!(runner),
                );
            }
            if let Some(assignment) = req.extra.get(rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY) {
                metadata.insert(
                    rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.into(),
                    assignment.clone(),
                );
            }
            if regenerate_turn_id.is_none() && active_turn_id.is_some() {
                metadata.insert(
                    rua_agent::turn_persistence::LOADED_MESSAGE_COUNT_METADATA_KEY.into(),
                    serde_json::json!(loaded_message_count),
                );
                metadata.insert(
                    rua_agent::turn_persistence::AUTHORITY_DEVICE_ID_METADATA_KEY.into(),
                    serde_json::json!(authority_placement.device_id.clone()),
                );
                metadata.insert(
                    rua_agent::turn_persistence::AUTHORITY_EPOCH_METADATA_KEY.into(),
                    serde_json::json!(authority_placement.authority_epoch),
                );
            }
            if let Some(allowed) = captured_configuration
                .as_ref()
                .and_then(|configuration| configuration.capability_configuration.as_ref())
            {
                metadata.insert(
                    rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.to_string(),
                    serde_json::to_value(allowed).expect("capability configuration serializes"),
                );
                metadata.insert(
                    rua_agent::runtime::CHAT_TOOL_ALLOWLIST_METADATA_KEY.to_string(),
                    serde_json::json!(allowed.tools),
                );
            }
            if let Some(configuration) = captured_configuration.as_ref() {
                metadata.insert(
                    rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY.to_string(),
                    serde_json::to_value(&configuration.security_config)
                        .expect("security configuration serializes"),
                );
                metadata.insert(
                    "chat_configuration_epoch".to_string(),
                    serde_json::json!(configuration.epoch),
                );
            }
            let runtime_tool_configs = load_tool_configs_for_runtime(
                tool_config_cells,
                tool_config_device_key,
                tool_catalog,
                &agent_id,
            )
            .await
            .unwrap_or_else(|| serde_json::json!({}));
            let tool_configs = match captured_configuration.as_ref() {
                Some(configuration) => match configuration
                    .tool_config_snapshot
                    .resolve(&runtime_tool_configs)
                {
                    Ok(configs) => configs,
                    Err(error) => {
                        let _ = tx.send(Err(error)).await;
                        return;
                    }
                },
                None => runtime_tool_configs,
            };
            if tool_configs.is_object() {
                metadata.insert("tool_configs".into(), tool_configs);
            }
            let max_input_length = running_config
                .as_ref()
                .map(|config| config.max_input_length.max(1) as u64);

            let _history_replacement_foreground =
                active_chat_model_identity.as_ref().and_then(|model| {
                    conversation_authority
                        .as_ref()
                        .map(|authority| authority.begin_history_replacement_foreground(model))
                });
            if let Some(rua_root) = device_context_data_dir.as_deref() {
                crate::services::chat_media::ChatMediaService.materialize_message_parts(
                    rua_root,
                    &address,
                    &mut all_messages,
                );
            } else {
                crate::services::chat_media::ChatMediaService
                    .materialize_message_parts_at(&scope_root, &mut all_messages);
            }
            let mut process_stream = channel_service.process(ProcessRequest {
                agent_id: agent_id.clone(),
                chat_id: chat_id.clone(),
                messages: all_messages,
                model_override,
                workspace_dir: Some(agent_workspace_dir.clone()),
                conversation_store_dir: Some(scope_root.clone()),
                working_dir: Some(working_dir.clone()),
                cancel_rx,
                user_message: user_message.clone(),
                persistence,
                disabled_tools,
                metadata,
            });

            if let Some(turn) = submitted_user_turn {
                if tx
                    .send(Ok(dto::ChatEvent::UserMessage {
                        message: turn.user_message,
                    }))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            let mut turn_token_usage: Option<serde_json::Value> = None;
            let mut pending_call_usage: Option<serde_json::Value> = None;
            let mut last_call_usage: Option<serde_json::Value> = None;
            'outer: while let Some(result) = process_stream.next().await {
                match result {
                    Ok(runtime_event) => {
                        if let RuntimeEvent::Usage {
                            input_tokens,
                            output_tokens,
                            total_tokens,
                            cache_read_tokens,
                            cache_write_tokens,
                        } = &runtime_event
                        {
                            let usage = serde_json::json!({
                                "prompt_tokens": input_tokens.unwrap_or(0),
                                "completion_tokens": output_tokens.unwrap_or(0),
                                "total_tokens": total_tokens.unwrap_or_else(|| {
                                    input_tokens.unwrap_or(0) + output_tokens.unwrap_or(0)
                                }),
                                "cache_read_tokens": cache_read_tokens,
                                "cache_write_tokens": cache_write_tokens,
                            });
                            turn_token_usage = Some(usage.clone());
                            pending_call_usage = Some(usage);
                        }
                        if let RuntimeEvent::IterationEnd { .. } = &runtime_event {
                            if let Some(usage) = pending_call_usage.take() {
                                last_call_usage = Some(usage);
                            }
                        }
                        let is_terminal_end = matches!(&runtime_event, RuntimeEvent::End { .. });
                        if let RuntimeEvent::Error { error } = &runtime_event {
                            if let Some(turn_id) = active_turn_id.as_ref() {
                                let error_detail = error.to_string();
                                match persist_conversation_runtime_failure_to_session(
                                    &scope_root,
                                    &agent_id,
                                    &chat_id,
                                    turn_id,
                                    &error_detail,
                                    None,
                                )
                                .await
                                {
                                    Err(persist_err) => warn!(
                                        error = %persist_err,
                                        chat_id,
                                        "failed to persist conversation runtime error event"
                                    ),
                                    Ok(_) => {
                                        if let Some(emitter) = session_sync.as_ref() {
                                            emitter
                                                .emit_evergreen_chat_snapshot(&scope_root, &address)
                                                .await;
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(pending_action) =
                            pending_action_from_runtime_event(&runtime_event)
                        {
                            if let Err(e) = persist_pending_action_snapshot(
                                &scope_root,
                                &agent_id,
                                &request_chat_id,
                                pending_action,
                                None,
                            )
                            .await
                            {
                                warn!(error = %e, "failed to persist pending action snapshot");
                            } else if let Some(emitter) = session_sync.as_ref() {
                                emitter
                                    .emit_evergreen_chat_snapshot(&scope_root, &address)
                                    .await;
                            }
                        }
                        for chat_event in
                            map_runtime_event_to_chat_events(runtime_event, &chat_id, &agent_id)
                        {
                            if tx.send(Ok(chat_event)).await.is_err() {
                                break 'outer;
                            }
                        }
                        if is_terminal_end && last_call_usage.is_none() {
                            last_call_usage = pending_call_usage.clone();
                        }
                        if is_terminal_end
                            && (turn_token_usage.is_some() || last_call_usage.is_some())
                            && tx
                                .send(Ok(dto::ChatEvent::Usage {
                                    usage: turn_token_usage.clone(),
                                    last_call_usage: last_call_usage.clone(),
                                    max_input_length,
                                }))
                                .await
                                .is_err()
                        {
                            break 'outer;
                        }
                    }
                    Err(e) => {
                        if let Some(turn_id) = active_turn_id.as_ref() {
                            match persist_conversation_runtime_failure_to_session(
                                &scope_root,
                                &agent_id,
                                &chat_id,
                                turn_id,
                                &e,
                                None,
                            )
                            .await
                            {
                                Err(persist_err) => warn!(
                                    error = %persist_err,
                                    chat_id,
                                    "failed to persist conversation runtime failure note"
                                ),
                                Ok(_) => {
                                    if let Some(emitter) = session_sync.as_ref() {
                                        emitter
                                            .emit_evergreen_chat_snapshot(&scope_root, &address)
                                            .await;
                                    }
                                }
                            }
                        }
                        if tx
                            .send(Ok(dto::ChatEvent::Error { detail: e }))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });

        tokio::spawn(async move {
            if let Err(join_error) = chat_stream_task.await {
                let error_detail = if join_error.is_panic() {
                    let payload = join_error.into_panic();
                    format!(
                        "chat stream task panicked: {}",
                        crate::services::conversation_input_intake::panic_payload_message(
                            payload.as_ref(),
                        )
                    )
                } else {
                    format!("chat stream task failed: {join_error}")
                };
                error!(
                    agent_id = %panic_agent_id,
                    request_chat_id = %panic_request_chat_id,
                    error = %error_detail,
                    "chat stream task panicked"
                );
                let _ = panic_tx.send(Err(error_detail)).await;
            }
        });

        Box::pin(
            futures::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|item| (item, rx))
            })
            .fuse(),
        )
    }
}

struct ResolvedConsoleAgentContext {
    system_prompt: Option<String>,
    active_prompt_files: Vec<String>,
    disabled_tools: Vec<String>,
}

pub fn extract_disabled_tools(tools: &Option<serde_json::Value>) -> Vec<String> {
    let tools_value = match tools {
        Some(v) => v,
        None => {
            debug!("extract_disabled_tools: tools is None");
            return Vec::new();
        }
    };
    let builtin_tools = match tools_value.get("builtin_tools").and_then(|v| v.as_object()) {
        Some(m) => m,
        None => {
            debug!("extract_disabled_tools: no builtin_tools key");
            return Vec::new();
        }
    };
    let disabled: Vec<String> = builtin_tools
        .iter()
        .filter_map(|(name, value)| {
            let enabled = value
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if !enabled {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect();
    debug!(?disabled, "extract_disabled_tools: result");
    disabled
}

pub fn extract_disabled_mcp_tools(mcp: &Option<serde_json::Value>) -> Vec<String> {
    let Some(clients) = mcp
        .as_ref()
        .and_then(|value| value.get("clients"))
        .and_then(|value| value.as_object())
    else {
        return Vec::new();
    };

    let mut disabled = Vec::new();
    for (server_id, client) in clients {
        let Some(tool_names) = client
            .get("disabled_tools")
            .and_then(|value| value.as_array())
        else {
            continue;
        };
        disabled.extend(tool_names.iter().filter_map(|value| {
            value
                .as_str()
                .map(|tool_name| format!("mcp__{server_id}__{tool_name}"))
        }));
    }
    disabled
}

pub fn turn_agent_assignment_from_agent(
    agent: &crate::endpoints::dto::AgentDetail,
) -> Result<rua_chat::TurnAgentAssignment, String> {
    if !agent.enabled {
        return Err(format!("Agent '{}' is disabled", agent.id));
    }
    let mut disabled_tools = extract_disabled_tools(&agent.tools);
    disabled_tools.extend(extract_disabled_mcp_tools(&agent.mcp));
    disabled_tools.sort();
    disabled_tools.dedup();
    Ok(rua_chat::TurnAgentAssignment {
        agent_id: agent.id.clone(),
        agent_name: agent.name.clone(),
        assigned_at: chrono::Utc::now().to_rfc3339(),
        system_prompt: agent
            .extra
            .get("systemPrompt")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        system_prompt_files: agent.system_prompt_files.clone(),
        disabled_tools,
    })
}

pub async fn collaboration_agent_inventory(
    manager: &AgentManager,
) -> Result<serde_json::Value, String> {
    let mut entries = Vec::new();
    for summary in manager
        .list_agents()
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|agent| agent.enabled)
        .take(32)
    {
        let Some(agent) = manager
            .get_agent(&summary.id)
            .await
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        if !agent.enabled {
            continue;
        }
        let tools = agent
            .tools
            .as_ref()
            .and_then(|value| value.get("builtin_tools"))
            .and_then(serde_json::Value::as_object);
        let mut enabled = tools
            .into_iter()
            .flat_map(|tools| tools.iter())
            .filter(|(_, value)| {
                value
                    .get("enabled")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true)
            })
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        enabled.sort();
        entries.push(serde_json::json!({"agent_id":agent.id,"name":agent.name,"description":agent.description.chars().take(240).collect::<String>(),"configured_tools":enabled}));
    }
    Ok(serde_json::Value::Array(entries))
}

async fn resolve_agent_context(
    agent_service: &Option<Arc<AgentManager>>,
    agent_id: &str,
) -> ResolvedConsoleAgentContext {
    let default = ResolvedConsoleAgentContext {
        system_prompt: None,
        active_prompt_files: Vec::new(),
        disabled_tools: Vec::new(),
    };
    let svc = match agent_service.as_ref() {
        Some(s) => s,
        None => return default,
    };
    let agent = match svc.get_agent(agent_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            warn!(agent_id, "resolve_agent_context: agent not found");
            return default;
        }
        Err(e) => {
            warn!(agent_id, error = %e, "resolve_agent_context: failed to load agent");
            return default;
        }
    };
    resolve_agent_context_from_detail(&agent)
}

async fn resolve_turn_agent_assignment(
    agent_service: &Option<Arc<AgentManager>>,
    agent_id: &str,
    allow_runtime_fallback: bool,
) -> std::result::Result<rua_chat::TurnAgentAssignment, String> {
    let Some(service) = agent_service.as_ref() else {
        return allow_runtime_fallback
            .then(|| runtime_turn_agent_assignment(agent_id))
            .ok_or_else(|| "Agent configuration service is unavailable".to_string());
    };
    let Some(agent) = service
        .get_agent(agent_id)
        .await
        .map_err(|error| error.to_string())?
    else {
        return allow_runtime_fallback
            .then(|| runtime_turn_agent_assignment(agent_id))
            .ok_or_else(|| format!("Agent not found: {agent_id}"));
    };
    turn_agent_assignment_from_agent(&agent)
}

fn runtime_turn_agent_assignment(agent_id: &str) -> rua_chat::TurnAgentAssignment {
    rua_chat::TurnAgentAssignment {
        agent_id: agent_id.to_string(),
        agent_name: agent_id.to_string(),
        assigned_at: chrono::Utc::now().to_rfc3339(),
        system_prompt: None,
        system_prompt_files: Vec::new(),
        disabled_tools: Vec::new(),
    }
}

fn resolve_agent_context_from_detail(
    agent: &crate::endpoints::dto::AgentDetail,
) -> ResolvedConsoleAgentContext {
    let mut disabled_tools = extract_disabled_tools(&agent.tools);
    disabled_tools.extend(extract_disabled_mcp_tools(&agent.mcp));
    debug!(
        agent_id = %agent.id,
        ?disabled_tools,
        "resolve_agent_context: resolved disabled tools"
    );
    ResolvedConsoleAgentContext {
        system_prompt: agent
            .extra
            .get("systemPrompt")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        active_prompt_files: agent.system_prompt_files.clone(),
        disabled_tools,
    }
}

fn turn_agent_context_from_extra(
    extra: &serde_json::Map<String, serde_json::Value>,
    expected_agent_id: &str,
) -> Option<ResolvedConsoleAgentContext> {
    let assignment: rua_chat::TurnAgentAssignment =
        serde_json::from_value(extra.get(rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY)?.clone())
            .ok()?;
    if assignment.agent_id != expected_agent_id {
        return None;
    }
    Some(ResolvedConsoleAgentContext {
        system_prompt: assignment.system_prompt,
        active_prompt_files: assignment.system_prompt_files,
        disabled_tools: assignment.disabled_tools,
    })
}

use rua_channel::core::console::{
    ConsoleChatFacade, ConsoleChatRequest, ConsoleStopOutcome, ConsoleSubmitOutcome,
    SseOutputStream, WebUiSseAdapter,
};

pub fn chat_event_to_runtime_event(event: dto::ChatEvent) -> RuntimeEvent {
    match event {
        dto::ChatEvent::Start {
            chat_id, agent_id, ..
        } => RuntimeEvent::Start {
            session_id: SessionId::new(chat_id),
            agent_id: agent_id.map(rua_core::id::AgentId::new),
        },
        dto::ChatEvent::TextDelta { message_id, delta } => RuntimeEvent::TextDelta {
            delta,
            message_id: (!message_id.is_empty()).then(|| message_id.into()),
        },
        dto::ChatEvent::ThinkingDelta { message_id, delta } => RuntimeEvent::ThinkingDelta {
            delta,
            signature: None,
            message_id: (!message_id.is_empty()).then(|| message_id.into()),
        },
        dto::ChatEvent::ModelCallReset { model_call_id } => {
            RuntimeEvent::ModelCallReset { model_call_id }
        }
        dto::ChatEvent::ToolCallDelta {
            message_id,
            id,
            index,
            name,
            arguments_delta,
        } => {
            let expected_message_id =
                agent_types::conversation_message_identity::tool_use_message_id(&id);
            RuntimeEvent::ToolCallDelta {
                id: if message_id == expected_message_id {
                    id
                } else {
                    String::new()
                },
                index,
                name,
                arguments_delta,
            }
        }
        dto::ChatEvent::ToolCall {
            message_id,
            id,
            name,
            arguments,
        } => {
            let mut call = rua_core::tool::ToolCall::new(
                id,
                name,
                serde_json::from_str(&arguments).unwrap_or(serde_json::Value::Null),
            );
            call.metadata.insert(
                agent_types::conversation_message_identity::CANONICAL_MESSAGE_ID_METADATA_KEY
                    .to_string(),
                serde_json::Value::String(message_id),
            );
            RuntimeEvent::ToolCall { call }
        }
        dto::ChatEvent::ToolResult {
            message_id,
            id,
            name,
            content,
        } => {
            let text = extract_text_from_content_json(&content);
            let mut result = rua_core::tool::ToolResult::text(id, text);
            result.name = name;
            result.metadata.insert(
                agent_types::conversation_message_identity::CANONICAL_MESSAGE_ID_METADATA_KEY
                    .to_string(),
                serde_json::Value::String(message_id),
            );
            RuntimeEvent::ToolResult { result }
        }
        dto::ChatEvent::ToolResultDelta {
            message_id,
            id,
            name,
            output_delta,
        } => {
            let expected_message_id =
                agent_types::conversation_message_identity::tool_result_message_id(&id);
            RuntimeEvent::ToolResultDelta {
                id: if message_id == expected_message_id {
                    id
                } else {
                    String::new()
                },
                name,
                output_delta,
            }
        }
        dto::ChatEvent::Usage { usage, .. } => RuntimeEvent::Usage {
            input_tokens: usage
                .as_ref()
                .and_then(|value| value.get("prompt_tokens"))
                .and_then(|value| value.as_u64()),
            output_tokens: usage
                .as_ref()
                .and_then(|value| value.get("completion_tokens"))
                .and_then(|value| value.as_u64()),
            total_tokens: usage
                .as_ref()
                .and_then(|value| value.get("total_tokens"))
                .and_then(|value| value.as_u64()),
            cache_read_tokens: usage
                .as_ref()
                .and_then(|value| value.get("cache_read_tokens"))
                .and_then(|value| value.as_u64()),
            cache_write_tokens: usage
                .as_ref()
                .and_then(|value| value.get("cache_write_tokens"))
                .and_then(|value| value.as_u64()),
        },
        dto::ChatEvent::Error { detail } => RuntimeEvent::Error {
            error: rua_core::RuaError::Runtime { message: detail },
        },
        dto::ChatEvent::Interrupted { reason } => RuntimeEvent::End {
            finish_reason: reason.or_else(|| Some("interrupted".to_string())),
        },
        dto::ChatEvent::Done { finish_reason } => RuntimeEvent::End { finish_reason },
        dto::ChatEvent::ToolApprovalRequest { content, .. } => {
            let data = content
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("data"));
            let id = data
                .and_then(|d| d.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = data
                .and_then(|d| d.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = data
                .and_then(|d| d.get("arguments"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let approval_request_id = data
                .and_then(|d| d.get("approval_request_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let call_id = data
                .and_then(|d| d.get("call_id"))
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            let prompt = data
                .and_then(|d| d.get("prompt"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let status = data.and_then(|d| d.get("status")).and_then(|v| v.as_str());
            if matches!(status, Some("waiting" | "pending")) || approval_request_id.is_some() {
                return RuntimeEvent::ToolApprovalPending {
                    call_id,
                    tool_name: name,
                    request_id: approval_request_id
                        .or_else(|| (!id.is_empty()).then(|| id.clone())),
                    prompt,
                };
            }
            RuntimeEvent::ToolApprovalRequest {
                id,
                name,
                arguments,
            }
        }
        dto::ChatEvent::AskUserRequest { id, content, .. } => {
            let data = content
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("data"));
            let request_id = data
                .and_then(|d| d.get("request_id"))
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            let questions = data
                .and_then(|d| d.get("questions"))
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default();
            RuntimeEvent::AskUser {
                request_id,
                questions,
            }
        }
        dto::ChatEvent::DeviceDeliveryAccepted { .. } => RuntimeEvent::End {
            finish_reason: Some("device_delivery_accepted".to_string()),
        },
        dto::ChatEvent::ContextCompaction {
            status,
            mode,
            message,
        } => RuntimeEvent::ContextCompaction {
            status,
            mode,
            message,
        },
        dto::ChatEvent::Message { id, .. } => RuntimeEvent::TextDelta {
            delta: String::new(),
            message_id: (!id.is_empty()).then(|| id.into()),
        },
        dto::ChatEvent::UserMessage { .. } => RuntimeEvent::TextDelta {
            delta: String::new(),
            message_id: None,
        },
    }
}

pub struct ConsoleChatFacadeImpl {
    registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
    sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    acp_chat_registry: Option<Arc<crate::services::acp_chat_registry::AcpChatRegistry>>,
    live_activity: ConversationLiveActivity,
    remote_input_delivery: RemoteConversationInputDelivery,
    input_intake: ConversationInputIntake,
    conversation_authority: Option<Arc<ConversationAuthority>>,
}

impl ConsoleChatFacadeImpl {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        input_intake: ConversationInputIntake,
        streams: Arc<ActiveStreamRegistry>,
        registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
        sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
        sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
        device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
        data_dir: PathBuf,
    ) -> Self {
        let live_activity = ConversationLiveActivity::new(
            Arc::clone(&streams),
            Arc::clone(&registry),
            Arc::clone(&sync_device),
            Arc::clone(&sync_node),
            Arc::clone(&device_network),
            data_dir.clone(),
        );
        let remote_input_delivery = RemoteConversationInputDelivery::new(
            Arc::clone(&sync_node),
            device_network,
            data_dir.clone(),
        );
        Self {
            registry,
            sync_device,
            acp_chat_registry: None,
            live_activity,
            remote_input_delivery,
            input_intake,
            conversation_authority: None,
        }
    }

    pub fn with_conversation_authority(mut self, authority: Arc<ConversationAuthority>) -> Self {
        self.live_activity = self
            .live_activity
            .with_conversation_authority(Arc::clone(&authority));
        self.input_intake = self
            .input_intake
            .with_conversation_authority(Arc::clone(&authority));
        self.conversation_authority = Some(authority);
        self
    }

    pub fn with_project_catalog(
        mut self,
        project_catalog: Arc<rua_project::ProjectCatalog>,
    ) -> Self {
        self.live_activity = self.live_activity.with_project_catalog(project_catalog);
        self
    }

    pub fn with_history_sync(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<
            crate::services::conversation_history_readiness::ConversationHistorySyncRequest,
        >,
    ) -> Self {
        self.live_activity = self.live_activity.with_history_sync(tx);
        self
    }

    pub fn with_chat_activity_wake_hub(
        mut self,
        wake_hub: Arc<crate::services::chat_activity_wake::ChatActivityWakeHub>,
    ) -> Self {
        self.live_activity = self.live_activity.with_chat_activity_wake_hub(wake_hub);
        self
    }

    pub fn with_acp_chat_registry(
        mut self,
        registry: Arc<crate::services::acp_chat_registry::AcpChatRegistry>,
    ) -> Self {
        self.input_intake = self
            .input_intake
            .with_acp_chat_registry(Arc::clone(&registry));
        self.acp_chat_registry = Some(registry);
        self
    }

    pub fn with_active_stream_broadcast_hub(
        mut self,
        hub: Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>,
    ) -> Self {
        self.live_activity = self
            .live_activity
            .with_active_stream_broadcast_hub(Arc::clone(&hub));
        self.input_intake = self
            .input_intake
            .with_active_stream_broadcast_hub(Arc::clone(&hub));
        self
    }

    async fn resolve_control_route(
        &self,
        agent_id: &str,
        chat_id: &str,
    ) -> ConversationControlRoute {
        let workspace_dir = match self.registry.get_or_create(agent_id).await {
            Ok(runtime) => Some(runtime.workspace_dir()),
            Err(_) => None,
        };
        let local_device_id = current_sync_device_id(&self.sync_device);
        let address = ChatAddress::agent(agent_id, chat_id);
        ConversationAuthorityRouter
            .resolve_control_route(ConversationControlRouteRequest {
                workspace_dir: workspace_dir.as_deref(),
                address: &address,
                local_device_id: local_device_id.as_deref(),
            })
            .await
    }
}

#[async_trait]
impl ConsoleChatFacade for ConsoleChatFacadeImpl {
    async fn connect_scoped_chat_activity(
        &self,
        address: ChatAddress,
        execution_agent_id: &str,
        viewer_id: &str,
        position: Option<rua_core::ConversationActivityPosition>,
    ) -> SseOutputStream {
        self.live_activity
            .subscribe_viewer(&address, execution_agent_id, viewer_id, position)
            .await
    }

    async fn connect_chat_activity(
        &self,
        agent_id: &str,
        chat_id: &str,
        viewer_id: &str,
        position: Option<rua_core::ConversationActivityPosition>,
    ) -> SseOutputStream {
        let resolved_chat_id = match resolve_chat_id(chat_id) {
            Ok(id) => id,
            Err(err) => {
                let body =
                    format!("event: error\ndata: {{\"error\":\"invalid chat_id: {err}\"}}\n\n",);
                return Box::pin(futures::stream::iter(vec![Ok(body)]));
            }
        };

        self.live_activity
            .subscribe_viewer(
                &ChatAddress::agent(agent_id, &resolved_chat_id),
                agent_id,
                viewer_id,
                position,
            )
            .await
    }

    async fn submit_chat(
        &self,
        agent_id: &str,
        request: ConsoleChatRequest,
    ) -> ConsoleSubmitOutcome {
        console_submit_outcome_from_delivery(
            self.input_intake.submit_delivery(agent_id, request).await,
        )
    }

    async fn submit_scoped_chat(
        &self,
        address: ChatAddress,
        execution_agent_id: &str,
        mut request: ConsoleChatRequest,
    ) -> ConsoleSubmitOutcome {
        request.chat_id = Some(address.chat_id().to_string());
        let outcome = match address.scope() {
            rua_core::ChatScope::Agent { agent_id, .. } => {
                self.input_intake
                    .submit_delivery(agent_id.as_str(), request)
                    .await
            }
            rua_core::ChatScope::Project { project_id, .. } => {
                self.input_intake
                    .submit_project_delivery(project_id, execution_agent_id, request)
                    .await
            }
        };
        console_submit_outcome_from_delivery(outcome)
    }

    async fn stop_chat(&self, agent_id: &str, chat_id: &str) -> bool {
        self.stop_chat_outcome(agent_id, chat_id).await.stopped
    }

    async fn stop_chat_outcome(&self, agent_id: &str, chat_id: &str) -> ConsoleStopOutcome {
        let resolved_chat_id = match resolve_chat_id(chat_id) {
            Ok(id) => id,
            Err(err) => {
                tracing::error!(
                    target: "diag.session",
                    agent_id = %agent_id,
                    chat_id = %chat_id,
                    error = %err,
                    "[DIAG-session] stop_chat rejected non-UUID chat_id",
                );
                return ConsoleStopOutcome::local(false);
            }
        };
        let address = ChatAddress::agent(agent_id, &resolved_chat_id);

        match self
            .resolve_control_route(agent_id, &resolved_chat_id)
            .await
        {
            ConversationControlRoute::RemoteAuthority {
                source_device_id,
                target_device_id,
                authority_epoch,
            } => {
                let command_id = format!("remote-control-{}", uuid::Uuid::new_v4());
                let device_delivery_id = self
                    .remote_input_delivery
                    .source_pending_device_delivery_id(&address)
                    .await
                    .ok()
                    .flatten();
                match self
                    .remote_input_delivery
                    .send_remote_control(RemoteConversationControlParams {
                        command_id: &command_id,
                        address: &address,
                        execution_agent_id: agent_id,
                        source_device_id: &source_device_id,
                        target_device_id: &target_device_id,
                        authority_epoch,
                        device_delivery_id: device_delivery_id.clone(),
                        action: RemoteChatControlAction::StopCurrentTurn,
                    })
                    .await
                {
                    Ok(response) => {
                        ConsoleStopOutcome::remote_control(response.applied, response.detail, None)
                    }
                    Err(DeviceCommandDeliveryError::AuthorityRedirect(redirect)) => {
                        let replacement = match self
                            .input_intake
                            .observe_authority_redirect(authority_epoch, &redirect)
                            .await
                        {
                            Ok(replacement) => replacement,
                            Err(detail) => {
                                return ConsoleStopOutcome::remote_control(
                                    false,
                                    Some(detail),
                                    Some("authority_redirect".to_string()),
                                );
                            }
                        };
                        match self
                            .remote_input_delivery
                            .send_remote_control(RemoteConversationControlParams {
                                command_id: &command_id,
                                address: &address,
                                execution_agent_id: agent_id,
                                source_device_id: &source_device_id,
                                target_device_id: &replacement.device_id,
                                authority_epoch: replacement.authority_epoch,
                                device_delivery_id,
                                action: RemoteChatControlAction::StopCurrentTurn,
                            })
                            .await
                        {
                            Ok(response) => ConsoleStopOutcome::remote_control(
                                response.applied,
                                response.detail,
                                None,
                            ),
                            Err(error) => ConsoleStopOutcome::remote_control(
                                false,
                                Some(error.to_string()),
                                Some(error.failure_kind().to_string()),
                            ),
                        }
                    }
                    Err(error) => ConsoleStopOutcome::remote_control(
                        false,
                        Some(error.to_string()),
                        Some(error.failure_kind().to_string()),
                    ),
                }
            }
            ConversationControlRoute::LocalAuthority { .. } => {
                if self
                    .cancel_acp_passthrough_turn(agent_id, &resolved_chat_id)
                    .await
                {
                    let _ = self
                        .input_intake
                        .request_local_stop(agent_id, &resolved_chat_id)
                        .await;
                    return ConsoleStopOutcome::local_acp_passthrough(true);
                }

                if !self
                    .input_intake
                    .request_local_stop(agent_id, &resolved_chat_id)
                    .await
                {
                    return ConsoleStopOutcome::local(false);
                }

                ConsoleStopOutcome::local(true)
            }
            ConversationControlRoute::Rejected { detail, .. } => {
                ConsoleStopOutcome::remote_control(
                    false,
                    Some(detail),
                    Some("business".to_string()),
                )
            }
        }
    }
}

impl ConsoleChatFacadeImpl {
    async fn cancel_acp_passthrough_turn(&self, agent_id: &str, chat_id: &str) -> bool {
        let Some(registry) = &self.acp_chat_registry else {
            return false;
        };
        let Some(handle) = registry.get(agent_id) else {
            return false;
        };
        let Some(session) = handle.manager.get(chat_id).await else {
            return false;
        };

        let sid = SessionId::new(chat_id.to_string());
        if let Err(error) = handle.service.cancel_turn(&sid, &session.runner).await {
            warn!(
                agent_id = %agent_id,
                chat_id = %chat_id,
                runner = %session.runner,
                error = %error,
                "failed to cancel active ACP passthrough turn"
            );
        }
        true
    }
}

pub const CONSOLE_CHANNEL_ID: &str = "console";
pub(crate) const TASK_WATCH_NOTIFICATION_TURN_META_KEY: &str = "task_watch_notification_turn";

#[derive(Debug, Clone)]
pub struct ConversationExecutionContext {
    pub address: ChatAddress,
    pub execution_agent_id: String,
    pub scope_root: PathBuf,
    pub working_dir: PathBuf,
}

pub fn candidate_completion_capability(
    driver: crate::services::conversation_authority::ConversationDriverKind,
) -> crate::services::candidate_completion::DriverCompletionCapability {
    match driver {
        crate::services::conversation_authority::ConversationDriverKind::Rua => {
            crate::services::candidate_completion::DriverCompletionCapability::Continuable
        }
        crate::services::conversation_authority::ConversationDriverKind::Acp => {
            crate::services::candidate_completion::DriverCompletionCapability::AcceptOnly
        }
    }
}

pub fn candidate_continuation_request(
    request: &dto::ChatRequest,
    contexts: Vec<crate::services::candidate_completion::HookContinuationContext>,
) -> dto::ChatRequest {
    let mut continuation = request.clone();
    continuation.message.clear();
    continuation.input = None;
    continuation.messages = None;
    continuation.attachments.clear();
    continuation
        .extra
        .remove(crate::services::conversation_authority::SUBMISSION_ID_KEY);
    continuation
        .extra
        .remove(crate::services::conversation_authority::INPUT_ID_KEY);
    continuation
        .extra
        .remove(crate::services::conversation_authority::ACCEPTANCE_MODE_KEY);
    continuation.extra.insert(
        rua_agent::runtime::HOOK_CONTINUATION_CONTEXT_METADATA_KEY.to_string(),
        serde_json::to_value(contexts).expect("Hook continuation contexts serialize"),
    );
    continuation
}

fn candidate_continuation_pending(
    request: &dto::ChatRequest,
    contexts: Vec<crate::services::candidate_completion::HookContinuationContext>,
) -> ConversationInputPayload {
    let continuation = candidate_continuation_request(request, contexts);
    ConversationInputPayload {
        message: continuation.message,
        model_override: continuation
            .model
            .as_ref()
            .map(dto::ChatModelInput::model_id)
            .map(str::to_string),
        input: continuation.input,
        messages: continuation.messages,
        extra: continuation.extra,
        attachments: Vec::new(),
        enqueued_at: Instant::now(),
    }
}

fn pending_to_dto_request(
    pending: ConversationInputPayload,
    resolved_chat_id: &str,
) -> dto::ChatRequest {
    let chat_id_for_dto = resolved_chat_id.to_string();
    let attachments: Vec<dto::AttachmentDto> = pending
        .attachments
        .iter()
        .map(|v| dto::AttachmentDto {
            url: v
                .get("url")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            mime_type: v
                .get("mime_type")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            stored_name: v
                .get("stored_name")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            size: v.get("size").and_then(|x| x.as_u64()),
            extra: serde_json::Map::new(),
        })
        .collect();
    dto::ChatRequest {
        message: pending.message,
        chat_id: Some(chat_id_for_dto),
        input: pending.input,
        messages: pending.messages,
        user_id: None,
        channel: Some(CONSOLE_CHANNEL_ID.into()),
        stream: Some(true),
        model: pending.model_override.map(dto::ChatModelInput::LegacyId),
        attachments,
        extra: pending.extra,
    }
}

async fn materialize_runtime_observation_turn(
    scope_root: &Path,
    chat_id: &str,
    turn_id: &str,
    value: &serde_json::Value,
    turn_agent_assignment: Option<&serde_json::Value>,
) -> std::result::Result<(), String> {
    let note: rua_session::RuntimeNote =
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    let store = WorkspaceSessionStore::new(scope_root);
    if store
        .read_turn_range(chat_id, 1, usize::MAX)
        .await
        .map_err(|error| error.to_string())?
        .iter()
        .any(|turn| turn.turn_id == turn_id)
    {
        store
            .append_runtime_note(chat_id, turn_id, note)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let created_at = note.created_at.clone();
    let mut message = note
        .into_new_conversation_message()
        .map_err(|error| error.to_string())?;
    if let Some(assignment) = turn_agent_assignment {
        message.metadata.insert(
            rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
            assignment.clone(),
        );
    }
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: created_at,
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![message],
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn current_sync_device_id(
    sync_device: &Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
) -> Option<String> {
    sync_device.read().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|device| hex::encode(device.device_id_short()))
    })
}

fn console_submit_outcome_from_delivery(
    outcome: crate::services::conversation_input_intake::ConversationInputDeliveryOutcome,
) -> ConsoleSubmitOutcome {
    use crate::services::conversation_input_intake::{
        ConversationInputDeliveryOutcome, ConversationInputDeliveryTransport,
    };

    match outcome {
        ConversationInputDeliveryOutcome::Delivered {
            transport: ConversationInputDeliveryTransport::LocalAuthority,
            accepted,
            chat_id,
            queued_to_active_authority,
            input_id,
            submission_id,
            detail,
            control,
            ..
        } => {
            if accepted && queued_to_active_authority {
                let mut outcome = ConsoleSubmitOutcome::queued(chat_id, input_id);
                outcome.submission_id = submission_id;
                outcome.detail = detail;
                outcome.control = control.and_then(|value| serde_json::to_value(*value).ok());
                outcome
            } else if accepted {
                let mut outcome = ConsoleSubmitOutcome::started(chat_id);
                outcome.input_id = input_id;
                outcome.submission_id = submission_id;
                outcome.detail = detail;
                outcome.control = control.and_then(|value| serde_json::to_value(*value).ok());
                outcome
            } else {
                ConsoleSubmitOutcome {
                    accepted,
                    kind: rua_channel::core::console::ConsoleSubmitKind::Started,
                    detail,
                    chat_id: Some(chat_id),
                    input_id,
                    submission_id,
                    device_delivery_id: None,
                    target_device_id: None,
                    device_delivery_sequence: None,
                    accepted_at: None,
                    failure_kind: None,
                    control: control.and_then(|value| serde_json::to_value(*value).ok()),
                }
            }
        }
        ConversationInputDeliveryOutcome::Delivered {
            transport: ConversationInputDeliveryTransport::RemoteAuthority,
            accepted,
            detail,
            chat_id,
            queued_to_active_authority,
            input_id,
            submission_id,
            device_delivery_id,
            target_device_id,
            device_delivery_sequence,
            accepted_at,
            control,
            ..
        } => ConsoleSubmitOutcome {
            accepted,
            kind: if queued_to_active_authority {
                rua_channel::core::console::ConsoleSubmitKind::Queued
            } else {
                rua_channel::core::console::ConsoleSubmitKind::Started
            },
            detail,
            chat_id: Some(chat_id),
            input_id,
            submission_id,
            device_delivery_id,
            target_device_id,
            device_delivery_sequence,
            accepted_at,
            failure_kind: None,
            control: control.and_then(|value| serde_json::to_value(*value).ok()),
        },
        ConversationInputDeliveryOutcome::Rejected {
            detail,
            target_device_id: Some(target_device_id),
            failure_kind,
        } => {
            let mut outcome = ConsoleSubmitOutcome::device_delivery(
                false,
                Some(detail),
                None,
                None,
                Some(target_device_id),
                None,
                None,
            );
            outcome.failure_kind = failure_kind;
            outcome
        }
        ConversationInputDeliveryOutcome::Rejected {
            detail,
            target_device_id: None,
            failure_kind,
        } => ConsoleSubmitOutcome {
            accepted: false,
            kind: rua_channel::core::console::ConsoleSubmitKind::Started,
            detail: Some(detail),
            chat_id: None,
            input_id: None,
            submission_id: None,
            device_delivery_id: None,
            target_device_id: None,
            device_delivery_sequence: None,
            accepted_at: None,
            failure_kind,
            control: None,
        },
    }
}

async fn publish_chat_live_activity(
    hub: Option<&Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>>,
    stream_id: &str,
    address: &ChatAddress,
    authority_epoch: ConversationAuthorityEpoch,
    payload: String,
) {
    if let Some(hub) = hub {
        hub.publish_conversation_event(stream_id, address, authority_epoch, payload)
            .await;
    }
}

struct TurnTerminalPublication {
    stream_id: String,
    authority_epoch: ConversationAuthorityEpoch,
    payloads: Vec<String>,
}

async fn publish_turn_terminal(
    publication: TurnTerminalPublication,
    hub: Option<&Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>>,
    streams: &ActiveStreamRegistry,
    key: &StreamKey,
    address: &ChatAddress,
) {
    for payload in publication.payloads {
        publish_chat_live_activity(
            hub,
            &publication.stream_id,
            address,
            publication.authority_epoch,
            payload.clone(),
        )
        .await;
        streams.push_event_keyed(key, payload).await;
    }
    if let Some(hub) = hub {
        hub.finish_conversation(&publication.stream_id, address, publication.authority_epoch)
            .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn start_initial_acp(
    registry: Option<&Arc<crate::services::acp_chat_registry::AcpChatRegistry>>,
    authority: &Arc<crate::services::conversation_authority::ConversationAuthority>,
    address: &rua_core::ChatAddress,
    agent_id: &str,
    scope_root: &Path,
    working_dir: &Path,
    mut control: tokio::sync::watch::Receiver<rua_core::RunControl>,
) -> Result<(), String> {
    let store = rua_session::WorkspaceSessionStore::new(scope_root);
    let header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .map_err(|error| error.to_string())?
        .ok_or("accepted Chat is missing")?;
    let Some(value) = header
        .extra
        .get(crate::services::conversation_input_intake::INITIAL_ACP_KEY)
    else {
        return Ok(());
    };
    let selection: crate::services::conversation_input_intake::InitialAcpSelection =
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    store
        .update_chat_header(address.chat_id().as_str(), |header| {
            header
                .extra
                .remove(crate::services::conversation_input_intake::INITIAL_ACP_KEY);
        })
        .await
        .map_err(|error| error.to_string())?;
    if control.borrow().is_terminal() {
        return Err("ACP startup interrupted before execution".into());
    }
    let handle = registry
        .and_then(|registry| registry.get(agent_id))
        .ok_or("ACP chat runtime is unavailable")?;
    let active = authority
        .active_turn(address)
        .await
        .ok_or("accepted ACP Turn is missing")?;
    let cwd = if address.project_id().is_some() {
        working_dir.to_string_lossy().to_string()
    } else {
        selection
            .cwd
            .clone()
            .unwrap_or_else(|| working_dir.to_string_lossy().to_string())
    };
    let chat_id = address.chat_id().as_str();
    let startup = async {
        if let Some(resume) = selection.resume_session_id.as_deref() {
            handle
                .controller
                .resume(chat_id, &selection.runner, resume, Some(&cwd))
                .await
        } else {
            handle
                .controller
                .enter(chat_id, &selection.runner, Some(&cwd))
                .await
        }
    };
    let interrupted = async {
        while !control.borrow().is_terminal() {
            if control.changed().await.is_err() {
                futures::future::pending::<()>().await;
            }
        }
    };
    let message = tokio::select! {
        biased;
        _ = interrupted => {
            handle.service.close_chat_session(&rua_core::id::SessionId::new(chat_id), &selection.runner).await.map_err(|error| error.to_string())?;
            handle.controller.exit(chat_id).await;
            return Err("ACP startup interrupted".into());
        }
        message = startup => message,
    };
    if control.borrow().is_terminal() {
        handle
            .service
            .close_chat_session(&rua_core::id::SessionId::new(chat_id), &selection.runner)
            .await
            .map_err(|error| error.to_string())?;
        handle.controller.exit(chat_id).await;
        return Err("ACP startup interrupted".into());
    }
    let session = handle.manager.get(chat_id).await.ok_or_else(|| {
        format!(
            "ACP runner '{}' failed to start: {message}",
            selection.runner
        )
    })?;
    let binding = authority
        .bind_initial_acp_turn(
            scope_root,
            address,
            agent_id,
            &active.turn_id,
            active.authority_epoch,
            crate::services::chat_metadata_store::AcpBindingMetadata {
                target_device_id: Some(active.authority_device_id),
                runner: selection.runner,
                cwd: Some(cwd),
                runner_session_id: session.runner_session_id,
            },
        )
        .await;
    if let Err(error) = binding {
        handle.controller.exit(chat_id).await;
        return Err(error.to_string());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_conversation_producer(
    key: StreamKey,
    mut producer_id: uuid::Uuid,
    mut cancel_rx: tokio::sync::watch::Receiver<rua_core::RunControl>,
    streams: Arc<ActiveStreamRegistry>,
    chat_service: Arc<crate::services::usage::UsageRecordingChatService>,
    workspace_registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
    acp_chat_registry: Option<Arc<crate::services::acp_chat_registry::AcpChatRegistry>>,
    execution: ConversationExecutionContext,
    active_stream_broadcast_hub: Option<
        Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>,
    >,
    conversation_authority: Option<Arc<ConversationAuthority>>,
    candidate_completion_runtime: Option<
        Arc<crate::services::candidate_completion::CandidateCompletionRuntime>,
    >,
    session_sync: Option<SessionSyncEmitter>,
    remote_input_delivery: Option<RemoteConversationInputDelivery>,
    task_watch_wake_receipts: TaskWatchWakeReceipts,
) {
    let ConversationExecutionContext {
        address,
        execution_agent_id: agent_id,
        scope_root: workspace_dir,
        working_dir,
    } = execution;
    let chat_id = address.chat_id().to_string();
    tracing::debug!(
        producer_id = %producer_id,
        agent_id = %agent_id,
        chat_id = %chat_id,
        "console producer started"
    );

    let mut events_emitted: u64 = 0;
    let mut errors_emitted: u64 = 0;
    let exit_reason: &'static str;
    let mut authority_finished = false;
    let mut claimed_next_input: Option<ConversationInputPayload> = None;
    let mut candidate_budget_turn_id: Option<String> = None;
    let mut candidate_budget_started = Instant::now();

    loop {
        let mut pending = if let Some(pending) = claimed_next_input.take() {
            pending
        } else if let Some(authority) = conversation_authority.as_ref() {
            match authority
                .claim_next_or_finish(streams.as_ref(), &key, producer_id, &address)
                .await
            {
                Some(dequeued) => dequeued.pending,
                None => {
                    exit_reason = "queue-empty";
                    authority_finished = true;
                    break;
                }
            }
        } else {
            exit_reason = "queue-empty";
            break;
        };
        let task_watch_notification_id = pending
            .extra
            .get(crate::services::conversation_input_intake::RUNTIME_OBSERVATION_RECEIPT_META_KEY)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        if let Some(observation) = pending
            .extra
            .get(crate::services::conversation_input_intake::RUNTIME_OBSERVATION_META_KEY)
            .cloned()
        {
            let turn_agent_assignment = pending
                .extra
                .get(rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY)
                .cloned();
            let Some(turn_id) = pending
                .extra
                .get("turn_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
            else {
                tracing::warn!(chat_id, "Task Watch notification is missing its Turn ID");
                if let Some(notification_id) = task_watch_notification_id.as_deref() {
                    task_watch_wake_receipts.complete(notification_id);
                }
                continue;
            };
            if let Err(error) = materialize_runtime_observation_turn(
                &workspace_dir,
                &chat_id,
                &turn_id,
                &observation,
                turn_agent_assignment.as_ref(),
            )
            .await
            {
                tracing::warn!(chat_id, error = %error, "failed to materialize Task Watch notification Turn");
                if let Some(notification_id) = task_watch_notification_id.as_deref() {
                    task_watch_wake_receipts.complete(notification_id);
                }
                continue;
            }
            pending.extra.insert(
                TASK_WATCH_NOTIFICATION_TURN_META_KEY.to_string(),
                serde_json::Value::Bool(true),
            );
        }
        let device_delivery = DeviceDeliveryMetadata::from_extra(&pending.extra);
        let acp_start_error = start_initial_acp(
            acp_chat_registry.as_ref(),
            conversation_authority.as_ref().expect("producer Authority"),
            &address,
            &agent_id,
            &workspace_dir,
            &working_dir,
            cancel_rx.clone(),
        )
        .await
        .err();
        let active_turn = conversation_authority
            .as_ref()
            .expect("Conversation producer requires Conversation Authority")
            .active_turn(&address)
            .await
            .expect("Conversation producer requires an Active Turn");
        let authority_epoch = active_turn.authority_epoch;
        let driver_capability = candidate_completion_capability(active_turn.driver);
        let live_stream_id = device_delivery
            .as_ref()
            .map(|remote| remote.device_delivery_id.clone())
            .unwrap_or_else(|| format!("chat-live-{}", uuid::Uuid::new_v4()));

        let existing_runtime = workspace_registry.get(&agent_id).await;
        let runtime_result = match existing_runtime {
            Some(runtime) => Ok(runtime),
            None => workspace_registry.get_or_create(&agent_id).await,
        };
        let runtime = match runtime_result {
            Ok(rt) => rt,
            Err(err) => {
                tracing::warn!(
                    agent_id = %agent_id,
                    error = %err,
                    "console producer: workspace unavailable, emitting error event"
                );
                let detail = format!("workspace for '{agent_id}' unavailable: {err}");
                let json =
                    rua_channel::core::console::make_response_done_json(&chat_id, Some("error"));
                streams.push_event_keyed(&key, detail).await;
                streams.push_event_keyed(&key, json).await;
                if let Some(notification_id) = task_watch_notification_id.as_deref() {
                    task_watch_wake_receipts.complete(notification_id);
                }
                continue;
            }
        };

        let mut dto_req = pending_to_dto_request(pending, &chat_id);
        if !dto_req.extra.contains_key("chat_model_slot") {
            let store = WorkspaceSessionStore::new(&workspace_dir);
            let slot = match persisted_chat_model_slot(&store, &chat_id).await {
                Some(slot) => Some(slot),
                None => {
                    chat_model_slot_from_conversation_authority(
                        conversation_authority.as_ref(),
                        &agent_id,
                        &chat_id,
                    )
                    .await
                }
            };
            if let Some(slot) = slot {
                dto_req.extra.insert("chat_model_slot".to_string(), slot);
            }
        }
        let authority_turn_id = dto_req
            .extra
            .get("turn_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        if let (Some(error), Some(turn_id)) =
            (acp_start_error.as_deref(), authority_turn_id.as_deref())
        {
            let result = if cancel_rx.borrow().is_stop() {
                persist_conversation_user_stop_to_session(
                    &workspace_dir,
                    &agent_id,
                    &chat_id,
                    turn_id,
                    session_sync.as_ref(),
                )
                .await
            } else {
                persist_conversation_runtime_failure_to_session(
                    &workspace_dir,
                    &agent_id,
                    &chat_id,
                    turn_id,
                    error,
                    session_sync.as_ref(),
                )
                .await
            };
            if let Err(persist_error) = result {
                tracing::error!(%address, %persist_error, "cannot commit initial ACP startup failure");
            }
        }
        if candidate_budget_turn_id.as_deref() != authority_turn_id.as_deref() {
            candidate_budget_turn_id = authority_turn_id.clone();
            candidate_budget_started = Instant::now();
        }
        let compatibility_error = match (
            candidate_completion_runtime.as_ref(),
            authority_turn_id.as_deref(),
        ) {
            (Some(runtime), Some(turn_id)) => runtime
                .validate_driver_compatibility(&address, turn_id, driver_capability)
                .await
                .err(),
            _ => None,
        };
        let event_stream: Pin<
            Box<
                dyn futures::Stream<Item = std::result::Result<dto::ChatEvent, String>>
                    + Send
                    + 'static,
            >,
        > = if let Some(error) = compatibility_error.or(acp_start_error) {
            Box::pin(futures::stream::once(async move { Err(error) }))
        } else {
            chat_service.chat_stream_in_context(
                ConversationExecutionContext {
                    address: address.clone(),
                    execution_agent_id: agent_id.clone(),
                    scope_root: workspace_dir.clone(),
                    working_dir: working_dir.clone(),
                },
                runtime,
                dto_req.clone(),
                cancel_rx.clone(),
            )
        };
        let mut adapter = WebUiSseAdapter::new(chat_id.clone());
        futures::pin_mut!(event_stream);

        let mut cancel_rx_inner = cancel_rx.clone();
        let mut cancel_channel_open = true;
        let mut live_turn_id = authority_turn_id;
        let mut live_terminal_status: Option<&'static str> = None;
        let mut terminal_events: Vec<std::result::Result<RuntimeEvent, String>> = Vec::new();
        let mut candidate_response = String::new();
        let mut execution_residency_heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + Duration::from_secs(60),
            Duration::from_secs(60),
        );
        execution_residency_heartbeat
            .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let inner_exit: &'static str;
        loop {
            tokio::select! {
                biased;
                _ = execution_residency_heartbeat.tick() => {
                    if let Some(authority) = conversation_authority.as_ref() {
                        authority
                            .refresh_execution_residency(&address)
                            .await;
                    }
                }
                changed = cancel_rx_inner.changed(), if cancel_channel_open => {
                        match changed {
                            Ok(()) => {
                            let control = *cancel_rx_inner.borrow();
                            if control.is_terminal() {
                                live_terminal_status = Some(
                                    turn_terminal_status_from_run_control(control),
                                );
                                if control.is_interject() {
                                    tracing::debug!(
                                        agent_id = %agent_id,
                                        chat_id = %chat_id,
                                        "console producer: interject signaled, waiting for current stream to finish"
                                    );
                                } else {
                                    tracing::debug!(
                                        agent_id = %agent_id,
                                        chat_id = %chat_id,
                                        "console producer: stop signaled, waiting for persisted terminal candidate"
                                    );
                                }
                            }
                            }
                            Err(_) => cancel_channel_open = false,
                        }
                    }
                item = event_stream.next() => match item {
                    Some(Ok(event)) => {
                        events_emitted += 1;
                        match &event {
                            dto::ChatEvent::TextDelta { delta, .. } => {
                                candidate_response.push_str(delta);
                            }
                            dto::ChatEvent::Error { .. } => {
                                live_terminal_status = Some("failed");
                            }
                            dto::ChatEvent::Interrupted { reason } => {
                                live_terminal_status = Some(
                                    turn_terminal_status_from_finish_reason(
                                        reason.as_deref().or(Some("interrupted")),
                                    ),
                                );
                            }
                            dto::ChatEvent::Done { finish_reason } => {
                                let control = *cancel_rx_inner.borrow();
                                let status = if control.is_terminal() {
                                    turn_terminal_status_from_run_control(control)
                                } else {
                                    turn_terminal_status_from_finish_reason(
                                        finish_reason.as_deref(),
                                    )
                                };
                                if live_terminal_status.is_none() {
                                    live_terminal_status = Some(status);
                                }
                            }
                            _ => {}
                        }
                        if matches!(event, dto::ChatEvent::UserMessage { .. }) {
                            if let Ok(payload) = serde_json::to_string(&event) {
                                let payload_turn_id = turn_id_from_live_user_payload(&payload);
                                live_turn_id = live_turn_id.or(payload_turn_id.clone());
                                if let (Some(authority), Some(turn_id)) =
                                    (conversation_authority.as_ref(), payload_turn_id)
                                {
                                    let _ = authority
                                        .bind_turn(ConversationTurnBinding {
                                            address: address.clone(),
                                            turn_id,
                                            user_message_id: user_message_id_from_live_user_payload(
                                                &payload,
                                            ),
                                        })
                                        .await;
                                }
                                publish_chat_live_activity(
                                    active_stream_broadcast_hub.as_ref(),
                                    &live_stream_id,
                                    &address,
                                    authority_epoch,
                                    payload.clone(),
                                )
                                .await;
                                streams.push_event_keyed(&key, payload).await;
                            }
                            continue;
                        }
                        if matches!(event, dto::ChatEvent::Usage { .. }) {
                            if let Ok(payload) = serde_json::to_string(&event) {
                                publish_chat_live_activity(
                                    active_stream_broadcast_hub.as_ref(),
                                    &live_stream_id,
                                    &address,
                                    authority_epoch,
                                    payload.clone(),
                                )
                                .await;
                                streams.push_event_keyed(&key, payload).await;
                            }
                            continue;
                        }
                        let rt_event = chat_event_to_runtime_event(event);
                        if let Some(authority) = conversation_authority.as_ref() {
                            let _ = authority
                                .observe_runtime_event(
                                    &address,
                                    live_turn_id.as_deref(),
                                    &rt_event,
                                )
                                .await;
                        }
                        if matches!(
                            rt_event,
                            RuntimeEvent::End { .. } | RuntimeEvent::Error { .. }
                        ) {
                            terminal_events.push(Ok(rt_event));
                            continue;
                        }
                        for payload in adapter.map_multi(Ok(rt_event)) {
                            publish_chat_live_activity(
                                active_stream_broadcast_hub.as_ref(),
                                &live_stream_id,
                                &address,
                                authority_epoch,
                                payload.clone(),
                            )
                            .await;
                            streams.push_event_keyed(&key, payload).await;
                        }
                    }
                    Some(Err(e)) => {
                        errors_emitted += 1;
                        live_terminal_status = Some("failed");




                        tracing::warn!(
                            agent_id = %agent_id,
                            chat_id = %chat_id,
                            error = %e,
                            events_emitted_before_error = events_emitted,
                            "console producer: chat_stream emitted Err item, forwarding as SSE error event"
                        );
                        terminal_events.push(Err(e.to_string()));
                    }
                    None => {
                        inner_exit = "stream-end";
                        break;
                    }
                }
            }
        }
        tracing::debug!(
            agent_id = %agent_id,
            chat_id = %chat_id,
            events_emitted,
            errors_emitted,
            reason = inner_exit,
            "console producer: inner stream loop ended"
        );

        let mut control = *cancel_rx.borrow();
        let mut status = live_terminal_status_from_control(live_terminal_status, control);
        if inner_exit == "stream-end" && status == "completed" && !control.is_terminal() {
            if let (Some(runtime), Some(turn_id)) = (
                candidate_completion_runtime.as_ref(),
                live_turn_id.as_deref(),
            ) {
                let cancellation = tokio_util::sync::CancellationToken::new();
                let cancellation_signal = cancellation.clone();
                let mut hook_cancel_rx = cancel_rx.clone();
                let cancellation_bridge = tokio::spawn(async move {
                    loop {
                        if hook_cancel_rx.borrow().is_terminal() {
                            cancellation_signal.cancel();
                            break;
                        }
                        if hook_cancel_rx.changed().await.is_err() {
                            break;
                        }
                    }
                });
                let disposition = runtime
                    .evaluate(crate::services::candidate_completion::CandidateCompletion {
                        address: address.clone(),
                        turn_id: turn_id.to_string(),
                        driver_capability,
                        candidate: serde_json::json!({
                            "status": status,
                            "response": candidate_response,
                        }),
                        cancellation,
                        remaining_budget: Duration::from_secs(30)
                            .saturating_sub(candidate_budget_started.elapsed()),
                    })
                    .await;
                cancellation_bridge.abort();
                control = *cancel_rx.borrow();
                if control.is_terminal() {
                    status = turn_terminal_status_from_run_control(control);
                } else {
                    match disposition {
                        crate::services::candidate_completion::CompletionDisposition::Accept => {}
                        crate::services::candidate_completion::CompletionDisposition::Continue {
                            contexts,
                        } => {
                            claimed_next_input =
                                Some(candidate_continuation_pending(&dto_req, contexts));
                            continue;
                        }
                        crate::services::candidate_completion::CompletionDisposition::Failed {
                            diagnostics,
                        } => {
                            status = "failed";
                            terminal_events.push(Err(diagnostics.join("; ")));
                        }
                    }
                }
            }
        }
        let completed_at = chrono::Utc::now().to_rfc3339();
        let queue_drain_paused = match conversation_authority.as_ref() {
            Some(authority) => authority.is_queue_drain_paused(&address).await,
            None => false,
        };
        let drain_queue = !control.is_stop() || !queue_drain_paused;

        if terminal_events.is_empty() {
            terminal_events.push(Ok(RuntimeEvent::End {
                finish_reason: Some(status.to_string()),
            }));
        }
        let mut provider_terminal_payloads = Vec::new();
        for event in terminal_events {
            for payload in adapter.map_multi(event) {
                provider_terminal_payloads.push(payload);
            }
        }
        let terminal_handoff = if inner_exit == "stream-end" {
            match (conversation_authority.as_ref(), live_turn_id.as_deref()) {
                (Some(authority), Some(turn_id)) => {
                    let hub = active_stream_broadcast_hub.clone();
                    let publish_streams = Arc::clone(&streams);
                    let publish_key = key.clone();
                    let publish_address = address.clone();
                    let publish_stream_id = live_stream_id.clone();
                    let publish_tail = provider_terminal_payloads.clone();
                    authority
                        .complete_active_turn_and_drain_queue(
                            streams.as_ref(),
                            &key,
                            &address,
                            turn_id,
                            status,
                            Some(completed_at.clone()),
                            device_delivery
                                .as_ref()
                                .map(|remote| remote.device_delivery_id.clone()),
                            drain_queue,
                            move |fact| async move {
                                let mut payloads = publish_tail;
                                payloads.push(turn_terminal_payload_from_fact(fact));
                                publish_turn_terminal(
                                    TurnTerminalPublication {
                                        stream_id: publish_stream_id,
                                        authority_epoch,
                                        payloads,
                                    },
                                    hub.as_ref(),
                                    publish_streams.as_ref(),
                                    &publish_key,
                                    &publish_address,
                                )
                                .await;
                            },
                        )
                        .await
                }
                _ => None,
            }
        } else {
            None
        };

        if inner_exit == "stream-end" {
            if let Some(device_delivery) = device_delivery.as_ref() {
                let session_sync = session_sync.clone();
                let remote_input_delivery = remote_input_delivery.clone();
                let workspace_dir = workspace_dir.clone();
                let address = address.clone();
                let source_device_id = device_delivery.source_device_id.clone();
                tokio::spawn(async move {
                    if let Some(session_sync) = session_sync.as_ref() {
                        session_sync
                            .emit_evergreen_chat_snapshot(&workspace_dir, &address)
                            .await;
                    }
                    if let Some(remote_input_delivery) = remote_input_delivery.as_ref() {
                        remote_input_delivery
                            .priority_sync_to_device(&source_device_id)
                            .await;
                    }
                });
            }
        }

        if terminal_handoff.is_none() {
            let mut payloads = provider_terminal_payloads;
            if let Some(turn_id) = live_turn_id.as_deref() {
                payloads.push(turn_terminal_payload(TurnTerminalPayload {
                    chat_id: &chat_id,
                    agent_id: &agent_id,
                    turn_id,
                    status,
                    completed_at: Some(&completed_at),
                    authority_device_id: None,
                    authority_epoch,
                    device_delivery_id: device_delivery
                        .as_ref()
                        .map(|remote| remote.device_delivery_id.as_str()),
                }));
            }
            publish_turn_terminal(
                TurnTerminalPublication {
                    stream_id: live_stream_id,
                    authority_epoch,
                    payloads,
                },
                active_stream_broadcast_hub.as_ref(),
                streams.as_ref(),
                &key,
                &address,
            )
            .await;
        }

        let turn_completed = terminal_handoff.is_some();
        let next_input = terminal_handoff.and_then(|handoff| handoff.next_input);
        if let Some(notification_id) = task_watch_notification_id.as_deref() {
            task_watch_wake_receipts.complete(notification_id);
        }
        if let Some(next) = next_input {
            if control.is_stop() {
                assert!(
                    streams.mark_completed(&key, producer_id).await,
                    "stopped console producer must own its active stream"
                );
                let (next_producer_id, next_cancel_rx) = streams.register_active_keyed(&key).await;
                producer_id = next_producer_id;
                cancel_rx = next_cancel_rx;
            }
            claimed_next_input = Some(next.pending);
            if !control.is_stop() {
                streams.clear_pending_control(&key).await;
            }
            continue;
        };
        if turn_completed {
            streams.mark_completed(&key, producer_id).await;
            authority_finished = true;
            exit_reason = if control.is_stop() {
                "stopped-after-completion"
            } else {
                "queue-empty"
            };
            break;
        }
        if control.is_terminal() && !control.is_stop() {
            streams.clear_pending_control(&key).await;
        }
        exit_reason = "stream-end-without-authority-completion";
        break;
    }

    tracing::debug!(
        producer_id = %producer_id,
        agent_id = %agent_id,
        chat_id = %chat_id,
        events_emitted,
        errors_emitted,
        reason = exit_reason,
        "console producer exiting, calling mark_completed"
    );
    if let Some(conversation_authority) = conversation_authority.as_ref() {
        if !authority_finished {
            conversation_authority
                .finish_producer(
                    streams.as_ref(),
                    &key,
                    producer_id,
                    &address,
                    cancel_rx.borrow().is_terminal()
                        && !cancel_rx.borrow().is_interject()
                        && !cancel_rx.borrow().is_stop(),
                )
                .await;
        }
    } else {
        streams.mark_completed(&key, producer_id).await;
    }
}

fn turn_id_from_live_user_payload(payload: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()?
        .get("message")?
        .get("turn_id")?
        .as_str()
        .map(str::to_string)
}

fn user_message_id_from_live_user_payload(payload: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()?
        .get("message")?
        .get("id")?
        .as_str()
        .map(str::to_string)
}

pub(crate) fn turn_terminal_status_from_finish_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("error" | "failed" | "failure") => "failed",
        Some("stopped" | "cancelled" | "canceled" | "interrupted" | "max_iterations") => {
            "interrupted"
        }
        Some("interject" | "interjected") => "interjected",
        _ => "completed",
    }
}

pub(crate) fn turn_terminal_status_from_run_control(control: rua_core::RunControl) -> &'static str {
    if control.is_interject() {
        "interjected"
    } else if matches!(control, rua_core::RunControl::ContinueAtBoundary) {
        "completed"
    } else {
        "interrupted"
    }
}

fn live_terminal_status_from_control(
    status: Option<&'static str>,
    control: rua_core::RunControl,
) -> &'static str {
    if control.is_terminal() {
        turn_terminal_status_from_run_control(control)
    } else {
        status.unwrap_or("completed")
    }
}

pub(crate) fn turn_terminal_payload_from_fact(fact: ConversationTurnTerminalFact) -> String {
    match fact.address.agent_id() {
        Some(agent_id) => turn_terminal_payload(TurnTerminalPayload {
            chat_id: fact.address.chat_id().as_str(),
            agent_id: agent_id.as_str(),
            turn_id: &fact.turn_id,
            status: &fact.status,
            completed_at: fact.completed_at.as_deref(),
            authority_device_id: Some(fact.authority_device_id.as_str()),
            authority_epoch: fact.authority_epoch,
            device_delivery_id: fact.device_delivery_id.as_deref(),
        }),
        None => {
            let chat_id = fact.address.chat_id().to_string();
            serde_json::json!({
                "type": "turn_terminal",
                "address": fact.address,
                "chat_id": chat_id,
                "turn_id": fact.turn_id,
                "status": fact.status,
                "completed_at": fact.completed_at,
                "authority_device_id": fact.authority_device_id,
                "authority_epoch": fact.authority_epoch,
                "device_delivery_id": fact.device_delivery_id,
            })
            .to_string()
        }
    }
}

struct TurnTerminalPayload<'a> {
    chat_id: &'a str,
    agent_id: &'a str,
    turn_id: &'a str,
    status: &'a str,
    completed_at: Option<&'a str>,
    authority_device_id: Option<&'a str>,
    authority_epoch: ConversationAuthorityEpoch,
    device_delivery_id: Option<&'a str>,
}

fn turn_terminal_payload(payload: TurnTerminalPayload<'_>) -> String {
    serde_json::json!({
        "type": "turn_terminal",
        "agent_id": payload.agent_id,
        "chat_id": payload.chat_id,
        "turn_id": payload.turn_id,
        "status": payload.status,
        "completed_at": payload.completed_at,
        "authority_device_id": payload.authority_device_id,
        "authority_epoch": payload.authority_epoch,
        "device_delivery_id": payload.device_delivery_id,
    })
    .to_string()
}

use rua_core::event::{HistoryManager, HistoryMessage};

pub fn build_history_from_events(
    user_message: &str,
    events: &[RuntimeEvent],
) -> Vec<HistoryMessage> {
    let mut mgr = HistoryManager::new();
    mgr.add_user_message(user_message);
    for event in events {
        mgr.process_event(event);
    }
    mgr.into_messages()
}

pub fn history_to_input_messages(history: &[HistoryMessage]) -> Vec<AgentInputMessage> {
    let mut result = Vec::new();
    for msg in history {
        match msg {
            HistoryMessage::User { content, .. } => {
                result.push(AgentInputMessage {
                    role: "user".into(),
                    content: vec![ContentPart::Text {
                        text: content.clone(),
                    }],
                });
            }
            HistoryMessage::Assistant { content, .. } => {
                result.push(AgentInputMessage {
                    role: "assistant".into(),
                    content: vec![ContentPart::Text {
                        text: content.clone(),
                    }],
                });
            }
            HistoryMessage::Reasoning { .. } => {}
            HistoryMessage::PluginCall { call, .. } => {
                if let Some(last) = result.last_mut() {
                    if last.role == "assistant" {
                        last.content
                            .push(ContentPart::ToolUse { call: call.clone() });
                        continue;
                    }
                }
                result.push(AgentInputMessage {
                    role: "assistant".into(),
                    content: vec![ContentPart::ToolUse { call: call.clone() }],
                });
            }
            HistoryMessage::PluginCallOutput { result: tr, .. } => {
                result.push(AgentInputMessage {
                    role: "user".into(),
                    content: vec![ContentPart::ToolResult { result: tr.clone() }],
                });
            }
            HistoryMessage::Error { .. } | HistoryMessage::Cancel { .. } => {}
        }
    }
    result
}

pub fn sanitize_filename(name: &str) -> String {
    let name = name
        .rsplit('/')
        .next()
        .unwrap_or(name)
        .rsplit('\\')
        .next()
        .unwrap_or(name);
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "upload.bin".to_string()
    } else {
        sanitized
    }
}

pub fn mime_type_from_extension(filename: &str) -> Option<String> {
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg".into()),
        "png" => Some("image/png".into()),
        "gif" => Some("image/gif".into()),
        "webp" => Some("image/webp".into()),
        "svg" => Some("image/svg+xml".into()),
        "bmp" => Some("image/bmp".into()),
        "mp3" => Some("audio/mpeg".into()),
        "wav" => Some("audio/wav".into()),
        "ogg" => Some("audio/ogg".into()),
        "mp4" => Some("video/mp4".into()),
        "webm" => Some("video/webm".into()),
        "pdf" => Some("application/pdf".into()),
        "txt" | "md" | "log" => Some("text/plain".into()),
        "json" => Some("application/json".into()),
        "csv" => Some("text/csv".into()),
        _ => None,
    }
}

pub fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/")
}

pub fn inject_file_path_hints(content: &mut Vec<ContentPart>) {
    let mut i = 0;
    while i < content.len() {
        let hint_text = match &content[i] {
            ContentPart::File { url, name, .. } => {
                Some(format!("用户上传文件 `{name}`，已经下载到 {url}"))
            }
            ContentPart::Image { url, alt, .. } => {
                let label = alt.as_deref().unwrap_or("image");
                Some(format!("用户上传图片 `{label}`，已经下载到 {url}"))
            }
            ContentPart::Audio { url, .. } => Some(format!("用户上传音频，已经下载到 {url}")),
            ContentPart::Video { url, .. } => Some(format!("用户上传视频，已经下载到 {url}")),
            _ => None,
        };
        if let Some(text) = hint_text {
            content.insert(i + 1, ContentPart::Text { text });
            i += 2;
        } else {
            i += 1;
        }
    }
}

pub fn attachments_to_content_parts(attachments: &[dto::AttachmentDto]) -> Vec<ContentPart> {
    attachments
        .iter()
        .map(|a| {
            let mime = a
                .mime_type
                .clone()
                .or_else(|| mime_type_from_extension(&a.name));
            if mime.as_deref().is_some_and(is_image_mime) {
                ContentPart::Image {
                    url: a.url.clone(),
                    media_type: mime,
                    alt: None,
                }
            } else {
                ContentPart::File {
                    url: a.url.clone(),
                    name: a.name.clone(),
                    media_type: None,
                    size: None,
                }
            }
        })
        .collect()
}
