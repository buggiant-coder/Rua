use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_types::{AgentEvent, ToolAnnotations, ToolCall, ToolDefinition, ToolResult};
use async_trait::async_trait;
use react_agent::hooks::NoopHook;
use react_agent::react_loop::execute_tools;
use react_agent::test_support::MockChatModel;
use react_agent::toolkit::{AgentToolkit, ToolExecutor};
use react_agent::{AgentContext, ReactConfig};

struct StaticTool {
    text: &'static str,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolExecutor for StaticTool {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::text(call.id.clone(), self.text))
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

fn read_only_annotations() -> ToolAnnotations {
    ToolAnnotations {
        read_only: Some(true),
        destructive: Some(false),
        idempotent: Some(true),
        requires_approval: Some(false),
        ..Default::default()
    }
}

fn write_annotations() -> ToolAnnotations {
    ToolAnnotations {
        read_only: Some(false),
        destructive: Some(false),
        idempotent: Some(false),
        requires_approval: Some(false),
        ..Default::default()
    }
}

fn make_context(write_calls: Arc<AtomicUsize>) -> AgentContext {
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        tool_definition("memory_search", read_only_annotations()),
        Arc::new(StaticTool {
            text: "No memory results.",
            calls: Arc::new(AtomicUsize::new(0)),
        }),
    );
    toolkit.register(
        tool_definition("memory_write", write_annotations()),
        Arc::new(StaticTool {
            text: "memory saved",
            calls: write_calls,
        }),
    );
    toolkit.register(
        tool_definition("Other_Tool", read_only_annotations()),
        Arc::new(StaticTool {
            text: "other ok",
            calls: Arc::new(AtomicUsize::new(0)),
        }),
    );

    AgentContext::new(
        Arc::new(MockChatModel::text_response("unused")),
        Arc::new(toolkit),
        ReactConfig {
            parallel_tool_calls: true,
            max_parallel_tools: 4,
            ..Default::default()
        },
        Arc::new(NoopHook),
        rua_core::tool::ToolContext {
            agent_id: "agent".into(),
            session_id: "session".into(),
            working_dir: std::env::temp_dir(),
            ..Default::default()
        },
    )
}

async fn execute(ctx: &AgentContext, calls: Vec<ToolCall>) -> Vec<ToolResult> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let results = execute_tools(ctx, &calls, 0, &tx)
        .await
        .expect("execute tools");
    drop(tx);

    let mut emitted = Vec::new();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::ToolResult { result } = event {
            emitted.push(result);
        }
    }
    assert_eq!(emitted, results);
    results
}

#[tokio::test]
async fn memory_write_is_rejected_without_recent_memory_search() {
    let write_calls = Arc::new(AtomicUsize::new(0));
    let ctx = make_context(Arc::clone(&write_calls));

    let results = execute(
        &ctx,
        vec![ToolCall::new(
            "write-1",
            "memory_write",
            serde_json::json!({"content": "remember this"}),
        )],
    )
    .await;

    assert_eq!(write_calls.load(Ordering::SeqCst), 0);
    assert_eq!(results.len(), 1);
    assert!(results[0].text_content().contains("memory_write rejected"));
    assert!(results[0].text_content().contains("memory_search"));
    assert!(results[0].text_content().contains("memory_update"));
}

#[tokio::test]
async fn memory_write_is_allowed_after_recent_memory_search() {
    let write_calls = Arc::new(AtomicUsize::new(0));
    let ctx = make_context(Arc::clone(&write_calls));

    let results = execute(
        &ctx,
        vec![
            ToolCall::new(
                "search-1",
                "memory_search",
                serde_json::json!({"keywords": "response style"}),
            ),
            ToolCall::new(
                "write-1",
                "memory_write",
                serde_json::json!({"content": "User prefers terse answers."}),
            ),
        ],
    )
    .await;

    assert_eq!(write_calls.load(Ordering::SeqCst), 1);
    assert_eq!(results.len(), 2);
    assert_eq!(results[1].text_content(), "memory saved");
}

#[tokio::test]
async fn memory_write_is_allowed_after_search_in_previous_tool_iteration() {
    let write_calls = Arc::new(AtomicUsize::new(0));
    let ctx = make_context(Arc::clone(&write_calls));

    let search_results = execute(
        &ctx,
        vec![ToolCall::new(
            "search-1",
            "memory_search",
            serde_json::json!({"keywords": "response style"}),
        )],
    )
    .await;
    assert_eq!(search_results[0].text_content(), "No memory results.");

    let write_results = execute(
        &ctx,
        vec![ToolCall::new(
            "write-1",
            "memory_write",
            serde_json::json!({"content": "User prefers terse answers."}),
        )],
    )
    .await;

    assert_eq!(write_calls.load(Ordering::SeqCst), 1);
    assert_eq!(write_results[0].text_content(), "memory saved");
}

#[tokio::test]
async fn memory_search_falls_out_of_the_write_guard_window() {
    let write_calls = Arc::new(AtomicUsize::new(0));
    let ctx = make_context(Arc::clone(&write_calls));

    let mut calls = vec![ToolCall::new(
        "search-1",
        "memory_search",
        serde_json::json!({"keywords": "response style"}),
    )];
    for index in 0..5 {
        calls.push(ToolCall::new(
            format!("other-{index}"),
            "Other_Tool",
            serde_json::json!({}),
        ));
    }
    calls.push(ToolCall::new(
        "write-1",
        "memory_write",
        serde_json::json!({"content": "User prefers terse answers."}),
    ));

    let results = execute(&ctx, calls).await;

    assert_eq!(write_calls.load(Ordering::SeqCst), 0);
    let last = results.last().expect("last result").text_content();
    assert!(last.contains("memory_write rejected"), "got: {last}");
}
