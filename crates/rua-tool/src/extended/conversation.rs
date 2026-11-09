use async_trait::async_trait;
use serde::Deserialize;

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_session::{ConversationReadSpanRequest, ConversationRecallRequest, WorkspaceSessionStore};

use crate::annotations::ExtendedToolAnnotations;
use crate::schema::ToolSpec;

#[derive(Debug, Deserialize)]
struct ConversationRecallArgs {
    query: String,
    #[serde(default = "default_max_results")]
    max_results: usize,
    #[serde(default)]
    surrounding_messages: u64,
}

#[derive(Debug, Deserialize)]
struct ConversationReadSpanArgs {
    start_sequence: u64,
    end_sequence: u64,
    #[serde(default)]
    expand_before: u64,
    #[serde(default)]
    expand_after: u64,
}

pub struct ConversationRecallTool;

impl ConversationRecallTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "conversation_search".to_string(),
            description: "Search the current chat's stored raw transcript when earlier details were omitted from the active compacted context. Respects selected regenerated responses. Use this for current-chat recall, not long-term memory search.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Text to search for in the active current chat transcript. Whitespace-separated terms are treated as alternatives, so aliases like \"笑话 joke\" can match either term."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of hits to return"
                    },
                    "surrounding_messages": {
                        "type": "integer",
                        "description": "Number of adjacent messages to include in each returned span boundary"
                    }
                },
                "required": ["query"]
            }),
            annotations: ExtendedToolAnnotations::read_only(),
            namespace: "builtin".into(),
        }
    }
}

impl Default for ConversationRecallTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ConversationRecallTool {
    fn name(&self) -> &str {
        "conversation_search"
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
        let args: ConversationRecallArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "conversation_search".into(),
                message: e.to_string(),
            })?;
        let store = WorkspaceSessionStore::new(workspace_dir(&ctx));
        let result = store
            .conversation_recall(
                &ctx.session_id,
                ConversationRecallRequest {
                    query: args.query,
                    max_results: args.max_results,
                    surrounding_messages: args.surrounding_messages,
                },
            )
            .await?;
        let body = serde_json::to_string(&result).map_err(|e| rua_core::RuaError::Tool {
            tool: "conversation_search".into(),
            message: format!("failed to serialize recall result: {e}"),
        })?;
        Ok(ToolResult::text("conversation_search", body))
    }
}

pub struct ConversationReadSpanTool;

impl ConversationReadSpanTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "conversation_read_span".to_string(),
            description: "Read or expand an exact sequence span from the current chat's stored raw transcript. Call this directly with coordinates from a History Replacement locator, or with a span returned by conversation_search; a prior search is not required.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "start_sequence": {
                        "type": "integer",
                        "description": "First message sequence from a History Replacement locator or conversation_search span"
                    },
                    "end_sequence": {
                        "type": "integer",
                        "description": "Last message sequence from a History Replacement locator or conversation_search span"
                    },
                    "expand_before": {
                        "type": "integer",
                        "description": "Additional messages to include before start_sequence"
                    },
                    "expand_after": {
                        "type": "integer",
                        "description": "Additional messages to include after end_sequence"
                    }
                },
                "required": ["start_sequence", "end_sequence"]
            }),
            annotations: ExtendedToolAnnotations::read_only(),
            namespace: "builtin".into(),
        }
    }
}

impl Default for ConversationReadSpanTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ConversationReadSpanTool {
    fn name(&self) -> &str {
        "conversation_read_span"
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
        let args: ConversationReadSpanArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "conversation_read_span".into(),
                message: e.to_string(),
            })?;
        let store = WorkspaceSessionStore::new(workspace_dir(&ctx));
        let result = store
            .conversation_read_span(
                &ctx.session_id,
                ConversationReadSpanRequest {
                    start_sequence: args.start_sequence,
                    end_sequence: args.end_sequence,
                    expand_before: args.expand_before,
                    expand_after: args.expand_after,
                },
            )
            .await?;
        let body = serde_json::to_string(&result).map_err(|e| rua_core::RuaError::Tool {
            tool: "conversation_read_span".into(),
            message: format!("failed to serialize span result: {e}"),
        })?;
        Ok(ToolResult::text("conversation_read_span", body))
    }
}

fn workspace_dir(ctx: &ToolContext) -> std::path::PathBuf {
    ctx.metadata
        .get("rua_workspace_dir")
        .and_then(serde_json::Value::as_str)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| ctx.working_dir.clone())
}

fn default_max_results() -> usize {
    10
}
