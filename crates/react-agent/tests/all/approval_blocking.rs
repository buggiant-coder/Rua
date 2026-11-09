use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_types::{
    AgentEvent, ModelDialogueItem, Role, ToolAnnotations, ToolCall, ToolDefinition, ToolResult,
};
use async_trait::async_trait;
use futures::StreamExt;
use react_agent::hooks::{AgentHook, ApprovalResult};
use react_agent::test_support::MockChatModel;
use react_agent::toolkit::{AgentToolkit, MockToolExecutor, ToolExecutor};
use react_agent::{ReactAgent, ReactConfig, ToolDecision};
use rua_security::approval_authority::{
    ApprovalAuthority, ApprovalDecision, ApprovalRequestInput, ApprovalTrigger, ToolCallSnapshot,
};

fn test_tool_ctx(session_id: &str, agent_id: &str) -> rua_core::tool::ToolContext {
    let mut meta = std::collections::HashMap::new();
    meta.insert(
        "agent_id".to_string(),
        serde_json::Value::String(agent_id.to_string()),
    );
    rua_core::tool::ToolContext {
        agent_id: agent_id.into(),
        session_id: session_id.into(),
        working_dir: std::path::PathBuf::from("/tmp"),
        metadata: meta,
        cancel: None,
        task_runtime: None,
        ..Default::default()
    }
}

fn toolkit_with_echo() -> Arc<AgentToolkit> {
    let mut toolkit = AgentToolkit::new();
    let def = ToolDefinition {
        name: "echo_tool".into(),
        description: "echoes back".into(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
        strict: None,
        annotations: parallel_safe_annotations(),
        metadata: Default::default(),
    };
    toolkit.register(def, Arc::new(MockToolExecutor::new("echo_ok")));
    Arc::new(toolkit)
}

fn parallel_safe_annotations() -> ToolAnnotations {
    ToolAnnotations {
        read_only: Some(true),
        destructive: Some(false),
        idempotent: Some(true),
        requires_approval: Some(false),
        ..Default::default()
    }
}

struct CountingToolExecutor {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolExecutor for CountingToolExecutor {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::text(call.id.clone(), "counted"))
    }
}

fn toolkit_with_counting_tool(calls: Arc<AtomicUsize>) -> Arc<AgentToolkit> {
    let mut toolkit = AgentToolkit::new();
    let def = ToolDefinition {
        name: "echo_tool".into(),
        description: "increments a counter".into(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
        strict: None,
        annotations: Default::default(),
        metadata: Default::default(),
    };
    toolkit.register(def, Arc::new(CountingToolExecutor { calls }));
    Arc::new(toolkit)
}

fn one_echo_tool_then_text(final_text: &str) -> MockChatModel {
    MockChatModel::from_sequences(vec![
        vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::ToolCallDelta {
                id: "tc-first".into(),
                index: 0,
                name: Some("echo_tool".into()),
                arguments_delta: serde_json::json!({}).to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: agent_types::FinishReason::ToolUse,
                usage: None,
                metadata: serde_json::Value::Null,
            },
        ],
        vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: final_text.into(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: agent_types::FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            },
        ],
    ])
}

fn two_echo_tools_then_text(final_text: &str) -> MockChatModel {
    MockChatModel::from_sequences(vec![
        vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::ToolCallDelta {
                id: "tc-first".into(),
                index: 0,
                name: Some("echo_tool".into()),
                arguments_delta: serde_json::json!({}).to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::ToolCallDelta {
                id: "tc-second".into(),
                index: 1,
                name: Some("echo_tool".into()),
                arguments_delta: serde_json::json!({}).to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: agent_types::FinishReason::ToolUse,
                usage: None,
                metadata: serde_json::Value::Null,
            },
        ],
        vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: final_text.into(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: agent_types::FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            },
        ],
    ])
}

fn two_approval_iterations_then_text(final_text: &str) -> MockChatModel {
    MockChatModel::from_sequences(vec![
        vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::ToolCallDelta {
                id: "tc-first".into(),
                index: 0,
                name: Some("echo_tool".into()),
                arguments_delta: serde_json::json!({}).to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: agent_types::FinishReason::ToolUse,
                usage: None,
                metadata: serde_json::Value::Null,
            },
        ],
        vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::ToolCallDelta {
                id: "tc-second-round".into(),
                index: 0,
                name: Some("echo_tool".into()),
                arguments_delta: serde_json::json!({}).to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: agent_types::FinishReason::ToolUse,
                usage: None,
                metadata: serde_json::Value::Null,
            },
        ],
        vec![
            AgentEvent::Start {
                run_id: "mock-run".into(),
                model: Some("mock-model".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: final_text.into(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: agent_types::FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            },
        ],
    ])
}

struct BlockingAuthorityHook {
    authority: Arc<ApprovalAuthority>,
    agent_id: String,
    chat_id: String,
}

impl BlockingAuthorityHook {
    fn new(authority: Arc<ApprovalAuthority>, agent_id: &str, chat_id: &str) -> Self {
        Self {
            authority,
            agent_id: agent_id.into(),
            chat_id: chat_id.into(),
        }
    }
}

#[async_trait]
impl AgentHook for BlockingAuthorityHook {
    async fn before_tool_call(
        &self,
        call: &ToolCall,
        _iteration: u32,
    ) -> Result<ToolDecision, react_agent::AgentError> {
        let request = self
            .authority
            .create_request(ApprovalRequestInput {
                owner: rua_security::ApprovalOwner::chat(rua_core::ChatAddress::agent(
                    &self.agent_id,
                    &self.chat_id,
                )),
                agent_id: self.agent_id.clone(),
                turn_id: self.chat_id.clone(),
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                trigger: ApprovalTrigger::approval_mode("approval required"),
                tool_call: ToolCallSnapshot {
                    display: format!("{} {}", call.name, call.arguments),
                    args: call.arguments.clone(),
                },
            })
            .await;
        Ok(ToolDecision::RequiresApproval {
            request_id: request.id,
            prompt: "approval required".into(),
        })
    }

    async fn wait_for_approval(
        &self,
        tool_use_id: &str,
        _tool_name: &str,
        _prompt: &str,
        _call: &ToolCall,
    ) -> ApprovalResult {
        let snapshot = self
            .authority
            .snapshot_at(&rua_core::ChatAddress::agent(&self.agent_id, &self.chat_id))
            .await;
        let request = snapshot
            .pending
            .into_iter()
            .find(|request| request.tool_call_id == tool_use_id)
            .expect("approval request for tool");
        match self.authority.wait_for_decision(&request.id).await {
            Ok(decided) if decided.status == rua_security::AuthorityStatus::Approved => {
                ApprovalResult::Allow
            }
            Ok(_) => ApprovalResult::Deny {
                reason: "rejected by the user".into(),
            },
            Err(err) => ApprovalResult::Deny {
                reason: err.to_string(),
            },
        }
    }
}

async fn next_pending_request_id(stream: &mut react_agent::AgentEventStream) -> (String, String) {
    while let Some(event) = stream.next().await {
        if let AgentEvent::ToolApprovalPending {
            call_id,
            request_id: Some(request_id),
            ..
        } = event
        {
            return (call_id, request_id);
        }
    }
    panic!("stream ended before approval pending event");
}

async fn assert_no_text_delta_within(stream: &mut react_agent::AgentEventStream, text: &str) {
    let deadline = tokio::time::sleep(Duration::from_millis(80));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return,
            event = stream.next() => match event {
                Some(AgentEvent::TextDelta { delta, .. }) if delta.contains(text) => {
                    panic!("agent continued before approvals settled: {delta}");
                }
                Some(_) => {}
                None => return,
            }
        }
    }
}

async fn collect_until_text(stream: &mut react_agent::AgentEventStream, text: &str) {
    while let Some(event) = stream.next().await {
        if let AgentEvent::TextDelta { delta, .. } = event {
            if delta.contains(text) {
                return;
            }
        }
    }
    panic!("stream ended before expected text: {text}");
}

#[tokio::test]
async fn approval_decision_unblocks_the_original_run_and_continues_llm() {
    let session_id = format!("authority-blocking-{}", uuid::Uuid::new_v4());
    let agent_id = "test-agent";
    let authority = Arc::new(ApprovalAuthority::new());
    let hook = Arc::new(BlockingAuthorityHook::new(
        Arc::clone(&authority),
        agent_id,
        &session_id,
    ));
    let agent = ReactAgent::builder(Arc::new(one_echo_tool_then_text("after approval")))
        .config(ReactConfig::default())
        .toolkit(toolkit_with_echo())
        .tool_ctx(test_tool_ctx(&session_id, agent_id))
        .approval_authority(Arc::clone(&authority))
        .hook(hook)
        .build();

    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "go")]);
    let (_call_id, request_id) = next_pending_request_id(&mut stream).await;

    assert_no_text_delta_within(&mut stream, "after approval").await;

    authority
        .decide(
            &request_id,
            ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .expect("approve request");

    collect_until_text(&mut stream, "after approval").await;
}

#[tokio::test]
async fn approval_blocks_tool_executor_until_decision_is_approved() {
    let session_id = format!("authority-exec-blocking-{}", uuid::Uuid::new_v4());
    let agent_id = "test-agent";
    let authority = Arc::new(ApprovalAuthority::new());
    let hook = Arc::new(BlockingAuthorityHook::new(
        Arc::clone(&authority),
        agent_id,
        &session_id,
    ));
    let executions = Arc::new(AtomicUsize::new(0));
    let agent = ReactAgent::builder(Arc::new(one_echo_tool_then_text("after approval")))
        .config(ReactConfig::default())
        .toolkit(toolkit_with_counting_tool(Arc::clone(&executions)))
        .tool_ctx(test_tool_ctx(&session_id, agent_id))
        .approval_authority(Arc::clone(&authority))
        .hook(hook)
        .build();

    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "go")]);
    let (_call_id, request_id) = next_pending_request_id(&mut stream).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        executions.load(Ordering::SeqCst),
        0,
        "tool executor must not run while approval is pending"
    );

    authority
        .decide(
            &request_id,
            ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .expect("approve request");

    collect_until_text(&mut stream, "after approval").await;
    assert_eq!(
        executions.load(Ordering::SeqCst),
        1,
        "tool executor should run exactly once after approval"
    );
}

#[tokio::test]
async fn approving_one_of_multiple_pending_tools_does_not_continue_the_agent() {
    let session_id = format!("authority-multi-{}", uuid::Uuid::new_v4());
    let agent_id = "test-agent";
    let authority = Arc::new(ApprovalAuthority::new());
    let hook = Arc::new(BlockingAuthorityHook::new(
        Arc::clone(&authority),
        agent_id,
        &session_id,
    ));
    let config = ReactConfig {
        parallel_tool_calls: true,
        ..Default::default()
    };
    let agent = ReactAgent::builder(Arc::new(two_echo_tools_then_text("after both approvals")))
        .config(config)
        .toolkit(toolkit_with_echo())
        .tool_ctx(test_tool_ctx(&session_id, agent_id))
        .approval_authority(Arc::clone(&authority))
        .hook(hook)
        .build();

    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "go")]);
    let (_, first_request_id) = next_pending_request_id(&mut stream).await;
    let (_, second_request_id) = next_pending_request_id(&mut stream).await;

    authority
        .decide(
            &first_request_id,
            ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .expect("approve first request");
    assert_no_text_delta_within(&mut stream, "after both approvals").await;

    authority
        .decide(
            &second_request_id,
            ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .expect("approve second request");
    collect_until_text(&mut stream, "after both approvals").await;
}

#[tokio::test]
async fn later_iteration_approval_blocks_and_resumes_the_same_run() {
    let session_id = format!("authority-later-{}", uuid::Uuid::new_v4());
    let agent_id = "test-agent";
    let authority = Arc::new(ApprovalAuthority::new());
    let hook = Arc::new(BlockingAuthorityHook::new(
        Arc::clone(&authority),
        agent_id,
        &session_id,
    ));
    let agent = ReactAgent::builder(Arc::new(two_approval_iterations_then_text(
        "after second approval",
    )))
    .config(ReactConfig::default())
    .toolkit(toolkit_with_echo())
    .tool_ctx(test_tool_ctx(&session_id, agent_id))
    .approval_authority(Arc::clone(&authority))
    .hook(hook)
    .build();

    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "go")]);
    let (_, first_request_id) = next_pending_request_id(&mut stream).await;

    authority
        .decide(
            &first_request_id,
            ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .expect("approve first request");

    let (second_call_id, second_request_id) = next_pending_request_id(&mut stream).await;
    assert_eq!(second_call_id, "tc-second-round");
    assert_no_text_delta_within(&mut stream, "after second approval").await;

    authority
        .decide(
            &second_request_id,
            ApprovalDecision::RunOnce {
                decided_by: "user".into(),
            },
        )
        .await
        .expect("approve second request");

    collect_until_text(&mut stream, "after second approval").await;
}

#[tokio::test]
async fn rejected_approval_returns_tool_result_and_continues_original_run() {
    let session_id = format!("authority-reject-{}", uuid::Uuid::new_v4());
    let agent_id = "test-agent";
    let authority = Arc::new(ApprovalAuthority::new());
    let hook = Arc::new(BlockingAuthorityHook::new(
        Arc::clone(&authority),
        agent_id,
        &session_id,
    ));
    let agent = ReactAgent::builder(Arc::new(one_echo_tool_then_text("after rejection")))
        .config(ReactConfig::default())
        .toolkit(toolkit_with_echo())
        .tool_ctx(test_tool_ctx(&session_id, agent_id))
        .approval_authority(Arc::clone(&authority))
        .hook(hook)
        .build();

    let mut stream = agent.run_stream(vec![ModelDialogueItem::text(Role::User, "go")]);
    let (_call_id, request_id) = next_pending_request_id(&mut stream).await;

    authority
        .decide(
            &request_id,
            ApprovalDecision::Reject {
                decided_by: "user".into(),
            },
        )
        .await
        .expect("reject request");

    let mut saw_rejected_tool_result = false;
    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::ToolResult { result } => {
                saw_rejected_tool_result = result.text_content().contains("rejected by the user");
            }
            AgentEvent::TextDelta { delta, .. } if delta.contains("after rejection") => {
                assert!(saw_rejected_tool_result);
                return;
            }
            _ => {}
        }
    }
    panic!("stream ended before rejected tool result and final text");
}
