pub mod agent_tools_cells;
pub mod annotations;
pub mod builtin;
pub mod config_store;
pub mod context;
pub mod core;
pub mod error;
pub mod executor;
pub mod extended;
pub mod fs;
pub mod mcp;
pub mod platform;
mod process_tree;
pub mod raw_capture;
pub mod registry;
pub mod result_store;
pub mod schema;
pub mod task_runner;
pub mod tool_config_secret_io;

pub use agent_tools_cells::{AgentToolsCell, AgentToolsCells};
pub use annotations::{
    foreground_wait_limit, tool_transfer_contract, ApprovalPolicy, ExtendedToolAnnotations,
    ToolTransferContract, ToolTransferPolicy, TOOL_TRANSFER_CONTRACT_METADATA_KEY,
};
pub use config_store::{
    ToolConfigError, ToolConfigStore, ToolInfo as ToolStoreInfo, ToolSelectionPolicyPreview,
};
pub use context::{SecurityContext, ToolExecutionContext, ToolExecutionLimits};
pub use core::{EditFileTool, ReadFileTool, ShellTool};
pub use error::ToolError;
pub use executor::{
    default_command_executor, CommandError, CommandExecutor, CommandOutput, CommandRequest,
    DisabledCommandExecutor, MockCommandExecutor, MockResponse, RawCommandCapture,
    RuntimeCommandExecutor, ShellKind, ShellSpec,
};
pub use fs::WorkspaceFs;
pub use registry::{tool_catalog_from_specs, ToolNamespace, ToolRegistry};
pub use result_store::{
    recall_key, NoOpResultStore, ToolCallArchive, ToolResultBlob, ToolResultOutputProcessor,
    ToolResultStore,
};
pub use rua_task::{TaskOwner, TaskRunnerSession};
pub use schema::{validate_args, ToolSpec};
pub use task_runner::{
    adapt_tool_task_result, adopt_running_tool, AdoptRunningToolRequest, RunningToolResultFuture,
    ToolTaskExecution, ToolTaskExecutionRequest, ToolTaskRunner,
};

pub use builtin::{register_all_builtin_tools, RuntimeGetRuntimeInfoTool};

pub use extended::register_extended_tools;
pub use platform::ToolProfile;

pub fn repair_and_validate(
    tool_name: &str,
    raw_args: &str,
    schema: &serde_json::Value,
) -> Result<(serde_json::Value, rua_json_repair::RepairOutcome), ToolError> {
    let effective_args = if raw_args.trim().is_empty() {
        "{}"
    } else {
        raw_args
    };

    let outcome =
        rua_json_repair::repair_json(effective_args).map_err(|e| ToolError::JsonRepairFailed {
            tool: tool_name.to_string(),
            message: e.to_string(),
        })?;

    validate_args(tool_name, &outcome.value, schema)?;

    Ok((outcome.value.clone(), outcome))
}
