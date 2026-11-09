use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use rua_core::ChatScopeStore;
use tokio::sync::{broadcast, Mutex as AsyncMutex, Notify, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::atomic_file;
use crate::authority::{TaskAuthorityError, TaskAuthorityResolver};
use crate::model::{
    TaskArtifact, TaskDeviceContext, TaskErrorInfo, TaskEvent, TaskEventKind, TaskId,
    TaskListFilter, TaskOwner, TaskRecord, TaskResourceRef, TaskResult, TaskRunnerKind, TaskSpec,
    TaskState,
};
use crate::runner::{
    NoopTaskNotificationSink, TaskAdoptedExecution, TaskCompletionNotification,
    TaskNotificationSink, TaskRunOutcome, TaskRunOutput, TaskRunRequest, TaskRunner,
    TaskWatchOutputNotification,
};
use crate::session::{
    TaskInputAcceptance, TaskInputSubmission, TaskLiveOutputCursor, TaskLiveOutputError,
    TaskLiveOutputRecord, TaskLiveOutputSource, TaskRunnerSession, TaskWatchStatus,
};
use crate::store::TaskStore;

const WATCH_BATCH_MAX_RECORDS: usize = 100;
const WATCH_BATCH_MAX_BYTES: usize = 64 * 1024;
const WATCH_WAKE_QUIET_PERIOD: Duration = Duration::from_millis(250);
const WATCH_WAKE_MAX_DELAY: Duration = Duration::from_secs(2);
const WATCH_OMISSION_MARKER_BYTES: usize = 256;
const RUNTIME_SHUTDOWN_MESSAGE: &str = "runtime shutdown interrupted the Task";

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("task {0} not found")]
    NotFound(String),
    #[error("invalid task: {0}")]
    Invalid(String),
    #[error("invalid task transition: {0}")]
    InvalidTransition(String),
    #[error("task runner '{0}' is not registered")]
    RunnerUnavailable(TaskRunnerKind),
    #[error("task store error: {0}")]
    Store(String),
    #[error("task I/O error: {0}")]
    Io(String),
    #[error("task runtime limit exceeded: {0}")]
    LimitExceeded(String),
    #[error("timed out waiting for task {0}")]
    WaitTimedOut(String),
    #[error("task authority rejected execution: {0}")]
    AuthorityRejected(String),
    #[error("task authority is unavailable: {0}")]
    AuthorityUnavailable(String),
    #[error("task resource is unavailable: {0}")]
    ResourceUnavailable(String),
    #[error("task input is unavailable: {0}")]
    InputUnavailable(String),
    #[error("task input submission conflicts with an earlier delivery: {0}")]
    InputConflict(String),
}

impl From<TaskAuthorityError> for TaskError {
    fn from(error: TaskAuthorityError) -> Self {
        match error {
            TaskAuthorityError::Rejected(message) => Self::AuthorityRejected(message),
            TaskAuthorityError::Unavailable(message) => Self::AuthorityUnavailable(message),
        }
    }
}

impl TaskError {
    pub(crate) fn store(error: impl std::fmt::Display) -> Self {
        Self::Store(error.to_string())
    }

    pub(crate) fn serialize(error: impl std::fmt::Display) -> Self {
        Self::Store(format!("serialization failed: {error}"))
    }

    pub(crate) fn io(error: impl std::fmt::Display) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct TaskRuntimeConfig {
    pub data_dir: PathBuf,
    pub device: TaskDeviceContext,
    pub limits: TaskRuntimeLimits,
}

#[derive(Debug, Clone)]
pub struct TaskReadSnapshot {
    pub task: TaskRecord,
    pub records: Vec<TaskLiveOutputRecord>,
    pub next_cursor: TaskLiveOutputCursor,
    pub has_more: bool,
    pub timed_out: bool,
}

impl TaskRuntimeConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            device: TaskDeviceContext::default(),
            limits: TaskRuntimeLimits::default(),
        }
    }

    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("task-runtime.sqlite")
    }

    pub fn task_dir(&self, owner: &TaskOwner, task_id: &str) -> PathBuf {
        ChatScopeStore::resolve(&self.data_dir, owner.chat_address.clone())
            .root()
            .join("sessions")
            .join(owner.chat_id())
            .join("tasks")
            .join(task_id)
    }

    pub fn attempt_dir(&self, owner: &TaskOwner, task_id: &str, attempt_no: u32) -> PathBuf {
        self.task_dir(owner, task_id)
            .join("attempts")
            .join(attempt_no.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct TaskRuntimeLimits {
    pub max_active_tasks_per_chat: usize,
    pub max_concurrent_tasks_per_chat: usize,
    pub max_concurrent_tasks_per_runner: usize,
    pub activity_channel_capacity: usize,
    pub max_result_output_chars: usize,
    pub max_raw_result_bytes: usize,
    pub max_attempt_history: usize,
    pub max_dependencies_per_task: usize,
    pub max_dependency_depth: usize,
    pub cancellation_grace: Duration,
}

impl Default for TaskRuntimeLimits {
    fn default() -> Self {
        Self {
            max_active_tasks_per_chat: 128,
            max_concurrent_tasks_per_chat: 4,
            max_concurrent_tasks_per_runner: 8,
            activity_channel_capacity: 512,
            max_result_output_chars: 16_000,
            max_raw_result_bytes: 1024 * 1024,
            max_attempt_history: 20,
            max_dependencies_per_task: 16,
            max_dependency_depth: 16,
            cancellation_grace: Duration::from_secs(5),
        }
    }
}

impl TaskRuntimeLimits {
    fn validate(&self) -> Result<(), TaskError> {
        if self.max_active_tasks_per_chat == 0
            || self.max_concurrent_tasks_per_chat == 0
            || self.max_concurrent_tasks_per_runner == 0
            || self.activity_channel_capacity == 0
            || self.max_result_output_chars == 0
            || self.max_raw_result_bytes == 0
            || self.max_attempt_history == 0
            || self.max_dependencies_per_task == 0
            || self.max_dependency_depth == 0
            || self.cancellation_grace.is_zero()
        {
            return Err(TaskError::Invalid(
                "Task Runtime limits must all be finite and greater than zero".to_string(),
            ));
        }
        Ok(())
    }
}

struct StaticTaskAuthorityResolver {
    device: TaskDeviceContext,
}

#[async_trait]
impl TaskAuthorityResolver for StaticTaskAuthorityResolver {
    async fn accept(&self, _owner: &TaskOwner) -> Result<TaskDeviceContext, TaskAuthorityError> {
        Ok(self.device.clone())
    }

    async fn is_current(
        &self,
        _owner: &TaskOwner,
        _accepted: &TaskDeviceContext,
    ) -> Result<bool, TaskAuthorityError> {
        Ok(true)
    }

    async fn local_device_id(&self) -> Result<Option<String>, TaskAuthorityError> {
        Ok(self.device.device_id.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskStopReason {
    None = 0,
    Cancelled = 1,
    Interrupted = 2,
    RuntimeShutdown = 3,
}

#[derive(Clone)]
struct TaskExecutionHandle {
    owner: TaskOwner,
    attempt_no: u32,
    cancel: CancellationToken,
    stop_reason: Arc<AtomicU8>,
    session: Arc<TaskRunnerSession>,
}

#[derive(Clone)]
struct TaskSessionEntry {
    owner: TaskOwner,
    attempt_no: u32,
    session: Arc<TaskRunnerSession>,
}

#[derive(Clone)]
struct TaskWatcherRegistration {
    owner: TaskOwner,
    attempt_no: u32,
    origin_turn_id: Option<String>,
    projection: crate::model::TaskLiveOutputProjection,
    root_cancel: CancellationToken,
    attempt_cancel: CancellationToken,
}

struct TaskWatcherAttempt {
    task_id: TaskId,
    owner: TaskOwner,
    attempt_no: u32,
    session: Arc<TaskRunnerSession>,
    origin_turn_id: Option<String>,
    projection: crate::model::TaskLiveOutputProjection,
}

struct TaskWatchSnapshot {
    records: Vec<TaskLiveOutputRecord>,
    next_cursor: TaskLiveOutputCursor,
    sealed: bool,
}

struct BoundedWatchRecords {
    head: Vec<TaskLiveOutputRecord>,
    tail: VecDeque<TaskLiveOutputRecord>,
    head_bytes: usize,
    tail_bytes: usize,
    omitted_records: usize,
    omitted_bytes: usize,
}

impl BoundedWatchRecords {
    fn new() -> Self {
        Self {
            head: Vec::new(),
            tail: VecDeque::new(),
            head_bytes: 0,
            tail_bytes: 0,
            omitted_records: 0,
            omitted_bytes: 0,
        }
    }

    fn push(&mut self, mut record: TaskLiveOutputRecord) {
        let byte_budget = (WATCH_BATCH_MAX_BYTES.saturating_sub(WATCH_OMISSION_MARKER_BYTES)) / 2;
        let original_bytes = record.text.len();
        record.text = bounded_watch_record_text(&record.text, byte_budget);
        self.omitted_bytes = self
            .omitted_bytes
            .saturating_add(original_bytes.saturating_sub(record.text.len()));
        let record_bytes = record.text.len();

        if self.head.len() < WATCH_BATCH_MAX_RECORDS / 2
            && self.head_bytes.saturating_add(record_bytes) <= byte_budget
        {
            self.head_bytes = self.head_bytes.saturating_add(record_bytes);
            self.head.push(record);
            return;
        }

        self.tail_bytes = self.tail_bytes.saturating_add(record_bytes);
        self.tail.push_back(record);
        while self.tail.len() > WATCH_BATCH_MAX_RECORDS / 2 || self.tail_bytes > byte_budget {
            let Some(omitted) = self.tail.pop_front() else {
                break;
            };
            self.tail_bytes = self.tail_bytes.saturating_sub(omitted.text.len());
            self.omitted_records = self.omitted_records.saturating_add(1);
            self.omitted_bytes = self.omitted_bytes.saturating_add(omitted.text.len());
        }
    }

    fn into_records(mut self) -> Vec<TaskLiveOutputRecord> {
        let has_omission = self.omitted_records > 0 || self.omitted_bytes > 0;
        if has_omission
            && self.head.len().saturating_add(self.tail.len()) >= WATCH_BATCH_MAX_RECORDS
        {
            if let Some(omitted) = self.head.pop() {
                self.omitted_records = self.omitted_records.saturating_add(1);
                self.omitted_bytes = self.omitted_bytes.saturating_add(omitted.text.len());
            }
        }

        let mut records =
            Vec::with_capacity(self.head.len() + self.tail.len() + usize::from(has_omission));
        records.append(&mut self.head);
        if has_omission {
            records.push(TaskLiveOutputRecord {
                source: "task_watch".to_string(),
                text: format!(
                    "... [{} records omitted, {} bytes omitted] ...",
                    self.omitted_records, self.omitted_bytes
                ),
            });
        }
        records.extend(self.tail);
        records
    }
}

struct DispatchedTask {
    task_id: TaskId,
    attempt_no: u32,
    authorization_agent_id: String,
    configuration_epoch: Option<String>,
    spec: TaskSpec,
    device: TaskDeviceContext,
}

async fn wait_for_watch_output_window(
    changes: &mut broadcast::Receiver<()>,
    cancel: &CancellationToken,
) -> bool {
    let max_deadline = tokio::time::Instant::now() + WATCH_WAKE_MAX_DELAY;
    let quiet = tokio::time::sleep(WATCH_WAKE_QUIET_PERIOD);
    tokio::pin!(quiet);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return false,
            _ = &mut quiet => return true,
            _ = tokio::time::sleep_until(max_deadline) => return true,
            change = changes.recv() => {
                if matches!(change, Err(broadcast::error::RecvError::Closed)) {
                    return true;
                }
                quiet.as_mut().reset(tokio::time::Instant::now() + WATCH_WAKE_QUIET_PERIOD);
            }
        }
    }
}

async fn read_watch_snapshot(
    source: &dyn TaskLiveOutputSource,
    cursor: &TaskLiveOutputCursor,
) -> Result<TaskWatchSnapshot, TaskLiveOutputError> {
    let snapshot_end = source.end_cursor().await?;
    let mut next_cursor = cursor.clone();
    let mut records = BoundedWatchRecords::new();
    let mut sealed = false;

    loop {
        let batch = source
            .read_records(&next_cursor, WATCH_BATCH_MAX_RECORDS, WATCH_BATCH_MAX_BYTES)
            .await?;
        let progressed = batch.next_cursor != next_cursor;
        for record in batch.records {
            records.push(record);
        }
        next_cursor = batch.next_cursor;
        sealed |= batch.sealed;

        if sealed || cursor_reached(&next_cursor, &snapshot_end) || !progressed {
            break;
        }
    }

    Ok(TaskWatchSnapshot {
        records: records.into_records(),
        next_cursor,
        sealed,
    })
}

fn cursor_reached(cursor: &TaskLiveOutputCursor, target: &TaskLiveOutputCursor) -> bool {
    target
        .positions
        .iter()
        .all(|(source, position)| cursor.positions.get(source).copied().unwrap_or(0) >= *position)
}

fn bounded_watch_record_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let marker = format!("... [{} bytes omitted] ...", text.len());
    if max_bytes <= marker.len() {
        return marker.chars().take(max_bytes).collect();
    }
    let retained = max_bytes - marker.len();
    let head_budget = retained / 2;
    let tail_budget = retained - head_budget;
    let head_end = floor_char_boundary(text, head_budget);
    let tail_start = ceil_char_boundary(text, text.len().saturating_sub(tail_budget));
    format!("{}{}{}", &text[..head_end], marker, &text[tail_start..])
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

impl TaskExecutionHandle {
    fn new(owner: TaskOwner, attempt_no: u32, session: Arc<TaskRunnerSession>) -> Self {
        Self {
            owner,
            attempt_no,
            cancel: CancellationToken::new(),
            stop_reason: Arc::new(AtomicU8::new(TaskStopReason::None as u8)),
            session,
        }
    }

    fn request_cancel(&self) {
        let _ = self.stop_reason.compare_exchange(
            TaskStopReason::None as u8,
            TaskStopReason::Cancelled as u8,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        self.cancel.cancel();
    }

    fn request_interrupt(&self) {
        self.stop_reason
            .store(TaskStopReason::Interrupted as u8, Ordering::SeqCst);
        self.cancel.cancel();
    }

    fn request_shutdown(&self) {
        self.stop_reason
            .store(TaskStopReason::RuntimeShutdown as u8, Ordering::SeqCst);
        self.cancel.cancel();
    }

    fn stop_output(&self) -> TaskRunOutput {
        match self.stop_reason.load(Ordering::SeqCst) {
            reason if reason == TaskStopReason::Interrupted as u8 => {
                TaskRunOutput::interrupted("conversation authority changed")
            }
            reason if reason == TaskStopReason::RuntimeShutdown as u8 => runtime_shutdown_output(),
            _ => TaskRunOutput::cancelled(),
        }
    }

    fn stop_output_preserving(&self, mut output: TaskRunOutput) -> TaskRunOutput {
        let stopped = self.stop_output();
        output.outcome = stopped.outcome;
        output.error = stopped.error;
        if output.output.trim().is_empty() {
            output.output = stopped.output;
        }
        output
    }
}

pub struct TaskRuntime {
    config: TaskRuntimeConfig,
    store: TaskStore,
    runners: RwLock<HashMap<TaskRunnerKind, Arc<dyn TaskRunner>>>,
    notification_sink: Arc<dyn TaskNotificationSink>,
    authority: Arc<dyn TaskAuthorityResolver>,
    executions: StdMutex<HashMap<TaskId, TaskExecutionHandle>>,
    sessions: StdMutex<HashMap<TaskId, TaskSessionEntry>>,
    watchers: StdMutex<HashMap<TaskId, TaskWatcherRegistration>>,
    execution_change: Notify,
    chat_permits: AsyncMutex<HashMap<TaskOwner, Arc<Semaphore>>>,
    runner_permits: AsyncMutex<HashMap<TaskRunnerKind, Arc<Semaphore>>>,
    admission_open: AsyncMutex<bool>,
    paused_agents: StdMutex<std::collections::HashSet<String>>,
    activity_tx: broadcast::Sender<TaskEvent>,
    next_event_sequence: AtomicU64,
}

impl TaskRuntime {
    fn ensure_tool_task_spec(spec: &TaskSpec) -> Result<(), TaskError> {
        if spec.runner.kind() != TaskRunnerKind::Tool {
            return Err(TaskError::Invalid(
                "Task only executes background Tools; use an independent Chat for Agent or ACP collaboration"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub fn open(config: TaskRuntimeConfig) -> Result<Self, TaskError> {
        let authority = Arc::new(StaticTaskAuthorityResolver {
            device: config.device.clone(),
        });
        Self::open_with_sink_and_authority(config, Arc::new(NoopTaskNotificationSink), authority)
    }

    pub fn open_with_sink(
        config: TaskRuntimeConfig,
        notification_sink: Arc<dyn TaskNotificationSink>,
    ) -> Result<Self, TaskError> {
        let authority = Arc::new(StaticTaskAuthorityResolver {
            device: config.device.clone(),
        });
        Self::open_with_sink_and_authority(config, notification_sink, authority)
    }

    pub fn open_with_sink_and_authority(
        config: TaskRuntimeConfig,
        notification_sink: Arc<dyn TaskNotificationSink>,
        authority: Arc<dyn TaskAuthorityResolver>,
    ) -> Result<Self, TaskError> {
        config.limits.validate()?;
        let store = TaskStore::open(config.database_path())?;
        let interrupted = store.reconcile_non_terminal(&config.device)?;
        let (activity_tx, _) = broadcast::channel(config.limits.activity_channel_capacity);
        let runtime = Self {
            config,
            store,
            runners: RwLock::new(HashMap::new()),
            notification_sink,
            authority,
            executions: StdMutex::new(HashMap::new()),
            sessions: StdMutex::new(HashMap::new()),
            watchers: StdMutex::new(HashMap::new()),
            execution_change: Notify::new(),
            chat_permits: AsyncMutex::new(HashMap::new()),
            runner_permits: AsyncMutex::new(HashMap::new()),
            admission_open: AsyncMutex::new(true),
            paused_agents: StdMutex::new(std::collections::HashSet::new()),
            activity_tx,
            next_event_sequence: AtomicU64::new(1),
        };
        for record in interrupted {
            runtime.publish_event(
                &record.spec.owner,
                &record.task_id,
                record.attempt_no,
                TaskEventKind::Interrupted,
                TaskState::Interrupted,
            );
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let sink = Arc::clone(&runtime.notification_sink);
                let authority = Arc::clone(&runtime.authority);
                handle.spawn(async move {
                    let accepted = record
                        .attempts
                        .last()
                        .map(|attempt| attempt.device.clone())
                        .unwrap_or_default();
                    if !matches!(
                        authority.is_current(&record.spec.owner, &accepted).await,
                        Ok(true)
                    ) {
                        return;
                    }
                    if let Err(error) = sink
                        .terminal(TaskCompletionNotification { record })
                        .await
                    {
                        tracing::warn!(error = %error, "failed to publish startup Task interruption");
                    }
                });
            }
        }
        Ok(runtime)
    }

    pub fn database_path(&self) -> &Path {
        self.store.path()
    }

    pub fn device_context(&self) -> &TaskDeviceContext {
        &self.config.device
    }

    pub fn register_runner(&self, runner: Arc<dyn TaskRunner>) {
        self.runners
            .write()
            .expect("task runner registry lock poisoned")
            .insert(runner.kind(), runner);
    }

    pub fn has_runner(&self, kind: TaskRunnerKind) -> bool {
        self.runners
            .read()
            .expect("task runner registry lock poisoned")
            .contains_key(&kind)
    }

    pub async fn create(
        self: &Arc<Self>,
        spec: TaskSpec,
        authorization: crate::model::TaskAttemptAuthorization,
    ) -> Result<TaskId, TaskError> {
        let admission = self.admission_open.lock().await;
        if self
            .paused_agents
            .lock()
            .expect("Task admission state poisoned")
            .contains(&authorization.agent_id)
        {
            return Err(TaskError::ResourceUnavailable(
                "Agent is shutting down and cannot accept new Tasks".to_string(),
            ));
        }
        if !*admission {
            return Err(TaskError::ResourceUnavailable(
                "Task Runtime is shutting down".to_string(),
            ));
        }
        spec.validate().map_err(TaskError::Invalid)?;
        Self::ensure_tool_task_spec(&spec)?;
        let device = self
            .authority
            .accept_attempt(&spec.owner, &authorization)
            .await?;
        let runner_kind = spec.runner.kind();
        let runner = self
            .runners
            .read()
            .expect("task runner registry lock poisoned")
            .get(&runner_kind)
            .cloned()
            .ok_or(TaskError::RunnerUnavailable(runner_kind))?;
        let task_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let record = TaskRecord {
            task_id: task_id.clone(),
            spec: spec.clone(),
            state: TaskState::Accepted,
            attempt_no: 1,
            input_capability: crate::session::TaskInputCapability::None,
            watch_active: false,
            created_at: now,
            updated_at: now,
            started_at: None,
            finished_at: None,
            result: None,
            attempts: Vec::new(),
            waiting_on: Vec::new(),
            blocked_by: Vec::new(),
        };
        let record = self
            .store
            .insert_with_limits(
                record,
                authorization,
                device,
                self.config.limits.max_active_tasks_per_chat,
                self.config.limits.max_dependencies_per_task,
                self.config.limits.max_dependency_depth,
            )
            .await?;
        self.publish_event(
            &spec.owner,
            &task_id,
            1,
            event_kind_for_state(record.state),
            record.state,
        );

        if record.state == TaskState::Accepted {
            self.dispatch(record, runner)?;
        }
        Ok(task_id)
    }

    pub async fn adopt_running(
        self: &Arc<Self>,
        task_id: TaskId,
        spec: TaskSpec,
        authorization: crate::model::TaskAttemptAuthorization,
        execution: TaskAdoptedExecution,
    ) -> Result<TaskId, TaskError> {
        let admission = self.admission_open.lock().await;
        if self
            .paused_agents
            .lock()
            .expect("Task admission state poisoned")
            .contains(&authorization.agent_id)
        {
            execution.cancel.cancel();
            return Err(TaskError::ResourceUnavailable(
                "Agent is shutting down and cannot accept new Tasks".to_string(),
            ));
        }
        if !*admission {
            execution.cancel.cancel();
            return Err(TaskError::ResourceUnavailable(
                "Task Runtime is shutting down".to_string(),
            ));
        }
        Self::ensure_tool_task_spec(&spec)?;
        spec.validate().map_err(TaskError::Invalid)?;
        if task_id.trim().is_empty() {
            execution.cancel.cancel();
            return Err(TaskError::Invalid(
                "adopted Task ID must not be empty".to_string(),
            ));
        }
        if !spec.depends_on.is_empty() {
            execution.cancel.cancel();
            return Err(TaskError::Invalid(
                "an already-running Task cannot declare dependencies".to_string(),
            ));
        }
        let runner_kind = spec.runner.kind();
        if !self.has_runner(runner_kind) {
            execution.cancel.cancel();
            return Err(TaskError::RunnerUnavailable(runner_kind));
        }
        let device = match self.authority.accept(&spec.owner).await {
            Ok(device) => device,
            Err(error) => {
                execution.cancel.cancel();
                return Err(error.into());
            }
        };

        let now = Utc::now();
        let record = TaskRecord {
            task_id: task_id.clone(),
            spec: spec.clone(),
            state: TaskState::Accepted,
            attempt_no: 1,
            input_capability: crate::session::TaskInputCapability::None,
            watch_active: false,
            created_at: now,
            updated_at: now,
            started_at: None,
            finished_at: None,
            result: None,
            attempts: Vec::new(),
            waiting_on: Vec::new(),
            blocked_by: Vec::new(),
        };
        let record = match self
            .store
            .insert_with_limits(
                record,
                authorization,
                device,
                self.config.limits.max_active_tasks_per_chat,
                self.config.limits.max_dependencies_per_task,
                self.config.limits.max_dependency_depth,
            )
            .await
        {
            Ok(record) => record,
            Err(error) => {
                execution.cancel.cancel();
                return Err(error);
            }
        };
        self.publish_event(
            &spec.owner,
            &task_id,
            record.attempt_no,
            TaskEventKind::Accepted,
            TaskState::Accepted,
        );

        let session = execution
            .session
            .clone()
            .unwrap_or_else(TaskRunnerSession::new);
        if let Err(error) = session.bind(task_id.clone(), record.attempt_no) {
            execution.cancel.cancel();
            return Err(TaskError::InputUnavailable(error.to_string()));
        }
        let task_execution =
            TaskExecutionHandle::new(spec.owner.clone(), record.attempt_no, session);
        self.insert_execution(task_id.clone(), task_execution.clone());
        if let Err(error) = self
            .store
            .mark_running(task_id.clone(), record.attempt_no, Utc::now())
            .await
        {
            self.remove_execution(&task_id);
            execution.cancel.cancel();
            return Err(error);
        }
        self.publish_event(
            &spec.owner,
            &task_id,
            record.attempt_no,
            TaskEventKind::Running,
            TaskState::Running,
        );

        let runtime = Arc::clone(self);
        let adopted_task_id = task_id.clone();
        tokio::spawn(async move {
            runtime
                .run_adopted_task(
                    adopted_task_id,
                    record.attempt_no,
                    spec,
                    execution,
                    task_execution,
                )
                .await;
        });
        Ok(task_id)
    }

    pub fn prepare_tool_session(
        &self,
        owner: &TaskOwner,
        task_id: &str,
    ) -> Result<Arc<TaskRunnerSession>, TaskError> {
        uuid::Uuid::parse_str(task_id)
            .map_err(|_| TaskError::Invalid("reserved Task ID must be a UUID".into()))?;
        let session = TaskRunnerSession::bound(task_id, 1);
        session.set_output_dir(self.config.attempt_dir(owner, task_id, 1).join("output"));
        Ok(session)
    }

    pub async fn get(&self, owner: &TaskOwner, task_id: &str) -> Result<TaskRecord, TaskError> {
        let record = self.store.get(owner.clone(), task_id.to_string()).await?;
        Ok(self.with_input_capability(record).await)
    }

    pub async fn set_waiting_approval(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        attempt_no: u32,
        waiting: bool,
    ) -> Result<bool, TaskError> {
        let record = self.store.get(owner.clone(), task_id.to_string()).await?;
        if record.attempt_no != attempt_no || record.state.is_terminal() {
            return Ok(false);
        }
        let changed = self
            .store
            .set_waiting_approval(task_id.to_string(), attempt_no, waiting)
            .await?;
        if changed {
            self.publish_event(
                owner,
                task_id,
                attempt_no,
                if waiting {
                    TaskEventKind::Waiting
                } else {
                    TaskEventKind::Running
                },
                if waiting {
                    TaskState::WaitingApproval
                } else {
                    TaskState::Running
                },
            );
        }
        Ok(changed)
    }

    pub async fn list(
        &self,
        owner: &TaskOwner,
        filter: TaskListFilter,
    ) -> Result<Vec<TaskRecord>, TaskError> {
        let records = self.store.list(owner.clone(), filter).await?;
        let mut resolved = Vec::with_capacity(records.len());
        for record in records {
            resolved.push(self.with_input_capability(record).await);
        }
        Ok(resolved)
    }

    /// Enumerates terminal Task Attempts whose result is durable but has not yet been
    /// acknowledged as delivered to the owning source, oldest delivery first.
    ///
    /// `Some(owner)` scopes the enumeration to one Chat; `None` returns every outstanding
    /// delivery, which is the shape a resumed server needs at startup. Each returned record
    /// is projected for the exact queued Attempt, so a delivery queued before a `retry`
    /// still reports that earlier Attempt's state and result.
    ///
    /// Deliveries survive process restarts. A successful `TaskNotificationSink::terminal`
    /// call does not acknowledge delivery; the source must call
    /// [`TaskRuntime::ack_terminal_delivery`] once the source input queue accepts the result.
    pub async fn pending_terminal_deliveries(
        &self,
        owner: Option<&TaskOwner>,
    ) -> Result<Vec<TaskRecord>, TaskError> {
        let records = self.store.list_pending_terminal(owner.cloned()).await?;
        let mut resolved = Vec::with_capacity(records.len());
        for record in records {
            resolved.push(self.with_input_capability(record).await);
        }
        Ok(resolved)
    }

    /// Acknowledges that the owning source input queue accepted the terminal result of one exact
    /// Task Attempt, removing it from the outstanding delivery set. Returns `true` when a
    /// delivery was outstanding for that Task, Attempt, and owner, and `false` otherwise, so
    /// repeat acknowledgements are idempotent. An owner mismatch never acknowledges another
    /// Chat's delivery.
    pub async fn ack_terminal_delivery(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        attempt_no: u32,
    ) -> Result<bool, TaskError> {
        self.store
            .ack_terminal_delivery(owner.clone(), task_id.to_string(), attempt_no)
            .await
    }

    pub async fn terminal_delivery_error(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        attempt_no: u32,
    ) -> Result<Option<String>, TaskError> {
        self.store
            .terminal_delivery_error(owner.clone(), task_id.into(), attempt_no)
            .await
    }

    pub async fn fail_terminal_delivery(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        attempt_no: u32,
        error: String,
    ) -> Result<(), TaskError> {
        self.store
            .fail_terminal_delivery(owner.clone(), task_id.into(), attempt_no, error)
            .await
    }

    pub async fn send_input(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        submission: TaskInputSubmission,
    ) -> Result<TaskInputAcceptance, TaskError> {
        let admission = self.admission_open.lock().await;
        if !*admission {
            return Err(TaskError::ResourceUnavailable(
                "Task Runtime is shutting down".to_string(),
            ));
        }
        let current = self.store.get(owner.clone(), task_id.to_string()).await?;
        let session = self
            .task_session(owner, task_id, current.attempt_no)
            .ok_or_else(|| {
                TaskError::InputUnavailable(
                    "the current Task Attempt has no live Runner Session".to_string(),
                )
            })?;
        session.send_input(submission).await.map_err(|error| {
            if matches!(
                error,
                crate::session::TaskInputError::SubmissionConflict { .. }
            ) {
                TaskError::InputConflict(error.to_string())
            } else {
                TaskError::InputUnavailable(error.to_string())
            }
        })
    }

    pub async fn watch(
        self: &Arc<Self>,
        owner: &TaskOwner,
        task_id: &str,
    ) -> Result<TaskWatchStatus, TaskError> {
        let admission = self.admission_open.lock().await;
        if !*admission {
            return Err(TaskError::ResourceUnavailable(
                "Task Runtime is shutting down".to_string(),
            ));
        }
        let current = self.get(owner, task_id).await?;
        Self::ensure_tool_task_spec(&current.spec)?;
        if current.state.is_terminal() {
            return Err(TaskError::InvalidTransition(format!(
                "cannot watch terminal Task '{task_id}'"
            )));
        }
        if let Some(existing) = self
            .watchers
            .lock()
            .expect("Task Watcher registry lock poisoned")
            .get(task_id)
            .cloned()
        {
            if existing.owner != *owner {
                return Err(TaskError::NotFound("task".to_string()));
            }
            return Ok(TaskWatchStatus {
                task_id: task_id.to_string(),
                attempt_no: existing.attempt_no,
                active: true,
            });
        }
        let session = self
            .task_session(owner, task_id, current.attempt_no)
            .ok_or_else(|| {
                TaskError::InvalidTransition(
                    "Task Attempt has no Runner Session to watch".to_string(),
                )
            })?;
        let initial_cursor = match session.live_output().await {
            Some(source) => Some(
                source
                    .end_cursor()
                    .await
                    .map_err(|error| TaskError::ResourceUnavailable(error.to_string()))?,
            ),
            None => None,
        };
        let root_cancel = CancellationToken::new();
        let attempt_cancel = root_cancel.child_token();
        self.watchers
            .lock()
            .expect("Task Watcher registry lock poisoned")
            .insert(
                task_id.to_string(),
                TaskWatcherRegistration {
                    owner: owner.clone(),
                    attempt_no: current.attempt_no,
                    origin_turn_id: current.spec.origin.turn_id.clone(),
                    projection: current.spec.runner.live_output_projection(),
                    root_cancel,
                    attempt_cancel: attempt_cancel.clone(),
                },
            );
        self.spawn_watcher_attempt(
            TaskWatcherAttempt {
                task_id: task_id.to_string(),
                owner: owner.clone(),
                attempt_no: current.attempt_no,
                session,
                origin_turn_id: current.spec.origin.turn_id,
                projection: current.spec.runner.live_output_projection(),
            },
            attempt_cancel,
            initial_cursor,
        );
        Ok(TaskWatchStatus {
            task_id: task_id.to_string(),
            attempt_no: current.attempt_no,
            active: true,
        })
    }

    pub async fn read(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        cursor: TaskLiveOutputCursor,
        max_records: usize,
        max_bytes: usize,
        wait: Duration,
    ) -> Result<TaskReadSnapshot, TaskError> {
        let deadline = tokio::time::Instant::now() + wait;
        let mut changes = None;
        loop {
            let task = self.get(owner, task_id).await?;
            let session = self.task_session(owner, task_id, task.attempt_no);
            if changes.is_none() {
                changes = session
                    .as_ref()
                    .map(|session| session.subscribe_live_output());
            }
            if let Some(source) = match session.as_ref() {
                Some(session) => session.live_output().await,
                None => None,
            } {
                let batch = source
                    .read_records(&cursor, max_records, max_bytes)
                    .await
                    .map_err(|error| TaskError::ResourceUnavailable(error.to_string()))?;
                let has_more = source
                    .end_cursor()
                    .await
                    .map(|end| end != batch.next_cursor)
                    .map_err(|error| TaskError::ResourceUnavailable(error.to_string()))?;
                if !batch.records.is_empty() || task.state.is_terminal() || wait.is_zero() {
                    return Ok(TaskReadSnapshot {
                        task,
                        records: batch.records,
                        next_cursor: batch.next_cursor,
                        has_more,
                        timed_out: false,
                    });
                }
            } else if task.state.is_terminal() || wait.is_zero() {
                return Ok(TaskReadSnapshot {
                    task,
                    records: Vec::new(),
                    next_cursor: cursor,
                    has_more: false,
                    timed_out: false,
                });
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(TaskReadSnapshot {
                    task,
                    records: Vec::new(),
                    next_cursor: cursor,
                    has_more: false,
                    timed_out: true,
                });
            }
            match changes.as_mut() {
                Some(changes) => {
                    let _ = tokio::time::timeout_at(deadline, changes.recv()).await;
                }
                None => tokio::time::sleep_until(deadline).await,
            }
        }
    }

    pub async fn unwatch(
        &self,
        owner: &TaskOwner,
        task_id: &str,
    ) -> Result<TaskWatchStatus, TaskError> {
        let current = self.get(owner, task_id).await?;
        let removed = {
            let mut watchers = self
                .watchers
                .lock()
                .expect("Task Watcher registry lock poisoned");
            match watchers.get(task_id) {
                Some(watcher) if watcher.owner != *owner => {
                    return Err(TaskError::NotFound("task".to_string()));
                }
                Some(_) => watchers.remove(task_id),
                None => None,
            }
        };
        if let Some(watcher) = removed {
            watcher.root_cancel.cancel();
        }
        Ok(TaskWatchStatus {
            task_id: task_id.to_string(),
            attempt_no: current.attempt_no,
            active: false,
        })
    }

    pub async fn read_full_result(
        &self,
        owner: &TaskOwner,
        task_id: &str,
    ) -> Result<Vec<u8>, TaskError> {
        let record = self.get(owner, task_id).await?;
        let attempt_no = record.attempt_no;
        let result = record
            .result
            .ok_or_else(|| TaskError::Invalid("task has no terminal result".to_string()))?;
        let reference = result
            .full_result_ref
            .ok_or_else(|| TaskError::NotFound("full result".to_string()))?;
        let expected = format!("task://{task_id}/attempts/{}/raw-output.json", attempt_no);
        if reference.uri != expected {
            return Err(TaskError::Invalid(
                "unsupported task result reference".to_string(),
            ));
        }
        let local_device_id = self.authority.local_device_id().await?;
        if reference.source_device_id.is_some()
            && reference.source_device_id.as_deref() != local_device_id.as_deref()
        {
            return Err(TaskError::ResourceUnavailable(format!(
                "full Task result is stored on Device '{}'",
                reference.source_device_id.as_deref().unwrap_or("unknown")
            )));
        }
        tokio::fs::read(
            self.config
                .attempt_dir(owner, task_id, attempt_no)
                .join("raw-output.json"),
        )
        .await
        .map_err(TaskError::io)
    }

    pub async fn resolve_output_file(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        attempt_no: u32,
        path: &Path,
    ) -> Result<PathBuf, TaskError> {
        let record = self.get(owner, task_id).await?;
        if record.attempt_no != attempt_no {
            return Err(TaskError::Invalid(
                "Output belongs to another Attempt".into(),
            ));
        }
        let root = tokio::fs::canonicalize(
            self.config
                .attempt_dir(owner, task_id, attempt_no)
                .join("output"),
        )
        .await
        .map_err(TaskError::io)?;
        let resolved = tokio::fs::canonicalize(path).await.map_err(TaskError::io)?;
        if !resolved.starts_with(&root) {
            return Err(TaskError::Invalid(
                "Output file escapes Task Attempt storage".into(),
            ));
        }
        Ok(resolved)
    }

    pub async fn read_artifact(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        attempt_no: u32,
        artifact_id: &str,
        offset: u64,
        max_bytes: usize,
    ) -> Result<(Vec<u8>, u64), TaskError> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        if !(1..=16 * 1024).contains(&max_bytes) {
            return Err(TaskError::Invalid(
                "artifact read limit must be between 1 and 16384".into(),
            ));
        }
        let record = self.get(owner, task_id).await?;
        if record.attempt_no != attempt_no {
            return Err(TaskError::Invalid(
                "Artifact belongs to another Attempt".into(),
            ));
        }
        let result = record
            .result
            .ok_or_else(|| TaskError::Invalid("Task has no terminal result".into()))?;
        let artifact = result
            .artifacts
            .iter()
            .find(|artifact| artifact.id == artifact_id)
            .ok_or_else(|| TaskError::NotFound("artifact".into()))?;
        let local_device = self.authority.local_device_id().await?;
        if artifact.resource.source_device_id.is_some()
            && artifact.resource.source_device_id != local_device
        {
            return Err(TaskError::ResourceUnavailable(
                "Artifact is stored on another Device".into(),
            ));
        }
        let prefix = format!("task://{task_id}/attempts/{attempt_no}/artifacts/");
        let name = artifact
            .resource
            .uri
            .strip_prefix(&prefix)
            .filter(|name| {
                !name.is_empty() && *name != "." && *name != ".." && !name.contains(['/', '\\'])
            })
            .filter(|_| artifact.resource.ref_type == "task_artifact")
            .ok_or_else(|| TaskError::Invalid("Unsupported artifact reference".into()))?;
        let root = tokio::fs::canonicalize(self.config.attempt_dir(owner, task_id, attempt_no))
            .await
            .map_err(TaskError::io)?;
        let path = tokio::fs::canonicalize(root.join("artifacts").join(name))
            .await
            .map_err(TaskError::io)?;
        if !path.starts_with(&root) {
            return Err(TaskError::Invalid("Artifact escapes Task storage".into()));
        }
        let mut file = tokio::fs::File::open(path).await.map_err(TaskError::io)?;
        let size = file.metadata().await.map_err(TaskError::io)?.len();
        if offset > size {
            return Err(TaskError::Invalid("Artifact offset exceeds length".into()));
        }
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(TaskError::io)?;
        let mut bytes = vec![0; (size - offset).min(max_bytes as u64) as usize];
        file.read_exact(&mut bytes).await.map_err(TaskError::io)?;
        if self.get(owner, task_id).await?.attempt_no != attempt_no {
            return Err(TaskError::Invalid(
                "Task Attempt changed during artifact read".into(),
            ));
        }
        Ok((bytes, size))
    }

    pub async fn retry(
        self: &Arc<Self>,
        owner: &TaskOwner,
        task_id: &str,
        authorization: crate::model::TaskAttemptAuthorization,
    ) -> Result<TaskRecord, TaskError> {
        let admission = self.admission_open.lock().await;
        if self
            .paused_agents
            .lock()
            .expect("Task admission state poisoned")
            .contains(&authorization.agent_id)
        {
            return Err(TaskError::ResourceUnavailable(
                "Agent is shutting down and cannot retry Tasks".to_string(),
            ));
        }
        if !*admission {
            return Err(TaskError::ResourceUnavailable(
                "Task Runtime is shutting down".to_string(),
            ));
        }
        let current = self.get(owner, task_id).await?;
        let device = self.authority.accept_attempt(owner, &authorization).await?;
        let runner_kind = current.spec.runner.kind();
        let runner = self
            .runners
            .read()
            .expect("task runner registry lock poisoned")
            .get(&runner_kind)
            .cloned()
            .ok_or(TaskError::RunnerUnavailable(runner_kind))?;
        let record = self
            .store
            .retry(
                owner.clone(),
                task_id.to_string(),
                authorization,
                device,
                self.config.limits.max_active_tasks_per_chat,
                self.config.limits.max_attempt_history,
            )
            .await?;
        self.publish_event(
            owner,
            task_id,
            record.attempt_no,
            TaskEventKind::Accepted,
            TaskState::Accepted,
        );
        self.dispatch(record.clone(), runner)?;
        drop(admission);
        self.refresh_dependents_and_dispatch(task_id).await?;
        Ok(record)
    }

    fn dispatch(
        self: &Arc<Self>,
        record: TaskRecord,
        runner: Arc<dyn TaskRunner>,
    ) -> Result<(), TaskError> {
        let attempt = record
            .attempts
            .iter()
            .find(|attempt| attempt.attempt_no == record.attempt_no)
            .ok_or_else(|| TaskError::Store("current Task Attempt is missing".to_string()))?;
        if attempt.authorization_agent_id.trim().is_empty() {
            return Err(TaskError::Store(
                "current Task Attempt has no authorization Agent".to_string(),
            ));
        }
        let device = attempt.device.clone();
        let authorization_agent_id = attempt.authorization_agent_id.clone();
        let configuration_epoch = attempt.configuration_epoch.clone();
        let session = TaskRunnerSession::bound(record.task_id.clone(), record.attempt_no);
        let execution =
            TaskExecutionHandle::new(record.spec.owner.clone(), record.attempt_no, session);
        self.insert_execution(record.task_id.clone(), execution.clone());
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            runtime
                .run_task(
                    DispatchedTask {
                        task_id: record.task_id,
                        attempt_no: record.attempt_no,
                        authorization_agent_id,
                        configuration_epoch,
                        spec: record.spec,
                        device,
                    },
                    runner,
                    execution,
                )
                .await;
        });
        Ok(())
    }

    pub fn subscribe_activity(&self) -> broadcast::Receiver<TaskEvent> {
        self.activity_tx.subscribe()
    }

    pub async fn activity_snapshot(&self, owner: &TaskOwner) -> Result<Vec<TaskRecord>, TaskError> {
        let active = self.store.list_non_terminal(owner.clone()).await?;
        let recent = self
            .store
            .list(
                owner.clone(),
                TaskListFilter {
                    state: None,
                    limit: Some(self.config.limits.activity_channel_capacity),
                    offset: 0,
                },
            )
            .await?;
        let mut records = active
            .into_iter()
            .map(|record| (record.task_id.clone(), record))
            .collect::<HashMap<_, _>>();
        for record in recent {
            records.insert(record.task_id.clone(), record);
        }
        let mut records = records.into_values().collect::<Vec<_>>();
        for record in &mut records {
            record.input_capability =
                match self.task_session(&record.spec.owner, &record.task_id, record.attempt_no) {
                    Some(session) => session.input_capability().await,
                    None => crate::session::TaskInputCapability::None,
                };
        }
        records.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.task_id.cmp(&left.task_id))
        });
        records.truncate(
            self.config
                .limits
                .max_active_tasks_per_chat
                .saturating_add(self.config.limits.activity_channel_capacity),
        );
        Ok(records)
    }

    pub async fn await_terminal(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        timeout: Duration,
    ) -> Result<TaskRecord, TaskError> {
        let mut receiver = self.subscribe_activity();
        let current = self.get(owner, task_id).await?;
        if current.state.is_terminal() {
            return Ok(current);
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match tokio::time::timeout_at(deadline, receiver.recv()).await {
                Ok(Ok(event))
                    if event.owner == *owner
                        && event.task_id == task_id
                        && event.state.is_terminal() =>
                {
                    return self.get(owner, task_id).await;
                }
                Ok(Ok(_)) => {}
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    let current = self.get(owner, task_id).await?;
                    if current.state.is_terminal() {
                        return Ok(current);
                    }
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    return Err(TaskError::Store("Task activity channel closed".to_string()));
                }
                Err(_) => return Err(TaskError::WaitTimedOut(task_id.to_string())),
            }
        }
    }

    pub async fn cancel(
        self: &Arc<Self>,
        owner: &TaskOwner,
        task_id: &str,
    ) -> Result<TaskRecord, TaskError> {
        let current = self.get(owner, task_id).await?;
        if current.state.is_terminal() {
            return Ok(current);
        }
        let execution = self
            .executions
            .lock()
            .expect("Task execution registry lock poisoned")
            .get(task_id)
            .cloned();
        let Some(execution) = execution else {
            let device = current_attempt_device(&current)?;
            self.finish_cancelled(task_id, current.attempt_no, &current.spec, device)
                .await;
            return self.get(owner, task_id).await;
        };
        execution.request_cancel();
        let record = self
            .await_terminal(
                owner,
                task_id,
                self.config.limits.cancellation_grace + Duration::from_secs(2),
            )
            .await?;
        self.await_execution_removed(
            task_id,
            self.config.limits.cancellation_grace + Duration::from_secs(2),
        )
        .await?;
        Ok(record)
    }

    pub async fn interrupt(
        self: &Arc<Self>,
        owner: &TaskOwner,
        task_id: &str,
    ) -> Result<TaskRecord, TaskError> {
        let current = self.get(owner, task_id).await?;
        if current.state.is_terminal() {
            return Ok(current);
        }
        let execution = self
            .executions
            .lock()
            .expect("Task execution registry lock poisoned")
            .get(task_id)
            .cloned();
        let Some(execution) = execution else {
            let device = current_attempt_device(&current)?;
            self.finish_interrupted(
                task_id,
                current.attempt_no,
                &current.spec,
                device,
                true,
                "conversation authority transfer interrupted the Task",
            )
            .await;
            return self.get(owner, task_id).await;
        };
        execution.request_interrupt();
        let record = self
            .await_terminal(
                owner,
                task_id,
                self.config.limits.cancellation_grace + Duration::from_secs(2),
            )
            .await?;
        self.await_execution_removed(
            task_id,
            self.config.limits.cancellation_grace + Duration::from_secs(2),
        )
        .await?;
        Ok(record)
    }

    pub async fn interrupt_owner(self: &Arc<Self>, owner: &TaskOwner) -> Result<usize, TaskError> {
        let records = self.store.list_non_terminal(owner.clone()).await?;
        if records.is_empty() {
            self.await_owner_quiescent(
                owner,
                self.config.limits.cancellation_grace + Duration::from_secs(2),
            )
            .await?;
            return Ok(0);
        }

        let mut without_execution = Vec::new();
        {
            let executions = self
                .executions
                .lock()
                .expect("Task execution registry lock poisoned");
            for record in &records {
                if let Some(execution) = executions.get(&record.task_id) {
                    execution.request_interrupt();
                } else {
                    without_execution.push(record.clone());
                }
            }
        }
        for record in without_execution {
            let device = current_attempt_device(&record)?;
            self.finish_interrupted(
                &record.task_id,
                record.attempt_no,
                &record.spec,
                device,
                true,
                "conversation authority transfer interrupted the Task",
            )
            .await;
        }
        self.await_owner_quiescent(
            owner,
            self.config.limits.cancellation_grace + Duration::from_secs(2),
        )
        .await?;
        Ok(records.len())
    }

    /// Every non-terminal Task Attempt this process holds, across all owners.
    ///
    /// An Agent shutdown must not rely on enumerating Chats: an Attempt can
    /// outlive the Chat record that created it, and a Project-Scoped Chat is
    /// owned by the Project rather than the Agent. Each record therefore reports
    /// its real `spec.owner`, and callers attribute work from that owner instead
    /// of guessing it from a Chat index.
    pub async fn outstanding_attempts(&self) -> Result<Vec<TaskRecord>, TaskError> {
        let mut records = self.store.list_all_non_terminal().await?;
        let executing: Vec<_> = self
            .executions
            .lock()
            .expect("Task execution registry lock poisoned")
            .iter()
            .map(|(id, handle)| (id.clone(), handle.owner.clone()))
            .collect();
        for (id, owner) in executing {
            if !records.iter().any(|record| record.task_id == id) {
                records.push(self.store.get(owner, id).await?);
            }
        }
        let mut resolved = Vec::with_capacity(records.len());
        for record in records {
            resolved.push(self.with_input_capability(record).await);
        }
        Ok(resolved)
    }

    pub async fn shutdown(self: &Arc<Self>) -> Result<usize, TaskError> {
        {
            let mut admission = self.admission_open.lock().await;
            *admission = false;
        }
        self.cancel_all_watchers();
        let records = self.store.list_all_non_terminal().await?;
        let owners = records
            .iter()
            .map(|record| record.spec.owner.clone())
            .collect::<HashSet<_>>();
        let mut without_execution = Vec::new();
        {
            let executions = self
                .executions
                .lock()
                .expect("Task execution registry lock poisoned");
            for record in &records {
                if let Some(execution) = executions.get(&record.task_id) {
                    execution.request_shutdown();
                } else {
                    without_execution.push(record.clone());
                }
            }
        }
        for record in without_execution {
            let device = current_attempt_device(&record)?;
            self.finish_interrupted(
                &record.task_id,
                record.attempt_no,
                &record.spec,
                device,
                true,
                RUNTIME_SHUTDOWN_MESSAGE,
            )
            .await;
        }
        for owner in owners {
            self.await_owner_quiescent(
                &owner,
                self.config.limits.cancellation_grace + Duration::from_secs(2),
            )
            .await?;
        }
        Ok(records.len())
    }

    pub async fn cancel_owner(self: &Arc<Self>, owner: &TaskOwner) -> Result<usize, TaskError> {
        self.cancel_owner_at_or_after(owner, None).await
    }

    pub async fn pause_agent_admission(&self, agent_id: &str) {
        let _admission = self.admission_open.lock().await;
        self.paused_agents
            .lock()
            .expect("Task admission state poisoned")
            .insert(agent_id.to_string());
    }

    pub async fn resume_agent_admission(&self, agent_id: &str) {
        let _admission = self.admission_open.lock().await;
        self.paused_agents
            .lock()
            .expect("Task admission state poisoned")
            .remove(agent_id);
    }

    pub async fn cancel_owner_at_or_after(
        self: &Arc<Self>,
        owner: &TaskOwner,
        branch_point: Option<chrono::DateTime<Utc>>,
    ) -> Result<usize, TaskError> {
        let records = self.store.list_non_terminal(owner.clone()).await?;
        let mut cancelled = 0;
        for record in records {
            if !record.state.is_terminal()
                && branch_point.is_none_or(|point| record.created_at >= point)
            {
                self.cancel(owner, &record.task_id).await?;
                cancelled += 1;
            }
        }
        Ok(cancelled)
    }

    pub async fn remove_owner(self: &Arc<Self>, owner: &TaskOwner) -> Result<usize, TaskError> {
        self.remove_watchers_for_owner(owner);
        let _ = self.cancel_owner(owner).await?;
        let removed = self.store.delete_owner(owner.clone()).await?;
        self.sessions
            .lock()
            .expect("Task Runner Session registry lock poisoned")
            .retain(|_, session| session.owner != *owner);
        let task_root = ChatScopeStore::resolve(&self.config.data_dir, owner.chat_address.clone())
            .root()
            .join("sessions")
            .join(owner.chat_id())
            .join("tasks");
        match tokio::fs::remove_dir_all(task_root).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(TaskError::io(error)),
        }
        Ok(removed)
    }

    fn remove_watchers_for_owner(&self, owner: &TaskOwner) {
        let removed = {
            let mut watchers = self
                .watchers
                .lock()
                .expect("Task Watcher registry lock poisoned");
            let task_ids = watchers
                .iter()
                .filter(|(_, watcher)| watcher.owner == *owner)
                .map(|(task_id, _)| task_id.clone())
                .collect::<Vec<_>>();
            task_ids
                .into_iter()
                .filter_map(|task_id| watchers.remove(&task_id))
                .collect::<Vec<_>>()
        };
        for watcher in removed {
            watcher.root_cancel.cancel();
        }
    }

    fn cancel_all_watchers(&self) {
        let watchers = self
            .watchers
            .lock()
            .expect("Task Watcher registry lock poisoned")
            .drain()
            .map(|(_, watcher)| watcher)
            .collect::<Vec<_>>();
        for watcher in watchers {
            watcher.root_cancel.cancel();
        }
    }

    async fn run_task(
        self: Arc<Self>,
        task: DispatchedTask,
        runner: Arc<dyn TaskRunner>,
        execution: TaskExecutionHandle,
    ) {
        let DispatchedTask {
            task_id,
            attempt_no,
            authorization_agent_id,
            configuration_epoch,
            spec,
            device,
        } = task;
        if !self.attempt_is_current(&spec.owner, &device).await {
            self.finish_interrupted(
                &task_id,
                attempt_no,
                &spec,
                device,
                false,
                "Task authority changed before execution started",
            )
            .await;
            return;
        }
        if let Err(error) = self
            .store
            .mark_waiting(task_id.clone(), attempt_no, Utc::now())
            .await
        {
            tracing::error!(task_id, error = %error, "failed to mark task waiting");
            self.remove_execution(&task_id);
            return;
        }
        self.publish_event(
            &spec.owner,
            &task_id,
            attempt_no,
            TaskEventKind::Waiting,
            TaskState::Waiting,
        );

        let chat_permit = self.chat_permit(&spec.owner).await;
        let runner_permit = self.runner_permit(spec.runner.kind()).await;
        let permits = tokio::select! {
            result = async {
                let chat = chat_permit.acquire_owned().await.map_err(|_| ())?;
                let runner = runner_permit.acquire_owned().await.map_err(|_| ())?;
                Ok::<_, ()>((chat, runner))
            } => result.ok(),
            _ = execution.cancel.cancelled() => None,
        };
        let Some((_chat_permit, _runner_permit)) = permits else {
            self.finish_task_output(task_id, attempt_no, spec, device, execution.stop_output())
                .await;
            return;
        };

        if !self.attempt_is_current(&spec.owner, &device).await {
            self.finish_interrupted(
                &task_id,
                attempt_no,
                &spec,
                device,
                false,
                "Task authority changed while execution was waiting",
            )
            .await;
            return;
        }
        let started_at = Utc::now();
        if let Err(error) = self
            .store
            .mark_running(task_id.clone(), attempt_no, started_at)
            .await
        {
            tracing::error!(task_id, error = %error, "failed to mark task running");
            self.remove_execution(&task_id);
            return;
        }
        self.publish_event(
            &spec.owner,
            &task_id,
            attempt_no,
            TaskEventKind::Running,
            TaskState::Running,
        );

        let task_dir = self.config.attempt_dir(&spec.owner, &task_id, attempt_no);
        let dependencies = match self.store.dependency_results(task_id.clone()).await {
            Ok(results) => results,
            Err(error) => {
                tracing::error!(task_id, error = %error, "failed to load Task dependencies");
                self.remove_execution(&task_id);
                return;
            }
        };
        let request = TaskRunRequest {
            task_id: task_id.clone(),
            attempt_no,
            authorization_agent_id,
            configuration_epoch,
            spec: spec.clone(),
            task_dir: task_dir.clone(),
            device: device.clone(),
            dependencies,
            cancel: execution.cancel.clone(),
            session: Arc::clone(&execution.session),
        };

        let mut runner_future = Box::pin(runner.run(request));
        let output = tokio::select! {
            biased;
            result = &mut runner_future => match result {
                Ok(output) => output,
                Err(error) => TaskRunOutput {
                    outcome: TaskRunOutcome::Failed,
                    output: runner_failure_output(&error.message),
                    raw_output: None,
                    artifacts: Vec::new(),
                    artifact_payloads: Vec::new(),
                    error: Some(TaskErrorInfo {
                        kind: crate::model::TaskErrorKind::Runner,
                        message: error.message,
                        retryable: error.retryable,
                        details: serde_json::Value::Null,
                    }),
                },
            },
            _ = execution.cancel.cancelled() => {
                match tokio::time::timeout(
                    self.config.limits.cancellation_grace,
                    &mut runner_future,
                ).await {
                    Ok(Ok(output)) => execution.stop_output_preserving(output),
                    Ok(Err(error)) => execution.stop_output_preserving(TaskRunOutput {
                        outcome: TaskRunOutcome::Failed,
                        output: runner_failure_output(&error.message),
                        raw_output: None,
                        artifacts: Vec::new(),
                        artifact_payloads: Vec::new(),
                        error: Some(TaskErrorInfo {
                            kind: crate::model::TaskErrorKind::Runner,
                            message: error.message,
                            retryable: error.retryable,
                            details: serde_json::Value::Null,
                        }),
                    }),
                    Err(_) => execution.stop_output(),
                }
            }
        };

        self.finish_task_output(task_id, attempt_no, spec, device, output)
            .await;
    }

    async fn run_adopted_task(
        self: Arc<Self>,
        task_id: TaskId,
        attempt_no: u32,
        spec: TaskSpec,
        execution: TaskAdoptedExecution,
        task_execution: TaskExecutionHandle,
    ) {
        let TaskAdoptedExecution {
            mut future,
            cancel: execution_cancel,
            session: _,
        } = execution;
        let output = tokio::select! {
            biased;
            output = &mut future => output,
            _ = task_execution.cancel.cancelled() => {
                execution_cancel.cancel();
                match tokio::time::timeout(
                    self.config.limits.cancellation_grace,
                    &mut future,
                ).await {
                    Ok(output) => task_execution.stop_output_preserving(output),
                    Err(_) => task_execution.stop_output(),
                }
            }
        };
        let device = match self
            .get(&spec.owner, &task_id)
            .await
            .and_then(|record| current_attempt_device(&record))
        {
            Ok(device) => device,
            Err(error) => {
                tracing::error!(task_id, error = %error, "failed to load adopted Task provenance");
                self.remove_execution(&task_id);
                return;
            }
        };
        self.finish_task_output(task_id, attempt_no, spec, device, output)
            .await;
    }

    async fn finish_task_output(
        self: &Arc<Self>,
        task_id: TaskId,
        attempt_no: u32,
        spec: TaskSpec,
        device: TaskDeviceContext,
        output: TaskRunOutput,
    ) {
        self.close_task_session(&spec.owner, &task_id, attempt_no)
            .await;
        if !self.attempt_is_current(&spec.owner, &device).await {
            self.finish_interrupted(
                &task_id,
                attempt_no,
                &spec,
                device,
                false,
                "Task output belongs to a stale conversation authority Epoch",
            )
            .await;
            return;
        }
        let TaskRunOutput {
            outcome,
            output,
            mut raw_output,
            mut artifacts,
            artifact_payloads,
            error,
        } = output;
        if matches!(outcome, TaskRunOutcome::Cancelled) && raw_output.is_none() {
            raw_output = serde_json::to_vec(&serde_json::json!({
                "status": "cancelled",
                "output": &output,
            }))
            .ok();
        }
        artifacts = persist_artifact_payloads(
            &self.config.attempt_dir(&spec.owner, &task_id, attempt_no),
            &task_id,
            attempt_no,
            artifacts,
            artifact_payloads,
            device.device_id.as_deref(),
        )
        .await;
        for artifact in &mut artifacts {
            artifact.resource.source_device_id = device.device_id.clone();
        }
        let (bounded_output, output_truncated) =
            bound_result_output(&output, self.config.limits.max_result_output_chars);
        if output_truncated && raw_output.is_none() {
            raw_output = serde_json::to_vec(&serde_json::json!({
                "output": output,
                "truncated": true,
            }))
            .ok();
        }
        let task_dir = self.config.attempt_dir(&spec.owner, &task_id, attempt_no);
        let full_result_ref = match raw_output {
            Some(bytes) => match write_raw_output(
                task_dir.join("raw-output.json"),
                bound_raw_output(bytes, self.config.limits.max_raw_result_bytes),
            )
            .await
            {
                Ok(()) => Some(TaskResourceRef {
                    uri: format!("task://{task_id}/attempts/{attempt_no}/raw-output.json"),
                    ref_type: "task_raw_output".to_string(),
                    source_device_id: device.device_id.clone(),
                }),
                Err(error) => {
                    tracing::error!(task_id, error = %error, "failed to persist task raw output");
                    None
                }
            },
            None => None,
        };

        let state = match outcome {
            TaskRunOutcome::Completed => TaskState::Completed,
            TaskRunOutcome::Failed => TaskState::Failed,
            TaskRunOutcome::Cancelled => TaskState::Cancelled,
            TaskRunOutcome::Interrupted => TaskState::Interrupted,
        };
        let result = TaskResult {
            status: state,
            output: bounded_output,
            full_result_ref,
            artifacts,
            device,
            error,
        };
        let finished_at = Utc::now();
        let terminal_record = match self
            .store
            .finish(task_id.clone(), attempt_no, state, result, finished_at)
            .await
        {
            Ok(record) => record,
            Err(error) => {
                tracing::error!(task_id, error = %error, "failed to persist terminal task result");
                self.remove_execution(&task_id);
                return;
            }
        };
        let event_kind = match state {
            TaskState::Completed => TaskEventKind::Completed,
            TaskState::Failed => TaskEventKind::Failed,
            TaskState::Cancelled => TaskEventKind::Cancelled,
            TaskState::Interrupted => TaskEventKind::Interrupted,
            _ => unreachable!("terminal Task state"),
        };
        self.publish_event(&spec.owner, &task_id, attempt_no, event_kind, state);
        if let Err(error) = self.refresh_dependents_and_dispatch(&task_id).await {
            tracing::warn!(task_id, error = %error, "failed to refresh dependent Tasks");
        }

        if let Err(error) = self
            .notification_sink
            .terminal(TaskCompletionNotification {
                record: terminal_record,
            })
            .await
        {
            tracing::warn!(task_id, error = %error, "failed to publish task completion");
        }
        self.remove_execution(&task_id);
    }

    async fn finish_cancelled(
        self: &Arc<Self>,
        task_id: &str,
        attempt_no: u32,
        spec: &TaskSpec,
        device: TaskDeviceContext,
    ) {
        self.close_task_session(&spec.owner, task_id, attempt_no)
            .await;
        if !self.attempt_is_current(&spec.owner, &device).await {
            self.finish_interrupted(
                task_id,
                attempt_no,
                spec,
                device,
                false,
                "Task cancellation completed under a stale conversation authority Epoch",
            )
            .await;
            return;
        }
        let result = TaskResult {
            status: TaskState::Cancelled,
            output: TaskRunOutput::cancelled().output,
            full_result_ref: None,
            artifacts: Vec::new(),
            device,
            error: Some(TaskErrorInfo {
                kind: crate::model::TaskErrorKind::Runner,
                message: "task cancelled".to_string(),
                retryable: false,
                details: serde_json::Value::Null,
            }),
        };
        let terminal_record = match self
            .store
            .finish(
                task_id.to_string(),
                attempt_no,
                TaskState::Cancelled,
                result,
                Utc::now(),
            )
            .await
        {
            Ok(record) => record,
            Err(error) => {
                tracing::warn!(task_id, error = %error, "failed to persist waiting Task cancellation");
                self.remove_execution(task_id);
                return;
            }
        };
        self.publish_event(
            &spec.owner,
            task_id,
            attempt_no,
            TaskEventKind::Cancelled,
            TaskState::Cancelled,
        );
        if let Err(error) = self.refresh_dependents_and_dispatch(task_id).await {
            tracing::warn!(task_id, error = %error, "failed to refresh dependent Tasks");
        }
        if let Err(error) = self
            .notification_sink
            .terminal(TaskCompletionNotification {
                record: terminal_record,
            })
            .await
        {
            tracing::warn!(task_id, error = %error, "failed to publish Task cancellation");
        }
        self.remove_execution(task_id);
    }

    async fn finish_interrupted(
        self: &Arc<Self>,
        task_id: &str,
        attempt_no: u32,
        spec: &TaskSpec,
        device: TaskDeviceContext,
        notify: bool,
        message: &str,
    ) {
        self.close_task_session(&spec.owner, task_id, attempt_no)
            .await;
        let notify = notify && self.attempt_is_current(&spec.owner, &device).await;
        let output = if message == RUNTIME_SHUTDOWN_MESSAGE {
            runtime_shutdown_output()
        } else {
            TaskRunOutput::interrupted(message)
        };
        let result = TaskResult {
            status: TaskState::Interrupted,
            output: output.output,
            full_result_ref: None,
            artifacts: Vec::new(),
            device,
            error: output.error,
        };
        let terminal_record = match self
            .store
            .finish(
                task_id.to_string(),
                attempt_no,
                TaskState::Interrupted,
                result,
                Utc::now(),
            )
            .await
        {
            Ok(record) => record,
            Err(error) => {
                tracing::warn!(task_id, error = %error, "failed to persist Task interruption");
                self.remove_execution(task_id);
                return;
            }
        };
        self.publish_event(
            &spec.owner,
            task_id,
            attempt_no,
            TaskEventKind::Interrupted,
            TaskState::Interrupted,
        );
        if let Err(error) = self.refresh_dependents_and_dispatch(task_id).await {
            tracing::warn!(task_id, error = %error, "failed to refresh dependent Tasks");
        }
        if notify {
            if let Err(error) = self
                .notification_sink
                .terminal(TaskCompletionNotification {
                    record: terminal_record,
                })
                .await
            {
                tracing::warn!(task_id, error = %error, "failed to publish Task interruption");
            }
        }
        self.remove_execution(task_id);
    }

    async fn attempt_is_current(&self, owner: &TaskOwner, device: &TaskDeviceContext) -> bool {
        match self.authority.is_current(owner, device).await {
            Ok(current) => current,
            Err(error) => {
                tracing::warn!(
                    chat_address = %owner.chat_address,
                    error = %error,
                    "failed to validate Task authority"
                );
                false
            }
        }
    }

    fn insert_execution(self: &Arc<Self>, task_id: TaskId, execution: TaskExecutionHandle) {
        execution.session.set_output_dir(
            self.config
                .attempt_dir(&execution.owner, &task_id, execution.attempt_no)
                .join("output"),
        );
        let watcher_task_id = task_id.clone();
        let watcher_execution = execution.clone();
        self.sessions
            .lock()
            .expect("Task Runner Session registry lock poisoned")
            .insert(
                task_id.clone(),
                TaskSessionEntry {
                    owner: execution.owner.clone(),
                    attempt_no: execution.attempt_no,
                    session: Arc::clone(&execution.session),
                },
            );
        self.executions
            .lock()
            .expect("Task execution registry lock poisoned")
            .insert(task_id, execution);
        self.execution_change.notify_waiters();
        self.reattach_watcher_for_execution(watcher_task_id, watcher_execution);
    }

    fn reattach_watcher_for_execution(
        self: &Arc<Self>,
        task_id: TaskId,
        execution: TaskExecutionHandle,
    ) {
        let (attempt_cancel, origin_turn_id, projection) = {
            let mut watchers = self
                .watchers
                .lock()
                .expect("Task Watcher registry lock poisoned");
            let Some(watcher) = watchers.get_mut(&task_id) else {
                return;
            };
            if watcher.owner != execution.owner || watcher.attempt_no == execution.attempt_no {
                return;
            }
            watcher.attempt_cancel.cancel();
            let attempt_cancel = watcher.root_cancel.child_token();
            watcher.attempt_no = execution.attempt_no;
            watcher.attempt_cancel = attempt_cancel.clone();
            (
                attempt_cancel,
                watcher.origin_turn_id.clone(),
                watcher.projection,
            )
        };
        self.spawn_watcher_attempt(
            TaskWatcherAttempt {
                task_id,
                owner: execution.owner,
                attempt_no: execution.attempt_no,
                session: execution.session,
                origin_turn_id,
                projection,
            },
            attempt_cancel,
            Some(TaskLiveOutputCursor::default()),
        );
    }

    fn spawn_watcher_attempt(
        self: &Arc<Self>,
        attempt: TaskWatcherAttempt,
        cancel: CancellationToken,
        initial_cursor: Option<TaskLiveOutputCursor>,
    ) {
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            runtime
                .run_watcher_attempt(attempt, cancel, initial_cursor)
                .await;
        });
    }

    async fn run_watcher_attempt(
        self: Arc<Self>,
        attempt: TaskWatcherAttempt,
        cancel: CancellationToken,
        initial_cursor: Option<TaskLiveOutputCursor>,
    ) {
        let TaskWatcherAttempt {
            task_id,
            owner,
            attempt_no,
            session,
            origin_turn_id,
            projection,
        } = attempt;
        let mut changes = session.subscribe_live_output();
        let source = loop {
            if let Some(source) = session.live_output().await {
                break source;
            }
            if session.is_closed().await {
                self.finish_watcher_attempt(&task_id, &owner, attempt_no, &cancel)
                    .await;
                return;
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = changes.recv() => {}
            }
        };
        let mut delay_before_read = initial_cursor
            .as_ref()
            .is_some_and(|cursor| !cursor.positions.is_empty() || !cursor.pending.is_empty());
        let mut cursor = initial_cursor
            .or(session.live_output_activation_cursor().await)
            .unwrap_or_default();
        let mut notification_seq = 1_u64;

        loop {
            if cancel.is_cancelled() {
                return;
            }
            if delay_before_read && !wait_for_watch_output_window(&mut changes, &cancel).await {
                return;
            }
            delay_before_read = false;
            let batch = match read_watch_snapshot(source.as_ref(), &cursor).await {
                Ok(batch) => batch,
                Err(error) => {
                    tracing::warn!(task_id, error = %error, "failed to read Task Watcher output");
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                    continue;
                }
            };
            if !batch.records.is_empty() {
                let notification = TaskWatchOutputNotification {
                    notification_id: format!(
                        "task-watch:{task_id}:attempt:{attempt_no}:batch:{notification_seq}"
                    ),
                    owner: owner.clone(),
                    task_id: task_id.clone(),
                    attempt_no,
                    origin_turn_id: origin_turn_id.clone(),
                    projection,
                    records: batch.records,
                };
                let delivery = self.notification_sink.watch_output(notification);
                tokio::pin!(delivery);
                let result = tokio::select! {
                    _ = cancel.cancelled() => return,
                    result = &mut delivery => result,
                };
                if let Err(error) = result {
                    tracing::warn!(task_id, error = %error, "failed to deliver Task Watcher output");
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                    continue;
                }
                notification_seq += 1;
            }
            cursor = batch.next_cursor;
            if batch.sealed {
                break;
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = changes.recv() => {
                    delay_before_read = true;
                }
            }
        }

        self.finish_watcher_attempt(&task_id, &owner, attempt_no, &cancel)
            .await;
    }

    async fn finish_watcher_attempt(
        &self,
        task_id: &str,
        owner: &TaskOwner,
        attempt_no: u32,
        cancel: &CancellationToken,
    ) {
        let mut activity = self.subscribe_activity();
        let terminal = loop {
            match self.store.get(owner.clone(), task_id.to_string()).await {
                Ok(record) if record.attempt_no != attempt_no => return,
                Ok(record) if record.state.is_terminal() => break record.state,
                Ok(_) => {}
                Err(_) => return,
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                event = activity.recv() => {
                    match event {
                        Ok(event) if event.owner == *owner
                            && event.task_id == task_id
                            && event.attempt_no == attempt_no
                            && event.state.is_terminal() => break event.state,
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        };
        if terminal == TaskState::Completed {
            let removed = {
                let mut watchers = self
                    .watchers
                    .lock()
                    .expect("Task Watcher registry lock poisoned");
                if watchers
                    .get(task_id)
                    .is_some_and(|watcher| watcher.attempt_no == attempt_no)
                {
                    watchers.remove(task_id)
                } else {
                    None
                }
            };
            if let Some(watcher) = removed {
                watcher.root_cancel.cancel();
            }
        }
    }

    fn task_session(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        attempt_no: u32,
    ) -> Option<Arc<TaskRunnerSession>> {
        self.sessions
            .lock()
            .expect("Task Runner Session registry lock poisoned")
            .get(task_id)
            .filter(|entry| entry.owner == *owner && entry.attempt_no == attempt_no)
            .map(|entry| Arc::clone(&entry.session))
    }

    async fn with_input_capability(&self, mut record: TaskRecord) -> TaskRecord {
        record.input_capability =
            match self.task_session(&record.spec.owner, &record.task_id, record.attempt_no) {
                Some(session) => session.input_capability().await,
                None => crate::session::TaskInputCapability::None,
            };
        record.watch_active = self
            .watchers
            .lock()
            .expect("Task Watcher registry lock poisoned")
            .get(&record.task_id)
            .is_some_and(|watcher| watcher.owner == record.spec.owner);
        record
    }

    async fn close_task_session(&self, owner: &TaskOwner, task_id: &str, attempt_no: u32) {
        if let Some(session) = self.task_session(owner, task_id, attempt_no) {
            session.close().await;
        }
    }

    pub fn attempt_execution_active(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        attempt_no: u32,
    ) -> bool {
        self.executions
            .lock()
            .expect("Task execution registry lock poisoned")
            .get(task_id)
            .is_some_and(|execution| {
                execution.owner == *owner && execution.attempt_no == attempt_no
            })
    }

    fn remove_execution(&self, task_id: &str) {
        self.executions
            .lock()
            .expect("Task execution registry lock poisoned")
            .remove(task_id);
        self.execution_change.notify_waiters();
    }

    async fn await_owner_quiescent(
        &self,
        owner: &TaskOwner,
        timeout: Duration,
    ) -> Result<(), TaskError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let changed = self.execution_change.notified();
            let active = self
                .executions
                .lock()
                .expect("Task execution registry lock poisoned")
                .values()
                .any(|execution| execution.owner == *owner);
            if !active {
                return Ok(());
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return Err(TaskError::WaitTimedOut(format!(
                    "Tasks owned by Chat Address '{}'",
                    owner.chat_address
                )));
            }
        }
    }

    async fn await_execution_removed(
        &self,
        task_id: &str,
        timeout: Duration,
    ) -> Result<(), TaskError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let changed = self.execution_change.notified();
            if !self
                .executions
                .lock()
                .expect("Task execution registry lock poisoned")
                .contains_key(task_id)
            {
                return Ok(());
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return Err(TaskError::WaitTimedOut(task_id.to_string()));
            }
        }
    }

    async fn chat_permit(&self, owner: &TaskOwner) -> Arc<Semaphore> {
        let mut permits = self.chat_permits.lock().await;
        permits
            .entry(owner.clone())
            .or_insert_with(|| {
                Arc::new(Semaphore::new(
                    self.config.limits.max_concurrent_tasks_per_chat,
                ))
            })
            .clone()
    }

    async fn runner_permit(&self, kind: TaskRunnerKind) -> Arc<Semaphore> {
        let mut permits = self.runner_permits.lock().await;
        permits
            .entry(kind)
            .or_insert_with(|| {
                Arc::new(Semaphore::new(
                    self.config.limits.max_concurrent_tasks_per_runner,
                ))
            })
            .clone()
    }

    async fn refresh_dependents_and_dispatch(
        self: &Arc<Self>,
        upstream_task_id: &str,
    ) -> Result<(), TaskError> {
        if !*self.admission_open.lock().await {
            return Ok(());
        }
        let records = self
            .store
            .refresh_dependents(
                upstream_task_id.to_string(),
                self.config.limits.max_dependency_depth,
            )
            .await?;
        for record in records {
            self.publish_event(
                &record.spec.owner,
                &record.task_id,
                record.attempt_no,
                event_kind_for_state(record.state),
                record.state,
            );
            if record.state == TaskState::Accepted {
                let runner_kind = record.spec.runner.kind();
                let runner = self
                    .runners
                    .read()
                    .expect("task runner registry lock poisoned")
                    .get(&runner_kind)
                    .cloned()
                    .ok_or(TaskError::RunnerUnavailable(runner_kind))?;
                self.dispatch(record, runner)?;
            }
        }
        Ok(())
    }

    fn publish_event(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        attempt_no: u32,
        kind: TaskEventKind,
        state: TaskState,
    ) {
        let _ = self.activity_tx.send(TaskEvent {
            sequence: self.next_event_sequence.fetch_add(1, Ordering::Relaxed),
            owner: owner.clone(),
            task_id: task_id.to_string(),
            attempt_no,
            kind,
            state,
            created_at: Utc::now(),
        });
    }
}

fn event_kind_for_state(state: TaskState) -> TaskEventKind {
    match state {
        TaskState::Accepted => TaskEventKind::Accepted,
        TaskState::Waiting => TaskEventKind::Waiting,
        TaskState::Blocked => TaskEventKind::Blocked,
        TaskState::Running => TaskEventKind::Running,
        TaskState::Completed => TaskEventKind::Completed,
        TaskState::Failed => TaskEventKind::Failed,
        TaskState::Cancelled => TaskEventKind::Cancelled,
        TaskState::Interrupted => TaskEventKind::Interrupted,
        TaskState::WaitingApproval => TaskEventKind::Waiting,
    }
}

fn current_attempt_device(record: &TaskRecord) -> Result<TaskDeviceContext, TaskError> {
    record
        .attempts
        .iter()
        .find(|attempt| attempt.attempt_no == record.attempt_no)
        .map(|attempt| attempt.device.clone())
        .ok_or_else(|| TaskError::Store("current Task Attempt is missing".to_string()))
}

async fn write_raw_output(path: PathBuf, bytes: Vec<u8>) -> Result<(), TaskError> {
    tokio::task::spawn_blocking(move || atomic_file::atomic_write(&path, &bytes))
        .await
        .map_err(|error| TaskError::Io(format!("raw output worker failed: {error}")))?
        .map_err(TaskError::io)
}

async fn persist_artifact_payloads(
    task_dir: &Path,
    task_id: &str,
    attempt_no: u32,
    artifacts: Vec<TaskArtifact>,
    payloads: Vec<crate::runner::TaskArtifactPayload>,
    source_device_id: Option<&str>,
) -> Vec<TaskArtifact> {
    let mut payloads = payloads
        .into_iter()
        .map(|payload| (payload.artifact_id, payload.bytes))
        .collect::<HashMap<_, _>>();
    let mut persisted = Vec::with_capacity(artifacts.len());

    for (index, mut artifact) in artifacts.into_iter().enumerate() {
        let Some(bytes) = payloads.remove(&artifact.id) else {
            persisted.push(artifact);
            continue;
        };
        let file_name = artifact_file_name(index, &artifact.name);
        let path = task_dir.join("artifacts").join(&file_name);
        if let Err(error) = write_raw_output(path, bytes.clone()).await {
            tracing::error!(
                task_id,
                artifact_id = %artifact.id,
                error = %error,
                "failed to persist Task artifact"
            );
            continue;
        }
        artifact.resource = TaskResourceRef {
            uri: format!("task://{task_id}/attempts/{attempt_no}/artifacts/{file_name}"),
            ref_type: "task_artifact".to_string(),
            source_device_id: source_device_id.map(str::to_string),
        };
        let metadata = artifact
            .metadata
            .as_object_mut()
            .map(|metadata| {
                metadata.insert("size_bytes".to_string(), serde_json::json!(bytes.len()));
                metadata.insert("content_externalized".to_string(), serde_json::json!(true));
            })
            .is_some();
        if !metadata {
            artifact.metadata = serde_json::json!({
                "size_bytes": bytes.len(),
                "content_externalized": true,
            });
        }
        persisted.push(artifact);
    }

    persisted
}

fn artifact_file_name(index: usize, name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('.').trim_matches('_');
    let sanitized = if sanitized.is_empty() {
        "artifact"
    } else {
        sanitized
    };
    format!("{index:04}-{sanitized}")
}

fn runtime_shutdown_output() -> TaskRunOutput {
    let mut output = TaskRunOutput::interrupted(RUNTIME_SHUTDOWN_MESSAGE);
    if let Some(error) = output.error.as_mut() {
        error.details = serde_json::json!({"provenance": "runtime_shutdown"});
    }
    output
}

fn runner_failure_output(message: &str) -> String {
    format!("Task runner failed: {message}")
}

fn bound_raw_output(bytes: Vec<u8>, max_bytes: usize) -> Vec<u8> {
    if bytes.len() <= max_bytes {
        return bytes;
    }
    let preview = String::from_utf8_lossy(&bytes[..max_bytes]);
    serde_json::to_vec(&serde_json::json!({
        "bounded": true,
        "original_bytes": bytes.len(),
        "preview": preview,
    }))
    .unwrap_or_else(|_| b"{\"bounded\":true}".to_vec())
}

fn bound_result_output(value: &str, max_chars: usize) -> (String, bool) {
    if value.chars().count() <= max_chars {
        return (value.to_string(), false);
    }
    const MARKER: &str = "\n\n[Task Result output truncated; inspect the full result reference.]";
    let marker_chars = MARKER.chars().count();
    if max_chars <= marker_chars {
        return (MARKER.chars().take(max_chars).collect(), true);
    }
    let keep = max_chars - marker_chars;
    let mut bounded: String = value.chars().take(keep).collect();
    bounded.push_str(MARKER);
    (bounded, true)
}
