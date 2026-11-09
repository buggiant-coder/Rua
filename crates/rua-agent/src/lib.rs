#![forbid(unsafe_code)]

pub mod cells;
pub mod collaboration_mode;
pub mod command;
pub mod command_adapters;
pub mod compaction;
pub mod context;
pub mod context_checker;
pub mod context_manager;
pub mod error;
pub mod hooks;
pub mod llm_summary;
pub mod media;
pub mod memory_delete_tool;
pub mod memory_get_tool;
pub mod memory_manager;
pub mod memory_search_tool;
pub mod memory_signal;
pub mod memory_signal_note;
pub mod memory_tools;
pub mod memory_update_tool;
pub mod memory_write_tool;
pub mod model_call_context;
pub mod model_selection;
pub mod plugin_tool_handler;
pub mod profile;
pub mod profile_store;
pub mod projection_preparation;
mod projection_tool_results;
pub mod prompt;
pub mod reactive_compaction;
pub mod registry;
pub mod runtime;
pub mod summary_checkpoint;
pub mod task_runtime_notes;
pub mod tool_disclosure;
pub mod turn_persistence;

pub mod acp_chat_controller;
pub mod toolkit;

pub use cells::{AgentCell, AgentCells, AgentSectionCell};
pub use command::PendingAction;
pub use compaction::{
    BoundaryCalculator, BoundaryPlan, CompactionAudit, CompactionConfig, CompactionGrade,
    CompactionResult, ConversationCompactor, DeviceState, HardTruncator, MobileCompactionPolicy,
    MockSummaryGenerator, SummaryGenerator,
};
pub use context::{load_workspace_context, WorkspaceContext};
pub use context_checker::{
    BpeEstimator, ContextCheckReason, ContextCheckResult, ContextChecker, ContextLimit,
    EstimationMethod, HeuristicEstimator, TokenEstimate, TokenEstimator, ToolSpec,
};
pub use context_manager::{
    CompactContextRequest, CompactContextResult, ContextManager, ContextSummaryRequest,
    ContextSummaryResult,
};
pub use error::{AgentError, AgentErrorKind};
pub use hooks::{
    bootstrap::BootstrapContextContributor, plugin_hook::PluginHook, CompositeHook,
    SecurityAgentHook, UsageAgentHook,
};
pub use llm_summary::LlmSummaryGenerator;
pub use media::{FileBlockSupportFormatter, MediaLimits, MediaProcessor, MediaType};
pub use memory_delete_tool::MemoryDeleteTool;
pub use memory_get_tool::MemoryGetTool;
pub use memory_manager::{
    InMemoryMemoryManager, LlmMemoryManager, MemoryManager, NoOpMemoryManager,
};
pub use memory_search_tool::MemorySearchTool;
pub use memory_update_tool::MemoryUpdateTool;
pub use memory_write_tool::MemoryWriteTool;
pub use model_selection::{
    DefaultModelConfig, DefaultProviderInfo, ModelSelection, ModelSelectionSource, ModelSelector,
};
pub use profile::{AgentProfile, AttachmentRef, ModelSlotRef, SecurityContext};
pub use profile_store::AgentProfileStore;
pub use prompt::{PromptBuilder, PromptVariables};
pub use reactive_compaction::ContextOverflowDetector;
pub use registry::{AgentRegistry, FileAgentRegistry, MockAgentRegistry, ResolvedAgent};
pub use runtime::ReactAgentRuntime;

pub use toolkit::{build_core_toolkit_with_recall, register_mcp_tools};
