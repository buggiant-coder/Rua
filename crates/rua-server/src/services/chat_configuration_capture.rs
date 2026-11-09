use rua_chat::TurnAgentAssignment;
use rua_config::ChatRunningConfig;
use rua_security::SecurityModule;
use rua_workspace::WorkspaceRuntime;
use serde_json::Value;
use std::path::Path;

use super::product_hooks::ChatConfigurationMaterialization;

/// Chat-owned choices supplied by the operation requesting a new candidate.
pub(crate) struct ChatConfigurationSelection {
    pub model: Option<Value>,
    pub thinking: Option<Value>,
    pub collaboration_mode: Value,
    pub harness_configuration: Option<Value>,
    pub authority_device_id: String,
}

/// Captures resources without persisting or adopting the candidate configuration.
pub(crate) async fn capture_chat_configuration(
    runtime: &WorkspaceRuntime,
    security: &SecurityModule,
    tools: Option<&Value>,
    assignment: TurnAgentAssignment,
    running_config: ChatRunningConfig,
    project_root: Option<&Path>,
    selection: ChatConfigurationSelection,
) -> Result<ChatConfigurationMaterialization, String> {
    let security_config = security
        .config_store()
        .load(&assignment.agent_id)
        .map_err(|error| error.to_string())?;
    let capability_configuration = runtime
        .capability_configuration(project_root)
        .await
        .map_err(|error| error.to_string())?;
    let prompt_file_content = super::console_chat::prompt_file_fragment_for_entries(
        &runtime.workspace_dir(),
        &assignment.system_prompt_files,
    );
    Ok(ChatConfigurationMaterialization {
        selected_agent_id: assignment.agent_id.clone(),
        security_config,
        capability_configuration: Some(capability_configuration),
        tool_config_snapshot: super::console_chat::snapshot_tool_configuration(tools),
        prompt_file_content,
        agent_assignment: Some(assignment),
        running_config,
        model: selection.model,
        thinking: selection.thinking,
        collaboration_mode: selection.collaboration_mode,
        harness_configuration: selection.harness_configuration,
        authority_device_id: selection.authority_device_id,
    })
}
