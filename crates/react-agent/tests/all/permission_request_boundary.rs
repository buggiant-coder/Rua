use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_types::{ToolCall, ToolDefinition, ToolResult};
use async_trait::async_trait;
use react_agent::hooks::{AgentHook, ApprovalResult};
use react_agent::react_loop::execute_single_tool_with_product_boundaries;
use react_agent::toolkit::{AgentToolkit, ToolExecutor};
use react_agent::{
    PermissionHookDecision, PermissionHookFailure, PermissionRequestBoundary,
    PermissionRequestInvocation, ToolDecision, ToolDisclosureConfig,
};
use tokio_util::sync::CancellationToken;

struct CountingExecutor(Arc<AtomicUsize>);

#[async_trait]
impl ToolExecutor for CountingExecutor {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::text(call.id.clone(), "executed"))
    }
}

struct ApprovalHook {
    decision: ToolDecision,
    interactive_calls: Arc<AtomicUsize>,
    recorded: Arc<Mutex<Vec<(String, PermissionHookDecision)>>>,
}

#[async_trait]
impl AgentHook for ApprovalHook {
    async fn before_tool_call(
        &self,
        _call: &ToolCall,
        _iteration: u32,
    ) -> Result<ToolDecision, react_agent::AgentError> {
        Ok(self.decision.clone())
    }

    async fn wait_for_approval(
        &self,
        _tool_use_id: &str,
        _tool_name: &str,
        _prompt: &str,
        _call: &ToolCall,
    ) -> ApprovalResult {
        self.interactive_calls.fetch_add(1, Ordering::SeqCst);
        ApprovalResult::Allow
    }

    async fn record_permission_hook_decision(
        &self,
        request_id: &str,
        _prompt: &str,
        _call: &ToolCall,
        decision: &PermissionHookDecision,
    ) -> ApprovalResult {
        self.recorded
            .lock()
            .unwrap()
            .push((request_id.to_string(), decision.clone()));
        match decision {
            PermissionHookDecision::Allow { .. } => ApprovalResult::Allow,
            PermissionHookDecision::Deny { reason } => ApprovalResult::Deny {
                reason: reason.clone(),
            },
            PermissionHookDecision::NoDecision => unreachable!(),
        }
    }
}

struct FixedPermissionBoundary {
    result: Result<PermissionHookDecision, PermissionHookFailure>,
    invocations: Arc<Mutex<Vec<PermissionRequestInvocation>>>,
}

type RecordedDecisions = Arc<Mutex<Vec<(String, PermissionHookDecision)>>>;

#[async_trait]
impl PermissionRequestBoundary for FixedPermissionBoundary {
    async fn decide(
        &self,
        invocation: PermissionRequestInvocation,
        _cancellation: CancellationToken,
    ) -> Result<PermissionHookDecision, PermissionHookFailure> {
        self.invocations.lock().unwrap().push(invocation);
        self.result.clone()
    }
}

fn toolkit(calls: Arc<AtomicUsize>) -> AgentToolkit {
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
        Arc::new(CountingExecutor(calls)),
    );
    toolkit
}

async fn execute(
    hook: Arc<dyn AgentHook>,
    boundary: Arc<dyn PermissionRequestBoundary>,
    calls: Arc<AtomicUsize>,
    arguments: serde_json::Value,
) -> ToolResult {
    let toolkit = toolkit(calls);
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    execute_single_tool_with_product_boundaries(
        &ToolCall::new("call-1", "echo", arguments),
        &toolkit,
        &hook,
        &rua_core::tool::ToolContext::default(),
        &ToolDisclosureConfig::default(),
        1,
        &CancellationToken::new(),
        Duration::ZERO,
        &event_tx,
        None,
        None,
        Some(&boundary),
    )
    .await
    .unwrap()
}

fn approval_hook(
    decision: ToolDecision,
) -> (Arc<ApprovalHook>, Arc<AtomicUsize>, RecordedDecisions) {
    let interactive = Arc::new(AtomicUsize::new(0));
    let recorded = Arc::new(Mutex::new(Vec::new()));
    (
        Arc::new(ApprovalHook {
            decision,
            interactive_calls: interactive.clone(),
            recorded: recorded.clone(),
        }),
        interactive,
        recorded,
    )
}

#[tokio::test]
async fn hard_deny_never_invokes_permission_hooks() {
    let (hook, _, _) = approval_hook(ToolDecision::Deny {
        reason: "hard policy".to_string(),
    });
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let result = execute(
        hook,
        Arc::new(FixedPermissionBoundary {
            result: Ok(PermissionHookDecision::Allow { reason: None }),
            invocations: invocations.clone(),
        }),
        calls.clone(),
        serde_json::json!({}),
    )
    .await;

    assert!(result.is_error);
    assert!(invocations.lock().unwrap().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn allow_and_deny_are_recorded_as_one_shot_approval_facts() {
    for decision in [
        PermissionHookDecision::Allow { reason: None },
        PermissionHookDecision::Deny {
            reason: "automation denied".to_string(),
        },
    ] {
        let (hook, interactive, recorded) = approval_hook(ToolDecision::RequiresApproval {
            request_id: "approval-1".to_string(),
            prompt: "approval required".to_string(),
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let result = execute(
            hook,
            Arc::new(FixedPermissionBoundary {
                result: Ok(decision.clone()),
                invocations: Arc::new(Mutex::new(Vec::new())),
            }),
            calls.clone(),
            serde_json::json!({"value": 1}),
        )
        .await;

        assert_eq!(interactive.load(Ordering::SeqCst), 0);
        assert_eq!(
            recorded.lock().unwrap().as_slice(),
            &[("approval-1".to_string(), decision.clone())]
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            usize::from(matches!(decision, PermissionHookDecision::Allow { .. }))
        );
        assert_eq!(
            result.is_error,
            matches!(decision, PermissionHookDecision::Deny { .. })
        );
    }
}

#[tokio::test]
async fn failure_and_no_decision_fall_back_to_interactive_approval() {
    for boundary_result in [
        Ok(PermissionHookDecision::NoDecision),
        Err(PermissionHookFailure {
            diagnostic: "handler failed".to_string(),
        }),
    ] {
        let (hook, interactive, recorded) = approval_hook(ToolDecision::RequiresApproval {
            request_id: "approval-1".to_string(),
            prompt: "approval required".to_string(),
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let result = execute(
            hook,
            Arc::new(FixedPermissionBoundary {
                result: boundary_result,
                invocations: Arc::new(Mutex::new(Vec::new())),
            }),
            calls.clone(),
            serde_json::json!({}),
        )
        .await;

        assert!(!result.is_error);
        assert_eq!(interactive.load(Ordering::SeqCst), 1);
        assert!(recorded.lock().unwrap().is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn decision_identity_is_bound_to_final_arguments() {
    let (hook, _, _) = approval_hook(ToolDecision::RequiresApproval {
        request_id: "approval-1".to_string(),
        prompt: "approval required".to_string(),
    });
    let invocations = Arc::new(Mutex::new(Vec::new()));
    for value in [1, 2] {
        execute(
            hook.clone(),
            Arc::new(FixedPermissionBoundary {
                result: Ok(PermissionHookDecision::Allow { reason: None }),
                invocations: invocations.clone(),
            }),
            Arc::new(AtomicUsize::new(0)),
            serde_json::json!({"value": value}),
        )
        .await;
    }
    let invocations = invocations.lock().unwrap();
    assert_ne!(
        invocations[0].argument_digest,
        invocations[1].argument_digest
    );
}
