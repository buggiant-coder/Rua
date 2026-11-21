use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role, ToolCall,
    ToolDefinition, ToolResult,
};
use async_trait::async_trait;
use futures::StreamExt;
use llm_multiplex::traits::{ChatModel, EventStream, ModelInfo};
use llm_multiplex::LlmError;
use react_agent::hooks::{AgentHook, ApprovalResult, NoopHook};
use react_agent::react_loop::{execute_single_tool, execute_single_tool_with_product_boundary};
use react_agent::toolkit::{AgentToolkit, ToolExecutor};
use react_agent::{
    AgentContext, AgentError, ModelCallSection, ModelCallSectionKind, ReactAgent, ReactConfig,
    ToolBeforeBoundary, ToolBeforeFailure, ToolBeforeInvocation, ToolBeforeOutcome, ToolDecision,
    ToolDisclosureConfig,
};
use rua_core::agents_instructions::{
    AgentsInstructionDecision, AgentsInstructionTarget, AgentsInstructions,
};
use rua_core::tool::ToolContext;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct InstructionState {
    discovered: bool,
    presented: bool,
}

#[derive(Default)]
struct Instructions {
    state: Mutex<InstructionState>,
    discovery_count: AtomicUsize,
    unreadable: bool,
}

#[async_trait]
impl AgentsInstructions for Instructions {
    async fn before_targets(
        &self,
        _targets: &[AgentsInstructionTarget],
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> rua_core::Result<AgentsInstructionDecision> {
        unreachable!("this fixture exercises explicit ToolCalls")
    }

    fn snapshot_for_model(&self) -> Option<String> {
        let mut state = self.state.lock().unwrap();
        if !state.discovered {
            return None;
        }
        state.presented = true;
        Some("Device phone; scope /new; source /new/AGENTS.md: Use the revised value.".into())
    }

    async fn before_tool(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> rua_core::Result<AgentsInstructionDecision> {
        if self.unreadable {
            return Err(rua_core::RuaError::Io {
                message: "instruction file is unreadable".into(),
            });
        }
        if !call.arguments["path"]
            .as_str()
            .is_some_and(|path| path.starts_with("/new/"))
        {
            return Ok(AgentsInstructionDecision::Continue);
        }
        let mut state = self.state.lock().unwrap();
        if !state.discovered {
            self.discovery_count.fetch_add(1, Ordering::SeqCst);
            state.discovered = true;
        }
        Ok(if state.presented {
            AgentsInstructionDecision::Continue
        } else {
            AgentsInstructionDecision::Reconsider {
                reason: "New instructions apply to /new.".into(),
            }
        })
    }
}

struct WritingTool(Arc<Mutex<Vec<Value>>>);

struct ObservedCalls(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl react_agent::ToolObserverBoundary for ObservedCalls {
    async fn observe(
        &self,
        invocation: react_agent::ToolObservationInvocation,
        _cancellation: CancellationToken,
    ) -> Result<Vec<react_agent::ToolObserverContribution>, react_agent::ToolObserverFailure> {
        self.0.lock().unwrap().push(invocation.call.id);
        Ok(Vec::new())
    }
}

#[async_trait]
impl ToolExecutor for WritingTool {
    async fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, AgentError> {
        self.0.lock().unwrap().push(call.arguments.clone());
        Ok(ToolResult::text(&call.id, "written"))
    }
}

fn toolkit(writes: Arc<Mutex<Vec<Value>>>) -> AgentToolkit {
    let mut toolkit = AgentToolkit::new();
    toolkit.register(ToolDefinition {
        name: "write_fixture".into(),
        description: "Record an observable fixture write".into(),
        parameters: json!({"type":"object","properties":{"path":{"type":"string"},"value":{"type":"string"}},"required":["path","value"]}),
        strict: None,
        annotations: Default::default(),
        metadata: [("supports_parallel_tool_calls".into(), true.into())].into_iter().collect(),
    }, Arc::new(WritingTool(writes)));
    toolkit
}

fn call(id: &str, path: &str, value: &str) -> ToolCall {
    ToolCall::new(id, "write_fixture", json!({"path":path,"value":value}))
}

fn response(calls: Vec<ToolCall>) -> Vec<AgentEvent> {
    let has_tools = !calls.is_empty();
    let mut events = calls
        .into_iter()
        .enumerate()
        .map(|(index, call)| AgentEvent::ToolCallDelta {
            id: call.id,
            index,
            name: Some(call.name),
            arguments_delta: call.arguments.to_string(),
            metadata: Value::Null,
        })
        .collect::<Vec<_>>();
    if !has_tools {
        events.push(AgentEvent::TextDelta {
            delta: "done".into(),
            metadata: Value::Null,
        });
    }
    events.push(AgentEvent::End {
        finish_reason: if has_tools {
            FinishReason::ToolUse
        } else {
            FinishReason::Stop
        },
        usage: None,
        metadata: Value::Null,
    });
    events
}

type ModelStep = Box<dyn FnOnce(ChatRequest) -> Vec<AgentEvent> + Send>;

struct ControlledModel {
    steps: Mutex<VecDeque<ModelStep>>,
    calls: AtomicUsize,
    info: ModelInfo,
}

impl ControlledModel {
    fn new(steps: Vec<ModelStep>) -> Self {
        Self {
            steps: Mutex::new(steps.into()),
            calls: AtomicUsize::new(0),
            info: react_agent::test_support::MockChatModel::text_response("unused")
                .model_info()
                .clone(),
        }
    }
}

#[async_trait]
impl ChatModel for ControlledModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only fixture")
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra model decision");
        Box::pin(futures::stream::iter(step(request).into_iter().map(Ok)))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

fn instructions_section(request: &ChatRequest) -> Option<String> {
    request
        .messages
        .iter()
        .find(|message| {
            message
                .metadata
                .get(react_agent::MODEL_CALL_CONTEXT_SECTION_METADATA_KEY)
                .and_then(Value::as_str)
                == Some("project_instructions")
        })
        .map(ModelDialogueItem::text_content)
}

#[tokio::test]
async fn new_scope_returns_to_model_before_affected_parallel_tools_execute() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let instructions = Arc::new(Instructions::default());
    let before_redecision = writes.clone();
    let after_redecision = writes.clone();
    let model = Arc::new(ControlledModel::new(vec![
        Box::new(|request| {
            assert!(
                instructions_section(&request).is_none(),
                "a stale section must not survive an empty snapshot"
            );
            response(vec![
                call("old-1", "/new/one", "old"),
                call("old-2", "/new/two", "old"),
                call("independent", "/other/file", "independent"),
            ])
        }),
        Box::new(move |request| {
            assert_eq!(
                *before_redecision.lock().unwrap(),
                vec![json!({"path":"/other/file","value":"independent"})],
                "affected tools ran before the model saw their instructions"
            );
            let section = instructions_section(&request)
                .expect("new instructions must reach the next model call");
            assert!(section.contains("/new/AGENTS.md") && section.contains("revised value"));
            let results = request
                .messages
                .iter()
                .flat_map(|message| &message.content)
                .filter_map(|part| part.as_tool_result())
                .filter(|result| {
                    result
                        .metadata
                        .get("execution_status")
                        .and_then(Value::as_str)
                        == Some("not_executed")
                })
                .collect::<Vec<_>>();
            assert_eq!(results.len(), 2);
            assert!(results.iter().all(
                |result| !result.is_error && result.text_content().contains("was not executed")
            ));
            response(vec![call("revised", "/new/revised", "revised")])
        }),
        Box::new(move |_| {
            assert_eq!(
                *after_redecision.lock().unwrap(),
                vec![
                    json!({"path":"/other/file","value":"independent"}),
                    json!({"path":"/new/revised","value":"revised"})
                ]
            );
            response(vec![])
        }),
    ]));
    let ctx = AgentContext::new(
        model.clone(),
        Arc::new(toolkit(writes.clone())),
        ReactConfig {
            parallel_tool_calls: true,
            max_parallel_tools: 3,
            max_iterations: 5,
            ..Default::default()
        },
        Arc::new(NoopHook),
        ToolContext {
            agents_instructions: Some(instructions.clone()),
            ..Default::default()
        },
    )
    .with_tool_observer_boundary(Arc::new(ObservedCalls(observed.clone())))
    .with_model_call_sections(vec![ModelCallSection::new(
        ModelCallSectionKind::ProjectInstructions,
        "stale prior Turn instructions",
    )]);
    let events = ReactAgent::new(ctx)
        .run_stream(vec![ModelDialogueItem::text(
            Role::User,
            "update the files",
        )])
        .collect::<Vec<_>>()
        .await;
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TextDelta { delta, .. } if delta == "done")));
    assert_eq!(model.calls.load(Ordering::SeqCst), 3);
    assert_eq!(instructions.discovery_count.load(Ordering::SeqCst), 1);
    assert_eq!(writes.lock().unwrap().len(), 2);
    assert_eq!(*observed.lock().unwrap(), vec!["independent", "revised"]);
}

#[tokio::test]
async fn unavailable_instructions_do_not_block_execution_or_add_a_decision_round() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let inspected = writes.clone();
    let model = Arc::new(ControlledModel::new(vec![
        Box::new(|_| response(vec![call("allowed", "/new/file", "requested")])),
        Box::new(move |request| {
            assert!(instructions_section(&request).is_none());
            assert_eq!(inspected.lock().unwrap().len(), 1);
            response(vec![])
        }),
    ]));
    let ctx = AgentContext::new(
        model.clone(),
        Arc::new(toolkit(writes)),
        ReactConfig::default(),
        Arc::new(NoopHook),
        ToolContext {
            agents_instructions: Some(Arc::new(Instructions {
                unreadable: true,
                ..Default::default()
            })),
            ..Default::default()
        },
    );
    let events = ReactAgent::new(ctx)
        .run_stream(vec![ModelDialogueItem::text(Role::User, "write")])
        .collect::<Vec<_>>()
        .await;
    assert!(events
        .iter()
        .any(|event| matches!(event, AgentEvent::TextDelta { delta, .. } if delta == "done")));
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
}

struct MutatingBoundary;

#[async_trait]
impl ToolBeforeBoundary for MutatingBoundary {
    async fn apply(
        &self,
        mut invocation: ToolBeforeInvocation,
        _cancellation: CancellationToken,
    ) -> Result<ToolBeforeOutcome, ToolBeforeFailure> {
        invocation.call.arguments["path"] = "/new/from-hook".into();
        Ok(ToolBeforeOutcome::Continue(Box::new(invocation.call)))
    }
}

struct CountingHook(Arc<AtomicUsize>);

#[async_trait]
impl AgentHook for CountingHook {
    async fn before_tool_call(
        &self,
        _call: &ToolCall,
        _iteration: u32,
    ) -> Result<ToolDecision, AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolDecision::Allow)
    }
}

#[tokio::test]
async fn discovery_uses_hook_modified_paths_before_security_and_execution() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let security_calls = Arc::new(AtomicUsize::new(0));
    let hook: Arc<dyn AgentHook> = Arc::new(CountingHook(security_calls.clone()));
    let boundary: Arc<dyn ToolBeforeBoundary> = Arc::new(MutatingBoundary);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let result = execute_single_tool_with_product_boundary(
        &call("candidate", "/other/file", "value"),
        &toolkit(writes.clone()),
        &hook,
        &ToolContext {
            agents_instructions: Some(Arc::new(Instructions::default())),
            ..Default::default()
        },
        &ToolDisclosureConfig::default(),
        0,
        &CancellationToken::new(),
        Duration::ZERO,
        &tx,
        Some(&boundary),
    )
    .await
    .unwrap();
    assert_eq!(result.metadata["execution_status"], "not_executed");
    assert!(writes.lock().unwrap().is_empty());
    assert_eq!(security_calls.load(Ordering::SeqCst), 0);
}

struct ApprovalMutation;

#[async_trait]
impl AgentHook for ApprovalMutation {
    async fn before_tool_call(
        &self,
        _call: &ToolCall,
        _iteration: u32,
    ) -> Result<ToolDecision, AgentError> {
        Ok(ToolDecision::RequiresApproval {
            request_id: "approval".into(),
            prompt: "fixture".into(),
        })
    }

    async fn wait_for_approval(
        &self,
        _tool_use_id: &str,
        _tool_name: &str,
        _prompt: &str,
        _call: &ToolCall,
    ) -> ApprovalResult {
        ApprovalResult::AllowWithModifiedArgs {
            args: json!({"path":"/new/from-approval","value":"approved"}),
        }
    }
}

#[tokio::test]
async fn approval_modified_paths_are_discovered_before_execution() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let hook: Arc<dyn AgentHook> = Arc::new(ApprovalMutation);
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let result = execute_single_tool(
        &call("candidate", "/other/file", "value"),
        &toolkit(writes.clone()),
        &hook,
        &ToolContext {
            agents_instructions: Some(Arc::new(Instructions::default())),
            ..Default::default()
        },
        &ToolDisclosureConfig::default(),
        0,
        &CancellationToken::new(),
        Duration::ZERO,
        &tx,
    )
    .await
    .unwrap();
    assert_eq!(result.metadata["execution_status"], "not_executed");
    assert!(writes.lock().unwrap().is_empty());
}

struct PendingInstructions(tokio::sync::Notify);

#[async_trait]
impl AgentsInstructions for PendingInstructions {
    async fn before_targets(
        &self,
        _targets: &[AgentsInstructionTarget],
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> rua_core::Result<AgentsInstructionDecision> {
        unreachable!("this fixture exercises explicit ToolCalls")
    }

    fn snapshot_for_model(&self) -> Option<String> {
        None
    }

    async fn before_tool(
        &self,
        _call: &ToolCall,
        _ctx: &ToolContext,
        _cancel: CancellationToken,
    ) -> rua_core::Result<AgentsInstructionDecision> {
        self.0.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn cancellation_during_instruction_discovery_never_starts_the_tool() {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let instructions = Arc::new(PendingInstructions(tokio::sync::Notify::new()));
    let cancel = CancellationToken::new();
    let operation = tokio::spawn({
        let instructions = instructions.clone();
        let writes = writes.clone();
        let cancel = cancel.clone();
        async move {
            let hook: Arc<dyn AgentHook> = Arc::new(NoopHook);
            let (tx, _rx) = tokio::sync::mpsc::channel(8);
            execute_single_tool(
                &call("pending", "/new/file", "value"),
                &toolkit(writes),
                &hook,
                &ToolContext {
                    agents_instructions: Some(instructions),
                    ..Default::default()
                },
                &ToolDisclosureConfig::default(),
                0,
                &cancel,
                Duration::ZERO,
                &tx,
            )
            .await
        }
    });
    instructions.0.notified().await;
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), operation)
            .await
            .unwrap()
            .unwrap(),
        Err(AgentError::Cancelled)
    ));
    assert!(writes.lock().unwrap().is_empty());
}
