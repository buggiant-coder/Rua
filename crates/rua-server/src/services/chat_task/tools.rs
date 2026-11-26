use std::time::Duration;

use futures::StreamExt;
use rua_core::agents_instruction_discovery::AgentsInstructionTargetKind;
use rua_core::agents_instructions::{AgentsInstructionDecision, AgentsInstructionTarget};
use rua_task::{TaskListFilter, TaskOrigin};
use rua_tool::extended::task_tools::{
    compact_task, compact_task_list_item, normalize_id_list, task_owner_from_context,
    task_spec_from_context,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::super::conversation_authority::{ConversationActiveTurnPhase, ConversationDriverKind};
use super::*;

impl ChatTaskService {
    pub(super) async fn execute_tool_request(
        &self,
        name: &str,
        args: Value,
        ctx: &ToolContext,
    ) -> Result<Option<ToolResult>, String> {
        let owner = task_owner_from_context(ctx);
        match name {
            "task_create" => {
                let spec = task_spec_from_context(args, ctx).map_err(|e| e.to_string())?;
                let device = match &spec.runner {
                    rua_task::TaskRunnerSpec::Tool {
                        tool_name,
                        arguments,
                    } if tool_name == "bash" => arguments
                        .get("device_id")
                        .and_then(Value::as_str)
                        .unwrap_or(&self.local_device_id),
                    _ => &self.local_device_id,
                }
                .to_string();
                if device == self.local_device_id {
                    return Ok(None);
                }
                let invocation = self.tool_invocation(&owner, ctx).await?;
                let id = format!(
                    "chat-task-{}",
                    hex::encode(Sha256::digest(
                        serde_json::to_vec(&(
                            &self.local_device_id,
                            &owner,
                            &invocation.authorization.origin
                        ))
                        .map_err(|e| e.to_string())?
                    ))
                );
                let reply = self
                    .dispatch(
                        &device,
                        &owner,
                        ChatTaskAction::PrepareCreate {
                            task_id: id.clone(),
                            spec: Box::new(spec),
                            invocation: invocation.clone(),
                        },
                    )
                    .await?;
                let preparation = prepared(reply, &id, &device, &owner, &invocation)?;
                self.accept_tool_task(name, &device, preparation, ctx)
                    .await
                    .map(Some)
            }
            "task_retry" => {
                let id = task_id(&args)?;
                let device = self.execution_device(&owner, id).await?;
                let invocation = self.tool_invocation(&owner, ctx).await?;
                let reply = self
                    .dispatch(
                        &device,
                        &owner,
                        ChatTaskAction::PrepareRetry {
                            task_id: id.into(),
                            invocation: invocation.clone(),
                        },
                    )
                    .await?;
                let preparation = prepared(reply, id, &device, &owner, &invocation)?;
                self.accept_tool_task(name, &device, preparation, ctx)
                    .await
                    .map(Some)
            }
            "task_list" => {
                let state = args
                    .get("state")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        serde_json::from_value(value.clone())
                            .map_err(|e| format!("Invalid Task state: {e}"))
                    })
                    .transpose()?;
                let limit = optional_u64(&args, "limit")?.unwrap_or(20).clamp(1, 100) as usize;
                let offset = optional_u64(&args, "offset")?.unwrap_or(0);
                let records = self
                    .list_records(
                        &owner,
                        TaskListFilter {
                            state,
                            limit: Some(limit + 1),
                            offset,
                        },
                    )
                    .await?;
                let tasks = records
                    .iter()
                    .take(limit)
                    .map(compact_projection)
                    .collect::<Vec<_>>();
                let mut reply = json!({"tasks":tasks,"has_more":records.len()>limit});
                if records.len() > limit {
                    reply["next_offset"] = offset.saturating_add(limit as u64).into();
                }
                Ok(Some(ToolResult::text(name, reply.to_string())))
            }
            "task_await" => self.await_tools(&owner, &args, ctx).await.map(Some),
            "task_read" | "task_send_input" | "task_cancel" | "Task_Watch" | "Task_Unwatch" => {
                let device = self.execution_device(&owner, task_id(&args)?).await?;
                if device == self.local_device_id {
                    return Ok(None);
                }
                let reply = self
                    .dispatch(
                        &device,
                        &owner,
                        ChatTaskAction::Tool {
                            tool_name: name.into(),
                            arguments: args,
                            tool_call_id: ctx
                                .metadata
                                .get("tool_call_id")
                                .or_else(|| ctx.metadata.get("tool_use_id"))
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        },
                    )
                    .await?;
                match reply {
                    ChatTaskResponse::Tool { result } => Ok(Some(*result)),
                    ChatTaskResponse::Rejected { reason, detail } => {
                        Err(format!("{reason}: {detail}"))
                    }
                    _ => Err("The execution Device returned an unexpected Task tool result".into()),
                }
            }
            _ => Ok(None),
        }
    }

    async fn tool_invocation(
        &self,
        owner: &TaskOwner,
        ctx: &ToolContext,
    ) -> Result<ChatTaskInvocation, String> {
        let metadata = |key| ctx.metadata.get(key).and_then(Value::as_str);
        let turn_id = metadata("turn_id").ok_or("Task acceptance requires the originating Turn")?;
        let call_id = metadata("tool_call_id")
            .or_else(|| metadata("tool_use_id"))
            .ok_or("Task acceptance requires the originating tool call")?;
        let active = self
            .authority
            .active_turn(&owner.chat_address)
            .await
            .filter(|active| {
                active.turn_id == turn_id
                    && active.authority_device_id == self.local_device_id
                    && active.driver == ConversationDriverKind::Rua
                    && matches!(
                        active.phase,
                        ConversationActiveTurnPhase::Streaming
                            | ConversationActiveTurnPhase::RunningTool
                            | ConversationActiveTurnPhase::WaitingApproval
                    )
            })
            .ok_or("The originating Chat invocation is no longer active")?;
        Ok(ChatTaskInvocation {
            authorization: TaskAttemptAuthorization::new(ctx.agent_id.clone())?.with_origin(
                TaskOrigin {
                    turn_id: Some(turn_id.into()),
                    tool_call_id: Some(call_id.into()),
                },
            ),
            authority_device_id: active.authority_device_id,
            authority_epoch: active.authority_epoch,
            extra: Default::default(),
        })
    }

    async fn accept_tool_task(
        &self,
        name: &str,
        device: &str,
        preparation: ChatTaskPreparation,
        ctx: &ToolContext,
    ) -> Result<ToolResult, String> {
        if let Some(instructions) = &ctx.agents_instructions {
            let cancel = CancellationToken::new();
            let _guard = cancel.clone().drop_guard();
            let targets = [AgentsInstructionTarget {
                device_id: device.into(),
                path: preparation
                    .device
                    .public_cwd
                    .clone()
                    .ok_or("Task has no resolved execution directory")?,
                kind: AgentsInstructionTargetKind::Directory,
            }];
            let decision = tokio::select! {
                _ = cancelled(ctx) => { cancel.cancel(); return Err("Task cancelled before acceptance".into()); }
                decision = instructions.before_targets(&targets, ctx, cancel.clone()) => decision,
            };
            match decision {
                Ok(AgentsInstructionDecision::Reconsider { reason }) => {
                    let mut result = ToolResult::text(name, format!("Task was not accepted. {reason} Review the newly loaded AGENTS.md before issuing another tool call."));
                    result
                        .metadata
                        .insert("execution_status".into(), "not_executed".into());
                    result
                        .metadata
                        .insert("agents_instructions".into(), "reconsider".into());
                    return Ok(result);
                }
                Err(rua_core::RuaError::Cancelled) => {
                    return Err("Task cancelled before acceptance".into())
                }
                _ => {}
            }
        }
        if ctx.cancel.as_ref().is_some_and(|cancel| *cancel.borrow()) {
            return Err("Task cancelled before acceptance".into());
        }
        // Keep the reference transaction and the single acceptance request
        // together even if a Tool timeout drops its caller. Before dispatch,
        // that drop forbids new work; after dispatch, only query the same ID.
        let caller_dropped = CancellationToken::new();
        let caller_guard = caller_dropped.clone().drop_guard();
        let service = self.clone();
        let context = ctx.clone();
        let name = name.to_string();
        let device = device.to_string();
        let result = tokio::spawn(async move {
            service
                .accept_prepared_tool_task(&name, &device, preparation, &context, &caller_dropped)
                .await
        })
        .await
        .map_err(|error| format!("Task acceptance is unconfirmed: {error}"))?;
        caller_guard.disarm();
        result
    }

    async fn accept_prepared_tool_task(
        &self,
        name: &str,
        device: &str,
        preparation: ChatTaskPreparation,
        ctx: &ToolContext,
        caller_dropped: &CancellationToken,
    ) -> Result<ToolResult, String> {
        let owner = &preparation.spec.owner;
        let initial_invocation = self.tool_invocation(owner, ctx).await?;
        if caller_dropped.is_cancelled()
            || ctx.cancel.as_ref().is_some_and(|cancel| *cancel.borrow())
            || initial_invocation != preparation.invocation
        {
            return Err("Task invocation changed before acceptance".into());
        }
        if preparation.expected_attempt_no.is_none() {
            self.remember_reference(&ChatTaskRef {
                task_id: preparation.task_id.clone(),
                owner: owner.clone(),
                execution: ChatTaskExecution {
                    device_id: device.into(),
                    public_cwd: preparation
                        .device
                        .public_cwd
                        .clone()
                        .ok_or("Task has no cwd")?,
                    extra: Default::default(),
                },
                origin: ChatTaskOrigin {
                    turn_id: preparation.spec.origin.turn_id.clone(),
                    tool_call_id: preparation.spec.origin.tool_call_id.clone(),
                    agent_id: preparation.invocation.authorization.agent_id.clone(),
                    extra: Default::default(),
                },
                last_target_evidence: None,
                not_accepted: false,
                extra: Default::default(),
            })
            .await?;
        }
        // Recheck after discovery: a stopped Turn may not create new work. Once
        // acceptance starts, only the Task's own control can stop accepted work.
        let current_invocation = self.tool_invocation(owner, ctx).await;
        if caller_dropped.is_cancelled()
            || ctx.cancel.as_ref().is_some_and(|cancel| *cancel.borrow())
            || current_invocation.as_ref() != Ok(&preparation.invocation)
        {
            if preparation.expected_attempt_no.is_none() {
                self.mark_not_accepted(owner, &preparation.task_id).await?;
            }
            return Err("Task invocation ended or changed before acceptance".into());
        }
        let action = if preparation.expected_attempt_no.is_some() {
            ChatTaskAction::Retry {
                preparation: Box::new(preparation.clone()),
            }
        } else {
            ChatTaskAction::Create {
                preparation: Box::new(preparation.clone()),
            }
        };
        let mut reply = self.dispatch(device, owner, action).await;
        for check in 0..4 {
            match reply {
                Ok(ChatTaskResponse::Record { record, reference })
                    if accepted_record(&record, &preparation, device) =>
                {
                    let persistence = match reference {
                        Some(reference)
                            if reference.task_id == record.task_id && reference.owner == *owner =>
                        {
                            self.remember_reference(&reference).await
                        }
                        _ => self.remember_task(&record).await,
                    };
                    let mut result = ToolResult::text(name, compact_task(&record).to_string());
                    result.metadata.insert(
                        "execution_location".into(),
                        json!({"device_id":device,"cwd":preparation.device.public_cwd}),
                    );
                    if let Err(error) = persistence {
                        result
                            .metadata
                            .insert("task_reference_error".into(), json!(error));
                    }
                    return Ok(result);
                }
                Ok(ChatTaskResponse::Rejected { reason, detail })
                    if check == 0
                        && matches!(reason.as_str(), "task_invalid" | "task_not_accepted") =>
                {
                    if preparation.expected_attempt_no.is_none() {
                        self.mark_not_accepted(owner, &preparation.task_id).await?;
                    }
                    return Err(format!("{reason}: {detail}"));
                }
                _ => {}
            }
            if check < 3 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                reply = self
                    .dispatch(
                        device,
                        owner,
                        ChatTaskAction::Get {
                            task_id: preparation.task_id.clone(),
                        },
                    )
                    .await;
            }
        }
        let mut result = ToolResult::text(name, json!({
            "task_id":preparation.task_id,"availability":"unknown","acceptance":"unconfirmed",
            "message":"The execution Device has not confirmed acceptance. Query this same Task ID; do not submit the command again."
        }).to_string());
        result
            .metadata
            .insert("execution_status".into(), "unconfirmed".into());
        Ok(result)
    }

    async fn await_tools(
        &self,
        owner: &TaskOwner,
        args: &Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, String> {
        let ids = normalize_id_list(&args["task_ids"]);
        if ids.is_empty() {
            return Ok(ToolResult::text(
                "task_await",
                "Error: No `task_ids` provided.",
            ));
        }
        let seconds = optional_u64(args, "timeout")?
            .filter(|seconds| *seconds > 0)
            .unwrap_or(30);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
        let refs = self.references(owner)?;
        let mut tasks = BTreeMap::new();
        let mut routes = Vec::new();
        for id in &ids {
            let device = self.execution_device(owner, id).await?;
            let mut value = refs
                .iter()
                .find(|reference| reference.task_id == *id)
                .and_then(|reference| reference.last_target_evidence.as_ref())
                .map(|evidence| compact_task(&evidence.record))
                .unwrap_or_else(|| json!({"task_id":id}));
            value["availability"] = "unknown".into();
            tasks.insert(id.clone(), value);
            routes.push((id.clone(), device));
        }
        let mut timed_out = true;
        loop {
            let reads =
                futures::stream::iter(routes.clone().into_iter().map(|(id, device)| async move {
                    (
                        id.clone(),
                        self.dispatch(
                            &device,
                            owner,
                            ChatTaskAction::Get {
                                task_id: id.clone(),
                            },
                        )
                        .await,
                    )
                }))
                .buffer_unordered(8);
            tokio::pin!(reads);
            let complete = loop {
                tokio::select! {
                    _ = cancelled(ctx) => return Err("Task wait cancelled; underlying Tasks continue running".into()),
                    _ = tokio::time::sleep_until(deadline) => break false,
                    next = reads.next() => {
                        let Some((id, reply)) = next else { break true; };
                        if let Ok(ChatTaskResponse::Record { record, reference }) = reply {
                            if record.task_id == id && record.spec.owner == *owner {
                                let mut value = compact_task(&record);
                                value["availability"] = "available".into();
                                tasks.insert(id.clone(),value);
                                if let Some(reference) = reference {
                                    tokio::select! {
                                        _ = cancelled(ctx) => return Err("Task wait cancelled; underlying Tasks continue running".into()),
                                        _ = tokio::time::sleep_until(deadline) => break false,
                                        _ = self.remember_reference(&reference) => {},
                                    }
                                }
                            }
                        } else if let Some(value) = tasks.get_mut(&id) { value["availability"] = "unknown".into(); }
                    }
                }
            };
            if !complete {
                break;
            }
            if tasks.values().all(|value| {
                value["availability"] == "available"
                    && serde_json::from_value::<rua_task::TaskState>(value["status"].clone())
                        .is_ok_and(|state| state.is_terminal())
            }) {
                timed_out = false;
                break;
            }
            tokio::select! {
                _ = cancelled(ctx) => return Err("Task wait cancelled; underlying Tasks continue running".into()),
                _ = tokio::time::sleep_until(deadline) => break,
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
        }
        Ok(ToolResult::text("task_await", json!({"timed_out":timed_out,"tasks":ids.iter().filter_map(|id| tasks.get(id)).collect::<Vec<_>>()}).to_string()))
    }
}

fn task_id(args: &Value) -> Result<&str, String> {
    args.get("task_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Task ID is required".into())
}
fn optional_u64(args: &Value, key: &str) -> Result<Option<u64>, String> {
    args.get(key)
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| format!("{key} must be a nonnegative integer"))
        })
        .transpose()
}
fn prepared(
    reply: ChatTaskResponse,
    id: &str,
    device: &str,
    owner: &TaskOwner,
    invocation: &ChatTaskInvocation,
) -> Result<ChatTaskPreparation, String> {
    match reply {
        ChatTaskResponse::Prepared { preparation }
            if preparation.task_id == id
                && preparation.spec.owner == *owner
                && preparation.device.device_id.as_deref() == Some(device)
                && preparation.invocation == *invocation =>
        {
            Ok(*preparation)
        }
        ChatTaskResponse::Rejected { reason, detail } => Err(format!("{reason}: {detail}")),
        _ => Err("The execution Device returned a different Task preparation".into()),
    }
}
fn accepted_record(record: &TaskRecord, preparation: &ChatTaskPreparation, device: &str) -> bool {
    record.task_id == preparation.task_id
        && record.spec == preparation.spec
        && record.attempt_no
            == preparation
                .expected_attempt_no
                .map_or(1, |attempt| attempt.saturating_add(1))
        && record.attempts.iter().any(|attempt| {
            attempt.attempt_no == record.attempt_no
                && attempt.device.device_id.as_deref() == Some(device)
                && attempt.device.public_cwd == preparation.device.public_cwd
                && attempt.authorization_origin == preparation.invocation.authorization.origin
                && attempt.authorization_agent_id == preparation.invocation.authorization.agent_id
        })
}
fn compact_projection(value: &Value) -> Value {
    let Ok(record) = serde_json::from_value::<TaskRecord>(value.clone()) else {
        return value.clone();
    };
    let mut result = compact_task_list_item(&record);
    for key in ["availability", "execution", "execution_location"] {
        if let Some(value) = value.get(key) {
            result[key] = value.clone();
        }
    }
    result
}
async fn cancelled(ctx: &ToolContext) {
    match ctx.cancel.clone() {
        Some(mut cancel) => {
            if *cancel.borrow() {
                return;
            }
            let _ = cancel.wait_for(|value| *value).await;
        }
        None => std::future::pending::<()>().await,
    }
}
