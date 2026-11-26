use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use rua_core::agents_instruction_discovery::{
    AgentsInstructionDiscovery, AgentsInstructionDiscoveryRequest, AgentsInstructionLoader,
    AGENTS_INSTRUCTIONS_MAX_BYTES,
};
use rua_core::tool::ToolContext;
use rua_core::{ChatAddress, Result, RuaError};
use tokio_util::sync::CancellationToken;

use super::chat_file_read_admission::TargetFileReadContext;
use super::conversation_authority::{
    ConversationActiveTurn, ConversationActiveTurnPhase, ConversationAuthority,
};
use super::device_command_bus::{
    deliver_agents_instructions_via_session, AgentsInstructionCommand,
};
use super::device_read_check::DeviceReadCheckRequest;
use super::device_session_access::DeviceNetworkAccess;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Read explicit paths on their owning Device using the current Chat configuration.
/// Missing instructions, unavailable Devices and reads requiring approval are best-effort skips.
pub struct RuntimeAgentsInstructionLoader {
    pub local: Arc<OnceLock<Arc<TargetFileReadContext>>>,
    pub authority: Weak<ConversationAuthority>,
    pub network: Weak<DeviceNetworkAccess>,
}

fn skipped(detail: &str) -> AgentsInstructionDiscovery {
    AgentsInstructionDiscovery {
        diagnostics: vec![detail.into()],
        ..Default::default()
    }
}

fn is_current(active: &ConversationActiveTurn, local: &str, turn: &str) -> bool {
    active.authority_device_id == local
        && active.turn_id == turn
        && matches!(
            active.phase,
            ConversationActiveTurnPhase::Preparing
                | ConversationActiveTurnPhase::Streaming
                | ConversationActiveTurnPhase::WaitingApproval
                | ConversationActiveTurnPhase::RunningTool
        )
}

#[async_trait::async_trait]
impl AgentsInstructionLoader for RuntimeAgentsInstructionLoader {
    async fn discover(
        &self,
        discovery: &AgentsInstructionDiscoveryRequest,
        ctx: &ToolContext,
        cancellation: CancellationToken,
    ) -> Result<AgentsInstructionDiscovery> {
        let cancel = cancellation.child_token();
        let _cancel_on_drop = cancel.clone().drop_guard();
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(RuaError::Cancelled),
            result = tokio::time::timeout(DISCOVERY_TIMEOUT, self.discover_current(discovery, ctx, cancel.clone())) => {
                match result {
                    Ok(result) => result,
                    Err(_) => Ok(skipped("AGENTS.md discovery timed out; continuing without these instructions.")),
                }
            }
        }
    }
}

impl RuntimeAgentsInstructionLoader {
    async fn discover_current(
        &self,
        discovery: &AgentsInstructionDiscoveryRequest,
        ctx: &ToolContext,
        cancellation: CancellationToken,
    ) -> Result<AgentsInstructionDiscovery> {
        let field = |name: &str| ctx.metadata.get(name).and_then(serde_json::Value::as_str);
        let (Some(local), Some(turn), Some(authority)) = (
            ctx.local_device_id.as_deref(),
            field("turn_id"),
            self.authority.upgrade(),
        ) else {
            return Ok(skipped(
                "AGENTS.md discovery has no active Chat configuration.",
            ));
        };
        let Some(address) = ctx
            .metadata
            .get(rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)
            .and_then(|value| serde_json::from_value::<ChatAddress>(value.clone()).ok())
        else {
            return Ok(skipped("AGENTS.md discovery has no Chat address."));
        };
        let Some(active) = authority
            .active_turn(&address)
            .await
            .filter(|active| is_current(active, local, turn))
        else {
            return Ok(skipped(
                "The Turn is no longer active for AGENTS.md discovery.",
            ));
        };
        let context = DeviceReadCheckRequest {
            tool_name: "read".into(),
            source_device_id: local.into(),
            target_device_id: discovery.device_id.clone(),
            agent_id: ctx.agent_id.clone(),
            address: address.clone(),
            authority_epoch: active.authority_epoch,
            turn_id: turn.into(),
            tool_call_id: field("tool_call_id").unwrap_or_default().into(),
            confirmation: None,
            arguments: serde_json::json!({}),
        };
        let result = if discovery.device_id == local {
            let Some(local) = self.local.get() else {
                return Ok(skipped("The AGENTS.md reader is unavailable."));
            };
            local
                .discover_agents_instructions(&context.as_target_read(), discovery, cancellation)
                .await
        } else {
            let Some(network) = self.network.upgrade() else {
                return Ok(skipped(
                    "The Device network is unavailable for AGENTS.md discovery.",
                ));
            };
            let manager = match network
                .session_manager(tokio::time::Instant::now() + DISCOVERY_TIMEOUT)
                .await
            {
                Ok(manager) => manager,
                Err(_) => {
                    return Ok(skipped(
                        "The Device network is unavailable for AGENTS.md discovery.",
                    ))
                }
            };
            match deliver_agents_instructions_via_session(
                &manager,
                &AgentsInstructionCommand {
                    context,
                    discovery: discovery.clone(),
                },
                DISCOVERY_TIMEOUT,
            )
            .await
            {
                Ok(result) => Ok(result),
                Err(_) => Ok(skipped(
                    "The owning Device could not provide AGENTS.md instructions.",
                )),
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(RuaError::Cancelled) => return Err(RuaError::Cancelled),
            Err(_) => return Ok(skipped("AGENTS.md instructions could not be read.")),
        };
        if !authority
            .active_turn(&address)
            .await
            .is_some_and(|current| {
                is_current(&current, local, turn)
                    && current.authority_epoch == active.authority_epoch
            })
        {
            return Ok(skipped("The Turn ended during AGENTS.md discovery."));
        }
        let bytes = result.sources.iter().fold(0usize, |bytes, source| {
            bytes.saturating_add(source.content.len())
        });
        if bytes > discovery.max_bytes.min(AGENTS_INSTRUCTIONS_MAX_BYTES)
            || result
                .sources
                .iter()
                .any(|source| source.device_id != discovery.device_id)
        {
            return Ok(skipped(
                "The owning Device returned an invalid AGENTS.md snapshot.",
            ));
        }
        Ok(result)
    }
}
