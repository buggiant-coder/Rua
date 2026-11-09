use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::model::{
    TaskArtifact, TaskDependencyResult, TaskDeviceContext, TaskErrorInfo, TaskErrorKind, TaskId,
    TaskLiveOutputProjection, TaskOwner, TaskRecord, TaskRunnerKind, TaskSpec,
};
use crate::session::{TaskLiveOutputRecord, TaskRunnerSession};

pub const TASK_COMPLETION_PROMPT: &str = "\
Complete the Task with a concise natural-language report using exactly these \
sections: Outcome, Key Results, Verification, and Remaining Work. \
Remaining Work is mandatory; write None when no work remains. \
Do not execute or imply follow-up Tasks. You may describe inert Task proposals \
for the owning Chat Agent to review.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRunOutcome {
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone)]
pub struct TaskArtifactPayload {
    pub artifact_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TaskRunOutput {
    pub outcome: TaskRunOutcome,
    pub output: String,
    pub raw_output: Option<Vec<u8>>,
    pub artifacts: Vec<TaskArtifact>,
    pub artifact_payloads: Vec<TaskArtifactPayload>,
    pub error: Option<TaskErrorInfo>,
}

impl TaskRunOutput {
    pub fn completed(output: impl Into<String>) -> Self {
        Self {
            outcome: TaskRunOutcome::Completed,
            output: output.into(),
            raw_output: None,
            artifacts: Vec::new(),
            artifact_payloads: Vec::new(),
            error: None,
        }
    }

    pub fn failed(output: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            outcome: TaskRunOutcome::Failed,
            output: output.into(),
            raw_output: None,
            artifacts: Vec::new(),
            artifact_payloads: Vec::new(),
            error: Some(TaskErrorInfo {
                kind: TaskErrorKind::Runner,
                message: message.into(),
                retryable: false,
                details: serde_json::Value::Null,
            }),
        }
    }

    pub fn cancelled() -> Self {
        Self {
            outcome: TaskRunOutcome::Cancelled,
            output: "Task cancelled. Inspect prior side effects before retrying.".to_string(),
            raw_output: None,
            artifacts: Vec::new(),
            artifact_payloads: Vec::new(),
            error: Some(TaskErrorInfo {
                kind: TaskErrorKind::Runner,
                message: "task cancelled".to_string(),
                retryable: false,
                details: serde_json::Value::Null,
            }),
        }
    }

    pub fn interrupted(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            outcome: TaskRunOutcome::Interrupted,
            output: format!(
                "Task interrupted: {message}\nInspect prior side effects before retrying."
            ),
            raw_output: None,
            artifacts: Vec::new(),
            artifact_payloads: Vec::new(),
            error: Some(TaskErrorInfo {
                kind: TaskErrorKind::Runner,
                message,
                retryable: false,
                details: serde_json::Value::Null,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskRunRequest {
    pub task_id: TaskId,
    pub attempt_no: u32,
    pub authorization_agent_id: String,
    pub configuration_epoch: Option<String>,
    pub spec: TaskSpec,
    pub task_dir: PathBuf,
    pub device: TaskDeviceContext,
    pub dependencies: Vec<TaskDependencyResult>,
    pub cancel: CancellationToken,
    pub session: std::sync::Arc<TaskRunnerSession>,
}

pub type TaskAdoptedFuture = Pin<Box<dyn Future<Output = TaskRunOutput> + Send + 'static>>;

pub struct TaskAdoptedExecution {
    pub future: TaskAdoptedFuture,
    pub cancel: CancellationToken,
    pub session: Option<std::sync::Arc<TaskRunnerSession>>,
}

impl TaskAdoptedExecution {
    pub fn new(
        future: impl Future<Output = TaskRunOutput> + Send + 'static,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            future: Box::pin(future),
            cancel,
            session: None,
        }
    }

    pub fn with_session(mut self, session: std::sync::Arc<TaskRunnerSession>) -> Self {
        self.session = Some(session);
        self
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct TaskRunnerError {
    pub message: String,
    pub retryable: bool,
}

impl TaskRunnerError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

#[async_trait]
pub trait TaskRunner: Send + Sync {
    fn kind(&self) -> TaskRunnerKind;

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError>;
}

#[derive(Debug, Clone)]
pub struct TaskCompletionNotification {
    pub record: TaskRecord,
}

impl TaskCompletionNotification {
    pub fn owner(&self) -> &TaskOwner {
        &self.record.spec.owner
    }
}

#[derive(Debug, Clone)]
pub struct TaskWatchOutputNotification {
    pub notification_id: String,
    pub owner: TaskOwner,
    pub task_id: TaskId,
    pub attempt_no: u32,
    pub origin_turn_id: Option<String>,
    pub projection: TaskLiveOutputProjection,
    pub records: Vec<TaskLiveOutputRecord>,
}

#[async_trait]
pub trait TaskNotificationSink: Send + Sync {
    async fn watch_output(
        &self,
        _notification: TaskWatchOutputNotification,
    ) -> Result<(), TaskRunnerError> {
        Ok(())
    }

    async fn terminal(
        &self,
        notification: TaskCompletionNotification,
    ) -> Result<(), TaskRunnerError>;
}

pub struct NoopTaskNotificationSink;

#[async_trait]
impl TaskNotificationSink for NoopTaskNotificationSink {
    async fn terminal(
        &self,
        _notification: TaskCompletionNotification,
    ) -> Result<(), TaskRunnerError> {
        Ok(())
    }
}
