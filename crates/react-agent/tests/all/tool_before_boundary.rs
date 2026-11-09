use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_types::{ToolCall, ToolDefinition, ToolResult};
use async_trait::async_trait;
use react_agent::hooks::AgentHook;
use react_agent::react_loop::execute_single_tool_with_product_boundary;
use react_agent::toolkit::{AgentToolkit, ToolExecutor};
use react_agent::{
    ToolBeforeBoundary, ToolBeforeFailure, ToolBeforeInvocation, ToolBeforeOutcome, ToolDecision,
    ToolDisclosureConfig,
};
use tokio_util::sync::CancellationToken;

struct MutatingBoundary;

#[async_trait]
impl ToolBeforeBoundary for MutatingBoundary {
    async fn apply(
        &self,
        mut invocation: ToolBeforeInvocation,
        _cancellation: CancellationToken,
    ) -> Result<ToolBeforeOutcome, ToolBeforeFailure> {
        invocation.call.arguments = serde_json::json!({"value": "mutated"});
        Ok(ToolBeforeOutcome::Continue(Box::new(invocation.call)))
    }
}

struct InvalidBoundary;

#[async_trait]
impl ToolBeforeBoundary for InvalidBoundary {
    async fn apply(
        &self,
        mut invocation: ToolBeforeInvocation,
        _cancellation: CancellationToken,
    ) -> Result<ToolBeforeOutcome, ToolBeforeFailure> {
        invocation.call.arguments = serde_json::json!({"value": 42});
        Ok(ToolBeforeOutcome::Continue(Box::new(invocation.call)))
    }
}

struct RejectingBoundary;

#[async_trait]
impl ToolBeforeBoundary for RejectingBoundary {
    async fn apply(
        &self,
        _invocation: ToolBeforeInvocation,
        _cancellation: CancellationToken,
    ) -> Result<ToolBeforeOutcome, ToolBeforeFailure> {
        Ok(ToolBeforeOutcome::Reject {
            reason: "blocked by automation".to_string(),
        })
    }
}

struct FailingBoundary;

#[async_trait]
impl ToolBeforeBoundary for FailingBoundary {
    async fn apply(
        &self,
        _invocation: ToolBeforeInvocation,
        _cancellation: CancellationToken,
    ) -> Result<ToolBeforeOutcome, ToolBeforeFailure> {
        Err(ToolBeforeFailure {
            diagnostic: "handler timed out".to_string(),
        })
    }
}

struct RecordingSecurityHook {
    arguments: Arc<Mutex<Vec<serde_json::Value>>>,
}

#[async_trait]
impl AgentHook for RecordingSecurityHook {
    async fn before_tool_call(
        &self,
        call: &ToolCall,
        _iteration: u32,
    ) -> Result<ToolDecision, react_agent::AgentError> {
        self.arguments.lock().unwrap().push(call.arguments.clone());
        Ok(ToolDecision::Allow)
    }
}

struct RecordingExecutor {
    arguments: Arc<Mutex<Vec<serde_json::Value>>>,
}

fn toolkit(arguments: Arc<Mutex<Vec<serde_json::Value>>>) -> AgentToolkit {
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "echo".to_string(),
            description: "echo".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"],
                "additionalProperties": false
            }),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(RecordingExecutor { arguments }),
    );
    toolkit
}

async fn execute_with(
    boundary: Arc<dyn ToolBeforeBoundary>,
    security_arguments: Arc<Mutex<Vec<serde_json::Value>>>,
    executor_arguments: Arc<Mutex<Vec<serde_json::Value>>>,
) -> ToolResult {
    let toolkit = toolkit(executor_arguments);
    let hook: Arc<dyn AgentHook> = Arc::new(RecordingSecurityHook {
        arguments: security_arguments,
    });
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    execute_single_tool_with_product_boundary(
        &ToolCall::new("call-1", "echo", serde_json::json!({"value": "original"})),
        &toolkit,
        &hook,
        &rua_core::tool::ToolContext::default(),
        &ToolDisclosureConfig::default(),
        1,
        &CancellationToken::new(),
        Duration::ZERO,
        &event_tx,
        Some(&boundary),
    )
    .await
    .unwrap()
}

#[async_trait]
impl ToolExecutor for RecordingExecutor {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        self.arguments.lock().unwrap().push(call.arguments.clone());
        Ok(ToolResult::text(call.id.clone(), "ok"))
    }
}

#[tokio::test]
async fn mutation_is_validated_before_existing_security_and_execution() {
    let security_arguments = Arc::new(Mutex::new(Vec::new()));
    let executor_arguments = Arc::new(Mutex::new(Vec::new()));
    let result = execute_with(
        Arc::new(MutatingBoundary),
        Arc::clone(&security_arguments),
        Arc::clone(&executor_arguments),
    )
    .await;

    assert!(!result.is_error);
    assert_eq!(
        security_arguments.lock().unwrap().as_slice(),
        &[serde_json::json!({"value": "mutated"})]
    );
    assert_eq!(
        executor_arguments.lock().unwrap().as_slice(),
        &[serde_json::json!({"value": "mutated"})]
    );
}

#[tokio::test]
async fn invalid_candidate_stops_before_security_and_execution() {
    let security_arguments = Arc::new(Mutex::new(Vec::new()));
    let executor_arguments = Arc::new(Mutex::new(Vec::new()));
    let result = execute_with(
        Arc::new(InvalidBoundary),
        Arc::clone(&security_arguments),
        Arc::clone(&executor_arguments),
    )
    .await;

    assert!(result.is_error);
    assert!(result.text_content().contains("invalid Tool arguments"));
    assert!(security_arguments.lock().unwrap().is_empty());
    assert!(executor_arguments.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rejection_is_not_reported_as_permission_denial() {
    let security_arguments = Arc::new(Mutex::new(Vec::new()));
    let executor_arguments = Arc::new(Mutex::new(Vec::new()));
    let result = execute_with(
        Arc::new(RejectingBoundary),
        Arc::clone(&security_arguments),
        Arc::clone(&executor_arguments),
    )
    .await;

    assert!(result.is_error);
    assert!(result
        .text_content()
        .contains("rejected by tool.before Hook"));
    assert!(!result.text_content().contains("permission"));
    assert!(security_arguments.lock().unwrap().is_empty());
    assert!(executor_arguments.lock().unwrap().is_empty());
}

#[tokio::test]
async fn handler_failure_is_distinct_and_stops_the_tool() {
    let security_arguments = Arc::new(Mutex::new(Vec::new()));
    let executor_arguments = Arc::new(Mutex::new(Vec::new()));
    let result = execute_with(
        Arc::new(FailingBoundary),
        Arc::clone(&security_arguments),
        Arc::clone(&executor_arguments),
    )
    .await;

    assert!(result.is_error);
    assert!(result.text_content().contains("tool.before Hook failed"));
    assert!(result.text_content().contains("handler timed out"));
    assert!(security_arguments.lock().unwrap().is_empty());
    assert!(executor_arguments.lock().unwrap().is_empty());
}
