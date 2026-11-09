use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use serde::Deserialize;

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::schema::ToolSpec;

#[derive(Debug, Clone)]
pub struct CollaborationInvocation {
    pub source: rua_core::ChatAddress,
    pub turn_id: String,
    pub tool_call_id: String,
    pub configuration_epoch: String,
    pub working_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub enum CollaborationRequest {
    CreateAgent {
        agent_id: String,
        message: Option<String>,
        yield_seconds: u64,
    },
    CreateAcp {
        runner: String,
        message: Option<String>,
        yield_seconds: u64,
    },
    Send {
        chat: rua_core::ChatAddress,
        message: String,
        yield_seconds: u64,
    },
    Read {
        chat: rua_core::ChatAddress,
        yield_seconds: u64,
        result: Option<rua_session::collaboration::CollaborationResultCursor>,
    },
    StopTurn {
        chat: rua_core::ChatAddress,
        turn_id: String,
        authority_epoch: String,
    },
}

#[async_trait]
pub trait CollaborationRuntime: Send + Sync {
    async fn inventory(&self) -> Result<serde_json::Value, String>;
    async fn request_progress(
        &self,
        source: &rua_core::ChatAddress,
    ) -> Result<serde_json::Value, String>;
    async fn execute(
        &self,
        invocation: CollaborationInvocation,
        request: CollaborationRequest,
    ) -> Result<serde_json::Value, String>;
}

pub struct CollaborationRuntimeHandle {
    pub runtime: Arc<dyn CollaborationRuntime>,
}

impl CollaborationRuntimeHandle {
    pub fn new(runtime: Arc<dyn CollaborationRuntime>) -> Self {
        Self { runtime }
    }
}

pub struct CollaborationTool {
    operation: CollaborationOperation,
}

#[derive(Clone, Copy)]
enum CollaborationOperation {
    Send,
    Read,
    StopTurn,
}

#[derive(Deserialize)]
struct SendArgs {
    chat: Option<rua_core::ChatAddress>,
    agent_id: Option<String>,
    acp_runner: Option<String>,
    message: String,
    #[serde(default = "default_yield")]
    yield_seconds: u64,
}

#[derive(Deserialize)]
struct ReadArgs {
    chat: rua_core::ChatAddress,
    #[serde(default)]
    result: Option<rua_session::collaboration::CollaborationResultCursor>,
    #[serde(default = "default_yield")]
    yield_seconds: u64,
}

#[derive(Deserialize)]
struct StopTurnArgs {
    chat: rua_core::ChatAddress,
    turn_id: String,
    authority_epoch: String,
}

fn default_yield() -> u64 {
    10
}

impl CollaborationTool {
    fn new(operation: CollaborationOperation) -> Self {
        Self { operation }
    }

    fn spec(operation: CollaborationOperation) -> ToolSpec {
        let (name, description, parameters, read_only) = match operation {
            CollaborationOperation::Send => (
                "chat_send",
                "Send a message to an owned collaboration Chat, or explicitly select an Agent/ACP runner to create a new Chat with its first message. Existing active execution is interjected at a safe boundary; already-started tools are not silently cancelled. Returns a result or bounded acceptance receipt with runtime request progress. Waiting is not failure: do not use Stop to hurry a request. Results automatically resume the source Chat; do independent work or end this Turn with a pending-work report instead of polling. A new Chat does not inherit source dialogue history.",
                serde_json::json!({"type":"object", "properties": {
                    "chat":{"type":"object","description":"Exact ChatAddress from an earlier send; omit when explicitly creating a new Chat"},
                    "agent_id":{"type":"string"},
                    "acp_runner":{"type":"string","description":"Exact configured ACP runner key, not a product or CLI name. Built-in keys: claude (Claude Code, command claude-agent-acp), codex (command codex-acp), opencode, qwen. Custom runner keys depend on Agent configuration; do not use claude-code unless it is explicitly configured as a key."},
                    "message":{"type":"string"},
                    "yield_seconds":{"type":"integer","minimum":0,"maximum":30,"default":10}
                },"required":["message"],"oneOf":[
                    {"required":["chat"],"not":{"anyOf":[{"required":["agent_id"]},{"required":["acp_runner"]}]}},
                    {"required":["agent_id"],"not":{"anyOf":[{"required":["chat"]},{"required":["acp_runner"]}]}},
                    {"required":["acp_runner"],"not":{"anyOf":[{"required":["chat"]},{"required":["agent_id"]}]}}
                ]}),
                false,
            ),
            CollaborationOperation::Read => (
                "chat_read",
                "Inspect state, cached progress, or an undelivered result when you need specific information, not to wait for completion. Results automatically resume the source Chat; with no independent work, normally end this Turn rather than polling. Positive yield_seconds bounds the entire read including routing and summary work. Zero wait skips progress generation; required IO and stored-result reads have a 5-second limit. A read timeout never cancels execution. For more result text, pass outcome.next unchanged as result; this only reads stored data and never submits input or triggers delivery. Does not expose the child transcript.",
                serde_json::json!({"type":"object", "properties": {
                    "chat":{"type":"object","description":"Exact ChatAddress returned by chat_send"},
                    "yield_seconds":{"type":"integer","minimum":0,"maximum":30,"default":10},
                    "result":{"type":"object","description":"Exact outcome.next or result_ref cursor from a reply or notification","properties":{"request_id":{"type":"string"},"result_id":{"type":"string"},"offset":{"type":"integer","minimum":0}},"required":["request_id","result_id","offset"]}
                },"required":["chat"]}),
                true,
            ),
            CollaborationOperation::StopTurn => (
                "chat_stop",
                "Abandon current work in an owned collaboration Chat: stop the observed active Turn and cancel accepted requests that have not started. Does not leave the Chat paused; a new Send can start work. Copy turn_id and authority_epoch together from active_turn in the latest send/read reply to avoid stopping newer execution. This is not a way to hurry a slow Send. A successful request is not proof execution has finished; Read observes termination.",
                serde_json::json!({"type":"object", "properties": {
                    "chat":{"type":"object","description":"Exact ChatAddress returned by chat_send"},
                    "turn_id":{"type":"string","description":"Exact active_turn.turn_id from a collaboration reply"},
                    "authority_epoch":{"type":"string","description":"Exact active_turn.authority_epoch paired with that Turn; not the Turn ID"}
                },"required":["chat","turn_id","authority_epoch"]}),
                false,
            ),
        };
        ToolSpec {
            name: name.into(),
            description: description.into(),
            parameters,
            annotations: ExtendedToolAnnotations {
                read_only,
                requires_approval: ApprovalPolicy::ToolGuardDecides,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }

    fn runtime(context: &ToolContext) -> Result<Arc<dyn CollaborationRuntime>, String> {
        context
            .collaboration_runtime
            .as_ref()
            .and_then(|handle| handle.downcast_ref::<CollaborationRuntimeHandle>())
            .map(|handle| Arc::clone(&handle.runtime))
            .ok_or_else(|| "Collaboration Runtime is unavailable".to_string())
    }

    fn invocation(context: &ToolContext) -> Result<CollaborationInvocation, String> {
        let field = |key: &str| {
            context
                .metadata
                .get(key)
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("Missing trusted invocation {key}"))
        };
        let source = context
            .metadata
            .get("security_chat_address")
            .cloned()
            .ok_or_else(|| "Missing source ChatAddress".to_string())?;
        Ok(CollaborationInvocation {
            source: serde_json::from_value(source).map_err(|error| error.to_string())?,
            turn_id: field("turn_id")?,
            tool_call_id: field("tool_call_id")?,
            configuration_epoch: field("chat_configuration_epoch")?,
            working_dir: context.working_dir.clone(),
        })
    }
}

#[async_trait]
impl Tool for CollaborationTool {
    fn name(&self) -> &str {
        match self.operation {
            CollaborationOperation::Send => "chat_send",
            CollaborationOperation::Read => "chat_read",
            CollaborationOperation::StopTurn => "chat_stop",
        }
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec(self.operation);
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        context: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let tool_name = self.name().to_string();
        let error = |message: String| rua_core::RuaError::Tool {
            tool: tool_name.clone(),
            message,
        };
        let runtime = Self::runtime(&context).map_err(error)?;
        let invocation = Self::invocation(&context).map_err(error)?;
        let request = match self.operation {
            CollaborationOperation::Send => {
                let args: SendArgs =
                    serde_json::from_value(args).map_err(|e| error(e.to_string()))?;
                match (args.chat, args.agent_id, args.acp_runner) {
                    (Some(chat), None, None) => CollaborationRequest::Send {
                        chat,
                        message: args.message,
                        yield_seconds: args.yield_seconds,
                    },
                    (None, Some(agent_id), None) => CollaborationRequest::CreateAgent {
                        agent_id,
                        message: Some(args.message),
                        yield_seconds: args.yield_seconds,
                    },
                    (None, None, Some(runner)) => CollaborationRequest::CreateAcp {
                        runner,
                        message: Some(args.message),
                        yield_seconds: args.yield_seconds,
                    },
                    _ => {
                        return Err(error(
                            "chat_send requires exactly one of chat, agent_id or acp_runner".into(),
                        ))
                    }
                }
            }
            CollaborationOperation::Read => {
                let args: ReadArgs =
                    serde_json::from_value(args).map_err(|e| error(e.to_string()))?;
                CollaborationRequest::Read {
                    chat: args.chat,
                    yield_seconds: args.yield_seconds,
                    result: args.result,
                }
            }
            CollaborationOperation::StopTurn => {
                let args: StopTurnArgs =
                    serde_json::from_value(args).map_err(|e| error(e.to_string()))?;
                CollaborationRequest::StopTurn {
                    chat: args.chat,
                    turn_id: args.turn_id,
                    authority_epoch: args.authority_epoch,
                }
            }
        };
        match runtime.execute(invocation, request).await {
            Ok(result) => Ok(ToolResult::text(&tool_name, result.to_string())),
            Err(message) => Ok(ToolResult::error(&tool_name, message)),
        }
    }
}

pub fn register_collaboration_tools(
    registry: &mut crate::ToolRegistry,
) -> Result<(), crate::ToolError> {
    for operation in [
        CollaborationOperation::Send,
        CollaborationOperation::Read,
        CollaborationOperation::StopTurn,
    ] {
        registry.register(
            Arc::new(CollaborationTool::new(operation)),
            CollaborationTool::spec(operation),
            crate::ToolNamespace::Builtin,
        )?;
    }
    Ok(())
}
