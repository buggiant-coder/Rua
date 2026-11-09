use std::sync::Arc;

use async_trait::async_trait;
use react_agent::toolkit::{ContractToolExecutor, ToolExecutor};
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_tool::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use rua_tool::schema::ToolSpec;
use serde::Deserialize;

use crate::memory_manager::{MemoryGetRequest, MemoryManager};

#[derive(Debug, Deserialize)]
struct MemoryGetArgs {
    ids: Vec<rua_memory::MemoryId>,
    #[serde(default)]
    include_inactive: bool,
}

pub struct MemoryGetTool {
    memory_manager: Arc<dyn MemoryManager>,
}

impl MemoryGetTool {
    pub fn new(memory_manager: Arc<dyn MemoryManager>) -> Self {
        Self { memory_manager }
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "memory_get".to_string(),
            description: "Load complete long-term Memory records by id, including source evidence and links. Use after memory_search returns concise hits.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Memory record ids to load."
                    },
                    "include_inactive": {
                        "type": "boolean",
                        "description": "Whether to include superseded, expired, or deleted records."
                    }
                },
                "required": ["ids"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: true,
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
impl Tool for MemoryGetTool {
    fn name(&self) -> &str {
        "memory_get"
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
        let args: MemoryGetArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "memory_get".into(),
                message: e.to_string(),
            })?;

        let records = self
            .memory_manager
            .memory_get_for_agent(
                &ctx.agent_id,
                MemoryGetRequest {
                    ids: args.ids,
                    include_inactive: args.include_inactive,
                },
            )
            .await
            .map_err(|e| rua_core::RuaError::Tool {
                tool: "memory_get".into(),
                message: format!("memory get failed: {e}"),
            })?;

        Ok(ToolResult::text(
            "memory_get",
            format_memory_records(&records),
        ))
    }
}

pub fn register_memory_get_tool(
    toolkit: &mut react_agent::AgentToolkit,
    memory_manager: Arc<dyn MemoryManager>,
) {
    if toolkit.has_tool("memory_get") {
        return;
    }

    let definition = MemoryGetTool::spec().to_tool_definition();
    let tool: Arc<dyn Tool> = Arc::new(MemoryGetTool::new(memory_manager));
    let executor: Arc<dyn ToolExecutor> =
        Arc::new(ContractToolExecutor::new(tool).with_schema(definition.parameters.clone()));

    toolkit.register_with_disclosure(
        definition,
        executor,
        crate::tool_disclosure::builtin_tool_disclosure("memory_get"),
    );
}

fn format_memory_records(records: &[rua_memory::MemoryRecord]) -> String {
    if records.is_empty() {
        return "No memory records found.".to_string();
    }

    let mut out = format!("Memory records ({}):\n\n", records.len());
    for (idx, record) in records.iter().enumerate() {
        out.push_str(&format!(
            "{}. id: {}; owner_agent: {}; title: {}; kind: {:?}; scope: {:?}; status: {:?}; confidence: {:.3}\n   written_on_device: {} ({})\n   resource_device: {} ({})\n   {}\n",
            idx + 1,
            record.id,
            record.agent_id,
            record.title,
            record.kind,
            record.scope,
            record.status,
            record.confidence,
            record
                .written_on_device_name_snapshot
                .as_deref()
                .unwrap_or("unknown"),
            record.written_on_device_id.as_deref().unwrap_or("unknown"),
            record
                .resource_device_name_snapshot
                .as_deref()
                .unwrap_or("unknown"),
            record.resource_device_id.as_deref().unwrap_or("unknown"),
            record.content.replace('\n', "\n   "),
        ));
        if !record.sources.is_empty() {
            out.push_str("   sources:\n");
            for source in &record.sources {
                out.push_str(&format!(
                    "   - chat_id: {}; turn_id: {}; message_id: {}; quote: {}\n",
                    source.chat_id.as_deref().unwrap_or(""),
                    source.turn_id.as_deref().unwrap_or(""),
                    source.message_id.as_deref().unwrap_or(""),
                    source.quote.as_deref().unwrap_or("")
                ));
            }
        }
        if !record.links.is_empty() {
            out.push_str("   links:\n");
            for link in &record.links {
                out.push_str(&format!(
                    "   - {} -> {} ({})\n",
                    link.from_id, link.to_id, link.relation
                ));
            }
        }
        out.push('\n');
    }
    out
}
