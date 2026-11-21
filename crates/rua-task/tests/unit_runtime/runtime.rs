use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rua_core::ChatAddress;
use rua_task::{
    TaskAdoptedExecution, TaskArtifact, TaskArtifactPayload, TaskAttemptAuthorization,
    TaskAuthorityError, TaskAuthorityResolver, TaskCompletionNotification, TaskDependencyResult,
    TaskDeviceContext, TaskError, TaskInput, TaskInputSubmission, TaskListFilter,
    TaskNotificationSink, TaskOwner, TaskResourceRef, TaskRunOutput, TaskRunRequest, TaskRunner,
    TaskRunnerError, TaskRunnerKind, TaskRunnerSpec, TaskRuntime, TaskRuntimeConfig, TaskSpec,
    TaskState,
};
use tokio::sync::{Mutex, Notify};

fn authorization(agent_id: &str) -> TaskAttemptAuthorization {
    TaskAttemptAuthorization::new(agent_id).unwrap()
}

struct BlockingRunner {
    calls: AtomicUsize,
    started: Notify,
    release: Notify,
}

impl BlockingRunner {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[async_trait]
impl TaskRunner for BlockingRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_waiters();
        tokio::select! {
            _ = self.release.notified() => {},
            _ = request.cancel.cancelled() => return Ok(TaskRunOutput::cancelled()),
        }
        Ok(TaskRunOutput {
            outcome: rua_task::TaskRunOutcome::Completed,
            output:
                "Outcome\nDone\n\nKey Results\nok\n\nVerification\npassed\n\nRemaining Work\nNone"
                    .to_string(),
            raw_output: Some(
                serde_json::to_vec(&serde_json::json!({
                    "task_id": request.task_id,
                    "value": "full"
                }))
                .unwrap(),
            ),
            artifacts: Vec::new(),
            artifact_payloads: Vec::new(),
            error: None,
        })
    }
}

#[derive(Default)]
struct RecordingSink {
    notifications: Mutex<Vec<TaskCompletionNotification>>,
    notify: Notify,
}

struct MutableAuthority {
    current: Mutex<TaskDeviceContext>,
}

impl MutableAuthority {
    fn new(current: TaskDeviceContext) -> Self {
        Self {
            current: Mutex::new(current),
        }
    }

    async fn set(&self, current: TaskDeviceContext) {
        *self.current.lock().await = current;
    }
}

#[async_trait]
impl TaskAuthorityResolver for MutableAuthority {
    async fn accept_attempt(
        &self,
        _spec: &TaskSpec,
        _authorization: &TaskAttemptAuthorization,
        _prior: Option<&TaskDeviceContext>,
    ) -> Result<TaskDeviceContext, TaskAuthorityError> {
        Ok(self.current.lock().await.clone())
    }

    async fn is_current(
        &self,
        _owner: &TaskOwner,
        accepted: &TaskDeviceContext,
    ) -> Result<bool, TaskAuthorityError> {
        let current = self.current.lock().await;
        Ok(accepted.device_id == current.device_id
            && accepted.authority_epoch == current.authority_epoch)
    }

    async fn local_device_id(&self) -> Result<Option<String>, TaskAuthorityError> {
        Ok(self.current.lock().await.device_id.clone())
    }
}

struct RejectingAuthority;

struct AcceptanceGate {
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl TaskAuthorityResolver for AcceptanceGate {
    async fn accept_attempt(
        &self,
        _: &TaskSpec,
        _: &TaskAttemptAuthorization,
        _: Option<&TaskDeviceContext>,
    ) -> Result<TaskDeviceContext, TaskAuthorityError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(TaskDeviceContext::default())
    }
    async fn is_current(
        &self,
        _: &TaskOwner,
        _: &TaskDeviceContext,
    ) -> Result<bool, TaskAuthorityError> {
        Ok(true)
    }
    async fn local_device_id(&self) -> Result<Option<String>, TaskAuthorityError> {
        Ok(None)
    }
}

#[tokio::test]
async fn cancel_during_acceptance_waits_for_the_original_execution_registration() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Arc::new(AcceptanceGate {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let runtime = Arc::new(
        TaskRuntime::open_with_sink_and_authority(
            TaskRuntimeConfig::new(dir.path()),
            Arc::new(RecordingSink::default()),
            gate.clone(),
        )
        .unwrap(),
    );
    runtime.register_runner(Arc::new(RedispatchProbeRunner {
        calls: AtomicUsize::new(0),
    }));
    let owner = TaskOwner::new("accept-cancel-race");
    let original_cancel = tokio_util::sync::CancellationToken::new();
    let adopting = tokio::spawn({
        let runtime = runtime.clone();
        let owner = owner.clone();
        let cancel = original_cancel.clone();
        async move {
            let observed = cancel.clone();
            runtime
                .adopt_running(
                    "chat-command-accepting".into(),
                    spec(owner),
                    authorization("agent-a"),
                    TaskAdoptedExecution::new(
                        async move {
                            observed.cancelled().await;
                            TaskRunOutput::cancelled()
                        },
                        cancel,
                    ),
                )
                .await
        }
    });
    gate.entered.notified().await;
    let cancelling = tokio::spawn({
        let runtime = runtime.clone();
        let owner = owner.clone();
        async move { runtime.cancel(&owner, "chat-command-accepting").await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !cancelling.is_finished(),
        "cancel cannot decide there is no execution during acceptance"
    );
    gate.release.notify_one();
    adopting.await.unwrap().unwrap();
    let cancelled = cancelling.await.unwrap().unwrap();
    assert!(original_cancel.is_cancelled());
    assert_eq!(cancelled.state, TaskState::Cancelled);
    assert_eq!(cancelled.attempts.len(), 1);
}

struct PausedTerminalSink {
    entered: Notify,
    release: Notify,
}

#[tokio::test]
async fn cancelled_attempt_receipt_does_not_follow_a_concurrent_retry() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("cancel-receipt-retry");
    let id = "adopted-cancel-receipt".to_owned();
    let original_cancel = tokio_util::sync::CancellationToken::new();
    let release = Arc::new(Notify::new());
    let execution_cancel = original_cancel.clone();
    let execution_release = release.clone();
    runtime
        .adopt_running(
            id.clone(),
            spec(owner.clone()),
            authorization("agent-a"),
            TaskAdoptedExecution::new(
                async move {
                    execution_cancel.cancelled().await;
                    execution_release.notified().await;
                    TaskRunOutput::cancelled()
                },
                original_cancel.clone(),
            ),
        )
        .await
        .unwrap();
    let mut cancellation = Box::pin(runtime.cancel(&owner, &id));
    tokio::select! {
        biased;
        result = &mut cancellation => panic!("Cancellation completed before execution stopped: {result:?}"),
        _ = original_cancel.cancelled() => {},
    }
    // Keep the caller unpolled while the target finishes and another caller
    // explicitly retries, as can happen before a remote Cancel reply arrives.
    release.notify_one();
    assert_eq!(
        runtime
            .await_terminal(&owner, &id, Duration::from_secs(2))
            .await
            .unwrap()
            .state,
        TaskState::Cancelled
    );
    runtime
        .retry(&owner, &id, authorization("agent-b"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runner.calls.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let receipt = cancellation.await.unwrap();
    assert_eq!(receipt.attempt_no, 1);
    assert_eq!(receipt.state, TaskState::Cancelled);
    let current = runtime.get(&owner, &id).await.unwrap();
    assert_eq!(current.attempt_no, 2);
    assert_eq!(current.state, TaskState::Running);
    runner.release.notify_one();
    runtime.shutdown().await.unwrap();
}

async fn assert_terminal_delivery_releases_execution_capacity(adopted: bool) {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(PausedTerminalSink {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.max_concurrent_tasks_per_chat = 1;
    config.limits.max_concurrent_tasks_per_runner = 1;
    let runtime = Arc::new(TaskRuntime::open_with_sink(config, sink.clone()).unwrap());
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("terminal-capacity");
    let (complete, completion) = tokio::sync::oneshot::channel();
    let first = if adopted {
        let id = "chat-command-terminal-capacity".to_string();
        runtime
            .adopt_running(
                id.clone(),
                spec(owner.clone()),
                authorization("agent-a"),
                TaskAdoptedExecution::new(
                    async { completion.await.unwrap() },
                    tokio_util::sync::CancellationToken::new(),
                ),
            )
            .await
            .unwrap();
        id
    } else {
        let id = runtime
            .create(spec(owner.clone()), authorization("agent-a"))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while runner.calls.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        id
    };
    let mut dependent = spec(owner.clone());
    dependent.depends_on = vec![first.clone()];
    let next = runtime
        .create(dependent, authorization("agent-a"))
        .await
        .unwrap();
    if adopted {
        complete
            .send(TaskRunOutput::completed("original completed"))
            .unwrap();
    } else {
        runner.release.notify_one();
    }
    sink.entered.notified().await;
    assert_eq!(
        runtime.get(&owner, &first).await.unwrap().state,
        TaskState::Completed
    );
    assert!(runtime.attempt_execution_active(&owner, &first, 1));
    tokio::time::timeout(Duration::from_secs(2), async {
        let expected = if adopted { 1 } else { 2 };
        while runner.calls.load(Ordering::SeqCst) != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dependent execution must start before the first result delivery is acknowledged");
    assert_eq!(
        runtime.get(&owner, &next).await.unwrap().state,
        TaskState::Running
    );
    runner.release.notify_one();
    sink.entered.notified().await;
    sink.release.notify_waiters();
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn runner_releases_capacity_while_terminal_delivery_waits() {
    assert_terminal_delivery_releases_execution_capacity(false).await;
}

#[tokio::test]
async fn adopted_execution_releases_capacity_while_terminal_delivery_waits() {
    assert_terminal_delivery_releases_execution_capacity(true).await;
}

#[tokio::test]
async fn cancellation_returns_real_terminal_before_notification_acknowledgement() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(PausedTerminalSink {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(TaskRuntimeConfig::new(dir.path()), sink.clone()).unwrap(),
    );
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("cancel-terminal-receipt");
    let id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runner.calls.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let cancelling = {
        let runtime = runtime.clone();
        let owner = owner.clone();
        let id = id.clone();
        tokio::spawn(async move { runtime.cancel(&owner, &id).await })
    };
    sink.entered.notified().await;
    let result = tokio::time::timeout(Duration::from_secs(1), cancelling)
        .await
        .expect("real cancellation must not wait for its Chat result receipt")
        .unwrap()
        .unwrap();
    assert_eq!(result.state, TaskState::Cancelled);
    assert!(runtime.attempt_execution_active(&owner, &id, 1));
    sink.release.notify_one();
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn waiting_dependency_cancellation_returns_before_notification_and_never_dispatches() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(PausedTerminalSink {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(TaskRuntimeConfig::new(dir.path()), sink.clone()).unwrap(),
    );
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("cancel-waiting-receipt");
    let upstream = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runner.calls.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut dependency = spec(owner.clone());
    dependency.depends_on = vec![upstream.clone()];
    let id = runtime
        .create(dependency, authorization("agent-a"))
        .await
        .unwrap();
    assert!(!runtime.attempt_execution_active(&owner, &id, 1));
    let cancelling = {
        let runtime = runtime.clone();
        let owner = owner.clone();
        let id = id.clone();
        tokio::spawn(async move { runtime.cancel(&owner, &id).await })
    };
    sink.entered.notified().await;
    let result = tokio::time::timeout(Duration::from_secs(1), cancelling)
        .await
        .expect("waiting cancellation must not wait for its result receipt")
        .unwrap()
        .unwrap();
    assert_eq!(result.state, TaskState::Cancelled);
    assert!(runtime.attempt_execution_active(&owner, &id, 1));
    assert!(runtime
        .outstanding_attempts()
        .await
        .unwrap()
        .iter()
        .any(|record| record.task_id == id && record.attempt_no == 1));
    runner.release.notify_one();
    sink.entered.notified().await;
    assert_eq!(
        runtime.get(&owner, &upstream).await.unwrap().state,
        TaskState::Completed
    );
    assert_eq!(
        runtime.get(&owner, &id).await.unwrap().state,
        TaskState::Cancelled
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        1,
        "a cancelled dependency must never dispatch when its parent becomes ready"
    );
    sink.release.notify_waiters();
    runtime.shutdown().await.unwrap();
    assert!(!runtime.attempt_execution_active(&owner, &id, 1));
}

#[async_trait]
impl TaskNotificationSink for PausedTerminalSink {
    async fn terminal(&self, _: TaskCompletionNotification) -> Result<(), TaskRunnerError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(())
    }
}

#[tokio::test]
async fn retry_preserves_both_attempts_until_old_notification_tail_exits() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(PausedTerminalSink {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(TaskRuntimeConfig::new(dir.path()), sink.clone()).unwrap(),
    );
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("retry-delivery-tail");
    let id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runner.calls.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        runtime.cancel(&owner, &id).await.unwrap().state,
        TaskState::Cancelled
    );
    sink.entered.notified().await;
    runtime
        .retry(&owner, &id, authorization("agent-b"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runner.calls.load(Ordering::SeqCst) != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        runtime.attempt_execution_active(&owner, &id, 1),
        "Retry must not hide the old notification tail from drain"
    );
    assert!(runtime.attempt_execution_active(&owner, &id, 2));
    assert!(matches!(
        runtime.cancel_attempt(&owner, &id, 1).await,
        Err(TaskError::InvalidTransition(_))
    ));
    assert_eq!(
        runtime.get(&owner, &id).await.unwrap().state,
        TaskState::Running
    );
    let outstanding = runtime.outstanding_attempts().await.unwrap();
    for (attempt_no, agent) in [(1, "agent-a"), (2, "agent-b")] {
        let record = outstanding
            .iter()
            .find(|record| record.task_id == id && record.attempt_no == attempt_no)
            .unwrap();
        assert_eq!(
            record
                .attempts
                .iter()
                .find(|attempt| attempt.attempt_no == attempt_no)
                .unwrap()
                .authorization_agent_id,
            agent
        );
    }
    sink.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.attempt_execution_active(&owner, &id, 1) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        runtime.attempt_execution_active(&owner, &id, 2),
        "the old receipt cannot remove the Retry execution handle"
    );
    let cancelled = runtime.cancel(&owner, &id).await.unwrap();
    assert_eq!(cancelled.state, TaskState::Cancelled);
    assert_eq!(cancelled.attempt_no, 2);
    sink.entered.notified().await;
    sink.release.notify_one();
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn terminal_attempt_remains_outstanding_until_notification_tail_exits() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(PausedTerminalSink {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(TaskRuntimeConfig::new(dir.path()), sink.clone()).unwrap(),
    );
    runtime.register_runner(Arc::new(FailingRunner));
    let owner = TaskOwner::new("tail");
    let id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    sink.entered.notified().await;
    assert!(runtime.get(&owner, &id).await.unwrap().state.is_terminal());
    assert!(runtime.attempt_execution_active(&owner, &id, 1));
    assert!(runtime
        .outstanding_attempts()
        .await
        .unwrap()
        .iter()
        .any(|record| record.task_id == id));
    sink.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.attempt_execution_active(&owner, &id, 1) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!runtime
        .outstanding_attempts()
        .await
        .unwrap()
        .iter()
        .any(|record| record.task_id == id));
    runtime.shutdown().await.unwrap();
}

#[async_trait]
impl TaskAuthorityResolver for RejectingAuthority {
    async fn accept_attempt(
        &self,
        _spec: &TaskSpec,
        _authorization: &TaskAttemptAuthorization,
        _prior: Option<&TaskDeviceContext>,
    ) -> Result<TaskDeviceContext, TaskAuthorityError> {
        Err(TaskAuthorityError::Rejected(
            "local Device is not the Chat authority".to_string(),
        ))
    }

    async fn is_current(
        &self,
        _owner: &TaskOwner,
        _accepted: &TaskDeviceContext,
    ) -> Result<bool, TaskAuthorityError> {
        Ok(false)
    }

    async fn local_device_id(&self) -> Result<Option<String>, TaskAuthorityError> {
        Ok(Some("device-b".to_string()))
    }
}

#[async_trait]
impl TaskNotificationSink for RecordingSink {
    async fn terminal(
        &self,
        notification: TaskCompletionNotification,
    ) -> Result<(), TaskRunnerError> {
        self.notifications.lock().await.push(notification);
        self.notify.notify_waiters();
        Ok(())
    }
}

fn spec(owner: TaskOwner) -> TaskSpec {
    graph_spec(owner, "read the fixture", Vec::new())
}

fn graph_spec(owner: TaskOwner, instruction: &str, depends_on: Vec<String>) -> TaskSpec {
    TaskSpec {
        owner,
        instruction: instruction.to_string(),
        runner: TaskRunnerSpec::Tool {
            tool_name: "Fixture_Read".to_string(),
            arguments: serde_json::json!({"path": "fixture.txt"}),
        },
        context_refs: Vec::new(),
        depends_on,
        origin: Default::default(),
    }
}

#[test]
fn task_spec_loads_legacy_timeout_without_reemitting_it() {
    let mut legacy = serde_json::to_value(spec(TaskOwner::new("chat-a"))).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .insert("timeout_secs".to_string(), serde_json::json!(5));

    let parsed: TaskSpec = serde_json::from_value(legacy).unwrap();
    let current = serde_json::to_value(parsed).unwrap();

    assert!(current.get("timeout_secs").is_none());
}

#[tokio::test]
async fn new_conversation_task_runners_are_rejected_without_creating_records() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let owner = TaskOwner::new("chat-a");
    for runner in [
        TaskRunnerSpec::RuaAgent {
            agent_id: "research".to_string(),
        },
        TaskRunnerSpec::SubAgent {},
        TaskRunnerSpec::Acp {
            runner: "codex".to_string(),
        },
    ] {
        let mut rejected = spec(owner.clone());
        rejected.runner = runner;
        let error = runtime
            .create(rejected, authorization("agent-a"))
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("independent Chat for Agent or ACP collaboration"));
    }
    assert!(runtime
        .list(&owner, TaskListFilter::default())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn create_persists_before_dispatch_and_completes_once() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(
            TaskRuntimeConfig::new(dir.path()),
            sink.clone() as Arc<dyn TaskNotificationSink>,
        )
        .unwrap(),
    );
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");

    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let running = runtime.get(&owner, &task_id).await.unwrap();
    assert_eq!(running.state, TaskState::Running);
    assert_eq!(running.attempt_no, 1);

    runner.release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while sink.notifications.lock().await.len() != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();

    let completed = runtime.get(&owner, &task_id).await.unwrap();
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    assert!(completed
        .result
        .as_ref()
        .and_then(|result| result.full_result_ref.as_ref())
        .is_some());

    let raw = runtime.read_full_result(&owner, &task_id).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["value"],
        "full"
    );

    let reopened = TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap();
    let persisted = reopened.get(&owner, &task_id).await.unwrap();
    assert_eq!(persisted.state, TaskState::Completed);
    assert_eq!(persisted.result, completed.result);
    let reopened_raw = reopened.read_full_result(&owner, &task_id).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&reopened_raw).unwrap()["value"],
        "full"
    );
}

#[tokio::test]
async fn owner_scope_is_enforced_for_get_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    assert!(runtime
        .get(&TaskOwner::new("chat-b"), &task_id)
        .await
        .is_err());
    assert!(runtime
        .list(&TaskOwner::new("chat-b"), TaskListFilter::default())
        .await
        .unwrap()
        .is_empty());
    let same_chat = TaskOwner::from_address(ChatAddress::new("chat-a"));
    assert_eq!(
        runtime.get(&same_chat, &task_id).await.unwrap().task_id,
        task_id
    );
    assert_eq!(
        runtime
            .list(&owner, TaskListFilter::default())
            .await
            .unwrap()
            .len(),
        1
    );

    runner.release.notify_one();
}

struct FailingRunner;

#[async_trait]
impl TaskRunner for FailingRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, _request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        Err(TaskRunnerError::new("fixture runner failed"))
    }
}

#[tokio::test]
async fn runner_failure_persists_common_failed_result() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(
            TaskRuntimeConfig::new(dir.path()),
            sink.clone() as Arc<dyn TaskNotificationSink>,
        )
        .unwrap(),
    );
    runtime.register_runner(Arc::new(FailingRunner));
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), sink.notify.notified())
        .await
        .unwrap();

    let record = runtime.get(&owner, &task_id).await.unwrap();
    assert_eq!(record.state, TaskState::Failed);
    let result = record.result.unwrap();
    assert_eq!(result.status, TaskState::Failed);
    assert!(result.output.contains("Task runner failed"));
    assert!(result.output.contains("fixture runner failed"));
    assert_eq!(result.error.unwrap().message, "fixture runner failed");
}

struct LongOutputRunner;

#[async_trait]
impl TaskRunner for LongOutputRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, _request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        Ok(TaskRunOutput::completed("x".repeat(500)))
    }
}

#[tokio::test]
async fn task_result_output_is_bounded_and_full_text_remains_recallable() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.max_result_output_chars = 96;
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    runtime.register_runner(Arc::new(LongOutputRunner));
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    let completed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    let result = completed.result.unwrap();
    assert_eq!(result.output.chars().count(), 96);
    assert!(result.output.contains("truncated"));
    assert!(result.full_result_ref.is_some());

    let raw = runtime.read_full_result(&owner, &task_id).await.unwrap();
    let raw: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(raw["output"].as_str().unwrap().chars().count(), 500);
}

#[tokio::test]
async fn await_timeout_detaches_without_cancelling_task() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let error = runtime
        .await_terminal(&owner, &task_id, Duration::from_millis(20))
        .await
        .unwrap_err();
    assert!(matches!(error, rua_task::TaskError::WaitTimedOut(_)));
    assert!(!runtime
        .get(&owner, &task_id)
        .await
        .unwrap()
        .state
        .is_terminal());

    runner.release.notify_one();
    let record = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(record.state, TaskState::Completed);
}

struct CancellationRunner {
    started: Notify,
    observed_cancel: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl TaskRunner for CancellationRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        self.started.notify_one();
        request.cancel.cancelled().await;
        self.observed_cancel.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut output = TaskRunOutput::cancelled();
        output.output = "runner cleanup complete".into();
        Ok(output)
    }
}

struct CompletionRaceRunner {
    started: Notify,
    release: Notify,
    returning: Notify,
}

#[async_trait]
impl TaskRunner for CompletionRaceRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, _request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        self.started.notify_one();
        self.release.notified().await;
        self.returning.notify_one();
        Ok(TaskRunOutput::completed(
            "Outcome\nDone\n\nKey Results\nnatural completion\n\nVerification\npassed\n\nRemaining Work\nNone",
        ))
    }
}

struct ShutdownRunner {
    started: AtomicUsize,
    observed_cancel: AtomicUsize,
}

struct ImmediateStoppedRunner {
    started: Notify,
}

#[async_trait]
impl TaskRunner for ImmediateStoppedRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        self.started.notify_one();
        request.cancel.cancelled().await;
        let mut output = TaskRunOutput::cancelled();
        output.output = "process output before shutdown".into();
        output.raw_output = Some(b"retained process output".to_vec());
        Ok(output)
    }
}

#[tokio::test]
async fn shutdown_cause_survives_a_runner_returning_cancelled_without_an_extra_yield() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(ImmediateStoppedRunner {
        started: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("shutdown-race");
    let id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    runner.started.notified().await;
    runtime.shutdown().await.unwrap();
    let record = runtime.get(&owner, &id).await.unwrap();
    assert_eq!(record.state, TaskState::Interrupted);
    assert_eq!(record.attempts[0].state, TaskState::Interrupted);
    let result = record.result.unwrap();
    assert_eq!(result.output, "process output before shutdown");
    assert_eq!(
        result.error.unwrap().details["provenance"],
        "runtime_shutdown"
    );
    assert_eq!(
        runtime.read_full_result(&owner, &id).await.unwrap(),
        b"retained process output"
    );
}

#[tokio::test]
async fn begin_shutdown_preserves_waiting_task_cause_during_workspace_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(ShutdownRunner {
        started: AtomicUsize::new(0),
        observed_cancel: AtomicUsize::new(0),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("waiting-at-shutdown");
    let upstream = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    while runner.started.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    let dependent = runtime
        .create(
            graph_spec(
                owner.clone(),
                "waiting for upstream",
                vec![upstream.clone()],
            ),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    assert_eq!(
        runtime.get(&owner, &dependent).await.unwrap().state,
        TaskState::Waiting
    );

    runtime.begin_shutdown().await;
    let stopped = runtime.cancel_attempt(&owner, &dependent, 1).await.unwrap();
    assert_eq!(stopped.state, TaskState::Interrupted);
    assert_eq!(stopped.attempts[0].state, TaskState::Interrupted);
    assert_eq!(
        stopped.result.unwrap().error.unwrap().details["provenance"],
        "runtime_shutdown"
    );
    assert!(matches!(
        runtime
            .create(spec(owner.clone()), authorization("agent-a"))
            .await,
        Err(TaskError::ResourceUnavailable(_))
    ));
    runtime.shutdown().await.unwrap();
    assert_eq!(runner.started.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime.get(&owner, &upstream).await.unwrap().state,
        TaskState::Interrupted
    );
}

#[tokio::test]
async fn begin_shutdown_cause_survives_adopted_process_cleanup_becoming_ready_first() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(RedispatchProbeRunner {
        calls: AtomicUsize::new(0),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("adopted-at-shutdown");
    let cancel = tokio_util::sync::CancellationToken::new();
    let execution_cancel = cancel.clone();
    let started = Arc::new(Notify::new());
    let execution_started = started.clone();
    let id = runtime
        .adopt_running(
            "chat-command-adopted-shutdown".into(),
            spec(owner.clone()),
            authorization("agent-a"),
            TaskAdoptedExecution::new(
                async move {
                    execution_started.notify_one();
                    execution_cancel.cancelled().await;
                    let mut output = TaskRunOutput::cancelled();
                    output.output = "original process stopped".into();
                    output
                },
                cancel.clone(),
            ),
        )
        .await
        .unwrap();
    started.notified().await;
    runtime.begin_shutdown().await;
    // Process/Workspace cleanup can independently stop the same original
    // execution before the Task worker polls its own cancellation branch.
    cancel.cancel();
    runtime.shutdown().await.unwrap();
    let record = runtime.get(&owner, &id).await.unwrap();
    assert_eq!(record.state, TaskState::Interrupted);
    let result = record.result.unwrap();
    assert_eq!(result.output, "original process stopped");
    assert_eq!(
        result.error.unwrap().details["provenance"],
        "runtime_shutdown"
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn natural_completion_ready_before_begin_shutdown_remains_completed() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(CompletionRaceRunner {
        started: Notify::new(),
        release: Notify::new(),
        returning: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("completed-at-shutdown");
    let id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    runner.started.notified().await;
    runner.release.notify_one();
    runner.returning.notified().await;
    runtime.begin_shutdown().await;
    runtime.shutdown().await.unwrap();
    let record = runtime.get(&owner, &id).await.unwrap();
    assert_eq!(record.state, TaskState::Completed);
    assert!(record.result.unwrap().output.contains("natural completion"));
}

#[async_trait]
impl TaskRunner for ShutdownRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        request.cancel.cancelled().await;
        self.observed_cancel.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(TaskRunOutput::completed("shutdown cleanup complete"))
    }
}

#[tokio::test]
async fn shutdown_interrupts_tasks_across_all_chat_owners() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(ShutdownRunner {
        started: AtomicUsize::new(0),
        observed_cancel: AtomicUsize::new(0),
    });
    runtime.register_runner(runner.clone());
    let owners = [TaskOwner::new("chat-a"), TaskOwner::new("chat-b")];
    let mut task_ids = Vec::new();
    for owner in &owners {
        task_ids.push(
            runtime
                .create(spec(owner.clone()), authorization("agent-a"))
                .await
                .unwrap(),
        );
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while runner.started.load(Ordering::SeqCst) != owners.len() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();

    assert_eq!(runtime.shutdown().await.unwrap(), owners.len());
    assert_eq!(runner.observed_cancel.load(Ordering::SeqCst), owners.len());
    for (owner, task_id) in owners.iter().zip(&task_ids) {
        let record = runtime.get(owner, task_id).await.unwrap();
        assert_eq!(record.state, TaskState::Interrupted);
        assert_eq!(
            record.result.unwrap().error.unwrap().details["provenance"],
            "runtime_shutdown"
        );
    }
    let owner = &owners[0];
    let task_id = &task_ids[0];
    assert!(matches!(
        runtime
            .create(spec(owner.clone()), authorization("agent-a"))
            .await,
        Err(TaskError::ResourceUnavailable(_))
    ));
    assert!(matches!(
        runtime
            .retry(owner, task_id, authorization("agent-a"))
            .await,
        Err(TaskError::ResourceUnavailable(_))
    ));
    assert!(matches!(
        runtime
            .send_input(
                owner,
                task_id,
                TaskInputSubmission::new(
                    "shutdown-input",
                    TaskInput::Text {
                        text: "late".to_string()
                    }
                )
                .unwrap()
            )
            .await,
        Err(TaskError::ResourceUnavailable(_))
    ));
    assert!(matches!(
        runtime.watch(owner, task_id).await,
        Err(TaskError::ResourceUnavailable(_))
    ));
}

#[tokio::test]
async fn paused_agent_admission_rejects_new_tasks_without_affecting_other_agents() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(ShutdownRunner {
        started: AtomicUsize::new(0),
        observed_cancel: AtomicUsize::new(0),
    }));
    runtime.pause_agent_admission("agent-a").await;
    assert!(matches!(
        runtime
            .create(spec(TaskOwner::new("chat-a")), authorization("agent-a"))
            .await,
        Err(TaskError::ResourceUnavailable(_))
    ));
    assert!(runtime
        .create(spec(TaskOwner::new("chat-b")), authorization("agent-b"))
        .await
        .is_ok());
    runtime.resume_agent_admission("agent-a").await;
    assert!(runtime
        .create(spec(TaskOwner::new("chat-a")), authorization("agent-a"))
        .await
        .is_ok());
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_interrupts_waiting_dependencies_without_dispatching_them() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(ShutdownRunner {
        started: AtomicUsize::new(0),
        observed_cancel: AtomicUsize::new(0),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let upstream = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runner.started.load(Ordering::SeqCst) != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let dependent = runtime
        .create(
            graph_spec(
                owner.clone(),
                "must remain waiting during shutdown",
                vec![upstream.clone()],
            ),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    assert_eq!(
        runtime.get(&owner, &dependent).await.unwrap().state,
        TaskState::Waiting
    );

    assert_eq!(runtime.shutdown().await.unwrap(), 2);
    assert_eq!(runner.started.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime.get(&owner, &upstream).await.unwrap().state,
        TaskState::Interrupted
    );
    assert_eq!(
        runtime.get(&owner, &dependent).await.unwrap().state,
        TaskState::Interrupted
    );
}

struct DropFlag(Arc<std::sync::atomic::AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct UncooperativeRunner {
    started: Notify,
    dropped: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl TaskRunner for UncooperativeRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, _request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        let _drop_flag = DropFlag(self.dropped.clone());
        self.started.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_reaches_runner_and_wins_terminal_race() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.cancellation_grace = Duration::from_secs(1);
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    let runner = Arc::new(CancellationRunner {
        started: Notify::new(),
        observed_cancel: std::sync::atomic::AtomicBool::new(false),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let cancelled = runtime.cancel(&owner, &task_id).await.unwrap();
    assert_eq!(cancelled.state, TaskState::Cancelled);
    assert!(runner.observed_cancel.load(Ordering::SeqCst));
    let result = cancelled.result.unwrap();
    assert_eq!(result.output, "runner cleanup complete");
    assert!(result.full_result_ref.is_some());
    assert_eq!(result.error.unwrap().message, "task cancelled");
    let raw = runtime.read_full_result(&owner, &task_id).await.unwrap();
    let raw: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(raw["status"], "cancelled");
    assert_eq!(raw["output"], "runner cleanup complete");
}

#[tokio::test]
async fn natural_completion_ready_before_cancel_remains_completed() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(CompletionRaceRunner {
        started: Notify::new(),
        release: Notify::new(),
        returning: Notify::new(),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    runner.started.notified().await;
    runner.release.notify_one();
    runner.returning.notified().await;

    let record = runtime.cancel(&owner, &task_id).await.unwrap();
    assert_eq!(record.state, TaskState::Completed);
    assert!(record.result.unwrap().output.contains("natural completion"));
}

#[tokio::test]
async fn cancellation_wait_timeout_keeps_uncooperative_runner_until_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.cancellation_grace = Duration::from_millis(25);
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runner = Arc::new(UncooperativeRunner {
        started: Notify::new(),
        dropped: dropped.clone(),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    assert!(matches!(
        runtime.cancel(&owner, &task_id).await,
        Err(TaskError::WaitTimedOut(_))
    ));
    assert_eq!(
        runtime.get(&owner, &task_id).await.unwrap().state,
        TaskState::Running
    );
    assert!(!dropped.load(Ordering::SeqCst));
    runtime.shutdown().await.unwrap();
    assert_eq!(
        runtime.get(&owner, &task_id).await.unwrap().state,
        TaskState::Interrupted
    );
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn concurrent_cancellation_produces_one_terminal_transition() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(
            TaskRuntimeConfig::new(dir.path()),
            sink.clone() as Arc<dyn TaskNotificationSink>,
        )
        .unwrap(),
    );
    let runner = Arc::new(CancellationRunner {
        started: Notify::new(),
        observed_cancel: std::sync::atomic::AtomicBool::new(false),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let first = {
        let runtime = runtime.clone();
        let owner = owner.clone();
        let task_id = task_id.clone();
        tokio::spawn(async move { runtime.cancel(&owner, &task_id).await })
    };
    let second = {
        let runtime = runtime.clone();
        let owner = owner.clone();
        let task_id = task_id.clone();
        tokio::spawn(async move { runtime.cancel(&owner, &task_id).await })
    };
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    assert_eq!(first.state, TaskState::Cancelled);
    assert_eq!(second.state, TaskState::Cancelled);
    assert_eq!(first.result, second.result);
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.attempt_execution_active(&owner, &task_id, 1) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(sink.notifications.lock().await.len(), 1);
}

#[tokio::test]
async fn active_task_limit_is_checked_transactionally() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.max_active_tasks_per_chat = 1;
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let first = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let error = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap_err();
    assert!(matches!(error, rua_task::TaskError::LimitExceeded(_)));
    assert!(runtime
        .retry(&owner, &first, authorization("agent-b"))
        .await
        .is_err());

    runner.release.notify_one();
    runtime
        .await_terminal(&owner, &first, Duration::from_secs(2))
        .await
        .unwrap();
    let second = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    assert_ne!(first, second);
    runner.release.notify_one();
}

#[tokio::test]
async fn activity_subscription_reports_scoped_lifecycle_events() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let mut activity = runtime.subscribe_activity();
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();
    runner.release.notify_one();
    runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();

    let mut states = Vec::new();
    while states.last() != Some(&TaskState::Completed) {
        let event = tokio::time::timeout(Duration::from_secs(2), activity.recv())
            .await
            .unwrap()
            .unwrap();
        if event.task_id == task_id {
            states.push(event.state);
        }
    }
    assert_eq!(
        states,
        vec![
            TaskState::Accepted,
            TaskState::Waiting,
            TaskState::Running,
            TaskState::Completed,
        ]
    );
}

#[tokio::test]
async fn activity_snapshot_keeps_all_active_tasks_beyond_recent_list_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.max_active_tasks_per_chat = 128;
    config.limits.max_concurrent_tasks_per_chat = 1;
    config.limits.activity_channel_capacity = 8;
    config.limits.cancellation_grace = Duration::from_millis(1);
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    runtime.register_runner(Arc::new(CancellationRunner {
        started: Notify::new(),
        observed_cancel: std::sync::atomic::AtomicBool::new(false),
    }));
    let owner = TaskOwner::new("chat-a");

    for _ in 0..105 {
        runtime
            .create(spec(owner.clone()), authorization("agent-a"))
            .await
            .unwrap();
    }

    let snapshot = runtime.activity_snapshot(&owner).await.unwrap();
    assert_eq!(snapshot.len(), 105);
    assert!(snapshot.iter().all(|record| !record.state.is_terminal()));
    assert_eq!(runtime.cancel_owner(&owner).await.unwrap(), 105);
}

#[tokio::test]
async fn rollback_cancellation_only_targets_tasks_at_or_after_branch_point() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.max_concurrent_tasks_per_chat = 2;
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    let runner = Arc::new(CancellationRunner {
        started: Notify::new(),
        observed_cancel: std::sync::atomic::AtomicBool::new(false),
    });
    runtime.register_runner(runner);
    let owner = TaskOwner::new("chat-a");

    let before = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let branch_point = chrono::Utc::now();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let after = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let cancelled = runtime
        .cancel_owner_at_or_after(&owner, Some(branch_point))
        .await
        .unwrap();
    assert_eq!(cancelled, 1);
    assert_eq!(
        runtime.get(&owner, &after).await.unwrap().state,
        TaskState::Cancelled
    );
    assert!(!runtime
        .get(&owner, &before)
        .await
        .unwrap()
        .state
        .is_terminal());

    runtime.cancel(&owner, &before).await.unwrap();
}

struct FailThenSucceedRunner {
    calls: AtomicUsize,
    authorization_agents: Mutex<Vec<String>>,
    provenance: Mutex<Vec<(TaskDeviceContext, rua_task::TaskOrigin)>>,
}

#[async_trait]
impl TaskRunner for FailThenSucceedRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        self.authorization_agents
            .lock()
            .await
            .push(request.authorization_agent_id.clone());
        self.provenance
            .lock()
            .await
            .push((request.device.clone(), request.authorization_origin.clone()));
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            let mut output = TaskRunOutput::failed(
                "Outcome\nFirst Attempt failed\n\nKey Results\nfixture failure\n\nVerification\nfailed\n\nRemaining Work\nRetry",
                "fixture failure",
            );
            output.raw_output = Some(b"{\"attempt\":1,\"ok\":false}".to_vec());
            output.artifacts.push(TaskArtifact {
                id: "attempt-1-report".to_string(),
                name: "attempt-1.txt".to_string(),
                media_type: "text/plain".to_string(),
                resource: TaskResourceRef {
                    uri: "runner://attempt-1-report".to_string(),
                    ref_type: "runner_attachment".to_string(),
                    source_device_id: None,
                },
                metadata: serde_json::json!({}),
            });
            output.artifact_payloads.push(TaskArtifactPayload {
                artifact_id: "attempt-1-report".to_string(),
                bytes: b"attempt one".to_vec(),
            });
            Ok(output)
        } else {
            let mut output = TaskRunOutput::completed(
                "Outcome\nRetry completed\n\nKey Results\nfixture success\n\nVerification\npassed\n\nRemaining Work\nNone",
            );
            output.raw_output = Some(
                serde_json::to_vec(&serde_json::json!({
                    "attempt": request.attempt_no,
                    "ok": true
                }))
                .unwrap(),
            );
            output.artifacts.push(TaskArtifact {
                id: "attempt-2-report".to_string(),
                name: "attempt-2.txt".to_string(),
                media_type: "text/plain".to_string(),
                resource: TaskResourceRef {
                    uri: "runner://attempt-2-report".to_string(),
                    ref_type: "runner_attachment".to_string(),
                    source_device_id: None,
                },
                metadata: serde_json::json!({}),
            });
            output.artifact_payloads.push(TaskArtifactPayload {
                artifact_id: "attempt-2-report".to_string(),
                bytes: b"attempt two".to_vec(),
            });
            Ok(output)
        }
    }
}

#[tokio::test]
async fn retry_preserves_attempt_history_and_immutable_specification() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.device = TaskDeviceContext {
        device_id: Some("device-original".into()),
        public_cwd: Some("/original/cwd".into()),
        ..Default::default()
    };
    let first_origin = rua_task::TaskOrigin {
        turn_id: Some("first-turn".into()),
        tool_call_id: Some("first-call".into()),
    };
    let retry_origin = rua_task::TaskOrigin {
        turn_id: Some("retry-turn".into()),
        tool_call_id: Some("retry-call".into()),
    };
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(config, sink.clone() as Arc<dyn TaskNotificationSink>).unwrap(),
    );
    let runner = Arc::new(FailThenSucceedRunner {
        calls: AtomicUsize::new(0),
        authorization_agents: Mutex::new(Vec::new()),
        provenance: Mutex::new(Vec::new()),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::from_address(rua_core::ChatAddress::new("chat-a"));
    let mut original_spec = spec(owner.clone());
    original_spec.origin = first_origin.clone();
    let task_id = runtime
        .create(
            original_spec.clone(),
            authorization("agent-a")
                .with_configuration_epoch("epoch-one".to_string())
                .with_origin(first_origin.clone()),
        )
        .await
        .unwrap();
    let failed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    assert_eq!(failed.attempts.len(), 1);
    assert_eq!(failed.spec.owner, owner);
    assert_eq!(failed.attempts[0].authorization_agent_id, "agent-a");

    let accepted_retry = runtime
        .retry(
            &owner,
            &task_id,
            authorization("agent-b")
                .with_configuration_epoch("epoch-two".to_string())
                .with_origin(retry_origin.clone()),
        )
        .await
        .unwrap();
    assert_eq!(accepted_retry.task_id, task_id);
    assert_eq!(accepted_retry.attempt_no, 2);
    assert_eq!(accepted_retry.spec, original_spec);
    let completed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(completed.attempts.len(), 2);
    assert_eq!(completed.attempts[0].state, TaskState::Failed);
    assert_eq!(completed.attempts[1].state, TaskState::Completed);
    assert_eq!(completed.spec.owner, owner);
    assert_eq!(completed.attempts[0].authorization_agent_id, "agent-a");
    assert_eq!(completed.attempts[1].authorization_agent_id, "agent-b");
    assert_eq!(
        completed.attempts[0].configuration_epoch.as_deref(),
        Some("epoch-one")
    );
    assert_eq!(
        completed.attempts[1].configuration_epoch.as_deref(),
        Some("epoch-two")
    );
    assert_eq!(
        runner.authorization_agents.lock().await.as_slice(),
        ["agent-a", "agent-b"]
    );
    assert_eq!(completed.attempts[0].authorization_origin, first_origin);
    assert_eq!(completed.attempts[1].authorization_origin, retry_origin);
    assert_eq!(completed.spec.origin, first_origin);
    let seen = runner.provenance.lock().await;
    assert_eq!(seen[0].0.device_id.as_deref(), Some("device-original"));
    assert_eq!(seen[0].0.public_cwd.as_deref(), Some("/original/cwd"));
    assert_eq!(seen[0].0, seen[1].0);
    assert_eq!(seen[1].1, retry_origin);
    drop(seen);
    let first_result = completed.attempts[0].result.as_ref().unwrap();
    assert_eq!(
        first_result.error.as_ref().unwrap().message,
        "fixture failure"
    );
    assert!(completed.attempts[0].started_at.is_some());
    assert!(completed.attempts[0].finished_at.is_some());
    assert_eq!(
        first_result.full_result_ref.as_ref().unwrap().uri,
        format!("task://{task_id}/attempts/1/raw-output.json")
    );
    assert_eq!(
        first_result.artifacts[0].resource.uri,
        format!("task://{task_id}/attempts/1/artifacts/0000-attempt-1.txt")
    );
    let second_result = completed.attempts[1].result.as_ref().unwrap();
    assert!(second_result.error.is_none());
    assert!(completed.attempts[1].started_at.is_some());
    assert!(completed.attempts[1].finished_at.is_some());
    assert_eq!(
        second_result.full_result_ref.as_ref().unwrap().uri,
        format!("task://{task_id}/attempts/2/raw-output.json")
    );
    assert_eq!(
        second_result.artifacts[0].resource.uri,
        format!("task://{task_id}/attempts/2/artifacts/0000-attempt-2.txt")
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 2);

    let raw = runtime.read_full_result(&owner, &task_id).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&raw).unwrap()["attempt"],
        2
    );
    assert!(runtime
        .retry(&owner, &task_id, authorization("agent-b"))
        .await
        .is_err());

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if sink.notifications.lock().await.len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let notifications = sink.notifications.lock().await;
    assert_eq!(notifications[0].record.attempt_no, 1);
    assert_eq!(notifications[1].record.attempt_no, 2);

    let reopened = TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap();
    let persisted = reopened.get(&owner, &task_id).await.unwrap();
    assert_eq!(persisted.attempts.len(), 2);
    assert_eq!(
        persisted.attempts[0].result.as_ref().unwrap().artifacts[0]
            .resource
            .uri,
        format!("task://{task_id}/attempts/1/artifacts/0000-attempt-1.txt")
    );
}

#[tokio::test]
async fn startup_reconciliation_interrupts_without_replaying() {
    let dir = tempfile::tempdir().unwrap();
    let first_runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(BlockingRunner::new());
    first_runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = first_runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    let sink = Arc::new(RecordingSink::default());
    let restarted = Arc::new(
        TaskRuntime::open_with_sink(
            TaskRuntimeConfig::new(dir.path()),
            sink.clone() as Arc<dyn TaskNotificationSink>,
        )
        .unwrap(),
    );
    let record = restarted.get(&owner, &task_id).await.unwrap();
    assert_eq!(record.state, TaskState::Interrupted);
    assert_eq!(record.attempts.len(), 1);
    assert_eq!(record.attempts[0].state, TaskState::Interrupted);
    assert_eq!(
        record
            .result
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .details["provenance"],
        "runtime_restart"
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if sink.notifications.lock().await.len() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(sink.notifications.lock().await[0].record.attempt_no, 1);

    let retry_runner = Arc::new(CancellationRunner {
        started: Notify::new(),
        observed_cancel: std::sync::atomic::AtomicBool::new(false),
    });
    restarted.register_runner(retry_runner.clone());
    let retry = restarted
        .retry(&owner, &task_id, authorization("agent-b"))
        .await
        .unwrap();
    assert_eq!(retry.attempt_no, 2);
    tokio::time::timeout(Duration::from_secs(2), retry_runner.started.notified())
        .await
        .unwrap();
    restarted.cancel(&owner, &task_id).await.unwrap();

    runner.release.notify_one();
}

#[tokio::test]
async fn attempt_history_limit_rejects_additional_retry() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.max_attempt_history = 1;
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    runtime.register_runner(Arc::new(FailingRunner));
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();

    let error = runtime
        .retry(&owner, &task_id, authorization("agent-b"))
        .await
        .unwrap_err();
    assert!(matches!(error, rua_task::TaskError::LimitExceeded(_)));
}

#[derive(Debug, Clone)]
struct ObservedRun {
    task_id: String,
    attempt_no: u32,
    instruction: String,
    dependencies: Vec<TaskDependencyResult>,
}

#[derive(Default)]
struct GraphRunner {
    runs: Mutex<Vec<ObservedRun>>,
    gates: Mutex<HashMap<String, Arc<Notify>>>,
}

impl GraphRunner {
    async fn gate(&self, name: &str) -> Arc<Notify> {
        let mut gates = self.gates.lock().await;
        gates
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    async fn release(&self, name: &str) {
        self.gate(name).await.notify_one();
    }

    async fn runs(&self) -> Vec<ObservedRun> {
        self.runs.lock().await.clone()
    }
}

#[async_trait]
impl TaskRunner for GraphRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        let instruction = request.spec.instruction.clone();
        let gate_name = instruction
            .strip_prefix("wait:")
            .or_else(|| instruction.strip_prefix("retry_success:"))
            .or_else(|| instruction.strip_prefix("retry_failure:"))
            .map(str::to_string);
        let gate = match (&gate_name, request.attempt_no) {
            (Some(name), 1) if instruction.starts_with("wait:") => Some(self.gate(name).await),
            (Some(name), 2) if instruction.starts_with("retry_") => Some(self.gate(name).await),
            _ => None,
        };
        self.runs.lock().await.push(ObservedRun {
            task_id: request.task_id.clone(),
            attempt_no: request.attempt_no,
            instruction: instruction.clone(),
            dependencies: request.dependencies.clone(),
        });

        if instruction == "cancel_then_success" && request.attempt_no == 1 {
            request.cancel.cancelled().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            return Ok(TaskRunOutput::cancelled());
        }
        if let Some(gate) = gate {
            gate.notified().await;
        }

        let should_fail = (instruction.starts_with("retry_success") && request.attempt_no == 1)
            || instruction.starts_with("retry_failure");
        if should_fail {
            return Ok(TaskRunOutput::failed(
                format!(
                    "Outcome\nAttempt {} failed\n\nKey Results\nfixture failure\n\nVerification\nfailed\n\nRemaining Work\nRetry",
                    request.attempt_no
                ),
                "fixture failure",
            ));
        }

        let mut output = TaskRunOutput::completed(format!(
            "Outcome\nCompleted\n\nKey Results\n{}\n\nVerification\npassed\n\nRemaining Work\nNone",
            instruction
        ));
        output.raw_output = Some(
            serde_json::to_vec(&serde_json::json!({
                "raw_only": format!("raw:{instruction}"),
                "attempt_no": request.attempt_no,
            }))
            .unwrap(),
        );
        output.artifacts.push(TaskArtifact {
            id: format!("{}:artifact", request.task_id),
            name: format!("{instruction}.txt"),
            media_type: "text/plain".to_string(),
            resource: TaskResourceRef {
                uri: format!("task://{}/artifact", request.task_id),
                ref_type: "fixture".to_string(),
                source_device_id: request.device.device_id.clone(),
            },
            metadata: serde_json::json!({"fixture": true}),
        });
        Ok(output)
    }
}

async fn wait_for_run_count(runner: &GraphRunner, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if runner.runs.lock().await.len() >= expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_task_state(
    runtime: &TaskRuntime,
    owner: &TaskOwner,
    task_id: &str,
    expected: TaskState,
) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if runtime.get(owner, task_id).await.unwrap().state == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn dependency_scope_count_and_depth_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.max_dependencies_per_task = 1;
    config.limits.max_dependency_depth = 1;
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    runtime.register_runner(Arc::new(GraphRunner::default()));
    let owner = TaskOwner::new("chat-a");

    let root_a = runtime
        .create(
            graph_spec(owner.clone(), "root-a", Vec::new()),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    let root_b = runtime
        .create(
            graph_spec(owner.clone(), "root-b", Vec::new()),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    runtime
        .await_terminal(&owner, &root_a, Duration::from_secs(2))
        .await
        .unwrap();
    runtime
        .await_terminal(&owner, &root_b, Duration::from_secs(2))
        .await
        .unwrap();

    let error = runtime
        .create(
            graph_spec(
                TaskOwner::new("chat-b"),
                "another-chat",
                vec![root_a.clone()],
            ),
            authorization("agent-a"),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, rua_task::TaskError::NotFound(_)));

    let error = runtime
        .create(
            graph_spec(owner.clone(), "too-many", vec![root_a.clone(), root_b]),
            authorization("agent-a"),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, rua_task::TaskError::LimitExceeded(_)));

    let child = runtime
        .create(
            graph_spec(owner.clone(), "child", vec![root_a]),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    runtime
        .await_terminal(&owner, &child, Duration::from_secs(2))
        .await
        .unwrap();
    let error = runtime
        .create(
            graph_spec(owner, "grandchild", vec![child]),
            authorization("agent-a"),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, rua_task::TaskError::LimitExceeded(_)));
}

#[tokio::test]
async fn fan_in_fan_out_wait_without_permits_and_inject_bounded_results_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.device.device_id = Some("device-a".to_string());
    config.limits.max_concurrent_tasks_per_chat = 3;
    config.limits.max_concurrent_tasks_per_runner = 3;
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    let runner = Arc::new(GraphRunner::default());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");

    let upstream_a = runtime
        .create(
            graph_spec(owner.clone(), "wait:a", Vec::new()),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    let upstream_b = runtime
        .create(
            graph_spec(owner.clone(), "wait:b", Vec::new()),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    wait_for_run_count(&runner, 2).await;

    let fan_in = runtime
        .create(
            graph_spec(
                owner.clone(),
                "fan-in",
                vec![upstream_a.clone(), upstream_b.clone()],
            ),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    let fan_out_a = runtime
        .create(
            graph_spec(owner.clone(), "fan-out-a", vec![upstream_a.clone()]),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    let fan_out_b = runtime
        .create(
            graph_spec(owner.clone(), "fan-out-b", vec![upstream_a.clone()]),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    let waiting = runtime.get(&owner, &fan_in).await.unwrap();
    assert_eq!(waiting.state, TaskState::Waiting);
    assert_eq!(waiting.waiting_on.len(), 2);
    assert_eq!(runner.runs().await.len(), 2);

    let unrelated = runtime
        .create(
            graph_spec(owner.clone(), "unrelated", Vec::new()),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    runtime
        .await_terminal(&owner, &unrelated, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(runner.runs().await.len(), 3);

    runner.release("a").await;
    for task_id in [&fan_out_a, &fan_out_b] {
        let record = runtime
            .await_terminal(&owner, task_id, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(record.state, TaskState::Completed);
    }
    let still_waiting = runtime.get(&owner, &fan_in).await.unwrap();
    assert_eq!(still_waiting.state, TaskState::Waiting);
    assert_eq!(still_waiting.waiting_on, vec![upstream_b.clone()]);

    runner.release("b").await;
    let completed = runtime
        .await_terminal(&owner, &fan_in, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(completed.state, TaskState::Completed);

    let runs = runner.runs().await;
    for task_id in [&fan_in, &fan_out_a, &fan_out_b] {
        assert_eq!(runs.iter().filter(|run| &run.task_id == task_id).count(), 1);
    }
    let fan_in_run = runs.iter().find(|run| run.task_id == fan_in).unwrap();
    assert_eq!(fan_in_run.dependencies.len(), 2);
    assert_eq!(fan_in_run.attempt_no, 1);
    assert_eq!(fan_in_run.instruction, "fan-in");
    for dependency in &fan_in_run.dependencies {
        assert_eq!(dependency.attempt_no, 1);
        assert_eq!(dependency.status, TaskState::Completed);
        assert_eq!(dependency.device.device_id.as_deref(), Some("device-a"));
        assert!(dependency.full_result_ref.is_some());
        assert_eq!(dependency.artifacts.len(), 1);
        let serialized = serde_json::to_string(dependency).unwrap();
        assert!(!serialized.contains("raw_only"));
    }
}

#[tokio::test]
async fn retry_reactivates_blocked_dependents_and_second_failure_blocks_again() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(GraphRunner::default());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");

    let recovery_upstream = runtime
        .create(
            graph_spec(owner.clone(), "retry_success:recover", Vec::new()),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    runtime
        .await_terminal(&owner, &recovery_upstream, Duration::from_secs(2))
        .await
        .unwrap();
    let recovery_downstream = runtime
        .create(
            graph_spec(
                owner.clone(),
                "recovered-downstream",
                vec![recovery_upstream.clone()],
            ),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    let blocked = runtime.get(&owner, &recovery_downstream).await.unwrap();
    assert_eq!(blocked.state, TaskState::Blocked);
    assert_eq!(blocked.blocked_by, vec![recovery_upstream.clone()]);

    runtime
        .retry(&owner, &recovery_upstream, authorization("agent-b"))
        .await
        .unwrap();
    let waiting = runtime.get(&owner, &recovery_downstream).await.unwrap();
    assert_eq!(waiting.state, TaskState::Waiting);
    assert_eq!(waiting.waiting_on, vec![recovery_upstream.clone()]);
    runner.release("recover").await;
    let recovered = runtime
        .await_terminal(&owner, &recovery_downstream, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(recovered.state, TaskState::Completed);

    let failure_upstream = runtime
        .create(
            graph_spec(owner.clone(), "retry_failure:fail-again", Vec::new()),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    runtime
        .await_terminal(&owner, &failure_upstream, Duration::from_secs(2))
        .await
        .unwrap();
    let failure_downstream = runtime
        .create(
            graph_spec(
                owner.clone(),
                "still-blocked",
                vec![failure_upstream.clone()],
            ),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    assert_eq!(
        runtime
            .get(&owner, &failure_downstream)
            .await
            .unwrap()
            .state,
        TaskState::Blocked
    );

    runtime
        .retry(&owner, &failure_upstream, authorization("agent-b"))
        .await
        .unwrap();
    assert_eq!(
        runtime
            .get(&owner, &failure_downstream)
            .await
            .unwrap()
            .state,
        TaskState::Waiting
    );
    runner.release("fail-again").await;
    runtime
        .await_terminal(&owner, &failure_upstream, Duration::from_secs(2))
        .await
        .unwrap();
    wait_for_task_state(
        runtime.as_ref(),
        &owner,
        &failure_downstream,
        TaskState::Blocked,
    )
    .await;
    let blocked_again = runtime.get(&owner, &failure_downstream).await.unwrap();
    assert_eq!(blocked_again.state, TaskState::Blocked);
    assert_eq!(blocked_again.blocked_by, vec![failure_upstream]);
}

#[tokio::test]
async fn cancelled_and_interrupted_dependencies_are_reported_as_blockers() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(GraphRunner::default());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");

    let cancelled_upstream = runtime
        .create(
            graph_spec(owner.clone(), "cancel_then_success", Vec::new()),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    wait_for_run_count(&runner, 1).await;
    runtime.cancel(&owner, &cancelled_upstream).await.unwrap();
    let cancelled_downstream = runtime
        .create(
            graph_spec(
                owner.clone(),
                "after-cancel",
                vec![cancelled_upstream.clone()],
            ),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    let blocked = runtime.get(&owner, &cancelled_downstream).await.unwrap();
    assert_eq!(blocked.state, TaskState::Blocked);
    assert_eq!(blocked.blocked_by, vec![cancelled_upstream.clone()]);

    runtime
        .retry(&owner, &cancelled_upstream, authorization("agent-b"))
        .await
        .unwrap();
    let recovered = runtime
        .await_terminal(&owner, &cancelled_downstream, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(recovered.state, TaskState::Completed);

    let old_runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let old_runner = Arc::new(BlockingRunner::new());
    old_runtime.register_runner(old_runner.clone());
    let interrupted_upstream = old_runtime
        .create(
            graph_spec(owner.clone(), "interrupted-upstream", Vec::new()),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), old_runner.started.notified())
        .await
        .unwrap();

    let restarted = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    restarted.register_runner(Arc::new(GraphRunner::default()));
    let interrupted_downstream = restarted
        .create(
            graph_spec(
                owner.clone(),
                "after-interruption",
                vec![interrupted_upstream.clone()],
            ),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    let blocked = restarted
        .get(&owner, &interrupted_downstream)
        .await
        .unwrap();
    assert_eq!(blocked.state, TaskState::Blocked);
    assert_eq!(blocked.blocked_by, vec![interrupted_upstream]);

    old_runner.release.notify_one();
}

struct RedispatchProbeRunner {
    calls: AtomicUsize,
}

#[async_trait]
impl TaskRunner for RedispatchProbeRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }

    async fn run(&self, _request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(TaskRunOutput::completed("unexpected redispatch"))
    }
}

#[tokio::test]
async fn adopted_execution_preserves_origin_and_completes_once_without_redispatch() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(
            TaskRuntimeConfig::new(dir.path()),
            sink.clone() as Arc<dyn TaskNotificationSink>,
        )
        .unwrap(),
    );
    let runner = Arc::new(RedispatchProbeRunner {
        calls: AtomicUsize::new(0),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = uuid::Uuid::new_v4().to_string();
    let executions = Arc::new(AtomicUsize::new(0));
    let executions_for_future = executions.clone();
    let mut adopted_spec = graph_spec(owner.clone(), "adopt running tool", Vec::new());
    adopted_spec.origin.turn_id = Some("turn-a".to_string());
    adopted_spec.origin.tool_call_id = Some("tool-call-a".to_string());

    let adopted_id = runtime
        .adopt_running(
            task_id.clone(),
            adopted_spec,
            authorization("agent-a"),
            TaskAdoptedExecution::new(
                async move {
                    executions_for_future.fetch_add(1, Ordering::SeqCst);
                    TaskRunOutput::completed(
                        "Outcome\nCompleted\n\nKey Results\nadopted\n\nVerification\npassed\n\nRemaining Work\nNone",
                    )
                },
                tokio_util::sync::CancellationToken::new(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(adopted_id, task_id);

    let completed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(completed.attempts.len(), 1);
    assert_eq!(completed.spec.origin.turn_id.as_deref(), Some("turn-a"));
    assert_eq!(
        completed.spec.origin.tool_call_id.as_deref(),
        Some("tool-call-a")
    );
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if sink.notifications.lock().await.len() == 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(sink.notifications.lock().await[0].record.attempt_no, 1);
}

#[tokio::test]
async fn adopted_task_cancellation_reaches_original_execution_handle() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.cancellation_grace = Duration::from_secs(1);
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    let runner = Arc::new(RedispatchProbeRunner {
        calls: AtomicUsize::new(0),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = uuid::Uuid::new_v4().to_string();
    let execution_cancel = tokio_util::sync::CancellationToken::new();
    let execution_cancel_for_future = execution_cancel.clone();
    let observed_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed_cancel_for_future = observed_cancel.clone();

    runtime
        .adopt_running(
            task_id.clone(),
            graph_spec(owner.clone(), "cancel adopted tool", Vec::new()),
            authorization("agent-a"),
            TaskAdoptedExecution::new(
                async move {
                    execution_cancel_for_future.cancelled().await;
                    observed_cancel_for_future.store(true, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    {
                        let mut output = TaskRunOutput::cancelled();
                        output.output = "cleanup complete".into();
                        output
                    }
                },
                execution_cancel,
            ),
        )
        .await
        .unwrap();

    let cancelled = runtime.cancel(&owner, &task_id).await.unwrap();
    assert_eq!(cancelled.state, TaskState::Cancelled);
    assert!(observed_cancel.load(Ordering::SeqCst));
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stable_task_id_acceptance_never_dispatches_the_same_specification_twice() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(RedispatchProbeRunner {
        calls: AtomicUsize::new(0),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("stable-acceptance");
    let task_id = "chat-command-stable-original".to_string();
    let original = spec(owner.clone());
    runtime
        .create_with_id(task_id.clone(), original.clone(), authorization("agent-a"))
        .await
        .unwrap();
    runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(
        runtime
            .create_with_id(task_id.clone(), original.clone(), authorization("agent-a"))
            .await
            .unwrap(),
        task_id
    );
    let mut different = original;
    different.instruction.push_str(" changed");
    assert!(matches!(
        runtime
            .create_with_id(task_id.clone(), different, authorization("agent-a"))
            .await,
        Err(TaskError::Invalid(_))
    ));
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        runtime
            .create_with_id("../outside".into(), spec(owner), authorization("agent-a"))
            .await,
        Err(TaskError::Invalid(_))
    ));
}

#[tokio::test]
async fn adoption_holds_normal_execution_capacity_until_original_completion() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.max_concurrent_tasks_per_chat = 1;
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    let runner = Arc::new(RedispatchProbeRunner {
        calls: AtomicUsize::new(0),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("capacity");
    let (complete, completion) = tokio::sync::oneshot::channel();
    let id = "chat-command-accepted".to_owned();
    runtime
        .adopt_running(
            id.clone(),
            spec(owner.clone()),
            authorization("agent-a"),
            TaskAdoptedExecution::new(
                async { completion.await.unwrap() },
                tokio_util::sync::CancellationToken::new(),
            ),
        )
        .await
        .unwrap();
    let queued = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .adopt_running(
                "chat-command-excess".into(),
                spec(owner.clone()),
                authorization("agent-a"),
                TaskAdoptedExecution::new(
                    async { TaskRunOutput::completed("must not run") },
                    tokio_util::sync::CancellationToken::new()
                )
            )
            .await,
        Err(TaskError::LimitExceeded(_))
    ));
    assert_eq!(
        runtime.get(&owner, &id).await.unwrap().state,
        TaskState::Running
    );
    assert!(matches!(
        runtime.get(&owner, "chat-command-excess").await,
        Err(TaskError::NotFound(_))
    ));
    assert_eq!(runner.calls.load(Ordering::SeqCst), 0);
    complete
        .send(TaskRunOutput::completed("original complete"))
        .unwrap();
    runtime
        .await_terminal(&owner, &queued, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn adopted_cancellation_waits_for_real_completion_past_control_grace() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.limits.cancellation_grace = Duration::from_millis(10);
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    runtime.register_runner(Arc::new(RedispatchProbeRunner {
        calls: AtomicUsize::new(0),
    }));
    let owner = TaskOwner::new("real-terminal");
    let task_id = "same-running-execution".to_owned();
    let cancel = tokio_util::sync::CancellationToken::new();
    let (finish, completed) = tokio::sync::oneshot::channel();
    runtime
        .adopt_running(
            task_id.clone(),
            graph_spec(owner.clone(), "terminal evidence", Vec::new()),
            authorization("agent-a"),
            TaskAdoptedExecution::new(async move { completed.await.unwrap() }, cancel.clone()),
        )
        .await
        .unwrap();
    let control = tokio::spawn({
        let runtime = runtime.clone();
        let owner = owner.clone();
        let task_id = task_id.clone();
        async move { runtime.cancel(&owner, &task_id).await }
    });
    cancel.cancelled().await;
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        runtime.get(&owner, &task_id).await.unwrap().state,
        TaskState::Running,
        "a stop request and expired wait are not terminal evidence"
    );
    finish
        .send(TaskRunOutput::completed(
            "completed before stop took effect",
        ))
        .unwrap();
    let record = control.await.unwrap().unwrap();
    assert_eq!(record.state, TaskState::Completed);
    assert_eq!(
        record.result.unwrap().output,
        "completed before stop took effect"
    );
}

#[tokio::test]
async fn authority_snapshot_is_per_attempt_and_stale_completion_is_suppressed() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let authority = Arc::new(MutableAuthority::new(TaskDeviceContext {
        device_id: Some("device-a".to_string()),
        public_cwd: Some("/original".into()),
        device_name: Some("Desktop A".to_string()),
        host_profile: Some("desktop".to_string()),
        authority_epoch: Some("epoch-1".to_string()),
    }));
    let runtime = Arc::new(
        TaskRuntime::open_with_sink_and_authority(
            TaskRuntimeConfig::new(dir.path()),
            sink.clone() as Arc<dyn TaskNotificationSink>,
            authority.clone() as Arc<dyn TaskAuthorityResolver>,
        )
        .unwrap(),
    );
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");

    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();
    let first = runtime.get(&owner, &task_id).await.unwrap();
    assert_eq!(
        first.attempts[0].device.authority_epoch.as_deref(),
        Some("epoch-1")
    );
    assert_eq!(
        first.attempts[0].device.device_name.as_deref(),
        Some("Desktop A")
    );

    authority
        .set(TaskDeviceContext {
            device_id: Some("device-a".to_string()),
            public_cwd: Some("/original".into()),
            device_name: Some("Desktop A renamed".to_string()),
            host_profile: Some("desktop".to_string()),
            authority_epoch: Some("epoch-2".to_string()),
        })
        .await;
    runner.release.notify_one();
    let interrupted = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(interrupted.state, TaskState::Interrupted);
    assert!(interrupted
        .result
        .as_ref()
        .unwrap()
        .output
        .contains("stale conversation authority Epoch"));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(sink.notifications.lock().await.is_empty());

    runtime
        .retry(&owner, &task_id, authorization("agent-b"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runner.calls.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    runner.release.notify_one();
    let completed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(completed.attempts.len(), 2);
    assert_eq!(
        completed.attempts[0].device.authority_epoch.as_deref(),
        Some("epoch-1")
    );
    assert_eq!(
        completed.attempts[1].device.authority_epoch.as_deref(),
        Some("epoch-2")
    );
    assert_eq!(
        completed.result.as_ref().unwrap().device.authority_epoch,
        Some("epoch-2".to_string())
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while sink.notifications.lock().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(sink.notifications.lock().await.len(), 1);
}

#[tokio::test]
async fn non_authority_rejection_happens_before_task_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        TaskRuntime::open_with_sink_and_authority(
            TaskRuntimeConfig::new(dir.path()),
            Arc::new(RecordingSink::default()),
            Arc::new(RejectingAuthority),
        )
        .unwrap(),
    );
    runtime.register_runner(Arc::new(BlockingRunner::new()));
    let owner = TaskOwner::new("chat-a");

    let error = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap_err();
    assert!(matches!(error, TaskError::AuthorityRejected(_)));
    assert!(runtime
        .list(&owner, TaskListFilter::default())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn full_result_is_unavailable_when_the_source_device_is_not_local() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = TaskRuntimeConfig::new(dir.path());
    config.device.device_id = Some("device-a".to_string());
    let runtime = Arc::new(TaskRuntime::open(config).unwrap());
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();
    runner.release.notify_one();
    runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    drop(runtime);

    let mut remote_config = TaskRuntimeConfig::new(dir.path());
    remote_config.device.device_id = Some("device-b".to_string());
    let remote = TaskRuntime::open(remote_config).unwrap();
    let error = remote.read_full_result(&owner, &task_id).await.unwrap_err();
    assert!(matches!(error, TaskError::ResourceUnavailable(_)));
}

async fn wait_for_sink_notifications(sink: &RecordingSink, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if sink.notifications.lock().await.len() == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_pending_deliveries(runtime: &TaskRuntime, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if runtime
                .pending_terminal_deliveries(None)
                .await
                .unwrap()
                .len()
                == expected
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn pending_terminal_delivery_survives_reopen_and_sink_success_does_not_ack() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let runtime = Arc::new(
        TaskRuntime::open_with_sink(
            TaskRuntimeConfig::new(dir.path()),
            sink.clone() as Arc<dyn TaskNotificationSink>,
        )
        .unwrap(),
    );
    let runner = Arc::new(BlockingRunner::new());
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), runner.started.notified())
        .await
        .unwrap();

    // A terminal Task that has not finished yet is not an outstanding delivery.
    assert!(runtime
        .pending_terminal_deliveries(Some(&owner))
        .await
        .unwrap()
        .is_empty());

    runner.release.notify_one();
    let completed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(completed.state, TaskState::Completed);

    // A successful sink delivery does not mean the source consumed the result.
    wait_for_sink_notifications(&sink, 1).await;
    let pending = runtime
        .pending_terminal_deliveries(Some(&owner))
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].task_id, task_id);
    assert_eq!(pending[0].attempt_no, 1);
    assert_eq!(pending[0].state, TaskState::Completed);
    assert_eq!(pending[0].result, completed.result);

    // The outstanding delivery survives a process restart, and startup enumerates it.
    drop(runtime);
    let reopened = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let all = reopened.pending_terminal_deliveries(None).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].task_id, task_id);
    assert_eq!(all[0].attempt_no, 1);
    assert_eq!(all[0].result, completed.result);

    reopened
        .fail_terminal_delivery(&owner, &task_id, 1, "offline".into())
        .await
        .unwrap();
    assert_eq!(
        reopened
            .terminal_delivery_error(&owner, &task_id, 1)
            .await
            .unwrap()
            .as_deref(),
        Some("offline")
    );
    assert!(reopened
        .ack_terminal_delivery(&owner, &task_id, 1)
        .await
        .unwrap());
    assert!(reopened
        .terminal_delivery_error(&owner, &task_id, 1)
        .await
        .unwrap()
        .is_none());
    assert!(reopened
        .pending_terminal_deliveries(None)
        .await
        .unwrap()
        .is_empty());

    // The acknowledgement is durable: the delivery stays cleared after a further reopen.
    drop(reopened);
    let final_runtime = TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap();
    assert!(final_runtime
        .pending_terminal_deliveries(None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn pending_terminal_deliveries_are_owner_scoped_and_ack_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(FailingRunner);
    runtime.register_runner(runner);
    let owner_a = TaskOwner::new("chat-a");
    let owner_b = TaskOwner::new("chat-b");
    let task_a = runtime
        .create(spec(owner_a.clone()), authorization("agent-a"))
        .await
        .unwrap();
    let task_b = runtime
        .create(spec(owner_b.clone()), authorization("agent-b"))
        .await
        .unwrap();
    wait_for_pending_deliveries(&runtime, 2).await;

    let scoped_a = runtime
        .pending_terminal_deliveries(Some(&owner_a))
        .await
        .unwrap();
    assert_eq!(scoped_a.len(), 1);
    assert_eq!(scoped_a[0].task_id, task_a);
    assert_eq!(scoped_a[0].state, TaskState::Failed);
    let scoped_b = runtime
        .pending_terminal_deliveries(Some(&owner_b))
        .await
        .unwrap();
    assert_eq!(scoped_b.len(), 1);
    assert_eq!(scoped_b[0].task_id, task_b);

    // An owner cannot acknowledge another Chat's delivery.
    assert!(!runtime
        .ack_terminal_delivery(&owner_b, &task_a, 1)
        .await
        .unwrap());
    assert_eq!(
        runtime
            .pending_terminal_deliveries(Some(&owner_a))
            .await
            .unwrap()
            .len(),
        1
    );

    // A wrong Attempt number is not an acknowledgement of the queued Attempt.
    assert!(!runtime
        .ack_terminal_delivery(&owner_a, &task_a, 2)
        .await
        .unwrap());
    assert_eq!(
        runtime
            .pending_terminal_deliveries(Some(&owner_a))
            .await
            .unwrap()
            .len(),
        1
    );

    // The exact Task, Attempt, and owner acknowledges once; repeats are idempotent no-ops.
    assert!(runtime
        .ack_terminal_delivery(&owner_a, &task_a, 1)
        .await
        .unwrap());
    assert!(!runtime
        .ack_terminal_delivery(&owner_a, &task_a, 1)
        .await
        .unwrap());
    let remaining = runtime.pending_terminal_deliveries(None).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].task_id, task_b);
}

#[tokio::test]
async fn retry_keeps_outstanding_delivery_for_the_previous_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(FailThenSucceedRunner {
        calls: AtomicUsize::new(0),
        authorization_agents: Mutex::new(Vec::new()),
        provenance: Mutex::new(Vec::new()),
    });
    runtime.register_runner(runner.clone());
    let owner = TaskOwner::new("chat-a");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    let failed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(failed.state, TaskState::Failed);
    wait_for_pending_deliveries(&runtime, 1).await;

    let retried = runtime
        .retry(&owner, &task_id, authorization("agent-b"))
        .await
        .unwrap();
    assert_eq!(retried.attempt_no, 2);
    let completed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(completed.state, TaskState::Completed);
    wait_for_pending_deliveries(&runtime, 2).await;

    // The unacknowledged first Attempt is not lost when the Task advances to a new Attempt,
    // and each delivery still reports its own Attempt state and result.
    let pending = runtime
        .pending_terminal_deliveries(Some(&owner))
        .await
        .unwrap();
    assert_eq!(pending.len(), 2);
    let first = pending
        .iter()
        .find(|record| record.attempt_no == 1)
        .expect("first Attempt delivery");
    let second = pending
        .iter()
        .find(|record| record.attempt_no == 2)
        .expect("second Attempt delivery");
    assert_eq!(first.task_id, task_id);
    assert_eq!(first.state, TaskState::Failed);
    assert_eq!(
        first
            .result
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .message,
        "fixture failure"
    );
    assert_eq!(second.task_id, task_id);
    assert_eq!(second.state, TaskState::Completed);
    assert!(second.result.as_ref().unwrap().error.is_none());

    // Acknowledging one Attempt leaves the other outstanding.
    assert!(runtime
        .ack_terminal_delivery(&owner, &task_id, 1)
        .await
        .unwrap());
    let remaining = runtime
        .pending_terminal_deliveries(Some(&owner))
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].attempt_no, 2);
    assert_eq!(runner.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn owner_removal_clears_outstanding_terminal_deliveries() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    runtime.register_runner(Arc::new(FailingRunner));
    let owner = TaskOwner::new("chat-a");
    let other = TaskOwner::new("chat-b");
    let task_id = runtime
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    let other_task = runtime
        .create(spec(other.clone()), authorization("agent-b"))
        .await
        .unwrap();
    wait_for_pending_deliveries(&runtime, 2).await;

    runtime.remove_owner(&owner).await.unwrap();

    let remaining = runtime.pending_terminal_deliveries(None).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].task_id, other_task);
    assert!(runtime
        .pending_terminal_deliveries(Some(&owner))
        .await
        .unwrap()
        .is_empty());
    // Nothing remains to acknowledge for the removed owner.
    assert!(!runtime
        .ack_terminal_delivery(&owner, &task_id, 1)
        .await
        .unwrap());
}

#[tokio::test]
async fn reconcile_cancellation_and_shutdown_queue_pending_terminal_deliveries() {
    let dir = tempfile::tempdir().unwrap();
    let first = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let blocking = Arc::new(BlockingRunner::new());
    first.register_runner(blocking.clone());
    let owner = TaskOwner::new("chat-a");
    let interrupted_on_restart = first
        .create(spec(owner.clone()), authorization("agent-a"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), blocking.started.notified())
        .await
        .unwrap();

    // Startup reconciliation persists a managed Interrupted result, so it is outstanding too.
    let mut second_config = TaskRuntimeConfig::new(dir.path());
    second_config.limits.cancellation_grace = Duration::from_millis(50);
    let second = Arc::new(TaskRuntime::open(second_config).unwrap());
    let reconciled = second
        .pending_terminal_deliveries(Some(&owner))
        .await
        .unwrap();
    assert_eq!(reconciled.len(), 1);
    assert_eq!(reconciled[0].task_id, interrupted_on_restart);
    assert_eq!(reconciled[0].attempt_no, 1);
    assert_eq!(reconciled[0].state, TaskState::Interrupted);
    assert_eq!(
        reconciled[0]
            .result
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .details["provenance"],
        "runtime_restart"
    );

    // Cancellation of a running Task queues its delivery.
    second.register_runner(blocking.clone());
    let cancelled = second
        .create(
            graph_spec(owner.clone(), "cancelled while running", Vec::new()),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while second.get(&owner, &cancelled).await.unwrap().state != TaskState::Running {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let cancelled_record = second.cancel(&owner, &cancelled).await.unwrap();
    assert_eq!(cancelled_record.state, TaskState::Cancelled);

    // Shutdown interruption of a waiting Task queues its delivery as well.
    let dependent = second
        .create(
            graph_spec(
                owner.clone(),
                "waiting dependency interrupted by shutdown",
                vec![interrupted_on_restart.clone()],
            ),
            authorization("agent-a"),
        )
        .await
        .unwrap();
    assert_eq!(
        second.get(&owner, &dependent).await.unwrap().state,
        TaskState::Blocked
    );
    second.shutdown().await.unwrap();
    assert_eq!(
        second.get(&owner, &dependent).await.unwrap().state,
        TaskState::Interrupted
    );

    let pending = second
        .pending_terminal_deliveries(Some(&owner))
        .await
        .unwrap();
    assert_eq!(pending.len(), 3);
    let queued = pending
        .iter()
        .map(|record| (record.task_id.clone(), record.state))
        .collect::<Vec<_>>();
    assert!(queued.contains(&(interrupted_on_restart.clone(), TaskState::Interrupted)));
    assert!(queued.contains(&(cancelled.clone(), TaskState::Cancelled)));
    assert!(queued.contains(&(dependent.clone(), TaskState::Interrupted)));

    // Deliveries stay outstanding across a restart that has nothing left to reconcile.
    drop(second);
    let third = TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap();
    assert_eq!(
        third.pending_terminal_deliveries(None).await.unwrap().len(),
        3
    );

    blocking.release.notify_one();
}
