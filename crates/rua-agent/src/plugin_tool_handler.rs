use std::sync::Arc;

use agent_types::{ToolCall, ToolDefinition, ToolResult};
use async_trait::async_trait;
use react_agent::toolkit::ToolExecutor;
use react_agent::AgentToolkit;
use rua_core::tool::ToolContext;
use rua_plugin_runtime::{ActivePluginRegistry, PluginRegistry};

pub struct PluginToolHandler {
    _registry: Arc<PluginRegistry>,
    plugin_id: String,
    tool_name: String,
}

impl PluginToolHandler {
    pub fn new(registry: Arc<PluginRegistry>, plugin_id: String, tool_name: String) -> Self {
        Self {
            _registry: registry,
            plugin_id,
            tool_name,
        }
    }
}

#[async_trait]
impl ToolExecutor for PluginToolHandler {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        Err(react_agent::AgentError::Tool {
            tool: call.name.clone(),
            message: format!(
                "Legacy WASM tool invocation removed. Plugin '{}' tool '{}' should use subprocess runtime.",
                self.plugin_id, self.tool_name
            ),
        })
    }
}

pub fn populate_plugin_tools(toolkit: &mut AgentToolkit, registry: &Arc<PluginRegistry>) {
    for (plugin_id, spec) in registry.list_all_tools() {
        if !registry.is_plugin_enabled(&plugin_id) {
            continue;
        }
        let definition = ToolDefinition {
            name: format!("plugin__{}__{}", plugin_id, spec.name),
            description: spec.description.clone(),
            parameters: spec.parameters.clone(),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        };
        let disclosure = crate::tool_disclosure::plugin_tool_disclosure(&plugin_id);
        let executor: Arc<dyn ToolExecutor> = Arc::new(PluginToolHandler::new(
            Arc::clone(registry),
            plugin_id,
            spec.name.clone(),
        ));
        toolkit.register_with_disclosure(definition, executor, disclosure);
    }
}

pub struct PluginClientToolHandler {
    active_registry: Arc<ActivePluginRegistry>,
    plugin_id: String,
    tool_name: String,
}

impl PluginClientToolHandler {
    pub fn new(
        active_registry: Arc<ActivePluginRegistry>,
        plugin_id: String,
        tool_name: String,
    ) -> Self {
        Self {
            active_registry,
            plugin_id,
            tool_name,
        }
    }
}

#[async_trait]
impl ToolExecutor for PluginClientToolHandler {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        let call_tool_name = call.name.clone();

        let result = self
            .active_registry
            .call_plugin_tool(&self.plugin_id, &self.tool_name, call.arguments.clone())
            .await
            .map_err(|e| react_agent::AgentError::Tool {
                tool: call_tool_name,
                message: e.to_string(),
            })?;

        Ok(ToolResult::text(&call.id, result.text()))
    }
}

pub async fn populate_active_plugin_tools(
    toolkit: &mut AgentToolkit,
    active_registry: &Arc<ActivePluginRegistry>,
) {
    let tools = active_registry.list_all_tools().await;
    for (plugin_id, tool_info) in tools {
        let definition = ToolDefinition {
            name: format!("plugin__{}__{}", plugin_id, tool_info.name),
            description: tool_info.description.clone(),
            parameters: tool_info.input_schema.clone(),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        };
        let disclosure = crate::tool_disclosure::plugin_tool_disclosure(&plugin_id);
        let executor: Arc<dyn ToolExecutor> = Arc::new(PluginClientToolHandler::new(
            Arc::clone(active_registry),
            plugin_id,
            tool_info.name.clone(),
        ));
        toolkit.register_with_disclosure(definition, executor, disclosure);
    }
}
