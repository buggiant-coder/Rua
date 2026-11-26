use super::*;
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use serde_json::json;

impl ChatTaskService {
    pub async fn handle(&self, command: ChatTaskCommand) -> Result<ChatTaskResponse, String> {
        if command.target_device_id != self.local_device_id {
            return Err("Task request belongs to another execution Device".into());
        }
        // Acceptance survives a lost RPC response. The Task Runtime's persisted
        // record determines ownership; dropping the transport cannot undo it.
        if matches!(
            command.action,
            ChatTaskAction::Create { .. } | ChatTaskAction::Retry { .. }
        ) {
            let service = self.clone();
            return tokio::spawn(async move {
                let _acceptance = service.acceptance.lock().await;
                service.handle_target(command).await
            })
            .await
            .map_err(|e| format!("Task acceptance is unconfirmed: {e}"))?;
        }
        self.handle_target(command).await
    }

    async fn handle_target(&self, command: ChatTaskCommand) -> Result<ChatTaskResponse, String> {
        let owner = &command.owner;
        let result = match command.action {
            ChatTaskAction::PrepareCreate {
                task_id,
                spec,
                invocation,
            } => {
                if spec.owner != *owner {
                    return Err("Task belongs to another Chat".into());
                }
                self.check_invocation(
                    &command.source_device_id,
                    owner,
                    &invocation,
                    "task_create",
                    &json!({"runner":spec.runner}),
                )
                .await?;
                let device = self
                    .resolver
                    .accept_attempt(&spec, &invocation.authorization, None)
                    .await
                    .map_err(|e| e.to_string())?;
                let mut spec = *spec;
                if let rua_task::TaskRunnerSpec::Tool {
                    tool_name,
                    arguments,
                } = &mut spec.runner
                {
                    if tool_name == "bash" {
                        arguments["device_id"] = json!(self.local_device_id);
                        arguments["cwd"] = json!(device.public_cwd);
                    }
                }
                return Ok(ChatTaskResponse::Prepared {
                    preparation: Box::new(ChatTaskPreparation {
                        task_id,
                        spec,
                        device,
                        invocation,
                        expected_attempt_no: None,
                        extra: Default::default(),
                    }),
                });
            }
            ChatTaskAction::PrepareRetry {
                task_id,
                invocation,
            } => {
                self.check_invocation(
                    &command.source_device_id,
                    owner,
                    &invocation,
                    "task_retry",
                    &json!({"task_id":task_id}),
                )
                .await?;
                let record = self
                    .runtime
                    .get(owner, &task_id)
                    .await
                    .map_err(|e| e.to_string())?;
                let prior = record
                    .attempts
                    .iter()
                    .find(|a| a.attempt_no == record.attempt_no)
                    .ok_or("Task has no current Attempt")?;
                let device = self
                    .resolver
                    .accept_attempt(&record.spec, &invocation.authorization, Some(&prior.device))
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok(ChatTaskResponse::Prepared {
                    preparation: Box::new(ChatTaskPreparation {
                        task_id,
                        spec: record.spec,
                        device,
                        invocation,
                        expected_attempt_no: Some(record.attempt_no),
                        extra: Default::default(),
                    }),
                });
            }
            ChatTaskAction::Create { preparation } => {
                let preflight: Result<(), String> = async {
                    if preparation.spec.owner != *owner || preparation.expected_attempt_no.is_some()
                    {
                        return Err("Task creation does not match the owning Chat".into());
                    }
                    self.check_invocation(
                        &command.source_device_id,
                        owner,
                        &preparation.invocation,
                        "task_create",
                        &json!({"runner":preparation.spec.runner}),
                    )
                    .await?;
                    let actual = self
                        .resolver
                        .accept_attempt(
                            &preparation.spec,
                            &preparation.invocation.authorization,
                            None,
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    same_position(&actual, &preparation.device)
                }
                .await;
                if let Err(detail) = preflight {
                    return Ok(ChatTaskResponse::Rejected {
                        reason: "task_not_accepted".into(),
                        detail,
                    });
                }
                match self
                    .runtime
                    .create_with_id(
                        preparation.task_id.clone(),
                        preparation.spec,
                        preparation.invocation.authorization,
                    )
                    .await
                {
                    Ok(task_id) => return self.record_response(owner, &task_id).await,
                    Err(
                        error @ (rua_task::TaskError::Invalid(_)
                        | rua_task::TaskError::RunnerUnavailable(_)
                        | rua_task::TaskError::LimitExceeded(_)
                        | rua_task::TaskError::AuthorityRejected(_)
                        | rua_task::TaskError::AuthorityUnavailable(_)
                        | rua_task::TaskError::ResourceUnavailable(_)),
                    ) => {
                        return Ok(ChatTaskResponse::Rejected {
                            reason: "task_not_accepted".into(),
                            detail: error.to_string(),
                        });
                    }
                    Err(error) => Err(error),
                }
            }
            ChatTaskAction::Retry { preparation } => {
                self.check_invocation(
                    &command.source_device_id,
                    owner,
                    &preparation.invocation,
                    "task_retry",
                    &json!({"task_id":preparation.task_id}),
                )
                .await?;
                let existing = self
                    .runtime
                    .get(owner, &preparation.task_id)
                    .await
                    .map_err(|e| e.to_string())?;
                let expected = preparation
                    .expected_attempt_no
                    .ok_or("Retry has no original Attempt")?;
                if existing.spec != preparation.spec {
                    return Err("Retry cannot change Task Spec".into());
                }
                if existing.attempt_no != expected {
                    let latest = existing
                        .attempts
                        .iter()
                        .find(|a| a.attempt_no == existing.attempt_no)
                        .ok_or("Task has no current Attempt")?;
                    if existing.attempt_no == expected.saturating_add(1)
                        && latest.authorization_origin
                            == preparation.invocation.authorization.origin
                        && latest.authorization_agent_id
                            == preparation.invocation.authorization.agent_id
                    {
                        return self.record_response(owner, &preparation.task_id).await;
                    }
                    return Err("Task Attempt changed before retry".into());
                }
                let prior = existing
                    .attempts
                    .iter()
                    .find(|a| a.attempt_no == expected)
                    .ok_or("Task original execution position is unavailable")?;
                same_position(&prior.device, &preparation.device)?;
                match self
                    .runtime
                    .retry(
                        owner,
                        &preparation.task_id,
                        preparation.invocation.authorization,
                    )
                    .await
                {
                    Ok(record) => return self.record_response(owner, &record.task_id).await,
                    Err(error) => Err(error),
                }
            }
            ChatTaskAction::Get { task_id } => return self.record_response(owner, &task_id).await,
            ChatTaskAction::Tool {
                tool_name,
                arguments,
                tool_call_id,
            } => {
                return self
                    .target_tool(owner, &tool_name, arguments, tool_call_id)
                    .await;
            }
            ChatTaskAction::Input {
                task_id,
                submission,
            } => self
                .runtime
                .send_input(owner, &task_id, submission)
                .await
                .and_then(to_value),
            ChatTaskAction::Cancel { task_id } => {
                match self.runtime.cancel(owner, &task_id).await {
                    Ok(_) => return self.record_response(owner, &task_id).await,
                    Err(error) => Err(error),
                }
            }
            ChatTaskAction::Watch { task_id } => {
                self.runtime.watch(owner, &task_id).await.and_then(to_value)
            }
            ChatTaskAction::Unwatch { task_id } => self
                .runtime
                .unwatch(owner, &task_id)
                .await
                .and_then(to_value),
            ChatTaskAction::Result { task_id } => self
                .runtime
                .read_full_result(owner, &task_id)
                .await
                .and_then(|bytes| {
                    serde_json::from_slice(&bytes)
                        .map_err(|e| rua_task::TaskError::Store(e.to_string()))
                }),
            ChatTaskAction::ApprovalSnapshot {
                task_id,
                attempt_no,
            } => {
                let approvals = self.target.security().approval_authority();
                let snapshot = super::super::chat_task_approval::target_snapshot(
                    &self.runtime,
                    approvals,
                    owner,
                    &task_id,
                    attempt_no,
                )
                .await?;
                return Ok(ChatTaskResponse::Value {
                    value: serde_json::to_value(snapshot).map_err(|e| e.to_string())?,
                });
            }
            ChatTaskAction::ApprovalDecide {
                task_id,
                attempt_no,
                approval_id,
                decision,
                authenticated_user_id,
            } => {
                if matches!(
                    decision,
                    super::super::device_command_bus::ConversationApprovalDecision::AllowChat { .. }
                ) {
                    let scope = ChatStore::resolve(&self.root, owner.chat_address.clone());
                    let placement = super::super::conversation_authority_target::load_chat_authority_placement_target(scope.root(), owner.chat_id()).await?;
                    if placement.device_id != self.local_device_id
                        || command.source_device_id != self.local_device_id
                    {
                        return Err(
                            "Persistent Chat approval must be committed by the Chat Authority"
                                .into(),
                        );
                    }
                }
                let approvals = self.target.security().approval_authority();
                let publish_decision = decision.clone();
                let request = super::super::chat_task_approval::target_decide(
                    &self.runtime,
                    approvals,
                    owner,
                    &task_id,
                    attempt_no,
                    &approval_id,
                    decision,
                    &authenticated_user_id,
                )
                .await?;
                if let Some(runtime) = self.publication.get() {
                    use super::super::device_command_bus::ConversationApprovalDecision;
                    use crate::sync_publication::{
                        publish_sync_fact_with_runtime, SyncPublicationFact,
                    };
                    let fact = match publish_decision {
                        ConversationApprovalDecision::AllowChat { .. } => {
                            let scope = ChatStore::resolve(&self.root, owner.chat_address.clone());
                            Some(SyncPublicationFact::StableConversationState {
                                scope_root: scope.root().to_path_buf(),
                                address: owner.chat_address.clone(),
                            })
                        }
                        ConversationApprovalDecision::AllowAgent { .. } => {
                            let config = self
                                .target
                                .security()
                                .config_store()
                                .load(&request.agent_id)
                                .map_err(|e| e.to_string())?;
                            Some(SyncPublicationFact::AgentSecurityCommitted {
                                agent_id: request.agent_id.clone(),
                                config: serde_json::to_value(config).map_err(|e| e.to_string())?,
                            })
                        }
                        _ => None,
                    };
                    if let Some(fact) = fact {
                        let result = publish_sync_fact_with_runtime(runtime, fact).await;
                        if !result.errors.is_empty() {
                            tracing::warn!(errors = ?result.errors, "Task approval synchronization remains pending");
                        }
                    }
                }
                return Ok(ChatTaskResponse::Value {
                    value: serde_json::to_value(request).map_err(|e| e.to_string())?,
                });
            }
            ChatTaskAction::ApprovalDecideAtAuthority {
                task_id,
                attempt_no,
                approval_id,
                decision,
                authenticated_user_id,
                authority_epoch,
            } => {
                let request = self
                    .decide_approval_at_authority(
                        owner,
                        &task_id,
                        attempt_no,
                        &approval_id,
                        decision,
                        &authenticated_user_id,
                        authority_epoch,
                    )
                    .await?;
                return Ok(ChatTaskResponse::Value {
                    value: serde_json::to_value(request).map_err(|error| error.to_string())?,
                });
            }
            ChatTaskAction::ApprovalApplyAtExecution {
                task_id,
                attempt_no,
                authority_device_id,
                authority_epoch,
                original,
                decided,
            } => {
                let gate = self.authority.transition_gate(&owner.chat_address);
                let _transition = gate.lock().await;
                let scope = ChatStore::resolve(&self.root, owner.chat_address.clone());
                let placement = super::super::conversation_authority_target::load_chat_authority_placement_target(scope.root(), owner.chat_id()).await?;
                if command.source_device_id != authority_device_id
                    || placement.device_id != authority_device_id
                    || placement.authority_epoch != authority_epoch
                {
                    return Err(
                        "Task approval decision is not from the current Chat Authority".into(),
                    );
                }
                let request = super::super::chat_task_approval::target_apply_authority_decision(
                    &self.runtime,
                    self.target.security().approval_authority(),
                    owner,
                    &task_id,
                    attempt_no,
                    &original,
                    &decided,
                )
                .await?;
                return Ok(ChatTaskResponse::Value {
                    value: serde_json::to_value(request).map_err(|error| error.to_string())?,
                });
            }
        };
        Ok(match result {
            Ok(value) => ChatTaskResponse::Value { value },
            Err(error) => task_error(error),
        })
    }

    async fn check_invocation(
        &self,
        source: &str,
        owner: &TaskOwner,
        invocation: &ChatTaskInvocation,
        tool: &str,
        arguments: &Value,
    ) -> Result<(), String> {
        let authorization = &invocation.authorization;
        if authorization.origin.turn_id.is_some() && source != invocation.authority_device_id {
            return Err("Task invocation does not originate at its Chat Authority".into());
        }
        let request = super::super::chat_file_read_admission::TargetChatRead {
            address: &owner.chat_address,
            agent_id: &authorization.agent_id,
            authority_device_id: &invocation.authority_device_id,
            authority_epoch: invocation.authority_epoch,
            turn_id: authorization.origin.turn_id.as_deref().unwrap_or_default(),
            tool_call_id: authorization
                .origin
                .tool_call_id
                .as_deref()
                .unwrap_or_default(),
            tool_name: tool,
            args: arguments,
        };
        if authorization.origin.turn_id.is_some() {
            self.target
                .checked_tool_configuration(&request)
                .await
                .map_err(|e| e.to_string())?;
        } else {
            self.target
                .checked_chat_configuration(&request)
                .await
                .map_err(|e| e.to_string())?;
        }
        if tool == "task_create" {
            let runner = arguments
                .get("runner")
                .ok_or("Task Runner is unavailable")?;
            let name = runner
                .get("tool_name")
                .and_then(Value::as_str)
                .ok_or("Task requires a Tool Runner")?;
            let inner = runner
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if name == "bash" {
                rua_tool::core::ShellTool::validate_arguments(&inner).map_err(|e| e.to_string())?;
            }
            let request = super::super::chat_file_read_admission::TargetChatRead {
                tool_name: name,
                args: &inner,
                ..request
            };
            self.target
                .checked_tool_configuration(&request)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    async fn record_response(
        &self,
        owner: &TaskOwner,
        task_id: &str,
    ) -> Result<ChatTaskResponse, String> {
        match self.runtime.get(owner, task_id).await {
            Ok(record) => {
                let reference = match self.capture_and_publish(&record).await {
                    Ok(reference) => Some(Box::new(reference)),
                    Err(error) => {
                        tracing::warn!(%error, %task_id, "Task reference publication remains pending");
                        None
                    }
                };
                Ok(ChatTaskResponse::Record {
                    record: Box::new(record),
                    reference,
                })
            }
            Err(error) => Ok(task_error(error)),
        }
    }

    async fn target_tool(
        &self,
        owner: &TaskOwner,
        name: &str,
        arguments: Value,
        tool_call_id: Option<String>,
    ) -> Result<ChatTaskResponse, String> {
        use rua_tool::extended::task_tools::*;
        let tool: Box<dyn Tool> = match name {
            "task_read" => Box::new(TaskReadTool::new()),
            "task_send_input" => Box::new(TaskSendInputTool::new()),
            "task_cancel" => Box::new(TaskCancelTool::new()),
            "Task_Watch" => Box::new(TaskWatchTool::new()),
            "Task_Unwatch" => Box::new(TaskUnwatchTool::new()),
            "task_await" => Box::new(TaskAwaitTool::new()),
            _ => return Err("This operation requires Task acceptance or Chat-wide routing".into()),
        };
        let mut context = ToolContext {
            session_id: owner.chat_id().into(),
            local_device_id: Some(self.local_device_id.clone()),
            task_runtime: Some(Arc::new(TaskRuntimeHandle::new(Arc::clone(&self.runtime)))),
            ..Default::default()
        };
        context.metadata.insert(
            rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.into(),
            serde_json::to_value(&owner.chat_address).map_err(|e| e.to_string())?,
        );
        if let Some(call_id) = tool_call_id {
            context
                .metadata
                .insert("tool_use_id".into(), call_id.clone().into());
            context
                .metadata
                .insert("tool_call_id".into(), call_id.into());
        }
        match tool.call(arguments, context).await {
            Ok(result) => Ok(ChatTaskResponse::Tool {
                result: Box::new(result),
            }),
            Err(error) => Ok(ChatTaskResponse::Rejected {
                reason: "task_operation_failed".into(),
                detail: error.to_string(),
            }),
        }
    }
}

fn to_value(value: impl Serialize) -> Result<Value, rua_task::TaskError> {
    serde_json::to_value(value).map_err(|e| rua_task::TaskError::Store(e.to_string()))
}

pub(super) fn same_position(
    actual: &TaskDeviceContext,
    expected: &TaskDeviceContext,
) -> Result<(), String> {
    if actual.device_id != expected.device_id || actual.public_cwd != expected.public_cwd {
        Err("The prepared Task execution position changed".into())
    } else {
        Ok(())
    }
}

pub(super) fn task_error(error: rua_task::TaskError) -> ChatTaskResponse {
    let reason = match &error {
        rua_task::TaskError::NotFound(_) => "task_not_found",
        rua_task::TaskError::Invalid(_)
        | rua_task::TaskError::InvalidTransition(_)
        | rua_task::TaskError::LimitExceeded(_)
        | rua_task::TaskError::AuthorityRejected(_)
        | rua_task::TaskError::InputUnavailable(_)
        | rua_task::TaskError::InputConflict(_) => "task_invalid",
        _ => "task_unavailable",
    };
    ChatTaskResponse::Rejected {
        reason: reason.into(),
        detail: error.to_string(),
    }
}
