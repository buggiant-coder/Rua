use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_types::{ToolCall, ToolDefinition, ToolResult};
use async_trait::async_trait;
use react_agent::hooks::{AgentHook, NoopHook};
use react_agent::react_loop::execute_single_tool_with_product_boundaries;
use react_agent::toolkit::{AgentToolkit, ToolExecutor};
use react_agent::{
    ToolDisclosureConfig, ToolObservationEvent, ToolObservationInvocation, ToolObserverBoundary,
    ToolObserverContribution, ToolObserverFailure,
};
use tokio_util::sync::CancellationToken;

struct FixedExecutor {
    error: bool,
}

#[async_trait]
impl ToolExecutor for FixedExecutor {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        Ok(if self.error {
            ToolResult::error(call.id.clone(), "canonical failure")
        } else {
            ToolResult::text(call.id.clone(), "canonical success")
        })
    }
}

struct RecordingObserver {
    events: Arc<Mutex<Vec<ToolObservationInvocation>>>,
    fail: bool,
}

#[async_trait]
impl ToolObserverBoundary for RecordingObserver {
    async fn observe(
        &self,
        invocation: ToolObservationInvocation,
        _cancellation: CancellationToken,
    ) -> Result<Vec<ToolObserverContribution>, ToolObserverFailure> {
        self.events.lock().unwrap().push(invocation);
        if self.fail {
            Err(ToolObserverFailure {
                diagnostic: "observer unavailable".to_string(),
            })
        } else {
            Ok(vec![ToolObserverContribution {
                definition_revision: "observer@v1".to_string(),
                source: "native".to_string(),
                resolved_order: 0,
                kind: "feedback".to_string(),
                content: serde_json::json!({"replace_result": "malicious"}),
            }])
        }
    }
}

fn toolkit(error: bool) -> AgentToolkit {
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "echo".to_string(),
            description: "echo".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            annotations: Default::default(),
            metadata: Default::default(),
        },
        Arc::new(FixedExecutor { error }),
    );
    toolkit
}

async fn execute(error: bool, observer: Arc<dyn ToolObserverBoundary>) -> ToolResult {
    let toolkit = toolkit(error);
    let hook: Arc<dyn AgentHook> = Arc::new(NoopHook);
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    execute_single_tool_with_product_boundaries(
        &ToolCall::new("call-1", "echo", serde_json::json!({})),
        &toolkit,
        &hook,
        &rua_core::tool::ToolContext::default(),
        &ToolDisclosureConfig::default(),
        1,
        &CancellationToken::new(),
        Duration::ZERO,
        &event_tx,
        None,
        Some(&observer),
        None,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn successful_result_is_observed_after_commit_and_cannot_be_replaced() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let result = execute(
        false,
        Arc::new(RecordingObserver {
            events: Arc::clone(&events),
            fail: false,
        }),
    )
    .await;

    assert!(!result.is_error);
    assert_eq!(result.text_content(), "canonical success");
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, ToolObservationEvent::After);
    assert_eq!(events[0].result, result);
}

#[tokio::test]
async fn failed_result_is_observed_without_converting_it_to_success() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let result = execute(
        true,
        Arc::new(RecordingObserver {
            events: Arc::clone(&events),
            fail: false,
        }),
    )
    .await;

    assert!(result.is_error);
    assert_eq!(result.text_content(), "canonical failure");
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, ToolObservationEvent::Failed);
    assert_eq!(events[0].result, result);
}

#[tokio::test]
async fn observer_failure_preserves_the_canonical_result() {
    let result = execute(
        false,
        Arc::new(RecordingObserver {
            events: Arc::new(Mutex::new(Vec::new())),
            fail: true,
        }),
    )
    .await;

    assert!(!result.is_error);
    assert_eq!(result.text_content(), "canonical success");
}
