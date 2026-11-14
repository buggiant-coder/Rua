use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;

#[derive(Debug, Default, serde::Serialize, Deserialize)]
struct TaskReadCursor {
    task_id: String,
    attempt_no: u32,
    output: rua_task::TaskLiveOutputCursor,
    result_offset: usize,
    error_offset: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact: Option<TaskArtifactCursor>,
}

#[derive(Debug, serde::Serialize, Deserialize)]
struct TaskArtifactCursor {
    id: String,
    offset: u64,
}

fn encode_read_cursor(cursor: &TaskReadCursor) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(cursor).expect("Task cursor is serializable"))
}

fn decode_read_cursor(value: &str) -> Result<TaskReadCursor, String> {
    if value.len() > 8192 {
        return Err("Task cursor is too large".into());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| "Invalid Task cursor".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "Invalid Task cursor".to_string())
}

use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::schema::ToolSpec;

fn compact_task(record: &rua_task::TaskRecord) -> serde_json::Value {
    let mut task = serde_json::json!({
        "task_id": record.task_id,
        "status": record.state,
        "attempt_no": record.attempt_no,
    });
    if !record.waiting_on.is_empty() {
        task["waiting_on"] = serde_json::json!(record.waiting_on);
    }
    if !record.blocked_by.is_empty() {
        task["blocked_by"] = serde_json::json!(record.blocked_by);
    }
    task
}

fn compact_task_list_item(record: &rua_task::TaskRecord) -> serde_json::Value {
    let mut task = compact_task(record);
    let description = record
        .spec
        .instruction
        .chars()
        .take(160)
        .collect::<String>();
    if !description.is_empty() {
        task["instruction"] = description.into();
    }
    if record.spec.instruction.chars().count() > 160 {
        task["instruction_truncated"] = true.into();
    }
    task
}

pub struct TaskRuntimeHandle {
    pub runtime: Arc<rua_task::TaskRuntime>,
}

impl TaskRuntimeHandle {
    pub fn new(runtime: Arc<rua_task::TaskRuntime>) -> Self {
        Self { runtime }
    }
}

pub struct TaskRunnerSessionHandle {
    pub session: Arc<rua_task::TaskRunnerSession>,
}

impl TaskRunnerSessionHandle {
    pub fn new(session: Arc<rua_task::TaskRunnerSession>) -> Self {
        Self { session }
    }
}

pub fn get_task_runtime(ctx: &ToolContext) -> Option<Arc<rua_task::TaskRuntime>> {
    ctx.task_runtime
        .as_ref()
        .and_then(|handle| handle.downcast_ref::<TaskRuntimeHandle>())
        .map(|handle| Arc::clone(&handle.runtime))
}

pub fn get_task_runner_session(ctx: &ToolContext) -> Option<Arc<rua_task::TaskRunnerSession>> {
    ctx.task_session
        .as_ref()
        .and_then(|handle| handle.downcast_ref::<TaskRunnerSessionHandle>())
        .map(|handle| Arc::clone(&handle.session))
}

fn require_task_runtime(
    ctx: &ToolContext,
    tool_name: &str,
) -> rua_core::Result<Arc<rua_task::TaskRuntime>> {
    get_task_runtime(ctx).ok_or_else(|| rua_core::RuaError::Tool {
        tool: tool_name.to_string(),
        message: "Task Runtime is not available in this execution context".to_string(),
    })
}

pub fn task_owner_from_context(ctx: &ToolContext) -> rua_task::TaskOwner {
    ctx.metadata
        .get("security_chat_address")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .map(rua_task::TaskOwner::from_address)
        .unwrap_or_else(|| {
            rua_task::TaskOwner::new(
                ctx.metadata
                    .get("agent_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&ctx.agent_id),
                &ctx.session_id,
            )
        })
}

#[derive(Debug, Deserialize)]
struct TaskCreateArgs {
    runner: rua_task::TaskRunnerSpec,
    #[serde(default)]
    instruction: Option<String>,
    #[serde(default)]
    context_refs: Vec<rua_task::TaskContextRef>,
    #[serde(default)]
    depends_on: Vec<String>,
}

pub struct TaskCreateTool;

impl TaskCreateTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "task_create".to_string(),
            description: "Submit background Tool work to the Chat-owned Task Runtime. Creation returns immediately with task_id. Results automatically resume the source Chat: continue independent work, or report pending work and normally end this Turn. Do not repeatedly read or sleep to wait for completion; task_read and task_list are for specific inspection needs.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "runner": {
                        "description": "Typed Task Runner selection",
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "type": {
                                        "type": "string",
                                        "enum": ["tool"]
                                    },
                                    "tool_name": {
                                        "type": "string",
                                        "description": "Canonical underlying tool name"
                                    },
                                    "arguments": {
                                        "type": "object",
                                        "description": "Arguments passed to the underlying tool"
                                    }
                                },
                                "required": ["type", "tool_name", "arguments"]
                            }
                        ]
                    },
                    "instruction": {
                        "type": "string",
                        "description": "Optional human-readable Task instruction"
                    },
                    "context_refs": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "uri": { "type": "string" },
                                "source_device_id": { "type": "string" }
                            },
                            "required": ["uri"]
                        },
                        "description": "Explicit context or result references"
                    },
                    "depends_on": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Existing Task IDs in the same Agent and Chat that must complete successfully first"
                    }
                },
                "required": ["runner"]
            }),
            annotations: ExtendedToolAnnotations {
                destructive: false,
                idempotent: false,
                requires_approval: ApprovalPolicy::ToolGuardDecides,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for TaskCreateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskCreateTool {
    fn name(&self) -> &str {
        "task_create"
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
        let args: TaskCreateArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "task_create".into(),
                message: e.to_string(),
            })?;

        let runtime = require_task_runtime(&ctx, "task_create")?;
        if !matches!(args.runner, rua_task::TaskRunnerSpec::Tool { .. }) {
            return Err(rua_core::RuaError::Tool {
                tool: "task_create".into(),
                message: "Task only executes background Tools; use chat_send for Agent or ACP collaboration".into(),
            });
        }
        if let rua_task::TaskRunnerSpec::Tool { tool_name, .. } = &args.runner {
            if let Some(value) = ctx
                .metadata
                .get(super::introspect::EFFECTIVE_TOOL_NAMES_METADATA_KEY)
                .or_else(|| ctx.metadata.get("chat_tool_allowlist"))
            {
                let allowed: Vec<String> =
                    serde_json::from_value(value.clone()).map_err(|error| {
                        rua_core::RuaError::Security {
                            message: format!("Invalid Chat tool allowlist: {error}"),
                        }
                    })?;
                if !allowed.contains(tool_name) {
                    return Err(rua_core::RuaError::Security {
                        message: format!(
                            "Tool '{tool_name}' is not available in this Chat configuration"
                        ),
                    });
                }
            }
        }
        let owner = task_owner_from_context(&ctx);
        let instruction = args.instruction.unwrap_or_else(|| match &args.runner {
            rua_task::TaskRunnerSpec::Tool { tool_name, .. } => {
                format!("Run tool {tool_name}")
            }
            _ => unreachable!("non-Tool Task runners are rejected above"),
        });
        let spec = rua_task::TaskSpec {
            owner: owner.clone(),
            instruction,
            runner: args.runner,
            context_refs: args.context_refs,
            depends_on: args.depends_on,
            origin: rua_task::TaskOrigin {
                turn_id: ctx
                    .metadata
                    .get("turn_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                tool_call_id: ctx
                    .metadata
                    .get("tool_use_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            },
        };
        let authorization =
            rua_task::TaskAttemptAuthorization::new(ctx.agent_id.clone()).map_err(|message| {
                rua_core::RuaError::Tool {
                    tool: "task_create".into(),
                    message,
                }
            })?;
        let authorization = match ctx
            .metadata
            .get("chat_configuration_epoch")
            .and_then(serde_json::Value::as_str)
        {
            Some(epoch) => authorization.with_configuration_epoch(epoch.to_string()),
            None => authorization,
        };
        let task_id = runtime.create(spec, authorization).await.map_err(|error| {
            rua_core::RuaError::Tool {
                tool: "task_create".into(),
                message: error.to_string(),
            }
        })?;
        let record =
            runtime
                .get(&owner, &task_id)
                .await
                .map_err(|error| rua_core::RuaError::Tool {
                    tool: "task_create".into(),
                    message: error.to_string(),
                })?;
        Ok(ToolResult::text(
            "task_create",
            compact_task(&record).to_string(),
        ))
    }
}

#[derive(Debug, Deserialize)]
struct TaskSendInputArgs {
    task_id: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    close_stdin: bool,
}

impl TaskSendInputArgs {
    fn into_input(self) -> Result<(String, rua_task::TaskInput), String> {
        match (self.text, self.close_stdin) {
            (Some(text), false) => Ok((self.task_id, rua_task::TaskInput::Text { text })),
            (None, true) => Ok((self.task_id, rua_task::TaskInput::CloseStdin)),
            _ => Err(
                "task_send_input requires exactly one of `text` or `close_stdin: true`".to_string(),
            ),
        }
    }
}

pub struct TaskSendInputTool;

impl TaskSendInputTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "task_send_input".to_string(),
            description: "Send exact text bytes or explicit EOF to the current live Runner Session of a Task owned by this Chat. Provide exactly one of `text` or `close_stdin: true`.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "ID of the Task whose current Runner Session receives input"
                    },
                    "text": {
                        "type": "string",
                        "description": "Exact UTF-8 text to send; include \\n explicitly when Enter is intended. Do not provide with close_stdin."
                    },
                    "close_stdin": {
                        "type": "boolean",
                        "description": "Set true to close Shell stdin and deliver EOF. Do not provide with text."
                    }
                },
                "required": ["task_id"],
                "additionalProperties": false
            }),
            annotations: ExtendedToolAnnotations {
                read_only: false,
                destructive: false,
                idempotent: true,
                requires_approval: ApprovalPolicy::Never,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for TaskSendInputTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskSendInputTool {
    fn name(&self) -> &str {
        "task_send_input"
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
        let args: TaskSendInputArgs =
            serde_json::from_value(args).map_err(|error| rua_core::RuaError::Tool {
                tool: "task_send_input".into(),
                message: error.to_string(),
            })?;
        let (task_id, input) = args
            .into_input()
            .map_err(|message| rua_core::RuaError::Tool {
                tool: "task_send_input".into(),
                message,
            })?;
        let tool_call_id = ctx
            .metadata
            .get("tool_use_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let submission = rua_task::TaskInputSubmission::new(format!("tool:{tool_call_id}"), input)
            .map_err(|message| rua_core::RuaError::Tool {
                tool: "task_send_input".into(),
                message,
            })?;
        let runtime = require_task_runtime(&ctx, "task_send_input")?;
        let acceptance = runtime
            .send_input(&task_owner_from_context(&ctx), &task_id, submission)
            .await
            .map_err(|error| rua_core::RuaError::Tool {
                tool: "task_send_input".into(),
                message: error.to_string(),
            })?;
        Ok(ToolResult::text(
            tool_call_id,
            serde_json::json!({
                "accepted": acceptance.accepted,
                "task_id": acceptance.task_id,
            })
            .to_string(),
        ))
    }
}

pub struct TaskWatchTool;

impl TaskWatchTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        task_watch_spec(
            "Task_Watch",
            "Watch future canonical live output from one Task owned by this Chat.",
        )
    }
}

impl Default for TaskWatchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskWatchTool {
    fn name(&self) -> &str {
        "Task_Watch"
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
        let args: TaskIdArg =
            serde_json::from_value(args).map_err(|error| rua_core::RuaError::Tool {
                tool: "Task_Watch".into(),
                message: error.to_string(),
            })?;
        let runtime = require_task_runtime(&ctx, "Task_Watch")?;
        let status = runtime
            .watch(&task_owner_from_context(&ctx), &args.task_id)
            .await
            .map_err(|error| rua_core::RuaError::Tool {
                tool: "Task_Watch".into(),
                message: error.to_string(),
            })?;
        Ok(ToolResult::text(
            "Task_Watch",
            serde_json::to_string_pretty(&status).unwrap_or_default(),
        ))
    }
}

pub struct TaskUnwatchTool;

impl TaskUnwatchTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        task_watch_spec(
            "Task_Unwatch",
            "Stop future live-output notifications for one Task without changing the Task.",
        )
    }
}

impl Default for TaskUnwatchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskUnwatchTool {
    fn name(&self) -> &str {
        "Task_Unwatch"
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
        let args: TaskIdArg =
            serde_json::from_value(args).map_err(|error| rua_core::RuaError::Tool {
                tool: "Task_Unwatch".into(),
                message: error.to_string(),
            })?;
        let runtime = require_task_runtime(&ctx, "Task_Unwatch")?;
        let status = runtime
            .unwatch(&task_owner_from_context(&ctx), &args.task_id)
            .await
            .map_err(|error| rua_core::RuaError::Tool {
                tool: "Task_Unwatch".into(),
                message: error.to_string(),
            })?;
        Ok(ToolResult::text(
            "Task_Unwatch",
            serde_json::to_string_pretty(&status).unwrap_or_default(),
        ))
    }
}

fn task_watch_spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "ID of the Task whose live output subscription is controlled"
                }
            },
            "required": ["task_id"],
            "additionalProperties": false
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

#[derive(Debug, Deserialize)]
struct TaskCancelArgs {
    task_id: String,
}

pub struct TaskCancelTool;

impl TaskCancelTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "task_cancel".to_string(),
            description: "Cancel a non-terminal Task owned by the current Chat.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "ID of the task to cancel"
                    }
                },
                "required": ["task_id"]
            }),
            annotations: ExtendedToolAnnotations {
                destructive: true,
                idempotent: true,
                requires_approval: ApprovalPolicy::ToolGuardDecides,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for TaskCancelTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskCancelTool {
    fn name(&self) -> &str {
        "task_cancel"
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
        let args: TaskCancelArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "task_cancel".into(),
                message: e.to_string(),
            })?;

        let runtime = require_task_runtime(&ctx, "task_cancel")?;
        let record = runtime
            .cancel(&task_owner_from_context(&ctx), &args.task_id)
            .await
            .map_err(|error| rua_core::RuaError::Tool {
                tool: "task_cancel".into(),
                message: error.to_string(),
            })?;
        Ok(ToolResult::text(
            "task_cancel",
            compact_task(&record).to_string(),
        ))
    }
}

#[derive(Debug, Deserialize)]
struct TaskRetryArgs {
    task_id: String,
}

pub struct TaskRetryTool;

impl TaskRetryTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "task_retry".to_string(),
            description: "Retry a failed, cancelled, or interrupted Task without changing its immutable specification.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "ID of the task to retry"
                    }
                },
                "required": ["task_id"]
            }),
            annotations: ExtendedToolAnnotations {
                idempotent: false,
                requires_approval: ApprovalPolicy::ToolGuardDecides,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }
}

impl Default for TaskRetryTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskRetryTool {
    fn name(&self) -> &str {
        "task_retry"
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
        let args: TaskRetryArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "task_retry".into(),
                message: e.to_string(),
            })?;

        let runtime = require_task_runtime(&ctx, "task_retry")?;
        let authorization =
            rua_task::TaskAttemptAuthorization::new(ctx.agent_id.clone()).map_err(|message| {
                rua_core::RuaError::Tool {
                    tool: "task_retry".into(),
                    message,
                }
            })?;
        let authorization = match ctx
            .metadata
            .get("chat_configuration_epoch")
            .and_then(serde_json::Value::as_str)
        {
            Some(epoch) => authorization.with_configuration_epoch(epoch.to_string()),
            None => authorization,
        };
        let record = runtime
            .retry(&task_owner_from_context(&ctx), &args.task_id, authorization)
            .await
            .map_err(|error| rua_core::RuaError::Tool {
                tool: "task_retry".into(),
                message: error.to_string(),
            })?;
        Ok(ToolResult::text(
            "task_retry",
            serde_json::json!({
                "task_id": record.task_id,
                "status": record.state,
                "attempt_no": record.attempts.len(),
            })
            .to_string(),
        ))
    }
}

pub struct TaskReadTool;

impl TaskReadTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "task_read".to_string(),
            description: "Read bounded output and state from a Task owned by this Chat. Initially provide only task_id; copy the returned cursor unchanged to continue. To read an attachment, copy its artifacts[].cursor instead; stored attachment bytes return immediately as base64 even if a wait is supplied. Completion does not mean output is exhausted: continue while has_more. Zero wait returns immediately; waiting never cancels work. max_bytes bounds text or base64 payload; protocol fields have separate model-output limits.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "ID of the task"
                    },
                    "cursor": {
                        "type": ["string", "null"],
                        "description": "Copy cursor or artifacts[].cursor from a previous reply unchanged. Omit or use null for the initial status/output read."
                    },
                    "wait_seconds": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 30,
                        "description": "Bounded wait for new output or terminal state"
                    },
                    "max_bytes": {
                        "type": "integer",
                        "minimum": 4,
                        "maximum": 16384,
                        "description": "Maximum output bytes returned"
                    }
                },
                "required": ["task_id"]
            }),
            annotations: ExtendedToolAnnotations::read_only(),
            namespace: "builtin".into(),
        }
    }
}

impl Default for TaskReadTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskReadTool {
    fn name(&self) -> &str {
        "task_read"
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
        let args: TaskReadArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "task_read".into(),
                message: e.to_string(),
            })?;

        let runtime = require_task_runtime(&ctx, "task_read")?;
        let owner = task_owner_from_context(&ctx);
        let error = |message: String| rua_core::RuaError::Tool {
            tool: "task_read".into(),
            message,
        };
        let decoded = args
            .cursor
            .as_deref()
            .map(decode_read_cursor)
            .transpose()
            .map_err(error)?;
        if let Some(cursor) = decoded.as_ref().filter(|cursor| cursor.artifact.is_some()) {
            if cursor.task_id != args.task_id {
                return Err(error("Cursor belongs to another Task".into()));
            }
            let artifact = cursor.artifact.as_ref().expect("artifact cursor");
            let limit = args.max_bytes.unwrap_or(8192);
            if !(4..=16384).contains(&limit) {
                return Err(error("max_bytes must be between 4 and 16384".into()));
            }
            let (bytes, size) = runtime
                .read_artifact(
                    &owner,
                    &args.task_id,
                    cursor.attempt_no,
                    &artifact.id,
                    artifact.offset,
                    (limit / 4) * 3,
                )
                .await
                .map_err(|e| error(e.to_string()))?;
            let next = artifact.offset + bytes.len() as u64;
            let next_cursor = encode_read_cursor(&TaskReadCursor {
                task_id: args.task_id.clone(),
                attempt_no: cursor.attempt_no,
                artifact: Some(TaskArtifactCursor {
                    id: artifact.id.clone(),
                    offset: next,
                }),
                ..Default::default()
            });
            let reply = serde_json::json!({"task_id":args.task_id,"artifact":{"id":artifact.id,"attempt_no":cursor.attempt_no,"offset":artifact.offset,"size_bytes":size,"encoding":"base64","data":base64::engine::general_purpose::STANDARD.encode(bytes)},"cursor":next_cursor,"has_more":next < size});
            return Ok(ToolResult::text("task_read", reply.to_string()));
        }
        let current = runtime
            .get(&owner, &args.task_id)
            .await
            .map_err(|e| error(e.to_string()))?;
        let mut cursor = match decoded {
            Some(cursor) => cursor,
            None => TaskReadCursor {
                task_id: args.task_id.clone(),
                attempt_no: current.attempt_no,
                ..Default::default()
            },
        };
        if cursor.task_id != args.task_id || cursor.attempt_no != current.attempt_no {
            return Err(error("Cursor belongs to another Task or Attempt; read without a cursor for the current Attempt".into()));
        }
        if !cursor.output.pending.is_empty() {
            return Err(error(
                "Task cursor contains unsupported pending output".into(),
            ));
        }
        let max_bytes = args.max_bytes.unwrap_or(8192);
        if !(4..=16 * 1024).contains(&max_bytes) {
            return Err(error("max_bytes must be between 4 and 16384".into()));
        }
        let reads_result_from_start = cursor.result_offset == 0 && cursor.error_offset == 0;
        let snapshot = runtime
            .read(
                &owner,
                &args.task_id,
                cursor.output.clone(),
                128,
                max_bytes,
                Duration::from_secs(args.wait_seconds.unwrap_or(0).min(30)),
            )
            .await
            .map_err(|error| rua_core::RuaError::Tool {
                tool: "task_read".into(),
                message: error.to_string(),
            })?;
        if snapshot.task.attempt_no != cursor.attempt_no {
            return Err(error(
                "Task Attempt changed during read; read without a cursor".into(),
            ));
        }
        let mut task = serde_json::json!({"task_id": snapshot.task.task_id, "status": snapshot.task.state, "attempt_no": snapshot.task.attempt_no});
        let mut output: Vec<rua_task::TaskLiveOutputRecord> = Vec::new();
        let mut used = 0;
        for record in snapshot.records {
            used += record.text.len();
            if let Some(last) = output
                .last_mut()
                .filter(|last| last.source == record.source)
            {
                last.text.push_str(&record.text);
            } else {
                output.push(record);
            }
        }
        cursor.output = snapshot.next_cursor;
        let mut has_more = snapshot.has_more;
        if let Some(result) = snapshot.task.result {
            let mut shell_observation = false;
            let text = if result.full_result_ref.is_some() {
                let bytes = runtime
                    .read_full_result(&owner, &args.task_id)
                    .await
                    .map_err(|e| error(e.to_string()))?;
                if runtime
                    .get(&owner, &args.task_id)
                    .await
                    .map_err(|e| error(e.to_string()))?
                    .attempt_no
                    != cursor.attempt_no
                {
                    return Err(error(
                        "Task Attempt changed while reading the result".into(),
                    ));
                }
                let raw: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|e| error(e.to_string()))?;
                if raw.get("content").is_some() {
                    let raw: ToolResult =
                        serde_json::from_value(raw).map_err(|e| error(e.to_string()))?;
                    if let Some(path) = raw
                        .metadata
                        .get("live_output_path")
                        .and_then(serde_json::Value::as_str)
                    {
                        shell_observation = true;
                        if used < max_bytes {
                            use rua_task::TaskLiveOutputSource;
                            let path = runtime
                                .resolve_output_file(
                                    &owner,
                                    &args.task_id,
                                    cursor.attempt_no,
                                    std::path::Path::new(path),
                                )
                                .await
                                .map_err(|e| error(e.to_string()))?;
                            let source = crate::executor::ShellLiveOutputSource::reopen(path)
                                .await
                                .map_err(|e| {
                                    error(format!("Stored Task output is unavailable: {e}"))
                                })?;
                            let batch = source
                                .read_records(&cursor.output, 128, max_bytes - used)
                                .await
                                .map_err(|e| error(e.to_string()))?;
                            for record in batch.records {
                                used += record.text.len();
                                if let Some(last) = output
                                    .last_mut()
                                    .filter(|last| last.source == record.source)
                                {
                                    last.text.push_str(&record.text);
                                } else {
                                    output.push(record);
                                }
                            }
                            cursor.output = batch.next_cursor;
                            has_more = batch.has_more;
                        }
                    }
                    if let Some(exit_code) = raw.metadata.get("exit_code") {
                        task["exit_code"] = exit_code.clone();
                    }
                    for flag in ["timed_out", "cancelled"] {
                        if raw.metadata.get(flag).and_then(serde_json::Value::as_bool) == Some(true)
                        {
                            task[flag] = true.into();
                        }
                    }
                    if let Some(truncation) = &raw.truncation {
                        task["truncation"] = serde_json::json!(truncation);
                    }
                    if shell_observation {
                        String::new()
                    } else {
                        crate::executor::plain_text(raw.text_content().as_bytes())
                    }
                } else if raw.get("bounded").and_then(serde_json::Value::as_bool) == Some(true) {
                    task["truncation"] = serde_json::json!({"reason":"raw_capture_limit","original_bytes":raw.get("original_bytes"),"recoverable":false});
                    crate::executor::plain_text(
                        raw.get("preview")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .as_bytes(),
                    )
                } else {
                    let text = raw
                        .get("output")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| error("Unsupported Task result format".into()))?;
                    crate::executor::plain_text(text.as_bytes())
                }
            } else {
                crate::executor::plain_text(result.output.as_bytes())
            };
            if !result.artifacts.is_empty() {
                task["artifacts"] = serde_json::json!(result
                    .artifacts
                    .iter()
                    .map(|artifact| {
                        let mut value = serde_json::json!(artifact);
                        value["cursor"] = encode_read_cursor(&TaskReadCursor {
                            task_id: args.task_id.clone(),
                            attempt_no: cursor.attempt_no,
                            artifact: Some(TaskArtifactCursor {
                                id: artifact.id.clone(),
                                offset: 0,
                            }),
                            ..Default::default()
                        })
                        .into();
                        value
                    })
                    .collect::<Vec<_>>());
            }
            if cursor.result_offset > text.len() || !text.is_char_boundary(cursor.result_offset) {
                return Err(error("Task result cursor is out of range".into()));
            }
            let remaining = max_bytes.saturating_sub(used);
            let mut end = cursor
                .result_offset
                .saturating_add(remaining)
                .min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if end > cursor.result_offset {
                output.push(rua_task::TaskLiveOutputRecord {
                    source: "result".into(),
                    text: text[cursor.result_offset..end].into(),
                });
            }
            cursor.result_offset = end;
            has_more |= end < text.len();
            used += output
                .last()
                .filter(|record| record.source == "result")
                .map_or(0, |record| record.text.len());
            if let Some(failure) = result.error {
                task["error"] =
                    serde_json::json!({"kind": failure.kind, "retryable": failure.retryable});
                let plain_error = crate::executor::plain_text(failure.message.as_bytes());
                let message = if shell_observation || text.contains(&plain_error) {
                    ""
                } else {
                    plain_error.as_str()
                };
                if cursor.error_offset > message.len()
                    || !message.is_char_boundary(cursor.error_offset)
                {
                    return Err(error("Task error cursor is out of range".into()));
                }
                let mut end = cursor
                    .error_offset
                    .saturating_add(max_bytes.saturating_sub(used))
                    .min(message.len());
                while !message.is_char_boundary(end) {
                    end -= 1;
                }
                if end > cursor.error_offset {
                    output.push(rua_task::TaskLiveOutputRecord {
                        source: "error".into(),
                        text: message[cursor.error_offset..end].into(),
                    });
                }
                cursor.error_offset = end;
                has_more |= end < message.len();
            }
        }
        let observed_terminal_output = snapshot.task.state.is_terminal()
            && reads_result_from_start
            && !has_more
            && output
                .iter()
                .any(|record| matches!(record.source.as_str(), "result" | "error"));
        if !output.is_empty() {
            task["output"] = serde_json::json!(output);
        }
        task["cursor"] = encode_read_cursor(&cursor).into();
        task["has_more"] = has_more.into();
        if snapshot.timed_out {
            task["timed_out"] = true.into();
        }
        if !snapshot.task.waiting_on.is_empty() {
            task["waiting_on"] = serde_json::json!(snapshot.task.waiting_on);
        }
        if !snapshot.task.blocked_by.is_empty() {
            task["blocked_by"] = serde_json::json!(snapshot.task.blocked_by);
        }
        let mut result = ToolResult::text("task_read", task.to_string());
        if observed_terminal_output {
            result.metadata.insert(
                "task_terminal_observation".into(),
                serde_json::json!({
                    "task_id": cursor.task_id, "attempt_no": cursor.attempt_no,
                }),
            );
        }
        Ok(result)
    }
}

#[derive(Debug, Deserialize)]
struct TaskIdArg {
    task_id: String,
}

#[derive(Debug, Deserialize)]
struct TaskReadArgs {
    task_id: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    wait_seconds: Option<u64>,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TaskListArgs {
    #[serde(default)]
    offset: u64,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

pub struct TaskListTool;

impl TaskListTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "task_list".to_string(),
            description: "List Tasks owned by the current Chat.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "offset": {"type":"integer","minimum":0,"description":"Use next_offset from the previous page; keep the same state filter"},
                    "state": {
                        "type": "string",
                        "enum": ["accepted", "waiting", "blocked", "running", "waiting_approval", "completed", "failed", "cancelled", "interrupted"],
                        "description": "Filter by task state"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of tasks to return"
                    }
                }
            }),
            annotations: ExtendedToolAnnotations::read_only(),
            namespace: "builtin".into(),
        }
    }
}

impl Default for TaskListTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskListTool {
    fn name(&self) -> &str {
        "task_list"
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
        let args: TaskListArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "task_list".into(),
                message: e.to_string(),
            })?;

        let runtime = require_task_runtime(&ctx, "task_list")?;
        let state = args
            .state
            .as_deref()
            .map(|value| {
                serde_json::from_value::<rua_task::TaskState>(serde_json::Value::String(
                    value.to_string(),
                ))
                .map_err(|_| rua_core::RuaError::Tool {
                    tool: "task_list".into(),
                    message: format!("unknown Task state '{value}'"),
                })
            })
            .transpose()?;
        let records = runtime
            .list(
                &task_owner_from_context(&ctx),
                rua_task::TaskListFilter {
                    state,
                    limit: Some(args.limit.unwrap_or(20).clamp(1, 100) + 1),
                    offset: args.offset,
                },
            )
            .await
            .map_err(|error| rua_core::RuaError::Tool {
                tool: "task_list".into(),
                message: error.to_string(),
            })?;
        let limit = args.limit.unwrap_or(20).clamp(1, 100);
        let tasks: Vec<_> = records
            .iter()
            .take(limit)
            .map(compact_task_list_item)
            .collect();
        let mut reply = serde_json::json!({"tasks":tasks,"has_more":records.len() > limit});
        if records.len() > limit {
            reply["next_offset"] = args.offset.saturating_add(limit as u64).into();
        }
        Ok(ToolResult::text("task_list", reply.to_string()))
    }
}

#[derive(Debug, Deserialize)]
struct TaskAwaitArgs {
    task_ids: serde_json::Value,
    #[serde(default)]
    timeout: Option<u64>,
}

const DEFAULT_AWAIT_TIMEOUT_SECS: u64 = 30;

fn normalize_id_list(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => {
                    let s = s.trim();
                    if s.is_empty() {
                        None
                    } else {
                        Some(s.to_string())
                    }
                }
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        serde_json::Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                return vec![];
            }
            if s.starts_with('[') {
                if let Ok(parsed) = serde_json::from_str::<Vec<serde_json::Value>>(s) {
                    return parsed
                        .iter()
                        .filter_map(|v| match v {
                            serde_json::Value::String(s) => {
                                let s = s.trim();
                                if s.is_empty() {
                                    None
                                } else {
                                    Some(s.to_string())
                                }
                            }
                            serde_json::Value::Number(n) => Some(n.to_string()),
                            _ => None,
                        })
                        .collect();
                }
            }
            if s.contains(',') {
                return s
                    .split(',')
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .collect();
            }
            vec![s.to_string()]
        }
        serde_json::Value::Number(n) => vec![n.to_string()],
        _ => vec![],
    }
}

pub struct TaskAwaitTool;

impl TaskAwaitTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "task_await".to_string(),
            description: "Wait up to the selected duration for Tasks owned by the current Chat to reach terminal states. A wait timeout returns their latest states without cancelling them.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Task IDs to wait for"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Maximum seconds to wait (default: 30; zero uses the default)"
                    }
                },
                "required": ["task_ids"]
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

impl Default for TaskAwaitTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskAwaitTool {
    fn name(&self) -> &str {
        "task_await"
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
        let args: TaskAwaitArgs =
            serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
                tool: "task_await".into(),
                message: e.to_string(),
            })?;

        let task_ids = normalize_id_list(&args.task_ids);
        if task_ids.is_empty() {
            return Ok(ToolResult::text(
                "task_await",
                "Error: No `task_ids` provided.",
            ));
        }

        let timeout_secs = match args.timeout {
            None | Some(0) => DEFAULT_AWAIT_TIMEOUT_SECS,
            Some(timeout_secs) => timeout_secs,
        };

        let runtime = require_task_runtime(&ctx, "task_await")?;
        let owner = task_owner_from_context(&ctx);
        let started = tokio::time::Instant::now();
        let wait_duration = Duration::from_secs(timeout_secs);
        let mut records = Vec::new();
        for id in &task_ids {
            let remaining = wait_duration.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return task_await_timeout_result(&runtime, &owner, &task_ids).await;
            }
            match runtime.await_terminal(&owner, id, remaining).await {
                Ok(record) => records.push(record),
                Err(rua_task::TaskError::WaitTimedOut(_)) => {
                    return task_await_timeout_result(&runtime, &owner, &task_ids).await;
                }
                Err(error) => {
                    return Err(rua_core::RuaError::Tool {
                        tool: "task_await".into(),
                        message: error.to_string(),
                    });
                }
            }
        }
        Ok(ToolResult::text(
            "task_await",
            serde_json::json!({
                "timed_out": false,
                "tasks": records.iter().map(compact_task).collect::<Vec<_>>(),
            })
            .to_string(),
        ))
    }
}

async fn task_await_timeout_result(
    runtime: &rua_task::TaskRuntime,
    owner: &rua_task::TaskOwner,
    task_ids: &[String],
) -> rua_core::Result<ToolResult> {
    let mut records = Vec::new();
    for task_id in task_ids {
        if let Ok(record) = runtime.get(owner, task_id).await {
            records.push(record);
        }
    }
    Ok(ToolResult::text(
        "task_await",
        serde_json::json!({
            "timed_out": true,
            "tasks": records.iter().map(compact_task).collect::<Vec<_>>(),
        })
        .to_string(),
    ))
}
