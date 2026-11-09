use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use crate::early_background_compaction::early_background_compaction_coordinator;
use crate::projection::{ProjectionMessageGroupEstimate, ProjectionPlanner};
use crate::{
    active_work_plan::{
        active_work_plan_checksum, ActiveWorkPlanContent, ActiveWorkPlanSnapshot,
        ACTIVE_WORK_PLAN_FORMAT_VERSION,
    },
    derive_history_replacement_record_id,
    history_replacement::{
        apply_ready_history_replacements_with_evidence, detect_execution_trace_blocks,
        history_replacement_checksum, validate_history_replacement_record,
    },
    ExecutionTraceBlock, HistoryReplacementReadyDraft, HistoryReplacementRecord,
    HistoryReplacementRecordState, HistoryReplacementRejectedDraft,
    HistoryReplacementRejectedGeneration, HISTORY_REPLACEMENT_FORMAT_VERSION,
    HISTORY_REPLACEMENT_MIN_SOURCE_TOKENS, HISTORY_REPLACEMENT_PROMPT_VERSION,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatTag {
    #[default]
    None,
    Red,
    Yellow,
    Green,
}

impl ChatTag {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Red => "red",
            Self::Yellow => "yellow",
            Self::Green => "green",
        }
    }
}

impl std::str::FromStr for ChatTag {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "red" => Ok(Self::Red),
            "yellow" => Ok(Self::Yellow),
            "green" => Ok(Self::Green),
            _ => Err(format!("unsupported Chat tag: {value}")),
        }
    }
}
use rua_core::{ChatAddress, Result};
use rua_text_search::{TextMatcher, TextSearchQuery};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatPreview {
    #[serde(default)]
    pub last_message_sequence: Option<u64>,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMetadata {
    pub chat_id: String,
    pub agent_id: String,
    pub title: String,
    pub created_at: String,
    pub last_activity_at: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub preview: Option<ChatPreview>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageLogHeader {
    #[serde(default = "default_message_chunk_size")]
    pub chunk_size: u64,
    #[serde(default)]
    pub latest_sequence: u64,
    #[serde(default)]
    pub allocated_sequence: u64,
}

impl Default for MessageLogHeader {
    fn default() -> Self {
        Self {
            chunk_size: default_message_chunk_size(),
            latest_sequence: 0,
            allocated_sequence: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TurnIndexHeader {
    #[serde(default)]
    pub latest_turn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageProviderLink {
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub response_id: Option<String>,
}

const HISTORY_REPLACEMENT_MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct HistoryReplacementManifestFile {
    record_id: String,
    len: u64,
    modified_nanos: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryReplacementManifest {
    version: u32,
    chat_address: ChatAddress,
    files: Vec<HistoryReplacementManifestFile>,
    records: Vec<HistoryReplacementRecord>,
}

#[derive(Debug, Clone, PartialEq)]
struct EffectiveProjectionCacheKey {
    chat_address: ChatAddress,
    header: ChatSessionHeader,
    turn_index: TurnIndex,
    projection_exclusions: ProjectionExclusions,
    history_replacement_files: Vec<HistoryReplacementManifestFile>,
    history_replacement_enabled: bool,
    token_estimate_divisor: f64,
}

pub const PROJECTION_EXCLUSIONS_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectionExclusions {
    pub format_version: u32,
    pub chat_id: String,
    #[serde(default)]
    pub message_ids: Vec<String>,
    pub updated_at: String,
}

impl ProjectionExclusions {
    fn filter_messages(&self, messages: &mut Vec<MessageLogEntry>) {
        if self.message_ids.is_empty() {
            return;
        }
        let excluded = self
            .message_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        messages.retain(|message| !excluded.contains(message.message_id.as_str()));
    }
}

#[derive(Debug, Clone)]
struct EffectiveProjectionCacheEntry {
    key: EffectiveProjectionCacheKey,
    projection: ContextProjection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageToolLink {
    pub tool_name: String,
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewConversationMessage {
    pub message_id: String,
    pub role: String,
    pub created_at: String,
    pub status: String,
    pub content: Value,
    #[serde(default)]
    pub provider: Option<MessageProviderLink>,
    #[serde(default)]
    pub tool: Option<MessageToolLink>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageLogEntry {
    pub message_id: String,
    pub sequence: u64,
    pub turn_id: String,
    pub role: String,
    pub created_at: String,
    pub status: String,
    pub content: Value,
    #[serde(default)]
    pub provider: Option<MessageProviderLink>,
    #[serde(default)]
    pub tool: Option<MessageToolLink>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnLifecycle {
    pub user_submitted_at: String,
    #[serde(default)]
    pub assistant_started_at: Option<String>,
    #[serde(default)]
    pub assistant_completed_at: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnLifecycleStatus {
    InProgress,
    Completed,
    Interrupted,
    Interjected,
    Failed,
    Unknown,
}

impl TurnLifecycleStatus {
    pub fn parse(value: &str) -> Self {
        match value {
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "interrupted" => Self::Interrupted,
            "interjected" => Self::Interjected,
            "failed" => Self::Failed,
            _ => Self::Unknown,
        }
    }

    pub fn is_syncable_conversation_state(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Interrupted | Self::Interjected | Self::Failed
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewConversationTurn {
    pub turn_id: String,
    pub lifecycle: TurnLifecycle,
    #[serde(default)]
    pub selected_variant_id: Option<String>,
    pub messages: Vec<NewConversationMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssistantResponseVariantRecord {
    pub variant_id: String,
    pub first_sequence: u64,
    pub last_sequence: u64,
    #[serde(default)]
    pub assistant_message_sequences: Vec<u64>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnIndexRecord {
    pub turn_id: String,
    pub ordinal: u64,
    pub user_message_sequence: u64,
    #[serde(default)]
    pub assistant_response_sequences: Vec<u64>,
    pub first_sequence: u64,
    pub last_sequence: u64,
    #[serde(default)]
    pub token_estimate: u64,
    pub lifecycle: TurnLifecycle,
    #[serde(default)]
    pub selected_variant_id: Option<String>,
    #[serde(default)]
    pub assistant_response_variants: Vec<AssistantResponseVariantRecord>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, Value>,
}

impl TurnIndexRecord {
    pub fn effective_assistant_response_variants(&self) -> Vec<AssistantResponseVariantRecord> {
        if !self.assistant_response_variants.is_empty() {
            return self.assistant_response_variants.clone();
        }
        if self.assistant_response_sequences.is_empty() {
            return Vec::new();
        }
        let variant_id = self.selected_variant_id.clone().unwrap_or_else(|| {
            format!(
                "assistant-response-{}",
                self.assistant_response_sequences[0]
            )
        });
        let first_output_sequence = if self.user_message_sequence >= self.first_sequence {
            self.user_message_sequence.saturating_add(1)
        } else {
            self.first_sequence
        };
        vec![AssistantResponseVariantRecord {
            variant_id,
            first_sequence: first_output_sequence.min(self.last_sequence),
            last_sequence: self.last_sequence,
            assistant_message_sequences: self.assistant_response_sequences.clone(),
            created_at: self
                .lifecycle
                .assistant_started_at
                .clone()
                .unwrap_or_else(|| self.lifecycle.user_submitted_at.clone()),
            status: self.lifecycle.status.clone(),
        }]
    }

    pub fn selected_assistant_response_variant(&self) -> Option<AssistantResponseVariantRecord> {
        let variants = self.effective_assistant_response_variants();
        self.selected_variant_id
            .as_deref()
            .and_then(|selected| {
                variants
                    .iter()
                    .find(|variant| variant.variant_id == selected)
                    .cloned()
            })
            .or_else(|| variants.last().cloned())
    }
}

fn turn_is_committed_to_selected_variant(
    turn: &TurnIndexRecord,
    selected_variant_id: &str,
) -> bool {
    let [variant] = turn.assistant_response_variants.as_slice() else {
        return false;
    };
    variant.variant_id == selected_variant_id
        && turn.assistant_response_sequences == variant.assistant_message_sequences
}

pub fn turn_origin_is_acp(turn: &TurnIndexRecord) -> bool {
    origin_kind_is_acp(turn.metadata.get("origin"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppendTurnOutputMode {
    ContinueSelected,
    NewVariant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AppendTurnOptions {
    mode: AppendTurnOutputMode,
    preserve_lifecycle: bool,
    deduplicate_message_ids: bool,
    include_in_assistant_variant: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingRuntimeNote {
    turn_id: String,
    note: crate::runtime_note::RuntimeNote,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PendingRuntimeNotes {
    notes: Vec<PendingRuntimeNote>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveTurnMessages {
    user_sequence: Option<u64>,
    output_range: Option<(u64, u64)>,
}

fn is_turn_aborted_marker(entry: &MessageLogEntry) -> bool {
    entry
        .metadata
        .get("model_visible_marker")
        .and_then(Value::as_str)
        == Some("turn_aborted")
}

fn is_primary_user_message(entry: &MessageLogEntry) -> bool {
    entry.role == "user" && !is_turn_aborted_marker(entry)
}

fn is_turn_output(entry: &MessageLogEntry) -> bool {
    entry.role != "user" || is_turn_aborted_marker(entry)
}

fn new_variant_record(
    variant_id: Option<String>,
    entries: &[MessageLogEntry],
    status: &str,
) -> Option<AssistantResponseVariantRecord> {
    let is_variant_output = |entry: &&MessageLogEntry| {
        is_turn_output(entry)
            && crate::runtime_note::RuntimeNote::from_message_log_entry(entry).is_none()
    };
    let first_output = entries.iter().find(is_variant_output)?;
    let last_output = entries.iter().rev().find(is_variant_output)?;
    let assistant_message_sequences = entries
        .iter()
        .filter(|entry| entry.role == "assistant")
        .map(|entry| entry.sequence)
        .collect::<Vec<_>>();
    let variant_id = variant_id
        .or_else(|| {
            entries
                .iter()
                .find(|entry| entry.role == "assistant")
                .map(|entry| entry.message_id.clone())
        })
        .unwrap_or_else(|| format!("assistant-response-{}", first_output.sequence));
    Some(AssistantResponseVariantRecord {
        variant_id,
        first_sequence: first_output.sequence,
        last_sequence: last_output.sequence,
        assistant_message_sequences,
        created_at: first_output.created_at.clone(),
        status: status.to_string(),
    })
}

fn ensure_variant_records_from_legacy(turn: &mut TurnIndexRecord) {
    if !turn.assistant_response_variants.is_empty() {
        return;
    }
    turn.assistant_response_variants = turn.effective_assistant_response_variants();
}

fn extend_selected_variant(
    turn: &mut TurnIndexRecord,
    selected_variant_id: Option<String>,
    entries: &[MessageLogEntry],
    new_assistant_sequences: Vec<u64>,
) {
    let selected_variant_id = turn
        .selected_variant_id
        .clone()
        .or(selected_variant_id)
        .or_else(|| {
            entries
                .iter()
                .find(|entry| entry.role == "assistant")
                .map(|entry| entry.message_id.clone())
        });
    if let Some(selected_variant_id) = selected_variant_id {
        turn.selected_variant_id = Some(selected_variant_id.clone());
        if turn
            .assistant_response_variants
            .iter()
            .all(|variant| variant.variant_id != selected_variant_id)
        {
            if let Some(variant) = new_variant_record(
                Some(selected_variant_id.clone()),
                entries,
                &turn.lifecycle.status,
            ) {
                turn.assistant_response_sequences = variant.assistant_message_sequences.clone();
                turn.assistant_response_variants.push(variant);
                return;
            }
        }
        if let Some(variant) = turn
            .assistant_response_variants
            .iter_mut()
            .find(|variant| variant.variant_id == selected_variant_id)
        {
            if let Some(first_output) = entries.iter().find(|entry| is_turn_output(entry)) {
                variant.first_sequence = variant.first_sequence.min(first_output.sequence);
                variant.created_at = if variant.created_at.is_empty() {
                    first_output.created_at.clone()
                } else {
                    variant.created_at.clone()
                };
            }
            if let Some(last_output) = entries.iter().rev().find(|entry| is_turn_output(entry)) {
                variant.last_sequence = variant.last_sequence.max(last_output.sequence);
            }
            variant
                .assistant_message_sequences
                .extend(new_assistant_sequences);
            variant.status = turn.lifecycle.status.clone();
            turn.assistant_response_sequences = variant.assistant_message_sequences.clone();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TurnIndex {
    #[serde(default)]
    pub turns: Vec<TurnIndexRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointTurnRange {
    pub start_turn_ordinal: u64,
    pub end_turn_ordinal: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointSequenceRange {
    pub start_sequence: u64,
    pub end_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointModelIdentity {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointFrontier {
    #[serde(default, rename = "kind", skip_serializing_if = "Option::is_none")]
    pub legacy_kind: Option<String>,
    pub turn_id: String,
    pub end_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointRetainedSequenceRange {
    pub start_sequence: u64,
    pub end_sequence: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointCoveredVariant {
    pub turn_id: String,
    #[serde(default)]
    pub selected_variant_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointProjectionState {
    pub frontier: CheckpointFrontier,
    pub retained_sequence_ranges: Vec<CheckpointRetainedSequenceRange>,
    pub projection_lineage: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummaryCheckpointDraft {
    pub checkpoint_id: String,
    #[serde(default)]
    pub previous_checkpoint_id: Option<String>,
    pub covered_turn_range: CheckpointTurnRange,
    pub covered_sequence_range: CheckpointSequenceRange,
    pub source_sequence_range: CheckpointSequenceRange,
    pub frontier: CheckpointFrontier,
    #[serde(default)]
    pub retained_sequence_ranges: Vec<CheckpointRetainedSequenceRange>,
    pub projection_lineage: String,
    pub summary: String,
    pub token_estimate: u64,
    pub model: CheckpointModelIdentity,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummaryCheckpoint {
    pub checkpoint_id: String,
    pub chat_id: String,
    pub agent_id: String,
    #[serde(default)]
    pub previous_checkpoint_id: Option<String>,
    pub covered_turn_range: CheckpointTurnRange,
    pub covered_sequence_range: CheckpointSequenceRange,
    pub source_sequence_range: CheckpointSequenceRange,
    pub frontier: CheckpointFrontier,
    #[serde(default)]
    pub retained_sequence_ranges: Vec<CheckpointRetainedSequenceRange>,
    pub projection_lineage: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covered_selected_variants: Vec<CheckpointCoveredVariant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covered_excluded_message_ids: Vec<String>,
    pub summary: String,
    pub token_estimate: u64,
    pub model: CheckpointModelIdentity,
    pub created_at: String,
    pub checksum: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextProjectionRequest {
    pub current_user_input: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextProjection {
    pub chat_id: String,
    pub agent_id: String,
    pub active_checkpoint_id: Option<String>,
    pub summary: Option<String>,
    pub summary_model: Option<CheckpointModelIdentity>,
    pub tail_start_sequence: Option<u64>,
    pub tail_end_sequence: Option<u64>,
    pub retained_messages: Vec<MessageLogEntry>,
    pub tail_messages: Vec<MessageLogEntry>,
    pub current_user_input: Option<String>,
    pub stable_token_estimate: u64,
    pub history_replacement_evidence:
        Vec<crate::history_replacement::HistoryReplacementApplicationEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationRecallRequest {
    pub query: String,
    pub max_results: usize,
    pub surrounding_messages: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationRecallResult {
    pub chat_id: String,
    pub hits: Vec<ConversationRecallHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationRecallHit {
    pub message: ConversationRecallMessage,
    pub snippet: String,
    pub span: ConversationSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationRecallMessage {
    pub sequence: u64,
    pub message_id: String,
    pub turn_id: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationSpan {
    pub start_sequence: u64,
    pub end_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationReadSpanRequest {
    pub start_sequence: u64,
    pub end_sequence: u64,
    pub expand_before: u64,
    pub expand_after: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationReadSpanResult {
    pub chat_id: String,
    pub span: ConversationSpan,
    pub messages: Vec<MessageLogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedConversationSearchRequest {
    pub agent_id: String,
    pub query: String,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedConversationSearchResult {
    pub results: Vec<SavedConversationSearchHit>,
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
    #[serde(default)]
    pub next_offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedConversationSearchHit {
    pub chat_id: String,
    pub title: String,
    #[serde(default)]
    pub preview: Option<ChatPreview>,
    pub message: ConversationRecallMessage,
    pub snippet: String,
    pub span: ConversationSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ChatContextSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sys_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_prompt_files: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_action: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectionBoundary {
    pub kind: String,
    pub sequence_start: u64,
    pub turn_start_ordinal: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatSessionHeader {
    #[serde(default = "default_chat_schema_version")]
    pub schema_version: u32,
    pub chat_id: String,
    pub agent_id: String,
    pub metadata: ChatMetadata,
    #[serde(default)]
    pub tag: ChatTag,
    #[serde(default)]
    pub active_checkpoint_id: Option<String>,
    #[serde(default)]
    pub message_log: MessageLogHeader,
    #[serde(default)]
    pub turn_index: TurnIndexHeader,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_boundary: Option<ProjectionBoundary>,
    #[serde(default)]
    pub context_snapshot: ChatContextSnapshot,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ChatSessionHeader {
    pub fn new(metadata: ChatMetadata) -> Self {
        Self {
            schema_version: default_chat_schema_version(),
            chat_id: metadata.chat_id.clone(),
            agent_id: metadata.agent_id.clone(),
            metadata,
            tag: ChatTag::default(),
            active_checkpoint_id: None,
            message_log: MessageLogHeader::default(),
            turn_index: TurnIndexHeader::default(),
            projection_boundary: None,
            context_snapshot: ChatContextSnapshot::default(),
            extra: serde_json::Map::new(),
        }
    }
}

fn default_chat_schema_version() -> u32 {
    3
}

fn projection_boundary_sequence_start(header: &ChatSessionHeader) -> u64 {
    header
        .projection_boundary
        .as_ref()
        .map(|boundary| boundary.sequence_start.max(1))
        .unwrap_or(1)
}

fn projection_boundary_turn_start(header: &ChatSessionHeader) -> u64 {
    header
        .projection_boundary
        .as_ref()
        .map(|boundary| boundary.turn_start_ordinal.max(1))
        .unwrap_or(1)
}

fn checkpoint_respects_projection_boundary(
    checkpoint: &SummaryCheckpoint,
    boundary: Option<&ProjectionBoundary>,
) -> bool {
    let Some(boundary) = boundary else {
        return true;
    };
    let start = boundary.sequence_start.max(1);
    checkpoint.covered_sequence_range.start_sequence >= start
        && checkpoint.source_sequence_range.start_sequence >= start
        && checkpoint
            .retained_sequence_ranges
            .iter()
            .all(|retained| retained.start_sequence >= start)
}

fn draft_respects_projection_boundary(
    draft: &SummaryCheckpointDraft,
    boundary: Option<&ProjectionBoundary>,
) -> bool {
    let Some(boundary) = boundary else {
        return true;
    };
    let start = boundary.sequence_start.max(1);
    draft.covered_sequence_range.start_sequence >= start
        && draft.source_sequence_range.start_sequence >= start
        && draft
            .retained_sequence_ranges
            .iter()
            .all(|retained| retained.start_sequence >= start)
}

fn default_message_chunk_size() -> u64 {
    1000
}

#[derive(Clone)]
pub struct WorkspaceSessionStore {
    sessions_dir: PathBuf,
    effective_projection_cache:
        Arc<tokio::sync::Mutex<HashMap<ChatAddress, EffectiveProjectionCacheEntry>>>,
}

impl WorkspaceSessionStore {
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            sessions_dir: workspace_dir.into().join("sessions"),
            effective_projection_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    fn chat_write_lock(&self, chat_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        chat_write_lock_for_path(self.chat_dir(chat_id))
    }

    pub fn chat_dir(&self, chat_id: &str) -> PathBuf {
        self.sessions_dir
            .join(sanitize_chat_path_component(chat_id))
    }

    pub async fn lock_chat_storage(&self, chat_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        self.chat_write_lock(chat_id).lock_owned().await
    }

    pub fn chat_header_path(&self, chat_id: &str) -> PathBuf {
        self.chat_dir(chat_id).join("session.json")
    }

    fn projection_exclusions_path(&self, chat_id: &str) -> PathBuf {
        self.chat_dir(chat_id).join("projection_exclusions.json")
    }

    pub async fn load_projection_exclusions(&self, chat_id: &str) -> Result<ProjectionExclusions> {
        let path = self.projection_exclusions_path(chat_id);
        match tokio::fs::read_to_string(&path).await {
            Ok(content) => serde_json::from_str(&content).map_err(|error| rua_core::RuaError::Io {
                message: format!(
                    "failed to parse projection exclusions {}: {error}",
                    path.display()
                ),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(ProjectionExclusions {
                    format_version: PROJECTION_EXCLUSIONS_FORMAT_VERSION,
                    chat_id: chat_id.to_string(),
                    message_ids: Vec::new(),
                    updated_at: String::new(),
                })
            }
            Err(error) => Err(rua_core::RuaError::Io {
                message: format!(
                    "failed to read projection exclusions {}: {error}",
                    path.display()
                ),
            }),
        }
    }

    pub async fn save_projection_exclusions(
        &self,
        chat_id: &str,
        mut message_ids: Vec<String>,
    ) -> Result<ProjectionExclusions> {
        message_ids.sort();
        message_ids.dedup();
        let snapshot = ProjectionExclusions {
            format_version: PROJECTION_EXCLUSIONS_FORMAT_VERSION,
            chat_id: chat_id.to_string(),
            message_ids,
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        write_json_atomic(&self.projection_exclusions_path(chat_id), &snapshot).await?;
        self.effective_projection_cache.lock().await.clear();
        Ok(snapshot)
    }

    fn active_work_plan_path(&self, chat_id: &str) -> PathBuf {
        self.chat_dir(chat_id).join("active_work_plan.json")
    }

    pub async fn save_active_work_plan(
        &self,
        chat_id: &str,
        source_turn_id: Option<&str>,
        source_sequence: Option<u64>,
        content: ActiveWorkPlanContent,
    ) -> Result<ActiveWorkPlanSnapshot> {
        let mut snapshot = ActiveWorkPlanSnapshot {
            format_version: ACTIVE_WORK_PLAN_FORMAT_VERSION,
            chat_id: chat_id.to_string(),
            source_turn_id: source_turn_id.map(str::to_string),
            source_sequence,
            last_injected_sequence: None,
            last_injected_checkpoint_id: None,
            explanation: content.explanation,
            plan: content.plan,
            updated_at: chrono::Utc::now().to_rfc3339(),
            checksum: String::new(),
        };
        snapshot.checksum = active_work_plan_checksum(&snapshot)?;
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        write_json_atomic(&self.active_work_plan_path(chat_id), &snapshot).await?;
        Ok(snapshot)
    }

    pub async fn load_active_work_plan(
        &self,
        chat_id: &str,
    ) -> Result<Option<ActiveWorkPlanSnapshot>> {
        let path = self.active_work_plan_path(chat_id);
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(rua_core::RuaError::Io {
                    message: format!(
                        "failed to read active work plan {}: {error}",
                        path.display()
                    ),
                });
            }
        };
        let snapshot: ActiveWorkPlanSnapshot =
            serde_json::from_str(&content).map_err(|error| rua_core::RuaError::Io {
                message: format!(
                    "failed to parse active work plan {}: {error}",
                    path.display()
                ),
            })?;
        if snapshot.format_version != ACTIVE_WORK_PLAN_FORMAT_VERSION
            || snapshot.chat_id != chat_id
            || snapshot.checksum != active_work_plan_checksum(&snapshot)?
        {
            return Ok(None);
        }
        Ok(Some(snapshot))
    }

    pub async fn active_message_log_end_sequence_for_chat(
        &self,
        chat_id: &str,
    ) -> Result<Option<u64>> {
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Ok(None);
        };
        let index = self.load_turn_index(chat_id).await?;
        Ok(Some(active_message_log_end_sequence(&header, &index)))
    }

    pub async fn mark_active_work_plan_injected(
        &self,
        chat_id: &str,
        expected_checksum: &str,
        sequence: Option<u64>,
        checkpoint_id: Option<&str>,
    ) -> Result<bool> {
        let path = self.active_work_plan_path(chat_id);
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(rua_core::RuaError::Io {
                    message: format!(
                        "failed to read active work plan {}: {error}",
                        path.display()
                    ),
                });
            }
        };
        let mut snapshot: ActiveWorkPlanSnapshot =
            serde_json::from_str(&content).map_err(|error| rua_core::RuaError::Io {
                message: format!(
                    "failed to parse active work plan {}: {error}",
                    path.display()
                ),
            })?;
        if snapshot.format_version != ACTIVE_WORK_PLAN_FORMAT_VERSION
            || snapshot.chat_id != chat_id
            || snapshot.checksum != active_work_plan_checksum(&snapshot)?
            || snapshot.checksum != expected_checksum
        {
            return Ok(false);
        }
        snapshot.last_injected_sequence = sequence;
        snapshot.last_injected_checkpoint_id = checkpoint_id.map(str::to_string);
        snapshot.checksum = active_work_plan_checksum(&snapshot)?;
        write_json_atomic(&path, &snapshot).await?;
        Ok(true)
    }

    pub async fn delete_chat_storage_by_chat_id(&self, chat_id: &str) -> Result<()> {
        let path = self.chat_dir(chat_id);
        let metadata = match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(rua_core::RuaError::Io {
                    message: format!("failed to inspect chat storage {}: {e}", path.display()),
                });
            }
        };
        let result = if metadata.is_dir() {
            tokio::fs::remove_dir_all(&path).await
        } else {
            tokio::fs::remove_file(&path).await
        };
        if let Err(e) = result {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(rua_core::RuaError::Io {
                    message: format!("failed to delete chat storage {}: {e}", path.display()),
                });
            }
        }
        Ok(())
    }

    pub async fn load_chat_header(&self, chat_id: &str) -> Result<Option<ChatSessionHeader>> {
        let path = self.chat_header_path(chat_id);
        if !path.exists() {
            return Ok(None);
        }
        let content =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| rua_core::RuaError::Io {
                    message: format!("failed to read chat session header {}: {e}", path.display()),
                })?;
        let mut header: ChatSessionHeader =
            serde_json::from_str(&content).map_err(|e| rua_core::RuaError::Io {
                message: format!(
                    "failed to parse chat session header {}: {e}",
                    path.display()
                ),
            })?;
        header.extra.remove("projection_cache");
        if header.extra.remove("runtime_notes").is_some() {
            tracing::warn!(
                chat_id,
                "discarded unsupported legacy Session-header Runtime Notes during pre-release migration"
            );
        }
        header.extra.remove("hook_configuration");
        if let Some(epoch) = self.load_configuration_head(chat_id).await? {
            let configuration = self
                .load_configuration_snapshot(chat_id, &epoch)
                .await?
                .ok_or_else(|| rua_core::RuaError::Validation {
                    message: format!("Chat configuration snapshot {epoch} is unavailable"),
                })?;
            for key in ["model", "thinking"] {
                match configuration.get(key).filter(|value| !value.is_null()) {
                    Some(value) => {
                        header.extra.insert(key.into(), value.clone());
                    }
                    None => {
                        header.extra.remove(key);
                    }
                }
            }
            header
                .extra
                .insert("hook_configuration".to_string(), configuration);
        }
        Ok(Some(header))
    }

    pub async fn save_chat_header(&self, header: &ChatSessionHeader) -> Result<()> {
        let head_lock = chat_write_lock_for_path(self.configuration_head_path(&header.chat_id));
        let _head_guard = head_lock.lock().await;
        let mut header = header.clone();
        if let Some(configuration) = header.extra.get("hook_configuration") {
            self.save_configuration_snapshot(&header.chat_id, configuration)
                .await?;
            if self
                .load_configuration_head(&header.chat_id)
                .await?
                .is_none()
            {
                let epoch = configuration["epoch"]
                    .as_str()
                    .expect("validated configuration epoch");
                write_json_atomic(
                    &self.configuration_head_path(&header.chat_id),
                    &serde_json::json!({"epoch": epoch}),
                )
                .await?;
            }
        }
        if let Some(epoch) = self.load_configuration_head(&header.chat_id).await? {
            let configuration = self
                .load_configuration_snapshot(&header.chat_id, &epoch)
                .await?
                .ok_or_else(|| rua_core::RuaError::Validation {
                    message: format!("Chat configuration snapshot {epoch} is unavailable"),
                })?;
            header
                .extra
                .insert("hook_configuration".to_string(), configuration);
        }
        let path = self.chat_header_path(&header.chat_id);
        write_json_atomic(&path, &header).await
    }

    /// Applies a replicated header without publishing or changing configuration heads.
    pub async fn merge_replicated_chat_header<F>(
        &self,
        chat_id: &str,
        mut incoming: Value,
        merge: F,
    ) -> Result<()>
    where
        F: FnOnce(Option<&Value>, &mut Value) -> Result<()>,
    {
        let write_lock = self.chat_write_lock(chat_id);
        let _write_guard = write_lock.lock().await;
        let head_lock = chat_write_lock_for_path(self.configuration_head_path(chat_id));
        let _head_guard = head_lock.lock().await;
        let path = self.chat_header_path(chat_id);
        let current = match tokio::fs::read(&path).await {
            Ok(bytes) => Some(serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                rua_core::RuaError::Validation {
                    message: format!("invalid local Chat header: {error}"),
                }
            })?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        merge(current.as_ref(), &mut incoming)?;
        write_json_atomic(&path, &incoming).await
    }

    pub fn configuration_head_path(&self, chat_id: &str) -> PathBuf {
        self.chat_dir(chat_id).join("configuration_head.json")
    }

    async fn load_configuration_head(&self, chat_id: &str) -> Result<Option<String>> {
        let bytes = match tokio::fs::read(self.configuration_head_path(chat_id)).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(rua_core::RuaError::Io {
                    message: format!("read Chat configuration head: {error}"),
                })
            }
        };
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|error| rua_core::RuaError::Validation {
                message: format!("invalid Chat configuration head: {error}"),
            })?;
        let epoch = value.get("epoch").and_then(Value::as_str).ok_or_else(|| {
            rua_core::RuaError::Validation {
                message: "Chat configuration head has no epoch".to_string(),
            }
        })?;
        self.configuration_snapshot_path(chat_id, epoch)?;
        Ok(Some(epoch.to_string()))
    }

    async fn publish_configuration_head(
        &self,
        chat_id: &str,
        expected_epoch: Option<&str>,
        configuration: &Value,
    ) -> Result<()> {
        let lock = chat_write_lock_for_path(self.configuration_head_path(chat_id));
        let _guard = lock.lock().await;
        let current = self.load_configuration_head(chat_id).await?;
        if current.as_deref() != expected_epoch {
            return Err(rua_core::RuaError::Validation {
                message: "Chat configuration changed; reload before updating".to_string(),
            });
        }
        self.save_configuration_snapshot(chat_id, configuration)
            .await?;
        write_json_atomic(
            &self.configuration_head_path(chat_id),
            &serde_json::json!({"epoch": configuration["epoch"]}),
        )
        .await
    }

    pub fn configuration_snapshot_path(&self, chat_id: &str, epoch: &str) -> Result<PathBuf> {
        if epoch.is_empty()
            || epoch.len() > 128
            || !epoch
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(rua_core::RuaError::Validation {
                message: "invalid Chat configuration epoch".to_string(),
            });
        }
        Ok(self
            .chat_dir(chat_id)
            .join("configurations")
            .join(format!("{epoch}.json")))
    }

    pub async fn load_configuration_snapshot(
        &self,
        chat_id: &str,
        epoch: &str,
    ) -> Result<Option<Value>> {
        let path = self.configuration_snapshot_path(chat_id, epoch)?;
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(rua_core::RuaError::Io {
                    message: format!("read Chat configuration snapshot: {error}"),
                })
            }
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| rua_core::RuaError::Validation {
                message: format!("invalid Chat configuration snapshot: {error}"),
            })
    }

    async fn save_configuration_snapshot(
        &self,
        chat_id: &str,
        configuration: &Value,
    ) -> Result<()> {
        let epoch = configuration
            .get("epoch")
            .and_then(Value::as_str)
            .ok_or_else(|| rua_core::RuaError::Validation {
                message: "Chat configuration epoch is required".to_string(),
            })?;
        let path = self.configuration_snapshot_path(chat_id, epoch)?;
        let lock = chat_write_lock_for_path(path.clone());
        let _guard = lock.lock().await;
        if let Some(existing) = self.load_configuration_snapshot(chat_id, epoch).await? {
            if existing != *configuration {
                return Err(rua_core::RuaError::Validation {
                    message: format!("Chat configuration epoch {epoch} is immutable"),
                });
            }
            return Ok(());
        }
        write_json_atomic(&path, configuration).await
    }

    pub async fn update_chat_header(
        &self,
        chat_id: &str,
        update: impl FnOnce(&mut ChatSessionHeader),
    ) -> Result<bool> {
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        let Some(mut header) = self.load_chat_header(chat_id).await? else {
            return Ok(false);
        };
        let previous = header.extra.get("hook_configuration").cloned();
        update(&mut header);
        if header.extra.get("hook_configuration") != previous.as_ref() {
            if let Some(configuration) = header.extra.get("hook_configuration") {
                let expected = self.load_configuration_head(chat_id).await?;
                self.publish_configuration_head(chat_id, expected.as_deref(), configuration)
                    .await?;
            }
        }
        self.save_chat_header(&header).await?;
        Ok(true)
    }

    pub async fn replace_chat_configuration(
        &self,
        chat_id: &str,
        expected_epoch: &str,
        configuration: serde_json::Value,
    ) -> Result<()> {
        let epoch = configuration
            .get("epoch")
            .and_then(serde_json::Value::as_str)
            .filter(|epoch| !epoch.is_empty())
            .ok_or_else(|| rua_core::RuaError::Validation {
                message: "Chat configuration epoch is required".to_string(),
            })?;
        if epoch == expected_epoch {
            return Err(rua_core::RuaError::Validation {
                message: "New Chat configuration must have a new epoch".to_string(),
            });
        }
        let lock = self.chat_write_lock(chat_id);
        let _guard = lock.lock().await;
        let mut header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::NotFound {
                    resource: "chat".to_string(),
                    id: chat_id.to_string(),
                })?;
        if header
            .extra
            .get("hook_configuration")
            .and_then(|value| value.get("epoch"))
            .and_then(serde_json::Value::as_str)
            != Some(expected_epoch)
        {
            return Err(rua_core::RuaError::Validation {
                message: "Chat configuration changed; reload before updating".to_string(),
            });
        }
        header
            .extra
            .insert("hook_configuration".to_string(), configuration);
        let expected = self.load_configuration_head(chat_id).await?;
        if expected
            .as_deref()
            .is_some_and(|epoch| epoch != expected_epoch)
        {
            return Err(rua_core::RuaError::Validation {
                message: "Chat configuration changed; reload before updating".to_string(),
            });
        }
        self.publish_configuration_head(
            chat_id,
            expected.as_deref(),
            &header.extra["hook_configuration"],
        )
        .await
    }

    pub async fn load_chat_running_config(
        &self,
        chat_id: &str,
    ) -> Result<Option<rua_config::ChatRunningConfig>> {
        if let Some(header) = self.load_chat_header(chat_id).await? {
            if let Some(configuration) = header.extra.get("hook_configuration") {
                let value = configuration.get("running_config").ok_or_else(|| {
                    rua_core::RuaError::Validation {
                        message: "Chat configuration snapshot has no running_config".to_string(),
                    }
                })?;
                let running: rua_config::ChatRunningConfig = serde_json::from_value(value.clone())
                    .map_err(|error| rua_core::RuaError::Validation {
                        message: format!("invalid Chat configuration snapshot: {error}"),
                    })?;
                running
                    .validate()
                    .map_err(|message| rua_core::RuaError::Validation { message })?;
                return Ok(Some(running));
            }
            return Err(rua_core::RuaError::Validation {
                message: "Chat configuration snapshot is unavailable".into(),
            });
        }
        Ok(None)
    }

    pub async fn update_chat_running_config(
        &self,
        chat_id: &str,
        running_config: rua_config::ChatRunningConfig,
    ) -> Result<bool> {
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        let Some(mut header) = self.load_chat_header(chat_id).await? else {
            return Ok(false);
        };
        running_config
            .validate()
            .map_err(|message| rua_core::RuaError::Validation { message })?;
        if let Some(configuration) = header.extra.get_mut("hook_configuration") {
            let expected = self.load_configuration_head(chat_id).await?;
            let configuration =
                configuration
                    .as_object_mut()
                    .ok_or_else(|| rua_core::RuaError::Validation {
                        message: "Chat configuration snapshot must be an object".to_string(),
                    })?;
            configuration.insert(
                "running_config".to_string(),
                serde_json::to_value(&running_config).map_err(|error| {
                    rua_core::RuaError::Validation {
                        message: error.to_string(),
                    }
                })?,
            );
            configuration.insert(
                "epoch".to_string(),
                serde_json::json!(format!("hook-config-{}", uuid::Uuid::new_v4())),
            );
            self.publish_configuration_head(
                chat_id,
                expected.as_deref(),
                &header.extra["hook_configuration"],
            )
            .await?;
            return Ok(true);
        }
        Err(rua_core::RuaError::Validation {
            message: "Chat configuration snapshot is unavailable".into(),
        })
    }

    pub async fn clear_active_summary_checkpoint(&self, chat_id: &str) -> Result<bool> {
        let Some(mut header) = self.load_chat_header(chat_id).await? else {
            return Ok(false);
        };
        header.active_checkpoint_id = None;
        self.save_chat_header(&header).await?;
        Ok(true)
    }

    pub async fn record_clear_projection_boundary(&self, chat_id: &str) -> Result<bool> {
        let Some(mut header) = self.load_chat_header(chat_id).await? else {
            return Ok(false);
        };
        let index = self.load_turn_index(chat_id).await?;
        let active_latest_sequence = active_message_log_end_sequence(&header, &index);
        let active_latest_turn = index
            .turns
            .last()
            .map(|turn| turn.ordinal)
            .unwrap_or(header.turn_index.latest_turn);
        header.active_checkpoint_id = None;
        header.projection_boundary = Some(ProjectionBoundary {
            kind: "clear".to_string(),
            sequence_start: active_latest_sequence.saturating_add(1).max(1),
            turn_start_ordinal: active_latest_turn.saturating_add(1).max(1),
            created_at: chrono::Utc::now().to_rfc3339(),
        });
        self.save_chat_header(&header).await?;
        Ok(true)
    }

    pub async fn publish_summary_checkpoint(
        &self,
        chat_id: &str,
        expected_active_checkpoint_id: Option<&str>,
        draft: SummaryCheckpointDraft,
    ) -> Result<SummaryCheckpoint> {
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        let resolved_previous_checkpoint_id = self
            .load_active_summary_checkpoint(chat_id)
            .await?
            .map(|checkpoint| checkpoint.checkpoint_id);
        if draft.previous_checkpoint_id != resolved_previous_checkpoint_id {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "checkpoint {} previous pointer does not match compatible projection base",
                    draft.checkpoint_id
                ),
            });
        }

        let mut header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        if !draft_respects_projection_boundary(&draft, header.projection_boundary.as_ref()) {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "summary checkpoint {} crosses active projection boundary for {chat_id}",
                    draft.checkpoint_id
                ),
            });
        }
        self.validate_checkpoint_draft(chat_id, &draft).await?;

        let covered_selected_variants = self
            .covered_selected_variants_through_sequence(chat_id, draft.frontier.end_sequence)
            .await?;

        let covered_excluded_message_ids = self
            .projection_excluded_ids_through_sequence(chat_id, draft.frontier.end_sequence)
            .await?;
        let mut checkpoint = SummaryCheckpoint {
            checkpoint_id: draft.checkpoint_id,
            chat_id: header.chat_id.clone(),
            agent_id: header.agent_id.clone(),
            previous_checkpoint_id: draft.previous_checkpoint_id,
            covered_turn_range: draft.covered_turn_range,
            covered_sequence_range: draft.covered_sequence_range,
            source_sequence_range: draft.source_sequence_range,
            frontier: draft.frontier,
            retained_sequence_ranges: draft.retained_sequence_ranges,
            projection_lineage: draft.projection_lineage,
            covered_selected_variants,
            covered_excluded_message_ids,
            summary: draft.summary,
            token_estimate: draft.token_estimate,
            model: draft.model,
            created_at: draft.created_at,
            checksum: String::new(),
        };
        checkpoint.checksum = checkpoint_checksum(&checkpoint)?;

        self.save_summary_checkpoint(&checkpoint).await?;
        let Some(validated) = self
            .read_summary_checkpoint(chat_id, &checkpoint.checkpoint_id)
            .await?
        else {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "summary checkpoint {} failed validation after write",
                    checkpoint.checkpoint_id
                ),
            });
        };

        header = self
            .load_chat_header(chat_id)
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("chat session header not found for {chat_id}"),
            })?;
        if header.active_checkpoint_id.as_deref() != expected_active_checkpoint_id {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "summary checkpoint publish CAS mismatch for {chat_id}: expected {:?}, found {:?}",
                    expected_active_checkpoint_id, header.active_checkpoint_id
                ),
            });
        }
        header.active_checkpoint_id = Some(validated.checkpoint_id.clone());
        self.save_chat_header(&header).await?;

        Ok(validated)
    }

    pub async fn publish_manual_context_checkpoint(
        &self,
        chat_id: &str,
        summary: String,
        model: CheckpointModelIdentity,
    ) -> Result<SummaryCheckpoint> {
        early_background_compaction_coordinator()
            .preempt_for_foreground(&early_background_compaction_key(self, chat_id));
        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let previous = self.load_active_summary_checkpoint(chat_id).await?;
        let previous_checkpoint_id = previous
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_id.clone());
        let expected_active_checkpoint_id = header.active_checkpoint_id.clone();
        let boundary_sequence_start = projection_boundary_sequence_start(&header);
        let boundary_turn_start = projection_boundary_turn_start(&header);
        let tail_start_sequence = previous
            .as_ref()
            .map(|checkpoint| checkpoint.frontier.end_sequence + 1)
            .unwrap_or(boundary_sequence_start);
        let message_groups = self
            .projection_message_group_estimates(chat_id)
            .await?
            .into_iter()
            .filter(|group| group.last_sequence >= tail_start_sequence)
            .collect::<Vec<_>>();
        let frontier =
            ProjectionPlanner::widest_fold_frontier(&message_groups).ok_or_else(|| {
                rua_core::RuaError::Io {
                    message: format!(
                        "cannot compact chat {chat_id} without a complete message boundary"
                    ),
                }
            })?;
        let covered_turn_start = previous
            .as_ref()
            .map(|checkpoint| checkpoint.covered_turn_range.start_turn_ordinal)
            .unwrap_or(boundary_turn_start);
        let covered_sequence_start = previous
            .as_ref()
            .map(|checkpoint| checkpoint.covered_sequence_range.start_sequence)
            .unwrap_or(boundary_sequence_start);
        let source_sequence_start = tail_start_sequence;
        let projection_state = self
            .checkpoint_projection_state(chat_id, frontier.end_sequence)
            .await?;

        self.publish_summary_checkpoint(
            chat_id,
            expected_active_checkpoint_id.as_deref(),
            SummaryCheckpointDraft {
                checkpoint_id: uuid::Uuid::new_v4().to_string(),
                previous_checkpoint_id,
                covered_turn_range: CheckpointTurnRange {
                    start_turn_ordinal: covered_turn_start,
                    end_turn_ordinal: frontier.end_turn_ordinal,
                },
                covered_sequence_range: CheckpointSequenceRange {
                    start_sequence: covered_sequence_start,
                    end_sequence: frontier.end_sequence,
                },
                source_sequence_range: CheckpointSequenceRange {
                    start_sequence: source_sequence_start,
                    end_sequence: frontier.end_sequence,
                },
                frontier: projection_state.frontier,
                retained_sequence_ranges: projection_state.retained_sequence_ranges,
                projection_lineage: projection_state.projection_lineage,
                token_estimate: (summary.len() as u64 / 4).max(1),
                summary,
                model,
                created_at: chrono::Utc::now().to_rfc3339(),
            },
        )
        .await
    }

    pub async fn load_active_summary_checkpoint(
        &self,
        chat_id: &str,
    ) -> Result<Option<SummaryCheckpoint>> {
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Ok(None);
        };
        let boundary = header.projection_boundary.clone();
        let mut checkpoint_id = header.active_checkpoint_id.clone();
        let mut visited = Vec::new();
        while let Some(current_id) = checkpoint_id {
            if visited.iter().any(|seen| seen == &current_id) {
                return Ok(None);
            }
            visited.push(current_id.clone());
            let Some(raw) = self
                .read_summary_checkpoint_raw(chat_id, &current_id)
                .await?
            else {
                return Ok(None);
            };
            if raw.chat_id != chat_id {
                return Ok(None);
            }

            let selection_valid = if raw.covered_selected_variants.is_empty() {
                self.projection_lineage_through_sequence(chat_id, raw.frontier.end_sequence)
                    .await?
                    == raw.projection_lineage
            } else {
                let index = self.load_turn_index(chat_id).await?;
                let current = Self::covered_selected_variants_for_index(
                    &header,
                    &index,
                    raw.frontier.end_sequence,
                );
                // A checkpoint ending at the user input does not cover this Turn's
                // response. Completing /compact (or choosing a later response) must
                // not invalidate the summary of the unchanged preceding history.
                let user_frontier_turn = index
                    .turns
                    .iter()
                    .find(|turn| turn.user_message_sequence == raw.frontier.end_sequence)
                    .map(|turn| turn.turn_id.as_str());
                current.len() == raw.covered_selected_variants.len()
                    && current.iter().zip(&raw.covered_selected_variants).all(
                        |(current, covered)| {
                            current.turn_id == covered.turn_id
                                && (current.selected_variant_id == covered.selected_variant_id
                                    || user_frontier_turn == Some(current.turn_id.as_str()))
                        },
                    )
            };
            if raw.checksum == checkpoint_checksum(&raw)?
                && checkpoint_respects_projection_boundary(&raw, boundary.as_ref())
                && selection_valid
                && self
                    .projection_excluded_ids_through_sequence(chat_id, raw.frontier.end_sequence)
                    .await?
                    == raw.covered_excluded_message_ids
            {
                return Ok(Some(raw));
            }
            checkpoint_id = raw.previous_checkpoint_id.clone();
        }
        Ok(None)
    }

    pub async fn inherit_compatible_summary_checkpoint(
        &self,
        source_chat_id: &str,
        target_chat_id: &str,
    ) -> Result<Option<SummaryCheckpoint>> {
        let Some(source_header) = self.load_chat_header(source_chat_id).await? else {
            return Ok(None);
        };
        let Some(target_header) = self.load_chat_header(target_chat_id).await? else {
            return Err(rua_core::RuaError::Io {
                message: format!("chat session header not found for {target_chat_id}"),
            });
        };
        if target_header.active_checkpoint_id.is_some() {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "cannot inherit a summary checkpoint into {target_chat_id} after its active pointer is set"
                ),
            });
        }
        let target_messages = self
            .read_projectable_message_range(
                target_chat_id,
                1,
                target_header.message_log.latest_sequence,
            )
            .await?;
        let target_index = self.load_turn_index(target_chat_id).await?;
        let mut source_projection_index = self.load_turn_index(source_chat_id).await?;
        for target_turn in &target_index.turns {
            let Some(source_turn) = source_projection_index
                .turns
                .iter_mut()
                .find(|turn| turn.turn_id == target_turn.turn_id)
            else {
                return Ok(None);
            };
            if let Some(selected_variant_id) = target_turn.selected_variant_id.as_ref() {
                if source_turn
                    .effective_assistant_response_variants()
                    .iter()
                    .all(|variant| variant.variant_id != *selected_variant_id)
                {
                    return Ok(None);
                }
                source_turn.selected_variant_id = Some(selected_variant_id.clone());
            } else {
                source_turn.selected_variant_id = None;
            }
        }

        let mut checkpoint_id = source_header.active_checkpoint_id.clone();
        let mut visited = Vec::new();
        while let Some(current_id) = checkpoint_id {
            if visited.iter().any(|seen| seen == &current_id) {
                return Ok(None);
            }
            visited.push(current_id.clone());
            let Some(source) = self
                .read_summary_checkpoint_raw(source_chat_id, &current_id)
                .await?
            else {
                return Ok(None);
            };
            checkpoint_id = source.previous_checkpoint_id.clone();
            if source.chat_id != source_chat_id || source.checksum != checkpoint_checksum(&source)?
            {
                continue;
            }
            if Self::projection_lineage_for_index(
                source_chat_id,
                &source_header,
                &source_projection_index,
                source.frontier.end_sequence,
                &source.covered_excluded_message_ids,
            )? != source.projection_lineage
            {
                continue;
            }

            let source_messages = self
                .read_active_messages_for_index(
                    source_chat_id,
                    &source_header,
                    &source_projection_index,
                    source.covered_sequence_range.start_sequence,
                )
                .await?
                .into_iter()
                .filter(|message| message.sequence <= source.frontier.end_sequence)
                .collect::<Vec<_>>();
            if source_messages.is_empty() || source_messages.len() > target_messages.len() {
                continue;
            }
            let Some(target_start_index) =
                target_messages
                    .windows(source_messages.len())
                    .position(|candidate| {
                        candidate
                            .iter()
                            .zip(source_messages.iter())
                            .all(|(target, source)| {
                                target.message_id == source.message_id
                                    && target.role == source.role
                                    && target.content == source.content
                            })
                    })
            else {
                continue;
            };
            let target_slice =
                &target_messages[target_start_index..target_start_index + source_messages.len()];
            let target_start_sequence = target_slice
                .first()
                .map(|message| message.sequence)
                .unwrap_or_default();
            let target_end_sequence = target_slice
                .last()
                .map(|message| message.sequence)
                .unwrap_or_default();
            if self
                .projection_excluded_ids_through_sequence(target_chat_id, target_end_sequence)
                .await?
                != source.covered_excluded_message_ids
            {
                continue;
            }
            let Ok(target_state) = self
                .checkpoint_projection_state(target_chat_id, target_end_sequence)
                .await
            else {
                continue;
            };
            let Some(target_start_turn) = target_index.turns.iter().find(|turn| {
                turn.first_sequence <= target_start_sequence
                    && turn.last_sequence >= target_start_sequence
            }) else {
                continue;
            };
            let Some(target_end_turn) = target_index.turns.iter().find(|turn| {
                turn.first_sequence <= target_end_sequence
                    && turn.last_sequence >= target_end_sequence
            }) else {
                continue;
            };

            let mut mapped_retained_ranges = Vec::new();
            let mut retained_compatible = true;
            for retained in &source.retained_sequence_ranges {
                for retained_message in source_messages.iter().filter(|message| {
                    message.sequence >= retained.start_sequence
                        && message.sequence <= retained.end_sequence
                }) {
                    let Some(target_message) = target_slice
                        .iter()
                        .find(|target| target.message_id == retained_message.message_id)
                    else {
                        retained_compatible = false;
                        break;
                    };
                    mapped_retained_ranges.push(CheckpointRetainedSequenceRange {
                        start_sequence: target_message.sequence,
                        end_sequence: target_message.sequence,
                        reason: retained.reason.clone(),
                    });
                }
                if !retained_compatible {
                    break;
                }
            }
            if !retained_compatible {
                continue;
            }
            mapped_retained_ranges.sort_by_key(|range| range.start_sequence);
            let mut merged_retained_ranges: Vec<CheckpointRetainedSequenceRange> = Vec::new();
            for retained in mapped_retained_ranges {
                if let Some(previous) = merged_retained_ranges.last_mut() {
                    if previous.reason == retained.reason
                        && previous.end_sequence.saturating_add(1) == retained.start_sequence
                    {
                        previous.end_sequence = retained.end_sequence;
                        continue;
                    }
                }
                merged_retained_ranges.push(retained);
            }

            return self
                .publish_summary_checkpoint(
                    target_chat_id,
                    None,
                    SummaryCheckpointDraft {
                        checkpoint_id: uuid::Uuid::new_v4().to_string(),
                        previous_checkpoint_id: None,
                        covered_turn_range: CheckpointTurnRange {
                            start_turn_ordinal: target_start_turn.ordinal,
                            end_turn_ordinal: target_end_turn.ordinal,
                        },
                        covered_sequence_range: CheckpointSequenceRange {
                            start_sequence: target_start_sequence,
                            end_sequence: target_end_sequence,
                        },
                        source_sequence_range: CheckpointSequenceRange {
                            start_sequence: target_start_sequence,
                            end_sequence: target_end_sequence,
                        },
                        frontier: target_state.frontier,
                        retained_sequence_ranges: merged_retained_ranges,
                        projection_lineage: target_state.projection_lineage,
                        summary: source.summary,
                        token_estimate: source.token_estimate,
                        model: source.model,
                        created_at: chrono::Utc::now().to_rfc3339(),
                    },
                )
                .await
                .map(Some);
        }

        Ok(None)
    }

    pub async fn build_context_projection(
        &self,
        chat_id: &str,
        request: ContextProjectionRequest,
    ) -> Result<ContextProjection> {
        self.build_context_projection_internal(chat_id, request, true)
            .await
            .map(|(projection, _)| projection)
    }

    async fn build_context_projection_internal(
        &self,
        chat_id: &str,
        request: ContextProjectionRequest,
        estimate_tokens: bool,
    ) -> Result<(ContextProjection, u64)> {
        let started = Instant::now();
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Err(rua_core::RuaError::Io {
                message: format!("chat session header not found for {chat_id}"),
            });
        };
        let index = self.load_turn_index(chat_id).await?;
        let active_latest_sequence = active_message_log_end_sequence(&header, &index);
        let projection_exclusions = self.load_projection_exclusions(chat_id).await?;
        let checkpoint = self.load_active_summary_checkpoint(chat_id).await?;
        let boundary_sequence_start = projection_boundary_sequence_start(&header);
        let tail_start_sequence = checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.frontier.end_sequence + 1)
            .unwrap_or(boundary_sequence_start)
            .max(boundary_sequence_start);
        let active_checkpoint_id = checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_id.clone());
        let summary = checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.summary.clone());
        let summary_model = checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.model.clone());
        let tail_start =
            (tail_start_sequence <= active_latest_sequence).then_some(tail_start_sequence);

        let mut retained_messages: Vec<MessageLogEntry> =
            if let Some(checkpoint) = checkpoint.as_ref() {
                self.read_retained_checkpoint_messages(chat_id, checkpoint)
                    .await?
            } else {
                Vec::new()
            }
            .into_iter()
            .filter(message_is_model_projectable)
            .collect();
        let tail_read_started = Instant::now();
        let mut tail_messages = if tail_start.is_some() {
            self.read_active_messages_for_index(chat_id, &header, &index, tail_start_sequence)
                .await?
        } else {
            Vec::new()
        };
        projection_exclusions.filter_messages(&mut retained_messages);
        projection_exclusions.filter_messages(&mut tail_messages);
        let tail_read_elapsed = tail_read_started.elapsed();
        let tail_end_sequence = tail_messages.last().map(|message| message.sequence);
        let checkpoint_token_estimate = checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.token_estimate)
            .unwrap_or_default();
        let stable_token_estimate = if estimate_tokens {
            checkpoint_token_estimate
                .saturating_add(estimate_message_tokens(&retained_messages))
                .saturating_add(estimate_message_tokens(&tail_messages))
        } else {
            checkpoint_token_estimate
        };

        debug!(
            chat_id = %header.chat_id,
            agent_id = %header.agent_id,
            active_checkpoint_id = ?active_checkpoint_id,
            retained_message_count = retained_messages.len(),
            tail_start_sequence = ?tail_messages.first().map(|message| message.sequence),
            tail_end_sequence = ?tail_end_sequence,
            tail_message_count = tail_messages.len(),
            stable_token_estimate,
            tail_read_elapsed_ms = tail_read_elapsed.as_millis(),
            elapsed_ms = started.elapsed().as_millis(),
            "built evergreen context projection"
        );

        Ok((
            ContextProjection {
                chat_id: header.chat_id,
                agent_id: header.agent_id,
                active_checkpoint_id,
                summary,
                summary_model,
                tail_start_sequence: tail_messages.first().map(|message| message.sequence),
                tail_end_sequence,
                retained_messages,
                tail_messages,
                current_user_input: request.current_user_input,
                stable_token_estimate,
                history_replacement_evidence: Vec::new(),
            },
            checkpoint_token_estimate,
        ))
    }

    pub async fn build_context_projection_with_history_replacements(
        &self,
        chat_id: &str,
        request: ContextProjectionRequest,
        token_estimate_divisor: f64,
    ) -> Result<ContextProjection> {
        self.build_effective_context_projection(chat_id, request, true, token_estimate_divisor)
            .await
    }

    pub async fn build_effective_context_projection(
        &self,
        chat_id: &str,
        request: ContextProjectionRequest,
        history_replacement_enabled: bool,
        token_estimate_divisor: f64,
    ) -> Result<ContextProjection> {
        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let chat_address = ChatAddress::agent(header.agent_id, chat_id);
        self.build_effective_context_projection_for_address(
            &chat_address,
            request,
            history_replacement_enabled,
            token_estimate_divisor,
        )
        .await
    }

    pub async fn build_effective_context_projection_for_address(
        &self,
        chat_address: &ChatAddress,
        request: ContextProjectionRequest,
        history_replacement_enabled: bool,
        token_estimate_divisor: f64,
    ) -> Result<ContextProjection> {
        let chat_id = chat_address.chat_id().as_str();
        let cacheable = request.current_user_input.is_none();
        let cache_key = if cacheable {
            Some(
                self.effective_projection_cache_key(
                    chat_address,
                    history_replacement_enabled,
                    token_estimate_divisor,
                )
                .await?,
            )
        } else {
            None
        };
        if let Some(key) = cache_key.as_ref() {
            if let Some(entry) = self
                .effective_projection_cache
                .lock()
                .await
                .get(chat_address)
                .filter(|entry| entry.key == *key)
            {
                return Ok(entry.projection.clone());
            }
        }
        let (mut projection, checkpoint_tokens) = self
            .build_context_projection_internal(chat_id, request, false)
            .await?;
        if history_replacement_enabled {
            self.apply_history_replacements_for_address(
                chat_address,
                &mut projection,
                token_estimate_divisor,
            )
            .await?;
        }
        projection.stable_token_estimate = checkpoint_tokens
            .saturating_add(estimate_message_tokens_with_divisor(
                &projection.retained_messages,
                token_estimate_divisor,
            ))
            .saturating_add(estimate_message_tokens_with_divisor(
                &projection.tail_messages,
                token_estimate_divisor,
            ));
        if let Some(key) = cache_key {
            self.effective_projection_cache.lock().await.insert(
                chat_address.clone(),
                EffectiveProjectionCacheEntry {
                    key,
                    projection: projection.clone(),
                },
            );
        }
        Ok(projection)
    }

    async fn effective_projection_cache_key(
        &self,
        chat_address: &ChatAddress,
        history_replacement_enabled: bool,
        token_estimate_divisor: f64,
    ) -> Result<EffectiveProjectionCacheKey> {
        let chat_id = chat_address.chat_id().as_str();
        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let turn_index = self.load_turn_index(chat_id).await?;
        let projection_exclusions = self.load_projection_exclusions(chat_id).await?;
        let history_replacement_files = if history_replacement_enabled {
            self.history_replacement_manifest_files(
                &self.chat_dir(chat_id).join("history_replacements"),
            )
            .await?
        } else {
            Vec::new()
        };
        Ok(EffectiveProjectionCacheKey {
            chat_address: chat_address.clone(),
            header,
            turn_index,
            projection_exclusions,
            history_replacement_files,
            history_replacement_enabled,
            token_estimate_divisor,
        })
    }

    pub async fn apply_history_replacements(
        &self,
        chat_id: &str,
        projection: &mut ContextProjection,
        token_estimate_divisor: f64,
    ) -> Result<usize> {
        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let chat_address = ChatAddress::agent(header.agent_id, chat_id);
        self.apply_history_replacements_for_address(
            &chat_address,
            projection,
            token_estimate_divisor,
        )
        .await
    }

    pub async fn apply_history_replacements_for_address(
        &self,
        chat_address: &ChatAddress,
        projection: &mut ContextProjection,
        token_estimate_divisor: f64,
    ) -> Result<usize> {
        let records = self
            .load_ready_history_replacements(chat_address, token_estimate_divisor)
            .await?;
        let retained_reports = apply_ready_history_replacements_with_evidence(
            &mut projection.retained_messages,
            &records,
        );
        let tail_reports =
            apply_ready_history_replacements_with_evidence(&mut projection.tail_messages, &records);
        let mut applied = 0;
        for ((record, retained), tail) in records.iter().zip(retained_reports).zip(tail_reports) {
            let mut reports = retained.into_iter().chain(tail).collect::<Vec<_>>();
            if reports.is_empty() {
                projection.history_replacement_evidence.push(
                    crate::history_replacement::HistoryReplacementApplicationEvidence {
                        record_id: record.record_id.clone(),
                        status: "skipped".to_string(),
                        reason: Some("source anchors not present in projection".to_string()),
                        mutations: Vec::new(),
                        digest_carrier: None,
                        removed_message_ids: Vec::new(),
                    },
                );
                continue;
            }
            let mut report = reports.remove(0);
            for additional in reports {
                report.mutations.extend(additional.mutations);
                report
                    .removed_message_ids
                    .extend(additional.removed_message_ids);
                if report.digest_carrier.is_none() {
                    report.digest_carrier = additional.digest_carrier;
                }
            }
            projection.history_replacement_evidence.push(report);
            applied += 1;
        }
        Ok(applied)
    }

    pub async fn apply_history_replacements_to_messages(
        &self,
        chat_id: &str,
        messages: &mut Vec<MessageLogEntry>,
        token_estimate_divisor: f64,
    ) -> Result<usize> {
        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let chat_address = ChatAddress::agent(header.agent_id, chat_id);
        self.apply_history_replacements_to_messages_for_address(
            &chat_address,
            messages,
            token_estimate_divisor,
        )
        .await
    }

    pub async fn apply_history_replacements_to_messages_for_address(
        &self,
        chat_address: &ChatAddress,
        messages: &mut Vec<MessageLogEntry>,
        token_estimate_divisor: f64,
    ) -> Result<usize> {
        let records = self
            .load_ready_history_replacements(chat_address, token_estimate_divisor)
            .await?;
        Ok(apply_history_replacement_records(messages, &records))
    }

    async fn load_ready_history_replacements(
        &self,
        chat_address: &ChatAddress,
        token_estimate_divisor: f64,
    ) -> Result<Vec<HistoryReplacementRecord>> {
        let _ = token_estimate_divisor;
        let mut records = self
            .list_history_replacement_records_for_address(chat_address)
            .await?;
        let exclusions = self
            .load_projection_exclusions(chat_address.chat_id().as_str())
            .await?;
        let excluded = exclusions.message_ids.iter().collect::<HashSet<_>>();
        records.retain(|record| {
            record.ready_digest().is_some()
                && !record
                    .source
                    .message_ids
                    .iter()
                    .any(|id| excluded.contains(id))
        });

        records.sort_by_key(|record| {
            std::cmp::Reverse((
                record.source.sequence_range.start_sequence,
                record.source.start_anchor.content_part_index,
            ))
        });
        Ok(records)
    }

    pub async fn detect_history_replacement_blocks(
        &self,
        chat_id: &str,
        token_estimate_divisor: f64,
    ) -> Result<Vec<ExecutionTraceBlock>> {
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Ok(Vec::new());
        };
        let index = self.load_turn_index(chat_id).await?;
        let boundary_sequence_start = projection_boundary_sequence_start(&header);
        let mut blocks = Vec::new();
        for turn in index.turns.iter().filter(|turn| {
            !turn_origin_is_acp(turn)
                && !turn
                    .metadata
                    .get("ui_only")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                && turn.last_sequence >= boundary_sequence_start
        }) {
            let messages = self
                .read_projectable_message_range(
                    chat_id,
                    turn.first_sequence.max(boundary_sequence_start),
                    turn.last_sequence,
                )
                .await?;
            let variant_terminal = turn
                .selected_assistant_response_variant()
                .map(|variant| {
                    TurnLifecycleStatus::parse(&variant.status).is_syncable_conversation_state()
                })
                .unwrap_or_else(|| {
                    TurnLifecycleStatus::parse(&turn.lifecycle.status)
                        .is_syncable_conversation_state()
                });
            for mut block in detect_execution_trace_blocks(
                chat_id,
                &turn.turn_id,
                &messages,
                String::new(),
                token_estimate_divisor,
                variant_terminal,
            ) {
                if block.source.token_estimate < HISTORY_REPLACEMENT_MIN_SOURCE_TOKENS {
                    continue;
                }
                block.source.projection_lineage = self
                    .projection_lineage_through_sequence(
                        chat_id,
                        block.source.sequence_range.end_sequence,
                    )
                    .await?;
                block.record_id = derive_history_replacement_record_id(chat_id, &block.source);
                blocks.push(block);
            }
        }
        Ok(blocks)
    }

    pub async fn load_history_replacement(
        &self,
        chat_id: &str,
        record_id: &str,
    ) -> Result<Option<HistoryReplacementRecord>> {
        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let expected_address = ChatAddress::agent(header.agent_id, chat_id);
        self.load_history_replacement_for_address(&expected_address, record_id)
            .await
    }

    pub async fn load_history_replacement_for_address(
        &self,
        chat_address: &ChatAddress,
        record_id: &str,
    ) -> Result<Option<HistoryReplacementRecord>> {
        let chat_id = chat_address.chat_id().as_str();
        let path = self.history_replacement_path(chat_id, record_id);
        if !path.exists() {
            return Ok(None);
        }
        let content =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|error| rua_core::RuaError::Io {
                    message: format!(
                        "failed to read history replacement {}: {error}",
                        path.display()
                    ),
                })?;
        let record: HistoryReplacementRecord =
            serde_json::from_str(&content).map_err(|error| rua_core::RuaError::Io {
                message: format!(
                    "failed to parse history replacement {}: {error}",
                    path.display()
                ),
            })?;
        validate_history_replacement_record(chat_address, record_id, &record).map_err(|error| {
            rua_core::RuaError::Io {
                message: format!("invalid history replacement at {}: {error}", path.display()),
            }
        })?;
        Ok(Some(record))
    }

    pub async fn list_history_replacement_records(
        &self,
        chat_id: &str,
    ) -> Result<Vec<HistoryReplacementRecord>> {
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Ok(Vec::new());
        };
        let chat_address = ChatAddress::agent(header.agent_id, chat_id);
        self.list_history_replacement_records_for_address(&chat_address)
            .await
    }

    pub async fn list_history_replacement_records_for_address(
        &self,
        chat_address: &ChatAddress,
    ) -> Result<Vec<HistoryReplacementRecord>> {
        let chat_id = chat_address.chat_id().as_str();
        let dir = self.chat_dir(chat_id).join("history_replacements");
        let files = self.history_replacement_manifest_files(&dir).await?;
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let manifest_path = self.history_replacement_manifest_path(chat_id);
        if let Ok(bytes) = tokio::fs::read(&manifest_path).await {
            if let Ok(manifest) = serde_json::from_slice::<HistoryReplacementManifest>(&bytes) {
                if manifest.version == HISTORY_REPLACEMENT_MANIFEST_VERSION
                    && manifest.chat_address == *chat_address
                    && manifest.files == files
                    && manifest.records.len() == manifest.files.len()
                    && manifest.records.iter().all(|record| {
                        validate_history_replacement_record(chat_address, &record.record_id, record)
                            .is_ok()
                            && record.chat_address == *chat_address
                    })
                    && manifest.files.iter().all(|file| {
                        manifest
                            .records
                            .iter()
                            .any(|record| record.record_id == file.record_id)
                    })
                {
                    return Ok(manifest.records);
                }
            }
        }

        let mut records = Vec::with_capacity(files.len());
        for file in &files {
            match self
                .load_history_replacement_for_address(chat_address, &file.record_id)
                .await
            {
                Ok(Some(record)) => records.push(record),
                Ok(None) => {}
                Err(error) => {
                    warn!(
                        %error,
                        chat_id,
                        record_id = file.record_id,
                        "ignoring invalid history replacement while listing records"
                    );
                }
            }
        }
        let manifest = HistoryReplacementManifest {
            version: HISTORY_REPLACEMENT_MANIFEST_VERSION,
            chat_address: chat_address.clone(),
            files,
            records: records.clone(),
        };
        if let Err(error) = write_json_atomic(&manifest_path, &manifest).await {
            warn!(
                %error,
                chat_id,
                "failed to refresh disposable history replacement manifest"
            );
        }
        Ok(records)
    }

    async fn history_replacement_manifest_files(
        &self,
        dir: &Path,
    ) -> Result<Vec<HistoryReplacementManifestFile>> {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(rua_core::RuaError::Io {
                    message: format!(
                        "failed to list history replacements {}: {error}",
                        dir.display()
                    ),
                });
            }
        };
        let mut files = Vec::new();
        while let Some(entry) =
            entries
                .next_entry()
                .await
                .map_err(|error| rua_core::RuaError::Io {
                    message: format!(
                        "failed to read history replacement directory {}: {error}",
                        dir.display()
                    ),
                })?
        {
            let path = entry.path();
            if !entry
                .file_type()
                .await
                .map(|file_type| file_type.is_file())
                .unwrap_or(false)
                || path.extension().and_then(|extension| extension.to_str()) != Some("json")
            {
                continue;
            }
            if let Ok(bytes) = tokio::fs::read(&path).await {
                if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
                    if crate::history_replacement_version_is_superseded(&value) {
                        continue;
                    }
                }
            }
            let Some(record_id) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            let metadata = entry
                .metadata()
                .await
                .map_err(|error| rua_core::RuaError::Io {
                    message: format!(
                        "failed to inspect history replacement {}: {error}",
                        path.display()
                    ),
                })?;
            let modified_nanos = metadata
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            files.push(HistoryReplacementManifestFile {
                record_id: record_id.to_string(),
                len: metadata.len(),
                modified_nanos,
            });
        }
        files.sort_by(|left, right| left.record_id.cmp(&right.record_id));
        Ok(files)
    }

    pub fn history_replacement_manifest_path(&self, chat_id: &str) -> PathBuf {
        self.chat_dir(chat_id)
            .join("projection_cache")
            .join("history_replacements_manifest.json")
    }

    pub async fn inherit_compatible_history_replacements(
        &self,
        source_chat_address: &ChatAddress,
        target_chat_address: &ChatAddress,
    ) -> Result<usize> {
        let source_chat_id = source_chat_address.chat_id().as_str();
        let target_chat_id = target_chat_address.chat_id().as_str();
        let Some(target_header) = self.load_chat_header(target_chat_id).await? else {
            return Err(rua_core::RuaError::Io {
                message: format!("chat session header not found for {target_chat_id}"),
            });
        };
        let source_records = self
            .list_history_replacement_records_for_address(source_chat_address)
            .await?;
        if source_records.is_empty() {
            return Ok(0);
        }
        let target_messages = self
            .read_projectable_message_range(
                target_chat_id,
                1,
                target_header.message_log.latest_sequence,
            )
            .await?;
        let mut inherited = 0usize;

        for source_record in source_records {
            let source_messages = self
                .read_message_range(
                    source_chat_id,
                    source_record.source.sequence_range.start_sequence,
                    source_record
                        .source
                        .sequence_range
                        .end_sequence
                        .saturating_sub(source_record.source.sequence_range.start_sequence)
                        .saturating_add(1) as usize,
                )
                .await?;
            let mut mapped_messages = Vec::with_capacity(source_record.source.message_ids.len());
            let mut compatible = true;
            for message_id in &source_record.source.message_ids {
                let Some(source_message) = source_messages
                    .iter()
                    .find(|message| message.message_id == *message_id)
                else {
                    compatible = false;
                    break;
                };
                let Some(target_message) = target_messages
                    .iter()
                    .find(|message| message.message_id == *message_id)
                else {
                    compatible = false;
                    break;
                };
                if target_message.turn_id != source_record.source.turn_id
                    || target_message.role != source_message.role
                    || target_message.content != source_message.content
                {
                    compatible = false;
                    break;
                }
                mapped_messages.push(target_message.clone());
            }
            if !compatible
                || mapped_messages.is_empty()
                || mapped_messages
                    .windows(2)
                    .any(|messages| messages[0].sequence >= messages[1].sequence)
                || validate_history_replacement_source(&source_record.source, &mapped_messages)
                    .is_err()
            {
                continue;
            }
            let start_sequence = mapped_messages
                .first()
                .map(|message| message.sequence)
                .unwrap_or_default();
            let end_sequence = mapped_messages
                .last()
                .map(|message| message.sequence)
                .unwrap_or_default();
            if start_sequence == 0 || end_sequence < start_sequence {
                continue;
            }

            let mut record = source_record;
            record.chat_address = target_chat_address.clone();
            let sequence_mapping = source_messages
                .iter()
                .filter_map(|source_message| {
                    target_messages
                        .iter()
                        .find(|target_message| {
                            target_message.message_id == source_message.message_id
                        })
                        .map(|target_message| (source_message.sequence, target_message.sequence))
                })
                .collect::<HashMap<_, _>>();
            for tool_reference in &mut record.source.tool_references {
                let Some(call_sequence) =
                    sequence_mapping.get(&tool_reference.call_sequence).copied()
                else {
                    compatible = false;
                    break;
                };
                let Some(result_sequence) = sequence_mapping
                    .get(&tool_reference.result_sequence)
                    .copied()
                else {
                    compatible = false;
                    break;
                };
                tool_reference.call_sequence = call_sequence;
                tool_reference.result_sequence = result_sequence;
            }
            if !compatible {
                continue;
            }
            record.source.sequence_range = CheckpointSequenceRange {
                start_sequence,
                end_sequence,
            };
            record.source.projection_lineage = self
                .projection_lineage_through_sequence(target_chat_id, end_sequence)
                .await?;
            record.record_id = derive_history_replacement_record_id(target_chat_id, &record.source);
            record.checksum.clear();

            if self
                .load_history_replacement_for_address(target_chat_address, &record.record_id)
                .await?
                .is_some()
            {
                continue;
            }
            record.checksum = history_replacement_checksum(&record)?;
            write_json_atomic(
                &self.history_replacement_path(target_chat_id, &record.record_id),
                &record,
            )
            .await?;
            inherited = inherited.saturating_add(1);
        }
        Ok(inherited)
    }

    pub async fn prune_unreachable_history_replacements(
        &self,
        chat_id: &str,
    ) -> Result<Vec<String>> {
        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let chat_address = ChatAddress::agent(header.agent_id, chat_id);
        self.prune_unreachable_history_replacements_for_address(&chat_address)
            .await
    }

    pub async fn prune_unreachable_history_replacements_for_address(
        &self,
        chat_address: &ChatAddress,
    ) -> Result<Vec<String>> {
        let chat_id = chat_address.chat_id().as_str();
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        let index = self.load_turn_index(chat_id).await?;
        let records = self
            .list_history_replacement_records_for_address(chat_address)
            .await?;
        let mut deleted_relatives = Vec::new();
        for record in records {
            if self
                .history_replacement_source_is_reachable(chat_id, &index, &record.source)
                .await?
            {
                continue;
            }
            let source = self.history_replacement_path(chat_id, &record.record_id);
            let tombstone = self.history_replacement_tombstone_path(chat_id, &record.record_id);
            write_json_atomic(
                &tombstone,
                &serde_json::json!({
                    "record_id": record.record_id,
                    "deleted_at": chrono::Utc::now().to_rfc3339(),
                }),
            )
            .await?;
            match tokio::fs::remove_file(&source).await {
                Ok(()) => deleted_relatives
                    .push(format!("history_replacements/{}.json", record.record_id)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(rua_core::RuaError::Io {
                        message: format!(
                            "failed to remove tombstoned history replacement {}: {error}",
                            source.display()
                        ),
                    });
                }
            }
        }
        Ok(deleted_relatives)
    }

    async fn history_replacement_source_is_reachable(
        &self,
        chat_id: &str,
        index: &TurnIndex,
        source: &crate::HistoryReplacementSource,
    ) -> Result<bool> {
        let Some(turn) = index
            .turns
            .iter()
            .find(|turn| turn.turn_id == source.turn_id)
        else {
            return Ok(false);
        };
        if !turn
            .effective_assistant_response_variants()
            .iter()
            .any(|variant| {
                variant.first_sequence <= source.sequence_range.start_sequence
                    && variant.last_sequence >= source.sequence_range.end_sequence
            })
        {
            return Ok(false);
        }
        let messages = self
            .read_message_range(
                chat_id,
                source.sequence_range.start_sequence,
                source
                    .sequence_range
                    .end_sequence
                    .saturating_sub(source.sequence_range.start_sequence)
                    .saturating_add(1) as usize,
            )
            .await?;
        Ok(validate_history_replacement_source(source, &messages).is_ok())
    }

    pub async fn publish_ready_history_replacement(
        &self,
        chat_address: &ChatAddress,
        draft: HistoryReplacementReadyDraft,
    ) -> Result<HistoryReplacementRecord> {
        self.publish_ready_history_replacement_if(chat_address, draft, || true)
            .await?
            .ok_or(rua_core::RuaError::Cancelled)
    }

    pub async fn publish_ready_history_replacement_if<F>(
        &self,
        chat_address: &ChatAddress,
        draft: HistoryReplacementReadyDraft,
        can_publish: F,
    ) -> Result<Option<HistoryReplacementRecord>>
    where
        F: Fn() -> bool,
    {
        let chat_id = chat_address.chat_id().as_str();
        if !can_publish() {
            return Ok(None);
        }
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        if !can_publish() {
            return Ok(None);
        }
        self.validate_history_replacement_publication(chat_id, &draft.record_id, &draft.source)
            .await?;
        let existing = self
            .load_history_replacement_for_address(chat_address, &draft.record_id)
            .await?;
        let created_at = match existing {
            Some(existing)
                if matches!(existing.state, HistoryReplacementRecordState::Ready { .. }) =>
            {
                return Ok(Some(existing));
            }
            Some(existing) => {
                if existing.is_rejected_for_model(&draft.model) {
                    return Err(rua_core::RuaError::Io {
                        message: format!(
                            "history replacement generation identity is already rejected for {chat_id}/{}",
                            draft.record_id
                        ),
                    });
                }
                existing.created_at
            }
            None => draft.generated_at.clone(),
        };

        let mut record = HistoryReplacementRecord {
            record_id: draft.record_id,
            chat_address: chat_address.clone(),
            format_version: HISTORY_REPLACEMENT_FORMAT_VERSION,
            prompt_version: HISTORY_REPLACEMENT_PROMPT_VERSION,
            source: draft.source,
            state: HistoryReplacementRecordState::Ready {
                digest: draft.digest,
                replacement_token_estimate: draft.replacement_token_estimate,
                model: draft.model,
                generated_at: draft.generated_at.clone(),
            },
            created_at,
            updated_at: draft.generated_at,
            checksum: String::new(),
        };
        self.save_history_replacement_record_if(&mut record, can_publish)
            .await
    }

    pub async fn publish_rejected_history_replacement(
        &self,
        chat_address: &ChatAddress,
        draft: HistoryReplacementRejectedDraft,
    ) -> Result<HistoryReplacementRecord> {
        self.publish_rejected_history_replacement_if(chat_address, draft, || true)
            .await?
            .ok_or(rua_core::RuaError::Cancelled)
    }

    pub async fn publish_rejected_history_replacement_if<F>(
        &self,
        chat_address: &ChatAddress,
        draft: HistoryReplacementRejectedDraft,
        can_publish: F,
    ) -> Result<Option<HistoryReplacementRecord>>
    where
        F: Fn() -> bool,
    {
        let chat_id = chat_address.chat_id().as_str();
        if !can_publish() {
            return Ok(None);
        }
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        if !can_publish() {
            return Ok(None);
        }
        self.validate_history_replacement_publication(chat_id, &draft.record_id, &draft.source)
            .await?;
        let existing = self
            .load_history_replacement_for_address(chat_address, &draft.record_id)
            .await?;
        let (created_at, mut generations) = match existing {
            Some(existing)
                if matches!(existing.state, HistoryReplacementRecordState::Ready { .. }) =>
            {
                return Ok(Some(existing));
            }
            Some(existing) => {
                let generations = match &existing.state {
                    HistoryReplacementRecordState::Rejected { generations } => generations.clone(),
                    HistoryReplacementRecordState::Ready { .. } => {
                        unreachable!("ready state returned above")
                    }
                };
                if generations
                    .iter()
                    .any(|generation| generation.model == draft.model)
                {
                    return Ok(Some(existing));
                }
                (existing.created_at, generations)
            }
            None => (draft.rejected_at.clone(), Vec::new()),
        };
        generations.push(HistoryReplacementRejectedGeneration {
            model: draft.model,
            reason: draft.reason,
            replacement_token_estimate: draft.replacement_token_estimate,
            saved_token_estimate: draft.saved_token_estimate,
            rejected_at: draft.rejected_at.clone(),
        });
        let mut record = HistoryReplacementRecord {
            record_id: draft.record_id,
            chat_address: chat_address.clone(),
            format_version: HISTORY_REPLACEMENT_FORMAT_VERSION,
            prompt_version: HISTORY_REPLACEMENT_PROMPT_VERSION,
            source: draft.source,
            state: HistoryReplacementRecordState::Rejected { generations },
            created_at,
            updated_at: draft.rejected_at,
            checksum: String::new(),
        };
        self.save_history_replacement_record_if(&mut record, can_publish)
            .await
    }

    async fn validate_history_replacement_publication(
        &self,
        chat_id: &str,
        record_id: &str,
        source: &crate::HistoryReplacementSource,
    ) -> Result<ChatSessionHeader> {
        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        if record_id != derive_history_replacement_record_id(chat_id, source) {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "history replacement record id does not match source for {chat_id}"
                ),
            });
        }

        validate_history_replacement_source(
            source,
            &self
                .read_projectable_message_range(
                    chat_id,
                    source.sequence_range.start_sequence,
                    source.sequence_range.end_sequence,
                )
                .await?,
        )?;
        Ok(header)
    }

    async fn save_history_replacement_record_if<F>(
        &self,
        record: &mut HistoryReplacementRecord,
        can_publish: F,
    ) -> Result<Option<HistoryReplacementRecord>>
    where
        F: Fn() -> bool,
    {
        if !can_publish() {
            return Ok(None);
        }
        record.checksum = history_replacement_checksum(record)?;
        if !can_publish() {
            return Ok(None);
        }
        write_json_atomic(
            &self.history_replacement_path(
                record.chat_address.chat_id().as_str(),
                &record.record_id,
            ),
            record,
        )
        .await?;
        let tombstone = self.history_replacement_tombstone_path(
            record.chat_address.chat_id().as_str(),
            &record.record_id,
        );
        if let Err(error) = tokio::fs::remove_file(&tombstone).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(rua_core::RuaError::Io {
                    message: format!(
                        "failed to clear history replacement tombstone {}: {error}",
                        tombstone.display()
                    ),
                });
            }
        }
        Ok(Some(record.clone()))
    }

    pub async fn build_emergency_context_projection(
        &self,
        chat_address: &ChatAddress,
        tail_start_sequence: u64,
        request: ContextProjectionRequest,
        history_replacement_enabled: bool,
        token_estimate_divisor: f64,
    ) -> Result<ContextProjection> {
        let started = Instant::now();
        let chat_id = chat_address.chat_id().as_str();
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Err(rua_core::RuaError::Io {
                message: format!("chat session header not found for {chat_id}"),
            });
        };
        let index = self.load_turn_index(chat_id).await?;
        let active_latest_sequence = active_message_log_end_sequence(&header, &index);
        let checkpoint = self.load_active_summary_checkpoint(chat_id).await?;
        let boundary_sequence_start = projection_boundary_sequence_start(&header);
        let checkpoint_tail_start = checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.frontier.end_sequence.saturating_add(1))
            .unwrap_or(boundary_sequence_start);
        let start = tail_start_sequence
            .max(1)
            .max(boundary_sequence_start)
            .max(checkpoint_tail_start);
        let mut retained_messages: Vec<MessageLogEntry> =
            if let Some(checkpoint) = checkpoint.as_ref() {
                self.read_retained_checkpoint_messages(chat_id, checkpoint)
                    .await?
            } else {
                Vec::new()
            }
            .into_iter()
            .filter(message_is_model_projectable)
            .collect();
        let mut tail_messages = if start <= active_latest_sequence {
            self.read_active_messages_for_index(chat_id, &header, &index, start)
                .await?
        } else {
            Vec::new()
        };
        for turn in index.turns.iter().filter(|turn| {
            TurnLifecycleStatus::parse(&turn.lifecycle.status) == TurnLifecycleStatus::InProgress
                && turn.user_message_sequence >= boundary_sequence_start
                && turn.user_message_sequence < start
        }) {
            let user_sequence = turn.user_message_sequence;
            if !retained_messages
                .iter()
                .any(|message| message.sequence == user_sequence)
            {
                retained_messages.extend(
                    self.read_projectable_message_range(chat_id, user_sequence, user_sequence)
                        .await?,
                );
            }
        }
        retained_messages.sort_by_key(|message| message.sequence);
        let projection_exclusions = self.load_projection_exclusions(chat_id).await?;
        projection_exclusions.filter_messages(&mut retained_messages);
        projection_exclusions.filter_messages(&mut tail_messages);
        let tail_end_sequence = tail_messages.last().map(|message| message.sequence);
        let mut projection = ContextProjection {
            chat_id: header.chat_id,
            agent_id: header.agent_id,
            active_checkpoint_id: checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.checkpoint_id.clone()),
            summary: checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.summary.clone()),
            summary_model: checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.model.clone()),
            tail_start_sequence: tail_messages.first().map(|message| message.sequence),
            tail_end_sequence,
            retained_messages,
            tail_messages,
            current_user_input: request.current_user_input,
            stable_token_estimate: 0,
            history_replacement_evidence: Vec::new(),
        };
        if history_replacement_enabled {
            self.apply_history_replacements_for_address(
                chat_address,
                &mut projection,
                token_estimate_divisor,
            )
            .await?;
        }
        projection.stable_token_estimate = checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.token_estimate)
            .unwrap_or_default()
            .saturating_add(estimate_message_tokens_with_divisor(
                &projection.retained_messages,
                token_estimate_divisor,
            ))
            .saturating_add(estimate_message_tokens_with_divisor(
                &projection.tail_messages,
                token_estimate_divisor,
            ));

        debug!(
            chat_id = %projection.chat_id,
            agent_id = %projection.agent_id,
            active_checkpoint_id = ?checkpoint.as_ref().map(|checkpoint| checkpoint.checkpoint_id.as_str()),
            retained_message_count = projection.retained_messages.len(),
            tail_start_sequence = ?projection.tail_messages.first().map(|message| message.sequence),
            tail_end_sequence = ?tail_end_sequence,
            tail_message_count = projection.tail_messages.len(),
            stable_token_estimate = projection.stable_token_estimate,
            elapsed_ms = started.elapsed().as_millis(),
            "built emergency evergreen context projection"
        );

        Ok(projection)
    }

    pub async fn conversation_recall(
        &self,
        chat_id: &str,
        request: ConversationRecallRequest,
    ) -> Result<ConversationRecallResult> {
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Err(rua_core::RuaError::Io {
                message: format!("chat session header not found for {chat_id}"),
            });
        };
        let index = self.load_turn_index(chat_id).await?;
        let boundary_sequence_start = projection_boundary_sequence_start(&header);
        let messages = self
            .read_active_messages_for_index(chat_id, &header, &index, boundary_sequence_start)
            .await?;
        let max_results = request.max_results;
        let query_terms = conversation_recall_query_terms(&request.query);
        if max_results == 0 || query_terms.is_empty() {
            return Ok(ConversationRecallResult {
                chat_id: header.chat_id,
                hits: Vec::new(),
            });
        }
        let mut matchers = Vec::with_capacity(query_terms.len());
        for term in query_terms {
            matchers.push(
                TextMatcher::new(TextSearchQuery::literal(term)).map_err(|e| {
                    rua_core::RuaError::Io {
                        message: format!("failed to build conversation recall matcher: {e}"),
                    }
                })?,
            );
        }
        let mut hits = Vec::new();

        let surrounding = request.surrounding_messages as usize;
        for (position, message) in messages.iter().enumerate() {
            let Some(text) = conversation_message_text(message) else {
                continue;
            };
            if !matchers.iter().any(|matcher| matcher.is_match(&text)) {
                continue;
            }
            let span_start = messages
                .get(position.saturating_sub(surrounding))
                .map(|message| message.sequence)
                .unwrap_or(message.sequence);
            let span_end = messages
                .get(
                    position
                        .saturating_add(surrounding)
                        .min(messages.len().saturating_sub(1)),
                )
                .map(|message| message.sequence)
                .unwrap_or(message.sequence);
            hits.push(ConversationRecallHit {
                span: ConversationSpan {
                    start_sequence: span_start,
                    end_sequence: span_end,
                },
                snippet: text,
                message: ConversationRecallMessage {
                    sequence: message.sequence,
                    message_id: message.message_id.clone(),
                    turn_id: message.turn_id.clone(),
                    role: message.role.clone(),
                },
            });
            if hits.len() == max_results {
                break;
            }
        }

        Ok(ConversationRecallResult {
            chat_id: header.chat_id,
            hits,
        })
    }

    pub async fn conversation_read_span(
        &self,
        chat_id: &str,
        request: ConversationReadSpanRequest,
    ) -> Result<ConversationReadSpanResult> {
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Err(rua_core::RuaError::Io {
                message: format!("chat session header not found for {chat_id}"),
            });
        };
        let index = self.load_turn_index(chat_id).await?;
        let active_latest_sequence = active_message_log_end_sequence(&header, &index);
        let boundary_sequence_start = projection_boundary_sequence_start(&header);
        let active_messages = self
            .read_active_messages_for_index(chat_id, &header, &index, boundary_sequence_start)
            .await?;
        let requested_start = request
            .start_sequence
            .min(request.end_sequence)
            .max(boundary_sequence_start);
        let requested_end = request
            .start_sequence
            .max(request.end_sequence)
            .min(active_latest_sequence)
            .max(boundary_sequence_start);
        let first_index = active_messages.iter().position(|message| {
            message.sequence >= requested_start && message.sequence <= requested_end
        });
        let last_index = active_messages.iter().rposition(|message| {
            message.sequence >= requested_start && message.sequence <= requested_end
        });
        let messages = match (first_index, last_index) {
            (Some(first_index), Some(last_index)) => {
                let start_index = first_index.saturating_sub(request.expand_before as usize);
                let end_index = last_index
                    .saturating_add(request.expand_after as usize)
                    .min(active_messages.len().saturating_sub(1));
                active_messages[start_index..=end_index].to_vec()
            }
            _ => Vec::new(),
        };
        let start_sequence = messages
            .first()
            .map(|message| message.sequence)
            .unwrap_or(requested_start);
        let end_sequence = messages
            .last()
            .map(|message| message.sequence)
            .unwrap_or(requested_end);

        Ok(ConversationReadSpanResult {
            chat_id: header.chat_id,
            span: ConversationSpan {
                start_sequence,
                end_sequence,
            },
            messages,
        })
    }

    pub async fn search_saved_conversations(
        &self,
        request: SavedConversationSearchRequest,
    ) -> Result<SavedConversationSearchResult> {
        self.search_saved_conversations_filtered(request, true)
            .await
    }

    pub async fn search_saved_conversations_in_current_scope(
        &self,
        request: SavedConversationSearchRequest,
    ) -> Result<SavedConversationSearchResult> {
        self.search_saved_conversations_filtered(request, false)
            .await
    }

    async fn search_saved_conversations_filtered(
        &self,
        request: SavedConversationSearchRequest,
        filter_agent: bool,
    ) -> Result<SavedConversationSearchResult> {
        let query = request.query.trim().to_lowercase();
        if query.is_empty() || request.limit == 0 {
            return Ok(SavedConversationSearchResult {
                results: Vec::new(),
                offset: request.offset,
                limit: request.limit,
                total: 0,
                next_offset: None,
            });
        }
        let matcher = TextMatcher::new(TextSearchQuery::literal(query)).map_err(|e| {
            rua_core::RuaError::Io {
                message: format!("failed to build saved conversation search matcher: {e}"),
            }
        })?;

        let headers = self.list_chat_headers(0, usize::MAX).await?;
        let mut matches = Vec::new();
        for header in headers
            .into_iter()
            .filter(|header| !filter_agent || header.agent_id == request.agent_id)
            .filter(|header| filter_agent || !header.metadata.archived)
            .filter(|header| {
                !header
                    .extra
                    .get(crate::collaboration::COLLABORATION_META_KEY)
                    .is_some_and(Value::is_object)
            })
        {
            let index = self.load_turn_index(&header.chat_id).await?;
            let messages = self
                .read_active_messages_for_index(&header.chat_id, &header, &index, 1)
                .await?;
            for message in messages {
                let Some(text) = conversation_message_text(&message) else {
                    continue;
                };
                if !matcher.is_match(&text) {
                    continue;
                }
                matches.push(SavedConversationSearchHit {
                    chat_id: header.chat_id.clone(),
                    title: header.metadata.title.clone(),
                    preview: header.metadata.preview.clone(),
                    snippet: text,
                    span: ConversationSpan {
                        start_sequence: message.sequence,
                        end_sequence: message.sequence,
                    },
                    message: ConversationRecallMessage {
                        sequence: message.sequence,
                        message_id: message.message_id,
                        turn_id: message.turn_id,
                        role: message.role,
                    },
                });
            }
        }

        let total = matches.len();
        let results: Vec<_> = matches
            .into_iter()
            .skip(request.offset)
            .take(request.limit)
            .collect();
        let consumed = request.offset.saturating_add(results.len());
        let next_offset = (consumed < total).then_some(consumed);

        Ok(SavedConversationSearchResult {
            results,
            offset: request.offset,
            limit: request.limit,
            total,
            next_offset,
        })
    }

    pub async fn read_summary_checkpoint(
        &self,
        chat_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<SummaryCheckpoint>> {
        let Some(checkpoint) = self
            .read_summary_checkpoint_raw(chat_id, checkpoint_id)
            .await?
        else {
            return Ok(None);
        };
        if checkpoint.chat_id != chat_id {
            return Ok(None);
        }
        if checkpoint.checksum != checkpoint_checksum(&checkpoint)? {
            return Ok(None);
        }
        Ok(Some(checkpoint))
    }

    pub async fn projection_lineage_through_sequence(
        &self,
        chat_id: &str,
        end_sequence: u64,
    ) -> Result<String> {
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Err(rua_core::RuaError::Io {
                message: format!("chat session header not found for {chat_id}"),
            });
        };
        let index = self.load_turn_index(chat_id).await?;
        let excluded = self
            .projection_excluded_ids_through_sequence(chat_id, end_sequence)
            .await?;
        Self::projection_lineage_for_index(chat_id, &header, &index, end_sequence, &excluded)
    }

    async fn projection_excluded_ids_through_sequence(
        &self,
        chat_id: &str,
        end_sequence: u64,
    ) -> Result<Vec<String>> {
        let exclusions = self.load_projection_exclusions(chat_id).await?;
        if exclusions.message_ids.is_empty() {
            return Ok(Vec::new());
        }
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Ok(Vec::new());
        };
        let mut index = self.load_turn_index(chat_id).await?;
        index.turns.retain(|turn| !turn_origin_is_acp(turn));
        let start_sequence = projection_boundary_sequence_start(&header);
        let end_sequence = end_sequence.min(active_message_log_end_sequence(&header, &index));
        if start_sequence > end_sequence {
            return Ok(Vec::new());
        }
        let excluded = exclusions.message_ids.iter().collect::<HashSet<_>>();
        let turns = index
            .turns
            .iter()
            .map(|turn| (turn.turn_id.as_str(), turn))
            .collect::<HashMap<_, _>>();
        let mut matching = self
            .read_message_sequence_range_from_header(
                chat_id,
                start_sequence,
                end_sequence,
                &header.message_log,
            )
            .await?
            .into_iter()
            .filter(|message| {
                excluded.contains(&message.message_id)
                    && turns.get(message.turn_id.as_str()).is_some_and(|turn| {
                        message.sequence >= turn.first_sequence
                            && message.sequence <= turn.last_sequence
                            && (message.sequence == turn.user_message_sequence
                                || crate::runtime_note::RuntimeNote::from_message_log_entry(
                                    message,
                                )
                                .is_some()
                                || turn.selected_assistant_response_variant().is_some_and(
                                    |variant| {
                                        message.sequence >= variant.first_sequence
                                            && message.sequence <= variant.last_sequence
                                    },
                                ))
                    })
            })
            .map(|message| message.message_id)
            .collect::<Vec<_>>();
        matching.sort();
        matching.dedup();
        Ok(matching)
    }

    pub async fn covered_selected_variants_through_sequence(
        &self,
        chat_id: &str,
        end_sequence: u64,
    ) -> Result<Vec<CheckpointCoveredVariant>> {
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Err(rua_core::RuaError::Io {
                message: format!("chat session header not found for {chat_id}"),
            });
        };
        let index = self.load_turn_index(chat_id).await?;
        Ok(Self::covered_selected_variants_for_index(
            &header,
            &index,
            end_sequence,
        ))
    }

    fn covered_selected_variants_for_index(
        header: &ChatSessionHeader,
        index: &TurnIndex,
        end_sequence: u64,
    ) -> Vec<CheckpointCoveredVariant> {
        let start_sequence = projection_boundary_sequence_start(header);
        index
            .turns
            .iter()
            .filter(|turn| {
                turn.last_sequence >= start_sequence && turn.first_sequence <= end_sequence
            })
            .map(|turn| CheckpointCoveredVariant {
                turn_id: turn.turn_id.clone(),
                selected_variant_id: turn.selected_variant_id.clone(),
            })
            .collect()
    }

    fn projection_lineage_for_index(
        chat_id: &str,
        header: &ChatSessionHeader,
        index: &TurnIndex,
        end_sequence: u64,
        excluded_message_ids: &[String],
    ) -> Result<String> {
        let start_sequence = projection_boundary_sequence_start(header);
        let lineage = index
            .turns
            .iter()
            .filter(|turn| {
                turn.last_sequence >= start_sequence && turn.first_sequence <= end_sequence
            })
            .map(|turn| {
                let selected = turn.selected_assistant_response_variant();
                serde_json::json!({
                    "turn_id": turn.turn_id,
                    "ordinal": turn.ordinal,
                    "user_message_sequence": (turn.user_message_sequence <= end_sequence)
                        .then_some(turn.user_message_sequence),
                    "selected_variant_id": turn.selected_variant_id,
                    "selected_assistant_sequences": selected
                        .as_ref()
                        .map(|variant| variant.assistant_message_sequences
                            .iter()
                            .copied()
                            .filter(|sequence| *sequence <= end_sequence)
                            .collect::<Vec<_>>())
                        .unwrap_or_default(),
                    "selected_output_start": selected
                        .as_ref()
                        .map(|variant| variant.first_sequence)
                        .filter(|sequence| *sequence <= end_sequence),
                    "selected_output_end": selected
                        .as_ref()
                        .map(|variant| variant.last_sequence.min(end_sequence)),
                })
            })
            .collect::<Vec<_>>();
        // Keep the legacy lineage byte-for-byte when no covered Message is excluded.
        let lineage = if excluded_message_ids.is_empty() {
            serde_json::json!(lineage)
        } else {
            serde_json::json!({ "turns": lineage, "excluded_message_ids": excluded_message_ids })
        };
        let bytes = serde_json::to_vec(&lineage).map_err(|error| rua_core::RuaError::Io {
            message: format!("failed to serialize projection lineage for {chat_id}: {error}"),
        })?;
        let digest = Sha256::digest(bytes);
        Ok(format!("sha256:{digest:x}"))
    }

    pub async fn checkpoint_projection_state(
        &self,
        chat_id: &str,
        end_sequence: u64,
    ) -> Result<CheckpointProjectionState> {
        let index = self.load_turn_index(chat_id).await?;
        let turn = index
            .turns
            .iter()
            .find(|turn| {
                turn.first_sequence <= end_sequence && turn.last_sequence >= end_sequence
            })
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!(
                    "checkpoint frontier sequence {end_sequence} does not belong to a turn in {chat_id}"
                ),
            })?;
        let inside_active_turn = if turn.last_sequence == end_sequence
            && TurnLifecycleStatus::parse(&turn.lifecycle.status) != TurnLifecycleStatus::InProgress
        {
            false
        } else if self
            .active_turn_sequence_is_safe(chat_id, turn, end_sequence)
            .await?
        {
            true
        } else {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "checkpoint frontier sequence {end_sequence} is not safe in {chat_id}/{}",
                    turn.turn_id
                ),
            });
        };
        let retained_sequence_ranges =
            if inside_active_turn && turn.user_message_sequence <= end_sequence {
                vec![CheckpointRetainedSequenceRange {
                    start_sequence: turn.user_message_sequence,
                    end_sequence: turn.user_message_sequence,
                    reason: "active_user_request".to_string(),
                }]
            } else {
                Vec::new()
            };
        Ok(CheckpointProjectionState {
            frontier: CheckpointFrontier {
                legacy_kind: None,
                turn_id: turn.turn_id.clone(),
                end_sequence,
            },
            retained_sequence_ranges,
            projection_lineage: self
                .projection_lineage_through_sequence(chat_id, end_sequence)
                .await?,
        })
    }

    pub async fn projection_message_group_estimates(
        &self,
        chat_id: &str,
    ) -> Result<Vec<ProjectionMessageGroupEstimate>> {
        self.projection_message_group_estimates_effective(
            chat_id,
            false,
            rua_core::tool::DEFAULT_TOKEN_ESTIMATE_DIVISOR,
        )
        .await
    }

    pub async fn projection_message_group_estimates_effective(
        &self,
        chat_id: &str,
        history_replacement_enabled: bool,
        token_estimate_divisor: f64,
    ) -> Result<Vec<ProjectionMessageGroupEstimate>> {
        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let chat_address = ChatAddress::agent(header.agent_id, chat_id);
        self.projection_message_group_estimates_effective_for_address(
            &chat_address,
            history_replacement_enabled,
            token_estimate_divisor,
        )
        .await
    }

    pub async fn projection_message_group_estimates_effective_for_address(
        &self,
        chat_address: &ChatAddress,
        history_replacement_enabled: bool,
        token_estimate_divisor: f64,
    ) -> Result<Vec<ProjectionMessageGroupEstimate>> {
        let chat_id = chat_address.chat_id().as_str();
        let boundary_sequence_start = self
            .load_chat_header(chat_id)
            .await?
            .and_then(|header| header.projection_boundary)
            .map(|boundary| boundary.sequence_start.max(1))
            .unwrap_or(1);
        let turns = self.read_turn_range(chat_id, 1, usize::MAX).await?;
        let replacement_records = if history_replacement_enabled {
            self.load_ready_history_replacements(chat_address, token_estimate_divisor)
                .await?
        } else {
            Vec::new()
        };
        let mut estimates = Vec::new();
        for turn in turns {
            if turn_origin_is_acp(&turn)
                || turn
                    .metadata
                    .get("ui_only")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                || turn
                    .metadata
                    .get("model_visible")
                    .and_then(Value::as_bool)
                    .is_some_and(|visible| !visible)
                || turn.last_sequence < boundary_sequence_start
            {
                continue;
            }
            let group_start = turn.first_sequence.max(boundary_sequence_start);
            let turn_messages = self
                .read_projectable_message_range(chat_id, group_start, turn.last_sequence)
                .await?;
            let mut foldable_ranges = foldable_message_ranges(&turn_messages, group_start);
            if history_replacement_enabled {
                foldable_ranges = merge_ranges_covering_history_replacements(
                    foldable_ranges,
                    &replacement_records,
                );
            }
            for (first_sequence, last_sequence) in foldable_ranges {
                let retained_sequence_ranges = if TurnLifecycleStatus::parse(&turn.lifecycle.status)
                    == TurnLifecycleStatus::InProgress
                    && turn.user_message_sequence <= last_sequence
                {
                    vec![CheckpointRetainedSequenceRange {
                        start_sequence: turn.user_message_sequence,
                        end_sequence: turn.user_message_sequence,
                        reason: "active_user_request".to_string(),
                    }]
                } else {
                    Vec::new()
                };
                let mut messages = turn_messages
                    .iter()
                    .filter(|message| {
                        message.sequence >= first_sequence && message.sequence <= last_sequence
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if history_replacement_enabled {
                    apply_history_replacement_records(&mut messages, &replacement_records);
                }
                let token_estimate = messages
                    .iter()
                    .filter(|message| {
                        !retained_sequence_ranges.iter().any(|retained| {
                            message.sequence >= retained.start_sequence
                                && message.sequence <= retained.end_sequence
                        })
                    })
                    .map(|message| {
                        estimate_message_tokens_with_divisor(
                            std::slice::from_ref(message),
                            token_estimate_divisor,
                        )
                    })
                    .sum::<u64>()
                    .max(1);
                estimates.push(ProjectionMessageGroupEstimate {
                    turn_id: turn.turn_id.clone(),
                    turn_ordinal: turn.ordinal,
                    first_sequence,
                    last_sequence,
                    token_estimate,
                    foldable: true,
                    retained_sequence_ranges,
                });
            }
        }
        Ok(estimates)
    }

    async fn validate_checkpoint_draft(
        &self,
        chat_id: &str,
        draft: &SummaryCheckpointDraft,
    ) -> Result<()> {
        validate_sequence_range("covered", &draft.covered_sequence_range)?;
        validate_sequence_range("source", &draft.source_sequence_range)?;
        if draft.source_sequence_range.start_sequence < draft.covered_sequence_range.start_sequence
            || draft.source_sequence_range.end_sequence > draft.covered_sequence_range.end_sequence
        {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "checkpoint {} source range is outside covered range",
                    draft.checkpoint_id
                ),
            });
        }
        if draft.frontier.end_sequence != draft.covered_sequence_range.end_sequence {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "checkpoint {} frontier does not end at covered sequence",
                    draft.checkpoint_id
                ),
            });
        }

        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let index = self.load_turn_index(chat_id).await?;
        let active_latest_sequence = active_message_log_end_sequence(&header, &index);
        if draft.covered_sequence_range.end_sequence > active_latest_sequence {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "checkpoint {} covers non-durable sequence {}",
                    draft.checkpoint_id, draft.covered_sequence_range.end_sequence
                ),
            });
        }
        let frontier_turn = index
            .turns
            .iter()
            .find(|turn| turn.turn_id == draft.frontier.turn_id)
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!(
                    "checkpoint {} frontier turn {} does not exist",
                    draft.checkpoint_id, draft.frontier.turn_id
                ),
            })?;
        if frontier_turn.ordinal != draft.covered_turn_range.end_turn_ordinal {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "checkpoint {} frontier turn ordinal does not match covered range",
                    draft.checkpoint_id
                ),
            });
        }
        let frontier_is_safe = (frontier_turn.last_sequence == draft.frontier.end_sequence
            && TurnLifecycleStatus::parse(&frontier_turn.lifecycle.status)
                != TurnLifecycleStatus::InProgress)
            || self
                .active_turn_sequence_is_safe(chat_id, frontier_turn, draft.frontier.end_sequence)
                .await?;
        if !frontier_is_safe {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "checkpoint {} frontier is not a durable safe boundary",
                    draft.checkpoint_id
                ),
            });
        }

        let mut previous_end = None;
        for retained in &draft.retained_sequence_ranges {
            if retained.start_sequence == 0
                || retained.start_sequence > retained.end_sequence
                || retained.start_sequence < draft.covered_sequence_range.start_sequence
                || retained.end_sequence > draft.covered_sequence_range.end_sequence
                || previous_end.is_some_and(|end| retained.start_sequence <= end)
            {
                return Err(rua_core::RuaError::Io {
                    message: format!(
                        "checkpoint {} contains an invalid retained sequence range",
                        draft.checkpoint_id
                    ),
                });
            }
            let retained_messages = self
                .read_projectable_message_range(
                    chat_id,
                    retained.start_sequence,
                    retained.end_sequence,
                )
                .await?;
            if retained_messages.len()
                != retained
                    .end_sequence
                    .saturating_sub(retained.start_sequence)
                    .saturating_add(1) as usize
            {
                return Err(rua_core::RuaError::Io {
                    message: format!(
                        "checkpoint {} retained range references missing or inactive messages",
                        draft.checkpoint_id
                    ),
                });
            }
            previous_end = Some(retained.end_sequence);
        }
        let active_lineage = self
            .projection_lineage_through_sequence(chat_id, draft.frontier.end_sequence)
            .await?;
        if active_lineage != draft.projection_lineage {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "checkpoint {} projection lineage is stale",
                    draft.checkpoint_id
                ),
            });
        }
        Ok(())
    }

    async fn active_turn_sequence_is_safe(
        &self,
        chat_id: &str,
        turn: &TurnIndexRecord,
        end_sequence: u64,
    ) -> Result<bool> {
        let messages = self
            .read_projectable_message_range(chat_id, turn.first_sequence, turn.last_sequence)
            .await?;
        // Include the next persisted part: a prefix-only read can mistake the
        // middle of one assistant response for its end.
        Ok(foldable_message_ranges(&messages, turn.first_sequence)
            .iter()
            .any(|(_, safe_end)| *safe_end == end_sequence))
    }

    async fn read_retained_checkpoint_messages(
        &self,
        chat_id: &str,
        checkpoint: &SummaryCheckpoint,
    ) -> Result<Vec<MessageLogEntry>> {
        let mut messages = Vec::new();
        for retained in &checkpoint.retained_sequence_ranges {
            messages.extend(
                self.read_projectable_message_range(
                    chat_id,
                    retained.start_sequence,
                    retained.end_sequence,
                )
                .await?,
            );
        }
        messages.sort_by_key(|message| message.sequence);
        messages.dedup_by_key(|message| message.sequence);
        Ok(messages)
    }

    pub async fn read_checkpoint_summary_source(
        &self,
        chat_id: &str,
        start_sequence: u64,
        end_sequence: u64,
        retained_sequence_ranges: &[CheckpointRetainedSequenceRange],
    ) -> Result<Vec<MessageLogEntry>> {
        Ok(self
            .read_projectable_message_range(chat_id, start_sequence, end_sequence)
            .await?
            .into_iter()
            .filter(|message| {
                !retained_sequence_ranges.iter().any(|retained| {
                    message.sequence >= retained.start_sequence
                        && message.sequence <= retained.end_sequence
                })
            })
            .collect())
    }

    pub async fn read_checkpoint_summary_source_effective(
        &self,
        chat_id: &str,
        start_sequence: u64,
        end_sequence: u64,
        retained_sequence_ranges: &[CheckpointRetainedSequenceRange],
        history_replacement_enabled: bool,
        token_estimate_divisor: f64,
    ) -> Result<Vec<MessageLogEntry>> {
        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let chat_address = ChatAddress::agent(header.agent_id, chat_id);
        self.read_checkpoint_summary_source_effective_for_address(
            &chat_address,
            start_sequence,
            end_sequence,
            retained_sequence_ranges,
            history_replacement_enabled,
            token_estimate_divisor,
        )
        .await
    }

    pub async fn read_checkpoint_summary_source_effective_for_address(
        &self,
        chat_address: &ChatAddress,
        start_sequence: u64,
        end_sequence: u64,
        retained_sequence_ranges: &[CheckpointRetainedSequenceRange],
        history_replacement_enabled: bool,
        token_estimate_divisor: f64,
    ) -> Result<Vec<MessageLogEntry>> {
        let chat_id = chat_address.chat_id().as_str();
        let mut messages = self
            .read_checkpoint_summary_source(
                chat_id,
                start_sequence,
                end_sequence,
                retained_sequence_ranges,
            )
            .await?;
        if history_replacement_enabled {
            self.apply_history_replacements_to_messages_for_address(
                chat_address,
                &mut messages,
                token_estimate_divisor,
            )
            .await?;
        }
        Ok(messages)
    }

    async fn read_summary_checkpoint_raw(
        &self,
        chat_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<SummaryCheckpoint>> {
        let path = self.summary_checkpoint_path(chat_id, checkpoint_id);
        if !path.exists() {
            return Ok(None);
        }
        let content =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| rua_core::RuaError::Io {
                    message: format!("failed to read summary checkpoint {}: {e}", path.display()),
                })?;
        let checkpoint: SummaryCheckpoint =
            serde_json::from_str(&content).map_err(|e| rua_core::RuaError::Io {
                message: format!("failed to parse summary checkpoint {}: {e}", path.display()),
            })?;
        Ok(Some(checkpoint))
    }

    async fn save_summary_checkpoint(&self, checkpoint: &SummaryCheckpoint) -> Result<()> {
        write_json_atomic(
            &self.summary_checkpoint_path(&checkpoint.chat_id, &checkpoint.checkpoint_id),
            checkpoint,
        )
        .await
    }

    pub async fn append_conversation_turn(
        &self,
        chat_id: &str,
        turn: NewConversationTurn,
    ) -> Result<TurnIndexRecord> {
        let header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let chat_address = ChatAddress::agent(header.agent_id, chat_id);
        self.append_conversation_turn_for_address(&chat_address, turn)
            .await
    }

    pub async fn append_conversation_turn_for_address(
        &self,
        chat_address: &ChatAddress,
        turn: NewConversationTurn,
    ) -> Result<TurnIndexRecord> {
        let store = self.clone();
        let chat_address = chat_address.clone();
        tokio::spawn(async move {
            let result = store
                .commit_conversation_turn_for_address(&chat_address, turn)
                .await;
            if let Err(error) = &result {
                warn!(
                    chat_id = %chat_address.chat_id(),
                    error = %error,
                    "conversation turn commit failed"
                );
            }
            result
        })
        .await
        .map_err(|error| rua_core::RuaError::Io {
            message: format!("conversation turn commit task failed: {error}"),
        })?
    }

    async fn commit_conversation_turn_for_address(
        &self,
        chat_address: &ChatAddress,
        turn: NewConversationTurn,
    ) -> Result<TurnIndexRecord> {
        let chat_id = chat_address.chat_id().as_str();
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        let mut header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let mut index = self.load_turn_index(chat_id).await?;
        let committed_pending_variants = self
            .commit_pending_turns_to_selected_variants(chat_id, &mut index)
            .await?;

        let persisted_sequence = self
            .latest_persisted_message_sequence(chat_id, &header.message_log)
            .await?;
        let first_sequence = persisted_sequence.saturating_add(1);
        let entries: Vec<MessageLogEntry> = turn
            .messages
            .into_iter()
            .enumerate()
            .map(|(offset, message)| MessageLogEntry {
                message_id: message.message_id,
                sequence: first_sequence + offset as u64,
                turn_id: turn.turn_id.clone(),
                role: message.role,
                created_at: message.created_at,
                status: message.status,
                content: message.content,
                provider: message.provider,
                tool: message.tool,
                metadata: message.metadata,
            })
            .collect();

        if entries.is_empty() {
            return Err(rua_core::RuaError::Io {
                message: format!("cannot append empty conversation turn {}", turn.turn_id),
            });
        }

        let last_sequence = entries
            .last()
            .map(|entry| entry.sequence)
            .unwrap_or(first_sequence);
        let chunk_size = header.message_log.chunk_size.max(1);
        self.append_message_log_entries(chat_id, chunk_size, &entries)
            .await?;
        let user_message_sequence = entries
            .iter()
            .find(|entry| is_primary_user_message(entry))
            .map(|entry| entry.sequence)
            .unwrap_or(first_sequence);
        let assistant_response_sequences = entries
            .iter()
            .filter(|entry| entry.role == "assistant")
            .map(|entry| entry.sequence)
            .collect();
        let assistant_response_variants = new_variant_record(
            turn.selected_variant_id.clone(),
            &entries,
            &turn.lifecycle.status,
        )
        .into_iter()
        .collect::<Vec<_>>();

        let record = TurnIndexRecord {
            turn_id: turn.turn_id,
            ordinal: index.turns.len() as u64 + 1,
            user_message_sequence,
            assistant_response_sequences,
            first_sequence,
            last_sequence,
            token_estimate: estimate_message_tokens(&entries).max(1),
            lifecycle: turn.lifecycle,
            selected_variant_id: assistant_response_variants
                .last()
                .map(|variant| variant.variant_id.clone())
                .or(turn.selected_variant_id),
            assistant_response_variants,
            metadata: turn_metadata_from_messages(&entries),
        };

        index.turns.push(record.clone());
        self.save_turn_index(chat_id, &index).await?;

        header.message_log.latest_sequence = last_sequence;
        header.message_log.allocated_sequence = last_sequence;
        header.turn_index.latest_turn = record.ordinal;
        if let Some(preview) = entries.iter().rev().find_map(message_preview_text) {
            header.metadata.preview = Some(ChatPreview {
                last_message_sequence: Some(last_sequence),
                text: preview,
            });
        }
        if let Some(last_activity_at) = entries.last().map(|entry| entry.created_at.clone()) {
            header.metadata.last_activity_at = last_activity_at;
        }
        self.save_chat_header(&header).await?;

        drop(_guard);
        if committed_pending_variants {
            if let Err(error) = self
                .prune_unreachable_history_replacements_for_address(chat_address)
                .await
            {
                warn!(
                    %error,
                    chat_id,
                    "failed to prune unreachable history replacements after committing selected variants"
                );
            }
        }
        Ok(record)
    }

    pub async fn read_message_range(
        &self,
        chat_id: &str,
        start_sequence: u64,
        limit: usize,
    ) -> Result<Vec<MessageLogEntry>> {
        if limit == 0 {
            return Ok(vec![]);
        }
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Ok(vec![]);
        };
        self.read_message_range_from_header(chat_id, start_sequence, limit, &header.message_log)
            .await
    }

    pub async fn read_message_range_from_header(
        &self,
        chat_id: &str,
        start_sequence: u64,
        limit: usize,
        message_log: &MessageLogHeader,
    ) -> Result<Vec<MessageLogEntry>> {
        if limit == 0 {
            return Ok(vec![]);
        }
        let chunk_size = message_log.chunk_size.max(1);
        let mut messages = Vec::new();
        let mut sequence = start_sequence.max(1);

        while sequence <= message_log.latest_sequence && messages.len() < limit {
            let chunk_start = chunk_start_sequence(sequence, chunk_size);
            for entry in self
                .read_message_chunk(chat_id, chunk_size, chunk_start)
                .await?
            {
                if entry.sequence < start_sequence {
                    continue;
                }
                if entry.sequence > message_log.latest_sequence {
                    break;
                }
                messages.push(entry);
                if messages.len() == limit {
                    break;
                }
            }
            sequence = chunk_start + chunk_size;
        }

        Ok(messages)
    }

    pub async fn read_message_sequence_range_from_header(
        &self,
        chat_id: &str,
        start_sequence: u64,
        end_sequence: u64,
        message_log: &MessageLogHeader,
    ) -> Result<Vec<MessageLogEntry>> {
        let start_sequence = start_sequence.max(1);
        let end_sequence = end_sequence.min(message_log.latest_sequence);
        if start_sequence > end_sequence {
            return Ok(vec![]);
        }

        let chunk_size = message_log.chunk_size.max(1);
        let mut messages = Vec::new();
        let mut sequence = start_sequence;
        while sequence <= end_sequence {
            let chunk_start = chunk_start_sequence(sequence, chunk_size);
            for entry in self
                .read_message_chunk(chat_id, chunk_size, chunk_start)
                .await?
            {
                if entry.sequence < start_sequence {
                    continue;
                }
                if entry.sequence > end_sequence {
                    break;
                }
                messages.push(entry);
            }
            sequence = chunk_start.saturating_add(chunk_size);
        }
        Ok(messages)
    }

    async fn latest_persisted_message_sequence(
        &self,
        chat_id: &str,
        message_log: &MessageLogHeader,
    ) -> Result<u64> {
        let messages_dir = self.messages_dir(chat_id);
        let chunk_size = message_log.chunk_size.max(1);
        tokio::task::spawn_blocking(move || {
            Ok(
                read_latest_persisted_message_entry_sync(&messages_dir, chunk_size)?
                    .map(|entry| entry.sequence)
                    .unwrap_or(0),
            )
        })
        .await
        .map_err(|error| rua_core::RuaError::Io {
            message: format!("failed to join message watermark recovery task: {error}"),
        })?
    }

    pub async fn read_latest_message_from_header(
        &self,
        chat_id: &str,
        message_log: &MessageLogHeader,
    ) -> Result<Option<MessageLogEntry>> {
        let messages_dir = self.messages_dir(chat_id);
        let chunk_size = message_log.chunk_size.max(1);
        // Read sequence and content from one opened chunk. A concurrent atomic append
        // may replace the path, but cannot change this read's inode snapshot.
        tokio::task::spawn_blocking(move || {
            read_latest_persisted_message_entry_sync(&messages_dir, chunk_size)
        })
        .await
        .map_err(|error| rua_core::RuaError::Io {
            message: format!("failed to join latest message read task: {error}"),
        })?
    }

    pub async fn append_messages_to_turn(
        &self,
        chat_id: &str,
        turn_id: &str,
        lifecycle: TurnLifecycle,
        selected_variant_id: Option<String>,
        messages: Vec<NewConversationMessage>,
    ) -> Result<TurnIndexRecord> {
        let terminal =
            TurnLifecycleStatus::parse(&lifecycle.status) != TurnLifecycleStatus::InProgress;
        let record = self
            .append_messages_to_turn_inner(
                chat_id,
                turn_id,
                Some(lifecycle),
                selected_variant_id,
                messages,
                AppendTurnOptions {
                    mode: AppendTurnOutputMode::ContinueSelected,
                    preserve_lifecycle: false,
                    deduplicate_message_ids: false,
                    include_in_assistant_variant: true,
                },
            )
            .await?;
        if terminal {
            self.flush_pending_runtime_notes(chat_id, turn_id).await?;
            return Ok(self
                .read_turn_range(chat_id, 1, usize::MAX)
                .await?
                .into_iter()
                .find(|turn| turn.turn_id == turn_id)
                .unwrap_or(record));
        }
        Ok(record)
    }

    pub async fn append_runtime_note(
        &self,
        chat_id: &str,
        turn_id: &str,
        note: crate::runtime_note::RuntimeNote,
    ) -> Result<TurnIndexRecord> {
        let write_lock = self.chat_write_lock(chat_id);
        let guard = write_lock.lock().await;
        let index = self.load_turn_index(chat_id).await?;
        let turn = index
            .turns
            .iter()
            .find(|turn| turn.turn_id == turn_id)
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("turn index record not found for {chat_id}/{turn_id}"),
            })?
            .clone();
        if TurnLifecycleStatus::parse(&turn.lifecycle.status) == TurnLifecycleStatus::InProgress {
            let mut pending = self.load_pending_runtime_notes(chat_id).await?;
            if pending
                .notes
                .iter()
                .all(|pending| pending.note.runtime_note_id != note.runtime_note_id)
            {
                pending.notes.push(PendingRuntimeNote {
                    turn_id: turn_id.to_string(),
                    note,
                });
                self.save_pending_runtime_notes(chat_id, &pending).await?;
            }
            return Ok(turn);
        }
        drop(guard);
        self.append_runtime_note_direct(chat_id, turn_id, note)
            .await
    }

    async fn append_runtime_note_direct(
        &self,
        chat_id: &str,
        turn_id: &str,
        note: crate::runtime_note::RuntimeNote,
    ) -> Result<TurnIndexRecord> {
        self.append_messages_to_turn_inner(
            chat_id,
            turn_id,
            None,
            None,
            vec![note.into_new_conversation_message()?],
            AppendTurnOptions {
                mode: AppendTurnOutputMode::ContinueSelected,
                preserve_lifecycle: true,
                deduplicate_message_ids: true,
                include_in_assistant_variant: false,
            },
        )
        .await
    }

    pub async fn flush_pending_runtime_notes(
        &self,
        chat_id: &str,
        turn_id: &str,
    ) -> Result<Vec<crate::runtime_note::RuntimeNote>> {
        let pending = {
            let write_lock = self.chat_write_lock(chat_id);
            let _guard = write_lock.lock().await;
            self.load_pending_runtime_notes(chat_id)
                .await?
                .notes
                .into_iter()
                .filter(|pending| pending.turn_id == turn_id)
                .collect::<Vec<_>>()
        };
        if pending.is_empty() {
            return Ok(Vec::new());
        }

        for pending_note in &pending {
            self.append_runtime_note_direct(chat_id, turn_id, pending_note.note.clone())
                .await?;
        }

        let flushed_ids = pending
            .iter()
            .map(|pending| pending.note.runtime_note_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        {
            let write_lock = self.chat_write_lock(chat_id);
            let _guard = write_lock.lock().await;
            let mut current = self.load_pending_runtime_notes(chat_id).await?;
            current.notes.retain(|pending| {
                pending.turn_id != turn_id
                    || !flushed_ids.contains(pending.note.runtime_note_id.as_str())
            });
            self.save_pending_runtime_notes(chat_id, &current).await?;
        }

        Ok(pending.into_iter().map(|pending| pending.note).collect())
    }

    pub async fn append_runtime_note_to_latest_turn(
        &self,
        chat_id: &str,
        note: crate::runtime_note::RuntimeNote,
    ) -> Result<TurnIndexRecord> {
        let write_lock = self.chat_write_lock(chat_id);
        let guard = write_lock.lock().await;
        let index = self.load_turn_index(chat_id).await?;
        let turn_id = index
            .turns
            .last()
            .map(|turn| turn.turn_id.clone())
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("chat '{chat_id}' has no Turn for Runtime Note"),
            })?;
        drop(guard);
        self.append_runtime_note(chat_id, &turn_id, note).await
    }

    pub async fn append_messages_to_turn_as_new_variant(
        &self,
        chat_id: &str,
        turn_id: &str,
        lifecycle: TurnLifecycle,
        selected_variant_id: Option<String>,
        messages: Vec<NewConversationMessage>,
    ) -> Result<TurnIndexRecord> {
        self.append_messages_to_turn_inner(
            chat_id,
            turn_id,
            Some(lifecycle),
            selected_variant_id,
            messages,
            AppendTurnOptions {
                mode: AppendTurnOutputMode::NewVariant,
                preserve_lifecycle: false,
                deduplicate_message_ids: false,
                include_in_assistant_variant: true,
            },
        )
        .await
    }

    async fn append_messages_to_turn_inner(
        &self,
        chat_id: &str,
        turn_id: &str,
        lifecycle: Option<TurnLifecycle>,
        selected_variant_id: Option<String>,
        messages: Vec<NewConversationMessage>,
        options: AppendTurnOptions,
    ) -> Result<TurnIndexRecord> {
        let store = self.clone();
        let chat_id = chat_id.to_string();
        let turn_id = turn_id.to_string();
        tokio::spawn(async move {
            let result = store
                .commit_messages_to_turn(
                    &chat_id,
                    &turn_id,
                    lifecycle,
                    selected_variant_id,
                    messages,
                    options,
                )
                .await;
            if let Err(error) = &result {
                warn!(chat_id, turn_id, error = %error, "turn message commit failed");
            }
            result
        })
        .await
        .map_err(|error| rua_core::RuaError::Io {
            message: format!("turn message commit task failed: {error}"),
        })?
    }

    async fn commit_messages_to_turn(
        &self,
        chat_id: &str,
        turn_id: &str,
        lifecycle: Option<TurnLifecycle>,
        selected_variant_id: Option<String>,
        mut messages: Vec<NewConversationMessage>,
        options: AppendTurnOptions,
    ) -> Result<TurnIndexRecord> {
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        let mut header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let mut index = self.load_turn_index(chat_id).await?;
        let Some(turn_position) = index.turns.iter().position(|turn| turn.turn_id == turn_id)
        else {
            return Err(rua_core::RuaError::Io {
                message: format!("turn index record not found for {chat_id}/{turn_id}"),
            });
        };

        if options.deduplicate_message_ids {
            let turn = &index.turns[turn_position];
            let existing_ids = self
                .read_message_sequence_range_from_header(
                    chat_id,
                    turn.first_sequence,
                    turn.last_sequence,
                    &header.message_log,
                )
                .await?
                .into_iter()
                .map(|message| message.message_id)
                .collect::<std::collections::HashSet<_>>();
            messages.retain(|message| !existing_ids.contains(&message.message_id));
            if messages.is_empty() {
                return Ok(turn.clone());
            }
        }

        let persisted_sequence = self
            .latest_persisted_message_sequence(chat_id, &header.message_log)
            .await?;
        let first_sequence = persisted_sequence.saturating_add(1);
        let entries: Vec<MessageLogEntry> = messages
            .into_iter()
            .enumerate()
            .map(|(offset, message)| MessageLogEntry {
                message_id: message.message_id,
                sequence: first_sequence + offset as u64,
                turn_id: turn_id.to_string(),
                role: message.role,
                created_at: message.created_at,
                status: message.status,
                content: message.content,
                provider: message.provider,
                tool: message.tool,
                metadata: message.metadata,
            })
            .collect();

        if entries.is_empty() {
            return Err(rua_core::RuaError::Io {
                message: format!("cannot append empty message batch to turn {turn_id}"),
            });
        }

        let last_sequence = entries
            .last()
            .map(|entry| entry.sequence)
            .unwrap_or(first_sequence);
        let chunk_size = header.message_log.chunk_size.max(1);
        self.append_message_log_entries(chat_id, chunk_size, &entries)
            .await?;
        let turn = &mut index.turns[turn_position];
        turn.last_sequence = last_sequence;
        turn.token_estimate = turn
            .token_estimate
            .saturating_add(estimate_message_tokens(&entries))
            .max(1);
        if !options.preserve_lifecycle {
            turn.lifecycle = lifecycle.ok_or_else(|| rua_core::RuaError::Io {
                message: format!(
                    "turn lifecycle is required for message append {chat_id}/{turn_id}"
                ),
            })?;
        }
        if turn.metadata.is_empty() {
            turn.metadata = turn_metadata_from_messages(&entries);
        }
        let new_assistant_sequences = entries
            .iter()
            .filter(|entry| entry.role == "assistant")
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>();
        let has_output = options.include_in_assistant_variant && entries.iter().any(is_turn_output);
        if has_output {
            ensure_variant_records_from_legacy(turn);
            match options.mode {
                AppendTurnOutputMode::ContinueSelected => {
                    extend_selected_variant(
                        turn,
                        selected_variant_id,
                        &entries,
                        new_assistant_sequences,
                    );
                }
                AppendTurnOutputMode::NewVariant => {
                    if let Some(variant) =
                        new_variant_record(selected_variant_id, &entries, &turn.lifecycle.status)
                    {
                        turn.selected_variant_id = Some(variant.variant_id.clone());
                        turn.assistant_response_sequences =
                            variant.assistant_message_sequences.clone();
                        turn.assistant_response_variants.push(variant);
                    }
                }
            }
        }
        if let Some(user_sequence) = entries
            .iter()
            .find(|entry| is_primary_user_message(entry))
            .map(|entry| entry.sequence)
        {
            turn.user_message_sequence = user_sequence;
        }
        let record = turn.clone();

        self.save_turn_index(chat_id, &index).await?;

        header.message_log.latest_sequence = last_sequence;
        header.message_log.allocated_sequence = last_sequence;
        if let Some(preview) = entries.iter().rev().find_map(message_preview_text) {
            header.metadata.preview = Some(ChatPreview {
                last_message_sequence: Some(last_sequence),
                text: preview,
            });
        }
        if let Some(last_activity_at) = entries.last().map(|entry| entry.created_at.clone()) {
            header.metadata.last_activity_at = last_activity_at;
        }
        self.save_chat_header(&header).await?;

        Ok(record)
    }

    pub async fn read_turn_range(
        &self,
        chat_id: &str,
        start_ordinal: u64,
        limit: usize,
    ) -> Result<Vec<TurnIndexRecord>> {
        if limit == 0 {
            return Ok(vec![]);
        }
        let start = start_ordinal.max(1);
        Ok(self
            .load_turn_index(chat_id)
            .await?
            .turns
            .into_iter()
            .filter(|turn| turn.ordinal >= start)
            .take(limit)
            .collect())
    }

    pub async fn read_turn_messages(
        &self,
        chat_id: &str,
        turn_id: &str,
    ) -> Result<Vec<MessageLogEntry>> {
        let Some(turn) = self
            .load_turn_index(chat_id)
            .await?
            .turns
            .into_iter()
            .find(|turn| turn.turn_id == turn_id)
        else {
            return Ok(vec![]);
        };

        self.read_message_range(
            chat_id,
            turn.first_sequence,
            (turn.last_sequence - turn.first_sequence + 1) as usize,
        )
        .await
    }

    pub async fn read_projectable_message_range(
        &self,
        chat_id: &str,
        start_sequence: u64,
        end_sequence: u64,
    ) -> Result<Vec<MessageLogEntry>> {
        if start_sequence > end_sequence {
            return Ok(Vec::new());
        }
        let Some(header) = self.load_chat_header(chat_id).await? else {
            return Ok(Vec::new());
        };
        let index = self.load_turn_index(chat_id).await?;
        let messages = self
            .read_active_messages_for_index(chat_id, &header, &index, start_sequence)
            .await?
            .into_iter()
            .filter(|message| message.sequence <= end_sequence)
            .collect();
        Ok(messages)
    }

    async fn read_active_messages_for_index(
        &self,
        chat_id: &str,
        header: &ChatSessionHeader,
        index: &TurnIndex,
        start_sequence: u64,
    ) -> Result<Vec<MessageLogEntry>> {
        if index.turns.is_empty() {
            return Ok(Vec::new());
        }
        let active_latest_sequence = active_message_log_end_sequence(header, index);
        let start_sequence = start_sequence.max(1);
        if start_sequence > active_latest_sequence {
            return Ok(Vec::new());
        }
        let mut active_turns = HashMap::new();
        for turn in &index.turns {
            if turn_origin_is_acp(turn) {
                continue;
            }
            if turn.last_sequence < start_sequence || turn.first_sequence > active_latest_sequence {
                continue;
            }
            let user_sequence = (turn.user_message_sequence >= start_sequence
                && turn.user_message_sequence <= active_latest_sequence)
                .then_some(turn.user_message_sequence);
            let output_range = turn
                .selected_assistant_response_variant()
                .and_then(|variant| {
                    let first_sequence = variant.first_sequence.max(start_sequence);
                    let last_sequence = variant.last_sequence.min(active_latest_sequence);
                    (first_sequence <= last_sequence).then_some((first_sequence, last_sequence))
                });
            active_turns.insert(
                turn.turn_id.clone(),
                ActiveTurnMessages {
                    user_sequence,
                    output_range,
                },
            );
        }
        if active_turns.is_empty() {
            return Ok(Vec::new());
        }

        let mut messages: Vec<_> = self
            .read_message_sequence_range_from_header(
                chat_id,
                start_sequence,
                active_latest_sequence,
                &header.message_log,
            )
            .await?
            .into_iter()
            .filter(|message| {
                let Some(active) = active_turns.get(&message.turn_id) else {
                    return false;
                };
                if message.role == "user" {
                    return active.user_sequence == Some(message.sequence)
                        || (is_turn_aborted_marker(message)
                            && active.output_range.is_some_and(|(start, end)| {
                                message.sequence >= start && message.sequence <= end
                            }));
                }
                if let Some(note) =
                    crate::runtime_note::RuntimeNote::from_message_log_entry(message)
                {
                    return note.is_model_visible();
                }
                if message_is_ui_only(message) {
                    return false;
                }
                active.output_range.is_some_and(|(start, end)| {
                    message.sequence >= start && message.sequence <= end
                })
            })
            .collect();
        messages.sort_by_key(|message| message.sequence);
        Ok(messages)
    }

    pub async fn set_turn_selected_variant(
        &self,
        chat_id: &str,
        turn_id: &str,
        selected_variant_id: String,
    ) -> Result<bool> {
        let write_lock = self.chat_write_lock(chat_id);
        let _guard = write_lock.lock().await;
        let mut index = self.load_turn_index(chat_id).await?;
        let Some(turn_position) = index.turns.iter().position(|turn| turn.turn_id == turn_id)
        else {
            return Ok(false);
        };
        let turn = &index.turns[turn_position];
        let Some(selected_variant) = turn
            .effective_assistant_response_variants()
            .into_iter()
            .find(|variant| variant.variant_id == selected_variant_id)
        else {
            return Ok(false);
        };
        index.turns[turn_position].selected_variant_id = Some(selected_variant_id);
        index.turns[turn_position].assistant_response_sequences =
            selected_variant.assistant_message_sequences;
        self.save_turn_index(chat_id, &index).await?;
        Ok(true)
    }

    pub async fn set_turn_lifecycle(
        &self,
        chat_id: &str,
        turn_id: &str,
        lifecycle: TurnLifecycle,
    ) -> Result<bool> {
        let write_lock = self.chat_write_lock(chat_id);
        let guard = write_lock.lock().await;
        let mut index = self.load_turn_index(chat_id).await?;
        let Some(turn) = index.turns.iter_mut().find(|turn| turn.turn_id == turn_id) else {
            return Ok(false);
        };
        let terminal =
            TurnLifecycleStatus::parse(&lifecycle.status) != TurnLifecycleStatus::InProgress;
        turn.lifecycle = lifecycle;
        let selected_variant = turn
            .selected_variant_id
            .as_deref()
            .and_then(|selected| {
                turn.assistant_response_variants
                    .iter()
                    .position(|variant| variant.variant_id == selected)
            })
            .or_else(|| turn.assistant_response_variants.len().checked_sub(1));
        if let Some(index) = selected_variant {
            turn.assistant_response_variants[index].status = turn.lifecycle.status.clone();
        }
        self.save_turn_index(chat_id, &index).await?;
        drop(guard);
        if terminal {
            self.flush_pending_runtime_notes(chat_id, turn_id).await?;
        }
        Ok(true)
    }

    pub async fn rollback_active_conversation_to_turn(
        &self,
        chat_id: &str,
        turn_id: &str,
        selected_variant_id: String,
    ) -> Result<Option<TurnIndexRecord>> {
        let mut header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let mut index = self.load_turn_index(chat_id).await?;
        let Some(turn_position) = index.turns.iter().position(|turn| turn.turn_id == turn_id)
        else {
            return Ok(None);
        };

        let selected_variant = match index.turns[turn_position]
            .effective_assistant_response_variants()
            .into_iter()
            .find(|variant| variant.variant_id == selected_variant_id)
        {
            Some(variant) => variant,
            None => return Ok(None),
        };
        index.turns[turn_position].selected_variant_id = Some(selected_variant_id);
        index.turns[turn_position].assistant_response_sequences =
            selected_variant.assistant_message_sequences.clone();
        index.turns[turn_position].assistant_response_variants = vec![selected_variant.clone()];
        index.turns[turn_position].last_sequence = selected_variant.last_sequence;
        index.turns.truncate(turn_position + 1);
        let active = index.turns[turn_position].clone();
        self.save_turn_index(chat_id, &index).await?;

        header.turn_index.latest_turn = index.turns.len() as u64;
        if let Some(checkpoint_id) = header.active_checkpoint_id.clone() {
            let checkpoint = self
                .read_summary_checkpoint(chat_id, &checkpoint_id)
                .await?;
            if checkpoint.as_ref().is_none_or(|checkpoint| {
                checkpoint.covered_sequence_range.end_sequence > active.last_sequence
            }) {
                header.active_checkpoint_id = None;
            }
        }
        if let Some(message) = self
            .read_message_range(chat_id, active.last_sequence, 1)
            .await?
            .into_iter()
            .next()
        {
            header.metadata.preview = message_preview_text(&message).map(|text| ChatPreview {
                last_message_sequence: Some(active.last_sequence),
                text,
            });
            header.metadata.last_activity_at = message.created_at;
        }
        self.save_chat_header(&header).await?;

        if let Err(error) = self.prune_unreachable_history_replacements(chat_id).await {
            warn!(
                %error,
                chat_id,
                "failed to prune unreachable history replacements after rollback"
            );
        }
        Ok(Some(active))
    }

    pub async fn rollback_active_conversation_to_empty(&self, chat_id: &str) -> Result<bool> {
        let write_lock = self.chat_write_lock(chat_id);
        let guard = write_lock.lock().await;
        let mut header =
            self.load_chat_header(chat_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("chat session header not found for {chat_id}"),
                })?;
        let mut index = self.load_turn_index(chat_id).await?;
        if index.turns.is_empty() {
            return Ok(false);
        }

        index.turns.clear();
        self.save_turn_index(chat_id, &index).await?;
        header.turn_index.latest_turn = 0;
        header.active_checkpoint_id = None;
        header.projection_boundary = None;
        header.context_snapshot = ChatContextSnapshot::default();
        header.metadata.preview = None;
        header.metadata.last_activity_at = header.metadata.created_at.clone();
        self.save_chat_header(&header).await?;
        drop(guard);

        if let Err(error) = self.prune_unreachable_history_replacements(chat_id).await {
            warn!(
                %error,
                chat_id,
                "failed to prune unreachable history replacements after empty rollback"
            );
        }
        Ok(true)
    }

    async fn commit_pending_turns_to_selected_variants(
        &self,
        chat_id: &str,
        index: &mut TurnIndex,
    ) -> Result<bool> {
        let mut changed = false;
        for position in 0..index.turns.len() {
            let Some(selected_variant_id) = index.turns[position].selected_variant_id.clone()
            else {
                continue;
            };
            if turn_is_committed_to_selected_variant(&index.turns[position], &selected_variant_id) {
                continue;
            }
            let turn = index.turns[position].clone();
            let Some(selected_variant) = turn
                .effective_assistant_response_variants()
                .into_iter()
                .find(|variant| variant.variant_id == selected_variant_id)
            else {
                continue;
            };
            index.turns[position].assistant_response_sequences =
                selected_variant.assistant_message_sequences.clone();
            index.turns[position].assistant_response_variants = vec![selected_variant.clone()];
            let latest_runtime_note_sequence = self
                .read_message_range(
                    chat_id,
                    turn.first_sequence,
                    turn.last_sequence.saturating_sub(turn.first_sequence) as usize + 1,
                )
                .await?
                .into_iter()
                .filter(|message| {
                    crate::runtime_note::RuntimeNote::from_message_log_entry(message).is_some()
                })
                .map(|message| message.sequence)
                .max();
            index.turns[position].last_sequence = latest_runtime_note_sequence
                .unwrap_or(selected_variant.last_sequence)
                .max(selected_variant.last_sequence);
            changed = true;
        }
        Ok(changed)
    }

    async fn load_turn_index(&self, chat_id: &str) -> Result<TurnIndex> {
        let path = self.turn_index_path(chat_id);
        if !path.exists() {
            return Ok(TurnIndex::default());
        }
        let content =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| rua_core::RuaError::Io {
                    message: format!("failed to read turn index {}: {e}", path.display()),
                })?;
        serde_json::from_str(&content).map_err(|e| rua_core::RuaError::Io {
            message: format!("failed to parse turn index {}: {e}", path.display()),
        })
    }

    async fn save_turn_index(&self, chat_id: &str, index: &TurnIndex) -> Result<()> {
        write_json_atomic(&self.turn_index_path(chat_id), index).await
    }

    async fn append_message_log_entries(
        &self,
        chat_id: &str,
        chunk_size: u64,
        entries: &[MessageLogEntry],
    ) -> Result<()> {
        let base = self.messages_dir(chat_id);
        let mut by_chunk: Vec<(u64, Vec<MessageLogEntry>)> = Vec::new();
        for entry in entries {
            let chunk_start = chunk_start_sequence(entry.sequence, chunk_size);
            match by_chunk.last_mut() {
                Some((current_start, current_entries)) if *current_start == chunk_start => {
                    current_entries.push(entry.clone());
                }
                _ => by_chunk.push((chunk_start, vec![entry.clone()])),
            }
        }

        tokio::task::spawn_blocking(move || {
            for (chunk_start, entries) in by_chunk {
                append_message_chunk_sync(&base, chunk_start, chunk_size, &entries)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| rua_core::RuaError::Io {
            message: format!("failed to join message append task: {e}"),
        })?
    }

    async fn read_message_chunk(
        &self,
        chat_id: &str,
        chunk_size: u64,
        chunk_start: u64,
    ) -> Result<Vec<MessageLogEntry>> {
        let path = self.message_chunk_path(chat_id, chunk_size, chunk_start);
        if !path.exists() {
            return Ok(vec![]);
        }
        let content =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| rua_core::RuaError::Io {
                    message: format!("failed to read message chunk {}: {e}", path.display()),
                })?;
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line).map_err(|e| rua_core::RuaError::Io {
                    message: format!("failed to parse message chunk {}: {e}", path.display()),
                })
            })
            .collect()
    }

    fn turn_index_path(&self, chat_id: &str) -> PathBuf {
        self.chat_dir(chat_id).join("turn_index.json")
    }

    fn messages_dir(&self, chat_id: &str) -> PathBuf {
        self.chat_dir(chat_id).join("messages")
    }

    fn pending_runtime_notes_path(&self, chat_id: &str) -> PathBuf {
        self.chat_dir(chat_id).join(".pending_runtime_notes.json")
    }

    async fn load_pending_runtime_notes(&self, chat_id: &str) -> Result<PendingRuntimeNotes> {
        let path = self.pending_runtime_notes_path(chat_id);
        if !path.exists() {
            return Ok(PendingRuntimeNotes::default());
        }
        let content =
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|error| rua_core::RuaError::Io {
                    message: format!(
                        "failed to read pending Runtime Notes {}: {error}",
                        path.display()
                    ),
                })?;
        serde_json::from_str(&content).map_err(|error| rua_core::RuaError::Io {
            message: format!(
                "failed to parse pending Runtime Notes {}: {error}",
                path.display()
            ),
        })
    }

    async fn save_pending_runtime_notes(
        &self,
        chat_id: &str,
        pending: &PendingRuntimeNotes,
    ) -> Result<()> {
        write_json_atomic(&self.pending_runtime_notes_path(chat_id), pending).await
    }

    fn message_chunk_path(&self, chat_id: &str, chunk_size: u64, chunk_start: u64) -> PathBuf {
        self.messages_dir(chat_id)
            .join(message_chunk_file_name(chunk_start, chunk_size))
    }

    fn summary_checkpoint_path(&self, chat_id: &str, checkpoint_id: &str) -> PathBuf {
        self.chat_dir(chat_id).join("checkpoints").join(format!(
            "{}.json",
            sanitize_chat_path_component(checkpoint_id)
        ))
    }

    pub fn history_replacement_path(&self, chat_id: &str, record_id: &str) -> PathBuf {
        self.chat_dir(chat_id)
            .join("history_replacements")
            .join(format!("{}.json", sanitize_chat_path_component(record_id)))
    }

    pub fn history_replacement_tombstone_path(&self, chat_id: &str, record_id: &str) -> PathBuf {
        self.chat_dir(chat_id)
            .join(".history_replacement_tombstones")
            .join(format!("{}.json", sanitize_chat_path_component(record_id)))
    }

    pub async fn list_chat_headers(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ChatSessionHeader>> {
        if !self.sessions_dir.exists() {
            return Ok(vec![]);
        }

        let mut entries =
            tokio::fs::read_dir(&self.sessions_dir)
                .await
                .map_err(|e| rua_core::RuaError::Io {
                    message: format!(
                        "failed to read sessions dir {}: {e}",
                        self.sessions_dir.display()
                    ),
                })?;

        let mut headers = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| rua_core::RuaError::Io {
                message: format!("failed to read directory entry: {e}"),
            })?
        {
            let path = entry.path();
            if !entry
                .file_type()
                .await
                .map(|ty| ty.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let header_path = path.join("session.json");
            if !header_path.exists() {
                continue;
            }
            let content = tokio::fs::read_to_string(&header_path).await.map_err(|e| {
                rua_core::RuaError::Io {
                    message: format!(
                        "failed to read chat session header {}: {e}",
                        header_path.display()
                    ),
                }
            })?;
            let header: ChatSessionHeader =
                serde_json::from_str(&content).map_err(|e| rua_core::RuaError::Io {
                    message: format!(
                        "failed to parse chat session header {}: {e}",
                        header_path.display()
                    ),
                })?;
            headers.push(header);
        }

        headers.sort_by(|a, b| {
            b.metadata
                .pinned
                .cmp(&a.metadata.pinned)
                .then_with(|| {
                    b.metadata
                        .last_activity_at
                        .cmp(&a.metadata.last_activity_at)
                })
                .then_with(|| a.chat_id.cmp(&b.chat_id))
        });

        Ok(headers.into_iter().skip(offset).take(limit).collect())
    }
}

pub fn sanitize_filename(name: &str) -> String {
    let mut result = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => result.push_str("--"),
            _ => result.push(c),
        }
    }
    match result.as_str() {
        "" => "--empty--".to_string(),
        "." => "--dot--".to_string(),
        ".." => "--dotdot--".to_string(),
        _ => result,
    }
}

fn sanitize_chat_path_component(name: &str) -> String {
    sanitize_filename(name)
}

fn chunk_start_sequence(sequence: u64, chunk_size: u64) -> u64 {
    ((sequence.saturating_sub(1)) / chunk_size) * chunk_size + 1
}

fn message_chunk_file_name(chunk_start: u64, chunk_size: u64) -> String {
    let chunk_ordinal = (chunk_start.saturating_sub(1)) / chunk_size;
    format!("{chunk_ordinal:012}.jsonl")
}

fn active_message_log_end_sequence(header: &ChatSessionHeader, index: &TurnIndex) -> u64 {
    index
        .turns
        .last()
        .map(|turn| turn.last_sequence)
        .unwrap_or_default()
        .max(header.message_log.latest_sequence)
}

fn append_message_chunk_sync(
    messages_dir: &Path,
    chunk_start: u64,
    chunk_size: u64,
    entries: &[MessageLogEntry],
) -> Result<()> {
    let path = messages_dir.join(message_chunk_file_name(chunk_start, chunk_size));
    let mut bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(rua_core::RuaError::Io {
                message: format!("failed to read message chunk {}: {error}", path.display()),
            });
        }
    };

    for entry in entries {
        serde_json::to_writer(&mut bytes, entry).map_err(|e| rua_core::RuaError::Io {
            message: format!("failed to serialize message entry {}: {e}", path.display()),
        })?;
        bytes.push(b'\n');
    }
    write_bytes_atomic_sync(&path, &bytes)
}

fn read_latest_message_entry_sync(path: &Path) -> Result<Option<MessageLogEntry>> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(rua_core::RuaError::Io {
                message: format!("failed to read message chunk {}: {error}", path.display()),
            });
        }
    };
    let mut position = file
        .seek(SeekFrom::End(0))
        .map_err(|error| rua_core::RuaError::Io {
            message: format!(
                "failed to seek latest message in {}: {error}",
                path.display()
            ),
        })?;
    let mut segments = Vec::new();
    let mut found_content = false;
    while position > 0 {
        let start = position.saturating_sub(8 * 1024);
        let mut block = vec![0; (position - start) as usize];
        file.seek(SeekFrom::Start(start))
            .and_then(|_| file.read_exact(&mut block))
            .map_err(|error| rua_core::RuaError::Io {
                message: format!(
                    "failed to read latest message from {}: {error}",
                    path.display()
                ),
            })?;
        let mut end = block.len();
        if !found_content {
            while end > 0 && matches!(block[end - 1], b'\n' | b'\r') {
                end -= 1;
            }
            if end == 0 {
                position = start;
                continue;
            }
            found_content = true;
        }
        if let Some(newline) = block[..end].iter().rposition(|byte| *byte == b'\n') {
            segments.push(block[newline + 1..end].to_vec());
            break;
        }
        segments.push(block[..end].to_vec());
        position = start;
    }
    if !found_content {
        return Ok(None);
    }
    let mut latest = Vec::with_capacity(segments.iter().map(Vec::len).sum());
    for segment in segments.into_iter().rev() {
        latest.extend_from_slice(&segment);
    }
    let entry: MessageLogEntry =
        serde_json::from_slice(&latest).map_err(|error| rua_core::RuaError::Io {
            message: format!(
                "failed to parse latest message from {}: {error}",
                path.display()
            ),
        })?;
    Ok(Some(entry))
}

fn read_latest_persisted_message_entry_sync(
    messages_dir: &Path,
    chunk_size: u64,
) -> Result<Option<MessageLogEntry>> {
    let mut latest_chunk_start = None;
    let entries = match std::fs::read_dir(messages_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "failed to read message log directory {}: {error}",
                    messages_dir.display()
                ),
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| rua_core::RuaError::Io {
            message: format!(
                "failed to inspect message log directory {}: {error}",
                messages_dir.display()
            ),
        })?;
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(chunk_ordinal) = stem.parse::<u64>() else {
            continue;
        };
        let chunk_start = chunk_ordinal.saturating_mul(chunk_size).saturating_add(1);
        latest_chunk_start = Some(latest_chunk_start.unwrap_or(0).max(chunk_start));
    }
    let Some(chunk_start) = latest_chunk_start else {
        return Ok(None);
    };
    let path = messages_dir.join(message_chunk_file_name(chunk_start, chunk_size));
    read_latest_message_entry_sync(&path)
}

fn turn_metadata_from_messages(
    entries: &[MessageLogEntry],
) -> serde_json::Map<String, serde_json::Value> {
    let mut metadata = serde_json::Map::new();
    if let Some(message) = entries.first() {
        for key in [
            "authority_device_id",
            "authority_epoch",
            "turn_agent_assignment",
            "turn_model",
            "turn_thinking",
            rua_core::CHAT_COLLABORATION_MODE_META_KEY,
        ] {
            if let Some(value) = message.metadata.get(key) {
                metadata.insert(key.to_string(), value.clone());
            }
        }
    }
    if let Some(origin) = entries
        .iter()
        .filter_map(|entry| entry.metadata.get("origin"))
        .find(|origin| origin_kind_is_acp(Some(origin)))
    {
        metadata.insert("origin".to_string(), origin.clone());
    }
    if !entries.is_empty() && entries.iter().all(message_is_ui_only) {
        metadata.insert("ui_only".to_string(), Value::Bool(true));
        metadata.insert("model_visible".to_string(), Value::Bool(false));
    }
    metadata
}

fn foldable_message_ranges(messages: &[MessageLogEntry], first_sequence: u64) -> Vec<(u64, u64)> {
    let mut ranges = Vec::new();
    let mut range_start = None;
    let mut open_tool_calls = HashSet::new();

    let mut messages = messages
        .iter()
        .filter(|message| message.sequence >= first_sequence)
        .filter(|message| message_is_model_projectable(message))
        .peekable();
    while let Some(message) = messages.next() {
        if message.status != "completed" {
            break;
        }

        let mut tool_use_ids = Vec::new();
        let mut tool_result_ids = Vec::new();
        for part in message_content_parts(&message.content) {
            match part.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    let Some(call_id) = tool_use_call_id(part) else {
                        return ranges;
                    };
                    tool_use_ids.push(call_id);
                }
                Some("tool_result") => {
                    let Some(call_id) = tool_result_call_id(part) else {
                        return ranges;
                    };
                    tool_result_ids.push(call_id);
                }
                _ => {}
            }
        }

        if range_start.is_none() {
            range_start = Some(message.sequence);
        }
        for call_id in tool_use_ids {
            if !open_tool_calls.insert(call_id.to_string()) {
                return ranges;
            }
        }
        for call_id in tool_result_ids {
            if !open_tool_calls.remove(call_id) {
                return ranges;
            }
        }

        let identity_key =
            agent_types::conversation_message_identity::MODEL_DIALOGUE_ITEM_ID_METADATA_KEY;
        let response_continues = message.role == "assistant"
            && messages.peek().is_some_and(|next| {
                next.role == "assistant"
                    && message
                        .metadata
                        .get(identity_key)
                        .and_then(Value::as_str)
                        .is_some_and(|id| {
                            next.metadata.get(identity_key).and_then(Value::as_str) == Some(id)
                        })
            });
        if open_tool_calls.is_empty() && !response_continues {
            ranges.push((
                range_start.expect("range starts at the current message"),
                message.sequence,
            ));
            range_start = None;
        }
    }

    ranges
}

fn merge_ranges_covering_history_replacements(
    mut ranges: Vec<(u64, u64)>,
    records: &[HistoryReplacementRecord],
) -> Vec<(u64, u64)> {
    for record in records {
        let source = &record.source.sequence_range;
        let Some(first) = ranges.iter().position(|(start, end)| {
            *start <= source.start_sequence && *end >= source.start_sequence
        }) else {
            continue;
        };
        let Some(last) = ranges
            .iter()
            .rposition(|(start, end)| *start <= source.end_sequence && *end >= source.end_sequence)
        else {
            continue;
        };
        if first >= last {
            continue;
        }
        let merged = (ranges[first].0, ranges[last].1);
        ranges.splice(first..=last, [merged]);
    }
    ranges
}

fn message_content_parts(content: &Value) -> Vec<&Value> {
    match content {
        Value::Array(parts) => parts.iter().collect(),
        other => vec![other],
    }
}

fn tool_use_call_id(part: &Value) -> Option<&str> {
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

fn validate_sequence_range(name: &str, range: &CheckpointSequenceRange) -> Result<()> {
    if range.start_sequence == 0 || range.start_sequence > range.end_sequence {
        return Err(rua_core::RuaError::Io {
            message: format!(
                "checkpoint {name} sequence range is invalid: {}..={}",
                range.start_sequence, range.end_sequence
            ),
        });
    }
    Ok(())
}

fn origin_kind_is_acp(origin: Option<&serde_json::Value>) -> bool {
    origin
        .and_then(|origin| origin.get("kind"))
        .and_then(|kind| kind.as_str())
        == Some("acp")
}

fn message_preview_text(entry: &MessageLogEntry) -> Option<String> {
    if message_is_hidden_model_marker(entry) {
        return None;
    }
    conversation_message_text(entry)
}

fn conversation_recall_query_terms(query: &str) -> Vec<String> {
    let mut terms = query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if terms.is_empty() {
        let trimmed = query.trim();
        if !trimmed.is_empty() {
            terms.push(trimmed.to_string());
        }
    }
    terms.sort();
    terms.dedup();
    terms
}

fn conversation_message_text(entry: &MessageLogEntry) -> Option<String> {
    if crate::runtime_note::RuntimeNote::from_message_log_entry(entry).is_none()
        && message_is_hidden_model_marker(entry)
    {
        return None;
    }
    match &entry.content {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => parts.iter().find_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        }),
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        _ => None,
    }
}

fn message_is_hidden_model_marker(entry: &MessageLogEntry) -> bool {
    message_is_ui_only(entry)
        || entry
            .metadata
            .get("hidden_from_ui")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || entry
            .metadata
            .get("model_visible_marker")
            .and_then(Value::as_str)
            .is_some_and(|marker| marker == "memory_signal")
        || message_text_contains_marker(entry, "<memory_signal>")
}

fn message_is_ui_only(entry: &MessageLogEntry) -> bool {
    entry.role == "ui"
        || entry
            .metadata
            .get("ui_only")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || entry
            .metadata
            .get("model_visible")
            .and_then(Value::as_bool)
            .is_some_and(|visible| !visible)
}

fn message_is_model_projectable(entry: &MessageLogEntry) -> bool {
    crate::runtime_note::RuntimeNote::from_message_log_entry(entry)
        .map(|note| note.is_model_visible())
        .unwrap_or_else(|| !message_is_ui_only(entry))
}

fn message_text_contains_marker(entry: &MessageLogEntry, marker: &str) -> bool {
    match &entry.content {
        Value::String(text) => text.contains(marker),
        Value::Array(parts) => parts.iter().any(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains(marker))
        }),
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains(marker)),
        _ => false,
    }
}

fn estimate_message_tokens(messages: &[MessageLogEntry]) -> u64 {
    estimate_message_tokens_with_divisor(messages, 4.0)
}

fn estimate_message_tokens_with_divisor(messages: &[MessageLogEntry], divisor: f64) -> u64 {
    messages
        .iter()
        .map(|message| rua_core::token_estimate::estimate_json_tokens(&message.content, divisor))
        .sum()
}

fn apply_history_replacement_records(
    messages: &mut Vec<MessageLogEntry>,
    records: &[HistoryReplacementRecord],
) -> usize {
    apply_ready_history_replacements_with_evidence(messages, records)
        .into_iter()
        .flatten()
        .count()
}

fn validate_history_replacement_source(
    source: &crate::HistoryReplacementSource,
    messages: &[MessageLogEntry],
) -> Result<()> {
    let mut source_message_ids = Vec::new();
    for message in messages {
        if source
            .message_ids
            .iter()
            .any(|message_id| message_id == &message.message_id)
            && source_message_ids.last() != Some(&message.message_id)
        {
            source_message_ids.push(message.message_id.clone());
        }
    }
    if source_message_ids != source.message_ids {
        return Err(rua_core::RuaError::Io {
            message: "history replacement source messages no longer match selected history"
                .to_string(),
        });
    }
    for anchor in [&source.start_anchor, &source.end_anchor] {
        let Some(message) = messages
            .iter()
            .find(|message| message.message_id == anchor.message_id)
        else {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "history replacement anchor message {} is missing",
                    anchor.message_id
                ),
            });
        };
        let part_count = message.content.as_array().map(Vec::len).unwrap_or(1);
        if anchor.content_part_index >= part_count {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "history replacement anchor {}:{} is out of bounds",
                    anchor.message_id, anchor.content_part_index
                ),
            });
        }
    }
    Ok(())
}

fn early_background_compaction_key(store: &WorkspaceSessionStore, chat_id: &str) -> String {
    let workspace_dir = store
        .sessions_dir()
        .parent()
        .unwrap_or_else(|| store.sessions_dir());
    format!("{}::{chat_id}", workspace_dir.display())
}

fn checkpoint_checksum(checkpoint: &SummaryCheckpoint) -> Result<String> {
    let mut payload = checkpoint.clone();
    payload.checksum.clear();
    let bytes = serde_json::to_vec(&payload).map_err(|e| rua_core::RuaError::Io {
        message: format!(
            "failed to serialize summary checkpoint {} for checksum: {e}",
            checkpoint.checkpoint_id
        ),
    })?;
    let digest = Sha256::digest(&bytes);
    Ok(format!("sha256:{}", hex_digest(&digest)))
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

async fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let path = path.to_path_buf();
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| rua_core::RuaError::Io {
        message: format!(
            "failed to serialize chat session header {}: {e}",
            path.display()
        ),
    })?;
    tokio::task::spawn_blocking(move || write_bytes_atomic_sync(&path, &bytes))
        .await
        .map_err(|e| rua_core::RuaError::Io {
            message: format!("failed to join atomic write task: {e}"),
        })?
}

fn write_bytes_atomic_sync(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| rua_core::RuaError::Io {
        message: format!("path has no parent: {}", path.display()),
    })?;
    std::fs::create_dir_all(parent).map_err(|e| rua_core::RuaError::Io {
        message: format!("failed to create parent dir {}: {e}", parent.display()),
    })?;

    let temp_path = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("session.json"),
        std::process::id(),
        uuid::Uuid::new_v4()
    ));

    let write_result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp_path, path)?;
        sync_parent_dir_sync(parent)?;
        Ok(())
    })();

    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(rua_core::RuaError::Io {
            message: format!("failed to atomically write {}: {error}", path.display()),
        });
    }

    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir_sync(parent: &Path) -> std::io::Result<()> {
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_dir_sync(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

static CHAT_WRITE_LOCKS: OnceLock<StdMutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();

fn chat_write_lock_for_path(path: PathBuf) -> Arc<tokio::sync::Mutex<()>> {
    let locks = CHAT_WRITE_LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .expect("chat write lock registry mutex poisoned");
    locks
        .entry(path)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}
