use std::sync::Weak;
use std::time::Duration;

use rua_core::tool::{DeviceReadAccess, DeviceReadCheck, ToolContext};
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
    deliver_conversation_control_via_session, deliver_read_check_via_session,
    ConversationControlDeviceAction, ConversationControlDeviceCommand,
    ConversationControlDeviceResponse,
};
use super::device_session_access::DeviceNetworkAccess;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceReadCheckRequest {
    pub source_device_id: String,
    pub target_device_id: String,
    pub agent_id: String,
    pub address: ChatAddress,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub configuration_epoch: String,
    pub turn_id: String,
    pub tool_call_id: String,
    pub approval_id: String,
    pub dispatch_token: Uuid,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DeviceReadCheckReply {
    Unavailable,
    Denied,
    Checked,
}

pub struct ForegroundReadAccess {
    pub authority: Weak<ConversationAuthority>,
    pub approvals: Weak<rua_security::ApprovalAuthority>,
    pub network: Weak<DeviceNetworkAccess>,
}

#[async_trait::async_trait]
impl DeviceReadAccess for ForegroundReadAccess {
    async fn check(&self, ctx: &ToolContext, args: &Value) -> rua_core::Result<DeviceReadCheck> {
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
        let Some(path) = args.get("file_path").and_then(Value::as_str) else {
            return Ok(unavailable("missing file path"));
        };
        if !rua_core::execution_directory::is_device_absolute_path(path)
            || path.starts_with("//")
            || path.starts_with(r"\\")
            || args.as_object().is_none_or(|args| {
                args.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "device_id" | "file_path" | "start_line" | "limit" | "byte_offset"
                    )
                })
            })
        {
            return Ok(unavailable(
                "this check supports a named regular file with an absolute path",
            ));
        }
        let Some(address) = ctx
            .metadata
            .get(rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)
            .and_then(|value| serde_json::from_value::<ChatAddress>(value.clone()).ok())
        else {
            return Ok(unavailable("the requesting Chat was not identified"));
        };
        let Some(active) = authority.active_turn(&address).await.filter(|active| {
            active.authority_device_id == local
                && active.turn_id == turn
                && active.driver == ConversationDriverKind::Rua
                && active.phase == ConversationActiveTurnPhase::RunningTool
        }) else {
            return Ok(unavailable(
                "the original tool call is no longer running on this device",
            ));
        };
        let records = approvals.snapshot_at(&address).await;
        let mut matches = records
            .resolved
            .iter()
            .filter(|r| r.agent_id == ctx.agent_id && r.turn_id == turn && r.tool_call_id == call);
        let Some(record) = matches.next() else {
            return Ok(unavailable(
                "no confirmed single-file request is available for this check",
            ));
        };
        if matches.next().is_some()
            || record.status != ApprovalStatus::Approved
            || record.tool_name != "read"
            || record.tool_call.args != *args
            || record
                .authenticated_user_decision
                .as_ref()
                .is_none_or(|user| user.decision != AuthenticatedApprovalDecision::RunOnce)
        {
            return Ok(unavailable(
                "no matching one-time user decision is available for this check",
            ));
        }
        let transport = network
            .lock()
            .await
            .as_ref()
            .map(|runtime| runtime.session_manager().clone());
        let Some(transport) = transport else {
            return Ok(unavailable("the device network is unavailable"));
        };
        let permit = authority.begin_read_dispatch(&record.id, device);
        let request = DeviceReadCheckRequest {
            source_device_id: local.into(),
            target_device_id: device.into(),
            agent_id: ctx.agent_id.clone(),
            address,
            authority_epoch: active.authority_epoch,
            configuration_epoch: epoch.into(),
            turn_id: turn.into(),
            tool_call_id: call.into(),
            approval_id: record.id.clone(),
            dispatch_token: permit.token(),
            arguments: args.clone(),
        };
        let check = deliver_read_check_via_session(&transport, &request, Duration::from_secs(20));
        let response = if let Some(mut cancel) = ctx.cancel.clone() {
            tokio::select! {
                biased;
                _ = async {
                    if cancel.wait_for(|stopped| *stopped).await.is_err() {
                        std::future::pending::<()>().await;
                    }
                } => return Err(rua_core::RuaError::Cancelled),
                response = check => response,
            }
        } else {
            check.await
        };
        drop(permit);
        Ok(match response {
            Ok(DeviceReadCheckReply::Checked) => DeviceReadCheck::Checked,
            Ok(DeviceReadCheckReply::Denied) => DeviceReadCheck::Denied(
                "the target device denied this read under its current policy".into(),
            ),
            Ok(DeviceReadCheckReply::Unavailable) => {
                unavailable("the target could not verify the Chat or this one-time read")
            }
            Err(_) => unavailable("the target did not confirm the permission check"),
        })
    }
}

impl DeviceReadCheckRequest {
    pub(super) async fn verify_at_origin(
        &self,
        manager: &rua_device_network::DeviceSessionManager,
    ) -> bool {
        let command = ConversationControlDeviceCommand {
            command_id: Uuid::new_v4().to_string(),
            address: self.address.clone(),
            agent_id: self.agent_id.clone(),
            authority_epoch: self.authority_epoch,
            source_device_id: self.target_device_id.clone(),
            target_device_id: self.source_device_id.clone(),
            action: ConversationControlDeviceAction::ClaimReadApproval {
                approval_id: self.approval_id.clone(),
                dispatch_token: self.dispatch_token,
                turn_id: self.turn_id.clone(),
                tool_call_id: self.tool_call_id.clone(),
                arguments: self.arguments.clone(),
            },
        };
        matches!(deliver_conversation_control_via_session(manager, &command, Duration::from_secs(8)).await,
            Ok(ConversationControlDeviceResponse::ReadApprovalClaimed {approval_id}) if approval_id == self.approval_id)
    }
}
