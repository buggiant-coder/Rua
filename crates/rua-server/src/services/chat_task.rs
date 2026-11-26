use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use rua_core::tool::{ToolContext, ToolResult};
use rua_core::ChatStore;
use rua_session::task_refs::{ChatTaskExecution, ChatTaskOrigin, ChatTaskRef};
use rua_task::{TaskAttemptAuthorization, TaskDeviceContext, TaskOwner, TaskRecord, TaskSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::chat_command::{ChatCommandContext, ChatCommandPreparation};
use super::chat_file_read_admission::TargetFileReadContext;
use super::conversation_authority::ConversationAuthority;
use super::conversation_authority_placement::ConversationAuthorityEpoch;
use super::device_session_access::DeviceNetworkAccess;

mod approvals;
mod recovery;
mod target;
mod tools;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatTaskInvocation {
    pub authorization: TaskAttemptAuthorization,
    pub authority_device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatTaskPreparation {
    pub task_id: String,
    pub spec: TaskSpec,
    pub device: TaskDeviceContext,
    pub invocation: ChatTaskInvocation,
    pub expected_attempt_no: Option<u32>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatTaskCommand {
    pub source_device_id: String,
    pub target_device_id: String,
    pub owner: TaskOwner,
    pub action: ChatTaskAction,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ChatTaskAction {
    PrepareCreate {
        task_id: String,
        spec: Box<TaskSpec>,
        invocation: ChatTaskInvocation,
    },
    Create {
        preparation: Box<ChatTaskPreparation>,
    },
    PrepareRetry {
        task_id: String,
        invocation: ChatTaskInvocation,
    },
    Retry {
        preparation: Box<ChatTaskPreparation>,
    },
    Get {
        task_id: String,
    },
    Tool {
        tool_name: String,
        arguments: Value,
        #[serde(default)]
        tool_call_id: Option<String>,
    },
    Input {
        task_id: String,
        submission: rua_task::TaskInputSubmission,
    },
    Cancel {
        task_id: String,
    },
    Watch {
        task_id: String,
    },
    Unwatch {
        task_id: String,
    },
    Result {
        task_id: String,
    },
    ApprovalSnapshot {
        task_id: String,
        attempt_no: u32,
    },
    ApprovalDecide {
        task_id: String,
        attempt_no: u32,
        approval_id: String,
        decision: super::device_command_bus::ConversationApprovalDecision,
        authenticated_user_id: String,
    },
    ApprovalDecideAtAuthority {
        task_id: String,
        attempt_no: u32,
        approval_id: String,
        decision: super::device_command_bus::ConversationApprovalDecision,
        authenticated_user_id: String,
        authority_epoch: ConversationAuthorityEpoch,
    },
    ApprovalApplyAtExecution {
        task_id: String,
        attempt_no: u32,
        authority_device_id: String,
        authority_epoch: ConversationAuthorityEpoch,
        original: Box<rua_security::approval_authority::ApprovalRequest>,
        decided: Box<rua_security::approval_authority::ApprovalRequest>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatTaskResponse {
    Prepared {
        preparation: Box<ChatTaskPreparation>,
    },
    Record {
        record: Box<TaskRecord>,
        reference: Option<Box<ChatTaskRef>>,
    },
    Tool {
        result: Box<ToolResult>,
    },
    Value {
        value: Value,
    },
    Rejected {
        reason: String,
        detail: String,
    },
}

/// Routing and discovery live with the Chat. Execution, attempts, input and
/// durable results remain exclusively in the accepting Device's Task Runtime.
#[derive(Clone)]
pub struct ChatTaskService {
    root: PathBuf,
    local_device_id: String,
    runtime: Arc<rua_task::TaskRuntime>,
    resolver: Arc<dyn rua_task::TaskAuthorityResolver>,
    target: Arc<TargetFileReadContext>,
    network: Arc<DeviceNetworkAccess>,
    authority: Arc<ConversationAuthority>,
    publication: OnceLock<crate::sync_publication::SyncPublicationRuntime>,
    acceptance: Arc<tokio::sync::Mutex<()>>,
}

impl ChatTaskService {
    pub fn new(
        root: PathBuf,
        local_device_id: String,
        runtime: Arc<rua_task::TaskRuntime>,
        resolver: Arc<dyn rua_task::TaskAuthorityResolver>,
        target: Arc<TargetFileReadContext>,
        network: Arc<DeviceNetworkAccess>,
        authority: Arc<ConversationAuthority>,
    ) -> Self {
        Self {
            root,
            local_device_id,
            runtime,
            resolver,
            target,
            network,
            authority,
            publication: OnceLock::new(),
            acceptance: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub fn install_publication(&self, runtime: crate::sync_publication::SyncPublicationRuntime) {
        let _ = self.publication.set(runtime);
    }

    pub async fn approval_snapshot(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        attempt_no: u32,
    ) -> Result<rua_security::approval_authority::ApprovalSnapshot, String> {
        let device = self.execution_device(owner, task_id).await?;
        let reply = self
            .dispatch(
                &device,
                owner,
                ChatTaskAction::ApprovalSnapshot {
                    task_id: task_id.into(),
                    attempt_no,
                },
            )
            .await?;
        response_value(reply)
            .and_then(|value| serde_json::from_value(value).map_err(|e| e.to_string()))
    }

    pub async fn decide_task_approval(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        attempt_no: u32,
        approval_id: &str,
        decision: super::device_command_bus::ConversationApprovalDecision,
        authenticated_user_id: &str,
    ) -> Result<rua_security::approval_authority::ApprovalRequest, String> {
        if authenticated_user_id.trim().is_empty() {
            return Err("A logged-in user must decide the Task approval".into());
        }
        if !matches!(
            decision,
            super::device_command_bus::ConversationApprovalDecision::AllowChat { .. }
        ) {
            let device = self.execution_device(owner, task_id).await?;
            let reply = self
                .dispatch(
                    &device,
                    owner,
                    ChatTaskAction::ApprovalDecide {
                        task_id: task_id.into(),
                        attempt_no,
                        approval_id: approval_id.into(),
                        decision,
                        authenticated_user_id: authenticated_user_id.into(),
                    },
                )
                .await?;
            return response_value(reply).and_then(|value| {
                serde_json::from_value(value).map_err(|error| error.to_string())
            });
        }
        let scope = ChatStore::resolve(&self.root, owner.chat_address.clone());
        let placement = super::conversation_authority_target::load_chat_authority_placement_target(
            scope.root(),
            owner.chat_id(),
        )
        .await?;
        let reply = self
            .dispatch(
                &placement.device_id,
                owner,
                ChatTaskAction::ApprovalDecideAtAuthority {
                    task_id: task_id.into(),
                    attempt_no,
                    approval_id: approval_id.into(),
                    decision,
                    authenticated_user_id: authenticated_user_id.into(),
                    authority_epoch: placement.authority_epoch,
                },
            )
            .await?;
        response_value(reply)
            .and_then(|value| serde_json::from_value(value).map_err(|e| e.to_string()))
    }

    pub async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &ToolContext,
    ) -> Result<Option<ToolResult>, String> {
        self.execute_tool_request(name, arguments, context).await
    }

    pub async fn register_foreground(
        &self,
        context: &ChatCommandContext,
        preparation: &ChatCommandPreparation,
    ) -> Result<(), String> {
        let reference = ChatTaskRef {
            not_accepted: false,
            task_id: context.execution_id.clone(),
            owner: TaskOwner::from_address(context.call.address.clone()),
            execution: ChatTaskExecution {
                device_id: preparation.device_id.clone(),
                public_cwd: preparation.public_cwd.clone(),
                extra: Default::default(),
            },
            origin: ChatTaskOrigin {
                turn_id: Some(context.call.turn_id.clone()),
                tool_call_id: Some(context.call.tool_call_id.clone()),
                agent_id: context.call.agent_id.clone(),
                extra: Default::default(),
            },
            last_target_evidence: None,
            extra: Default::default(),
        };
        self.remember_reference(&reference).await
    }

    pub async fn remember_task(&self, record: &TaskRecord) -> Result<(), String> {
        let device = record
            .attempts
            .iter()
            .find(|attempt| attempt.attempt_no == record.attempt_no)
            .ok_or("Task has no current Attempt")?
            .device
            .device_id
            .as_deref()
            .ok_or("Task has no execution Device")?;
        if device == self.local_device_id {
            self.capture_and_publish(record).await.map(|_| ())
        } else {
            match self
                .dispatch(
                    device,
                    &record.spec.owner,
                    ChatTaskAction::Get {
                        task_id: record.task_id.clone(),
                    },
                )
                .await?
            {
                ChatTaskResponse::Record {
                    reference: Some(reference),
                    ..
                } => self.remember_reference(&reference).await,
                _ => Err("The accepting Device has not published its Task reference".into()),
            }
        }
    }

    pub async fn remember_reference(&self, reference: &ChatTaskRef) -> Result<(), String> {
        let scope = ChatStore::resolve(&self.root, reference.owner.chat_address.clone());
        let changed = reference
            .register(
                &scope
                    .root()
                    .join("sessions")
                    .join(reference.owner.chat_id()),
                reference.owner.chat_id(),
            )
            .map_err(|e| e.to_string())?;
        if changed {
            self.publish_reference(reference).await?;
        }
        Ok(())
    }

    /// Resolve a pending reference only after the one acceptance request is
    /// known not to have created a Task. Transport loss is never such evidence.
    pub async fn mark_not_accepted(&self, owner: &TaskOwner, task_id: &str) -> Result<(), String> {
        let scope = ChatStore::resolve(&self.root, owner.chat_address.clone());
        let reference = ChatTaskRef::mark_not_accepted(
            &scope.root().join("sessions").join(owner.chat_id()),
            owner.chat_id(),
            task_id,
        )
        .map_err(|e| e.to_string())?;
        self.publish_reference(&reference).await
    }

    async fn capture_and_publish(&self, record: &TaskRecord) -> Result<ChatTaskRef, String> {
        let scope = ChatStore::resolve(&self.root, record.spec.owner.chat_address.clone());
        let previous = ChatTaskRef::load(
            &scope
                .root()
                .join("sessions")
                .join(record.spec.owner.chat_id()),
            record.spec.owner.chat_id(),
            &record.task_id,
        )
        .map_err(|e| e.to_string())?;
        let reference = capture_task_reference(&self.root, &self.local_device_id, record)?;
        if previous.as_ref() != Some(&reference) {
            self.publish_reference(&reference).await?;
        }
        Ok(reference)
    }

    async fn publish_reference(&self, reference: &ChatTaskRef) -> Result<(), String> {
        let scope = ChatStore::resolve(&self.root, reference.owner.chat_address.clone());
        if let Some(runtime) = self.publication.get() {
            let result = crate::sync_publication::publish_sync_fact_with_runtime(
                runtime,
                crate::sync_publication::SyncPublicationFact::StableConversationState {
                    scope_root: scope.root().to_path_buf(),
                    address: reference.owner.chat_address.clone(),
                },
            )
            .await;
            if !result.errors.is_empty() {
                tracing::warn!(errors = ?result.errors, "Task reference synchronization remains pending");
            }
        }
        Ok(())
    }

    fn references(&self, owner: &TaskOwner) -> Result<Vec<ChatTaskRef>, String> {
        let scope = ChatStore::resolve(&self.root, owner.chat_address.clone());
        ChatTaskRef::list(
            &scope.root().join("sessions").join(owner.chat_id()),
            owner.chat_id(),
        )
        .map_err(|e| e.to_string())
    }

    pub async fn execution_device(
        &self,
        owner: &TaskOwner,
        task_id: &str,
    ) -> Result<String, String> {
        if let Some(reference) = self
            .references(owner)?
            .into_iter()
            .find(|r| r.task_id == task_id)
        {
            return Ok(reference.execution.device_id);
        }
        let record = self
            .runtime
            .get(owner, task_id)
            .await
            .map_err(|e| e.to_string())?;
        if let Err(error) = self.remember_task(&record).await {
            tracing::warn!(%error, %task_id, "Local Task remains available while its reference is pending");
        }
        Ok(self.local_device_id.clone())
    }

    pub async fn route_action(
        &self,
        owner: &TaskOwner,
        task_id: &str,
        action: ChatTaskAction,
    ) -> Result<ChatTaskResponse, String> {
        let device = match self
            .references(owner)?
            .into_iter()
            .find(|r| r.task_id == task_id)
        {
            Some(reference) => reference.execution.device_id,
            None => match self.runtime.get(owner, task_id).await {
                Ok(_) => self.local_device_id.clone(),
                Err(error) => return Ok(target::task_error(error)),
            },
        };
        let response = self.dispatch(&device, owner, action).await?;
        if let ChatTaskResponse::Record {
            reference: Some(reference),
            ..
        } = &response
        {
            if let Err(error) = self.remember_reference(reference).await {
                tracing::warn!(%error, "Task operation completed but its reference receipt could not be stored");
            }
        }
        Ok(response)
    }

    pub async fn list_records(
        &self,
        owner: &TaskOwner,
        filter: rua_task::TaskListFilter,
    ) -> Result<Vec<Value>, String> {
        let mut records = BTreeMap::new();
        let mut offset = 0;
        loop {
            let page = self
                .runtime
                .list(
                    owner,
                    rua_task::TaskListFilter {
                        offset,
                        limit: Some(100),
                        state: None,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            let count = page.len();
            for record in page {
                let mut value = serde_json::to_value(&record).map_err(|e| e.to_string())?;
                value["availability"] = "available".into();
                records.insert(record.task_id.clone(), value);
            }
            if count < 100 {
                break;
            }
            offset += count as u64;
        }
        // Known execution Devices come from durable Chat references, so an
        // offline Device does not disappear from listing or unfinished work.
        let references = self
            .references(owner)?
            .into_iter()
            .filter(|r| !r.not_accepted || r.last_target_evidence.is_some())
            .filter(|r| !records.contains_key(&r.task_id))
            .collect::<Vec<_>>();
        let queries = references.into_iter().map(|reference| async move {
            let result = self
                .dispatch(
                    &reference.execution.device_id,
                    owner,
                    ChatTaskAction::Get {
                        task_id: reference.task_id.clone(),
                    },
                )
                .await;
            (reference, result)
        });
        use futures::StreamExt;
        let mut queries = futures::stream::iter(queries).buffer_unordered(8);
        while let Some((reference, result)) = queries.next().await {
            let mut value = match result {
                Ok(ChatTaskResponse::Record {
                    record,
                    reference: target_reference,
                }) => {
                    if let Some(target_reference) = target_reference {
                        if let Err(error) = self.remember_reference(&target_reference).await {
                            tracing::warn!(%error, "Task reference receipt could not be stored");
                        }
                    }
                    let mut value = serde_json::to_value(record).map_err(|e| e.to_string())?;
                    value["availability"] = "available".into();
                    value
                }
                _ => {
                    let mut value = match &reference.last_target_evidence {
                        Some(evidence) => {
                            serde_json::to_value(&evidence.record).map_err(|e| e.to_string())?
                        }
                        None => {
                            serde_json::json!({"task_id":reference.task_id,"execution":reference.execution})
                        }
                    };
                    value["availability"] = "unknown".into();
                    value
                }
            };
            value["execution"] =
                serde_json::to_value(&reference.execution).map_err(|e| e.to_string())?;
            records.insert(reference.task_id, value);
        }
        let wanted = filter
            .state
            .map(|state| serde_json::to_value(state).unwrap_or_default());
        let mut records = records
            .into_values()
            .filter(|value| {
                wanted
                    .as_ref()
                    .is_none_or(|state| value.get("state") == Some(state))
            })
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            right["created_at"]
                .as_str()
                .cmp(&left["created_at"].as_str())
                .then_with(|| left["task_id"].as_str().cmp(&right["task_id"].as_str()))
        });
        Ok(records
            .into_iter()
            .skip(filter.offset as usize)
            .take(filter.limit.unwrap_or(100).min(201))
            .collect())
    }

    async fn dispatch(
        &self,
        device: &str,
        owner: &TaskOwner,
        action: ChatTaskAction,
    ) -> Result<ChatTaskResponse, String> {
        let wait = match &action {
            ChatTaskAction::Tool {
                tool_name,
                arguments,
                ..
            } if tool_name == "task_read" => arguments
                .get("wait_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(30),
            _ => 0,
        };
        let command = ChatTaskCommand {
            source_device_id: self.local_device_id.clone(),
            target_device_id: device.into(),
            owner: owner.clone(),
            action,
            extra: Default::default(),
        };
        if device == self.local_device_id {
            return self.handle(command).await;
        }
        let manager = super::device_session_access::current_session_manager(&self.network).await?;
        super::device_command_bus::deliver_chat_task_via_session(
            &manager,
            &command,
            std::time::Duration::from_secs(15 + wait),
        )
        .await
    }
}

fn response_value(response: ChatTaskResponse) -> Result<Value, String> {
    match response {
        ChatTaskResponse::Value { value } => Ok(value),
        ChatTaskResponse::Rejected { reason, detail } => Err(format!("{reason}: {detail}")),
        _ => Err("Unexpected Task response".into()),
    }
}

/// Only the execution Device increments evidence revisions. An originating
/// Device registers the received reference; it never invents target versions.
pub fn capture_task_reference(
    root: &Path,
    local_device_id: &str,
    record: &TaskRecord,
) -> Result<ChatTaskRef, String> {
    let current = record
        .attempts
        .iter()
        .find(|a| a.attempt_no == record.attempt_no)
        .ok_or("Task has no current Attempt")?;
    if current.device.device_id.as_deref() != Some(local_device_id) {
        return Err("Task evidence can only be captured on its execution Device".into());
    }
    let scope = ChatStore::resolve(root, record.spec.owner.chat_address.clone());
    let chat_dir = scope
        .root()
        .join("sessions")
        .join(record.spec.owner.chat_id());
    if ChatTaskRef::load(&chat_dir, record.spec.owner.chat_id(), &record.task_id)
        .map_err(|e| e.to_string())?
        .is_none()
    {
        let first = record
            .attempts
            .iter()
            .min_by_key(|a| a.attempt_no)
            .ok_or("Task has no Attempt")?;
        ChatTaskRef {
            not_accepted: false,
            task_id: record.task_id.clone(),
            owner: record.spec.owner.clone(),
            execution: ChatTaskExecution {
                device_id: local_device_id.into(),
                public_cwd: current
                    .device
                    .public_cwd
                    .clone()
                    .ok_or("Task has no resolved cwd")?,
                extra: Default::default(),
            },
            origin: ChatTaskOrigin {
                turn_id: record.spec.origin.turn_id.clone(),
                tool_call_id: record.spec.origin.tool_call_id.clone(),
                agent_id: first.authorization_agent_id.clone(),
                extra: Default::default(),
            },
            last_target_evidence: None,
            extra: Default::default(),
        }
        .register(&chat_dir, record.spec.owner.chat_id())
        .map_err(|e| e.to_string())?;
    }
    ChatTaskRef::capture_target_evidence(
        &chat_dir,
        record.spec.owner.chat_id(),
        &record.task_id,
        local_device_id,
        record.clone(),
    )
    .map_err(|e| e.to_string())
}
