use std::sync::Arc;
use std::time::Duration;

use agent_types::{
    AgentEvent, FinishReason, ModelDialogueItem, Role, ToolAnnotations, ToolCall, ToolDefinition,
    ToolResult,
};
use async_trait::async_trait;
use react_agent::hooks::NoopHook;
use react_agent::react_loop::execute_single_tool;
use react_agent::test_support::MockChatModel;
use react_agent::{AgentToolkit, ContractToolExecutor, ReactAgent, ToolExecutor};
use rua_tool::extended::task_tools::TaskRuntimeHandle;
use rua_tool::{ToolTaskExecution, ToolTaskExecutionRequest, ToolTaskRunner};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

struct BlockingTool {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    executions: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl ToolExecutor for BlockingTool {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        self.executions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(started) = self.started.lock().await.take() {
            let _ = started.send(());
        }
        if let Some(release) = self.release.lock().await.take() {
            let _ = release.await;
        }
        let mut result = ToolResult::text(call.id.clone(), "background-complete");
        result.name = Some(call.name.clone());
        Ok(result)
    }
}

struct RedispatchProbeExecution {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl ToolTaskExecution for RedispatchProbeExecution {
    async fn execute(&self, _request: ToolTaskExecutionRequest) -> Result<ToolResult, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("adoption must not redispatch the tool".to_string())
    }
}

fn runtime_with_probe(
    data_dir: &std::path::Path,
    redispatches: Arc<std::sync::atomic::AtomicUsize>,
) -> Arc<rua_task::TaskRuntime> {
    let runtime =
        Arc::new(rua_task::TaskRuntime::open(rua_task::TaskRuntimeConfig::new(data_dir)).unwrap());
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        RedispatchProbeExecution {
            calls: redispatches,
        },
    ))));
    runtime
}

fn blocking_tool(
    started: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
    executions: Arc<std::sync::atomic::AtomicUsize>,
) -> (AgentToolkit, ToolCall) {
    blocking_tool_with_wait(started, release, executions, 30)
}

fn blocking_tool_with_wait(
    started: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
    executions: Arc<std::sync::atomic::AtomicUsize>,
    default_wait_seconds: u64,
) -> (AgentToolkit, ToolCall) {
    let mut toolkit = AgentToolkit::new();
    let mut definition = ToolDefinition {
        name: "slow_tool".to_string(),
        description: "blocks until released".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "yield_seconds": {
                    "type": "integer",
                    "default": default_wait_seconds
                }
            }
        }),
        strict: None,
        annotations: ToolAnnotations::default(),
        metadata: serde_json::Map::new(),
    };
    definition.metadata.insert(
        rua_tool::TOOL_TRANSFER_CONTRACT_METADATA_KEY.to_string(),
        serde_json::to_value(rua_tool::ToolTransferContract::transferable_seconds(
            "yield_seconds",
            default_wait_seconds,
        ))
        .unwrap(),
    );
    toolkit.register(
        definition,
        Arc::new(BlockingTool {
            started: Mutex::new(Some(started)),
            release: Mutex::new(Some(release)),
            executions,
        }),
    );
    (
        toolkit,
        ToolCall::new(
            format!("tool-call-{}", uuid::Uuid::new_v4()),
            "slow_tool",
            serde_json::json!({"fixture": true}),
        ),
    )
}

fn interjection_model() -> MockChatModel {
    MockChatModel::from_sequences(vec![
        vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::ToolCallDelta {
                id: "tool-call-1".into(),
                index: 0,
                name: Some("slow_tool".into()),
                arguments_delta: serde_json::json!({}).to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::ToolUse,
                usage: None,
                metadata: serde_json::Value::Null,
            },
        ],
        vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: "should not continue".into(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            },
        ],
    ])
}

fn shell_transfer_model(command: &str, yield_seconds: u64) -> MockChatModel {
    MockChatModel::from_sequences(vec![
        vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::ToolCallDelta {
                id: "shell-call-1".into(),
                index: 0,
                name: Some("bash".into()),
                arguments_delta: serde_json::json!({
                    "command": command,
                    "yield_seconds": yield_seconds,
                })
                .to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::ToolUse,
                usage: None,
                metadata: serde_json::Value::Null,
            },
        ],
        vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: "Task transferred".into(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            },
        ],
    ])
}

#[tokio::test]
async fn active_interjection_adopts_running_tool_into_process_task_runtime() {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let redispatches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let data_dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    let runtime = runtime_with_probe(data_dir.path(), redispatches.clone());
    let (toolkit, _) = blocking_tool(started_tx, release_rx, executions.clone());
    let session_id = format!("session-{}", uuid::Uuid::new_v4());
    let tool_ctx = rua_core::tool::ToolContext {
        agent_id: "agent-a".to_string(),
        session_id: session_id.clone(),
        working_dir: working_dir.path().to_path_buf(),
        task_runtime: Some(Arc::new(TaskRuntimeHandle::new(runtime.clone()))),
        ..Default::default()
    };
    let (control_tx, control_rx) = tokio::sync::watch::channel(rua_core::RunControl::Running);
    let agent = ReactAgent::builder(Arc::new(interjection_model()))
        .toolkit(Arc::new(toolkit))
        .tool_ctx(tool_ctx)
        .run_control(Some(control_rx))
        .build();

    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "run slow")]);
    let collect = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = futures::StreamExt::next(&mut stream).await {
            let terminal = event.is_terminal();
            events.push(event);
            if terminal {
                break;
            }
        }
        events
    });

    started_rx.await.unwrap();
    control_tx.send(rua_core::RunControl::Interject).unwrap();
    let events = tokio::time::timeout(Duration::from_secs(5), collect)
        .await
        .expect("interjected run should detach from the running tool")
        .unwrap();
    let task_id = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result } => result
                .metadata
                .get("background_task_id")
                .and_then(serde_json::Value::as_str),
            _ => None,
        })
        .expect("synthetic Task result")
        .to_string();
    release_tx.send(()).unwrap();

    let completed = runtime
        .await_terminal(
            &rua_task::TaskOwner::new("agent-a", &session_id),
            &task_id,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(completed.state, rua_task::TaskState::Completed);
    assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(redispatches.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn foreground_wait_expiry_adopts_running_tool_without_redispatch() {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let redispatches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let data_dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    let runtime = runtime_with_probe(data_dir.path(), redispatches.clone());
    let (toolkit, mut call) =
        blocking_tool_with_wait(started_tx, release_rx, executions.clone(), 30);
    call.arguments = serde_json::json!({"fixture": true, "yield_seconds": 1});
    let session_id = format!("session-timeout-{}", uuid::Uuid::new_v4());
    let tool_ctx = rua_core::tool::ToolContext {
        agent_id: "agent-a".to_string(),
        session_id: session_id.clone(),
        working_dir: working_dir.path().to_path_buf(),
        task_runtime: Some(Arc::new(TaskRuntimeHandle::new(runtime.clone()))),
        ..Default::default()
    };
    let hook: Arc<dyn react_agent::AgentHook> = Arc::new(NoopHook);
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let cancel = CancellationToken::new();

    let foreground = tokio::spawn(async move {
        execute_single_tool(
            &call,
            &toolkit,
            &hook,
            &tool_ctx,
            &react_agent::ToolDisclosureConfig::default(),
            0,
            &cancel,
            Duration::ZERO,
            &event_tx,
        )
        .await
        .unwrap()
    });

    started_rx.await.unwrap();
    let synthetic = tokio::time::timeout(Duration::from_secs(2), foreground)
        .await
        .expect("foreground wait expiry should return task_id")
        .unwrap();
    let task_id = synthetic
        .metadata
        .get("background_task_id")
        .and_then(serde_json::Value::as_str)
        .expect("synthetic Task result")
        .to_string();

    assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(redispatches.load(std::sync::atomic::Ordering::SeqCst), 0);
    release_tx.send(()).unwrap();

    let completed = runtime
        .await_terminal(
            &rua_task::TaskOwner::new("agent-a", &session_id),
            &task_id,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(completed.state, rua_task::TaskState::Completed);
    assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(redispatches.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn task_owned_tool_ignores_foreground_transfer_wait() {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let working_dir = tempfile::tempdir().unwrap();
    let (toolkit, mut call) =
        blocking_tool_with_wait(started_tx, release_rx, executions.clone(), 30);
    call.arguments = serde_json::json!({"fixture": true, "yield_seconds": 1});
    let mut tool_ctx = rua_core::tool::ToolContext {
        agent_id: "agent-a".to_string(),
        session_id: format!("task-session-{}", uuid::Uuid::new_v4()),
        working_dir: working_dir.path().to_path_buf(),
        ..Default::default()
    };
    tool_ctx.metadata.insert(
        "task_id".to_string(),
        serde_json::Value::String(uuid::Uuid::new_v4().to_string()),
    );
    let hook: Arc<dyn react_agent::AgentHook> = Arc::new(NoopHook);
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let cancel = CancellationToken::new();

    let execution = tokio::spawn(async move {
        execute_single_tool(
            &call,
            &toolkit,
            &hook,
            &tool_ctx,
            &react_agent::ToolDisclosureConfig::default(),
            0,
            &cancel,
            Duration::ZERO,
            &event_tx,
        )
        .await
    });

    started_rx.await.unwrap();
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    release_tx.send(()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(2), execution)
        .await
        .expect("task-owned tool should keep running past its transfer wait")
        .unwrap()
        .unwrap();
    assert_eq!(result.text_content(), "background-complete");
    assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn shell_foreground_wait_expiry_keeps_the_same_process_running() {
    let redispatches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let data_dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    let runtime = runtime_with_probe(data_dir.path(), redispatches.clone());
    let definition = rua_tool::ShellTool::spec().to_tool_definition();
    let schema = definition.parameters.clone();
    let shell: Arc<dyn rua_contract::Tool> = Arc::new(rua_tool::ShellTool::new(Arc::new(
        rua_tool::RuntimeCommandExecutor::new(),
    )));
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        definition,
        Arc::new(ContractToolExecutor::new(shell).with_schema(schema)),
    );
    let call = ToolCall::new(
        format!("shell-call-{}", uuid::Uuid::new_v4()),
        "bash",
        serde_json::json!({
            "command": "sleep 1.15; printf shell-finished",
            "yield_seconds": 1
        }),
    );
    let session_id = format!("shell-session-{}", uuid::Uuid::new_v4());
    let tool_result_store_dir = data_dir.path().join("tool-results");
    std::fs::create_dir_all(&tool_result_store_dir).unwrap();
    let tool_ctx = rua_core::tool::ToolContext {
        agent_id: "agent-a".to_string(),
        session_id: session_id.clone(),
        working_dir: working_dir.path().to_path_buf(),
        tool_result_store_dir: Some(tool_result_store_dir),
        task_runtime: Some(Arc::new(TaskRuntimeHandle::new(runtime.clone()))),
        ..Default::default()
    };
    let hook: Arc<dyn react_agent::AgentHook> = Arc::new(NoopHook);
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let cancel = CancellationToken::new();

    let synthetic = tokio::time::timeout(
        Duration::from_secs(2),
        execute_single_tool(
            &call,
            &toolkit,
            &hook,
            &tool_ctx,
            &react_agent::ToolDisclosureConfig::default(),
            0,
            &cancel,
            Duration::ZERO,
            &event_tx,
        ),
    )
    .await
    .expect("Shell foreground wait should return task_id")
    .unwrap();
    let task_id = synthetic
        .metadata
        .get("background_task_id")
        .and_then(serde_json::Value::as_str)
        .expect("synthetic Task result")
        .to_string();

    let completed = runtime
        .await_terminal(
            &rua_task::TaskOwner::new("agent-a", &session_id),
            &task_id,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(completed.state, rua_task::TaskState::Completed);
    assert!(completed.result.unwrap().output.contains("shell-finished"));
    assert_eq!(redispatches.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn completed_agent_turn_does_not_cancel_adopted_shell_execution() {
    let redispatches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let data_dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    let runtime = runtime_with_probe(data_dir.path(), redispatches.clone());
    let definition = rua_tool::ShellTool::spec().to_tool_definition();
    let schema = definition.parameters.clone();
    let shell: Arc<dyn rua_contract::Tool> = Arc::new(rua_tool::ShellTool::new(Arc::new(
        rua_tool::RuntimeCommandExecutor::new(),
    )));
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        definition,
        Arc::new(ContractToolExecutor::new(shell).with_schema(schema)),
    );
    let session_id = format!("shell-turn-session-{}", uuid::Uuid::new_v4());
    let owner = rua_task::TaskOwner::new("agent-a", &session_id);
    let tool_result_store_dir = data_dir.path().join("tool-results");
    std::fs::create_dir_all(&tool_result_store_dir).unwrap();
    let tool_ctx = rua_core::tool::ToolContext {
        agent_id: "agent-a".to_string(),
        session_id: session_id.clone(),
        working_dir: working_dir.path().to_path_buf(),
        tool_result_store_dir: Some(tool_result_store_dir),
        task_runtime: Some(Arc::new(TaskRuntimeHandle::new(runtime.clone()))),
        ..Default::default()
    };
    let agent = ReactAgent::builder(Arc::new(shell_transfer_model(
        "sleep 1.3; printf shell-finished-after-turn",
        1,
    )))
    .toolkit(Arc::new(toolkit))
    .tool_ctx(tool_ctx)
    .build();

    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "run shell")]);
    let mut events = Vec::new();
    while let Some(event) = futures::StreamExt::next(&mut stream).await {
        let terminal = event.is_terminal();
        events.push(event);
        if terminal {
            break;
        }
    }
    drop(stream);

    let task_id = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result } => result
                .metadata
                .get("background_task_id")
                .and_then(serde_json::Value::as_str),
            _ => None,
        })
        .expect("synthetic Task result")
        .to_string();
    let completed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(5))
        .await
        .unwrap();

    assert_eq!(completed.state, rua_task::TaskState::Completed);
    assert!(completed
        .result
        .unwrap()
        .output
        .contains("shell-finished-after-turn"));
    assert_eq!(redispatches.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn adopted_shell_task_keeps_the_original_stdin_surface() {
    let redispatches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let data_dir = tempfile::tempdir().unwrap();
    let working_dir = tempfile::tempdir().unwrap();
    let runtime = runtime_with_probe(data_dir.path(), redispatches.clone());
    let definition = rua_tool::ShellTool::spec().to_tool_definition();
    let schema = definition.parameters.clone();
    let shell: Arc<dyn rua_contract::Tool> = Arc::new(rua_tool::ShellTool::new(Arc::new(
        rua_tool::RuntimeCommandExecutor::new(),
    )));
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        definition,
        Arc::new(ContractToolExecutor::new(shell).with_schema(schema)),
    );
    let call = ToolCall::new(
        format!("shell-stdin-call-{}", uuid::Uuid::new_v4()),
        "bash",
        serde_json::json!({
            "command": "cat",
            "yield_seconds": 1
        }),
    );
    let session_id = format!("shell-stdin-session-{}", uuid::Uuid::new_v4());
    let owner = rua_task::TaskOwner::new("agent-a", &session_id);
    let tool_result_store_dir = data_dir.path().join("tool-results");
    std::fs::create_dir_all(&tool_result_store_dir).unwrap();
    let tool_ctx = rua_core::tool::ToolContext {
        agent_id: "agent-a".to_string(),
        session_id: session_id.clone(),
        working_dir: working_dir.path().to_path_buf(),
        tool_result_store_dir: Some(tool_result_store_dir),
        task_runtime: Some(Arc::new(TaskRuntimeHandle::new(runtime.clone()))),
        ..Default::default()
    };
    let hook: Arc<dyn react_agent::AgentHook> = Arc::new(NoopHook);
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let cancel = CancellationToken::new();

    let synthetic = execute_single_tool(
        &call,
        &toolkit,
        &hook,
        &tool_ctx,
        &react_agent::ToolDisclosureConfig::default(),
        0,
        &cancel,
        Duration::ZERO,
        &event_tx,
    )
    .await
    .unwrap();
    let task_id = synthetic
        .metadata
        .get("background_task_id")
        .and_then(serde_json::Value::as_str)
        .expect("synthetic Task result")
        .to_string();
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime
            .get(&owner, &task_id)
            .await
            .is_ok_and(|record| record.input_capability != rua_task::TaskInputCapability::Stdin)
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();

    let text = rua_task::TaskInputSubmission::new(
        "adopted-text",
        rua_task::TaskInput::Text {
            text: "adopted-input".to_string(),
        },
    )
    .unwrap();
    let accepted = runtime
        .send_input(&owner, &task_id, text.clone())
        .await
        .unwrap();
    assert_eq!(
        runtime.send_input(&owner, &task_id, text).await.unwrap(),
        accepted
    );
    runtime
        .send_input(
            &owner,
            &task_id,
            rua_task::TaskInputSubmission::new("adopted-close", rua_task::TaskInput::CloseStdin)
                .unwrap(),
        )
        .await
        .unwrap();

    let completed = runtime
        .await_terminal(&owner, &task_id, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(completed.state, rua_task::TaskState::Completed);
    assert!(completed.result.unwrap().output.contains("adopted-input"));
    assert_eq!(redispatches.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn active_interjection_without_task_runtime_waits_for_tool_boundary() {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let working_dir = tempfile::tempdir().unwrap();
    let (toolkit, _) = blocking_tool(started_tx, release_rx, executions.clone());
    let tool_ctx = rua_core::tool::ToolContext {
        agent_id: "agent-a".to_string(),
        session_id: "session-no-runtime".to_string(),
        working_dir: working_dir.path().to_path_buf(),
        task_runtime: None,
        ..Default::default()
    };
    let (control_tx, control_rx) = tokio::sync::watch::channel(rua_core::RunControl::Running);
    let agent = ReactAgent::builder(Arc::new(interjection_model()))
        .toolkit(Arc::new(toolkit))
        .tool_ctx(tool_ctx)
        .run_control(Some(control_rx))
        .build();

    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "run slow")]);
    let mut collect = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = futures::StreamExt::next(&mut stream).await {
            let terminal = event.is_terminal();
            events.push(event);
            if terminal {
                break;
            }
        }
        events
    });

    started_rx.await.unwrap();
    control_tx.send(rua_core::RunControl::Interject).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut collect)
            .await
            .is_err(),
        "without a Task Runtime, interjection must preserve the running foreground tool"
    );
    release_tx.send(()).unwrap();
    let events = tokio::time::timeout(Duration::from_secs(5), collect)
        .await
        .unwrap()
        .unwrap();
    let result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result } => Some(result),
            _ => None,
        })
        .expect("foreground Tool result");
    assert_eq!(result.text_content(), "background-complete");
    assert!(!result.metadata.contains_key("background_task_id"));
    assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 1);
}
