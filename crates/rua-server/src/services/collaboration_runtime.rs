use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;

use super::collaboration::{CollaborationCaller, CollaborationReply, CollaborationService};

fn model_reply(reply: CollaborationReply) -> Result<serde_json::Value, String> {
    let mut value = serde_json::json!({"chat": reply.chat, "state": reply.state});
    if let Some(input_id) = reply.input_id.as_ref() {
        value["input_id"] = input_id.clone().into();
    }
    if let Some(turn_id) = reply.turn_id.as_ref() {
        value["turn_id"] = turn_id.clone().into();
    }
    if reply.request_id.is_some() {
        use super::conversation_authority::ConversationActiveTurnPhase;
        let phase = match (reply.state.as_str(), reply.active_turn.as_ref()) {
            ("queued", Some(active)) => match active.phase {
                ConversationActiveTurnPhase::RunningTool => "waiting_for_tool",
                ConversationActiveTurnPhase::WaitingApproval => "waiting_for_approval",
                _ => "handing_off",
            },
            ("queued", None) => "queued_without_active_turn",
            ("running", _) => "started",
            _ => "terminal",
        };
        value["request_progress"] = serde_json::json!({"phase": phase});
    }
    if let Some(request_id) = reply.request_id {
        value["request_id"] = request_id.clone().into();
        if let Some(outcome) = reply.outcome.as_ref() {
            value["outcome"] = rua_session::collaboration::collaboration_result_page(
                &request_id,
                &outcome.result_id,
                &outcome.status,
                &outcome.output,
                outcome.partial,
                0,
            )?;
        }
    }
    if let Some(active) = reply.active_turn {
        value["active_turn"] = serde_json::to_value(active).map_err(|error| error.to_string())?;
    }
    if let Some(reference) = reply.result_ref {
        value["result_ref"] = serde_json::to_value(reference).map_err(|error| error.to_string())?;
    }
    if let Some(progress) = reply.progress {
        let mut view = serde_json::json!({"status": progress.status});
        if let Some(summary) = progress.summary {
            view["summary"] = summary.into();
            view["stale"] = progress.stale.into();
            if progress.stale {
                view["covered_at"] = progress.covered_at.into();
            }
        }
        if let Some(error) = progress.error {
            view["error"] = error.into();
        }
        value["progress"] = view;
    }
    if let Some(error) = reply.error {
        value["error"] = error.into();
    }
    Ok(value)
}

pub struct CollaborationRuntimeAdapter {
    service: Arc<OnceLock<Weak<CollaborationService>>>,
}

impl CollaborationRuntimeAdapter {
    pub fn new(service: Arc<OnceLock<Weak<CollaborationService>>>) -> Self {
        Self { service }
    }

    fn service(&self) -> Result<Arc<CollaborationService>, String> {
        self.service
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| "Collaboration service unavailable".to_string())
    }
}

#[async_trait]
impl rua_tool::extended::collaboration::CollaborationRuntime for CollaborationRuntimeAdapter {
    async fn request_progress(
        &self,
        source: &rua_core::ChatAddress,
    ) -> Result<serde_json::Value, String> {
        self.service()?.request_progress(source).await
    }
    async fn inventory(&self) -> Result<serde_json::Value, String> {
        self.service
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| "Collaboration service is unavailable".to_string())?
            .inventory()
            .await
    }

    async fn execute(
        &self,
        invocation: rua_tool::extended::collaboration::CollaborationInvocation,
        request: rua_tool::extended::collaboration::CollaborationRequest,
    ) -> Result<serde_json::Value, String> {
        let service = self.service()?;
        let caller = CollaborationCaller {
            address: invocation.source,
            turn_id: invocation.turn_id,
            tool_call_id: invocation.tool_call_id,
            configuration_epoch: invocation.configuration_epoch,
            working_dir: invocation.working_dir,
        };
        match request {
            rua_tool::extended::collaboration::CollaborationRequest::CreateAgent {
                agent_id,
                message,
                yield_seconds,
            } => model_reply(
                service
                    .create(caller, agent_id, message, yield_seconds)
                    .await?,
            ),
            rua_tool::extended::collaboration::CollaborationRequest::CreateAcp {
                runner,
                message,
                yield_seconds,
            } => model_reply(
                service
                    .create_acp(caller, runner, message, yield_seconds)
                    .await?,
            ),
            rua_tool::extended::collaboration::CollaborationRequest::Send {
                chat,
                message,
                yield_seconds,
            } => model_reply(service.send(caller, chat, message, yield_seconds).await?),
            rua_tool::extended::collaboration::CollaborationRequest::Read {
                chat,
                yield_seconds,
                result,
            } => {
                if let Some(cursor) = result {
                    let reply = service.read_result(caller, chat, cursor.clone()).await?;
                    let outcome = reply.outcome.as_ref().ok_or("Result is unavailable")?;
                    let page = rua_session::collaboration::collaboration_result_page(
                        &cursor.request_id,
                        &cursor.result_id,
                        &outcome.status,
                        &outcome.output,
                        outcome.partial,
                        cursor.offset,
                    )?;
                    let mut value = model_reply(reply)?;
                    value["outcome"] = page;
                    Ok(value)
                } else {
                    model_reply(service.read(caller, chat, yield_seconds).await?)
                }
            }
            rua_tool::extended::collaboration::CollaborationRequest::StopTurn {
                chat,
                turn_id,
                authority_epoch,
            } => {
                let authority_epoch =
                    serde_json::from_value(serde_json::Value::String(authority_epoch))
                        .map_err(|error| error.to_string())?;
                let reply = service
                    .stop_turn(caller, chat, turn_id, authority_epoch)
                    .await?;
                Ok(
                    serde_json::json!({"chat": reply.chat, "turn_id": reply.turn_id, "status": reply.status}),
                )
            }
        }
    }
}

pub fn server_tool_catalog(
) -> Result<serde_json::Map<String, serde_json::Value>, rua_tool::ToolError> {
    let mut registry = rua_tool::ToolRegistry::new();
    rua_tool::register_all_builtin_tools(
        &mut registry,
        rua_tool::default_command_executor(),
        None,
    )?;
    rua_tool::register_extended_tools(&mut registry, rua_tool::ToolProfile::ServerFull)?;
    let mut catalog = rua_config::types::tools::default_builtin_tools();
    catalog.extend(rua_tool::tool_catalog_from_specs(registry.list_specs()));
    Ok(catalog)
}
