use std::sync::Arc;

use async_trait::async_trait;
use react_agent::toolkit::{ContractToolExecutor, ToolExecutor};
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_tool::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use rua_tool::schema::ToolSpec;
use serde::Deserialize;

use crate::memory_manager::{MemoryManager, MemorySearchHit, MemorySearchRequest};

#[derive(Debug, Deserialize)]
struct MemorySearchArgs {
    keywords: String,
    #[serde(default)]
    top_k: Option<usize>,
}

const DEFAULT_TOP_K: usize = 10;
const RICH_RESULT_LIMIT: usize = 3;
const COMPACT_AFTER_RESULTS: usize = 5;
const CONTENT_PREVIEW_CHARS: usize = 150;

pub struct MemorySearchTool {
    memory_manager: Arc<dyn MemoryManager>,
}

impl MemorySearchTool {
    pub fn new(memory_manager: Arc<dyn MemoryManager>) -> Self {
        Self { memory_manager }
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "memory_search".to_string(),
            description: "Search the agent's long-term memory for relevant durable facts. Use compact multi-keyword queries with likely synonyms and translations when recalling short facts, e.g. 'user name username 名字 用户名'.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "keywords": {
                        "type": "string",
                        "description": "Compact keywords, aliases, and translations to search for. Do not pass the user's full sentence."
                    },
                    "top_k": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 10)"
                    }
                },
                "required": ["keywords"]
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
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
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
        let args: MemorySearchArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "memory_search".into(),
                message: e.to_string(),
            })?;

        let req = MemorySearchRequest {
            search_mode: match ctx.metadata.get("memory_search_mode") {
                Some(value) => serde_json::from_value(value.clone()).map_err(|_| {
                    rua_core::RuaError::Validation {
                        message: "Invalid captured Memory search mode".to_string(),
                    }
                })?,
                None => rua_config::MemorySearchMode::default(),
            },
            query: args.keywords.clone(),
            agent_id: ctx.agent_id,
            top_k: args.top_k.unwrap_or(DEFAULT_TOP_K),
        };

        let hits =
            self.memory_manager
                .memory_search(req)
                .await
                .map_err(|e| rua_core::RuaError::Tool {
                    tool: "memory_search".into(),
                    message: format!("memory search failed: {e}"),
                })?;

        Ok(ToolResult::text(
            "memory_search",
            format_memory_results(&args.keywords, &hits),
        ))
    }
}

pub fn register_memory_search_tool(
    toolkit: &mut react_agent::AgentToolkit,
    memory_manager: Arc<dyn MemoryManager>,
) {
    if toolkit.has_tool("memory_search") {
        return;
    }

    let definition = MemorySearchTool::spec().to_tool_definition();
    let tool: Arc<dyn Tool> = Arc::new(MemorySearchTool::new(memory_manager));
    let executor: Arc<dyn ToolExecutor> =
        Arc::new(ContractToolExecutor::new(tool).with_schema(definition.parameters.clone()));

    toolkit.register_with_disclosure(
        definition,
        executor,
        crate::tool_disclosure::builtin_tool_disclosure("memory_search"),
    );
}

pub fn format_memory_results(_query: &str, hits: &[MemorySearchHit]) -> String {
    if hits.is_empty() {
        return "No memory results.".to_string();
    }

    hits.iter()
        .enumerate()
        .map(|(i, hit)| {
            let hit_fields = MemoryHitFields::from_hit(hit);
            let content = compact_content(&hit.content);

            if hits.len() > COMPACT_AFTER_RESULTS && i >= RICH_RESULT_LIMIT {
                return format!(
                    "{}. [{}] content={}",
                    i + 1,
                    hit_fields.compact_fields().join(" "),
                    content
                );
            }

            let mut fields = vec![format!("score={:.3}", hit.score)];
            push_optional_field(&mut fields, "id", hit_fields.id);
            push_optional_field(&mut fields, "kind", hit_fields.kind);
            push_optional_field(&mut fields, "title", hit_fields.title);
            if let Some(confidence) = hit_fields.confidence {
                fields.push(format!("confidence={confidence:.3}"));
            }
            push_optional_field(&mut fields, "updated", hit_fields.updated_at);
            if let Some(device) = format_device(
                hit_fields.written_on_device_name,
                hit_fields.written_on_device_id,
            ) {
                fields.push(format!("device={device}"));
            }
            if let Some(device) = format_device(
                hit_fields.resource_device_name,
                hit_fields.resource_device_id,
            ) {
                fields.push(format!("resource_device={device}"));
            }

            format!("{}. [{}] content={}", i + 1, fields.join(" "), content)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct MemoryHitFields<'a> {
    score: f64,
    id: Option<&'a str>,
    kind: Option<&'a str>,
    title: Option<&'a str>,
    confidence: Option<f64>,
    updated_at: Option<&'a str>,
    written_on_device_id: Option<&'a str>,
    written_on_device_name: Option<&'a str>,
    resource_device_id: Option<&'a str>,
    resource_device_name: Option<&'a str>,
}

impl<'a> MemoryHitFields<'a> {
    fn from_hit(hit: &'a MemorySearchHit) -> Self {
        Self {
            score: hit.score,
            id: hit.metadata.get("id").and_then(|value| value.as_str()),
            kind: hit.metadata.get("kind").and_then(|value| value.as_str()),
            title: hit.metadata.get("title").and_then(|value| value.as_str()),
            confidence: hit
                .metadata
                .get("confidence")
                .and_then(|value| value.as_f64()),
            updated_at: hit
                .metadata
                .get("updated_at")
                .and_then(|value| value.as_str()),
            written_on_device_id: hit
                .metadata
                .get("written_on_device_id")
                .and_then(|value| value.as_str()),
            written_on_device_name: hit
                .metadata
                .get("written_on_device_name_snapshot")
                .and_then(|value| value.as_str()),
            resource_device_id: hit
                .metadata
                .get("resource_device_id")
                .and_then(|value| value.as_str()),
            resource_device_name: hit
                .metadata
                .get("resource_device_name_snapshot")
                .and_then(|value| value.as_str()),
        }
    }

    fn compact_fields(&self) -> Vec<String> {
        let mut fields = vec![format!("score={:.3}", self.score)];
        push_optional_field(&mut fields, "id", self.id);
        push_optional_field(&mut fields, "kind", self.kind);
        push_optional_field(&mut fields, "title", self.title);
        fields
    }
}

fn push_optional_field(fields: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        fields.push(format!("{key}={value}"));
    }
}

fn format_device(name: Option<&str>, id: Option<&str>) -> Option<String> {
    let name = name.map(str::trim).filter(|value| !value.is_empty());
    let id = id.map(str::trim).filter(|value| !value.is_empty());
    match (name, id) {
        (Some(name), Some(id)) => Some(format!("{name}({id})")),
        (Some(name), None) => Some(name.to_string()),
        (None, Some(id)) => Some(id.to_string()),
        (None, None) => None,
    }
}

fn compact_content(content: &str) -> String {
    let compacted = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated = compacted.chars().count() > CONTENT_PREVIEW_CHARS;
    let mut preview = compacted
        .chars()
        .take(CONTENT_PREVIEW_CHARS)
        .collect::<String>();
    if truncated {
        preview.push_str(" (use memory_get for full record)");
    }
    preview
}
