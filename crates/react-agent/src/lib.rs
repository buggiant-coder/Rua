#![forbid(unsafe_code)]

pub mod agent;
pub mod config;
pub mod context;
pub mod error;
pub mod hooks;
pub mod skill_tool;
pub mod snip;
pub mod tool_accumulator;
pub mod tool_before;
pub mod tool_disclosure;
pub mod tool_observer;
pub mod tool_result_compressor;
pub mod toolkit;

pub mod runner;

pub mod loop_runner;
pub mod model_call_context;
pub mod permission_request;
pub mod react_loop;

pub mod test_support;

pub use agent::{AgentEventStream, ReactAgent, ReactAgentBuilder};
pub use config::ReactConfig;
pub use context::AgentContext;
pub use error::AgentError;
pub use hooks::ApprovalResult;
pub use hooks::{AgentHook, HookEvent, NoopHook, RecordingHook};
pub use loop_runner::SkillEnvHook;
pub use model_call_context::{
    estimate_model_visible_conversation_tokens, estimate_model_visible_input_tokens_with_messages,
    replace_model_call_section, ModelCallContext, ModelCallProjector, ModelCallSection,
    ModelCallSectionKind, ModelInputTokenEstimate, ModelInputTokenEstimateMethod,
    MODEL_CALL_CONTEXT_METADATA_KEY, MODEL_CALL_CONTEXT_SECTION_METADATA_KEY,
    MODEL_CALL_ID_METADATA_KEY, MODEL_INPUT_TOKEN_ESTIMATE_METADATA_KEY,
};
pub use permission_request::{
    argument_digest, PermissionHookDecision, PermissionHookFailure, PermissionRequestBoundary,
    PermissionRequestInvocation,
};
pub use react_loop::{
    has_media_blocks, is_multimodal_error, is_multimodal_error_message,
    strip_media_blocks_from_memory, TokenBudget,
};
pub use rua_security::SecurityVerdict as ToolDecision;
pub use runner::{agent_event_to_runtime_event, agent_event_to_runtime_events};
pub use skill_tool::{
    build_skills_prompt_block, register_skill_pseudo_tool, SkillInfo, SkillRuntimeSource,
};
pub use tool_accumulator::ToolCallAccumulator;
pub use tool_before::{
    ToolBeforeBoundary, ToolBeforeFailure, ToolBeforeInvocation, ToolBeforeOutcome,
};
pub use tool_disclosure::{ToolDisclosureConfig, ToolDisclosureMode};
pub use tool_observer::{
    ToolObservationEvent, ToolObservationInvocation, ToolObserverBoundary,
    ToolObserverContribution, ToolObserverFailure,
};
pub use toolkit::AgentToolkitHandle;
pub use toolkit::{
    AgentToolkit, AllowAllPolicy, ApprovalChannel, ApprovalOutcome, ContractToolExecutor,
    FailingToolExecutor, FatalToolExecutor, GuardedToolExecutor, ListBasedPolicy, McpToolRegistry,
    MockToolExecutor, ToolExecutor, ToolPolicy, ToolPolicyProvider,
};
