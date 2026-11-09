use std::sync::Arc;

use agent_types::ToolDefinition;
use react_agent::AgentToolkit;
use tracing::debug;

pub fn register_mcp_tools(
    toolkit: &mut AgentToolkit,
    tools: Vec<Arc<dyn rua_contract::Tool>>,
) -> usize {
    let count = tools.len();
    for tool in tools {
        let schema = tool.schema();
        let definition = ToolDefinition {
            name: schema.name,
            description: schema.description,
            parameters: schema.parameters,
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        };
        let disclosure = crate::tool_disclosure::mcp_tool_disclosure(None);
        let executor: Arc<dyn react_agent::toolkit::ToolExecutor> = Arc::new(
            react_agent::toolkit::ContractToolExecutor::new(tool)
                .with_schema(definition.parameters.clone()),
        );
        toolkit.register_with_disclosure(definition, executor, disclosure);
    }
    debug!(count, "registered MCP tools into agent toolkit");
    count
}

pub fn build_core_toolkit_with_recall(
    executor: Arc<dyn rua_tool::CommandExecutor>,
    profile: rua_tool::ToolProfile,
) -> Arc<AgentToolkit> {
    let mut registry = rua_tool::ToolRegistry::new();
    if let Err(e) = rua_tool::register_all_builtin_tools(&mut registry, executor, None) {
        tracing::warn!("Failed to register builtin tools: {e}");
    }
    if let Err(e) = rua_tool::register_extended_tools(&mut registry, profile) {
        tracing::warn!("Failed to register extended tools: {e}");
    }
    let mut toolkit = AgentToolkit::new();
    crate::tool_disclosure::populate_toolkit_from_registry(&mut toolkit, &registry);
    Arc::new(toolkit)
}
