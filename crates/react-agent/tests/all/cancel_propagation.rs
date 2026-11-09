use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role, ToolCall,
    ToolDefinition, ToolResult,
};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo};
use llm_multiplex::LlmError;
use react_agent::hooks::NoopHook;
use react_agent::react_loop::execute_tools;
use react_agent::toolkit::{AgentToolkit, ToolExecutor};
use react_agent::{AgentContext, AgentError, ReactConfig};
use rua_core::tool::ToolContext;
use serde_json::Map;
use tokio::sync::Notify;

struct PendingModel {
    info: ModelInfo,
}

impl PendingModel {
    fn new() -> Self {
        Self {
            info: ModelInfo {
                id: "pending-mock".into(),
                name: "pending-mock".into(),
                context_window: None,
                max_output_tokens: None,
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: Map::new(),
            },
        }
    }
}

#[async_trait]
impl ChatModel for PendingModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("PendingModel.chat should not be called in the streaming path")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let pending: Pin<Box<dyn Stream<Item = Result<AgentEvent, LlmError>> + Send>> =
            Box::pin(futures::stream::pending());
        pending
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct ThinkingThenPendingModel {
    info: ModelInfo,
}

#[derive(Default)]
struct PersistenceHook {
    saved: AtomicBool,
    finished_after_save: AtomicBool,
}

#[async_trait]
impl react_agent::AgentHook for PersistenceHook {
    async fn after_model_call(
        &self,
        messages: &[ModelDialogueItem],
        _iteration: u32,
        _total_usage: &agent_types::TokenUsage,
        _last_call_usage: Option<&agent_types::TokenUsage>,
        terminal: bool,
    ) -> Result<(), AgentError> {
        // Yield during persistence so outer cancellation cannot win unnoticed.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(terminal);
        assert!(messages.iter().any(|message| message.content.iter().any(|part|
            matches!(part, agent_types::ContentPart::Thinking { text, .. } if text == "partial thinking")
        )));
        self.saved.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn on_finish(&self, _: u32, _: &Result<(), AgentError>) {
        self.finished_after_save
            .store(self.saved.load(Ordering::SeqCst), Ordering::SeqCst);
    }
}

#[tokio::test]
async fn stop_waits_for_partial_thinking_persistence_before_finish() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let hook = Arc::new(PersistenceHook::default());
    let mut stream = react_agent::ReactAgent::builder(Arc::new(ThinkingThenPendingModel::new()))
        .cancel(cancel.clone())
        .hook(hook.clone())
        .build()
        .run_stream(vec![ModelDialogueItem::text(Role::User, "hi")]);
    let events = tokio::time::timeout(Duration::from_secs(1), async {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            if matches!(event, AgentEvent::ThinkingDelta { .. }) {
                cancel.cancel();
            }
            events.push(event);
        }
        events
    })
    .await
    .expect("stop must finish promptly");
    assert!(
        hook.saved.load(Ordering::SeqCst),
        "Stop discarded partial thinking"
    );
    assert!(hook.finished_after_save.load(Ordering::SeqCst));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::End {
            finish_reason: FinishReason::Cancelled,
            ..
        }
    )));
}

struct PendingPreparationHook;

#[async_trait]
impl react_agent::AgentHook for PendingPreparationHook {
    async fn before_model_call(
        &self,
        _: &[ModelDialogueItem],
        _: u32,
        _: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<(), AgentError> {
        std::future::pending().await
    }
}

#[tokio::test(start_paused = true)]
async fn stop_bounds_cleanup_when_preparation_does_not_cooperate() {
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut stream = react_agent::ReactAgent::builder(Arc::new(ThinkingThenPendingModel::new()))
        .cancel(cancel.clone())
        .hook(Arc::new(PendingPreparationHook))
        .build()
        .run_stream(vec![ModelDialogueItem::text(Role::User, "hi")]);
    assert!(matches!(
        stream.next().await,
        Some(AgentEvent::Start { .. })
    ));
    tokio::task::yield_now().await;
    cancel.cancel();
    let events = tokio::time::timeout(Duration::from_secs(6), stream.collect::<Vec<_>>())
        .await
        .expect("cancel cleanup must be bounded");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::End {
            finish_reason: FinishReason::Cancelled,
            ..
        }
    )));
}

struct BoundaryBlockingTool {
    started: Notify,
    release: Notify,
    completed: AtomicBool,
}

#[async_trait]
impl ToolExecutor for BoundaryBlockingTool {
    async fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, AgentError> {
        self.started.notify_one();
        self.release.notified().await;
        self.completed.store(true, Ordering::SeqCst);
        Ok(ToolResult::text(call.id.clone(), "completed"))
    }
}

impl ThinkingThenPendingModel {
    fn new() -> Self {
        Self {
            info: ModelInfo {
                id: "thinking-pending-mock".into(),
                name: "thinking-pending-mock".into(),
                context_window: None,
                max_output_tokens: None,
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: Map::new(),
            },
        }
    }
}

#[async_trait]
impl ChatModel for ThinkingThenPendingModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("ThinkingThenPendingModel.chat should not be called in the streaming path")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let stream = futures::stream::iter(vec![Ok(AgentEvent::ThinkingDelta {
            delta: "partial thinking".into(),
            signature: None,
            metadata: serde_json::Value::Null,
        })])
        .chain(futures::stream::pending());
        Box::pin(stream)
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_aborts_blocking_model_stream_within_1s() {
    let model = Arc::new(PendingModel::new());
    let ctx = AgentContext::new(
        model,
        Arc::new(AgentToolkit::new()),
        ReactConfig::default(),
        Arc::new(NoopHook),
        ToolContext::default(),
    );

    let (tx, _rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
    let iteration_counter = AtomicU32::new(0);
    let messages = vec![ModelDialogueItem::text(Role::User, "hi")];

    let cancel = ctx.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        react_agent::react_loop::run_react_loop(&ctx, messages, tx, &iteration_counter),
    )
    .await
    .expect("run_react_loop must return within 1s after cancel");

    assert!(
        matches!(result, Err(AgentError::Cancelled)),
        "expected Err(Cancelled), got: {result:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn interject_ends_blocking_model_stream_as_interjected_not_cancelled() {
    let model = Arc::new(ThinkingThenPendingModel::new());
    let (control_tx, control_rx) = tokio::sync::watch::channel(rua_core::RunControl::Running);
    let ctx = AgentContext::new(
        model,
        Arc::new(AgentToolkit::new()),
        ReactConfig::default(),
        Arc::new(NoopHook),
        ToolContext::default(),
    )
    .with_run_control(Some(control_rx));

    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
    let iteration_counter = AtomicU32::new(0);
    let messages = vec![ModelDialogueItem::text(Role::User, "hi")];

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        control_tx.send(rua_core::RunControl::Interject).unwrap();
    });

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        react_agent::react_loop::run_react_loop(&ctx, messages, tx, &iteration_counter),
    )
    .await
    .expect("run_react_loop must return within 1s after interject");

    assert!(matches!(result, Ok(())), "expected Ok, got: {result:?}");

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ThinkingDelta { delta, .. } if delta == "partial thinking"
        )),
        "partial thinking should be preserved before interjection"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::End {
                finish_reason: FinishReason::Unknown(reason),
                ..
            } if reason == "interjected"
        )),
        "interjection should end the turn with finish_reason=interjected; events={events:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn boundary_interjection_does_not_abort_in_flight_model_call() {
    let model = Arc::new(PendingModel::new());
    let (control_tx, control_rx) = tokio::sync::watch::channel(rua_core::RunControl::Running);
    let ctx = AgentContext::new(
        model,
        Arc::new(AgentToolkit::new()),
        ReactConfig::default(),
        Arc::new(NoopHook),
        ToolContext::default(),
    )
    .with_run_control(Some(control_rx));
    control_tx
        .send(rua_core::RunControl::InterjectAtBoundary)
        .unwrap();

    let (tx, _rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
    let iteration_counter = AtomicU32::new(0);
    let messages = vec![ModelDialogueItem::text(Role::User, "hi")];
    let result = tokio::time::timeout(
        Duration::from_millis(150),
        react_agent::react_loop::run_react_loop(&ctx, messages, tx, &iteration_counter),
    )
    .await;

    assert!(
        result.is_err(),
        "passive boundary steering must not abort an in-flight Model Call"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn boundary_interjection_waits_for_the_current_tool_group() {
    assert_interjection_waits_for_tool(rua_core::RunControl::InterjectAtBoundary).await;
}

#[tokio::test(flavor = "current_thread")]
async fn collaboration_interjection_waits_for_the_current_tool_group() {
    assert_interjection_waits_for_tool(rua_core::RunControl::InterjectAfterTool).await;
}

async fn assert_interjection_waits_for_tool(control: rua_core::RunControl) {
    let tool = Arc::new(BoundaryBlockingTool {
        started: Notify::new(),
        release: Notify::new(),
        completed: AtomicBool::new(false),
    });
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "blocking".to_string(),
            description: "blocks until released".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        tool.clone(),
    );
    let (control_tx, control_rx) = tokio::sync::watch::channel(rua_core::RunControl::Running);
    let ctx = AgentContext::new(
        Arc::new(PendingModel::new()),
        Arc::new(toolkit),
        ReactConfig {
            parallel_tool_calls: false,
            ..Default::default()
        },
        Arc::new(NoopHook),
        ToolContext::default(),
    )
    .with_run_control(Some(control_rx));
    let calls = vec![ToolCall::new(
        "blocking-call",
        "blocking",
        serde_json::json!({}),
    )];
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let execution = execute_tools(&ctx, &calls, 0, &event_tx);
    tokio::pin!(execution);

    tokio::select! {
        _ = tool.started.notified() => {}
        result = &mut execution => panic!("Tool Group ended before the Tool started: {result:?}"),
    }
    control_tx.send(control).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), &mut execution)
            .await
            .is_err(),
        "passive boundary steering must not abort the running Tool Group"
    );
    tool.release.notify_one();
    let results = tokio::time::timeout(Duration::from_secs(1), &mut execution)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(tool.completed.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn collaboration_interjection_interrupts_pending_model() {
    let (control_tx, control_rx) = tokio::sync::watch::channel(rua_core::RunControl::Running);
    let ctx = AgentContext::new(
        Arc::new(PendingModel::new()),
        Arc::new(AgentToolkit::new()),
        ReactConfig::default(),
        Arc::new(NoopHook),
        ToolContext::default(),
    )
    .with_run_control(Some(control_rx));
    control_tx
        .send(rua_core::RunControl::InterjectAfterTool)
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
    let counter = AtomicU32::new(0);
    tokio::time::timeout(
        Duration::from_secs(1),
        react_agent::react_loop::run_react_loop(
            &ctx,
            vec![ModelDialogueItem::text(Role::User, "hi")],
            tx,
            &counter,
        ),
    )
    .await
    .expect("model must yield promptly")
    .unwrap();
    let mut interjected = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, AgentEvent::End { finish_reason: FinishReason::Unknown(ref reason), .. } if reason == "interjected")
        {
            interjected = true;
        }
    }
    assert!(interjected);
}
