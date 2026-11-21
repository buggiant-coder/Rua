use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use rua_core::agents_instruction_discovery::AgentsInstructionTargetKind;
use rua_core::agents_instructions::{AgentsInstructionDecision, AgentsInstructionTarget};
use rua_core::tool::{DeviceReadCheck, ToolContext, ToolResult};
use rua_device_network::DeviceSessionCommandChannel;
use rua_security::approval_authority::{ApprovalStatus, AuthenticatedApprovalDecision};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::chat_command::{
    ChatCommandAction, ChatCommandCommand, ChatCommandContext, ChatCommandPreparation,
    ChatCommandResponse, ChatCommandSnapshot, ChatCommandState,
};
use super::conversation_authority::{
    ConversationActiveTurnPhase, ConversationAuthority, ConversationDriverKind,
};
use super::device_command_bus::{
    deliver_chat_command_via_channel, deliver_chat_command_via_session,
};
use super::device_context::current_sync_device_id;
use super::device_read_check::{
    DeviceReadCheckReply, DeviceReadCheckRequest, DeviceReadConfirmation,
};
use super::device_session_access::{current_session_manager, DeviceNetworkAccess};

const RPC_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone)]
pub struct ChatCommandToolService {
    sync_enabled: Arc<AtomicBool>,
    sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    network: Arc<DeviceNetworkAccess>,
    authority: Arc<ConversationAuthority>,
    approvals: Arc<rua_security::ApprovalAuthority>,
    tasks: Arc<OnceLock<Arc<super::chat_task::ChatTaskService>>>,
}

impl ChatCommandToolService {
    pub fn new(
        sync_enabled: Arc<AtomicBool>,
        sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
        network: Arc<DeviceNetworkAccess>,
        authority: Arc<ConversationAuthority>,
        approvals: Arc<rua_security::ApprovalAuthority>,
        tasks: Arc<OnceLock<Arc<super::chat_task::ChatTaskService>>>,
    ) -> Self {
        Self {
            sync_enabled,
            sync_device,
            network,
            authority,
            approvals,
            tasks,
        }
    }

    async fn execute(&self, arguments: Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let local = current_sync_device_id(&self.sync_device).ok_or("Local Device unavailable")?;
        let target = arguments["device_id"]
            .as_str()
            .ok_or("Missing execution Device")?;
        if !self.sync_enabled.load(Ordering::SeqCst) {
            return Err("Multi-device sync is disabled".into());
        }
        if target == local
            || ctx.local_device_id.as_deref() != Some(&local)
            || ctx.metadata.contains_key("task_id")
        {
            return Err("Remote foreground execution requires the originating Chat".into());
        }
        let field = |name| ctx.metadata.get(name).and_then(Value::as_str);
        let turn = field("turn_id").ok_or("No active Chat Turn")?;
        let call = field("tool_call_id").ok_or("No active tool call")?;
        let configuration_epoch =
            field("chat_configuration_epoch").ok_or("No Chat configuration")?;
        let address: rua_core::ChatAddress = serde_json::from_value(
            ctx.metadata
                .get(rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)
                .cloned()
                .ok_or("No originating Chat")?,
        )
        .map_err(|error| error.to_string())?;
        let active = self
            .authority
            .active_turn(&address)
            .await
            .filter(|active| {
                active.authority_device_id == local
                    && active.turn_id == turn
                    && active.driver == ConversationDriverKind::Rua
                    && matches!(
                        active.phase,
                        ConversationActiveTurnPhase::RunningTool
                            | ConversationActiveTurnPhase::Streaming
                            | ConversationActiveTurnPhase::WaitingApproval
                    )
            })
            .ok_or("The originating Chat invocation is no longer active")?;
        let context = ChatCommandContext {
            execution_id: format!(
                "chat-command-{}",
                hex::encode(Sha256::digest(
                    serde_json::to_vec(&(&local, &address, turn, call))
                        .map_err(|e| e.to_string())?
                ))
            ),
            call: DeviceReadCheckRequest {
                source_device_id: local,
                target_device_id: target.into(),
                tool_name: "bash".into(),
                agent_id: ctx.agent_id.clone(),
                address,
                authority_epoch: active.authority_epoch,
                configuration_epoch: configuration_epoch.into(),
                turn_id: turn.into(),
                tool_call_id: call.into(),
                arguments,
                confirmation: None,
            },
            extra: Default::default(),
        };
        let manager = current_session_manager(&self.network).await?;
        let channel = tokio::select! {
            _ = cancelled(ctx) => return Err("Command cancelled before execution".into()),
            result = manager.open_command_channel(&context.call.target_device_id, RPC_TIMEOUT) => result.map_err(|error| error.to_string())?,
        };
        // A dropped foreground invocation still requests cancellation. Ordinary
        // completed waits (including unknown outcomes) never cancel by implication.
        let ended = CancellationToken::new();
        let guard = ended.clone().drop_guard();
        let finished = CancellationToken::new();
        let pending_adoption = Arc::new(AtomicBool::new(false));
        let prepared_for_cleanup = Arc::new(OnceLock::<ChatCommandPreparation>::new());
        let cleanup = {
            let service = self.clone();
            let context = context.clone();
            let channel = channel.clone();
            let finished = finished.clone();
            let pending_adoption = pending_adoption.clone();
            let prepared_for_cleanup = prepared_for_cleanup.clone();
            tokio::spawn(async move {
                tokio::select! {
                    biased;
                    _ = finished.cancelled() => {},
                    _ = ended.cancelled() => {
                        let response = service.control(&channel, &context, ChatCommandAction::Cancel).await;
                        if let Some(preparation) = prepared_for_cleanup
                            .get()
                            .filter(|_| pending_adoption.load(Ordering::SeqCst))
                        {
                            // The caller can disappear after its reference is
                            // durable but before adoption is accepted. Keep
                            // the same execution identity and reconcile only.
                            service.reconcile(&channel, &context, preparation, response, &pending_adoption).await;
                        }
                    }
                }
            })
        };
        let prepared = tokio::select! {
            _ = cancelled(ctx) => return Err("Command cancelled before execution".into()),
            result = dispatch(&channel, &context, ChatCommandAction::Prepare) => result?,
        };
        let preparation = match prepared {
            ChatCommandResponse::Prepared { preparation }
                if preparation.execution_id == context.execution_id
                    && preparation.device_id == context.call.target_device_id
                    && rua_core::execution_directory::is_device_absolute_path(
                        &preparation.public_cwd,
                    ) =>
            {
                preparation
            }
            other => {
                return Err(format!(
                    "The target did not prepare this command: {other:?}"
                ))
            }
        };
        let _ = prepared_for_cleanup.set((*preparation).clone());

        let started = AtomicBool::new(false);
        let cancel = CancellationToken::new();
        let result = tokio::select! {
            biased;
            _ = cancelled(ctx) => {
                cancel.cancel();
                let response = self.control(&channel, &context, ChatCommandAction::Cancel).await;
                Ok(self.reconcile(&channel, &context, &preparation, response, &pending_adoption).await)
            },
            _ = channel.closed() => {
                cancel.cancel();
                if started.load(Ordering::SeqCst) {
                    let response = self.control(&channel, &context, ChatCommandAction::Status).await;
                    Ok(self.reconcile(&channel, &context, &preparation, response, &pending_adoption).await)
                } else {
                    Err("Execution connection closed before command start".into())
                }
            },
            result = self.run(&channel, &context, &preparation, ctx, &cancel, &started, &pending_adoption) => result,
        };
        if !started.load(Ordering::SeqCst) && !cancel.is_cancelled() {
            let _ = self
                .control(&channel, &context, ChatCommandAction::Cancel)
                .await;
        }
        finished.cancel();
        drop(guard);
        drop(cleanup);
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        channel: &DeviceSessionCommandChannel,
        context: &ChatCommandContext,
        preparation: &ChatCommandPreparation,
        ctx: &ToolContext,
        cancel: &CancellationToken,
        started: &AtomicBool,
        pending_adoption: &AtomicBool,
    ) -> Result<ToolResult, String> {
        if let Some(instructions) = &ctx.agents_instructions {
            if let AgentsInstructionDecision::Reconsider { reason } = instructions
                .before_targets(
                    &[AgentsInstructionTarget {
                        device_id: preparation.device_id.clone(),
                        path: preparation.public_cwd.clone(),
                        kind: AgentsInstructionTargetKind::Directory,
                    }],
                    ctx,
                    cancel.clone(),
                )
                .await
                .map_err(|error| error.to_string())?
            {
                let mut result = ToolResult::text(&context.call.tool_call_id, reason);
                result
                    .metadata
                    .insert("execution_status".into(), json!("not_executed"));
                result
                    .metadata
                    .insert("agents_instructions".into(), json!("reconsider"));
                result
                    .metadata
                    .insert("execution_location".into(), location(preparation));
                return Ok(result);
            }
        }
        let mut approval = self.existing_approval(context).await?;
        match preparation.policy {
            DeviceReadCheckReply::Checked => {}
            DeviceReadCheckReply::ConfirmationRequired => {
                if approval.is_none() {
                    approval = Some(self.confirm(context, preparation, ctx).await?);
                }
            }
            ref other => return Err(format!("Command execution was not authorized: {other:?}")),
        }
        let permit = approval.as_ref().map(|id| {
            self.authority
                .begin_read_dispatch(id, &context.call.target_device_id)
        });
        let mut invocation = context.clone();
        invocation.call.confirmation =
            approval
                .as_ref()
                .zip(permit.as_ref())
                .map(|(id, permit)| DeviceReadConfirmation {
                    approval_id: id.clone(),
                    dispatch_token: permit.token(),
                });
        started.store(true, Ordering::SeqCst);
        let foreground_deadline = rua_tool::foreground_wait_limit(
            &rua_tool::ShellTool::spec().to_tool_definition(),
            &context.call.arguments,
        )
        .map(|wait| tokio::time::Instant::now() + wait);
        // Start is deliberately outside the polling loop: a lost receipt is not
        // permission to run the command again, even on another connection.
        let mut response = dispatch(channel, &invocation, ChatCommandAction::Start).await;
        if let Ok(ChatCommandResponse::Policy {
            response:
                response @ (DeviceReadCheckReply::Denied
                | DeviceReadCheckReply::ConfirmationRequired
                | DeviceReadCheckReply::Unavailable),
        }) = &response
        {
            // The target returns a policy rejection before spawning. Preserve
            // that evidence and release the prepared execution on return.
            started.store(false, Ordering::SeqCst);
            let mut result = ToolResult::error(
                &context.call.tool_call_id,
                format!(
                    "Command was not executed on Device {} in {}: {response:?}",
                    preparation.device_id, preparation.public_cwd
                ),
            );
            result
                .metadata
                .insert("execution_status".into(), json!("not_executed"));
            result
                .metadata
                .insert("execution_id".into(), json!(context.execution_id));
            result
                .metadata
                .insert("execution_location".into(), location(preparation));
            return Ok(result);
        }
        let mut failures = 0;
        let mut adoption_requested = false;
        let mut direct_adoption_reply = false;
        loop {
            match response {
                Ok(ChatCommandResponse::Task { task, reference }) => {
                    return Ok(self
                        .project_task(context, preparation, *task, reference)
                        .await);
                }
                Ok(ChatCommandResponse::Operation { operation }) => {
                    failures = 0;
                    if !valid_snapshot(context, preparation, &operation) {
                        return Ok(unknown(
                            context,
                            preparation,
                            "Target execution identity or directory did not match",
                        ));
                    }
                    if operation.state != ChatCommandState::Running {
                        self.resolve_unaccepted_adoption(context, pending_adoption)
                            .await;
                        if !self.invocation_active(context).await {
                            return Ok(unknown(
                                context,
                                preparation,
                                "The originating Chat invocation is no longer active",
                            ));
                        }
                        return Ok(project_result(context, preparation, *operation));
                    }
                    if !adoption_requested
                        && foreground_deadline
                            .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
                    {
                        let Some(tasks) = self.tasks.get() else {
                            return Ok(unknown(
                                context,
                                preparation,
                                "Task routing is unavailable; target adoption was not requested",
                            ));
                        };
                        pending_adoption.store(true, Ordering::SeqCst);
                        if let Err(error) = tasks.register_foreground(context, preparation).await {
                            return Ok(unknown(context, preparation, &format!("Could not retain the Task execution reference before adoption: {error}")));
                        }
                        // A lost adoption acknowledgement is reconciled through Status,
                        // never another Adopt or Start. The target owns acceptance.
                        adoption_requested = true;
                        direct_adoption_reply = true;
                        response = dispatch(channel, context, ChatCommandAction::Adopt).await;
                        continue;
                    }
                }
                Ok(ChatCommandResponse::Policy { response }) => {
                    if direct_adoption_reply {
                        self.resolve_unaccepted_adoption(context, pending_adoption)
                            .await;
                    }
                    return Ok(unknown(
                        context,
                        preparation,
                        &format!("The target could not confirm execution: {response:?}"),
                    ));
                }
                Ok(_) => {
                    return Ok(unknown(
                        context,
                        preparation,
                        "The target did not report execution state",
                    ))
                }
                Err(ref error) => {
                    failures += 1;
                    if failures >= 3 {
                        return Ok(unknown(context, preparation, error));
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            direct_adoption_reply = false;
            response = dispatch(channel, context, ChatCommandAction::Status).await;
        }
    }

    async fn invocation_active(&self, context: &ChatCommandContext) -> bool {
        self.authority
            .active_turn(&context.call.address)
            .await
            .is_some_and(|active| {
                active.turn_id == context.call.turn_id
                    && active.authority_device_id == context.call.source_device_id
                    && active.authority_epoch == context.call.authority_epoch
                    && active.driver == ConversationDriverKind::Rua
                    && matches!(
                        active.phase,
                        ConversationActiveTurnPhase::RunningTool
                            | ConversationActiveTurnPhase::Streaming
                            | ConversationActiveTurnPhase::WaitingApproval
                    )
            })
    }

    async fn existing_approval(
        &self,
        context: &ChatCommandContext,
    ) -> Result<Option<String>, String> {
        let records = self.approvals.snapshot_at(&context.call.address).await;
        let matching = records
            .pending
            .iter()
            .chain(&records.resolved)
            .filter(|record| {
                record.agent_id == context.call.agent_id
                    && record.turn_id == context.call.turn_id
                    && record.tool_call_id == context.call.tool_call_id
            })
            .collect::<Vec<_>>();
        if matching.len() > 1
            || matching.first().is_some_and(|record| {
                record.status != ApprovalStatus::Approved
                    || record.tool_name != "bash"
                    || record.tool_call.args != context.call.arguments
            })
        {
            return Err("The approval does not match this active command".into());
        }
        Ok(matching
            .first()
            .filter(|record| {
                record
                    .authenticated_user_decision
                    .as_ref()
                    .is_some_and(|user| {
                        matches!(
                            user.decision,
                            AuthenticatedApprovalDecision::RunOnce
                                | AuthenticatedApprovalDecision::AllowChat
                                | AuthenticatedApprovalDecision::AllowAgent
                        )
                    })
            })
            .map(|record| record.id.clone()))
    }

    async fn confirm(
        &self,
        context: &ChatCommandContext,
        preparation: &ChatCommandPreparation,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let prompt = format!(
            "Confirm command on Device {} in {}: {}",
            preparation.device_id,
            preparation.public_cwd,
            context.call.arguments["command"]
                .as_str()
                .unwrap_or_default()
        );
        match context
            .call
            .wait_for_confirmation_with_prompt(ctx, &self.approvals, &self.authority, prompt)
            .await
            .map_err(|error| error.to_string())?
        {
            Ok((id, _)) => Ok(id),
            Err(DeviceReadCheck::Denied(detail) | DeviceReadCheck::Unavailable(detail)) => {
                Err(detail)
            }
            Err(_) => Err("The user did not approve this command".into()),
        }
    }

    async fn control(
        &self,
        channel: &DeviceSessionCommandChannel,
        context: &ChatCommandContext,
        action: ChatCommandAction,
    ) -> Result<ChatCommandResponse, String> {
        debug_assert!(matches!(
            action,
            ChatCommandAction::Status | ChatCommandAction::Cancel
        ));
        if !channel.is_closed() {
            return dispatch(channel, context, action).await;
        }
        let manager = current_session_manager(&self.network).await?;
        deliver_chat_command_via_session(&manager, &command(context, action), RPC_TIMEOUT).await
    }

    async fn reconcile(
        &self,
        channel: &DeviceSessionCommandChannel,
        context: &ChatCommandContext,
        preparation: &ChatCommandPreparation,
        mut response: Result<ChatCommandResponse, String>,
        pending_adoption: &AtomicBool,
    ) -> ToolResult {
        for _ in 0..3 {
            if let Ok(ChatCommandResponse::Task { task, reference }) = response {
                return self
                    .project_task(context, preparation, *task, reference)
                    .await;
            }
            if let Ok(ChatCommandResponse::Operation { operation }) = response {
                if valid_snapshot(context, preparation, &operation)
                    && operation.state != ChatCommandState::Running
                {
                    self.resolve_unaccepted_adoption(context, pending_adoption)
                        .await;
                    return project_result(context, preparation, *operation);
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            response = self
                .control(channel, context, ChatCommandAction::Status)
                .await;
        }
        unknown(
            context,
            preparation,
            "No target terminal evidence was available; the command was not replayed",
        )
    }

    async fn resolve_unaccepted_adoption(
        &self,
        context: &ChatCommandContext,
        pending: &AtomicBool,
    ) {
        if !pending.load(Ordering::SeqCst) {
            return;
        }
        if let Some(tasks) = self.tasks.get() {
            let owner = rua_task::TaskOwner::from_address(context.call.address.clone());
            if let Err(error) = tasks.mark_not_accepted(&owner, &context.execution_id).await {
                tracing::warn!(%error, execution_id = %context.execution_id, "Unaccepted Task reference remains pending");
            }
        }
    }

    async fn project_task(
        &self,
        context: &ChatCommandContext,
        preparation: &ChatCommandPreparation,
        task: rua_task::TaskRecord,
        reference: Option<Box<rua_session::task_refs::ChatTaskRef>>,
    ) -> ToolResult {
        let attempt = task
            .attempts
            .iter()
            .find(|attempt| attempt.attempt_no == task.attempt_no);
        if task.task_id != context.execution_id
            || task.spec.owner.chat_address != context.call.address
            || task.spec.origin.turn_id.as_deref() != Some(&context.call.turn_id)
            || task.spec.origin.tool_call_id.as_deref() != Some(&context.call.tool_call_id)
            || !attempt.is_some_and(|attempt| {
                attempt.device.device_id.as_deref() == Some(&preparation.device_id)
                    && attempt.device.public_cwd.as_deref() == Some(&preparation.public_cwd)
            })
        {
            return unknown(
                context,
                preparation,
                "The target Task acceptance did not match this execution",
            );
        }
        let persistence = match self.tasks.get() {
            Some(tasks) => match reference {
                Some(reference)
                    if reference.task_id == task.task_id
                        && reference.owner == task.spec.owner
                        && reference.execution.device_id == preparation.device_id
                        && reference.execution.public_cwd == preparation.public_cwd =>
                {
                    tasks.remember_reference(&reference).await
                }
                _ => tasks.remember_task(&task).await,
            },
            None => Err("Chat Task routing is unavailable".into()),
        };
        let mut result = ToolResult::text(
            &context.call.tool_call_id,
            format!("Tool call moved to Background Tool Task on Device {} in {}. task_id: {}. The target accepted the original execution once; its results will return to this Chat.", preparation.device_id, preparation.public_cwd, task.task_id),
        );
        result.name = Some("bash".into());
        result
            .metadata
            .insert("background_task_id".into(), json!(task.task_id));
        result.metadata.insert(
            "origin_tool_call_id".into(),
            json!(context.call.tool_call_id),
        );
        result
            .metadata
            .insert("owner_agent_id".into(), json!(context.call.agent_id));
        result.metadata.insert(
            "owner_conversation_id".into(),
            json!(context.call.address.chat_id().as_str()),
        );
        result
            .metadata
            .insert("execution_location".into(), location(preparation));
        result
            .metadata
            .insert("execution_id".into(), json!(context.execution_id));
        if let Err(error) = persistence {
            result
                .metadata
                .insert("task_reference_error".into(), json!(error));
        }
        result
    }
}

fn command(context: &ChatCommandContext, action: ChatCommandAction) -> ChatCommandCommand {
    ChatCommandCommand {
        context: context.clone(),
        action,
        extra: Default::default(),
    }
}

async fn dispatch(
    channel: &DeviceSessionCommandChannel,
    context: &ChatCommandContext,
    action: ChatCommandAction,
) -> Result<ChatCommandResponse, String> {
    deliver_chat_command_via_channel(channel, &command(context, action), RPC_TIMEOUT).await
}

fn valid_snapshot(
    context: &ChatCommandContext,
    preparation: &ChatCommandPreparation,
    snapshot: &ChatCommandSnapshot,
) -> bool {
    snapshot.execution_id == context.execution_id
        && snapshot.device_id == preparation.device_id
        && snapshot.public_cwd == preparation.public_cwd
}

fn location(preparation: &ChatCommandPreparation) -> Value {
    json!({"device_id": preparation.device_id, "cwd": preparation.public_cwd})
}

fn project_result(
    context: &ChatCommandContext,
    preparation: &ChatCommandPreparation,
    snapshot: ChatCommandSnapshot,
) -> ToolResult {
    let Some(mut result) = snapshot.result else {
        return unknown(
            context,
            preparation,
            "The target has not supplied a completed execution result",
        );
    };
    if snapshot.state == ChatCommandState::Unknown {
        return unknown(context, preparation, &result.text_content());
    }
    if !matches!(
        snapshot.state,
        ChatCommandState::Succeeded | ChatCommandState::Failed | ChatCommandState::Cancelled
    ) || (snapshot.state == ChatCommandState::Succeeded) == result.is_error
    {
        return unknown(
            context,
            preparation,
            "The target execution result could not be confirmed",
        );
    }
    result.call_id = context.call.tool_call_id.clone();
    result.name = Some("bash".into());
    // These are host-local implementation paths. The target exports explicit
    // Device/public-path references for reading retained output instead.
    result.metadata.remove("raw_capture");
    result.metadata.remove("live_output_path");
    result
        .metadata
        .insert("execution_location".into(), location(preparation));
    result
        .metadata
        .insert("execution_id".into(), json!(context.execution_id));
    result
        .metadata
        .insert("execution_state".into(), json!(snapshot.state));
    let exit_code = result
        .metadata
        .get("exit_code")
        .and_then(Value::as_i64)
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unavailable".into());
    result.content.insert(
        0,
        agent_types::ContentPart::Text {
            text: format!(
                "Device: {}\nWorking directory: {}\nExecution: {} ({:?})\nExit code: {}\n",
                preparation.device_id,
                preparation.public_cwd,
                context.execution_id,
                snapshot.state,
                exit_code
            ),
        },
    );
    result
}

fn unknown(
    context: &ChatCommandContext,
    preparation: &ChatCommandPreparation,
    detail: &str,
) -> ToolResult {
    let mut result = ToolResult::error(
        &context.call.tool_call_id,
        format!(
        "Command outcome is unconfirmed on Device {} in {}. Execution: {}. {detail}. Do not run this command again to recover a missing result; it may already have produced effects.",
            preparation.device_id, preparation.public_cwd, context.execution_id
        ),
    );
    result
        .metadata
        .insert("execution_state".into(), json!("unknown"));
    result
        .metadata
        .insert("execution_id".into(), json!(context.execution_id));
    result
        .metadata
        .insert("execution_location".into(), location(preparation));
    result.metadata.insert("replayed".into(), json!(false));
    result
}

async fn cancelled(ctx: &ToolContext) {
    if let Some(mut cancel) = ctx.cancel.clone() {
        if cancel.wait_for(|value| *value).await.is_ok() {
            return;
        }
    }
    std::future::pending().await
}

pub struct RemoteCommandRuntimeAdapter {
    service: Arc<OnceLock<Arc<ChatCommandToolService>>>,
}

impl RemoteCommandRuntimeAdapter {
    pub fn new(service: Arc<OnceLock<Arc<ChatCommandToolService>>>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl rua_tool::core::shell::RemoteCommandRuntime for RemoteCommandRuntimeAdapter {
    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult, String> {
        self.service
            .get()
            .ok_or("Remote command service unavailable")?
            .execute(arguments, context)
            .await
    }
}
