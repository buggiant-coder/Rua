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

pub fn product_mcp_sources(
    statuses: &[mcp_manager::status::McpServerStatus],
) -> std::collections::HashMap<String, String> {
    statuses
        .iter()
        .filter_map(|status| {
            let capability = match status.transport_type.as_str() {
                "bundled_browser" => "browser_use",
                "bundled_computer" => "computer_use",
                _ => return None,
            };
            Some((status.key.clone(), capability.to_string()))
        })
        .collect()
}

pub fn register_discovered_mcp_tools(
    toolkit: &mut AgentToolkit,
    tools: Vec<rua_mcp::bridge::McpBridgedTool>,
    product_sources: &std::collections::HashMap<String, String>,
) -> usize {
    let count = tools.len();
    for tool in tools {
        let server = tool.server_id().to_string();
        let original_name = tool.original_name().to_string();
        let canonical_name = rua_contract::Tool::name(&tool).to_string();
        register_mcp_tools(toolkit, vec![Arc::new(tool)]);
        toolkit.set_source_identity(
            &canonical_name,
            server.clone(),
            original_name,
            product_sources.get(&server).cloned(),
        );
    }
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
