use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rua_security::approval_authority::{
    ApprovalAuthority, ApprovalOwner, ApprovalRequest, ApprovalRequestInput, ApprovalStatus,
    ApprovalTrigger, ToolCallSnapshot,
};
use rua_server::services::chat_task_approval::{target_decide, target_snapshot};
use rua_server::services::device_command_bus::ConversationApprovalDecision;
use rua_task::{
    TaskAttemptAuthorization, TaskOrigin, TaskOwner, TaskRunOutput, TaskRunRequest, TaskRunner,
    TaskRunnerError, TaskRunnerKind, TaskRunnerSpec, TaskRuntime, TaskRuntimeConfig, TaskSpec,
    TaskState,
};
use serde_json::json;
use tokio::sync::Notify;

struct WaitingRunner(Notify);
#[async_trait]
impl TaskRunner for WaitingRunner {
    fn kind(&self) -> TaskRunnerKind {
        TaskRunnerKind::Tool
    }
    async fn run(&self, request: TaskRunRequest) -> Result<TaskRunOutput, TaskRunnerError> {
        self.0.notify_one();
        request.cancel.cancelled().await;
        Ok(TaskRunOutput::cancelled())
    }
}

fn authorization(agent: &str, turn: &str) -> TaskAttemptAuthorization {
    TaskAttemptAuthorization::new(agent)
        .unwrap()
        .with_origin(TaskOrigin {
            turn_id: Some(turn.into()),
            tool_call_id: Some(format!("create-{turn}")),
        })
}

async fn request(
    authority: &ApprovalAuthority,
    owner: &TaskOwner,
    task_id: &str,
    attempt: u32,
    agent: &str,
    turn: &str,
) -> ApprovalRequest {
    authority
        .create_request(ApprovalRequestInput {
            agent_id: agent.into(),
            owner: ApprovalOwner::chat(owner.chat_address.clone()),
            turn_id: turn.into(),
            tool_call_id: format!("task:{task_id}:attempt:{attempt}:tool"),
            tool_name: "bash".into(),
            trigger: ApprovalTrigger::approval_mode("Confirm background command"),
            tool_call: ToolCallSnapshot {
                display: "pwd on phone".into(),
                args: json!({"device_id":"phone","command":"pwd"}),
            },
        })
        .await
}

async fn wait_terminal(runtime: &TaskRuntime, owner: &TaskOwner, task_id: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !runtime
            .get(owner, task_id)
            .await
            .unwrap()
            .state
            .is_terminal()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn task_approval_uses_captured_attempt_without_an_active_chat_turn() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(WaitingRunner(Notify::new()));
    runtime.register_runner(runner.clone());
    let approvals = Arc::new(ApprovalAuthority::new());
    let owner = TaskOwner::new("photo-chat");
    let task_id = runtime
        .create(
            TaskSpec {
                owner: owner.clone(),
                instruction: "Inspect the phone photos".into(),
                runner: TaskRunnerSpec::Tool {
                    tool_name: "bash".into(),
                    arguments: json!({"command":"pwd"}),
                },
                context_refs: vec![],
                depends_on: vec![],
                origin: TaskOrigin {
                    turn_id: Some("finished-turn".into()),
                    tool_call_id: Some("original-call".into()),
                },
            },
            authorization("captured-agent", "finished-turn"),
        )
        .await
        .unwrap();
    runner.0.notified().await;
    runtime
        .set_waiting_approval(&owner, &task_id, 1, true)
        .await
        .unwrap();
    let card = request(
        &approvals,
        &owner,
        &task_id,
        1,
        "captured-agent",
        "finished-turn",
    )
    .await;
    let other = request(
        &approvals,
        &TaskOwner::new("different-chat"),
        &task_id,
        1,
        "captured-agent",
        "finished-turn",
    )
    .await;
    assert_eq!(
        target_snapshot(&runtime, &approvals, &owner, &task_id, 1)
            .await
            .unwrap()
            .pending,
        vec![card.clone()]
    );
    for (approval_id, user) in [(other.id.as_str(), "user"), (card.id.as_str(), "")] {
        assert!(target_decide(
            &runtime,
            &approvals,
            &owner,
            &task_id,
            1,
            approval_id,
            ConversationApprovalDecision::RunOnce {
                decided_by: "webui".into()
            },
            user
        )
        .await
        .is_err());
    }
    let resolved = target_decide(
        &runtime,
        &approvals,
        &owner,
        &task_id,
        1,
        &card.id,
        ConversationApprovalDecision::RunOnce {
            decided_by: "webui".into(),
        },
        "logged-in-user",
    )
    .await
    .unwrap();
    assert_eq!(resolved.status, ApprovalStatus::Approved);
    assert_eq!(
        resolved.authenticated_user_decision.unwrap().user_id,
        "logged-in-user"
    );
    assert_eq!(
        approvals.request(&other.id).await.unwrap().status,
        ApprovalStatus::Pending
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn retry_approval_uses_new_authorization_and_rejects_the_old_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Arc::new(TaskRuntime::open(TaskRuntimeConfig::new(dir.path())).unwrap());
    let runner = Arc::new(WaitingRunner(Notify::new()));
    runtime.register_runner(runner.clone());
    let approvals = Arc::new(ApprovalAuthority::new());
    let owner = TaskOwner::new("photo-chat");
    let original = TaskOrigin {
        turn_id: Some("first-turn".into()),
        tool_call_id: Some("first-call".into()),
    };
    let task_id = runtime
        .create(
            TaskSpec {
                owner: owner.clone(),
                instruction: "Inspect photos".into(),
                runner: TaskRunnerSpec::Tool {
                    tool_name: "bash".into(),
                    arguments: json!({"command":"pwd"}),
                },
                context_refs: vec![],
                depends_on: vec![],
                origin: original.clone(),
            },
            authorization("first-agent", "first-turn"),
        )
        .await
        .unwrap();
    runner.0.notified().await;
    runtime
        .set_waiting_approval(&owner, &task_id, 1, true)
        .await
        .unwrap();
    let old = request(&approvals, &owner, &task_id, 1, "first-agent", "first-turn").await;
    runtime.cancel(&owner, &task_id).await.unwrap();
    wait_terminal(&runtime, &owner, &task_id).await;
    runtime
        .retry(&owner, &task_id, authorization("next-agent", "retry-turn"))
        .await
        .unwrap();
    runner.0.notified().await;
    runtime
        .set_waiting_approval(&owner, &task_id, 2, true)
        .await
        .unwrap();
    let current = request(&approvals, &owner, &task_id, 2, "next-agent", "retry-turn").await;
    assert!(target_snapshot(&runtime, &approvals, &owner, &task_id, 1)
        .await
        .is_err());
    assert_eq!(
        target_snapshot(&runtime, &approvals, &owner, &task_id, 2)
            .await
            .unwrap()
            .pending,
        vec![current.clone()]
    );
    assert!(target_decide(
        &runtime,
        &approvals,
        &owner,
        &task_id,
        2,
        &old.id,
        ConversationApprovalDecision::RunOnce {
            decided_by: "webui".into()
        },
        "user"
    )
    .await
    .is_err());
    let denied = target_decide(
        &runtime,
        &approvals,
        &owner,
        &task_id,
        2,
        &current.id,
        ConversationApprovalDecision::Reject {
            decided_by: "webui".into(),
        },
        "user",
    )
    .await
    .unwrap();
    assert_eq!(denied.status, ApprovalStatus::Rejected);
    let record = runtime.get(&owner, &task_id).await.unwrap();
    assert_eq!(record.spec.origin, original);
    assert_eq!(record.state, TaskState::WaitingApproval);
    runtime.shutdown().await.unwrap();
}
