use std::sync::Arc;

use agent_types::{ToolCall, ToolDefinition};
use react_agent::hooks::{HookEvent, RecordingHook};
use react_agent::react_loop::execute_single_tool;
use react_agent::toolkit::{
    AgentToolkit, DisclosureClass, MockToolExecutor, ToolDisclosureMeta, ToolSourceKind,
};
use react_agent::AgentHook;
use react_agent::ToolDisclosureConfig;
use rua_core::tool::ToolContext;
use tokio_util::sync::CancellationToken;

fn tool_definition(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} test tool"),
        parameters: serde_json::json!({"type": "object"}),
        strict: None,
        annotations: Default::default(),
        metadata: Default::default(),
    }
}

fn meta(class: DisclosureClass) -> ToolDisclosureMeta {
    ToolDisclosureMeta {
        class,
        source_kind: ToolSourceKind::Builtin,
        source_name: Some("test".into()),
        category: Some("test".into()),
        schema_tokens: Some(10),
    }
}

fn toolkit() -> AgentToolkit {
    let mut toolkit = AgentToolkit::new();
    toolkit.register_with_disclosure(
        tool_definition("read"),
        Arc::new(MockToolExecutor::new("read ok")),
        meta(DisclosureClass::Direct),
    );
    toolkit.register_with_disclosure(
        tool_definition("task_create"),
        Arc::new(MockToolExecutor::new("task ok")),
        meta(DisclosureClass::DeferredCandidate),
    );
    toolkit
}

fn toolkit_with_two_deferred_tools() -> AgentToolkit {
    let mut toolkit = toolkit();
    toolkit.register_with_disclosure(
        ToolDefinition {
            name: "plugin__jira__create_issue".into(),
            description: "Create a project issue".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" }
                }
            }),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(MockToolExecutor::new("jira ok")),
        ToolDisclosureMeta {
            class: DisclosureClass::DeferredCandidate,
            source_kind: ToolSourceKind::Plugin,
            source_name: Some("jira".into()),
            category: Some("external".into()),
            schema_tokens: Some(10),
        },
    );
    toolkit
}

fn bridge_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall::new(
        id,
        "tool_call",
        serde_json::json!({
            "name": name,
            "arguments": arguments
        }),
    )
}

#[tokio::test]
async fn tool_call_unwraps_to_real_tool_before_hook_and_logging() {
    let tmp = tempfile::tempdir().unwrap();
    let toolkit = toolkit();
    let recording_hook = Arc::new(RecordingHook::new());
    let hook: Arc<dyn AgentHook> = recording_hook.clone();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let ctx = ToolContext {
        working_dir: tmp.path().to_path_buf(),
        tool_result_store_dir: Some(tmp.path().join("tool_call_logs")),
        ..ToolContext::default()
    };

    let result = execute_single_tool(
        &bridge_call(
            "call-1",
            "task_create",
            serde_json::json!({"title": "ship"}),
        ),
        &toolkit,
        &hook,
        &ctx,
        &ToolDisclosureConfig::default(),
        7,
        &CancellationToken::new(),
        std::time::Duration::from_secs(5),
        &tx,
    )
    .await
    .unwrap();

    assert_eq!(result.name.as_deref(), Some("task_create"));
    let events = recording_hook.events().await;
    assert!(events.iter().any(|event| matches!(
        event,
        HookEvent::BeforeToolCall {
            tool_name,
            iteration: 7
        } if tool_name == "task_create"
    )));
    assert!(events.iter().all(|event| !matches!(
        event,
        HookEvent::BeforeToolCall { tool_name, .. } if tool_name == "tool_call"
    )));

    let _ = rx.recv().await.expect("tool result event");
    let log_key = rua_tool::recall_key("call-1");
    let log_path = tmp
        .path()
        .join("tool_call_logs")
        .join(format!("{log_key}.json"));
    let log: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(log_path).await.unwrap()).unwrap();
    assert_eq!(log["tool_name"], "task_create");
    assert_eq!(log["input"], serde_json::json!({"title": "ship"}));
}

#[tokio::test]
async fn tool_calls_require_exact_canonical_tool_names() {
    let tmp = tempfile::tempdir().unwrap();
    let toolkit = toolkit();
    let hook: Arc<dyn AgentHook> = Arc::new(RecordingHook::new());
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let ctx = ToolContext {
        working_dir: tmp.path().to_path_buf(),
        ..ToolContext::default()
    };

    let result = execute_single_tool(
        &ToolCall::new(
            "read-lower",
            "file_read",
            serde_json::json!({"path": "README.md"}),
        ),
        &toolkit,
        &hook,
        &ctx,
        &ToolDisclosureConfig::default(),
        9,
        &CancellationToken::new(),
        std::time::Duration::from_secs(5),
        &tx,
    )
    .await
    .unwrap();

    assert!(result.is_error);
    assert!(result.text_content().contains("Tool 'file_read' not found"));
}

#[tokio::test]
async fn bridge_tool_call_target_names_require_exact_canonical_case() {
    let tmp = tempfile::tempdir().unwrap();
    let toolkit = toolkit();
    let hook: Arc<dyn AgentHook> = Arc::new(RecordingHook::new());
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let ctx = ToolContext {
        working_dir: tmp.path().to_path_buf(),
        ..ToolContext::default()
    };

    let result = execute_single_tool(
        &bridge_call(
            "bridge-mixed",
            "TASK_CREATE",
            serde_json::json!({"title": "ship"}),
        ),
        &toolkit,
        &hook,
        &ctx,
        &ToolDisclosureConfig::default(),
        10,
        &CancellationToken::new(),
        std::time::Duration::from_secs(5),
        &tx,
    )
    .await
    .unwrap();

    assert!(result.is_error);
    assert!(
        result.text_content().contains("deferred"),
        "got: {}",
        result.text_content()
    );
}

#[tokio::test]
async fn tool_call_cannot_invoke_direct_bridge_or_out_of_scope_tools() {
    let toolkit = toolkit();
    let hook: Arc<dyn AgentHook> = Arc::new(RecordingHook::new());
    let (tx, _rx) = tokio::sync::mpsc::channel(8);

    for target in ["read", "tool_search", "missing_tool"] {
        let result = execute_single_tool(
            &bridge_call("blocked", target, serde_json::json!({})),
            &toolkit,
            &hook,
            &ToolContext::default(),
            &ToolDisclosureConfig::default(),
            0,
            &CancellationToken::new(),
            std::time::Duration::from_secs(5),
            &tx,
        )
        .await
        .unwrap();

        assert!(result.is_error);
        assert!(
            result.text_content().contains("deferred"),
            "error should explain deferred scope for {target}: {}",
            result.text_content()
        );
    }
}

#[tokio::test]
async fn tool_call_rejects_non_object_arguments_with_describe_hint() {
    let toolkit = toolkit();
    let hook: Arc<dyn AgentHook> = Arc::new(RecordingHook::new());
    let (tx, _rx) = tokio::sync::mpsc::channel(8);

    let result = execute_single_tool(
        &bridge_call(
            "bad-args",
            "task_create",
            serde_json::json!("not an object"),
        ),
        &toolkit,
        &hook,
        &ToolContext::default(),
        &ToolDisclosureConfig::default(),
        0,
        &CancellationToken::new(),
        std::time::Duration::from_secs(5),
        &tx,
    )
    .await
    .unwrap();

    assert!(result.is_error);
    assert!(result.text_content().contains("tool_describe"));
}

#[tokio::test]
async fn tool_search_and_describe_run_against_current_deferred_catalog() {
    let toolkit = toolkit_with_two_deferred_tools();
    let hook: Arc<dyn AgentHook> = Arc::new(RecordingHook::new());
    let (tx, _rx) = tokio::sync::mpsc::channel(8);

    let search = execute_single_tool(
        &ToolCall::new(
            "search-1",
            "tool_search",
            serde_json::json!({"query": "project issue"}),
        ),
        &toolkit,
        &hook,
        &ToolContext::default(),
        &ToolDisclosureConfig::default(),
        0,
        &CancellationToken::new(),
        std::time::Duration::from_secs(5),
        &tx,
    )
    .await
    .unwrap();
    assert!(!search.is_error);
    assert!(search.text_content().contains("plugin__jira__create_issue"));
    assert!(!search.text_content().contains("\"properties\""));

    let described = execute_single_tool(
        &ToolCall::new(
            "describe-1",
            "tool_describe",
            serde_json::json!({"name": "plugin__jira__create_issue"}),
        ),
        &toolkit,
        &hook,
        &ToolContext::default(),
        &ToolDisclosureConfig::default(),
        0,
        &CancellationToken::new(),
        std::time::Duration::from_secs(5),
        &tx,
    )
    .await
    .unwrap();
    assert!(!described.is_error);
    assert!(described.text_content().contains("\"properties\""));
}

#[tokio::test]
async fn tool_describe_rejects_direct_tools() {
    let toolkit = toolkit();
    let hook: Arc<dyn AgentHook> = Arc::new(RecordingHook::new());
    let (tx, _rx) = tokio::sync::mpsc::channel(8);

    let result = execute_single_tool(
        &ToolCall::new(
            "describe-read",
            "tool_describe",
            serde_json::json!({"name": "read"}),
        ),
        &toolkit,
        &hook,
        &ToolContext::default(),
        &ToolDisclosureConfig::default(),
        0,
        &CancellationToken::new(),
        std::time::Duration::from_secs(5),
        &tx,
    )
    .await
    .unwrap();

    assert!(result.is_error);
    assert!(result.text_content().contains("deferred"));
}

#[tokio::test]
async fn direct_tool_calls_still_execute_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let toolkit = toolkit();
    let recording_hook = Arc::new(RecordingHook::new());
    let hook: Arc<dyn AgentHook> = recording_hook.clone();
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let ctx = ToolContext {
        working_dir: tmp.path().to_path_buf(),
        ..ToolContext::default()
    };

    let result = execute_single_tool(
        &ToolCall::new("read-1", "read", serde_json::json!({"path": "README.md"})),
        &toolkit,
        &hook,
        &ctx,
        &ToolDisclosureConfig::default(),
        3,
        &CancellationToken::new(),
        std::time::Duration::from_secs(5),
        &tx,
    )
    .await
    .unwrap();

    assert_eq!(result.name.as_deref(), Some("read"));
    assert!(recording_hook.events().await.iter().any(|event| matches!(
        event,
        HookEvent::BeforeToolCall {
            tool_name,
            iteration: 3
        } if tool_name == "read"
    )));
}
