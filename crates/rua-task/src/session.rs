use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskInputCapability {
    #[default]
    None,
    Stdin,
    SteeringMessage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskInput {
    Text { text: String },
    CloseStdin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskInputSubmission {
    pub submission_id: String,
    pub input: TaskInput,
}

impl TaskInputSubmission {
    pub fn new(submission_id: impl Into<String>, input: TaskInput) -> Result<Self, String> {
        let submission_id = submission_id.into();
        if submission_id.trim().is_empty() {
            return Err("Task Input submission ID must not be empty".to_string());
        }
        Ok(Self {
            submission_id,
            input,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskInputAcceptance {
    pub accepted: bool,
    pub task_id: String,
    pub attempt_no: u32,
    pub input_id: String,
    pub input_seq: u64,
    pub input_capability: TaskInputCapability,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskInputError {
    #[error("Task Runner Session input is unavailable")]
    Unavailable,
    #[error("Task Input is not supported by this Runner Session")]
    Unsupported,
    #[error("Task Runner Session rejected input: {0}")]
    Rejected(String),
    #[error("Task Input submission '{submission_id}' was reused with different content")]
    SubmissionConflict { submission_id: String },
    #[error("Task Runner Session is not bound to a Task Attempt")]
    Unbound,
    #[error("Task Runner Session is already bound to another Task Attempt")]
    AlreadyBound,
}

#[async_trait]
pub trait TaskInputSink: Send + Sync {
    fn declared_capability(&self) -> TaskInputCapability;

    fn active_capability(&self) -> TaskInputCapability;

    async fn send_input(&self, input: &TaskInput) -> Result<(), TaskInputError>;

    async fn close(&self);
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLiveOutputCursor {
    pub positions: BTreeMap<String, u64>,
    pub pending: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLiveOutputRecord {
    pub source: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskWatchStatus {
    pub task_id: String,
    pub attempt_no: u32,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct TaskLiveOutputBatch {
    pub records: Vec<TaskLiveOutputRecord>,
    pub next_cursor: TaskLiveOutputCursor,
    pub sealed: bool,
    pub has_more: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct TaskLiveOutputError {
    pub message: String,
}

impl TaskLiveOutputError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait TaskLiveOutputSource: Send + Sync {
    async fn end_cursor(&self) -> Result<TaskLiveOutputCursor, TaskLiveOutputError>;

    async fn read_records(
        &self,
        cursor: &TaskLiveOutputCursor,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<TaskLiveOutputBatch, TaskLiveOutputError>;

    async fn seal(&self);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionIdentity {
    task_id: String,
    attempt_no: u32,
}

struct AcceptedInput {
    input: TaskInput,
    acceptance: TaskInputAcceptance,
}

struct SessionState {
    sink: Option<Arc<dyn TaskInputSink>>,
    closed: bool,
    next_input_seq: u64,
    accepted: HashMap<String, AcceptedInput>,
}

pub struct TaskRunnerSession {
    identity: StdMutex<Option<SessionIdentity>>,
    output_dir: StdMutex<Option<std::path::PathBuf>>,
    state: Mutex<SessionState>,
    live_output: Mutex<Option<Arc<dyn TaskLiveOutputSource>>>,
    live_output_activation_cursor: Mutex<Option<TaskLiveOutputCursor>>,
    live_output_tx: broadcast::Sender<()>,
}

impl std::fmt::Debug for TaskRunnerSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskRunnerSession")
            .field(
                "identity",
                &self
                    .identity
                    .lock()
                    .expect("Task Runner Session identity lock poisoned"),
            )
            .finish_non_exhaustive()
    }
}

impl TaskRunnerSession {
    pub fn new() -> Arc<Self> {
        let (live_output_tx, _) = broadcast::channel(64);
        Arc::new(Self {
            identity: StdMutex::new(None),
            output_dir: StdMutex::new(None),
            state: Mutex::new(SessionState {
                sink: None,
                closed: false,
                next_input_seq: 1,
                accepted: HashMap::new(),
            }),
            live_output: Mutex::new(None),
            live_output_activation_cursor: Mutex::new(None),
            live_output_tx,
        })
    }

    pub fn bound(task_id: impl Into<String>, attempt_no: u32) -> Arc<Self> {
        let session = Self::new();
        session
            .bind(task_id, attempt_no)
            .expect("new Task Runner Session must bind");
        session
    }

    pub(crate) fn set_output_dir(&self, path: std::path::PathBuf) {
        let mut directory = self
            .output_dir
            .lock()
            .expect("Task output directory lock poisoned");
        if let Some(existing) = directory.as_ref() {
            assert_eq!(
                existing, &path,
                "Task output storage cannot change during execution"
            );
        } else {
            *directory = Some(path);
        }
    }

    pub fn output_dir(&self) -> Option<std::path::PathBuf> {
        self.output_dir
            .lock()
            .expect("Task output directory lock poisoned")
            .clone()
    }

    pub fn bind(&self, task_id: impl Into<String>, attempt_no: u32) -> Result<(), TaskInputError> {
        let identity = SessionIdentity {
            task_id: task_id.into(),
            attempt_no,
        };
        let mut current = self
            .identity
            .lock()
            .expect("Task Runner Session identity lock poisoned");
        match current.as_ref() {
            None => {
                *current = Some(identity);
                Ok(())
            }
            Some(existing) if existing == &identity => Ok(()),
            Some(_) => Err(TaskInputError::AlreadyBound),
        }
    }

    pub async fn activate(&self, sink: Arc<dyn TaskInputSink>) -> Result<(), TaskInputError> {
        let mut state = self.state.lock().await;
        if state.closed {
            sink.close().await;
            return Err(TaskInputError::Unavailable);
        }
        if state.sink.is_some() {
            sink.close().await;
            return Err(TaskInputError::Rejected(
                "Runner Session input surface is already active".to_string(),
            ));
        }
        state.sink = Some(sink);
        Ok(())
    }

    pub async fn input_capability(&self) -> TaskInputCapability {
        let state = self.state.lock().await;
        if state.closed {
            TaskInputCapability::None
        } else {
            state
                .sink
                .as_ref()
                .map(|sink| sink.active_capability())
                .unwrap_or_default()
        }
    }

    pub async fn is_closed(&self) -> bool {
        self.state.lock().await.closed
    }

    pub async fn activate_live_output(
        &self,
        source: Arc<dyn TaskLiveOutputSource>,
    ) -> Result<(), TaskLiveOutputError> {
        let activation_cursor = source.end_cursor().await?;
        let mut live_output = self.live_output.lock().await;
        if live_output.is_some() {
            return Err(TaskLiveOutputError::new(
                "Runner Session live output is already active",
            ));
        }
        *live_output = Some(source);
        *self.live_output_activation_cursor.lock().await = Some(activation_cursor);
        drop(live_output);
        self.notify_live_output();
        Ok(())
    }

    pub async fn live_output(&self) -> Option<Arc<dyn TaskLiveOutputSource>> {
        self.live_output.lock().await.clone()
    }

    pub async fn live_output_activation_cursor(&self) -> Option<TaskLiveOutputCursor> {
        self.live_output_activation_cursor.lock().await.clone()
    }

    pub fn subscribe_live_output(&self) -> broadcast::Receiver<()> {
        self.live_output_tx.subscribe()
    }

    pub fn notify_live_output(&self) {
        let _ = self.live_output_tx.send(());
    }

    pub async fn send_input(
        &self,
        submission: TaskInputSubmission,
    ) -> Result<TaskInputAcceptance, TaskInputError> {
        let identity = self
            .identity
            .lock()
            .expect("Task Runner Session identity lock poisoned")
            .clone()
            .ok_or(TaskInputError::Unbound)?;
        let mut state = self.state.lock().await;
        if let Some(existing) = state.accepted.get(&submission.submission_id) {
            if existing.input == submission.input {
                return Ok(existing.acceptance.clone());
            }
            return Err(TaskInputError::SubmissionConflict {
                submission_id: submission.submission_id,
            });
        }
        if state.closed {
            return Err(TaskInputError::Unavailable);
        }
        let sink = state.sink.clone().ok_or(TaskInputError::Unavailable)?;
        if sink.declared_capability() == TaskInputCapability::None {
            return Err(TaskInputError::Unsupported);
        }

        sink.send_input(&submission.input).await?;

        let acceptance = TaskInputAcceptance {
            accepted: true,
            task_id: identity.task_id,
            attempt_no: identity.attempt_no,
            input_id: uuid::Uuid::new_v4().to_string(),
            input_seq: state.next_input_seq,
            input_capability: sink.active_capability(),
        };
        state.next_input_seq += 1;
        state.accepted.insert(
            submission.submission_id,
            AcceptedInput {
                input: submission.input,
                acceptance: acceptance.clone(),
            },
        );
        Ok(acceptance)
    }

    pub async fn close_input(&self) {
        {
            let mut state = self.state.lock().await;
            if !state.closed {
                state.closed = true;
                if let Some(sink) = state.sink.as_ref() {
                    sink.close().await;
                }
            }
        }
    }

    pub async fn seal_live_output(&self) {
        if let Some(source) = self.live_output().await {
            source.seal().await;
            self.notify_live_output();
        }
    }

    pub async fn close(&self) {
        self.close_input().await;
        self.seal_live_output().await;
    }
}
