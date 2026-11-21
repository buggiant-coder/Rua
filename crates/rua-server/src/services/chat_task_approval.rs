//! A Task's existing Approval Authority remains on its execution Device. The
//! owning Chat relays user decisions to that authority without reviving a Turn.
use std::sync::Arc;

use rua_security::approval_authority::{
    ApprovalAuthority, ApprovalDecision, ApprovalRequest, ApprovalSnapshot,
};
use rua_task::{TaskAttempt, TaskOwner, TaskRecord, TaskRuntime, TaskState};

use super::device_command_bus::ConversationApprovalDecision;

async fn current_task(
    runtime: &Arc<TaskRuntime>,
    owner: &TaskOwner,
    task_id: &str,
    attempt_no: u32,
) -> Result<TaskRecord, String> {
    let record = runtime
        .get(owner, task_id)
        .await
        .map_err(|error| error.to_string())?;
    if record.attempt_no != attempt_no
        || attempt_no == 0
        || !record
            .attempts
            .iter()
            .any(|attempt| attempt.attempt_no == attempt_no)
    {
        return Err("Task approval belongs to a different Attempt".into());
    }
    Ok(record)
}

fn belongs_to_attempt(
    request: &ApprovalRequest,
    record: &TaskRecord,
    attempt: &TaskAttempt,
) -> bool {
    request.owner.as_chat_address() == Some(&record.spec.owner.chat_address)
        && request.agent_id == attempt.authorization_agent_id
        && request.turn_id
            == attempt
                .authorization_origin
                .turn_id
                .as_deref()
                .unwrap_or_default()
        && request.tool_call_id
            == format!(
                "task:{}:attempt:{}:tool",
                record.task_id, attempt.attempt_no
            )
}

pub async fn target_snapshot(
    runtime: &Arc<TaskRuntime>,
    approvals: &Arc<ApprovalAuthority>,
    owner: &TaskOwner,
    task_id: &str,
    attempt_no: u32,
) -> Result<ApprovalSnapshot, String> {
    let record = current_task(runtime, owner, task_id, attempt_no).await?;
    let attempt = record
        .attempts
        .iter()
        .find(|attempt| attempt.attempt_no == attempt_no)
        .unwrap();
    let mut snapshot = approvals.snapshot_at(&owner.chat_address).await;
    snapshot.pending.retain(|request| {
        record.state == TaskState::WaitingApproval && belongs_to_attempt(request, &record, attempt)
    });
    snapshot
        .resolved
        .retain(|request| belongs_to_attempt(request, &record, attempt));
    Ok(snapshot)
}

#[allow(clippy::too_many_arguments)]
pub async fn target_decide(
    runtime: &Arc<TaskRuntime>,
    approvals: &Arc<ApprovalAuthority>,
    owner: &TaskOwner,
    task_id: &str,
    attempt_no: u32,
    approval_id: &str,
    decision: ConversationApprovalDecision,
    authenticated_user_id: &str,
) -> Result<ApprovalRequest, String> {
    if authenticated_user_id.trim().is_empty() {
        return Err("A logged-in user must decide the Task approval".into());
    }
    let record = current_task(runtime, owner, task_id, attempt_no).await?;
    if record.state != TaskState::WaitingApproval {
        return Err("Task is no longer awaiting this approval".into());
    }
    let attempt = record
        .attempts
        .iter()
        .find(|attempt| attempt.attempt_no == attempt_no)
        .unwrap();
    let request = approvals
        .request(approval_id)
        .await
        .ok_or("Task approval not found")?;
    if !belongs_to_attempt(&request, &record, attempt) {
        return Err("Approval does not belong to this Chat and Task Attempt".into());
    }
    let decision = match decision {
        ConversationApprovalDecision::RunOnce { decided_by } => {
            ApprovalDecision::RunOnce { decided_by }
        }
        ConversationApprovalDecision::AllowChat {
            decided_by,
            matcher,
        } => ApprovalDecision::AllowChat {
            decided_by,
            matcher,
        },
        ConversationApprovalDecision::AllowAgent {
            decided_by,
            matcher,
        } => ApprovalDecision::AllowAgent {
            decided_by,
            matcher,
        },
        ConversationApprovalDecision::Reject { decided_by } => {
            ApprovalDecision::Reject { decided_by }
        }
    };
    approvals
        .decide_for_authenticated_user(
            &request.agent_id,
            approval_id,
            decision,
            authenticated_user_id,
        )
        .await
        .map_err(|error| error.to_string())
}

pub async fn target_apply_authority_decision(
    runtime: &Arc<TaskRuntime>,
    approvals: &Arc<ApprovalAuthority>,
    owner: &TaskOwner,
    task_id: &str,
    attempt_no: u32,
    original: &ApprovalRequest,
    decided: &ApprovalRequest,
) -> Result<ApprovalRequest, String> {
    let record = current_task(runtime, owner, task_id, attempt_no).await?;
    let attempt = record
        .attempts
        .iter()
        .find(|attempt| attempt.attempt_no == attempt_no)
        .unwrap();
    if !belongs_to_attempt(original, &record, attempt)
        || !belongs_to_attempt(decided, &record, attempt)
    {
        return Err("Approval does not belong to this Chat and Task Attempt".into());
    }
    let current = approvals
        .request(&original.id)
        .await
        .ok_or("Task approval not found")?;
    if record.state != TaskState::WaitingApproval && current != *decided {
        return Err("Task is no longer awaiting this approval".into());
    }
    approvals
        .apply_chat_authority_decision(original, decided)
        .await
        .map_err(|error| error.to_string())
}
