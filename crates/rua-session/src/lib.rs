pub mod active_work_plan;
pub mod chat_deletion;
pub mod chat_index;
pub mod chat_media;
pub mod collaboration;
pub mod commands;
pub mod display_media;
pub mod early_background_compaction;
pub mod external_chat;
pub mod history_replacement;
pub mod model_switch;
pub mod projection;
pub mod runtime_note;
pub mod store;
pub mod tool_loop;
pub mod turn_writer;

pub use active_work_plan::{
    ActiveWorkPlanContent, ActiveWorkPlanSnapshot, ActiveWorkPlanStep, ActiveWorkPlanStepStatus,
    ACTIVE_WORK_PLAN_FORMAT_VERSION,
};
pub use commands::{handle_skills_command, SkillsCommandHandler, SkillsContext};
pub use early_background_compaction::{
    early_background_compaction_coordinator, EarlyBackgroundCompactionCoordinator,
    EarlyBackgroundCompactionDecision, EarlyBackgroundCompactionInput,
    EarlyBackgroundCompactionLease, EarlyBackgroundCompactionPolicy,
    EarlyBackgroundCompactionPriority,
};
pub use history_replacement::{
    derive_history_replacement_record_id, detect_execution_trace_blocks, estimate_text_tokens,
    evaluate_history_replacement_candidate, history_replacement_locator,
    history_replacement_version_is_superseded, validate_history_replacement_record,
    ExecutionTraceBlock, HistoryReplacementCandidateDecision, HistoryReplacementContentAnchor,
    HistoryReplacementReadyDraft, HistoryReplacementRecord, HistoryReplacementRecordState,
    HistoryReplacementRejectedDraft, HistoryReplacementRejectedGeneration,
    HistoryReplacementRejectionReason, HistoryReplacementSource, HistoryReplacementToolReference,
    HISTORY_REPLACEMENT_FORMAT_VERSION, HISTORY_REPLACEMENT_MIN_REDUCTION_PERCENT,
    HISTORY_REPLACEMENT_MIN_SAVED_TOKENS, HISTORY_REPLACEMENT_MIN_SOURCE_TOKENS,
    HISTORY_REPLACEMENT_PROMPT_VERSION,
};
pub use model_switch::{
    ChatModelSwitchState, ModelSwitchApplyError, ModelSwitchMaintenanceProof, ModelSwitchPreflight,
    ModelSwitchPreflightDecision, ModelSwitchPreflightInput,
};
pub use projection::{
    context_target_band_from_running, ContextTargetBand, ContextTargetBandOverride,
    ProjectionEmergencyTail, ProjectionEntryAction, ProjectionEntryMode, ProjectionFitStatus,
    ProjectionFoldFrontier, ProjectionMessageGroupEstimate, ProjectionPlan, ProjectionPlanner,
    ProjectionPlanningInput,
};
pub use runtime_note::{
    AgentsInstructionsRuntimeNote, CapabilityStateChange, CapabilityStateRuntimeNote,
    CollaborationResultRuntimeNote, ConversationRuntimeTerminalRuntimeNote,
    MemorySignalRuntimeNote, RuntimeNote, RuntimeNotePayload, TaskAttemptTerminalRuntimeNote,
    TaskWatchOutputProjection, TaskWatchOutputRecord, TaskWatchOutputRuntimeNote,
    RUNTIME_NOTE_MESSAGE_TYPE,
};
pub use store::{
    turn_origin_is_acp, AssistantResponseVariantRecord, ChatContextSnapshot, ChatMetadata,
    ChatPreview, ChatSessionHeader, ChatTag, CheckpointFrontier, CheckpointModelIdentity,
    CheckpointProjectionState, CheckpointRetainedSequenceRange, CheckpointSequenceRange,
    CheckpointTurnRange, ContextProjection, ContextProjectionRequest, ConversationReadSpanRequest,
    ConversationReadSpanResult, ConversationRecallHit, ConversationRecallMessage,
    ConversationRecallRequest, ConversationRecallResult, ConversationSpan, MessageLogEntry,
    MessageLogHeader, MessageProviderLink, MessageToolLink, NewConversationMessage,
    NewConversationTurn, ProjectionExclusions, SavedConversationSearchHit,
    SavedConversationSearchRequest, SavedConversationSearchResult, SummaryCheckpoint,
    SummaryCheckpointDraft, TurnIndex, TurnIndexHeader, TurnIndexRecord, TurnLifecycle,
    TurnLifecycleStatus, WorkspaceSessionStore, PROJECTION_EXCLUSIONS_FORMAT_VERSION,
};
pub use turn_writer::TurnWriter;

/// Device and native path captured for the Chat execution directory.
pub const EXECUTION_DIRECTORY_KEY: &str = "execution_directory";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChatExecutionDirectory {
    pub device_id: String,
    pub path: std::path::PathBuf,
    #[serde(default)]
    pub managed: bool,
    #[serde(default, flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl ChatExecutionDirectory {
    pub fn host_path(&self) -> rua_core::Result<std::path::PathBuf> {
        rua_core::runtime_layout::RuntimeLayout::from_current_process(None)
            .execution_host_path(&self.path)
    }
}

/// Immutable resource association metadata recorded on a claimed Project Turn.
pub const TURN_PROJECT_RESOURCES_KEY: &str = "turn_project_resources";

/// Ordered changes to the selected Agent of a materialized Chat.
pub const AGENT_SWITCH_EVENTS_KEY: &str = "agent_switch_events";
