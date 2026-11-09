use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Duration;

use agent_types::{
    AgentEvent, FinishReason, ModelDialogueItem, Role, ToolCall, ToolDefinition, ToolResult,
};
use async_trait::async_trait;
use llm_multiplex::debug_capture::{self, ActiveCapture, CHAT_ADDRESS_METADATA_KEY};
use react_agent::react_loop::run_react_loop;
use react_agent::test_support::MockChatModel;
use react_agent::{AgentContext, AgentError, AgentHook, AgentToolkit, ReactConfig, ToolExecutor};
use rua_core::ChatAddress;
use serde_json::{json, Value};

struct Capture {
    dir: tempfile::TempDir,
    address: ChatAddress,
}

impl Capture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let address = ChatAddress::project("timing-project", uuid::Uuid::new_v4().to_string());
        debug_capture::register_capture(
            address.clone(),
            ActiveCapture {
                capture_id: uuid::Uuid::new_v4().to_string(),
                capture_path: dir.path().to_path_buf(),
            },
        );
        Self { dir, address }
    }

    fn traces(&self, stage: &str) -> Vec<Value> {
        debug_capture::flush_capture(self.dir.path(), Duration::from_secs(5)).unwrap();
        let mut traces: Vec<Value> = Vec::new();
        for call in std::fs::read_dir(self.dir.path().join("calls")).unwrap() {
            for file in std::fs::read_dir(call.unwrap().path()).unwrap() {
                let path = file.unwrap().path();
                if path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&format!("{stage}-timing-"))
                {
                    traces.push(serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap());
                }
            }
        }
        traces.sort_by(|a, b| {
            a["started_at_process_ms"]
                .as_f64()
                .unwrap()
                .total_cmp(&b["started_at_process_ms"].as_f64().unwrap())
        });
        traces
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        debug_capture::unregister_capture(&self.address);
        debug_capture::flush_capture(self.dir.path(), Duration::from_secs(5)).unwrap();
    }
}

struct DelayedTool;

#[async_trait]
impl ToolExecutor for DelayedTool {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, AgentError> {
        tokio::time::sleep(Duration::from_millis(80)).await;
        // A Tool is not required to supply duration_ms. The runtime must measure it.
        Ok(ToolResult::text(call.id.clone(), "done"))
    }
}

struct DelayedHook;

#[async_trait]
impl AgentHook for DelayedHook {
    async fn after_model_call(
        &self,
        _messages: &[ModelDialogueItem],
        _iteration: u32,
        _total: &agent_types::TokenUsage,
        _last: Option<&agent_types::TokenUsage>,
        _terminal: bool,
    ) -> Result<(), AgentError> {
        tokio::time::sleep(Duration::from_millis(25)).await;
        Ok(())
    }

    async fn after_tool_call(
        &self,
        _call: &ToolCall,
        _result: &ToolResult,
        _duration: u64,
    ) -> Result<(), AgentError> {
        tokio::time::sleep(Duration::from_millis(15)).await;
        Ok(())
    }
}

fn end(reason: FinishReason) -> AgentEvent {
    AgentEvent::End {
        finish_reason: reason,
        usage: None,
        metadata: Value::Null,
    }
}

fn context(capture: &Capture, calls: Vec<ToolCall>, hook: Arc<dyn AgentHook>) -> AgentContext {
    let mut events = calls
        .into_iter()
        .map(|call| AgentEvent::ToolCall { call })
        .collect::<Vec<_>>();
    events.push(end(FinishReason::ToolUse));
    let model = MockChatModel::from_sequences(vec![events, vec![end(FinishReason::Stop)]]);
    let mut toolkit = AgentToolkit::new();
    let mut definition = ToolDefinition {
        name: "read".into(),
        description: "test read".into(),
        parameters: json!({"type": "object"}),
        strict: None,
        annotations: Default::default(),
        metadata: Default::default(),
    };
    definition.annotations.read_only = Some(true);
    definition.annotations.destructive = Some(false);
    definition.annotations.idempotent = Some(true);
    toolkit.register(definition, Arc::new(DelayedTool));
    let mut tool_ctx = rua_core::tool::ToolContext::default();
    tool_ctx.metadata.insert(
        CHAT_ADDRESS_METADATA_KEY.into(),
        serde_json::to_value(&capture.address).unwrap(),
    );
    AgentContext::new(
        Arc::new(model),
        Arc::new(toolkit),
        ReactConfig {
            parallel_tool_calls: true,
            max_parallel_tools: 2,
            auto_continue_on_text_only: false,
            ..Default::default()
        },
        hook,
        tool_ctx,
    )
}

async fn run(ctx: &AgentContext) -> Result<(), AgentError> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let result = run_react_loop(
        ctx,
        vec![ModelDialogueItem::text(Role::User, "go")],
        tx,
        &AtomicU32::new(0),
    )
    .await;
    while rx.recv().await.is_some() {}
    result
}

#[tokio::test]
async fn captures_complete_handoff_and_parallel_executor_intervals_without_tool_reported_duration()
{
    let capture = Capture::new();
    let calls = [
        ToolCall::new("tool-a", "read", json!({})),
        ToolCall::new("tool-b", "read", json!({})),
    ];
    let ctx = context(&capture, calls.to_vec(), Arc::new(DelayedHook));
    run(&ctx).await.unwrap();

    let iterations = capture.traces("iteration");
    assert_eq!(
        iterations.len(),
        2,
        "each logical call needs a runtime timing artifact"
    );
    let first = &iterations[0];
    assert_eq!(first["outcome"], "continued");
    assert_eq!(
        first["data"]["next_model_call_id"],
        iterations[1]["model_call_id"]
    );
    assert_eq!(first["data"]["tool_call_count"], 2);
    let marks = &first["milestones_ms"];
    let mut last = 0.0;
    for name in [
        "model_end_received",
        "model_iteration_finished",
        "assistant_after_model_call_started",
        "assistant_after_model_call_finished",
        "tools_started",
        "tools_finished",
        "tool_results_after_model_call_started",
        "tool_results_after_model_call_finished",
        "next_loop_started",
        "next_preparation_started",
        "next_request_prepared",
    ] {
        let time = marks[name]
            .as_f64()
            .unwrap_or_else(|| panic!("missing {name}: {first}"));
        assert!(time >= last, "out-of-order {name}");
        last = time;
    }
    assert!(
        marks["assistant_after_model_call_finished"]
            .as_f64()
            .unwrap()
            - marks["assistant_after_model_call_started"]
                .as_f64()
                .unwrap()
            >= 25.0
    );
    assert!(
        marks["tool_results_after_model_call_finished"]
            .as_f64()
            .unwrap()
            - marks["tool_results_after_model_call_started"]
                .as_f64()
                .unwrap()
            >= 25.0
    );
    assert_eq!(iterations[1]["outcome"], "completed");
    assert!(iterations[1]["data"]["next_model_call_id"].is_null());

    let batches = capture.traces("tool-batch");
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0]["outcome"], "completed");
    assert!(
        batches[0]["milestones_ms"]["result_emitted"]
            .as_f64()
            .unwrap()
            >= batches[0]["milestones_ms"]["result_emit_started"]
                .as_f64()
                .unwrap()
    );

    let tools = capture.traces("tool");
    assert_eq!(tools.len(), 2);
    let mut intervals = Vec::new();
    for tool in tools {
        assert_eq!(tool["model_call_id"], first["model_call_id"]);
        assert_eq!(tool["process_clock_id"], first["process_clock_id"]);
        assert_eq!(tool["outcome"], "completed");
        assert_eq!(tool["data"]["executor_invoked"], true);
        let origin = tool["started_at_process_ms"].as_f64().unwrap();
        let marks = &tool["milestones_ms"];
        let start = origin + marks["executor_started"].as_f64().unwrap();
        let finish = origin + marks["executor_finished"].as_f64().unwrap();
        assert!(finish - start >= 80.0);
        assert!(
            marks["after_tool_hook_finished"].as_f64().unwrap()
                - marks["after_tool_hook_started"].as_f64().unwrap()
                >= 15.0
        );
        intervals.push((start, finish));
    }
    assert!(
        intervals[0].0.max(intervals[1].0) < intervals[0].1.min(intervals[1].1),
        "parallel intervals must retain their overlap"
    );
}

struct ApprovalHook;

#[async_trait]
impl AgentHook for ApprovalHook {
    async fn before_tool_call(
        &self,
        _call: &ToolCall,
        _iteration: u32,
    ) -> Result<react_agent::ToolDecision, AgentError> {
        Ok(react_agent::ToolDecision::RequiresApproval {
            request_id: "permission-1".into(),
            prompt: "approve".into(),
        })
    }

    async fn wait_for_approval(
        &self,
        _id: &str,
        _name: &str,
        _prompt: &str,
        call: &ToolCall,
    ) -> react_agent::ApprovalResult {
        tokio::time::sleep(Duration::from_millis(30)).await;
        if call.id == "deny" {
            react_agent::ApprovalResult::Deny {
                reason: "denied".into(),
            }
        } else {
            react_agent::ApprovalResult::Allow
        }
    }
}

struct DelayedBoundary;

#[async_trait]
impl react_agent::ToolBeforeBoundary for DelayedBoundary {
    async fn apply(
        &self,
        invocation: react_agent::ToolBeforeInvocation,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<react_agent::ToolBeforeOutcome, react_agent::ToolBeforeFailure> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(react_agent::ToolBeforeOutcome::Continue(Box::new(
            invocation.call,
        )))
    }
}

#[async_trait]
impl react_agent::ToolObserverBoundary for DelayedBoundary {
    async fn observe(
        &self,
        _invocation: react_agent::ToolObservationInvocation,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Vec<react_agent::ToolObserverContribution>, react_agent::ToolObserverFailure> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn approvals_rejections_and_product_hooks_are_not_counted_as_execution() {
    let capture = Capture::new();
    let calls = vec![
        ToolCall::new("allow", "read", json!({})),
        ToolCall::new("deny", "read", json!({})),
    ];
    let mut ctx = context(&capture, calls, Arc::new(ApprovalHook));
    ctx.config.parallel_tool_calls = false;
    ctx.tool_before_boundary = Some(Arc::new(DelayedBoundary));
    ctx.tool_observer_boundary = Some(Arc::new(DelayedBoundary));
    run(&ctx).await.unwrap();
    let tools = capture.traces("tool");
    assert_eq!(tools.len(), 2);
    for trace in &tools {
        let marks = &trace["milestones_ms"];
        for (start, finish, minimum) in [
            ("approval_wait_started", "approval_wait_finished", 30.0),
            ("product_before_started", "product_before_finished", 20.0),
            (
                "product_observer_started",
                "product_observer_finished",
                20.0,
            ),
        ] {
            assert!(marks[finish].as_f64().unwrap() - marks[start].as_f64().unwrap() >= minimum);
        }
        if trace["data"]["tool_call_id"] == "deny" {
            assert_eq!(trace["data"]["executor_invoked"], false);
            assert!(marks["executor_started"].is_null());
            assert_eq!(trace["outcome"], "tool_error");
        } else {
            assert!(
                marks["executor_started"].as_f64().unwrap()
                    >= marks["approval_wait_finished"].as_f64().unwrap()
            );
            assert!(
                marks["product_observer_started"].as_f64().unwrap()
                    >= marks["executor_finished"].as_f64().unwrap()
            );
        }
    }
}

#[tokio::test]
async fn cancellation_during_execution_preserves_partial_timing_without_a_fake_next_call() {
    let capture = Capture::new();
    let ctx = context(
        &capture,
        vec![ToolCall::new("cancel", "read", json!({}))],
        Arc::new(react_agent::NoopHook),
    );
    let cancel = ctx.cancel.clone();
    let cancellation = async {
        loop {
            if capture.dir.path().join("calls").exists()
                && capture
                    .traces("tool")
                    .iter()
                    .any(|trace| !trace["milestones_ms"]["executor_wait_started"].is_null())
            {
                cancel.cancel();
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(run(&ctx), cancellation)
    })
    .await
    .unwrap();
    assert!(result.unwrap_err().is_cancelled());
    let tools = capture.traces("tool");
    assert_eq!(tools[0]["outcome"], "cancelled");
    assert!(tools[0]["milestones_ms"]["executor_finished"].is_number());
    assert!(capture.traces("iteration")[0]["data"]["next_model_call_id"].is_null());
}

#[tokio::test]
async fn disabled_capture_and_write_failure_do_not_change_the_tool_loop() {
    let capture = Capture::new();
    debug_capture::unregister_capture(&capture.address);
    let ctx = context(
        &capture,
        vec![ToolCall::new("off", "read", json!({}))],
        Arc::new(react_agent::NoopHook),
    );
    run(&ctx).await.unwrap();
    assert!(!capture.dir.path().join("calls").exists());

    let invalid_root = tempfile::NamedTempFile::new().unwrap();
    debug_capture::register_capture(
        capture.address.clone(),
        ActiveCapture {
            capture_id: "write-failure".into(),
            capture_path: invalid_root.path().to_path_buf(),
        },
    );
    let ctx = context(
        &capture,
        vec![ToolCall::new("write-failed", "read", json!({}))],
        Arc::new(react_agent::NoopHook),
    );
    run(&ctx).await.unwrap();
}

#[tokio::test]
async fn responses_http_capture_links_provider_iteration_and_tool_timings() {
    use llm_multiplex::{OpenAiCompatibleConfig, OpenAiResponsesProvider, Provider};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let capture = Capture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let stop = tokio_util::sync::CancellationToken::new();
    let cancelled = stop.clone();
    let server = tokio::spawn(async move {
        tokio::select! {
            _ = cancelled.cancelled() => {}
            _ = async {
                for output in [
                    json!([{"type": "function_call", "id": "fc-1", "call_id": "http-tool", "name": "read", "arguments": "{}"}]),
                    json!([]),
                ] {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    loop {
                        let mut buffer = [0; 8192];
                        let count = socket.read(&mut buffer).await.unwrap();
                        assert_ne!(count, 0);
                        bytes.extend_from_slice(&buffer[..count]);
                        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..end]);
                            let length = headers.lines().find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                            }).unwrap();
                            if bytes.len() >= end + 4 + length { break; }
                        }
                    }
                    let mut body = String::new();
                    for item in output.as_array().unwrap() {
                        body.push_str(&format!("data: {}\n\n", json!({
                            "type": "response.output_item.done", "output_index": 0, "item": item,
                        })));
                    }
                    body.push_str(&format!("data: {}\n\n", json!({
                        "type": "response.completed",
                        "response": {"id": "response-http", "output": output, "usage": {"input_tokens": 12, "output_tokens": 4, "total_tokens": 16}},
                    })));
                    let reply = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body);
                    socket.write_all(reply.as_bytes()).await.unwrap();
                }
            } => {}
        }
    });
    let mut ctx = context(&capture, Vec::new(), Arc::new(DelayedHook));
    let provider = OpenAiResponsesProvider::new(OpenAiCompatibleConfig {
        provider_id: "dashscope".into(),
        provider_name: "DashScope test".into(),
        base_url,
        api_key: Some("test-placeholder".into()),
        api_keys: Vec::new(),
        headers: Default::default(),
        models: vec![llm_multiplex::ModelInfo {
            id: "qwen-test".into(),
            name: "test".into(),
            context_window: None,
            max_output_tokens: None,
            supports_multimodal: None,
            supports_image: None,
            supports_video: None,
            probe_source: None,
            generate_kwargs: Default::default(),
        }],
        timeout_secs: 5,
        generate_kwargs: Default::default(),
        support_model_discovery: false,
        support_connection_check: false,
        is_local: true,
    })
    .unwrap();
    ctx.model = Arc::from(provider.create_chat_model("qwen-test").unwrap());
    let result = tokio::time::timeout(Duration::from_secs(10), run(&ctx)).await;
    stop.cancel();
    server.await.unwrap();
    result.unwrap().unwrap();

    debug_capture::flush_capture(capture.dir.path(), Duration::from_secs(5)).unwrap();

    let iterations = capture.traces("iteration");
    let providers = capture.traces("provider");
    let tools = capture.traces("tool");
    assert_eq!(iterations.len(), 2);
    assert_eq!(providers.len(), 2);
    assert_eq!(tools.len(), 1);
    assert_eq!(iterations[0]["outcome"], "continued");
    assert_eq!(iterations[1]["outcome"], "completed");
    assert_eq!(
        iterations[0]["data"]["next_model_call_id"],
        iterations[1]["model_call_id"]
    );
    for (iteration, provider) in iterations.iter().zip(&providers) {
        assert_eq!(iteration["model_call_id"], provider["model_call_id"]);
        assert_eq!(iteration["process_clock_id"], provider["process_clock_id"]);
        assert_eq!(provider["outcome"], "completed");
    }
    let tool = &tools[0];
    assert_eq!(tool["model_call_id"], iterations[0]["model_call_id"]);
    assert_eq!(tool["process_clock_id"], providers[0]["process_clock_id"]);
    assert_eq!(tool["data"]["executor_invoked"], true);
    let start = tool["started_at_process_ms"].as_f64().unwrap()
        + tool["milestones_ms"]["executor_started"].as_f64().unwrap();
    let finish = tool["started_at_process_ms"].as_f64().unwrap()
        + tool["milestones_ms"]["executor_finished"].as_f64().unwrap();
    let provider_finish = providers[0]["started_at_process_ms"].as_f64().unwrap()
        + providers[0]["milestones_ms"]["provider_finished"]
            .as_f64()
            .unwrap();
    let next_send = providers[1]["started_at_process_ms"].as_f64().unwrap()
        + providers[1]["milestones_ms"]["http_send_started"]
            .as_f64()
            .unwrap();
    assert!(start >= provider_finish);
    assert!(finish - start >= 80.0);
    assert!(next_send >= finish);
}
