use std::sync::Arc;

use agent_types::{AgentEvent, ToolCall, ToolDefinition, ToolResult};
use async_trait::async_trait;
use react_agent::hooks::NoopHook;
use react_agent::react_loop::execute_tools;
use react_agent::test_support::MockChatModel;
use react_agent::toolkit::{AgentToolkit, ContractToolExecutor, ToolExecutor};
use react_agent::{AgentContext, ReactConfig};
use rua_tool::ToolResultStore;
use rua_tool::{MockCommandExecutor, MockResponse, ShellTool};

struct LongOutputTool {
    output: String,
}

#[async_trait]
impl ToolExecutor for LongOutputTool {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        Ok(ToolResult::text(call.id.clone(), self.output.clone()))
    }
}

#[tokio::test]
async fn oversized_tool_result_is_compacted_before_event_and_raw_output_is_recallable() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_output = (0..500)
        .map(|line| format!("line {line}: {}", "x".repeat(40)))
        .collect::<Vec<_>>()
        .join("\n");

    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "long_output".into(),
            description: "returns long text".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(LongOutputTool {
            output: raw_output.clone(),
        }),
    );

    let ctx = AgentContext::new(
        Arc::new(MockChatModel::text_response("unused")),
        Arc::new(toolkit),
        ReactConfig {
            parallel_tool_calls: false,
            ..Default::default()
        },
        Arc::new(NoopHook),
        rua_core::tool::ToolContext {
            agent_id: "agent".into(),
            session_id: "session".into(),
            working_dir: tmp.path().to_path_buf(),
            tool_result_store_dir: Some(tmp.path().join("tool_call_logs")),
            execution_output_max_tokens: 128,
            ..Default::default()
        },
    );

    let call = ToolCall::new("call-long", "long_output", serde_json::json!({}));
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let results = execute_tools(&ctx, &[call], 0, &tx)
        .await
        .expect("execute tools");
    drop(tx);

    let mut emitted = Vec::new();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::ToolResult { result } = event {
            emitted.push(result);
        }
    }

    assert_eq!(results.len(), 1);
    assert_eq!(emitted.len(), 1);

    let returned_text = results[0].text_content();
    let emitted_text = emitted[0].text_content();
    assert_eq!(
        emitted_text, returned_text,
        "ToolResult event should match the compacted result returned to the model"
    );
    assert!(
        returned_text.contains("<truncated_output"),
        "oversized output should be compacted before emission; got: {returned_text}"
    );
    assert!(
        returned_text.contains("recall_tool_result"),
        "compacted output should point the model at recall"
    );

    let recall_key = results[0]
        .truncation
        .as_ref()
        .and_then(|meta| meta.recall_key.as_deref())
        .expect("recall key");
    let logger = rua_tool::ToolCallArchive::new(tmp.path().join("tool_call_logs"));
    let blob = logger.read(recall_key).await.expect("raw output log");
    assert_eq!(blob.content, raw_output);
}

#[tokio::test]
async fn shell_output_is_compacted_by_shared_processor() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_output = (0..500)
        .map(|line| format!("line {line}: {}", "x".repeat(40)))
        .collect::<Vec<_>>()
        .join("\n");

    let executor = Arc::new(MockCommandExecutor::new());
    executor.add_response(
        "long",
        MockResponse {
            stdout: raw_output.clone(),
            stderr: String::new(),
            exit_code: 0,
            delay_ms: 0,
        },
    );

    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "bash".into(),
            description: "Execute shell commands".into(),
            parameters: ShellTool::spec().parameters,
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(ContractToolExecutor::new(Arc::new(ShellTool::new(
            executor,
        )))),
    );

    let ctx = AgentContext::new(
        Arc::new(MockChatModel::text_response("unused")),
        Arc::new(toolkit),
        ReactConfig {
            parallel_tool_calls: false,
            ..Default::default()
        },
        Arc::new(NoopHook),
        rua_core::tool::ToolContext {
            agent_id: "agent".into(),
            session_id: "session".into(),
            working_dir: tmp.path().to_path_buf(),
            tool_result_store_dir: Some(tmp.path().join("tool_call_logs")),
            execution_output_max_tokens: 128,
            ..Default::default()
        },
    );

    let call = ToolCall::new("call-shell", "bash", serde_json::json!({"command": "long"}));
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let results = execute_tools(&ctx, &[call], 0, &tx)
        .await
        .expect("execute tools");
    drop(tx);

    let mut emitted = Vec::new();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::ToolResult { result } = event {
            emitted.push(result);
        }
    }

    assert_eq!(results.len(), 1);
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].text_content(), results[0].text_content());
    assert!(results[0].text_content().contains("<truncated_output"));
    assert!(results[0].text_content().contains("recall_tool_result"));

    let recall_key = results[0]
        .truncation
        .as_ref()
        .and_then(|meta| meta.recall_key.as_deref())
        .expect("recall key");
    let logger = rua_tool::ToolCallArchive::new(tmp.path().join("tool_call_logs"));
    let blob = logger.read(recall_key).await.expect("raw output log");
    assert_eq!(blob.content, raw_output);
}
