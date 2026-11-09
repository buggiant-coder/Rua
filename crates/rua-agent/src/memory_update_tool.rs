use std::sync::Arc;

use async_trait::async_trait;
use react_agent::toolkit::{ContractToolExecutor, ToolExecutor};
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_tool::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use rua_tool::schema::ToolSpec;
use serde::Deserialize;

use crate::memory_manager::{
    MemoryGetRequest, MemoryKind, MemoryManager, MemoryScope, MemoryUpdateRequest,
};

#[derive(Debug, Deserialize)]
struct MemoryUpdateArgs {
    id: rua_memory::MemoryId,
    content: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    kind: Option<MemoryKind>,
    #[serde(default)]
    scope: Option<MemoryScope>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    importance: Option<f32>,
    #[serde(default)]
    search_terms: Option<Vec<String>>,
}

pub struct MemoryUpdateTool {
    memory_manager: Arc<dyn MemoryManager>,
}

impl MemoryUpdateTool {
    pub fn new(memory_manager: Arc<dyn MemoryManager>) -> Self {
        Self { memory_manager }
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "memory_update".to_string(),
            description: concat!(
                "Update one existing long-term Memory record by id. ",
                "Use after memory_search and, when needed, memory_get identify the record to revise. ",
                "This only updates Memory owned by the current Agent."
            )
            .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Memory record id to update."
                    },
                    "content": {
                        "type": "string",
                        "description": "Replacement declarative fact content."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["fact", "preference", "convention", "project_fact", "tool_quirk", "personal_fact"],
                        "description": "Optional replacement durable fact kind. Defaults to the existing kind."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["user", "agent", "workspace", "project"],
                        "description": "Optional replacement applicability scope. Defaults to the existing scope."
                    },
                    "title": {
                        "type": "string",
                        "maxLength": rua_memory::MEMORY_TITLE_MAX_CHARS,
                        "description": "Optional replacement natural-language display title. Defaults to the existing title."
                    },
                    "confidence": {
                        "type": "number",
                        "description": "Optional replacement confidence score from 0.0 to 1.0. Defaults to the existing confidence."
                    },
                    "importance": {
                        "type": "number",
                        "description": "Optional replacement importance score from 0.0 to 1.0. Defaults to the existing importance."
                    },
                    "search_terms": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "type": "string" },
                        "description": "Optional replacement retrieval aliases and keywords. Defaults to the existing search terms."
                    }
                },
                "required": ["id", "content"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: false,
                idempotent: false,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

#[async_trait]
impl Tool for MemoryUpdateTool {
    fn name(&self) -> &str {
        "memory_update"
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
        let mut args: MemoryUpdateArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "memory_update".into(),
                message: e.to_string(),
            })?;
        if let Some(title) = args.title.as_ref() {
            args.title = Some(
                rua_memory::normalize_memory_title(title).map_err(|message| {
                    rua_core::RuaError::Tool {
                        tool: "memory_update".into(),
                        message,
                    }
                })?,
            );
        }
        if args
            .search_terms
            .as_ref()
            .is_some_and(|terms| !terms.iter().any(|term| !term.trim().is_empty()))
        {
            return Err(rua_core::RuaError::Tool {
                tool: "memory_update".into(),
                message: "memory search_terms must not be empty".to_string(),
            });
        }
        let id = args.id.clone();
        let id_text = id.to_string();
        let mut records = self
            .memory_manager
            .memory_get_for_agent(
                &ctx.agent_id,
                MemoryGetRequest {
                    ids: vec![id.clone()],
                    include_inactive: false,
                },
            )
            .await
            .map_err(|e| rua_core::RuaError::Tool {
                tool: "memory_update".into(),
                message: format!("memory update lookup failed: {e}"),
            })?;
        let Some(existing) = records.pop() else {
            return Ok(ToolResult::text(
                "memory_update",
                format!("memory_update rejected: id={id_text} not found."),
            ));
        };
        if existing.agent_id != ctx.agent_id {
            return Ok(ToolResult::text(
                "memory_update",
                format!("memory_update rejected: id={id_text} is not owned by current agent."),
            ));
        }

        let updated = self
            .memory_manager
            .memory_update(MemoryUpdateRequest {
                id,
                agent_id: ctx.agent_id,
                kind: args.kind.unwrap_or(existing.kind),
                scope: args.scope.unwrap_or(existing.scope),
                title: args.title.unwrap_or(existing.title),
                content: args.content,
                confidence: args.confidence.unwrap_or(existing.confidence),
                importance: args.importance.or(existing.importance),
                search_terms: args.search_terms,
            })
            .await
            .map_err(|e| rua_core::RuaError::Tool {
                tool: "memory_update".into(),
                message: format!("memory update failed: {e}"),
            })?;

        let Some(record) = updated else {
            return Ok(ToolResult::text(
                "memory_update",
                format!("memory_update rejected: id={id_text} not updated."),
            ));
        };
        Ok(ToolResult::text(
            "memory_update",
            format!(
                "memory updated: id={} kind={:?} scope={:?} title={}",
                record.id, record.kind, record.scope, record.title
            ),
        ))
    }
}

pub fn register_memory_update_tool(
    toolkit: &mut react_agent::AgentToolkit,
    memory_manager: Arc<dyn MemoryManager>,
) {
    if toolkit.has_tool("memory_update") {
        return;
    }

    let definition = MemoryUpdateTool::spec().to_tool_definition();
    let tool: Arc<dyn Tool> = Arc::new(MemoryUpdateTool::new(memory_manager));
    let executor: Arc<dyn ToolExecutor> =
        Arc::new(ContractToolExecutor::new(tool).with_schema(definition.parameters.clone()));

    toolkit.register_with_disclosure(
        definition,
        executor,
        crate::tool_disclosure::builtin_tool_disclosure("memory_update"),
    );
}
