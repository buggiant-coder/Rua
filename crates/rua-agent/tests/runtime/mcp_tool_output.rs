use std::sync::Arc;

use agent_types::{AgentEvent, ContentPart, ToolCall, ToolResult};
use async_trait::async_trait;
use react_agent::hooks::NoopHook;
use react_agent::react_loop::execute_tools;
use react_agent::test_support::MockChatModel;
use react_agent::{AgentContext, ReactConfig};
use rua_core::tool::{ToolContext, ToolSchema};

struct FakeTextTool {
    output: String,
    extra_content: Vec<ContentPart>,
}

#[async_trait]
impl rua_contract::Tool for FakeTextTool {
    fn name(&self) -> &str {
        "fake_text"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "fake_text".into(),
            description: "fake".into(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    async fn call(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let mut result = ToolResult::text("mcp", &self.output);
        result.content.extend(self.extra_content.clone());
        Ok(result)
    }
}

fn fake_text_tool(output: String) -> Arc<dyn rua_contract::Tool> {
    Arc::new(FakeTextTool {
        output,
        extra_content: Vec::new(),
    })
}

fn toolkit_with(tool: Arc<dyn rua_contract::Tool>) -> react_agent::AgentToolkit {
    let mut toolkit = react_agent::AgentToolkit::new();
    rua_agent::toolkit::register_mcp_tools(&mut toolkit, vec![tool]);
    toolkit
}

#[tokio::test]
async fn mcp_registered_tool_direct_execution_returns_raw_result() {
    let big_text = "x".repeat(5_000);
    let toolkit = toolkit_with(fake_text_tool(big_text.clone()));
    let executor = toolkit
        .get_executor("fake_text")
        .expect("tool should be registered");

    let call = ToolCall::new("tc-direct", "fake_text", serde_json::json!({}));
    let result = executor
        .execute(&call, &ToolContext::default())
        .await
        .unwrap();

    assert!(
        result.truncation.is_none(),
        "MCP registration should not install a special truncation wrapper"
    );
    assert_eq!(result.text_content(), big_text);
}

#[tokio::test]
async fn mcp_tool_output_is_compacted_by_shared_react_processor() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let big_text = "x".repeat(5_000);
    let image = ContentPart::Image {
        url: "data:image/png;base64,abc".into(),
        media_type: Some("image/png".into()),
        alt: Some("chart".into()),
    };
    let toolkit = toolkit_with(Arc::new(FakeTextTool {
        output: big_text.clone(),
        extra_content: vec![image.clone()],
    }));
    let ctx = AgentContext::new(
        Arc::new(MockChatModel::text_response("unused")),
        Arc::new(toolkit),
        ReactConfig {
            parallel_tool_calls: false,
            ..Default::default()
        },
        Arc::new(NoopHook),
        ToolContext {
            working_dir: tmp.path().to_path_buf(),
            tool_result_store_dir: Some(tmp.path().join("tool_call_logs")),
            execution_output_max_tokens: 250,
            ..Default::default()
        },
    );

    let call = ToolCall::new("tc-mcp", "fake_text", serde_json::json!({}));
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
    assert_eq!(emitted, results);
    let result = &results[0];
    assert_eq!(
        result
            .truncation
            .as_ref()
            .and_then(|meta| meta.recall_key.as_deref()),
        Some(rua_tool::recall_key("tc-mcp").as_str()),
        "recall_key should be derived from the real tool call id"
    );
    assert!(result.text_content().contains("recall_tool_result"));
    assert!(
        result.content.iter().any(|part| part == &image),
        "non-text content part should be preserved"
    );

    let logger = rua_tool::ToolCallArchive::new(tmp.path().join("tool_call_logs"));
    let blob = rua_tool::ToolResultStore::read(&logger, &rua_tool::recall_key("tc-mcp"))
        .await
        .expect("raw output");
    assert_eq!(blob.content, big_text);
}
