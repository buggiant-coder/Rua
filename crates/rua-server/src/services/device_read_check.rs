use std::sync::{Arc, Weak};
use std::time::Duration;

use rua_core::tool::{DeviceReadAccess, DeviceReadCheck, SharedReadPage, ToolContext, ToolResult};
use rua_core::ChatAddress;
use rua_security::approval_authority::{ApprovalStatus, AuthenticatedApprovalDecision};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::conversation_authority::{
    ConversationActiveTurnPhase, ConversationAuthority, ConversationDriverKind,
};
use super::conversation_authority_placement::ConversationAuthorityEpoch;
use super::device_command_bus::{
    deliver_allowed_file_read_via_session, deliver_conversation_control_via_session,
    ConversationControlDeviceAction, ConversationControlDeviceCommand,
    ConversationControlDeviceResponse,
};
use super::device_session_access::DeviceNetworkAccess;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceReadCheckRequest {
    #[serde(default = "read_tool_name")]
    pub tool_name: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub agent_id: String,
    pub address: ChatAddress,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub configuration_epoch: String,
    pub turn_id: String,
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<DeviceReadConfirmation>,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceReadConfirmation {
    pub approval_id: String,
    pub dispatch_token: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DeviceReadCheckReply {
    Unavailable,
    Denied,
    ConfirmationRequired,
    Checked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DeviceFileReadReply {
    Result(Box<ToolResult>),
    Policy(DeviceReadCheckReply),
    FileNotAuthorized,
    FileUnavailable,
    Page(SharedReadPage),
}

pub(crate) fn read_tool_name() -> String {
    "read".into()
}

pub struct ForegroundReadAccess {
    pub authority: Weak<ConversationAuthority>,
    pub approvals: Weak<rua_security::ApprovalAuthority>,
    pub network: Weak<DeviceNetworkAccess>,
}

#[async_trait::async_trait]
impl DeviceReadAccess for ForegroundReadAccess {
    fn manages_confirmation_wait(&self, ctx: &ToolContext, args: &Value) -> bool {
        self.manages_operation_confirmation_wait("read", ctx, args)
    }

    fn manages_operation_confirmation_wait(
        &self,
        tool: &str,
        ctx: &ToolContext,
        args: &Value,
    ) -> bool {
        let local = ctx.local_device_id.as_deref();
        let target = args["device_id"].as_str();
        let targets_operation = if tool == "sync_send" {
            ["source_device_id", "target_device_id"]
                .iter()
                .all(|key| args[key].as_str().is_some_and(|id| !id.trim().is_empty()))
        } else {
            target.is_some_and(|id| !id.trim().is_empty()) && target != local
        };
        local.is_some_and(|id| !id.trim().is_empty())
            && targets_operation
            && !ctx.metadata.contains_key("task_id")
            && ctx
                .metadata
                .contains_key(rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)
            && ctx.metadata.contains_key("turn_id")
            && ctx.metadata.contains_key("chat_configuration_epoch")
            && matches!(tool, "read" | "edit" | "grep" | "glob" | "sync_send")
    }

    async fn check(&self, ctx: &ToolContext, args: &Value) -> rua_core::Result<DeviceReadCheck> {
        self.check_operation("read", ctx, args).await
    }

    async fn check_operation(
        &self,
        tool: &str,
        ctx: &ToolContext,
        args: &Value,
    ) -> rua_core::Result<DeviceReadCheck> {
        let unavailable = |detail: &str| DeviceReadCheck::Unavailable(detail.to_owned());
        let (Some(authority), Some(approvals), Some(network)) = (
            self.authority.upgrade(),
            self.approvals.upgrade(),
            self.network.upgrade(),
        ) else {
            return Ok(unavailable("the device service is unavailable"));
        };
        let field = |name: &str| ctx.metadata.get(name).and_then(Value::as_str);
        let (Some(local), Some(device), Some(turn), Some(call), Some(epoch)) = (
            ctx.local_device_id.as_deref(),
            args.get("device_id").and_then(Value::as_str),
            field("turn_id"),
            field("tool_call_id"),
            field("chat_configuration_epoch"),
        ) else {
            return Ok(unavailable("no active Chat invocation was identified"));
        };
        if !self.manages_operation_confirmation_wait(tool, ctx, args) {
            return Ok(unavailable(
                "this operation requires an active Chat and an explicit target device",
            ));
        }
        let Some(address) = ctx
            .metadata
            .get(rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)
            .and_then(|value| serde_json::from_value::<ChatAddress>(value.clone()).ok())
        else {
            return Ok(unavailable("the requesting Chat was not identified"));
        };
        let records = approvals.snapshot_at(&address).await;
        let parallel_wait = other_pending_target_read(&records.pending, &ctx.agent_id, turn, call);
        let Some(active) = authority.active_turn(&address).await.filter(|active| {
            active.authority_device_id == local
                && active.turn_id == turn
                && active.driver == ConversationDriverKind::Rua
                && (matches!(
                    active.phase,
                    ConversationActiveTurnPhase::Streaming
                        | ConversationActiveTurnPhase::RunningTool
                ) || (active.phase == ConversationActiveTurnPhase::WaitingApproval
                    && parallel_wait))
        }) else {
            return Ok(unavailable(
                "the original tool call is no longer running on this device",
            ));
        };
        let mut matches =
            records.pending.iter().chain(&records.resolved).filter(|r| {
                r.agent_id == ctx.agent_id && r.turn_id == turn && r.tool_call_id == call
            });
        let record = matches.next();
        if matches.next().is_some()
            || record.is_some_and(|r| {
                // A target-only card belongs to the waiter that created it. A new
                // invocation cannot pick it up if the original was cancelled.
                r.trigger.source == rua_security::ApprovalTriggerSource::TargetDevice
                    || r.status != ApprovalStatus::Approved
                    || r.tool_name != tool
                    || r.tool_call.args != *args
            })
        {
            return Ok(unavailable(
                "the approval state does not match this running read",
            ));
        }
        // A fresh user decision also authorizes this invocation while reusable settings sync.
        let confirmed = record.filter(|r| {
            r.authenticated_user_decision.as_ref().is_some_and(|user| {
                matches!(
                    user.decision,
                    AuthenticatedApprovalDecision::RunOnce
                        | AuthenticatedApprovalDecision::AllowChat
                        | AuthenticatedApprovalDecision::AllowAgent
                )
            })
        });
        if confirmed.is_some()
            && active.phase != ConversationActiveTurnPhase::RunningTool
            && !(active.phase == ConversationActiveTurnPhase::WaitingApproval && parallel_wait)
        {
            return Ok(unavailable("the confirmed read is no longer running"));
        }
        let transport = network
            .lock()
            .await
            .as_ref()
            .map(|runtime| runtime.session_manager().clone());
        let Some(transport) = transport else {
            return Ok(unavailable("the device network is unavailable"));
        };
        let permit = confirmed.map(|r| authority.begin_read_dispatch(&r.id, device));
        let mut request = DeviceReadCheckRequest {
            tool_name: tool.into(),
            source_device_id: local.into(),
            target_device_id: device.into(),
            agent_id: ctx.agent_id.clone(),
            address,
            authority_epoch: active.authority_epoch,
            configuration_epoch: epoch.into(),
            turn_id: turn.into(),
            tool_call_id: call.into(),
            confirmation: confirmed
                .zip(permit.as_ref())
                .map(|(r, p)| DeviceReadConfirmation {
                    approval_id: r.id.clone(),
                    dispatch_token: p.token(),
                }),
            arguments: args.clone(),
        };
        let mut response = request.send(&transport, ctx).await?;
        drop(permit);
        let mut permission_saved = confirmed.is_some_and(|r| {
            r.authenticated_user_decision
                .as_ref()
                .is_some_and(|user| user.decision != AuthenticatedApprovalDecision::RunOnce)
        });
        if response.as_ref().is_ok_and(|reply| {
            *reply == DeviceFileReadReply::Policy(DeviceReadCheckReply::ConfirmationRequired)
        }) && record.is_none()
        {
            let decision = match request
                .wait_for_confirmation(ctx, &approvals, &authority)
                .await?
            {
                Ok(decision) => decision,
                Err(status) => return Ok(status),
            };
            permission_saved = decision.1;
            let id = decision.0;
            let permit = {
                let permit = authority.begin_read_dispatch(&id, device);
                request.confirmation = Some(DeviceReadConfirmation {
                    approval_id: id,
                    dispatch_token: permit.token(),
                });
                permit
            };
            response = request.send(&transport, ctx).await?;
            drop(permit);
        }
        Ok(match response {
            Ok(DeviceFileReadReply::Result(result)) => {
                if result.call_id != call
                    || result.attachments.is_some()
                    || result
                        .content
                        .iter()
                        .any(|part| !matches!(part, agent_types::ContentPart::Text { .. }))
                    || result.text_content().len() > rua_core::tool::MAX_SHARED_READ_TEXT_BYTES
                    || !authority
                        .active_turn(&request.address)
                        .await
                        .is_some_and(|current| {
                            current.authority_device_id == local
                                && current.authority_epoch == request.authority_epoch
                                && current.turn_id == turn
                                && current.driver == ConversationDriverKind::Rua
                                && matches!(
                                    current.phase,
                                    ConversationActiveTurnPhase::Streaming
                                        | ConversationActiveTurnPhase::RunningTool
                                        | ConversationActiveTurnPhase::WaitingApproval
                                )
                        })
                {
                    if tool == "edit" {
                        return Ok(DeviceReadCheck::OutcomeUnconfirmed(
                            "the target edit reply could not be accepted by the original active call".into(),
                        ));
                    }
                    return Ok(unavailable(
                        "the target returned an invalid or stale tool result",
                    ));
                }
                DeviceReadCheck::Executed(result)
            }
            Ok(DeviceFileReadReply::FileNotAuthorized) => DeviceReadCheck::FileNotAuthorized,
            Ok(DeviceFileReadReply::FileUnavailable) => DeviceReadCheck::FileUnavailable,
            Ok(DeviceFileReadReply::Page(page)) => {
                if !page.is_valid() {
                    return Ok(unavailable("the target returned an invalid file page"));
                }
                if !authority
                    .active_turn(&request.address)
                    .await
                    .is_some_and(|current| {
                        current.authority_device_id == local
                            && current.authority_epoch == request.authority_epoch
                            && current.turn_id == turn
                            && current.driver == ConversationDriverKind::Rua
                            && matches!(
                                current.phase,
                                ConversationActiveTurnPhase::Streaming
                                    | ConversationActiveTurnPhase::RunningTool
                                    | ConversationActiveTurnPhase::WaitingApproval
                            )
                    })
                {
                    return Ok(unavailable("the original read is no longer active"));
                }
                DeviceReadCheck::Shared(page)
            }
            Ok(DeviceFileReadReply::Policy(DeviceReadCheckReply::Checked)) => {
                DeviceReadCheck::Checked
            }
            Ok(DeviceFileReadReply::Policy(DeviceReadCheckReply::ConfirmationRequired)) => {
                DeviceReadCheck::ConfirmationRequired { permission_saved }
            }
            Ok(DeviceFileReadReply::Policy(DeviceReadCheckReply::Denied)) => {
                DeviceReadCheck::Denied(
                    "the target device denied this file operation under its current policy".into(),
                )
            }
            Ok(DeviceFileReadReply::Policy(DeviceReadCheckReply::Unavailable)) => {
                unavailable("the target could not verify this Chat's file operation")
            }
            Err(super::device_command_bus::DeviceCommandDeliveryError::OutcomeUnconfirmed(
                detail,
            )) => DeviceReadCheck::OutcomeUnconfirmed(detail),
            Err(error) => error.into(),
        })
    }
}

impl From<super::device_command_bus::DeviceCommandDeliveryError> for DeviceReadCheck {
    fn from(error: super::device_command_bus::DeviceCommandDeliveryError) -> Self {
        use super::device_command_bus::DeviceCommandDeliveryError as Delivery;
        use rua_device_network::DeviceSessionError as Session;

        let detail = match error {
            Delivery::Timeout | Delivery::Session(Session::Timeout { .. }) => {
                "the permission check timed out before the target confirmed it"
            }
            Delivery::Connect(_) | Delivery::Session(Session::Connect(_)) => {
                "could not connect to the target device"
            }
            Delivery::Session(Session::PeerNotPaired(_)) => "the target device is not paired",
            Delivery::Session(Session::Closed(_) | Session::Replaced) => {
                "the device connection ended before the target confirmed the permission check"
            }
            _ => "the target did not confirm the permission check",
        };
        Self::Unavailable(detail.into())
    }
}

pub(super) fn other_pending_target_read(
    pending: &[rua_security::ApprovalRequest],
    agent: &str,
    turn: &str,
    call: &str,
) -> bool {
    pending.iter().any(|request| {
        request.agent_id == agent
            && request.turn_id == turn
            && request.tool_call_id != call
            && matches!(
                request.tool_name.as_str(),
                "read" | "edit" | "grep" | "glob" | "sync_send" | "bash"
            )
            && request.trigger.source == rua_security::ApprovalTriggerSource::TargetDevice
            && request.status == ApprovalStatus::Pending
    })
}

async fn read_cancelled(ctx: &ToolContext) {
    if let Some(mut receiver) = ctx.cancel.clone() {
        if receiver.wait_for(|stopped| *stopped).await.is_ok() {
            return;
        }
    }
    std::future::pending().await
}

impl DeviceReadCheckRequest {
    pub(super) fn operation_key(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.source_device_id,
            self.address.chat_id(),
            self.turn_id,
            self.tool_call_id,
            self.tool_name
        )
    }

    pub(super) fn as_target_read(&self) -> super::chat_file_read_admission::TargetChatRead<'_> {
        super::chat_file_read_admission::TargetChatRead {
            address: &self.address,
            agent_id: &self.agent_id,
            authority_device_id: &self.source_device_id,
            authority_epoch: self.authority_epoch,
            configuration_epoch: &self.configuration_epoch,
            turn_id: &self.turn_id,
            tool_call_id: &self.tool_call_id,
            tool_name: &self.tool_name,
            args: &self.arguments,
        }
    }

    async fn send(
        &self,
        manager: &rua_device_network::DeviceSessionManager,
        ctx: &ToolContext,
    ) -> rua_core::Result<
        Result<DeviceFileReadReply, super::device_command_bus::DeviceCommandDeliveryError>,
    > {
        let stopped = tokio_util::sync::CancellationToken::new();
        let guard = stopped.clone().drop_guard();
        let finished = tokio_util::sync::CancellationToken::new();
        let cleanup = {
            let manager = manager.clone();
            let request = self.clone();
            let finished = finished.clone();
            tokio::spawn(async move {
                tokio::select! {
                    biased;
                    _ = finished.cancelled() => {},
                    _ = stopped.cancelled() => {
                        super::device_command_bus::cancel_file_operation_via_session(&manager, &request).await;
                    }
                }
            })
        };
        let outcome = tokio::select! {
            biased;
            _ = read_cancelled(ctx) => Err(rua_core::RuaError::Cancelled),
            reply = deliver_allowed_file_read_via_session(
                manager, self,
                ctx.raw_capture_max_bytes.clamp(1, rua_core::tool::MAX_SHARED_READ_TEXT_BYTES),
                Duration::from_secs(20),
            ) => Ok(reply),
        };
        if matches!(&outcome, Ok(Ok(_))) {
            finished.cancel();
        }
        drop(guard);
        let _ = cleanup.await;
        outcome
    }

    pub(super) async fn wait_for_confirmation(
        &self,
        ctx: &ToolContext,
        approvals: &Arc<rua_security::ApprovalAuthority>,
        authority: &Arc<ConversationAuthority>,
    ) -> rua_core::Result<Result<(String, bool), DeviceReadCheck>> {
        let prompt = format!(
            "Confirm {} on Device {} with arguments {}.",
            self.tool_name, self.target_device_id, self.arguments
        );
        self.wait_for_confirmation_with_prompt(ctx, approvals, authority, prompt)
            .await
    }

    pub(super) async fn wait_for_confirmation_with_prompt(
        &self,
        ctx: &ToolContext,
        approvals: &Arc<rua_security::ApprovalAuthority>,
        authority: &Arc<ConversationAuthority>,
        prompt: String,
    ) -> rua_core::Result<Result<(String, bool), DeviceReadCheck>> {
        use rua_security::{
            ApprovalOwner, ApprovalRequestInput, ApprovalTrigger, ToolCallSnapshot,
        };
        use ConversationActiveTurnPhase::{RunningTool, Streaming, WaitingApproval};
        let Some(request) = approvals
            .create_target_file_confirmation(ApprovalRequestInput {
                agent_id: self.agent_id.clone(),
                owner: ApprovalOwner::chat(self.address.clone()),
                turn_id: self.turn_id.clone(),
                tool_call_id: self.tool_call_id.clone(),
                tool_name: self.tool_name.clone(),
                trigger: ApprovalTrigger::approval_mode(prompt.clone()),
                tool_call: ToolCallSnapshot {
                    display: format!("{} {}", self.tool_name, self.arguments),
                    args: self.arguments.clone(),
                },
            })
            .await
        else {
            return Ok(Err(DeviceReadCheck::ConfirmationRequired {
                permission_saved: false,
            }));
        };

        // The ReAct executor can drop this future on Stop or interjection. Close only its card.
        let ended = tokio_util::sync::CancellationToken::new();
        let cancel_on_drop = ended.clone().drop_guard();
        let cleanup = {
            let approvals = Arc::clone(approvals);
            let authority = Arc::clone(authority);
            let address = self.address.clone();
            let id = request.id.clone();
            let turn = self.turn_id.clone();
            tokio::spawn(async move {
                ended.cancelled().await;
                approvals.supersede_request(&id).await;
                if approvals
                    .list_pending_for_address(&address)
                    .await
                    .is_empty()
                {
                    authority
                        .transition_active_turn_phase_if(
                            &address,
                            Some(&turn),
                            RunningTool,
                            |active| active.phase == WaitingApproval,
                        )
                        .await;
                }
            })
        };
        let wait = async {
            if authority
                .transition_active_turn_phase_if(
                    &self.address,
                    Some(&self.turn_id),
                    WaitingApproval,
                    |active| {
                        active.authority_epoch == self.authority_epoch
                            && active.authority_device_id == self.source_device_id
                            && active.driver == ConversationDriverKind::Rua
                            && matches!(active.phase, Streaming | RunningTool | WaitingApproval)
                    },
                )
                .await
                .is_none()
            {
                return Err(DeviceReadCheck::Unavailable(
                    "the original tool call is no longer running".into(),
                ));
            }
            if let Some(events) = ctx.event_tx.as_ref() {
                events
                    .send(agent_types::AgentEvent::ToolApprovalPending {
                        call_id: self.tool_call_id.clone(),
                        tool_name: self.tool_name.clone(),
                        request_id: Some(request.id.clone()),
                        metadata: serde_json::json!({ "prompt": prompt }),
                    })
                    .await
                    .map_err(|_| DeviceReadCheck::Unavailable("the original Chat closed".into()))?;
            }
            let waiting_user = ctx
                .device_read_confirmation_wait
                .as_ref()
                .map(|wait| wait.begin());
            let decision = approvals
                .wait_for_decision(&request.id)
                .await
                .map_err(|_| {
                    DeviceReadCheck::Unavailable("the confirmation is no longer available".into())
                })?;
            drop(waiting_user);
            if decision.status == ApprovalStatus::Rejected {
                return Err(DeviceReadCheck::Denied(
                    "the user declined confirmation of this read".into(),
                ));
            }
            if decision.status == ApprovalStatus::Approved {
                match decision
                    .authenticated_user_decision
                    .map(|user| user.decision)
                {
                    Some(AuthenticatedApprovalDecision::RunOnce) => return Ok((request.id, false)),
                    Some(
                        AuthenticatedApprovalDecision::AllowChat
                        | AuthenticatedApprovalDecision::AllowAgent,
                    ) => return Ok((request.id, true)),
                    _ => {}
                }
            }
            Err(DeviceReadCheck::Unavailable(
                "the active read did not receive user confirmation".into(),
            ))
        };
        let outcome = tokio::select! {
            biased;
            _ = read_cancelled(ctx) => Err(rua_core::RuaError::Cancelled),
            outcome = wait => Ok(outcome),
        };
        drop(cancel_on_drop);
        cleanup.await.map_err(|error| rua_core::RuaError::Runtime {
            message: error.to_string(),
        })?;
        outcome
    }

    pub(super) async fn verify_at_origin(
        &self,
        manager: &rua_device_network::DeviceSessionManager,
        confirmation: &DeviceReadConfirmation,
    ) -> bool {
        let command = ConversationControlDeviceCommand {
            command_id: Uuid::new_v4().to_string(),
            address: self.address.clone(),
            agent_id: self.agent_id.clone(),
            authority_epoch: self.authority_epoch,
            source_device_id: self.target_device_id.clone(),
            target_device_id: self.source_device_id.clone(),
            action: ConversationControlDeviceAction::ClaimReadApproval {
                tool_name: self.tool_name.clone(),
                approval_id: confirmation.approval_id.clone(),
                dispatch_token: confirmation.dispatch_token,
                turn_id: self.turn_id.clone(),
                tool_call_id: self.tool_call_id.clone(),
                arguments: self.arguments.clone(),
            },
        };
        matches!(deliver_conversation_control_via_session(manager, &command, Duration::from_secs(8)).await,
            Ok(ConversationControlDeviceResponse::ReadApprovalClaimed {approval_id}) if approval_id == confirmation.approval_id)
    }
}
