use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_types::{
    AgentEvent, FinishReason, ModelDialogueItem, Role, TokenUsage, ToolCall, ToolDefinition,
    ToolResult,
};
use async_trait::async_trait;
use futures::StreamExt;
use react_agent::test_support::MockChatModel;
use react_agent::toolkit::{AgentToolkit, ToolExecutor};
use react_agent::ReactAgent;

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
        Ok(ToolResult::text(call.id.clone(), "tool completed"))
    }
}

fn counting_toolkit(calls: Arc<AtomicUsize>) -> Arc<AgentToolkit> {
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "Read_Project".into(),
            description: "Read a project".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(CountingTool { calls }),
    );
    Arc::new(toolkit)
}

#[tokio::test]
async fn tool_calls_execute_even_when_provider_finishes_with_stop() {
    let tool_events = vec![
        AgentEvent::ToolCallDelta {
            id: "call-1".into(),
            index: 0,
            name: Some("Read_Project".into()),
            arguments_delta: r#"{"path":"/tmp/project"}"#.into(),
            metadata: serde_json::Value::Null,
        },
        AgentEvent::End {
            finish_reason: FinishReason::Stop,
            usage: Some(TokenUsage::new(10, 5)),
            metadata: serde_json::Value::Null,
        },
    ];
    let final_events = vec![
        AgentEvent::TextDelta {
            delta: "project summary".into(),
            metadata: serde_json::Value::Null,
        },
        AgentEvent::End {
            finish_reason: FinishReason::Stop,
            usage: Some(TokenUsage::new(10, 5)),
            metadata: serde_json::Value::Null,
        },
    ];
    let model = Arc::new(MockChatModel::from_sequences(vec![
        tool_events,
        final_events,
    ]));

    let calls = Arc::new(AtomicUsize::new(0));
    let workdir = tempfile::tempdir().expect("temp workdir");
    let agent = ReactAgent::builder(model)
        .toolkit(counting_toolkit(Arc::clone(&calls)))
        .tool_ctx(rua_core::tool::ToolContext {
            agent_id: "test-agent".into(),
            session_id: "test-session".into(),
            working_dir: workdir.path().to_path_buf(),
            ..Default::default()
        })
        .build();
    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "read project")]);

    let mut saw_tool_result = false;
    let mut final_text = String::new();
    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::ToolResult { result } if result.call_id == "call-1" => {
                saw_tool_result = true;
            }
            AgentEvent::TextDelta { delta, .. } => final_text.push_str(&delta),
            _ => {}
        }
    }

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(saw_tool_result);
    assert_eq!(final_text, "project summary");
}

#[tokio::test]
async fn duplicate_complete_and_delta_tool_call_id_executes_once() {
    let tool_events = vec![
        AgentEvent::ToolCallDelta {
            id: "call-1".into(),
            index: 0,
            name: Some("Read_Project".into()),
            arguments_delta: r#"{"path":"/tmp/project"}"#.into(),
            metadata: serde_json::Value::Null,
        },
        AgentEvent::ToolCall {
            call: ToolCall::new(
                "call-1",
                "Read_Project",
                serde_json::json!({"path": "/tmp/project"}),
            ),
        },
        AgentEvent::End {
            finish_reason: FinishReason::ToolUse,
            usage: None,
            metadata: serde_json::Value::Null,
        },
    ];
    let final_events = vec![
        AgentEvent::TextDelta {
            delta: "done".into(),
            metadata: serde_json::Value::Null,
        },
        AgentEvent::End {
            finish_reason: FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        },
    ];
    let model = Arc::new(MockChatModel::from_sequences(vec![
        tool_events,
        final_events,
    ]));
    let calls = Arc::new(AtomicUsize::new(0));
    let workdir = tempfile::tempdir().expect("temp workdir");
    let agent = ReactAgent::builder(model)
        .toolkit(counting_toolkit(Arc::clone(&calls)))
        .tool_ctx(rua_core::tool::ToolContext {
            working_dir: workdir.path().to_path_buf(),
            ..Default::default()
        })
        .build();
    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "read")]);
    while stream.next().await.is_some() {}

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn identical_tool_payloads_with_distinct_ids_execute_independently() {
    let tool_events = vec![
        AgentEvent::ToolCall {
            call: ToolCall::new(
                "call-1",
                "Read_Project",
                serde_json::json!({"path": "/tmp/project"}),
            ),
        },
        AgentEvent::ToolCall {
            call: ToolCall::new(
                "call-2",
                "Read_Project",
                serde_json::json!({"path": "/tmp/project"}),
            ),
        },
        AgentEvent::End {
            finish_reason: FinishReason::ToolUse,
            usage: None,
            metadata: serde_json::Value::Null,
        },
    ];
    let final_events = vec![
        AgentEvent::TextDelta {
            delta: "done".into(),
            metadata: serde_json::Value::Null,
        },
        AgentEvent::End {
            finish_reason: FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        },
    ];
    let model = Arc::new(MockChatModel::from_sequences(vec![
        tool_events,
        final_events,
    ]));
    let calls = Arc::new(AtomicUsize::new(0));
    let workdir = tempfile::tempdir().expect("temp workdir");
    let agent = ReactAgent::builder(model)
        .toolkit(counting_toolkit(Arc::clone(&calls)))
        .tool_ctx(rua_core::tool::ToolContext {
            working_dir: workdir.path().to_path_buf(),
            ..Default::default()
        })
        .build();
    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "read")]);
    while stream.next().await.is_some() {}

    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
