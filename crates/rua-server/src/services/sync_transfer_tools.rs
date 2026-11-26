use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use rua_core::agents_instruction_discovery::AgentsInstructionTargetKind;
use rua_core::agents_instructions::{AgentsInstructionDecision, AgentsInstructionTarget};
use rua_core::tool::{DeviceReadCheck, ToolContext};
use rua_security::approval_authority::{ApprovalStatus, AuthenticatedApprovalDecision};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::chat_transfer::{
    ChatTransferAction, ChatTransferCommand, ChatTransferContext, ChatTransferResponse,
    ChatTransferService, PreparedChatReceive,
};
use super::conversation_authority::{
    ConversationActiveTurnPhase, ConversationAuthority, ConversationDriverKind,
};
use super::device_command_bus::deliver_chat_transfer_via_session;
use super::device_context::current_sync_device_id;
use super::device_read_check::{
    DeviceReadCheckReply, DeviceReadCheckRequest, DeviceReadConfirmation,
};
use super::device_session_access::{current_session_manager, DeviceNetworkAccess};
use super::sync_transfer::{SourceTransferOperationSnapshot, TransferJobStatus};

#[derive(Clone)]
pub struct SyncTransferToolService {
    sync_enabled: Arc<AtomicBool>,
    sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    device_network: Arc<DeviceNetworkAccess>,
    participant: Arc<ChatTransferService>,
    authority: Arc<ConversationAuthority>,
    approvals: Arc<rua_security::ApprovalAuthority>,
}

impl SyncTransferToolService {
    pub fn new(
        sync_enabled: Arc<AtomicBool>,
        sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
        device_network: Arc<DeviceNetworkAccess>,
        participant: Arc<ChatTransferService>,
        authority: Arc<ConversationAuthority>,
        approvals: Arc<rua_security::ApprovalAuthority>,
    ) -> Self {
        Self {
            sync_enabled,
            sync_device,
            device_network,
            participant,
            authority,
            approvals,
        }
    }

    async fn dispatch(&self, command: ChatTransferCommand) -> Result<ChatTransferResponse, String> {
        if current_sync_device_id(&self.sync_device).as_deref()
            == Some(command.context.call.target_device_id.as_str())
        {
            return self.participant.handle(command).await;
        }
        let manager = current_session_manager(&self.device_network).await?;
        deliver_chat_transfer_via_session(&manager, &command, Duration::from_secs(20))
            .await
            .map_err(|error| error.to_string())
    }

    async fn send(&self, arguments: Value, ctx: &ToolContext) -> Result<Value, String> {
        let source = arguments["source_device_id"]
            .as_str()
            .ok_or("Missing source Device")?;
        let target = arguments["target_device_id"]
            .as_str()
            .ok_or("Missing target Device")?;
        let local = current_sync_device_id(&self.sync_device).ok_or("Local Device unavailable")?;
        if (source != local || target != local) && !self.sync_enabled.load(Ordering::SeqCst) {
            return Err("Multi-device sync is disabled".into());
        }
        let field = |name: &str| ctx.metadata.get(name).and_then(Value::as_str);
        let turn = field("turn_id").ok_or("No active Chat Turn")?;
        let call = field("tool_call_id").ok_or("No active tool call")?;
        let address: rua_core::ChatAddress = serde_json::from_value(
            ctx.metadata
                .get(rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)
                .cloned()
                .ok_or("No originating Chat")?,
        )
        .map_err(|error| error.to_string())?;
        if ctx.metadata.contains_key("task_id") || ctx.local_device_id.as_deref() != Some(&local) {
            return Err("File transfer requires the originating active Chat".into());
        }
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
        let operation_id = format!(
            "chat-transfer-{}",
            hex::encode(Sha256::digest(
                serde_json::to_vec(&(&local, &address, turn, call))
                    .map_err(|error| error.to_string())?
            ))
        );
        let base = ChatTransferContext {
            operation_id,
            extra: Default::default(),
            call: DeviceReadCheckRequest {
                tool_name: "sync_send".into(),
                source_device_id: local,
                target_device_id: source.into(),
                agent_id: ctx.agent_id.clone(),
                address,
                authority_epoch: active.authority_epoch,
                turn_id: turn.into(),
                tool_call_id: call.into(),
                confirmation: None,
                arguments: arguments.clone(),
            },
        };
        let mut destination = base.clone();
        destination.call.target_device_id = target.into();
        let ended = CancellationToken::new();
        let guard = ended.clone().drop_guard();
        let finished = CancellationToken::new();
        let cleanup = {
            let service = self.clone();
            let source = base.clone();
            let target = destination.clone();
            let finished = finished.clone();
            tokio::spawn(async move {
                tokio::select! {
                    biased;
                    _ = finished.cancelled() => {},
                    _ = ended.cancelled() => {
                        // Cancel also records an operation that has not reached Start yet.
                        let _ = tokio::join!(
                            service.dispatch(ChatTransferCommand { extra: Default::default(), context: source, action: ChatTransferAction::Cancel }),
                            service.dispatch(ChatTransferCommand { extra: Default::default(), context: target, action: ChatTransferAction::CancelReceive }),
                        );
                    }
                }
            })
        };
        let cancel = CancellationToken::new();
        let started = AtomicBool::new(false);
        let result = tokio::select! {
            biased;
            _ = cancelled(ctx) => {
                cancel.cancel();
                Ok(json!({"operation_id":base.operation_id,
                    "status":if started.load(Ordering::SeqCst) { "unknown" } else { "cancelled" },
                    "detail":"Transfer was stopped; a started transfer may already have committed files. It was not replayed."}))
            },
            result = self.run(&base, &destination, ctx, &cancel, &started) => result,
        };
        // A confirmed terminal receipt needs no cancellation. Other exits, including
        // instruction discovery and dropped calls, release the prepared destination.
        if result
            .as_ref()
            .is_ok_and(|value| matches!(value["status"].as_str(), Some("succeeded")))
        {
            finished.cancel();
        }
        drop(guard);
        // Cleanup is bounded by the existing Device command deadlines; do not await
        // network cleanup on the cancelled model/tool future.
        drop(cleanup);
        result
    }

    async fn run(
        &self,
        base: &ChatTransferContext,
        destination: &ChatTransferContext,
        ctx: &ToolContext,
        cancel: &CancellationToken,
        started: &AtomicBool,
    ) -> Result<Value, String> {
        let records = self.approvals.snapshot_at(&base.call.address).await;
        let matching = records
            .pending
            .iter()
            .chain(&records.resolved)
            .filter(|record| {
                record.agent_id == base.call.agent_id
                    && record.turn_id == base.call.turn_id
                    && record.tool_call_id == base.call.tool_call_id
            })
            .collect::<Vec<_>>();
        if matching.len() > 1
            || matching.first().is_some_and(|record| {
                record.status != ApprovalStatus::Approved
                    || record.tool_name != "sync_send"
                    || record.tool_call.args != base.call.arguments
                    || record.trigger.source == rua_security::ApprovalTriggerSource::TargetDevice
            })
        {
            return Err("The approval does not match this active transfer".into());
        }
        let mut approval = matching
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
            .map(|record| record.id.clone());
        loop {
            // Allocate the phases of this exact invocation before consuming its Run Once.
            let permits = approval.as_ref().map(|id| {
                [
                    self.authority
                        .begin_read_dispatch(id, &base.call.target_device_id),
                    self.authority
                        .begin_read_dispatch(id, &destination.call.target_device_id),
                    self.authority
                        .begin_read_dispatch(id, &base.call.target_device_id),
                ]
            });
            let with_confirmation = |context: &ChatTransferContext, index: usize| {
                let mut context = context.clone();
                context.call.confirmation =
                    approval
                        .as_ref()
                        .zip(permits.as_ref())
                        .map(|(id, permits)| DeviceReadConfirmation {
                            approval_id: id.clone(),
                            dispatch_token: permits[index].token(),
                        });
                context
            };
            let source = with_confirmation(base, 0);
            let target = with_confirmation(destination, 1);
            let start = with_confirmation(base, 2);
            let planned = self
                .dispatch(ChatTransferCommand {
                    extra: Default::default(),
                    context: source,
                    action: ChatTransferAction::SourcePlan,
                })
                .await?;
            let (manifest, paths) = match planned {
                ChatTransferResponse::SourcePlan { manifest, paths } => (manifest, paths),
                ChatTransferResponse::Policy {
                    response: DeviceReadCheckReply::ConfirmationRequired,
                } if approval.is_none() => {
                    approval = Some(self.confirm(base, ctx).await?);
                    continue;
                }
                response => {
                    return Err(format!("Source planning was not authorized: {response:?}"))
                }
            };
            let prepared = self
                .dispatch(ChatTransferCommand {
                    extra: Default::default(),
                    context: target,
                    action: ChatTransferAction::PrepareReceive {
                        manifest: manifest.clone(),
                    },
                })
                .await?;
            let prepared = match prepared {
                ChatTransferResponse::Prepared { prepared } => prepared,
                ChatTransferResponse::Policy {
                    response: DeviceReadCheckReply::ConfirmationRequired,
                } if approval.is_none() => {
                    approval = Some(self.confirm(destination, ctx).await?);
                    continue;
                }
                response => {
                    return Err(format!(
                        "Destination preparation was not authorized: {response:?}"
                    ))
                }
            };
            if let Some(instructions) = &ctx.agents_instructions {
                let targets = paths
                    .into_iter()
                    .map(|path| AgentsInstructionTarget {
                        device_id: base.call.target_device_id.clone(),
                        path,
                        kind: AgentsInstructionTargetKind::File,
                    })
                    .chain(
                        prepared
                            .paths
                            .iter()
                            .cloned()
                            .map(|path| AgentsInstructionTarget {
                                device_id: destination.call.target_device_id.clone(),
                                path,
                                kind: AgentsInstructionTargetKind::File,
                            }),
                    )
                    .collect::<Vec<_>>();
                if let AgentsInstructionDecision::Reconsider { reason } = instructions
                    .before_targets(&targets, ctx, cancel.clone())
                    .await
                    .map_err(|error| error.to_string())?
                {
                    return Ok(
                        json!({"status":"not_executed","agents_instructions":"reconsider","reason":reason}),
                    );
                }
            }
            started.store(true, Ordering::SeqCst);
            let initial = self
                .dispatch(ChatTransferCommand {
                    extra: Default::default(),
                    context: start,
                    action: ChatTransferAction::Start {
                        manifest,
                        destination: prepared.clone(),
                    },
                })
                .await;
            // Delivery can fail after Start was accepted. Query only; never resend Start.
            let mut response = initial;
            let mut failed_polls = 0;
            loop {
                match response {
                    Ok(ChatTransferResponse::Operation { operation }) => {
                        if operation.operation_id != base.operation_id {
                            return Ok(unknown_result(
                                base,
                                destination,
                                "The transfer receipt identifies a different operation",
                            ));
                        }
                        failed_polls = 0;
                        if operation.status != TransferJobStatus::Running {
                            return Ok(transfer_result(base, destination, &prepared, operation)
                                .unwrap_or_else(|detail| {
                                    unknown_result(base, destination, &detail)
                                }));
                        }
                    }
                    Ok(other) => {
                        return Ok(unknown_result(
                            base,
                            destination,
                            &format!("Transfer completion could not be confirmed: {other:?}"),
                        ))
                    }
                    Err(ref error) => {
                        failed_polls += 1;
                        if failed_polls >= 3 {
                            return Ok(unknown_result(base, destination, error));
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
                response = self
                    .dispatch(ChatTransferCommand {
                        extra: Default::default(),
                        context: base.clone(),
                        action: ChatTransferAction::Status,
                    })
                    .await;
            }
        }
    }

    async fn confirm(
        &self,
        request: &ChatTransferContext,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        match request
            .call
            .wait_for_confirmation(ctx, &self.approvals, &self.authority)
            .await
            .map_err(|error| error.to_string())?
        {
            Ok((id, _)) => Ok(id),
            Err(DeviceReadCheck::Denied(detail) | DeviceReadCheck::Unavailable(detail)) => {
                Err(detail)
            }
            Err(_) => Err("The user did not approve this transfer".into()),
        }
    }
}

fn transfer_result(
    source: &ChatTransferContext,
    target: &ChatTransferContext,
    prepared: &PreparedChatReceive,
    mut operation: SourceTransferOperationSnapshot,
) -> Result<Value, String> {
    // Receipts follow the manifest order and are translated to the destination's
    // public filesystem, never an Android backing/rootfs path.
    if operation.files.len() > prepared.paths.len() {
        return Err("Invalid target transfer receipt".into());
    }
    for (file, path) in operation.files.iter_mut().zip(&prepared.paths) {
        file.target_path = path.into();
        if let Some(digest) = file.sha256.strip_prefix("sha256:") {
            file.sha256 = digest.to_owned();
        }
    }
    if operation.status == TransferJobStatus::Succeeded
        && (operation.files.len() != prepared.paths.len()
            || operation.files.len() != operation.files_written
            || operation.bytes_written != operation.total_bytes
            || operation.files.iter().any(|file| {
                file.sha256.len() != 64 || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            || operation.files.iter().map(|file| file.bytes).sum::<u64>()
                != operation.bytes_written)
    {
        return Err("The target did not confirm all selected files".into());
    }
    let mut value = serde_json::to_value(operation).map_err(|error| error.to_string())?;
    value["source_device_id"] = source.call.target_device_id.clone().into();
    value["target_device_id"] = target.call.target_device_id.clone().into();
    value["target_dir"] = target.call.arguments["target_dir"].clone();
    value["source_deleted"] = false.into();
    Ok(value)
}

fn unknown_result(
    source: &ChatTransferContext,
    target: &ChatTransferContext,
    detail: &str,
) -> Value {
    json!({"operation_id":source.operation_id,"status":"unknown",
        "source_device_id":source.call.target_device_id,"target_device_id":target.call.target_device_id,
        "detail":detail,"replayed":false})
}

async fn cancelled(ctx: &ToolContext) {
    if let Some(mut receiver) = ctx.cancel.clone() {
        if receiver.wait_for(|stopped| *stopped).await.is_ok() {
            return;
        }
    }
    std::future::pending().await
}

pub struct SyncTransferRuntimeAdapter {
    service: Arc<OnceLock<Arc<SyncTransferToolService>>>,
}

impl SyncTransferRuntimeAdapter {
    pub fn new(service: Arc<OnceLock<Arc<SyncTransferToolService>>>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl rua_tool::extended::sync_transfer::SyncTransferRuntime for SyncTransferRuntimeAdapter {
    async fn execute(
        &self,
        request: rua_tool::extended::sync_transfer::SyncTransferRequest,
        context: &ToolContext,
    ) -> Result<Value, String> {
        let service = self
            .service
            .get()
            .ok_or("Sync Transfer service unavailable")?;
        let rua_tool::extended::sync_transfer::SyncTransferRequest::Send { arguments, .. } =
            request;
        service.send(arguments, context).await
    }
}
