use std::sync::Arc;

use async_trait::async_trait;
use react_agent::toolkit::{ContractToolExecutor, ToolExecutor};
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_tool::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use rua_tool::schema::ToolSpec;
use serde::Deserialize;

use crate::memory_manager::MemoryManager;

#[derive(Debug, Deserialize)]
struct MemoryDeleteArgs {
    id: rua_memory::MemoryId,
}

pub struct MemoryDeleteTool {
    memory_manager: Arc<dyn MemoryManager>,
}

impl MemoryDeleteTool {
    pub fn new(memory_manager: Arc<dyn MemoryManager>) -> Self {
        Self { memory_manager }
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "memory_delete".to_string(),
            description: concat!(
                "Delete one long-term Memory record by id. ",
                "Use only after memory_search and, when needed, memory_get identify the record the user wants forgotten. ",
                "This only deletes Memory owned by the current Agent."
            )
            .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Memory record id to delete."
                    }
                },
                "required": ["id"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: false,
                destructive: true,
                idempotent: true,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

#[async_trait]
impl Tool for MemoryDeleteTool {
    fn name(&self) -> &str {
        "memory_delete"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let args: MemoryDeleteArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "memory_delete".into(),
                message: e.to_string(),
            })?;
        let id = args.id.to_string();
        let deleted = self
            .memory_manager
            .delete_memory_for_agent(&ctx.agent_id, &id)
            .await
            .map_err(|e| rua_core::RuaError::Tool {
                tool: "memory_delete".into(),
                message: format!("memory delete failed: {e}"),
            })?;

        let text = if deleted {
            format!("memory deleted: id={id}")
        } else {
            format!("memory not deleted: id={id}")
        };
        Ok(ToolResult::text("memory_delete", text))
    }
}

pub fn register_memory_delete_tool(
    toolkit: &mut react_agent::AgentToolkit,
    memory_manager: Arc<dyn MemoryManager>,
) {
    if toolkit.has_tool("memory_delete") {
        return;
    }

    let definition = MemoryDeleteTool::spec().to_tool_definition();
    let tool: Arc<dyn Tool> = Arc::new(MemoryDeleteTool::new(memory_manager));
    let executor: Arc<dyn ToolExecutor> =
        Arc::new(ContractToolExecutor::new(tool).with_schema(definition.parameters.clone()));

    toolkit.register_with_disclosure(
        definition,
        executor,
        crate::tool_disclosure::builtin_tool_disclosure("memory_delete"),
    );
}
