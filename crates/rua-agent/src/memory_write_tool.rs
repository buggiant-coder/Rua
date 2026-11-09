use std::sync::Arc;

use async_trait::async_trait;
use react_agent::toolkit::{ContractToolExecutor, ToolExecutor};
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_tool::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use rua_tool::schema::ToolSpec;
use serde::Deserialize;

use crate::memory_manager::{
    MemoryKind, MemoryManager, MemoryScope, MemorySourceEvidence, MemoryWriteRequest,
};

#[derive(Debug, Deserialize)]
struct MemoryWriteArgs {
    #[serde(default)]
    kind: Option<MemoryKind>,
    #[serde(default)]
    scope: Option<MemoryScope>,
    title: String,
    content: String,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    importance: Option<f32>,
    #[serde(default)]
    ttl_days: Option<i64>,
    #[serde(default)]
    evidence: Option<MemorySourceEvidence>,
    #[serde(default)]
    labels: Vec<String>,
    search_terms: Vec<String>,
    #[serde(default)]
    metadata: serde_json::Value,
}

const LONG_MEMORY_CONTENT_WARNING_CHARS: usize = 500;

pub struct MemoryWriteTool {
    memory_manager: Arc<dyn MemoryManager>,
}

impl MemoryWriteTool {
    pub fn new(memory_manager: Arc<dyn MemoryManager>) -> Self {
        Self { memory_manager }
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "memory_write".to_string(),
            description: concat!(
                "Save durable facts to long-term Memory through MemoryManager. ",
                "Use only for stable facts that reduce future user steering: user preferences, ",
                "recurring corrections, stable project conventions, environment details, and tool quirks. ",
                "Do not save task progress, completed-work logs, temporary TODOs, PR/issue/commit ids, ",
                "or facts likely stale within 7 days. Write declarative facts, not commands to yourself. ",
                "Procedures and reusable workflows belong in skills, not Memory."
            )
            .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["fact", "preference", "convention", "project_fact", "tool_quirk", "personal_fact"],
                        "description": "Optional durable fact kind. Defaults to fact."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["user", "agent", "workspace", "project"],
                        "description": "Optional applicability scope. Defaults to agent."
                    },
                    "title": {
                        "type": "string",
                        "maxLength": rua_memory::MEMORY_TITLE_MAX_CHARS,
                        "description": "Required concise, natural-language display title. Use the user's language and keep it to one line."
                    },
                    "content": {
                        "type": "string",
                        "description": "A concise declarative fact. Do not write imperative instructions."
                    },
                    "labels": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Human-facing labels for grouping and display, e.g. ['家庭照片', '本地路径']."
                    },
                    "confidence": {
                        "type": "number",
                        "description": "Optional confidence score from 0.0 to 1.0."
                    },
                    "importance": {
                        "type": "number",
                        "description": "Optional importance score from 0.0 to 1.0."
                    },
                    "ttl_days": {
                        "type": ["integer", "null"],
                        "description": "Optional time-to-live in days for facts with an expiration boundary."
                    },
                    "evidence": {
                        "type": "object",
                        "properties": {
                            "chat_id": { "type": "string" },
                            "turn_id": { "type": "string" },
                            "message_id": { "type": "string" },
                            "quote": { "type": "string" }
                        },
                        "additionalProperties": false,
                        "description": "Optional source evidence explaining why this memory exists."
                    },
                    "search_terms": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "type": "string" },
                        "description": "Required retrieval aliases and keywords in the user's likely languages, e.g. ['user name', 'username', '名字', '用户名']. Include stable synonyms for short facts and local resources."
                    },
                    "metadata": {
                        "type": "object",
                        "description": "Optional structured metadata."
                    }
                },
                "required": ["title", "content", "search_terms"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: false,
                idempotent: false,
                destructive: false,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

#[async_trait]
impl Tool for MemoryWriteTool {
    fn name(&self) -> &str {
        "memory_write"
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
        let mut args: MemoryWriteArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "memory_write".into(),
                message: e.to_string(),
            })?;
        args.title = rua_memory::normalize_memory_title(&args.title).map_err(|message| {
            rua_core::RuaError::Tool {
                tool: "memory_write".into(),
                message,
            }
        })?;
        if !args.search_terms.iter().any(|term| !term.trim().is_empty()) {
            return Err(rua_core::RuaError::Tool {
                tool: "memory_write".into(),
                message: "memory search_terms must not be empty".to_string(),
            });
        }
        let search_terms = args.search_terms.clone();
        let device_context = memory_device_context_from_tool_context(&ctx);
        let metadata = inject_search_terms(args.metadata, search_terms.clone());
        let metadata = inject_memory_device_context(metadata, &device_context);
        let evidence = args
            .evidence
            .or_else(|| memory_source_from_tool_context(&ctx));

        let outcome = self
            .memory_manager
            .memory_write(MemoryWriteRequest {
                agent_id: ctx.agent_id,
                kind: args.kind.unwrap_or(MemoryKind::Fact),
                scope: args.scope.unwrap_or(MemoryScope::Agent),
                title: args.title,
                content: args.content,
                confidence: args.confidence,
                importance: args.importance,
                ttl_days: args.ttl_days,
                labels: args.labels,
                search_terms,
                evidence,
                metadata,
            })
            .await
            .map_err(|e| rua_core::RuaError::Tool {
                tool: "memory_write".into(),
                message: format!("memory write failed: {e}"),
            })?;

        if outcome.rejected {
            let reason = outcome
                .reason
                .unwrap_or_else(|| "MemoryManager rejected the write.".to_string());
            return Ok(ToolResult::text(
                "memory_write",
                format!("memory_write rejected: {reason}"),
            ));
        }

        let Some(record) = outcome.record else {
            return Ok(ToolResult::text(
                "memory_write",
                "memory_write completed without creating a record.",
            ));
        };

        let mut text = format!(
            "memory saved: id={} kind={:?} scope={:?} title={} resource_device_id={} resource_device_name={}",
            record.id,
            record.kind,
            record.scope,
            record.title,
            record.resource_device_id.as_deref().unwrap_or(""),
            record
                .resource_device_name_snapshot
                .as_deref()
                .unwrap_or("")
        );
        if record.content.chars().count() > LONG_MEMORY_CONTENT_WARNING_CHARS {
            text.push_str(
                "\nwarning: Memory content is long; durable Memory works best as a concise declarative fact.",
            );
        }

        Ok(ToolResult::text("memory_write", text))
    }
}

#[derive(Debug, Clone, Default)]
struct MemoryDeviceContext {
    written_on_device_id: Option<String>,
    written_on_device_name_snapshot: Option<String>,
    written_on_host_profile: Option<String>,
    resource_device_id: Option<String>,
    resource_device_name_snapshot: Option<String>,
}

fn memory_device_context_from_tool_context(ctx: &ToolContext) -> MemoryDeviceContext {
    let written_on_device_id = metadata_string(&ctx.metadata, "device_id");
    let written_on_device_name_snapshot = metadata_string(&ctx.metadata, "device_friendly_name");
    let written_on_host_profile = metadata_string(&ctx.metadata, "host_profile");

    MemoryDeviceContext {
        written_on_device_id,
        written_on_device_name_snapshot,
        written_on_host_profile,
        resource_device_id: metadata_string(&ctx.metadata, "resource_device_id"),
        resource_device_name_snapshot: metadata_string(
            &ctx.metadata,
            "resource_device_name_snapshot",
        ),
    }
}

fn memory_source_from_tool_context(ctx: &ToolContext) -> Option<MemorySourceEvidence> {
    let chat_id = metadata_string(&ctx.metadata, "chat_id")
        .or_else(|| (!ctx.session_id.trim().is_empty()).then(|| ctx.session_id.clone()));
    let turn_id = metadata_string(&ctx.metadata, "turn_id");
    if chat_id.is_none() && turn_id.is_none() {
        return None;
    }
    Some(MemorySourceEvidence {
        chat_id,
        turn_id,
        message_id: None,
        quote: None,
    })
}

fn metadata_string(
    metadata: &std::collections::HashMap<String, serde_json::Value>,
    key: &str,
) -> Option<String> {
    metadata
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn inject_memory_device_context(
    metadata: serde_json::Value,
    context: &MemoryDeviceContext,
) -> serde_json::Value {
    let mut object = metadata.as_object().cloned().unwrap_or_default();
    insert_optional(
        &mut object,
        "written_on_device_id",
        context.written_on_device_id.as_deref(),
    );
    insert_optional(
        &mut object,
        "written_on_device_name_snapshot",
        context.written_on_device_name_snapshot.as_deref(),
    );
    insert_optional(
        &mut object,
        "written_on_host_profile",
        context.written_on_host_profile.as_deref(),
    );
    insert_optional(
        &mut object,
        "resource_device_id",
        context
            .resource_device_id
            .as_deref()
            .or(context.written_on_device_id.as_deref()),
    );
    insert_optional(
        &mut object,
        "resource_device_name_snapshot",
        context
            .resource_device_name_snapshot
            .as_deref()
            .or(context.written_on_device_name_snapshot.as_deref()),
    );
    serde_json::Value::Object(object)
}

fn inject_search_terms(
    metadata: serde_json::Value,
    search_terms: Vec<String>,
) -> serde_json::Value {
    let mut object = metadata.as_object().cloned().unwrap_or_default();
    let mut terms = object
        .get("search_terms")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    for term in search_terms {
        let term = term.trim();
        if !term.is_empty()
            && !terms
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(term))
        {
            terms.push(term.to_string());
        }
    }

    if !terms.is_empty() {
        object.insert("search_terms".into(), serde_json::json!(terms));
    }
    serde_json::Value::Object(object)
}

fn insert_optional(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        object.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    } else {
        object.remove(key);
    }
}

pub fn register_memory_write_tool(
    toolkit: &mut react_agent::AgentToolkit,
    memory_manager: Arc<dyn MemoryManager>,
) {
    if toolkit.has_tool("memory_write") {
        return;
    }

    let definition = MemoryWriteTool::spec().to_tool_definition();
    let tool: Arc<dyn Tool> = Arc::new(MemoryWriteTool::new(memory_manager));
    let executor: Arc<dyn ToolExecutor> =
        Arc::new(ContractToolExecutor::new(tool).with_schema(definition.parameters.clone()));

    toolkit.register_with_disclosure(
        definition,
        executor,
        crate::tool_disclosure::builtin_tool_disclosure("memory_write"),
    );
}
