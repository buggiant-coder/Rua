use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use rua_task::{
    TaskAdoptedExecution, TaskArtifact, TaskArtifactPayload, TaskError, TaskErrorInfo, TaskId,
    TaskOrigin, TaskOwner, TaskResourceRef, TaskRunOutcome, TaskRunOutput, TaskRunRequest,
    TaskRunner, TaskRunnerError, TaskRunnerKind, TaskRunnerSpec, TaskRuntime, TaskSpec,
};
use tokio_util::sync::CancellationToken;

const RESULT_PREVIEW_CHARS: usize = 4_000;

#[derive(Debug, Clone)]
pub struct ToolTaskExecutionRequest {
    pub task_id: String,
    pub attempt_no: u32,
    pub owner: TaskOwner,
    pub authorization_agent_id: String,
    pub configuration_epoch: Option<String>,
    pub origin_turn_id: Option<String>,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub dependencies: Vec<rua_task::TaskDependencyResult>,
    pub cancel: tokio_util::sync::CancellationToken,
    pub session: Arc<rua_task::TaskRunnerSession>,
}

#[async_trait]
pub trait ToolTaskExecution: Send + Sync {
    async fn execute(
        &self,
        request: ToolTaskExecutionRequest,
    ) -> Result<agent_types::ToolResult, String>;
}

pub type RunningToolResultFuture =
    Pin<Box<dyn Future<Output = agent_types::ToolResult> + Send + 'static>>;

pub struct AdoptRunningToolRequest {
    pub task_id: TaskId,
    pub owner: TaskOwner,
    pub authorization_agent_id: String,
    pub configuration_epoch: Option<String>,
    pub origin_turn_id: Option<String>,
    pub origin_tool_call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub instruction: String,
    pub result: RunningToolResultFuture,
    pub cancel: CancellationToken,
    pub session: Option<Arc<rua_task::TaskRunnerSession>>,
}

pub async fn adopt_running_tool(
    runtime: Arc<TaskRuntime>,
    request: AdoptRunningToolRequest,
) -> Result<TaskId, TaskError> {
    let AdoptRunningToolRequest {
        task_id,
        owner,
        authorization_agent_id,
        configuration_epoch,
        origin_turn_id,
        origin_tool_call_id,
        tool_name,
        arguments,
        instruction,
        result,
        cancel,
        session,
    } = request;
    let source_device_id = runtime.device_context().device_id.clone();
    let result_task_id = task_id.clone();
    let result_tool_name = tool_name.clone();
    let mut adopted = TaskAdoptedExecution::new(
        async move {
            adapt_tool_task_result(
                &result_task_id,
                1,
                &result_tool_name,
                result.await,
                source_device_id.as_deref(),
            )
        },
        cancel,
    );
    if let Some(session) = session {
        adopted = adopted.with_session(session);
    }
    let mut authorization = rua_task::TaskAttemptAuthorization::new(authorization_agent_id)
        .map_err(TaskError::Invalid)?;
    authorization.configuration_epoch = configuration_epoch;
    runtime
        .adopt_running(
            task_id,
            TaskSpec {
                owner,
                instruction,
                runner: TaskRunnerSpec::Tool {
                    tool_name,
                    arguments,
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: TaskOrigin {
                    turn_id: origin_turn_id,
                    tool_call_id: Some(origin_tool_call_id),
                },
            },
            authorization,
            adopted,
        )
        .await
}

pub struct ToolTaskRunner {
    execution: std::sync::Arc<dyn ToolTaskExecution>,
}

impl ToolTaskRunner {
    pub fn new(execution: std::sync::Arc<dyn ToolTaskExecution>) -> Self {
        Self { execution }
    }
}

#[async_trait]
impl TaskRunner for ToolTaskRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        let (tool_name, arguments) = match request.spec.runner.clone() {
            TaskRunnerSpec::Tool {
                tool_name,
                arguments,
            } => (tool_name, arguments),
            other => {
                return Err(TaskRunnerError::new(format!(
                    "Tool Task Runner cannot execute {} Runner specification",
                    other.kind()
                )));
            }
        };

        if tool_name.starts_with("Task_") {
            return Err(TaskRunnerError::new(format!(
                "tool '{tool_name}' cannot be executed through Tool Runner"
            )));
        }

        let result = self
            .execution
            .execute(ToolTaskExecutionRequest {
                task_id: request.task_id.clone(),
                attempt_no: request.attempt_no,
                owner: request.spec.owner.clone(),
                authorization_agent_id: request.authorization_agent_id,
                configuration_epoch: request.configuration_epoch,
                origin_turn_id: request.spec.origin.turn_id.clone(),
                tool_name: tool_name.clone(),
                arguments,
                dependencies: request.dependencies,
                cancel: request.cancel,
                session: request.session,
            })
            .await
            .map_err(TaskRunnerError::new)?;

        Ok(adapt_tool_task_result(
            &request.task_id,
            request.attempt_no,
            &tool_name,
            result,
            request.device.device_id.as_deref(),
        ))
    }
}

pub fn adapt_tool_task_result(
    task_id: &str,
    attempt_no: u32,
    tool_name: &str,
    result: agent_types::ToolResult,
    source_device_id: Option<&str>,
) -> TaskRunOutput {
    let raw_output = serde_json::to_vec(&result).ok();
    let text = result.text_content();
    let (preview, bounded) = bounded_preview(&text, RESULT_PREVIEW_CHARS);
    let outcome = if result.is_error {
        TaskRunOutcome::Failed
    } else {
        TaskRunOutcome::Completed
    };

    let outcome_text = if result.is_error {
        format!("Tool `{tool_name}` reported failure.")
    } else {
        format!("Tool `{tool_name}` completed.")
    };
    let key_results = if preview.trim().is_empty() {
        "(Tool returned no text output.)"
    } else {
        preview.as_str()
    };
    let output = format!(
        "{outcome_text}\n{}{}",
        crate::executor::plain_text(key_results.as_bytes()),
        if bounded || result.truncation.is_some() {
            "\nPreview limited; use task_read for the stored result."
        } else {
            ""
        }
    );

    let artifacts = result
        .attachments
        .as_ref()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, attachment)| TaskArtifact {
            id: format!("{task_id}:{attempt_no}:attachment:{index}"),
            name: attachment.name.clone(),
            media_type: attachment.mime_type.clone(),
            resource: TaskResourceRef {
                uri: format!("task://{task_id}/attempts/{attempt_no}/artifacts/{index}"),
                ref_type: "task_attachment".to_string(),
                source_device_id: source_device_id.map(str::to_string),
            },
            metadata: serde_json::json!({
                "content_externalization_pending": true
            }),
        })
        .collect();
    let artifact_payloads = result
        .attachments
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, attachment)| TaskArtifactPayload {
            artifact_id: format!("{task_id}:{attempt_no}:attachment:{index}"),
            bytes: attachment.content.into_bytes(),
        })
        .collect();

    TaskRunOutput {
        outcome,
        output,
        raw_output,
        artifacts,
        artifact_payloads,
        error: result.is_error.then(|| TaskErrorInfo {
            kind: rua_task::TaskErrorKind::Runner,
            message: if text.trim().is_empty() {
                format!("tool '{tool_name}' failed")
            } else {
                text
            },
            retryable: false,
            details: serde_json::Value::Null,
        }),
    }
}

fn bounded_preview(value: &str, max_chars: usize) -> (String, bool) {
    let mut chars = value.chars();
    let preview: String = chars.by_ref().take(max_chars).collect();
    let truncated = chars.next().is_some();
    if truncated {
        (format!("{preview}\n\n[bounded preview]"), true)
    } else {
        (preview, false)
    }
}
