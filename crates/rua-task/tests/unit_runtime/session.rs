use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rua_task::{
    TaskAttemptAuthorization, TaskInput, TaskInputCapability, TaskInputError, TaskInputSink,
    TaskInputSubmission, TaskLiveOutputBatch, TaskLiveOutputCursor, TaskLiveOutputError,
    TaskLiveOutputProjection, TaskLiveOutputRecord, TaskLiveOutputSource, TaskNotificationSink,
    TaskOwner, TaskRunOutput, TaskRunRequest, TaskRunner, TaskRunnerError, TaskRunnerKind,
    TaskRunnerSession, TaskRunnerSpec, TaskRuntime, TaskRuntimeConfig, TaskSpec, TaskState,
    TaskWatchOutputNotification,
};
use tokio::sync::{Mutex, Notify};

struct RecordingInputSink {
    open: AtomicBool,
    inputs: Mutex<Vec<TaskInput>>,
    close_calls: AtomicUsize,
}

impl RecordingInputSink {
    fn new() -> Self {
        Self {
            open: AtomicBool::new(true),
            inputs: Mutex::new(Vec::new()),
            close_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl TaskInputSink for RecordingInputSink {
    fn declared_capability(&self) -> TaskInputCapability {
        TaskInputCapability::Stdin
    }

    fn active_capability(&self) -> TaskInputCapability {
        if self.open.load(Ordering::SeqCst) {
            TaskInputCapability::Stdin
        } else {
            TaskInputCapability::None
        }
    }

    async fn send_input(&self, input: &TaskInput) -> Result<(), TaskInputError> {
        match input {
            TaskInput::Text { .. } if !self.open.load(Ordering::SeqCst) => {
                return Err(TaskInputError::Unavailable);
            }
            TaskInput::CloseStdin => {
                self.open.store(false, Ordering::SeqCst);
            }
            TaskInput::Text { .. } => {}
        }
        self.inputs.lock().await.push(input.clone());
        Ok(())
    }

    async fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
        self.close_calls.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn runner_session_serializes_identity_idempotency_and_close() {
    let session = TaskRunnerSession::bound("task-a", 3);
    let sink = Arc::new(RecordingInputSink::new());
    session.activate(sink.clone()).await.unwrap();
    assert_eq!(session.input_capability().await, TaskInputCapability::Stdin);

    let text = TaskInputSubmission::new(
        "submission-a",
        TaskInput::Text {
            text: "hello".to_string(),
        },
    )
    .unwrap();
    let first = session.send_input(text.clone()).await.unwrap();
    let duplicate = session.send_input(text).await.unwrap();
    assert_eq!(duplicate, first);
    assert_eq!(first.task_id, "task-a");
    assert_eq!(first.attempt_no, 3);
    assert_eq!(first.input_seq, 1);
    assert_eq!(
        sink.inputs.lock().await.as_slice(),
        &[TaskInput::Text {
            text: "hello".to_string()
        }]
    );

    let conflict = session
        .send_input(
            TaskInputSubmission::new(
                "submission-a",
                TaskInput::Text {
                    text: "different".to_string(),
                },
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        conflict,
        TaskInputError::SubmissionConflict { .. }
    ));

    let close = session
        .send_input(TaskInputSubmission::new("close-a", TaskInput::CloseStdin).unwrap())
        .await
        .unwrap();
    assert_eq!(close.input_seq, 2);
    assert_eq!(session.input_capability().await, TaskInputCapability::None);
    let second_close = session
        .send_input(TaskInputSubmission::new("close-b", TaskInput::CloseStdin).unwrap())
        .await
        .unwrap();
    assert_eq!(second_close.input_seq, 3);

    session.close().await;
    assert!(session
        .send_input(
            TaskInputSubmission::new(
                "after-close",
                TaskInput::Text {
                    text: "rejected".to_string(),
                },
            )
            .unwrap(),
        )
        .await
        .is_err());
    assert_eq!(
        session
            .send_input(
                TaskInputSubmission::new(
                    "submission-a",
                    TaskInput::Text {
                        text: "hello".to_string(),
                    },
                )
                .unwrap(),
            )
            .await
            .unwrap(),
        first
    );
}

struct InteractiveRunner {
    sink: Arc<RecordingInputSink>,
    started: Notify,
    release: Notify,
}

#[async_trait]
impl TaskRunner for InteractiveRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        request.session.activate(self.sink.clone()).await.unwrap();
        self.started.notify_one();
        self.release.notified().await;
        Ok(TaskRunOutput::completed(
            "Outcome\nDone\n\nKey Results\ninput accepted\n\nVerification\npassed\n\nRemaining Work\nNone",
        ))
    }
}

#[tokio::test]
async fn runtime_routes_input_by_owner_and_current_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(InteractiveRunner {
        sink: Arc::new(RecordingInputSink::new()),
        started: Notify::new(),
        release: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("agent-a", "chat-a");
    let task_id = runtime
        .create(
            TaskSpec {
                owner: owner.clone(),
                instruction: "accept input".to_string(),
                runner: TaskRunnerSpec::Tool {
                    tool_name: "Interactive".to_string(),
                    arguments: serde_json::json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            TaskAttemptAuthorization::new("agent-a").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    assert_eq!(
        runtime
            .get(&owner, &task_id)
            .await
            .unwrap()
            .input_capability,
        TaskInputCapability::Stdin
    );
    assert!(runtime
        .send_input(
            &TaskOwner::new("agent-a", "another-chat"),
            &task_id,
            TaskInputSubmission::new(
                "wrong-owner",
                TaskInput::Text {
                    text: "no".to_string(),
                },
            )
            .unwrap(),
        )
        .await
        .is_err());

    let submission = TaskInputSubmission::new(
        "runtime-submission",
        TaskInput::Text {
            text: "exact".to_string(),
        },
    )
    .unwrap();
    let accepted = runtime
        .send_input(&owner, &task_id, submission.clone())
        .await
        .unwrap();
    assert_eq!(accepted.attempt_no, 1);
    assert_eq!(accepted.input_seq, 1);
    assert_eq!(
        runtime
            .send_input(&owner, &task_id, submission)
            .await
            .unwrap(),
        accepted
    );

    runner.release.notify_one();
    let terminal = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(terminal.state, TaskState::Completed);
    assert_eq!(terminal.input_capability, TaskInputCapability::None);
    assert_eq!(
        runtime
            .send_input(
                &owner,
                &task_id,
                TaskInputSubmission::new(
                    "runtime-submission",
                    TaskInput::Text {
                        text: "exact".to_string(),
                    },
                )
                .unwrap(),
            )
            .await
            .unwrap(),
        accepted
    );
}

struct RetryInputRunner {
    sinks: Mutex<Vec<(u32, Arc<RecordingInputSink>)>>,
    second_started: Notify,
    release_second: Notify,
}

#[async_trait]
impl TaskRunner for RetryInputRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        let sink = Arc::new(RecordingInputSink::new());
        request.session.activate(sink.clone()).await.unwrap();
        self.sinks.lock().await.push((request.attempt_no, sink));
        if request.attempt_no == 1 {
            return Ok(TaskRunOutput::failed("first failed", "retry fixture"));
        }
        self.second_started.notify_one();
        self.release_second.notified().await;
        Ok(TaskRunOutput::completed("second completed"))
    }
}

#[tokio::test]
async fn retry_routes_input_only_to_the_current_attempt_session() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(RetryInputRunner {
        sinks: Mutex::new(Vec::new()),
        second_started: Notify::new(),
        release_second: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("agent-a", "chat-a");
    let task_id = runtime
        .create(
            TaskSpec {
                owner: owner.clone(),
                instruction: "retry input".to_string(),
                runner: TaskRunnerSpec::Tool {
                    tool_name: "RetryInput".to_string(),
                    arguments: serde_json::json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            TaskAttemptAuthorization::new("agent-a").unwrap(),
        )
        .await
        .unwrap();
    let first = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(first.state, TaskState::Failed);

    runtime
        .retry(
            &owner,
            &task_id,
            TaskAttemptAuthorization::new("agent-b").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.second_started.notified())
        .await
        .unwrap();
    let accepted = runtime
        .send_input(
            &owner,
            &task_id,
            TaskInputSubmission::new(
                "attempt-two",
                TaskInput::Text {
                    text: "current".to_string(),
                },
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.attempt_no, 2);
    assert_eq!(accepted.input_seq, 1);

    let sinks = runner.sinks.lock().await;
    assert_eq!(sinks.len(), 2);
    assert!(sinks[0].1.inputs.lock().await.is_empty());
    assert_eq!(
        sinks[1].1.inputs.lock().await.as_slice(),
        &[TaskInput::Text {
            text: "current".to_string()
        }]
    );
    drop(sinks);

    runner.release_second.notify_one();
    let second = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(second.state, TaskState::Completed);
    assert_eq!(second.attempt_no, 2);
}

#[derive(Default)]
struct MemoryLiveOutputSource {
    records: Mutex<Vec<TaskLiveOutputRecord>>,
    sealed: AtomicBool,
}

impl MemoryLiveOutputSource {
    async fn push(&self, source: &str, text: &str) {
        self.records.lock().await.push(TaskLiveOutputRecord {
            source: source.to_string(),
            text: text.to_string(),
        });
    }
}

#[async_trait]
impl TaskLiveOutputSource for MemoryLiveOutputSource {
    async fn end_cursor(&self) -> Result<TaskLiveOutputCursor, TaskLiveOutputError> {
        let mut cursor = TaskLiveOutputCursor::default();
        cursor.positions.insert(
            "records".to_string(),
            self.records.lock().await.len() as u64,
        );
        Ok(cursor)
    }

    async fn read_records(
        &self,
        cursor: &TaskLiveOutputCursor,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<TaskLiveOutputBatch, TaskLiveOutputError> {
        let start = cursor.positions.get("records").copied().unwrap_or(0) as usize;
        let records = self.records.lock().await;
        let mut selected = Vec::new();
        let mut bytes = 0;
        for record in records.iter().skip(start).take(max_records) {
            let record_bytes = record.text.len();
            if !selected.is_empty() && bytes + record_bytes > max_bytes {
                break;
            }
            bytes += record_bytes;
            selected.push(record.clone());
        }
        let mut next_cursor = cursor.clone();
        let next_index = start + selected.len();
        next_cursor
            .positions
            .insert("records".to_string(), next_index as u64);
        Ok(TaskLiveOutputBatch {
            records: selected,
            next_cursor,
            sealed: self.sealed.load(Ordering::SeqCst) && next_index >= records.len(),
            has_more: next_index < records.len(),
        })
    }

    async fn seal(&self) {
        self.sealed.store(true, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct WatchNotificationSink {
    notifications: Mutex<Vec<TaskWatchOutputNotification>>,
    changed: Notify,
}

#[async_trait]
impl TaskNotificationSink for WatchNotificationSink {
    async fn watch_output(
        &self,
        notification: TaskWatchOutputNotification,
    ) -> Result<(), TaskRunnerError> {
        self.notifications.lock().await.push(notification);
        self.changed.notify_one();
        Ok(())
    }

    async fn terminal(
        &self,
        _notification: rua_task::TaskCompletionNotification,
    ) -> Result<(), TaskRunnerError> {
        Ok(())
    }
}

struct WatchRunner {
    source: Arc<MemoryLiveOutputSource>,
    session: Mutex<Option<Arc<TaskRunnerSession>>>,
    started: Notify,
    release: Notify,
}

#[async_trait]
impl TaskRunner for WatchRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        *self.session.lock().await = Some(request.session.clone());
        request
            .session
            .activate_live_output(self.source.clone())
            .await
            .unwrap();
        self.source.push("stdout", "historical").await;
        request.session.notify_live_output();
        self.started.notify_one();
        self.release.notified().await;
        self.source.seal().await;
        request.session.notify_live_output();
        Ok(TaskRunOutput::completed("watched"))
    }
}

#[tokio::test]
async fn watcher_is_prospective_idempotent_and_unwatch_stops_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(WatchNotificationSink::default());
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(TaskRuntimeConfig::new(dir.path()), sink.clone()).unwrap(),
    );
    let runner = Arc::new(WatchRunner {
        source: Arc::new(MemoryLiveOutputSource::default()),
        session: Mutex::new(None),
        started: Notify::new(),
        release: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("agent-a", "chat-a");
    let task_id = runtime
        .create(
            TaskSpec {
                owner: owner.clone(),
                instruction: "watch".to_string(),
                runner: TaskRunnerSpec::Tool {
                    tool_name: "Watch".to_string(),
                    arguments: serde_json::json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            TaskAttemptAuthorization::new("agent-a").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let first = runtime.watch(&owner, &task_id).await.unwrap();
    let duplicate = runtime.watch(&owner, &task_id).await.unwrap();
    assert_eq!(duplicate, first);
    runner.source.push("stdout", "future-one").await;
    runner.source.push("stderr", "future-two").await;
    runner
        .session
        .lock()
        .await
        .as_ref()
        .expect("session")
        .notify_live_output();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !sink.notifications.lock().await.is_empty() {
                break;
            }
            sink.changed.notified().await;
        }
    })
    .await
    .unwrap();
    let notifications = sink.notifications.lock().await;
    assert!(notifications
        .iter()
        .all(|notification| notification.projection == TaskLiveOutputProjection::ModelVisible));
    let records = notifications
        .iter()
        .flat_map(|notification| notification.records.iter())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        records,
        vec![
            TaskLiveOutputRecord {
                source: "stdout".to_string(),
                text: "future-one".to_string(),
            },
            TaskLiveOutputRecord {
                source: "stderr".to_string(),
                text: "future-two".to_string(),
            },
        ]
    );
    drop(notifications);

    runtime.unwatch(&owner, &task_id).await.unwrap();
    runner.source.push("stdout", "after-unwatch").await;
    runner
        .session
        .lock()
        .await
        .as_ref()
        .expect("session")
        .notify_live_output();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), sink.changed.notified())
            .await
            .is_err()
    );
    runner.release.notify_one();
    runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
}

#[test]
fn runner_kind_defines_live_output_projection_without_source_name_inference() {
    assert_eq!(
        TaskRunnerSpec::Tool {
            tool_name: "bash".to_string(),
            arguments: serde_json::json!({}),
        }
        .live_output_projection(),
        TaskLiveOutputProjection::ModelVisible
    );
    for runner in [
        TaskRunnerSpec::RuaAgent {
            agent_id: "worker".to_string(),
        },
        TaskRunnerSpec::SubAgent {},
        TaskRunnerSpec::Acp {
            runner: "codex".to_string(),
        },
    ] {
        assert_eq!(
            runner.live_output_projection(),
            TaskLiveOutputProjection::ActivityOnly
        );
    }
}

struct RetryWatchRunner {
    sources: Mutex<Vec<(u32, Arc<MemoryLiveOutputSource>)>>,
    started: Notify,
    release_first: Notify,
    release_second: Notify,
}

#[async_trait]
impl TaskRunner for RetryWatchRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        let source = Arc::new(MemoryLiveOutputSource::default());
        request
            .session
            .activate_live_output(source.clone())
            .await
            .unwrap();
        source
            .push("stdout", &format!("attempt-{}-line", request.attempt_no))
            .await;
        request.session.notify_live_output();
        self.sources
            .lock()
            .await
            .push((request.attempt_no, source.clone()));
        self.started.notify_one();
        if request.attempt_no == 1 {
            self.release_first.notified().await;
            source.seal().await;
            request.session.notify_live_output();
            return Ok(TaskRunOutput::failed("first failed", "retry watcher"));
        }
        self.release_second.notified().await;
        source.seal().await;
        request.session.notify_live_output();
        Ok(TaskRunOutput::completed("second completed"))
    }
}

#[tokio::test]
async fn failed_watcher_reattaches_from_retry_attempt_start() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(WatchNotificationSink::default());
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(TaskRuntimeConfig::new(dir.path()), sink.clone()).unwrap(),
    );
    let runner = Arc::new(RetryWatchRunner {
        sources: Mutex::new(Vec::new()),
        started: Notify::new(),
        release_first: Notify::new(),
        release_second: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("agent-a", "chat-a");
    let task_id = runtime
        .create(
            TaskSpec {
                owner: owner.clone(),
                instruction: "retry watch".to_string(),
                runner: TaskRunnerSpec::Tool {
                    tool_name: "RetryWatch".to_string(),
                    arguments: serde_json::json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            TaskAttemptAuthorization::new("agent-a").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();
    runtime.watch(&owner, &task_id).await.unwrap();
    runner.release_first.notify_one();
    let failed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    assert!(sink.notifications.lock().await.is_empty());

    runtime
        .retry(
            &owner,
            &task_id,
            TaskAttemptAuthorization::new("agent-b").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !sink.notifications.lock().await.is_empty() {
                break;
            }
            sink.changed.notified().await;
        }
    })
    .await
    .unwrap();
    let notifications = sink.notifications.lock().await;
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].attempt_no, 2);
    assert_eq!(
        notifications[0].projection,
        TaskLiveOutputProjection::ModelVisible
    );
    assert_eq!(
        notifications[0].records,
        vec![TaskLiveOutputRecord {
            source: "stdout".to_string(),
            text: "attempt-2-line".to_string(),
        }]
    );
    drop(notifications);

    runner.release_second.notify_one();
    let completed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(completed.state, TaskState::Completed);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !runtime.unwatch(&owner, &task_id).await.unwrap().active {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

struct FailOnceWatchSink {
    calls: AtomicUsize,
    accepted: Mutex<Vec<TaskWatchOutputNotification>>,
    changed: Notify,
}

#[async_trait]
impl TaskNotificationSink for FailOnceWatchSink {
    async fn watch_output(
        &self,
        notification: TaskWatchOutputNotification,
    ) -> Result<(), TaskRunnerError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(TaskRunnerError::new("fixture rejection"));
        }
        self.accepted.lock().await.push(notification);
        self.changed.notify_one();
        Ok(())
    }

    async fn terminal(
        &self,
        _notification: rua_task::TaskCompletionNotification,
    ) -> Result<(), TaskRunnerError> {
        Ok(())
    }
}

struct BlockingWatchSink {
    notifications: Mutex<Vec<TaskWatchOutputNotification>>,
    changed: Notify,
    release_first: Notify,
    calls: AtomicUsize,
}

#[async_trait]
impl TaskNotificationSink for BlockingWatchSink {
    async fn watch_output(
        &self,
        notification: TaskWatchOutputNotification,
    ) -> Result<(), TaskRunnerError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.notifications.lock().await.push(notification);
        self.changed.notify_waiters();
        if call == 0 {
            self.release_first.notified().await;
        }
        Ok(())
    }

    async fn terminal(
        &self,
        _notification: rua_task::TaskCompletionNotification,
    ) -> Result<(), TaskRunnerError> {
        Ok(())
    }
}

#[tokio::test]
async fn watcher_coalesces_dense_output_and_waits_for_consumption_before_next_wake() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(BlockingWatchSink {
        notifications: Mutex::new(Vec::new()),
        changed: Notify::new(),
        release_first: Notify::new(),
        calls: AtomicUsize::new(0),
    });
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(TaskRuntimeConfig::new(dir.path()), sink.clone()).unwrap(),
    );
    let runner = Arc::new(WatchRunner {
        source: Arc::new(MemoryLiveOutputSource::default()),
        session: Mutex::new(None),
        started: Notify::new(),
        release: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("agent-a", "chat-a");
    let task_id = runtime
        .create(
            TaskSpec {
                owner: owner.clone(),
                instruction: "dense watch".to_string(),
                runner: TaskRunnerSpec::Tool {
                    tool_name: "Watch".to_string(),
                    arguments: serde_json::json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            TaskAttemptAuthorization::new("agent-a").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();
    runtime.watch(&owner, &task_id).await.unwrap();

    for index in 0..250 {
        runner
            .source
            .push("stdout", &format!("dense-{index:03}"))
            .await;
    }
    runner
        .session
        .lock()
        .await
        .as_ref()
        .unwrap()
        .notify_live_output();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !sink.notifications.lock().await.is_empty() {
                break;
            }
            sink.changed.notified().await;
        }
    })
    .await
    .unwrap();

    let first = sink.notifications.lock().await[0].clone();
    assert!(first.records.len() <= 100);
    assert_eq!(first.records.first().unwrap().text, "dense-000");
    assert_eq!(first.records.last().unwrap().text, "dense-249");
    assert!(first
        .records
        .iter()
        .any(|record| record.text.contains("records omitted")));

    runner.source.push("stdout", "after-first-wake").await;
    runner
        .session
        .lock()
        .await
        .as_ref()
        .unwrap()
        .notify_live_output();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(sink.notifications.lock().await.len(), 1);

    sink.release_first.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if sink.notifications.lock().await.len() >= 2 {
                break;
            }
            sink.changed.notified().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        sink.notifications.lock().await[1].records,
        vec![TaskLiveOutputRecord {
            source: "stdout".to_string(),
            text: "after-first-wake".to_string(),
        }]
    );

    runner.release.notify_one();
    runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn watcher_advances_only_after_notification_sink_accepts_batch() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(FailOnceWatchSink {
        calls: AtomicUsize::new(0),
        accepted: Mutex::new(Vec::new()),
        changed: Notify::new(),
    });
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(TaskRuntimeConfig::new(dir.path()), sink.clone()).unwrap(),
    );
    let runner = Arc::new(WatchRunner {
        source: Arc::new(MemoryLiveOutputSource::default()),
        session: Mutex::new(None),
        started: Notify::new(),
        release: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("agent-a", "chat-a");
    let task_id = runtime
        .create(
            TaskSpec {
                owner: owner.clone(),
                instruction: "retry delivery".to_string(),
                runner: TaskRunnerSpec::Tool {
                    tool_name: "Watch".to_string(),
                    arguments: serde_json::json!({}),
                },
                context_refs: Vec::new(),
                depends_on: Vec::new(),
                origin: Default::default(),
            },
            TaskAttemptAuthorization::new("agent-a").unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();
    runtime.watch(&owner, &task_id).await.unwrap();
    runner.source.push("stdout", "retry-me").await;
    runner
        .session
        .lock()
        .await
        .as_ref()
        .unwrap()
        .notify_live_output();

    tokio::time::timeout(Duration::from_secs(2), sink.changed.notified())
        .await
        .unwrap();
    assert!(sink.calls.load(Ordering::SeqCst) >= 2);
    let accepted = sink.accepted.lock().await;
    assert_eq!(accepted.len(), 1);
    assert_eq!(
        accepted[0].records,
        vec![TaskLiveOutputRecord {
            source: "stdout".to_string(),
            text: "retry-me".to_string(),
        }]
    );
    drop(accepted);

    runner.release.notify_one();
    runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
}
