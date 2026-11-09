use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role, TokenUsage,
    ToolCall, ToolDefinition, ToolResult,
};
use async_trait::async_trait;
use futures::StreamExt;
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo};
use llm_multiplex::LlmError;
use react_agent::toolkit::{AgentToolkit, ToolExecutor};
use react_agent::{ReactAgent, ReactConfig};

struct ToolUntilDisabledModel {
    info: ModelInfo,
    advertised_tool_counts: Arc<tokio::sync::Mutex<Vec<usize>>>,
    call_count: AtomicUsize,
}

#[async_trait]
impl ChatModel for ToolUntilDisabledModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test model")
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        let advertised_tool_counts = Arc::clone(&self.advertised_tool_counts);
        let tool_count = request.tools.len();
        let call_index = self.call_count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async_stream::stream! {
            advertised_tool_counts.lock().await.push(tool_count);
            if tool_count == 0 {
                yield Ok(AgentEvent::TextDelta {
                    delta: "final summary".into(),
                    metadata: serde_json::Value::Null,
                });
                yield Ok(AgentEvent::End {
                    finish_reason: FinishReason::Stop,
                    usage: Some(TokenUsage::new(200, 20)),
                    metadata: serde_json::Value::Null,
                });
            } else {
                yield Ok(AgentEvent::ToolCallDelta {
                    id: format!("call-{call_index}"),
                    index: 0,
                    name: Some("Read_Project".into()),
                    arguments_delta: "{}".into(),
                    metadata: serde_json::Value::Null,
                });
                yield Ok(AgentEvent::End {
                    finish_reason: FinishReason::ToolUse,
                    usage: Some(TokenUsage::new(100, 10)),
                    metadata: serde_json::Value::Null,
                });
            }
        })
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct CountingTool {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolExecutor for CountingTool {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::text(call.id.clone(), "ok"))
    }
}

#[tokio::test]
async fn max_iterations_counts_tool_rounds_then_runs_one_tool_free_final_call() {
    let advertised_tool_counts = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let model = Arc::new(ToolUntilDisabledModel {
        info: ModelInfo {
            id: "tool-until-disabled".into(),
            name: "tool-until-disabled".into(),
            context_window: None,
            max_output_tokens: None,
            supports_multimodal: None,
            supports_image: None,
            supports_video: None,
            probe_source: None,
            generate_kwargs: serde_json::Map::new(),
        },
        advertised_tool_counts: Arc::clone(&advertised_tool_counts),
        call_count: AtomicUsize::new(0),
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "Read_Project".into(),
            description: "read project".into(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(CountingTool {
            calls: Arc::clone(&calls),
        }),
    );
    let workdir = tempfile::tempdir().expect("temp workdir");
    let agent = ReactAgent::builder(model)
        .toolkit(Arc::new(toolkit))
        .config(ReactConfig {
            max_iterations: 1,
            ..Default::default()
        })
        .tool_ctx(rua_core::tool::ToolContext {
            working_dir: workdir.path().to_path_buf(),
            ..Default::default()
        })
        .build();
    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "read")]);
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*advertised_tool_counts.lock().await, vec![1, 0]);
    let usage_prompt_tokens = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Usage { usage } => Some(usage.prompt_tokens),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(usage_prompt_tokens, vec![100, 200, 300]);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::IterationEnd { .. }))
            .count(),
        2
    );
}
