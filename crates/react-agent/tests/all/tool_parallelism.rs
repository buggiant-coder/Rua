use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_types::{AgentEvent, ToolAnnotations, ToolCall, ToolDefinition, ToolResult};
use async_trait::async_trait;
use react_agent::hooks::NoopHook;
use react_agent::react_loop::{execute_single_tool, execute_tools};
use react_agent::test_support::MockChatModel;
use react_agent::toolkit::{AgentToolkit, ToolExecutor};
use react_agent::{AgentContext, ReactConfig};
use rua_tool::extended::task_tools::TaskRuntimeHandle;
use rua_tool::{ToolTaskExecution, ToolTaskExecutionRequest, ToolTaskRunner};
use tokio_util::sync::CancellationToken;

fn update_max(max_active: &AtomicUsize, current: usize) {
    let mut observed = max_active.load(Ordering::SeqCst);
    while current > observed {
        match max_active.compare_exchange(observed, current, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(next) => observed = next,
        }
    }
}

struct DelayedTool {
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolExecutor for DelayedTool {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        update_max(&self.max_active, current);
        let delay_ms = call
            .arguments
            .get("delay_ms")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(ToolResult::text(
            call.id.clone(),
            format!("result:{}", call.id),
        ))
    }
}

struct FatalOrSlowTool {
    slow_completed: Arc<AtomicUsize>,
}

struct CancellationObservingTool {
    cancelled: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl ToolExecutor for CancellationObservingTool {
    async fn execute(
        &self,
        _call: &ToolCall,
        ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        let mut cancel = ctx.cancel.clone().expect("tool cancellation receiver");
        let cancelled = Arc::clone(&self.cancelled);
        tokio::spawn(async move {
            if !*cancel.borrow() {
                while cancel.changed().await.is_ok() {
                    if *cancel.borrow() {
                        break;
                    }
                }
            }
            cancelled.notify_one();
        });
        std::future::pending().await
    }
}

struct RedispatchProbeExecution {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolTaskExecution for RedispatchProbeExecution {
    async fn execute(&self, _request: ToolTaskExecutionRequest) -> Result<ToolResult, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err("adoption must not redispatch the tool".to_string())
    }
}

#[async_trait]
impl ToolExecutor for FatalOrSlowTool {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        if call
            .arguments
            .get("fatal")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return Err(react_agent::AgentError::ToolFatal {
                tool: call.name.clone(),
                message: "fatal test failure".into(),
            });
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        self.slow_completed.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::text(call.id.clone(), "slow completed"))
    }
}

fn tool_definition(name: &str, annotations: ToolAnnotations) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} test tool"),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
        strict: None,
        annotations,
        metadata: Default::default(),
    }
}

fn readonly_annotations() -> ToolAnnotations {
    ToolAnnotations {
        read_only: Some(true),
        destructive: Some(false),
        idempotent: Some(true),
        requires_approval: Some(false),
        ..Default::default()
    }
}

fn destructive_annotations() -> ToolAnnotations {
    ToolAnnotations {
        read_only: Some(false),
        destructive: Some(true),
        idempotent: Some(false),
        requires_approval: Some(true),
        ..Default::default()
    }
}

fn make_context(
    tool_name: &str,
    annotations: ToolAnnotations,
    parallel_tool_calls: bool,
    max_parallel_tools: usize,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
) -> AgentContext {
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        tool_definition(tool_name, annotations),
        Arc::new(DelayedTool { active, max_active }),
    );
    let config = ReactConfig {
        parallel_tool_calls,
        max_parallel_tools,
        ..Default::default()
    };
    AgentContext::new(
        Arc::new(MockChatModel::text_response("unused")),
        Arc::new(toolkit),
        config,
        Arc::new(NoopHook),
        rua_core::tool::ToolContext {
            agent_id: format!("agent-{}", uuid::Uuid::new_v4()),
            session_id: format!("session-{}", uuid::Uuid::new_v4()),
            working_dir: std::env::temp_dir()
                .join(format!("rua-react-agent-parallel-{}", uuid::Uuid::new_v4())),
            ..Default::default()
        },
    )
}

fn tool_calls(tool_name: &str, delays: &[u64]) -> Vec<ToolCall> {
    delays
        .iter()
        .enumerate()
        .map(|(index, delay_ms)| {
            ToolCall::new(
                format!("tc-{index}"),
                tool_name.to_string(),
                serde_json::json!({ "delay_ms": delay_ms }),
            )
        })
        .collect()
}

async fn execute_and_collect_events(
    ctx: &AgentContext,
    calls: &[ToolCall],
) -> (Vec<ToolResult>, Vec<String>) {
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let results = execute_tools(ctx, calls, 0, &tx)
        .await
        .expect("execute tools");
    drop(tx);

    let mut event_ids = Vec::new();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::ToolResult { result } = event {
            event_ids.push(result.call_id);
        }
    }
    (results, event_ids)
}

#[tokio::test]
async fn read_only_tools_respect_max_parallel_tools_and_preserve_result_order() {
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let ctx = make_context(
        "read_tool",
        readonly_annotations(),
        true,
        2,
        Arc::clone(&active),
        Arc::clone(&max_active),
    );
    let calls = tool_calls("read_tool", &[60, 40, 20, 0]);

    let (results, event_ids) = execute_and_collect_events(&ctx, &calls).await;

    let result_ids: Vec<_> = results
        .iter()
        .map(|result| result.call_id.as_str())
        .collect();
    assert_eq!(result_ids, vec!["tc-0", "tc-1", "tc-2", "tc-3"]);
    assert_eq!(event_ids, vec!["tc-0", "tc-1", "tc-2", "tc-3"]);
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        2,
        "parallel execution must honor max_parallel_tools"
    );
}

#[tokio::test]
async fn parallel_transferable_tools_use_independent_waits_and_preserve_result_order() {
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let redispatches = Arc::new(AtomicUsize::new(0));
    let data_dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(
        rua_task::TaskRuntime::open(rua_task::TaskRuntimeConfig::new(data_dir.path())).unwrap(),
    );
    runtime.register_runner(Arc::new(ToolTaskRunner::new(Arc::new(
        RedispatchProbeExecution {
            calls: Arc::clone(&redispatches),
        },
    ))));

    let mut definition = tool_definition("transferable_tool", readonly_annotations());
    definition.parameters = serde_json::json!({
        "type": "object",
        "properties": {
            "delay_ms": { "type": "integer" },
            "yield_seconds": { "type": "integer", "default": 10 }
        }
    });
    definition.metadata.insert(
        rua_tool::TOOL_TRANSFER_CONTRACT_METADATA_KEY.to_string(),
        serde_json::to_value(rua_tool::ToolTransferContract::transferable_seconds(
            "yield_seconds",
            10,
        ))
        .unwrap(),
    );
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        definition,
        Arc::new(DelayedTool {
            active: Arc::clone(&active),
            max_active: Arc::clone(&max_active),
        }),
    );
    let registered = toolkit.get_definition("transferable_tool").unwrap();
    assert_eq!(
        rua_tool::tool_transfer_contract(registered).policy,
        rua_tool::ToolTransferPolicy::Transferable
    );
    assert_eq!(
        rua_tool::foreground_wait_limit(
            registered,
            &serde_json::json!({"delay_ms": 1_100, "yield_seconds": 1}),
        ),
        Some(Duration::from_secs(1))
    );
    let owner_agent_id = format!("agent-{}", uuid::Uuid::new_v4());
    let session_id = format!("session-{}", uuid::Uuid::new_v4());
    let ctx = AgentContext::new(
        Arc::new(MockChatModel::text_response("unused")),
        Arc::new(toolkit),
        ReactConfig {
            parallel_tool_calls: true,
            max_parallel_tools: 2,
            ..Default::default()
        },
        Arc::new(NoopHook),
        rua_core::tool::ToolContext {
            agent_id: owner_agent_id.clone(),
            session_id: session_id.clone(),
            working_dir: data_dir.path().to_path_buf(),
            task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(&runtime)))),
            ..Default::default()
        },
    );
    let calls = vec![
        ToolCall::new(
            "tc-transfer",
            "transferable_tool",
            serde_json::json!({"delay_ms": 1_100, "yield_seconds": 1}),
        ),
        ToolCall::new(
            "tc-normal",
            "transferable_tool",
            serde_json::json!({"delay_ms": 5, "yield_seconds": 10}),
        ),
    ];

    let (results, event_ids) = tokio::time::timeout(
        Duration::from_secs(2),
        execute_and_collect_events(&ctx, &calls),
    )
    .await
    .expect("Tool Group must not wait for adopted Task completion");
    assert_eq!(
        results
            .iter()
            .map(|result| result.call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["tc-transfer", "tc-normal"]
    );
    assert_eq!(event_ids, vec!["tc-transfer", "tc-normal"]);
    let task_id = results[0]
        .metadata
        .get("background_task_id")
        .and_then(serde_json::Value::as_str)
        .expect("first call should transfer")
        .to_string();
    assert_eq!(results[1].text_content(), "result:tc-normal");
    assert_eq!(max_active.load(Ordering::SeqCst), 2);
    assert_eq!(redispatches.load(Ordering::SeqCst), 0);

    let completed = runtime
        .await_terminal(
            &rua_task::TaskOwner::new(&owner_agent_id, &session_id),
            &task_id,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
    assert_eq!(completed.state, rua_task::TaskState::Completed);
    assert_eq!(redispatches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn destructive_tools_run_sequentially_even_when_parallel_is_enabled() {
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let ctx = make_context(
        "write_tool",
        destructive_annotations(),
        true,
        4,
        Arc::clone(&active),
        Arc::clone(&max_active),
    );
    let calls = tool_calls("write_tool", &[20, 20, 20]);

    let (results, event_ids) = execute_and_collect_events(&ctx, &calls).await;

    let result_ids: Vec<_> = results
        .iter()
        .map(|result| result.call_id.as_str())
        .collect();
    assert_eq!(result_ids, vec!["tc-0", "tc-1", "tc-2"]);
    assert_eq!(event_ids, vec!["tc-0", "tc-1", "tc-2"]);
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        1,
        "destructive tools must not run concurrently"
    );
}

#[tokio::test]
async fn zero_tool_timeout_delegates_deadline_to_the_tool() {
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let mut ctx = make_context(
        "slow_tool",
        readonly_annotations(),
        false,
        1,
        Arc::clone(&active),
        Arc::clone(&max_active),
    );
    ctx.config.tool_timeout_secs = 0;
    let calls = tool_calls("slow_tool", &[50]);

    let (results, _) = execute_and_collect_events(&ctx, &calls).await;

    assert_eq!(results.len(), 1);
    assert!(!results[0].is_error, "got: {}", results[0].text_content());
    assert_eq!(results[0].text_content(), "result:tc-0");
}

#[tokio::test]
async fn nonzero_tool_timeout_remains_an_explicit_orchestrator_deadline() {
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let mut ctx = make_context(
        "bounded_tool",
        readonly_annotations(),
        false,
        1,
        Arc::clone(&active),
        Arc::clone(&max_active),
    );
    ctx.config.tool_timeout_secs = 1;
    let calls = tool_calls("bounded_tool", &[1_100]);

    let (results, _) = execute_and_collect_events(&ctx, &calls).await;

    assert_eq!(results.len(), 1);
    assert!(results[0].is_error);
    assert!(
        results[0].text_content().contains("timed out after 1s"),
        "got: {}",
        results[0].text_content()
    );
}

#[tokio::test]
async fn foreground_only_timeout_signals_tool_context_cancellation() {
    let cancelled = Arc::new(tokio::sync::Notify::new());
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        tool_definition("bounded_tool", readonly_annotations()),
        Arc::new(CancellationObservingTool {
            cancelled: Arc::clone(&cancelled),
        }),
    );
    let call = ToolCall::new("bounded-call", "bounded_tool", serde_json::json!({}));
    let hook: Arc<dyn react_agent::AgentHook> = Arc::new(NoopHook);
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);

    let result = execute_single_tool(
        &call,
        &toolkit,
        &hook,
        &rua_core::tool::ToolContext::default(),
        &react_agent::ToolDisclosureConfig::default(),
        0,
        &CancellationToken::new(),
        Duration::from_millis(20),
        &event_tx,
    )
    .await
    .unwrap();

    assert!(result.is_error);
    tokio::time::timeout(Duration::from_secs(1), cancelled.notified())
        .await
        .expect("foreground-only timeout must reach ToolContext cancellation");
}

#[tokio::test]
async fn fatal_parallel_tool_cancels_and_drains_sibling_tasks() {
    let slow_completed = Arc::new(AtomicUsize::new(0));
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        tool_definition("parallel_tool", readonly_annotations()),
        Arc::new(FatalOrSlowTool {
            slow_completed: Arc::clone(&slow_completed),
        }),
    );
    let ctx = AgentContext::new(
        Arc::new(MockChatModel::text_response("unused")),
        Arc::new(toolkit),
        ReactConfig {
            parallel_tool_calls: true,
            max_parallel_tools: 2,
            ..Default::default()
        },
        Arc::new(NoopHook),
        rua_core::tool::ToolContext::default(),
    );
    let calls = vec![
        ToolCall::new(
            "slow-call",
            "parallel_tool",
            serde_json::json!({"fatal": false}),
        ),
        ToolCall::new(
            "fatal-call",
            "parallel_tool",
            serde_json::json!({"fatal": true}),
        ),
    ];
    let (tx, _rx) = tokio::sync::mpsc::channel(16);

    let result = execute_tools(&ctx, &calls, 0, &tx).await;

    assert!(matches!(
        result,
        Err(react_agent::AgentError::ToolFatal { .. })
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        slow_completed.load(Ordering::SeqCst),
        0,
        "fatal batch failure must not leave detached sibling tools running"
    );
}
