use std::collections::{BTreeSet, HashMap};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures::FutureExt;
use rua_acp::client::AcpClientAdapter;
use rua_channel::core::console::ConsoleChatRequest;
use rua_contract::agent::AgentInputMessage;
use rua_core::{ChatAddress, ChatScopeStore, ProjectId};

use crate::services::chat_metadata_store::{AcpBindingMetadata, ChatMetadataRecord};
use crate::services::console_chat::{
    drive_conversation_producer, persist_conversation_runtime_failure_to_session, resolve_chat_id,
    truncate_chat_name, ConversationExecutionContext, SessionSyncEmitter, CONSOLE_CHANNEL_ID,
};
use crate::services::conversation_authority::{
    ConversationActiveTurn, ConversationActiveTurnStart, ConversationAuthority,
    ConversationDriverKind, ConversationInputAcceptanceMode, ConversationInputEntrySnapshot,
    ConversationInputPayload, ConversationSendNowOutcome, ConversationSubmissionAcceptance,
    ConversationSubmissionPersistCallback, ACCEPTANCE_MODE_KEY, INPUT_ID_KEY, SUBMISSION_ID_KEY,
};
use crate::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement, CHAT_MODEL_META_KEY,
};
use crate::services::conversation_authority_router::{
    verify_and_observe_authority_redirect, ConversationAuthorityRouter, ConversationSubmitRoute,
    ConversationSubmitRouteRequest,
};
use crate::services::device_command_bus::{
    acp_runner_infos_from_service, AcpChatDeviceAction, AcpChatDeviceCommand,
    AcpChatDeviceResponse, AcpChatDeviceSessionEntry, AcpChatDeviceStatus,
    ConversationApprovalDecision, ConversationAuthorityRedirect, ConversationControlDeviceAction,
    ConversationControlDeviceCommand, ConversationControlDeviceResponse,
    DeviceCommandDeliveryError, ProjectResourcePolicyView, RemoteConversationInputAck,
    RemoteConversationInputMediaRequirement, RemoteConversationInputRequest,
};
use crate::services::device_context::current_sync_device_id;
use crate::services::project_lifecycle::{
    ProjectArchiveResult, ProjectLifecycleImpact, ProjectLifecyclePreview,
    ProjectPermanentDeleteResult, ProjectReopenResult,
};
use crate::services::remote_conversation_input::{
    RemoteConversationInputDelivery, RemoteConversationInputDeliveryOutcome,
    RemoteConversationInputParams,
};
use crate::streaming::{ActiveStreamRegistry, Priority, StreamKey};

mod configuration_update;

use configuration_update::ChatConfigurationUpdate;

const LOCAL_STOP_CONFIRM_TIMEOUT: Duration = Duration::from_secs(8);

pub(crate) const TASK_WATCH_NOTIFICATION_META_KEY: &str = "task_watch_notification";
pub(crate) const RUNTIME_OBSERVATION_META_KEY: &str = "runtime_observation";
pub(crate) const RUNTIME_OBSERVATION_RECEIPT_META_KEY: &str = "runtime_observation_receipt_id";

struct TaskWatchWakeReceipt {
    address: ChatAddress,
    sender: tokio::sync::oneshot::Sender<()>,
}

#[derive(Clone, Default)]
pub(crate) struct TaskWatchWakeReceipts {
    inner: Arc<StdMutex<HashMap<String, TaskWatchWakeReceipt>>>,
}

struct TaskWatchWakeRegistration {
    notification_id: String,
    receipts: TaskWatchWakeReceipts,
    receiver: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl TaskWatchWakeReceipts {
    fn register(
        &self,
        notification_id: String,
        address: ChatAddress,
    ) -> Result<TaskWatchWakeRegistration, String> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner.contains_key(&notification_id) {
            return Err(format!(
                "Task Watch wake '{notification_id}' is already registered"
            ));
        }
        inner.insert(
            notification_id.clone(),
            TaskWatchWakeReceipt { address, sender },
        );
        Ok(TaskWatchWakeRegistration {
            notification_id,
            receipts: self.clone(),
            receiver: Some(receiver),
        })
    }

    pub(crate) fn complete(&self, notification_id: &str) {
        let receipt = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(notification_id);
        if let Some(receipt) = receipt {
            let _ = receipt.sender.send(());
        }
    }

    pub(crate) fn complete_all_for_address(&self, address: &ChatAddress) {
        let receipts = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let ids = inner
                .iter()
                .filter(|(_, receipt)| receipt.address == *address)
                .map(|(notification_id, _)| notification_id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|notification_id| inner.remove(&notification_id))
                .collect::<Vec<_>>()
        };
        for receipt in receipts {
            let _ = receipt.sender.send(());
        }
    }
}

impl TaskWatchWakeRegistration {
    async fn wait(mut self) -> Result<(), String> {
        self.receiver
            .take()
            .expect("Task Watch wake registration receiver")
            .await
            .map_err(|_| "Task Watch wake acknowledgement was dropped".to_string())
    }
}

impl Drop for TaskWatchWakeRegistration {
    fn drop(&mut self) {
        self.receipts
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.notification_id);
    }
}

fn project_chat_mutation_error(error: rua_core::RuaError) -> DeviceCommandDeliveryError {
    let detail = error.to_string();
    let reason = match error {
        rua_core::RuaError::NotFound { .. } => "project_chat_not_found",
        rua_core::RuaError::Validation { .. } if detail.contains("ACP mode") => {
            "project_chat_branch_conflict"
        }
        rua_core::RuaError::Validation { .. } => "invalid_project_chat_branch_point",
        _ => "project_chat_branch_failed",
    };
    DeviceCommandDeliveryError::Rejected {
        reason: reason.to_string(),
        detail,
    }
}

#[derive(Clone)]
pub struct ConversationInputIntake {
    chat_service: Arc<crate::services::usage::UsageRecordingChatService>,
    streams: Arc<ActiveStreamRegistry>,
    registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
    agent_service: Option<Arc<crate::services::agent_manager::AgentManager>>,
    provider_manager: Option<Arc<rua_provider::manager::ProviderManager>>,
    provider_cells: Option<Arc<rua_provider::ProviderCells>>,
    session_sync: SessionSyncEmitter,
    sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    acp_chat_registry: Option<Arc<crate::services::acp_chat_registry::AcpChatRegistry>>,
    active_stream_broadcast_hub:
        Option<Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>>,
    remote_input_delivery: RemoteConversationInputDelivery,
    conversation_authority: Option<Arc<ConversationAuthority>>,
    data_dir: Option<PathBuf>,
    project_catalog: Option<Arc<rua_project::ProjectCatalog>>,
    project_bindings: Option<Arc<rua_project::ProjectResourceBindingStore>>,
    security: Option<Arc<rua_security::SecurityModule>>,
    approval_authority: Option<Arc<rua_security::ApprovalAuthority>>,
    acp_permission_relay: Option<Arc<rua_acp::permissions::AcpPermissionRelay>>,
    task_runtime: Option<Arc<rua_task::TaskRuntime>>,
    task_notifications:
        Option<std::sync::Weak<crate::services::task_notification::ServerTaskNotificationSink>>,
    product_hooks: Option<Arc<crate::services::product_hooks::ProductHookService>>,
    candidate_completion_runtime:
        Option<Arc<crate::services::candidate_completion::CandidateCompletionRuntime>>,
    task_watch_wake_receipts: TaskWatchWakeReceipts,
    collaboration: Option<
        Arc<
            std::sync::OnceLock<
                std::sync::Weak<crate::services::collaboration::CollaborationService>,
            >,
        >,
    >,
}

#[derive(Debug, Clone)]
pub struct AppliedAgentRunningConfig {
    pub running: serde_json::Value,
    pub agent: serde_json::Value,
}

#[derive(Debug, Clone)]
pub(crate) struct ConversationControlTarget {
    pub address: ChatAddress,
    pub execution_agent_id: String,
    pub scope_root: PathBuf,
    pub working_dir: PathBuf,
    pub placement: ConversationAuthorityPlacement,
}

#[derive(Debug, Clone)]
pub(crate) struct InitialConversationControlTarget {
    pub scope_root: PathBuf,
    pub project_agent_state: Option<crate::services::project_chat::ProjectChatAgentState>,
}

struct ProjectConversationScopeTarget {
    scope_root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationInputDeliveryTransport {
    LocalAuthority,
    RemoteAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationInputDeliveryOutcome {
    Delivered {
        chat_id: String,
        transport: ConversationInputDeliveryTransport,
        accepted: bool,
        queued_to_active_authority: bool,
        input_id: Option<String>,
        submission_id: Option<String>,
        detail: Option<String>,
        device_delivery_id: Option<String>,
        target_device_id: Option<String>,
        device_delivery_sequence: Option<u64>,
        accepted_at: Option<String>,
        control: Option<Box<crate::services::conversation_authority::ConversationControlView>>,
    },
    Rejected {
        detail: String,
        target_device_id: Option<String>,
        failure_kind: Option<String>,
    },
}

#[derive(Clone, Copy)]
struct AuthorityRouteTarget<'a> {
    device_id: &'a str,
    epoch: ConversationAuthorityEpoch,
}

struct FirstAgentSubmission {
    placement: ConversationAuthorityPlacement,
    initial_model: Option<serde_json::Value>,
    configuration: crate::services::product_hooks::ChatHookConfiguration,
}

pub(crate) const INITIAL_ACP_KEY: &str = "initial_acp";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct InitialAcpSelection {
    pub(crate) runner: String,
    pub(crate) cwd: Option<String>,
    pub(crate) resume_session_id: Option<String>,
}

impl ConversationInputDeliveryOutcome {
    fn local_started(
        chat_id: String,
        input_id: Option<String>,
        submission_id: Option<String>,
    ) -> Self {
        Self::Delivered {
            chat_id,
            transport: ConversationInputDeliveryTransport::LocalAuthority,
            accepted: true,
            queued_to_active_authority: false,
            input_id,
            submission_id,
            detail: None,
            device_delivery_id: None,
            target_device_id: None,
            device_delivery_sequence: None,
            accepted_at: None,
            control: None,
        }
    }

    fn local_queued(
        chat_id: String,
        input_id: Option<String>,
        submission_id: Option<String>,
    ) -> Self {
        Self::Delivered {
            chat_id,
            transport: ConversationInputDeliveryTransport::LocalAuthority,
            accepted: true,
            queued_to_active_authority: true,
            input_id,
            submission_id,
            detail: None,
            device_delivery_id: None,
            target_device_id: None,
            device_delivery_sequence: None,
            accepted_at: None,
            control: None,
        }
    }

    fn remote(chat_id: String, outcome: RemoteConversationInputDeliveryOutcome) -> Self {
        if outcome.accepted {
            Self::Delivered {
                chat_id,
                transport: ConversationInputDeliveryTransport::RemoteAuthority,
                accepted: true,
                queued_to_active_authority: matches!(
                    outcome.mode,
                    Some(ConversationInputAcceptanceMode::Queued)
                ),
                input_id: outcome.input_id,
                submission_id: outcome.submission_id,
                detail: outcome.detail,
                device_delivery_id: outcome.device_delivery_id,
                target_device_id: outcome.target_device_id,
                device_delivery_sequence: outcome.device_delivery_sequence,
                accepted_at: outcome.accepted_at,
                control: outcome.control,
            }
        } else {
            Self::Rejected {
                detail: outcome
                    .detail
                    .unwrap_or_else(|| "remote authority delivery failed".to_string()),
                target_device_id: outcome.target_device_id,
                failure_kind: outcome.failure_kind,
            }
        }
    }

    fn with_control(
        mut self,
        control: Option<Box<crate::services::conversation_authority::ConversationControlView>>,
    ) -> Self {
        if let Self::Delivered {
            control: outcome_control,
            ..
        } = &mut self
        {
            *outcome_control = control;
        }
        self
    }
}

impl ConversationInputIntake {
    pub(crate) fn project_work_admission(
        &self,
        address: &ChatAddress,
    ) -> Result<Option<rua_project::ProjectAdmissionGuard<'_>>, String> {
        match address.project_id() {
            Some(project_id) => self
                .project_catalog
                .as_ref()
                .ok_or("Project catalog unavailable")?
                .begin_admission(project_id)
                .map(Some)
                .map_err(|error| error.to_string()),
            None => Ok(None),
        }
    }

    pub(crate) async fn resolve_collaboration_working_dir(
        &self,
        address: &ChatAddress,
        local_device_id: &str,
    ) -> Result<PathBuf, String> {
        match address.scope() {
            rua_core::ChatScope::Agent { agent_id, .. } => self
                .registry
                .get_or_create(agent_id.as_str())
                .await
                .map(|runtime| runtime.workspace_dir())
                .map_err(|error| error.to_string()),
            rua_core::ChatScope::Project { project_id, .. } => {
                let catalog = self
                    .project_catalog
                    .as_ref()
                    .ok_or("Project Catalog unavailable")?;
                let project = catalog
                    .get(project_id)
                    .map_err(|error| error.to_string())?
                    .ok_or("Project not found")?;
                if project.resource_device_id != local_device_id {
                    return Err("Project collaboration is fixed to another resource Device".into());
                }
                let binding = self
                    .project_bindings
                    .as_ref()
                    .ok_or("Project Resource Binding Store unavailable")?
                    .get(project_id)
                    .map_err(|error| error.to_string())?
                    .ok_or("Project Resource Binding missing")?;
                if binding.resource_device_id != local_device_id {
                    return Err("Project Resource Binding targets another Device".into());
                }
                let target = self
                    .resolve_project_conversation_scope_target(address, local_device_id)
                    .await?;
                let header = rua_session::WorkspaceSessionStore::new(&target.scope_root)
                    .load_chat_header(address.chat_id().as_str())
                    .await
                    .map_err(|error| error.to_string())?;
                crate::services::project_chat::working_directory(&binding, header.as_ref(), None)
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chat_service: Arc<crate::services::usage::UsageRecordingChatService>,
        streams: Arc<ActiveStreamRegistry>,
        registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
        session_sync: SessionSyncEmitter,
        sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
        acp_chat_registry: Option<Arc<crate::services::acp_chat_registry::AcpChatRegistry>>,
        active_stream_broadcast_hub: Option<
            Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>,
        >,
        remote_input_delivery: RemoteConversationInputDelivery,
    ) -> Self {
        Self {
            chat_service,
            streams,
            registry,
            agent_service: None,
            provider_manager: None,
            provider_cells: None,
            session_sync,
            sync_device,
            acp_chat_registry,
            active_stream_broadcast_hub,
            remote_input_delivery,
            conversation_authority: None,
            data_dir: None,
            project_catalog: None,
            project_bindings: None,
            security: None,
            approval_authority: None,
            acp_permission_relay: None,
            task_runtime: None,
            task_notifications: None,
            product_hooks: None,
            candidate_completion_runtime: None,
            task_watch_wake_receipts: TaskWatchWakeReceipts::default(),
            collaboration: None,
        }
    }

    pub fn with_agent_service(
        mut self,
        agent_service: Arc<crate::services::agent_manager::AgentManager>,
    ) -> Self {
        self.agent_service = Some(agent_service);
        self
    }

    pub fn with_provider_manager(
        mut self,
        provider_manager: Arc<rua_provider::manager::ProviderManager>,
    ) -> Self {
        self.provider_manager = Some(provider_manager);
        self
    }

    pub fn with_provider_cells(mut self, provider_cells: Arc<rua_provider::ProviderCells>) -> Self {
        self.provider_cells = Some(provider_cells);
        self
    }

    pub fn with_conversation_authority(mut self, authority: Arc<ConversationAuthority>) -> Self {
        self.conversation_authority = Some(authority);
        self
    }

    pub(crate) fn device_network(
        &self,
    ) -> &Arc<crate::services::device_session_access::DeviceNetworkAccess> {
        self.remote_input_delivery.device_network()
    }

    pub fn with_product_hooks(
        mut self,
        product_hooks: Arc<crate::services::product_hooks::ProductHookService>,
    ) -> Self {
        self.product_hooks = Some(product_hooks);
        self
    }

    pub fn with_candidate_completion_runtime(
        mut self,
        runtime: Arc<crate::services::candidate_completion::CandidateCompletionRuntime>,
    ) -> Self {
        self.candidate_completion_runtime = Some(runtime);
        self
    }

    pub fn with_project_runtime(
        mut self,
        data_dir: PathBuf,
        catalog: Arc<rua_project::ProjectCatalog>,
        bindings: Arc<rua_project::ProjectResourceBindingStore>,
    ) -> Self {
        self.data_dir = Some(data_dir);
        self.project_catalog = Some(catalog);
        self.project_bindings = Some(bindings);
        self
    }

    pub fn with_approval_authority(
        mut self,
        approval_authority: Arc<rua_security::ApprovalAuthority>,
    ) -> Self {
        self.approval_authority = Some(approval_authority);
        self
    }

    pub fn with_security(mut self, security: Arc<rua_security::SecurityModule>) -> Self {
        self.security = Some(security);
        self
    }

    pub fn with_acp_permission_relay(
        mut self,
        relay: Arc<rua_acp::permissions::AcpPermissionRelay>,
    ) -> Self {
        self.acp_permission_relay = Some(relay);
        self
    }

    pub fn with_task_runtime(mut self, task_runtime: Arc<rua_task::TaskRuntime>) -> Self {
        self.task_runtime = Some(task_runtime);
        self
    }

    pub fn with_task_notifications(
        mut self,
        sink: std::sync::Weak<crate::services::task_notification::ServerTaskNotificationSink>,
    ) -> Self {
        self.task_notifications = Some(sink);
        self
    }

    pub async fn resume_task_results(&self, address: Option<&ChatAddress>) -> Result<(), String> {
        if let Some(sink) = self
            .task_notifications
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
        {
            let owner = address.map(|address| rua_task::TaskOwner::from_address(address.clone()));
            sink.resume_pending(owner.as_ref())
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub(crate) async fn publish_background_work(&self, address: &ChatAddress) {
        let (Some(root), Some(authority)) =
            (self.data_dir.as_ref(), self.conversation_authority.as_ref())
        else {
            return;
        };
        if let Some(agent_id) = address.agent_id() {
            if authority.is_draining(agent_id.as_str()).await {
                return;
            }
        }
        let scope = ChatScopeStore::resolve(root, address.clone());
        if let Ok(Some(header)) = rua_session::WorkspaceSessionStore::new(scope.root())
            .load_chat_header(address.chat_id().as_str())
            .await
        {
            if rua_session::WorkspaceSessionStore::new(scope.root())
                .load_collaboration_wake_state(address.chat_id().as_str())
                .await
                .map_or(true, |state| state.suppressed)
            {
                return;
            }
            authority
                .publish_control_snapshot(
                    address,
                    "background_work_changed",
                    &serde_json::json!(header.extra),
                )
                .await;
        }
    }

    pub fn with_acp_chat_registry(
        mut self,
        registry: Arc<crate::services::acp_chat_registry::AcpChatRegistry>,
    ) -> Self {
        self.acp_chat_registry = Some(registry);
        self
    }

    pub fn with_active_stream_broadcast_hub(
        mut self,
        hub: Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>,
    ) -> Self {
        self.active_stream_broadcast_hub = Some(hub);
        self
    }

    pub fn with_collaboration_binding(
        mut self,
        binding: Arc<
            std::sync::OnceLock<
                std::sync::Weak<crate::services::collaboration::CollaborationService>,
            >,
        >,
    ) -> Self {
        self.collaboration = Some(binding);
        self
    }

    pub async fn deliver_task_watch_output(
        &self,
        notification: rua_task::TaskWatchOutputNotification,
    ) -> Result<(), String> {
        let local_device_id = current_sync_device_id(&self.sync_device)
            .ok_or_else(|| "local Device identity is unavailable".to_string())?;
        let address = notification.owner.chat_address.clone();
        let (execution_agent_id, scope_root, working_dir) = match address.scope() {
            rua_core::ChatScope::Agent { agent_id, .. } => {
                let runtime = self
                    .registry
                    .get_or_create(agent_id.as_str())
                    .await
                    .map_err(|error| error.to_string())?;
                let root = runtime.workspace_dir();
                (agent_id.to_string(), root.clone(), root)
            }
            rua_core::ChatScope::Project { project_id, .. } => {
                let target = self
                    .resolve_project_conversation_scope_target(&address, &local_device_id)
                    .await?;
                let header = rua_session::WorkspaceSessionStore::new(&target.scope_root)
                    .load_chat_header(address.chat_id().as_str())
                    .await
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("Project Chat not found: {}", address.chat_id()))?;
                let execution_agent_id =
                    crate::services::project_chat::selection_from_header(&header)?.agent_id;
                let bindings = self
                    .project_bindings
                    .as_ref()
                    .ok_or_else(|| "Project Resource Binding Store is unavailable".to_string())?;
                let binding = bindings
                    .get(project_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("Project Resource Binding not found: {project_id}"))?;
                self.registry
                    .get_or_create(&execution_agent_id)
                    .await
                    .map_err(|error| error.to_string())?;
                (
                    execution_agent_id,
                    target.scope_root,
                    crate::services::project_chat::working_directory(
                        &binding,
                        Some(&header),
                        None,
                    )?,
                )
            }
        };
        if notification.projection == rua_task::TaskLiveOutputProjection::ActivityOnly {
            let store = rua_session::WorkspaceSessionStore::new(&scope_root);
            let note = rua_session::RuntimeNote {
                runtime_note_id: notification.notification_id,
                created_at: chrono::Utc::now().to_rfc3339(),
                payload: rua_session::RuntimeNotePayload::TaskWatchOutput(
                    rua_session::TaskWatchOutputRuntimeNote {
                        task_id: notification.task_id,
                        attempt_no: notification.attempt_no,
                        projection: rua_session::TaskWatchOutputProjection::ActivityOnly,
                        records: notification
                            .records
                            .into_iter()
                            .map(|record| rua_session::TaskWatchOutputRecord {
                                source: record.source,
                                text: record.text,
                            })
                            .collect(),
                    },
                ),
            };
            let turns = store
                .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
                .await
                .map_err(|error| error.to_string())?;
            let target_turn_id = notification
                .origin_turn_id
                .filter(|turn_id| turns.iter().any(|turn| turn.turn_id == *turn_id))
                .or_else(|| turns.last().map(|turn| turn.turn_id.clone()));
            if let Some(turn_id) = target_turn_id {
                store
                    .append_runtime_note(address.chat_id().as_str(), &turn_id, note)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            return Ok(());
        }
        let mut extra = serde_json::Map::new();
        let runtime_note = rua_session::RuntimeNote {
            runtime_note_id: notification.notification_id.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            payload: rua_session::RuntimeNotePayload::TaskWatchOutput(
                rua_session::TaskWatchOutputRuntimeNote {
                    task_id: notification.task_id.clone(),
                    attempt_no: notification.attempt_no,
                    projection: match notification.projection {
                        rua_task::TaskLiveOutputProjection::ModelVisible => {
                            rua_session::TaskWatchOutputProjection::ModelVisible
                        }
                        rua_task::TaskLiveOutputProjection::ActivityOnly => {
                            rua_session::TaskWatchOutputProjection::ActivityOnly
                        }
                    },
                    records: notification
                        .records
                        .iter()
                        .map(|record| rua_session::TaskWatchOutputRecord {
                            source: record.source.clone(),
                            text: record.text.clone(),
                        })
                        .collect(),
                },
            ),
        };
        extra.insert(
            TASK_WATCH_NOTIFICATION_META_KEY.to_string(),
            serde_json::json!({
                "notification_id": notification.notification_id,
                "task_id": notification.task_id,
                "attempt_no": notification.attempt_no,
                "origin_turn_id": notification.origin_turn_id,
                "projection": notification.projection,
                "records": notification.records,
            }),
        );
        extra.insert(
            crate::services::background_work::BACKGROUND_RESULT_KEY.into(),
            serde_json::Value::Bool(true),
        );
        self.deliver_runtime_observation_resolved(
            ConversationExecutionContext {
                address,
                execution_agent_id,
                scope_root,
                working_dir,
            },
            local_device_id,
            runtime_note,
            extra,
        )
        .await
    }

    pub(crate) async fn deliver_task_result(
        &self,
        record: rua_task::TaskRecord,
        tasks: Arc<rua_task::TaskRuntime>,
    ) -> Result<(), String> {
        let note = rua_agent::task_runtime_notes::task_terminal_note(&record)
            .map_err(|e| e.to_string())?;
        let root = self
            .data_dir
            .as_ref()
            .ok_or("Conversation data directory unavailable")?;
        let scope = ChatScopeStore::resolve(root, record.spec.owner.chat_address.clone());
        let store = rua_session::WorkspaceSessionStore::new(scope.root());
        if store
            .load_collaboration_wake_state(record.spec.owner.chat_id())
            .await
            .map_err(|error| error.to_string())?
            .suppressed
        {
            let turns = store
                .read_turn_range(record.spec.owner.chat_id(), 1, usize::MAX)
                .await
                .map_err(|error| error.to_string())?;
            if let Some(turn_id) = record
                .spec
                .origin
                .turn_id
                .as_ref()
                .filter(|id| turns.iter().any(|turn| &turn.turn_id == *id))
            {
                store
                    .append_runtime_note(record.spec.owner.chat_id(), turn_id, note)
                    .await
                    .map_err(|error| error.to_string())?;
                tasks
                    .ack_terminal_delivery(&record.spec.owner, &record.task_id, record.attempt_no)
                    .await
                    .map_err(|error| error.to_string())?;
                return Ok(());
            }
            return Err(
                "Task origin Turn is unavailable; terminal result was retained".to_string(),
            );
        }
        let result = self
            .deliver_background_result(record.spec.owner.chat_address.clone(), note)
            .await;
        if let Err(error) = &result {
            tasks
                .fail_terminal_delivery(
                    &record.spec.owner,
                    &record.task_id,
                    record.attempt_no,
                    error.clone(),
                )
                .await
                .map_err(|e| e.to_string())?;
            self.publish_background_work(&record.spec.owner.chat_address)
                .await;
        }
        result?;
        tasks
            .ack_terminal_delivery(&record.spec.owner, &record.task_id, record.attempt_no)
            .await
            .map_err(|e| e.to_string())?;
        self.publish_background_work(&record.spec.owner.chat_address)
            .await;
        Ok(())
    }

    pub(crate) async fn deliver_background_result(
        &self,
        address: ChatAddress,
        note: rua_session::RuntimeNote,
    ) -> Result<(), String> {
        let local_device_id = current_sync_device_id(&self.sync_device)
            .ok_or_else(|| "local Device identity is unavailable".to_string())?;
        let root = self
            .data_dir
            .as_ref()
            .ok_or("Conversation data directory unavailable")?;
        let scope = ChatScopeStore::resolve(root, address.clone());
        if rua_session::WorkspaceSessionStore::new(scope.root())
            .load_collaboration_wake_state(address.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
            .suppressed
        {
            return Err(
                "Background result delivery is suppressed; result was retained".to_string(),
            );
        }
        if rua_session::WorkspaceSessionStore::new(scope.root())
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Err("Source Chat is unavailable; background result was retained".into());
        }
        let placement =
            crate::services::conversation_authority_target::load_chat_authority_placement_target(
                scope.root(),
                address.chat_id().as_str(),
            )
            .await?;
        if placement.device_id != local_device_id {
            let id = note.runtime_note_id.clone();
            let header = rua_session::WorkspaceSessionStore::new(scope.root())
                .load_chat_header(address.chat_id().as_str())
                .await
                .map_err(|e| e.to_string())?
                .ok_or("Source Chat unavailable")?;
            let agent_id = match address.agent_id() {
                Some(agent_id) => agent_id.to_string(),
                None => crate::services::project_chat::selection_from_header(&header)?.agent_id,
            };
            let response = Box::pin(crate::services::conversation_control_dispatcher::dispatch_conversation_control(self, crate::services::device_command_bus::ConversationControlDeviceCommand {
                agent_id,
                command_id: format!("background:{}", id), source_device_id: local_device_id,
                target_device_id: placement.device_id, address, authority_epoch: placement.authority_epoch,
                action: crate::services::device_command_bus::ConversationControlDeviceAction::DeliverBackgroundResult { note: serde_json::to_value(note).map_err(|e| e.to_string())? },
            })).await.map_err(|e| e.to_string())?;
            return match response {
                crate::services::device_command_bus::ConversationControlDeviceResponse::BackgroundResultDelivered { observation_id } if observation_id == id => Ok(()),
                _ => Err("Background delivery was not acknowledged by the current Authority".into()),
            };
        }
        let (execution_agent_id, scope_root, working_dir) = match address.scope() {
            rua_core::ChatScope::Agent { agent_id, .. } => {
                let data_dir = self
                    .data_dir
                    .as_ref()
                    .ok_or("Conversation data directory is unavailable")?;
                let root = ChatScopeStore::resolve(data_dir, address.clone())
                    .root()
                    .to_path_buf();
                if rua_session::WorkspaceSessionStore::new(&root)
                    .load_chat_header(address.chat_id().as_str())
                    .await
                    .map_err(|error| error.to_string())?
                    .is_none()
                {
                    return Err("Source Chat is unavailable; background result was retained".into());
                }
                self.registry
                    .get_or_create(agent_id.as_str())
                    .await
                    .map_err(|e| e.to_string())?;
                (agent_id.to_string(), root.clone(), root)
            }
            rua_core::ChatScope::Project { project_id, .. } => {
                let target = self
                    .resolve_project_conversation_scope_target(&address, &local_device_id)
                    .await?;
                let header = rua_session::WorkspaceSessionStore::new(&target.scope_root)
                    .load_chat_header(address.chat_id().as_str())
                    .await
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("Project Chat not found: {}", address.chat_id()))?;
                let execution_agent_id =
                    crate::services::project_chat::selection_from_header(&header)?.agent_id;
                let binding = self
                    .project_bindings
                    .as_ref()
                    .ok_or_else(|| "Project Resource Binding Store is unavailable".to_string())?
                    .get(project_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("Project Resource Binding not found: {project_id}"))?;
                self.registry
                    .get_or_create(&execution_agent_id)
                    .await
                    .map_err(|error| error.to_string())?;
                (
                    execution_agent_id,
                    target.scope_root,
                    crate::services::project_chat::working_directory(
                        &binding,
                        Some(&header),
                        None,
                    )?,
                )
            }
        };
        let mut extra = serde_json::Map::new();
        extra.insert(
            crate::services::background_work::BACKGROUND_RESULT_KEY.into(),
            serde_json::Value::Bool(true),
        );
        self.deliver_runtime_observation_resolved(
            ConversationExecutionContext {
                address: address.clone(),
                execution_agent_id,
                scope_root: scope_root.clone(),
                working_dir,
            },
            local_device_id,
            note,
            extra,
        )
        .await
    }

    async fn deliver_runtime_observation_resolved(
        &self,
        execution: ConversationExecutionContext,
        local_device_id: String,
        note: rua_session::RuntimeNote,
        mut extra: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        let ConversationExecutionContext {
            address,
            execution_agent_id,
            scope_root,
            working_dir,
        } = execution;
        let observation_id = note.runtime_note_id.clone();
        let wait_for_watch = matches!(
            note.payload,
            rua_session::RuntimeNotePayload::TaskWatchOutput(_)
        );
        let wake_receipt = if wait_for_watch {
            Some(
                self.task_watch_wake_receipts
                    .register(observation_id.clone(), address.clone())?,
            )
        } else {
            None
        };
        let placement =
            crate::services::conversation_authority_target::load_chat_authority_placement_target(
                &scope_root,
                address.chat_id().as_str(),
            )
            .await?;
        if placement.device_id != local_device_id {
            return Err("Runtime observation is not on the current Conversation Authority".into());
        }
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        let (driver, driver_detail) = self
            .conversation_driver_for(
                &execution_agent_id,
                address.chat_id().as_str(),
                Some(scope_root.as_path()),
            )
            .await;
        let key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
        extra.insert(
            SUBMISSION_ID_KEY.to_string(),
            serde_json::json!(observation_id),
        );
        extra.insert(
            RUNTIME_OBSERVATION_META_KEY.to_string(),
            serde_json::to_value(note).map_err(|error| error.to_string())?,
        );
        if wait_for_watch {
            extra.insert(
                RUNTIME_OBSERVATION_RECEIPT_META_KEY.to_string(),
                serde_json::json!(observation_id),
            );
        }
        if address.project_id().is_some() {
            let assignment = self.resolve_project_turn_agent(&execution_agent_id).await?;
            extra.insert(
                rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
                serde_json::to_value(assignment).map_err(|error| error.to_string())?,
            );
        }
        let pending = ConversationInputPayload {
            message: String::new(),
            model_override: None,
            input: None,
            messages: None,
            extra,
            attachments: Vec::new(),
            enqueued_at: std::time::Instant::now(),
        };
        match authority
            .accept_submission(
                self.streams.as_ref(),
                &key,
                ConversationActiveTurnStart {
                    address: address.clone(),
                    driver,
                    authority_device_id: placement.device_id,
                    authority_epoch: placement.authority_epoch,
                    driver_detail,
                },
                pending,
                Some(local_device_id),
                None,
            )
            .await?
        {
            ConversationSubmissionAcceptance::Started {
                producer_id,
                cancel_rx,
                ..
            } => {
                self.spawn_console_producer(
                    key,
                    producer_id,
                    cancel_rx,
                    ConversationExecutionContext {
                        address,
                        execution_agent_id,
                        scope_root,
                        working_dir,
                    },
                );
            }
            ConversationSubmissionAcceptance::Queued { .. } => {}
        }
        if let Some(receipt) = wake_receipt {
            receipt.wait().await
        } else {
            Ok(())
        }
    }

    async fn resolve_project_turn_agent(
        &self,
        agent_id: &str,
    ) -> Result<rua_chat::TurnAgentAssignment, String> {
        let service = self
            .agent_service
            .as_ref()
            .ok_or_else(|| "Agent configuration service is unavailable".to_string())?;
        let agent = service
            .get_agent(agent_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Agent not found: {agent_id}"))?;
        crate::services::console_chat::turn_agent_assignment_from_agent(&agent)
    }

    async fn resolve_agent_turn_assignment(
        &self,
        agent_id: &str,
    ) -> Result<rua_chat::TurnAgentAssignment, String> {
        let Some(service) = self.agent_service.as_ref() else {
            return Ok(runtime_turn_agent_assignment(agent_id));
        };
        let Some(agent) = service
            .get_agent(agent_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(runtime_turn_agent_assignment(agent_id));
        };
        crate::services::console_chat::turn_agent_assignment_from_agent(&agent)
    }

    async fn capture_chat_configuration(
        &self,
        assignment: rua_chat::TurnAgentAssignment,
        running_config: rua_config::ChatRunningConfig,
        project_root: Option<&Path>,
        selection: crate::services::chat_configuration_capture::ChatConfigurationSelection,
    ) -> Result<crate::services::product_hooks::ChatConfigurationMaterialization, String> {
        let runtime = self
            .registry
            .get_or_create(&assignment.agent_id)
            .await
            .map_err(|error| error.to_string())?;
        let security = self
            .security
            .as_ref()
            .ok_or("Security module unavailable")?;
        let agent = match self.agent_service.as_ref() {
            Some(service) => service
                .get_agent(&assignment.agent_id)
                .await
                .map_err(|error| error.to_string())?,
            None => None,
        };
        crate::services::chat_configuration_capture::capture_chat_configuration(
            &runtime,
            security,
            agent.as_ref().and_then(|agent| agent.tools.as_ref()),
            assignment,
            running_config,
            project_root,
            selection,
        )
        .await
    }

    async fn resolve_project_conversation_scope_target(
        &self,
        address: &ChatAddress,
        local_device_id: &str,
    ) -> Result<ProjectConversationScopeTarget, String> {
        let project_id = address
            .project_id()
            .ok_or_else(|| "Project Conversation scope requires a Project id".to_string())?;
        let data_dir = self
            .data_dir
            .as_ref()
            .ok_or_else(|| "Project Chat runtime is unavailable".to_string())?;
        let catalog = self
            .project_catalog
            .as_ref()
            .ok_or_else(|| "Project Catalog is unavailable".to_string())?;
        if catalog.lifecycle_transition_in_progress(project_id) {
            return Err("Project lifecycle transition is in progress".to_string());
        }
        let project = catalog
            .get(project_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Project not found: {project_id}"))?;
        if project.lifecycle != rua_project::ProjectLifecycle::Active {
            return Err("Archived Project cannot provide Chat activity".to_string());
        }
        if project.resource_device_id != local_device_id {
            return Err("Project Chat authority is fixed to its resource Device".to_string());
        }
        let bindings = self
            .project_bindings
            .as_ref()
            .ok_or_else(|| "Project Resource Binding Store is unavailable".to_string())?;
        let binding = bindings
            .get(project_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Project Resource Binding not found: {project_id}"))?;
        if binding.resource_device_id != project.resource_device_id {
            return Err("Project Resource Binding Device does not match the Project".to_string());
        }
        Ok(ProjectConversationScopeTarget {
            scope_root: ChatScopeStore::resolve(data_dir, address.clone())
                .root()
                .to_path_buf(),
        })
    }

    pub(crate) async fn resolve_initial_conversation_control_target(
        &self,
        address: &ChatAddress,
        local_device_id: &str,
    ) -> Result<InitialConversationControlTarget, String> {
        match address.scope() {
            rua_core::ChatScope::Agent { agent_id, .. } => {
                let runtime = self
                    .registry
                    .get_or_create(agent_id.as_str())
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(InitialConversationControlTarget {
                    scope_root: runtime.workspace_dir(),
                    project_agent_state: None,
                })
            }
            rua_core::ChatScope::Project { project_id, .. } => {
                let project_target = self
                    .resolve_project_conversation_scope_target(address, local_device_id)
                    .await?;
                let data_dir = self
                    .data_dir
                    .as_ref()
                    .ok_or_else(|| "Project Chat runtime is unavailable".to_string())?;
                let catalog = self
                    .project_catalog
                    .as_ref()
                    .ok_or_else(|| "Project Catalog is unavailable".to_string())?;
                let scope_root = project_target.scope_root;
                let header = rua_session::WorkspaceSessionStore::new(&scope_root)
                    .load_chat_header(address.chat_id().as_str())
                    .await
                    .map_err(|error| error.to_string())?;
                let Some(header) = header else {
                    if let Some(owner) = find_project_chat_owner_on_target(
                        data_dir,
                        catalog,
                        project_id,
                        address.chat_id().as_str(),
                    )
                    .await?
                    {
                        return Err(format!(
                            "Project Chat {} belongs to Project {owner}",
                            address.chat_id()
                        ));
                    }
                    return Err(format!("Project Chat not found: {}", address.chat_id()));
                };
                let execution_agent_id =
                    crate::services::project_chat::selection_from_header(&header)?.agent_id;
                self.registry
                    .get_or_create(&execution_agent_id)
                    .await
                    .map_err(|error| error.to_string())?;
                let project_agent_state = self
                    .project_chat_agent_state(project_id, address.chat_id().as_str())
                    .await?;
                Ok(InitialConversationControlTarget {
                    scope_root,
                    project_agent_state: Some(project_agent_state),
                })
            }
        }
    }

    pub(crate) async fn resolve_conversation_control_target(
        &self,
        command: &ConversationControlDeviceCommand,
    ) -> Result<ConversationControlTarget, DeviceCommandDeliveryError> {
        let reject = |reason: &str, detail: String| DeviceCommandDeliveryError::Rejected {
            reason: reason.to_string(),
            detail,
        };
        let chat_id = command.chat_id();
        if matches!(&command.action, ConversationControlDeviceAction::Discover)
            && command.address.project_id().is_none()
        {
            return Err(reject(
                "wrong_chat_scope",
                "Conversation control discovery is available only for Project Chat".to_string(),
            ));
        }
        let project_discovery = command.address.project_id().is_some()
            && matches!(&command.action, ConversationControlDeviceAction::Discover);
        let (execution_agent_id, scope_root, working_dir, project_resource_device_id) =
            match command.address.scope() {
                rua_core::ChatScope::Agent { agent_id, .. } => {
                    if agent_id.as_str() != command.agent_id {
                        return Err(reject(
                            "wrong_agent_scope",
                            "Conversation control Agent does not match its Chat Address"
                                .to_string(),
                        ));
                    }
                    let runtime = self
                        .registry
                        .get_or_create(&command.agent_id)
                        .await
                        .map_err(|error| reject("workspace_unavailable", error.to_string()))?;
                    let root = runtime.workspace_dir();
                    (command.agent_id.clone(), root.clone(), root, None)
                }
                rua_core::ChatScope::Project { project_id, .. } => {
                    let data_dir = self.data_dir.as_ref().ok_or_else(|| {
                        reject(
                            "project_runtime_unavailable",
                            "Project Chat runtime is unavailable".to_string(),
                        )
                    })?;
                    let catalog = self.project_catalog.as_ref().ok_or_else(|| {
                        reject(
                            "project_catalog_unavailable",
                            "Project Catalog is unavailable".to_string(),
                        )
                    })?;
                    if catalog.lifecycle_transition_in_progress(project_id) {
                        return Err(reject(
                            "project_lifecycle_transition",
                            "Project lifecycle transition is in progress".to_string(),
                        ));
                    }
                    let project = catalog
                        .get(project_id)
                        .map_err(|error| reject("project_store_failed", error.to_string()))?
                        .ok_or_else(|| {
                            reject(
                                "project_not_found",
                                format!("Project not found: {project_id}"),
                            )
                        })?;
                    if project.lifecycle != rua_project::ProjectLifecycle::Active {
                        return Err(reject(
                            "project_archived",
                            "Archived Project cannot accept Chat controls".to_string(),
                        ));
                    }
                    let bindings = self.project_bindings.as_ref().ok_or_else(|| {
                        reject(
                            "project_binding_unavailable",
                            "Project Resource Binding Store is unavailable".to_string(),
                        )
                    })?;
                    let binding = bindings
                        .get(project_id)
                        .map_err(|error| reject("project_binding_failed", error.to_string()))?
                        .ok_or_else(|| {
                            reject(
                                "project_binding_not_found",
                                format!("Project Resource Binding not found: {project_id}"),
                            )
                        })?;
                    if binding.resource_device_id != project.resource_device_id {
                        return Err(reject(
                            "project_binding_mismatch",
                            "Project Resource Binding Device does not match the Project"
                                .to_string(),
                        ));
                    }
                    if command.target_device_id != project.resource_device_id {
                        return Err(reject(
                            "project_authority_fixed",
                            "Project Chat authority is fixed to its resource Device".to_string(),
                        ));
                    }
                    let scope_root = ChatScopeStore::resolve(data_dir, command.address.clone())
                        .root()
                        .to_path_buf();
                    let header = rua_session::WorkspaceSessionStore::new(&scope_root)
                        .load_chat_header(chat_id)
                        .await
                        .map_err(|error| reject("project_chat_unavailable", error.to_string()))?
                        .ok_or_else(|| {
                            reject(
                                "project_chat_not_found",
                                format!("Project Chat not found: {chat_id}"),
                            )
                        })?;
                    let selected_agent_id =
                        crate::services::project_chat::selection_from_header(&header)
                            .map_err(|error| reject("project_agent_unavailable", error))?
                            .agent_id;
                    if command.agent_id != selected_agent_id && !project_discovery {
                        return Err(reject(
                            "stale_project_agent_selection",
                            format!(
                                "Project Chat Agent Selection changed from '{}' to '{}'",
                                command.agent_id, selected_agent_id
                            ),
                        ));
                    }
                    self.registry
                        .get_or_create(&selected_agent_id)
                        .await
                        .map_err(|error| reject("workspace_unavailable", error.to_string()))?;
                    (
                        selected_agent_id,
                        scope_root,
                        crate::services::project_chat::working_directory(
                            &binding,
                            Some(&header),
                            None,
                        )
                        .map_err(|error| reject("project_directory_unavailable", error))?,
                        Some(project.resource_device_id),
                    )
                }
            };
        let placement =
            crate::services::conversation_authority_target::load_chat_authority_placement_target(
                &scope_root,
                chat_id,
            )
            .await
            .map_err(|detail| reject("authority_placement_unavailable", detail))?;
        if let Some(resource_device_id) = project_resource_device_id.as_deref() {
            if placement.device_id != resource_device_id {
                return Err(reject(
                    "project_authority_fixed",
                    "Project Chat authority does not match its resource Device".to_string(),
                ));
            }
        }
        if placement.device_id != command.target_device_id
            || placement.authority_epoch != command.authority_epoch
        {
            if project_discovery {
                return Ok(ConversationControlTarget {
                    address: command.address.clone(),
                    execution_agent_id,
                    scope_root,
                    working_dir,
                    placement: ConversationAuthorityPlacement {
                        device_id: placement.device_id,
                        authority_epoch: placement.authority_epoch,
                        base_manifest_hash: placement.base_manifest_hash,
                    },
                });
            }
            if command.address.project_id().is_some() {
                return Err(reject(
                    "stale_authority_epoch",
                    "Project Chat control targets a stale fixed authority Epoch".to_string(),
                ));
            }
            if placement.authority_epoch > command.authority_epoch {
                return Err(DeviceCommandDeliveryError::AuthorityRedirect(Box::new(
                    ConversationAuthorityRedirect {
                        address: command.address.clone(),
                        placement: ConversationAuthorityPlacement {
                            device_id: placement.device_id,
                            authority_epoch: placement.authority_epoch,
                            base_manifest_hash: placement.base_manifest_hash,
                        },
                    },
                )));
            }
            return Err(reject(
                "stale_authority_epoch",
                "Conversation control targets a stale authority Epoch".to_string(),
            ));
        }
        Ok(ConversationControlTarget {
            address: command.address.clone(),
            execution_agent_id,
            scope_root,
            working_dir,
            placement: ConversationAuthorityPlacement {
                device_id: placement.device_id,
                authority_epoch: placement.authority_epoch,
                base_manifest_hash: placement.base_manifest_hash,
            },
        })
    }

    pub(crate) async fn apply_conversation_control(
        &self,
        command: ConversationControlDeviceCommand,
    ) -> Result<ConversationControlDeviceResponse, DeviceCommandDeliveryError> {
        let reject = |reason: &str, detail: String| DeviceCommandDeliveryError::Rejected {
            reason: reason.to_string(),
            detail,
        };
        let target = self.resolve_conversation_control_target(&command).await?;
        let authority = self.conversation_authority.as_ref().ok_or_else(|| {
            reject(
                "conversation_authority_unavailable",
                "target Conversation Authority is unavailable".to_string(),
            )
        })?;
        let mut meta = crate::services::conversation_authority_target::load_chat_control_meta(
            &target.scope_root,
            target.address.chat_id().as_str(),
        )
        .await
        .map_err(|error| reject("chat_session_unavailable", error))?
        .unwrap_or_else(|| serde_json::json!({}));
        if !meta.is_object() {
            meta = serde_json::json!({});
        }
        if let Ok(model) = self
            .authority_chat_model_for(&target.scope_root, target.address.chat_id().as_str())
            .await
        {
            meta.as_object_mut()
                .expect("control metadata is an object")
                .insert(CHAT_MODEL_META_KEY.to_string(), model);
        }

        match command.action {
            ConversationControlDeviceAction::DeliverBackgroundResult { note } => {
                let note: rua_session::RuntimeNote = serde_json::from_value(note)
                    .map_err(|e| reject("invalid_background_result", e.to_string()))?;
                if !matches!(
                    note.payload,
                    rua_session::RuntimeNotePayload::TaskAttemptTerminal(_)
                        | rua_session::RuntimeNotePayload::CollaborationResult(_)
                ) {
                    return Err(reject(
                        "invalid_background_result",
                        "Only Task or collaboration terminal results may resume the source".into(),
                    ));
                }
                let id = note.runtime_note_id.clone();
                Box::pin(self.deliver_background_result(target.address.clone(), note))
                    .await
                    .map_err(|e| reject("background_delivery_failed", e))?;
                Ok(
                    ConversationControlDeviceResponse::BackgroundResultDelivered {
                        observation_id: id,
                    },
                )
            }
            ConversationControlDeviceAction::PauseQueuedInputs { authority_epoch } => {
                let gate = authority.transition_gate_at(&target.address);
                let _transition = gate.lock().await;
                if target.placement.authority_epoch != authority_epoch
                    || authority.active_turn(&target.address).await.is_some()
                {
                    return Ok(ConversationControlDeviceResponse::StopRequested { applied: false });
                }
                let applied = authority.pause_queued_inputs_locked(&target.address).await;
                Ok(ConversationControlDeviceResponse::StopRequested { applied })
            }
            ConversationControlDeviceAction::StopTurn {
                turn_id,
                authority_epoch,
            } => {
                let applied = self
                    .stop_observed_console_turn(&target, &turn_id, authority_epoch)
                    .await
                    .map_err(|detail| reject("stop_failed", detail))?;
                Ok(ConversationControlDeviceResponse::StopRequested { applied })
            }
            ConversationControlDeviceAction::Discover | ConversationControlDeviceAction::View => {
                Ok(ConversationControlDeviceResponse::View {
                    control: authority.control_view(&target.address, &meta).await,
                    execution_agent_id: Some(target.execution_agent_id.clone()),
                    authority_placement: Some(target.placement.clone()),
                })
            }
            ConversationControlDeviceAction::ViewRunningConfig => {
                let running = self
                    .authority_chat_running_config(
                        &target.address,
                        &target.execution_agent_id,
                        &target.scope_root,
                    )
                    .await
                    .map_err(|error| {
                        reject("chat_running_config_unavailable", error.to_string())
                    })?;
                Ok(ConversationControlDeviceResponse::RunningConfig {
                    running: serde_json::to_value(running)
                        .map_err(|error| reject("chat_protocol_error", error.to_string()))?,
                })
            }
            ConversationControlDeviceAction::ViewConfiguration
            | ConversationControlDeviceAction::UpdateConfiguration { .. } => {
                let update = match &command.action {
                    ConversationControlDeviceAction::UpdateConfiguration {
                        expected_epoch,
                        candidate_epoch,
                    } => Some((expected_epoch.as_str(), candidate_epoch.as_str())),
                    _ => None,
                };
                let configuration = self
                    .chat_configuration_control(
                        &target.address,
                        &target.execution_agent_id,
                        &target.scope_root,
                        update,
                    )
                    .await
                    .map_err(|detail| reject("chat_configuration_invalid", detail))?;
                Ok(ConversationControlDeviceResponse::Configuration { configuration })
            }
            ConversationControlDeviceAction::ViewProjectionExclusions => {
                let store = rua_session::WorkspaceSessionStore::new(&target.scope_root);
                let exclusions = store
                    .load_projection_exclusions(target.address.chat_id().as_str())
                    .await
                    .map_err(|error| {
                        reject("projection_exclusions_unavailable", error.to_string())
                    })?;
                Ok(ConversationControlDeviceResponse::ProjectionExclusions {
                    message_ids: exclusions.message_ids,
                })
            }
            ConversationControlDeviceAction::ViewResourcePolicy => {
                if target.address.project_id().is_none() {
                    return Err(reject(
                        "wrong_chat_scope",
                        "Resource Policy is available only for Project Chat".to_string(),
                    ));
                }
                let configuration = self
                    .load_chat_configuration(
                        &target.address,
                        &target.execution_agent_id,
                        &target.scope_root,
                    )
                    .await
                    .map_err(|error| reject("security_config_unavailable", error))?
                    .ok_or_else(|| {
                        reject(
                            "security_config_unavailable",
                            "Chat configuration missing".to_string(),
                        )
                    })?;
                let config = configuration.security_config;
                let binding = self
                    .project_bindings
                    .as_ref()
                    .ok_or_else(|| {
                        reject(
                            "project_resources_unavailable",
                            "Project bindings are unavailable".into(),
                        )
                    })?
                    .get(
                        target
                            .address
                            .project_id()
                            .expect("Project scope checked above"),
                    )
                    .map_err(|error| reject("project_resources_unavailable", error.to_string()))?
                    .ok_or_else(|| {
                        reject(
                            "project_resources_unavailable",
                            "Project binding is missing".into(),
                        )
                    })?;
                let roots = binding
                    .resources
                    .directories
                    .iter()
                    .map(|directory| directory.resource_root.clone())
                    .collect::<Vec<_>>();
                let policy = rua_security::EffectiveProjectResourcePolicy::resolve_with_roots(
                    &config,
                    &target.working_dir,
                    &roots,
                );
                let approval_authority = self.approval_authority.as_ref().ok_or_else(|| {
                    reject(
                        "approval_authority_unavailable",
                        "Approval Authority is unavailable".to_string(),
                    )
                })?;
                let snapshot = approval_authority.snapshot_at(&target.address).await;
                let chat_rule_count = approval_authority
                    .list_chat_rules(&target.address)
                    .await
                    .len();
                let agent_rule_count = config
                    .extra
                    .get("permission_rules")
                    .and_then(serde_json::Value::as_array)
                    .map_or(0, Vec::len);
                Ok(ConversationControlDeviceResponse::ResourcePolicy {
                    workspace: ProjectResourcePolicyView {
                        policy,
                        pending_approval_count: snapshot.pending.len(),
                        chat_rule_count,
                        agent_rule_count,
                    },
                })
            }
            ConversationControlDeviceAction::UpdateModel { model } => {
                let model = self
                    .update_authority_chat_model_for(
                        &target.address,
                        &target.execution_agent_id,
                        &target.scope_root,
                        model,
                    )
                    .await
                    .map_err(|detail| reject("chat_model_update_rejected", detail))?;
                meta.as_object_mut()
                    .expect("control metadata is an object")
                    .insert(CHAT_MODEL_META_KEY.to_string(), model.clone());
                Ok(ConversationControlDeviceResponse::ModelUpdated {
                    model: model.clone(),
                    control: authority.control_view(&target.address, &meta).await,
                    chat: crate::services::chat_index::get(
                        &target.scope_root,
                        target.address.chat_id().as_str(),
                    )
                    .await
                    .map_err(|error| reject("chat_index_unavailable", error))?
                    .map(crate::endpoints::chats::chat_from_index_entry)
                    .map(|mut chat| {
                        chat.model = serde_json::from_value(model.clone()).ok();
                        chat
                    })
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|error| reject("chat_protocol_error", error.to_string()))?,
                })
            }
            ConversationControlDeviceAction::UpdateRunningConfig { running } => {
                let applied = self
                    .update_authority_chat_running_config(
                        &target.address,
                        &target.execution_agent_id,
                        &target.scope_root,
                        running,
                    )
                    .await
                    .map_err(|error| {
                        reject("chat_running_config_update_rejected", error.to_string())
                    })?;
                let mut control = authority.control_view(&target.address, &meta).await;
                let running = serde_json::to_value(&applied)
                    .map_err(|error| reject("chat_protocol_error", error.to_string()))?;
                control.running_config = Some(running.clone());
                Ok(ConversationControlDeviceResponse::RunningConfigUpdated { running, control })
            }
            ConversationControlDeviceAction::UpdateProjectionExclusions { message_ids } => {
                let store = rua_session::WorkspaceSessionStore::new(&target.scope_root);
                let chat_id = target.address.chat_id().as_str();
                let messages = store
                    .read_message_range(chat_id, 1, usize::MAX)
                    .await
                    .map_err(|error| {
                        reject("projection_exclusions_unavailable", error.to_string())
                    })?;
                let by_id = messages
                    .iter()
                    .map(|message| (message.message_id.as_str(), message))
                    .collect::<std::collections::HashMap<_, _>>();
                let mut excluded = std::collections::HashSet::new();
                let mut tool_call_ids = std::collections::HashSet::new();
                for message_id in message_ids {
                    let message = by_id.get(message_id.as_str()).ok_or_else(|| {
                        reject(
                            "projection_exclusion_message_not_found",
                            format!("message '{message_id}' does not belong to this Chat"),
                        )
                    })?;
                    if message.role == "user" {
                        return Err(reject(
                            "projection_exclusion_user_message_rejected",
                            "User messages cannot be excluded from Context Projection".to_string(),
                        ));
                    }
                    excluded.insert(message.message_id.clone());
                    if let Some(tool_call_id) = message
                        .tool
                        .as_ref()
                        .and_then(|tool| tool.tool_call_id.as_ref())
                    {
                        tool_call_ids.insert(tool_call_id.clone());
                    }
                }
                for message in &messages {
                    if message
                        .tool
                        .as_ref()
                        .and_then(|tool| tool.tool_call_id.as_ref())
                        .is_some_and(|tool_call_id| tool_call_ids.contains(tool_call_id))
                    {
                        excluded.insert(message.message_id.clone());
                    }
                }
                let exclusions = store
                    .save_projection_exclusions(chat_id, excluded.into_iter().collect())
                    .await
                    .map_err(|error| {
                        reject("projection_exclusions_update_rejected", error.to_string())
                    })?;
                self.session_sync
                    .emit_evergreen_chat_snapshot(&target.scope_root, &target.address)
                    .await;
                authority
                    .publish_history_mutation(
                        &target.address,
                        target.placement.authority_epoch,
                        serde_json::json!({
                            "action": "projection_exclusions_updated",
                            "message_ids": exclusions.message_ids,
                        }),
                    )
                    .await;
                self.sync_committed_state_after_foreground(
                    &target.address,
                    &command.source_device_id,
                )
                .await;
                Ok(
                    ConversationControlDeviceResponse::ProjectionExclusionsUpdated {
                        message_ids: exclusions.message_ids,
                    },
                )
            }
            ConversationControlDeviceAction::SelectVariant {
                turn_id,
                selected_variant_id,
            } => {
                let common_chat = rua_chat::ChatService::for_resolved_scope(
                    &target.scope_root,
                    target.address.clone(),
                )
                .select_variant(&target.address, &turn_id, &selected_variant_id)
                .await
                .map_err(project_chat_mutation_error)?;
                let chat = crate::endpoints::chats::chat_spec_from_common_chat(
                    &target.scope_root,
                    &common_chat,
                )
                .await
                .map_err(project_chat_mutation_error)?;
                self.session_sync
                    .emit_chat_index_upsert_for(
                        &target.address,
                        serde_json::to_value(&chat)
                            .map_err(|error| reject("chat_protocol_error", error.to_string()))?,
                    )
                    .await;
                self.session_sync
                    .emit_evergreen_chat_snapshot(&target.scope_root, &target.address)
                    .await;
                authority
                    .publish_history_mutation(
                        &target.address,
                        target.placement.authority_epoch,
                        serde_json::json!({
                            "action": "variant_selected",
                            "turn_id": turn_id,
                            "selected_variant_id": selected_variant_id,
                        }),
                    )
                    .await;
                self.sync_committed_state_after_foreground(
                    &target.address,
                    &command.source_device_id,
                )
                .await;
                Ok(ConversationControlDeviceResponse::VariantSelected {
                    turn_id,
                    selected_variant_id,
                })
            }
            ConversationControlDeviceAction::RollbackBeforeTurn { before_turn_id } => {
                rua_session::WorkspaceSessionStore::new(&target.scope_root)
                    .set_collaboration_wake_suppressed(target.address.chat_id().as_str(), true)
                    .await
                    .map_err(|error| {
                        reject("rollback_background_suppression_failed", error.to_string())
                    })?;
                let chat_service = rua_chat::ChatService::for_resolved_scope(
                    &target.scope_root,
                    target.address.clone(),
                );
                let branch_point = chat_service
                    .resolve_history_boundary(&target.address, &before_turn_id)
                    .await
                    .map_err(project_chat_mutation_error)?;
                let selected_turn_id = branch_point
                    .target_turn
                    .as_ref()
                    .map(|turn| turn.turn_id.clone());
                let selected_variant_id = branch_point.selected_variant_id.clone();
                let task_branch_point = branch_point
                    .target_turn
                    .as_ref()
                    .and_then(|turn| {
                        turn.lifecycle
                            .assistant_completed_at
                            .as_deref()
                            .or(Some(turn.lifecycle.user_submitted_at.as_str()))
                    })
                    .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
                    .map(|value| value.with_timezone(&chrono::Utc));
                let cancelled_tasks = if let Some(task_runtime) = self.task_runtime.as_ref() {
                    task_runtime
                        .cancel_owner_at_or_after(
                            &rua_task::TaskOwner::from_address(target.address.clone()),
                            task_branch_point,
                        )
                        .await
                        .map_err(|error| {
                            reject("project_chat_task_cancel_failed", error.to_string())
                        })?
                } else {
                    0
                };
                let common_chat = chat_service
                    .rollback_before(&target.address, &branch_point.before_turn_id)
                    .await
                    .map_err(project_chat_mutation_error)?;
                let chat = crate::endpoints::chats::chat_spec_from_common_chat(
                    &target.scope_root,
                    &common_chat,
                )
                .await
                .map_err(project_chat_mutation_error)?;
                let chat_value = serde_json::to_value(&chat)
                    .map_err(|error| reject("chat_protocol_error", error.to_string()))?;
                self.session_sync
                    .emit_chat_index_upsert_for(&target.address, chat_value.clone())
                    .await;
                self.session_sync
                    .emit_evergreen_chat_snapshot(&target.scope_root, &target.address)
                    .await;
                authority
                    .publish_history_mutation(
                        &target.address,
                        target.placement.authority_epoch,
                        serde_json::json!({
                            "action": "rolled_back",
                            "before_turn_id": before_turn_id,
                            "selected_turn_id": selected_turn_id,
                            "selected_variant_id": selected_variant_id,
                        }),
                    )
                    .await;
                self.sync_committed_state_after_foreground(
                    &target.address,
                    &command.source_device_id,
                )
                .await;
                Ok(ConversationControlDeviceResponse::RolledBack {
                    chat: chat_value,
                    cancelled_tasks,
                })
            }
            ConversationControlDeviceAction::BranchBeforeTurn { before_turn_id } => {
                self.authority_chat_running_config(
                    &target.address,
                    &target.execution_agent_id,
                    &target.scope_root,
                )
                .await
                .map_err(project_chat_mutation_error)?;
                let chat_service = rua_chat::ChatService::for_resolved_scope(
                    &target.scope_root,
                    target.address.clone(),
                );
                let branch_point = chat_service
                    .resolve_history_boundary(&target.address, &before_turn_id)
                    .await
                    .map_err(project_chat_mutation_error)?;
                let common_chat = chat_service
                    .branch_before(&target.address, &branch_point.before_turn_id)
                    .await
                    .map_err(project_chat_mutation_error)?;
                let chat = crate::endpoints::chats::chat_spec_from_common_chat(
                    &target.scope_root,
                    &common_chat,
                )
                .await
                .map_err(project_chat_mutation_error)?;
                let branch_address = ChatAddress::project(
                    target
                        .address
                        .project_id()
                        .expect("Project branch action requires Project Scope")
                        .clone(),
                    &chat.id,
                );
                let chat_value = serde_json::to_value(&chat)
                    .map_err(|error| reject("chat_protocol_error", error.to_string()))?;
                self.session_sync
                    .emit_chat_index_upsert_for(&branch_address, chat_value.clone())
                    .await;
                self.session_sync
                    .emit_evergreen_chat_snapshot(&target.scope_root, &branch_address)
                    .await;
                self.sync_committed_state_after_foreground(
                    &target.address,
                    &command.source_device_id,
                )
                .await;
                Ok(ConversationControlDeviceResponse::Branched { chat: chat_value })
            }
            ConversationControlDeviceAction::EditQueuedInput { input_id, message } => {
                let entry = authority
                    .edit_queued_input(&target.address, &input_id, &message)
                    .await
                    .ok_or_else(|| {
                        reject(
                            "queued_input_not_editable",
                            "queued input is not editable".to_string(),
                        )
                    })?;
                Ok(ConversationControlDeviceResponse::Edited {
                    entry,
                    control: authority.control_view(&target.address, &meta).await,
                })
            }
            ConversationControlDeviceAction::DeleteQueuedInput { input_id } => {
                let entry = authority
                    .delete_queued_input(&target.address, &input_id)
                    .await
                    .ok_or_else(|| {
                        reject(
                            "queued_input_not_editable",
                            "queued input is not editable".to_string(),
                        )
                    })?;
                Ok(ConversationControlDeviceResponse::Deleted {
                    deleted_input_id: entry.input_id,
                    control: authority.control_view(&target.address, &meta).await,
                })
            }
            ConversationControlDeviceAction::SendNow {
                input_id,
                control_command_id,
            } => {
                let entry = self
                    .send_now_for(
                        &target,
                        &input_id,
                        command.authority_epoch,
                        &control_command_id,
                    )
                    .await
                    .map_err(|detail| reject("send_now_rejected", detail))?;
                Ok(ConversationControlDeviceResponse::SendNow {
                    input_id: entry.input_id,
                    applied: true,
                    control: authority.control_view(&target.address, &meta).await,
                })
            }
            ConversationControlDeviceAction::ApprovalSnapshot => {
                let approval_authority = self.approval_authority.as_ref().ok_or_else(|| {
                    reject(
                        "approval_authority_unavailable",
                        "Approval Authority is unavailable".to_string(),
                    )
                })?;
                let snapshot = approval_authority.snapshot_at(&target.address).await;
                Ok(ConversationControlDeviceResponse::ApprovalSnapshot {
                    snapshot: serde_json::to_value(snapshot)
                        .map_err(|error| reject("approval_protocol_error", error.to_string()))?,
                })
            }
            ConversationControlDeviceAction::DecideApproval {
                approval_id,
                decision,
            } => {
                let approval_authority = self.approval_authority.as_ref().ok_or_else(|| {
                    reject(
                        "approval_authority_unavailable",
                        "Approval Authority is unavailable".to_string(),
                    )
                })?;
                let request = approval_authority
                    .request(&approval_id)
                    .await
                    .ok_or_else(|| {
                        reject(
                            "approval_not_found",
                            format!("approval request not found: {approval_id}"),
                        )
                    })?;
                if request.agent_id != target.execution_agent_id
                    || request.owner.as_chat_address() != Some(&target.address)
                {
                    return Err(reject(
                        "approval_wrong_scope",
                        format!("approval request not found: {approval_id}"),
                    ));
                }
                let should_publish_agent =
                    matches!(decision, ConversationApprovalDecision::AllowAgent { .. });
                let decision = match decision {
                    ConversationApprovalDecision::RunOnce { decided_by } => {
                        rua_security::approval_authority::ApprovalDecision::RunOnce { decided_by }
                    }
                    ConversationApprovalDecision::AllowChat {
                        decided_by,
                        matcher,
                    } => rua_security::approval_authority::ApprovalDecision::AllowChat {
                        decided_by,
                        matcher,
                    },
                    ConversationApprovalDecision::AllowAgent {
                        decided_by,
                        matcher,
                    } => rua_security::approval_authority::ApprovalDecision::AllowAgent {
                        decided_by,
                        matcher,
                    },
                    ConversationApprovalDecision::Reject { decided_by } => {
                        rua_security::approval_authority::ApprovalDecision::Reject { decided_by }
                    }
                };
                let _ = authority
                    .transition_active_turn_phase(
                        &target.address,
                        Some(&request.turn_id),
                        crate::services::conversation_authority::ConversationActiveTurnPhase::RunningTool,
                    )
                    .await;
                let outcome = approval_authority
                    .decide_for_agent(&target.execution_agent_id, &approval_id, decision)
                    .await;
                if outcome.is_err()
                    && approval_authority
                        .request(&approval_id)
                        .await
                        .is_some_and(|request| {
                            request.status
                                == rua_security::approval_authority::ApprovalStatus::Pending
                        })
                {
                    let _ = authority
                        .transition_active_turn_phase(
                            &target.address,
                            Some(&request.turn_id),
                            crate::services::conversation_authority::ConversationActiveTurnPhase::WaitingApproval,
                        )
                        .await;
                }
                let request = outcome.map_err(|error| {
                    let reason = match error {
                        rua_security::approval_authority::ApprovalAuthorityError::NotFound(_) => {
                            "approval_not_found"
                        }
                        rua_security::approval_authority::ApprovalAuthorityError::InvalidState {
                            ..
                        } => "approval_invalid_state",
                        rua_security::approval_authority::ApprovalAuthorityError::Store(_) => {
                            "approval_store_failed"
                        }
                    };
                    reject(reason, error.to_string())
                })?;
                if should_publish_agent {
                    if let Some(service) = self.agent_service.as_ref() {
                        if let Ok(Some(agent)) = service.get_agent(&target.execution_agent_id).await
                        {
                            self.session_sync
                                .emit_agent_config_committed(
                                    &target.execution_agent_id,
                                    serde_json::to_value(agent).unwrap_or_default(),
                                )
                                .await;
                        }
                    }
                }
                Ok(ConversationControlDeviceResponse::ApprovalDecided {
                    request: serde_json::to_value(request)
                        .map_err(|error| reject("approval_protocol_error", error.to_string()))?,
                })
            }
        }
    }

    pub(crate) async fn apply_acp_control(
        &self,
        command: AcpChatDeviceCommand,
    ) -> Result<AcpChatDeviceResponse, DeviceCommandDeliveryError> {
        let reject = |reason: &str, detail: String| DeviceCommandDeliveryError::Rejected {
            reason: reason.to_string(),
            detail,
        };
        let _resource_admission = self
            .project_work_admission(&command.address)
            .map_err(|error| reject("project_resources_changing", error))?;
        let draft_discovery = if command.address.agent_id().is_some()
            && matches!(
                &command.action,
                AcpChatDeviceAction::ListRunners | AcpChatDeviceAction::ListSessions { .. }
            ) {
            let runtime = self
                .registry
                .get_or_create(&command.agent_id)
                .await
                .map_err(|error| reject("workspace_unavailable", error.to_string()))?;
            rua_session::WorkspaceSessionStore::new(runtime.workspace_dir())
                .load_chat_header(command.chat_id())
                .await
                .map_err(|error| reject("chat_unavailable", error.to_string()))?
                .is_none()
        } else {
            false
        };
        if draft_discovery {
            if current_sync_device_id(&self.sync_device).as_deref()
                != Some(command.target_device_id.as_str())
                || command.address.agent_id().map(|id| id.as_str())
                    != Some(command.agent_id.as_str())
            {
                return Err(reject(
                    "invalid_discovery_target",
                    "ACP discovery target mismatch".into(),
                ));
            }
            let handle = self
                .acp_chat_registry
                .as_ref()
                .and_then(|registry| registry.get(&command.agent_id))
                .ok_or_else(|| {
                    reject("acp_runtime_unavailable", "ACP runtime unavailable".into())
                })?;
            handle.refresh_config();
            if let AcpChatDeviceAction::ListSessions { runner, cwd } = &command.action {
                let config = handle.service.agent_config(runner).ok_or_else(|| {
                    reject(
                        "acp_runner_not_found",
                        format!("ACP runner `{runner}` not configured"),
                    )
                })?;
                if !config.enabled {
                    return Err(reject(
                        "acp_runner_disabled",
                        format!("ACP runner `{runner}` is disabled"),
                    ));
                }
                let filter = match cwd
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    Some("all") => None,
                    Some(path) => Some(expand_control_tilde(path)),
                    None => Some(handle.workspace_dir.to_string_lossy().to_string()),
                };
                let data_dir = self.data_dir.as_deref().ok_or_else(|| {
                    reject(
                        "workspace_unavailable",
                        "Workspace data root unavailable".into(),
                    )
                })?;
                let sessions = AcpClientAdapter::list_sessions_for_runtime(
                    &config,
                    filter,
                    &rua_config::dirs::resolve_cache_dir(data_dir),
                )
                .await
                .map_err(|error| reject("acp_list_sessions_failed", error.to_string()))?
                .into_iter()
                .map(|session| AcpChatDeviceSessionEntry {
                    session_id: session.session_id.to_string(),
                    title: session.title,
                    cwd: session.cwd.to_string_lossy().to_string(),
                    updated_at: session.updated_at,
                })
                .collect();
                return Ok(AcpChatDeviceResponse::Sessions { sessions });
            }
            return Ok(AcpChatDeviceResponse::Runners {
                runners: acp_runner_infos_from_service(&handle.service),
            });
        }
        let target = self
            .resolve_conversation_control_target(&ConversationControlDeviceCommand {
                command_id: command.command_id.clone(),
                agent_id: command.agent_id.clone(),
                address: command.address.clone(),
                source_device_id: command.source_device_id.clone(),
                target_device_id: command.target_device_id.clone(),
                authority_epoch: command.authority_epoch,
                action: ConversationControlDeviceAction::View,
            })
            .await?;
        let registry = self.acp_chat_registry.as_ref().ok_or_else(|| {
            reject(
                "acp_runtime_unavailable",
                "target device has no ACP chat runtime registry".to_string(),
            )
        })?;
        let handle = registry.get(&target.execution_agent_id).ok_or_else(|| {
            reject(
                "acp_runtime_unavailable",
                format!(
                    "ACP chat runtime for agent '{}' is not registered on target device",
                    target.execution_agent_id
                ),
            )
        })?;
        handle.refresh_config();
        let chat_id = target.address.chat_id().to_string();
        if matches!(
            &command.action,
            AcpChatDeviceAction::Enter { .. } | AcpChatDeviceAction::ListSessions { .. }
        ) {
            if let Some(project_id) = target.address.project_id() {
                let bindings = self.project_bindings.as_ref().ok_or_else(|| {
                    reject(
                        "project_binding_unavailable",
                        "Project Resource Binding Store is unavailable".to_string(),
                    )
                })?;
                bindings
                    .validate_resource_root(project_id)
                    .map_err(|error| reject("project_resource_unavailable", error.to_string()))?;
                self.validate_acp_project_resources(project_id)
                    .map_err(|error| reject("unsupported_project_resources", error))?;
            }
        }
        let project_cwd = target
            .address
            .project_id()
            .map(|_| target.working_dir.to_string_lossy().to_string());

        match command.action {
            AcpChatDeviceAction::ListRunners => Ok(AcpChatDeviceResponse::Runners {
                runners: acp_runner_infos_from_service(&handle.service),
            }),
            AcpChatDeviceAction::Enter {
                runner,
                cwd,
                resume_session_id,
            } => {
                if handle.manager.get(&chat_id).await.is_some() {
                    return Err(reject(
                        "acp_already_active",
                        "chat is already in ACP passthrough mode".to_string(),
                    ));
                }
                if let Some(authority) = self.conversation_authority.as_ref() {
                    if authority.active_turn(&target.address).await.is_some() {
                        return Err(reject(
                            "chat_active_turn",
                            "cannot enter ACP mode while the chat has an active turn".to_string(),
                        ));
                    }
                }
                let requested_cwd = project_cwd.or_else(|| {
                    cwd.as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(expand_control_tilde)
                });
                let (message, history_preview) = match resume_session_id.as_deref() {
                    Some(session_id) if !session_id.trim().is_empty() => {
                        let resumed = handle
                            .controller
                            .resume_with_history_preview(
                                &chat_id,
                                &runner,
                                session_id.trim(),
                                requested_cwd.as_deref(),
                            )
                            .await;
                        (resumed.message, resumed.history_preview)
                    }
                    _ => (
                        handle
                            .controller
                            .enter(&chat_id, &runner, requested_cwd.as_deref())
                            .await,
                        None,
                    ),
                };
                let status = acp_status_for_target(
                    &handle,
                    &chat_id,
                    &target.working_dir,
                    &command.target_device_id,
                )
                .await;
                if status.active {
                    let binding = AcpBindingMetadata {
                        target_device_id: Some(command.target_device_id.clone()),
                        runner: status.runner.clone().unwrap_or(runner),
                        cwd: status.cwd.clone(),
                        runner_session_id: status.session_id.clone(),
                    };
                    if let Err(detail) = self
                        .enter_authority_acp_for(
                            &target.address,
                            &target.execution_agent_id,
                            &target.scope_root,
                            binding,
                        )
                        .await
                    {
                        let _ = handle.controller.exit(&chat_id).await;
                        return Err(reject("acp_binding_update_rejected", detail));
                    }
                }
                Ok(AcpChatDeviceResponse::Entered {
                    ok: status.active,
                    message,
                    status,
                    history_preview,
                })
            }
            AcpChatDeviceAction::Exit => {
                let message = handle.controller.exit(&chat_id).await;
                self.exit_authority_acp_for(
                    &target.address,
                    &target.execution_agent_id,
                    &target.scope_root,
                )
                .await
                .map_err(|detail| reject("acp_binding_update_rejected", detail))?;
                Ok(AcpChatDeviceResponse::Exited { ok: true, message })
            }
            AcpChatDeviceAction::Status => Ok(AcpChatDeviceResponse::Status {
                status: acp_status_for_target(
                    &handle,
                    &chat_id,
                    &target.working_dir,
                    &command.target_device_id,
                )
                .await,
            }),
            AcpChatDeviceAction::ListSessions { runner, cwd } => {
                let config = handle.service.agent_config(&runner).ok_or_else(|| {
                    reject(
                        "acp_runner_not_found",
                        format!("ACP runner `{runner}` not configured on target device"),
                    )
                })?;
                if !config.enabled {
                    return Err(reject(
                        "acp_runner_disabled",
                        format!("ACP runner `{runner}` is disabled on target device"),
                    ));
                }
                let cwd_filter = if let Some(project_cwd) = project_cwd {
                    Some(project_cwd)
                } else {
                    match cwd
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        Some("all") => None,
                        Some(path) => Some(expand_control_tilde(path)),
                        None => Some(handle.workspace_dir.to_string_lossy().to_string()),
                    }
                };
                let data_dir = self.data_dir.as_deref().ok_or_else(|| {
                    reject(
                        "workspace_unavailable",
                        "target has no workspace data root".to_string(),
                    )
                })?;
                let cache_dir = rua_config::dirs::resolve_cache_dir(data_dir);
                let sessions =
                    AcpClientAdapter::list_sessions_for_runtime(&config, cwd_filter, &cache_dir)
                        .await
                        .map_err(|error| reject("acp_list_sessions_failed", error.to_string()))?
                        .into_iter()
                        .map(|session| AcpChatDeviceSessionEntry {
                            session_id: session.session_id.to_string(),
                            title: session.title,
                            cwd: session.cwd.to_string_lossy().to_string(),
                            updated_at: session.updated_at,
                        })
                        .collect();
                Ok(AcpChatDeviceResponse::Sessions { sessions })
            }
            AcpChatDeviceAction::PermissionSnapshot => {
                let relay = self.acp_permission_relay.as_ref().ok_or_else(|| {
                    reject(
                        "acp_permission_relay_unavailable",
                        "target ACP permission relay is unavailable".to_string(),
                    )
                })?;
                Ok(AcpChatDeviceResponse::PermissionSnapshot {
                    snapshot: relay.snapshot_for_address(&target.address).await,
                })
            }
            AcpChatDeviceAction::DecidePermission {
                request_id,
                option_id,
            } => {
                let relay = self.acp_permission_relay.as_ref().ok_or_else(|| {
                    reject(
                        "acp_permission_relay_unavailable",
                        "target ACP permission relay is unavailable".to_string(),
                    )
                })?;
                if let Some(request) = relay.request(&request_id).await {
                    if request.chat_address.as_ref() == &target.address
                        && request
                            .options
                            .iter()
                            .any(|option| option.option_id == option_id)
                    {
                        if let Some(authority) = self.conversation_authority.as_ref() {
                            let _ = authority
                                .transition_active_turn_phase(
                                    &target.address,
                                    None,
                                    crate::services::conversation_authority::ConversationActiveTurnPhase::RunningTool,
                                )
                                .await;
                        }
                    }
                }
                let request = relay
                    .decide_for_address(&target.address, &request_id, &option_id)
                    .await
                    .map_err(|error| match error {
                        rua_acp::permissions::AcpPermissionRelayError::NotFound(_) => {
                            reject("acp_permission_not_found", request_id.clone())
                        }
                        rua_acp::permissions::AcpPermissionRelayError::WrongScope => {
                            reject("acp_permission_wrong_scope", request_id.clone())
                        }
                        rua_acp::permissions::AcpPermissionRelayError::UnknownOption(_) => {
                            reject("acp_permission_unknown_option", option_id.clone())
                        }
                    })?;
                Ok(AcpChatDeviceResponse::PermissionDecided { request })
            }
        }
    }

    pub async fn switch_project_chat_agent(
        &self,
        project_id: &ProjectId,
        chat_id: &str,
        expected_agent_id: &str,
        selected_agent_id: &str,
        source_device_id: &str,
    ) -> Result<crate::services::project_chat::ProjectChatAgentState, String> {
        let data_dir = self
            .data_dir
            .as_ref()
            .ok_or_else(|| "Project Chat runtime is unavailable".to_string())?;
        let catalog = self
            .project_catalog
            .as_ref()
            .ok_or_else(|| "Project Catalog is unavailable".to_string())?;
        if catalog.lifecycle_transition_in_progress(project_id) {
            return Err("Project lifecycle transition is in progress".to_string());
        }
        let project = catalog
            .get(project_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Project not found: {project_id}"))?;
        if project.lifecycle != rua_project::ProjectLifecycle::Active {
            return Err("Archived Project cannot switch Agent".to_string());
        }
        let bindings = self
            .project_bindings
            .as_ref()
            .ok_or_else(|| "Project Resource Binding Store is unavailable".to_string())?;
        let binding = bindings
            .get(project_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Project Resource Binding not found: {project_id}"))?;
        if binding.resource_device_id != project.resource_device_id {
            return Err("Project Resource Binding Device does not match the Project".to_string());
        }

        let address = ChatAddress::project(project_id.clone(), chat_id);
        let scope_root = ChatScopeStore::resolve(data_dir, address.clone())
            .root()
            .to_path_buf();
        if rua_session::WorkspaceSessionStore::new(&scope_root)
            .load_chat_header(chat_id)
            .await
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err(format!("Project Chat not found: {chat_id}"));
        }
        let service = self
            .agent_service
            .as_ref()
            .ok_or_else(|| "Agent configuration service is unavailable".to_string())?;
        let selected_agent = service
            .get_agent(selected_agent_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Agent not found: {selected_agent_id}"))?;
        if !selected_agent.enabled {
            return Err(format!("Agent '{selected_agent_id}' is disabled"));
        }
        self.registry
            .get_or_create(selected_agent_id)
            .await
            .map_err(|error| format!("workspace unavailable: {error}"))?;
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        let approval_authority = self
            .approval_authority
            .as_ref()
            .ok_or_else(|| "Approval Authority is unavailable".to_string())?;
        let header = rua_session::WorkspaceSessionStore::new(&scope_root)
            .load_chat_header(chat_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Project Chat not found: {chat_id}"))?;
        let configuration = match self.product_hooks.as_ref() {
            Some(product_hooks) => {
                let configuration_assignment =
                    self.resolve_project_turn_agent(selected_agent_id).await?;
                let authority_device_id = header
                    .extra
                    .get(
                        crate::services::conversation_authority_placement::CHAT_AUTHORITY_PLACEMENT_META_KEY,
                    )
                    .and_then(|placement| placement.get("device_id"))
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        "Project Chat configuration requires an explicit Authority Device"
                            .to_string()
                    })?
                    .to_string();
                Some(
                    product_hooks
                        .resolve_complete_chat_configuration(
                            &address,
                            self.capture_chat_configuration(
                                configuration_assignment,
                                rua_config::ChatRunningConfig::from_agent_defaults(
                                    &self.authority_running_config_typed(selected_agent_id).await.map_err(|error| error.to_string())?
                                ),
                                Some(&crate::services::project_chat::working_directory(&binding, Some(&header), None)?),
                                crate::services::chat_configuration_capture::ChatConfigurationSelection {
                                    model: header.extra.get("model").cloned(),
                                    thinking: header.extra.get("thinking").cloned(),
                                    collaboration_mode: header
                                    .extra
                                    .get(rua_core::CHAT_COLLABORATION_MODE_META_KEY)
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        serde_json::json!(
                                            rua_core::AgentCollaborationMode::default()
                                        )
                                    }),
                                    harness_configuration: header
                                    .extra
                                    .get("harness_configuration")
                                    .cloned(),
                                    authority_device_id,
                                },
                            ).await?,
                        )
                        .await?,
                )
            }
            None => None,
        };
        let state = crate::services::project_chat::switch_project_chat_agent_with_configuration(
            &scope_root,
            &address,
            expected_agent_id,
            &selected_agent,
            source_device_id,
            authority,
            approval_authority,
            configuration.as_ref(),
        )
        .await?;
        if let (Some(product_hooks), Some(configuration)) =
            (self.product_hooks.as_ref(), configuration)
        {
            product_hooks
                .adopt_chat_configuration(&address, configuration)
                .await;
        }
        let meta = crate::services::conversation_authority_target::load_chat_control_meta(
            &scope_root,
            chat_id,
        )
        .await?
        .unwrap_or_else(|| serde_json::json!({}));
        authority
            .publish_project_agent_state_snapshot(&address, &meta, state.clone())
            .await;
        self.session_sync
            .emit_evergreen_chat_snapshot(&scope_root, &address)
            .await;
        self.remote_input_delivery
            .spawn_priority_sync_to_device(source_device_id);
        Ok(state)
    }

    pub async fn project_chat_agent_state(
        &self,
        project_id: &ProjectId,
        chat_id: &str,
    ) -> Result<crate::services::project_chat::ProjectChatAgentState, String> {
        let data_dir = self
            .data_dir
            .as_ref()
            .ok_or_else(|| "Project Chat runtime is unavailable".to_string())?;
        let address = ChatAddress::project(project_id.clone(), chat_id);
        let scope_root = ChatScopeStore::resolve(data_dir, address.clone())
            .root()
            .to_path_buf();
        let selection =
            crate::services::project_chat::load_project_chat_agent_selection(&scope_root, chat_id)
                .await?;
        let selected_agent_available = match self.agent_service.as_ref() {
            Some(service) => service
                .get_agent(&selection.agent_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|agent| agent.enabled),
            None => false,
        };
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        let approval_authority = self
            .approval_authority
            .as_ref()
            .ok_or_else(|| "Approval Authority is unavailable".to_string())?;
        crate::services::project_chat::project_chat_agent_state(
            &scope_root,
            &address,
            selected_agent_available,
            authority,
            approval_authority,
        )
        .await
    }

    pub async fn archive_idle_project_chats(
        &self,
        project_id: &ProjectId,
        chat_ids: Option<Vec<String>>,
        source_device_id: &str,
    ) -> Result<crate::services::project_chat::ProjectBulkArchiveResult, String> {
        use crate::services::project_chat::{
            ProjectBulkArchiveResult, ProjectChatArchiveItem, ProjectChatArchiveStatus,
        };
        let data_dir = self
            .data_dir
            .as_ref()
            .ok_or("Project runtime unavailable")?;
        let catalog = self
            .project_catalog
            .as_ref()
            .ok_or("Project catalog unavailable")?;
        let project = catalog
            .get(project_id)
            .map_err(|error| error.to_string())?
            .ok_or("Project not found")?;
        if project.lifecycle != rua_project::ProjectLifecycle::Active {
            return Err("Archived Project cannot update Chats".into());
        }
        let binding = self
            .project_bindings
            .as_ref()
            .ok_or("Project bindings unavailable")?
            .get(project_id)
            .map_err(|error| error.to_string())?
            .ok_or("Project binding unavailable")?;
        if binding.resource_device_id != project.resource_device_id {
            return Err("Resource Device mismatch".into());
        }
        let scope_root = ChatScopeStore::resolve(
            data_dir,
            ChatAddress::project(project_id.clone(), "00000000-0000-4000-8000-000000000000"),
        )
        .root()
        .to_path_buf();
        // Snapshot before mutation; never paginate a list that is shrinking as we archive.
        let ids = match chat_ids {
            Some(ids) => ids,
            None => crate::services::chat_index::list_all(&scope_root)
                .await?
                .into_iter()
                .filter(|chat| !chat.archived)
                .map(|chat| chat.id)
                .collect(),
        };
        let mut seen = std::collections::HashSet::new();
        let mut items = Vec::new();
        for chat_id in ids.into_iter().filter(|id| seen.insert(id.clone())) {
            let outcome = self
                .archive_idle_project_chat(project_id, &scope_root, &chat_id)
                .await;
            let (status, reason) = match outcome {
                Ok(reason) => (
                    if reason.is_some() {
                        ProjectChatArchiveStatus::Skipped
                    } else {
                        ProjectChatArchiveStatus::Archived
                    },
                    reason,
                ),
                Err(error) => (ProjectChatArchiveStatus::Failed, Some(error)),
            };
            items.push(ProjectChatArchiveItem {
                chat_id,
                status,
                reason,
            });
        }
        self.remote_input_delivery
            .spawn_priority_sync_to_device(source_device_id);
        Ok(ProjectBulkArchiveResult { items })
    }

    async fn archive_idle_project_chat(
        &self,
        project_id: &ProjectId,
        scope_root: &Path,
        chat_id: &str,
    ) -> Result<Option<String>, String> {
        // Coordinate with all Project work admissions and the per-Chat transition
        // gate. Busy work is observed, never cancelled by this listing operation.
        if chat_id.is_empty()
            || !chat_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err("Invalid Chat identity".into());
        }
        let catalog = self
            .project_catalog
            .as_ref()
            .ok_or("Project catalog unavailable")?;
        let admission = match catalog.begin_resource_edit(project_id) {
            Ok(guard) => guard,
            Err(_) => return Ok(Some("project_admission_in_progress".into())),
        };
        if catalog
            .get(project_id)
            .map_err(|error| error.to_string())?
            .is_none_or(|project| project.lifecycle != rua_project::ProjectLifecycle::Active)
        {
            return Err("Project is no longer active".into());
        }
        let address = ChatAddress::project(project_id.clone(), chat_id);
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or("Conversation authority unavailable")?;
        let gate = authority.transition_gate(&address);
        let transition = gate.lock().await;
        let header = rua_session::WorkspaceSessionStore::new(scope_root)
            .load_chat_header(chat_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("Chat no longer exists")?;
        if header.metadata.archived {
            return Ok(None);
        }
        let impact = self
            .project_lifecycle_impact(scope_root, &[(address.clone(), header)])
            .await?;
        if impact.affected_chat_count > 0 {
            let mut reasons = Vec::new();
            if impact.active_turn_count > 0 {
                reasons.push("active_turn");
            }
            if impact.queued_input_count > 0 {
                reasons.push("queued_input");
            }
            if impact.pending_approval_count > 0 {
                reasons.push("pending_approval");
            }
            if impact.pending_prompt_count > 0 {
                reasons.push("pending_prompt");
            }
            if impact.background_task_count > 0 {
                reasons.push("background_task");
            }
            return Ok(Some(reasons.join(",")));
        }
        let updated = crate::services::project_chat::update_project_chat(
            scope_root,
            &address,
            crate::services::project_chat::ProjectChatUpdate {
                archived: Some(true),
                ..Default::default()
            },
        )
        .await?;
        drop(transition);
        drop(admission);
        self.session_sync
            .emit_chat_index_upsert_for(
                &address,
                serde_json::to_value(&updated).map_err(|error| error.to_string())?,
            )
            .await;
        self.session_sync
            .emit_evergreen_chat_snapshot(scope_root, &address)
            .await;
        Ok(None)
    }

    pub async fn update_project_chat(
        &self,
        project_id: &ProjectId,
        chat_id: &str,
        update: crate::services::project_chat::ProjectChatUpdate,
        source_device_id: &str,
    ) -> Result<crate::endpoints::chats::ChatSpecDto, String> {
        let data_dir = self
            .data_dir
            .as_ref()
            .ok_or_else(|| "Project Chat runtime is unavailable".to_string())?;
        let catalog = self
            .project_catalog
            .as_ref()
            .ok_or_else(|| "Project Catalog is unavailable".to_string())?;
        if catalog.lifecycle_transition_in_progress(project_id) {
            return Err("Project lifecycle transition is in progress".to_string());
        }
        let project = catalog
            .get(project_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Project not found: {project_id}"))?;
        if project.lifecycle != rua_project::ProjectLifecycle::Active {
            return Err("Archived Project cannot update Chats".to_string());
        }
        let bindings = self
            .project_bindings
            .as_ref()
            .ok_or_else(|| "Project Resource Binding Store is unavailable".to_string())?;
        let binding = bindings
            .get(project_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Project Resource Binding not found: {project_id}"))?;
        if binding.resource_device_id != project.resource_device_id {
            return Err("Project Resource Binding Device does not match the Project".to_string());
        }

        let address = ChatAddress::project(project_id.clone(), chat_id);
        let scope_root = ChatScopeStore::resolve(data_dir, address.clone())
            .root()
            .to_path_buf();
        let configuration_gate = self
            .conversation_authority
            .as_ref()
            .map(|authority| authority.transition_gate(&address));
        let _transition = match configuration_gate.as_ref() {
            Some(gate) => Some(gate.lock().await),
            None => None,
        };
        let configuration_changed = update.collaboration_mode.is_some();
        if configuration_changed {
            let selection = crate::services::project_chat::load_project_chat_agent_selection(
                &scope_root,
                chat_id,
            )
            .await?;
            self.load_chat_configuration_locked(&address, &selection.agent_id, &scope_root)
                .await?;
        }
        let updated =
            crate::services::project_chat::update_project_chat(&scope_root, &address, update)
                .await?;
        if configuration_changed {
            if let Some(hooks) = self.product_hooks.as_ref() {
                if let Some(header) = rua_session::WorkspaceSessionStore::new(&scope_root)
                    .load_chat_header(chat_id)
                    .await
                    .map_err(|error| error.to_string())?
                {
                    if let Some(value) = header
                        .extra
                        .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
                    {
                        hooks
                            .adopt_chat_configuration(
                                &address,
                                serde_json::from_value(value.clone()).map_err(|error| {
                                    format!("invalid Chat configuration: {error}")
                                })?,
                            )
                            .await;
                    }
                }
            }
        }
        self.session_sync
            .emit_chat_index_upsert_for(
                &address,
                serde_json::to_value(&updated).map_err(|error| error.to_string())?,
            )
            .await;
        self.session_sync
            .emit_evergreen_chat_snapshot(&scope_root, &address)
            .await;
        self.remote_input_delivery
            .spawn_priority_sync_to_device(source_device_id);
        Ok(updated)
    }

    pub async fn delete_project_chat(
        &self,
        project_id: &ProjectId,
        chat_id: &str,
        force: bool,
        source_device_id: &str,
    ) -> Result<
        crate::services::project_chat::ProjectChatDeleteResult,
        crate::services::project_chat::ProjectChatDeleteError,
    > {
        use crate::services::project_chat::ProjectChatDeleteError;

        let data_dir = self.data_dir.as_ref().ok_or_else(|| {
            ProjectChatDeleteError::Failed("Project Chat runtime is unavailable".to_string())
        })?;
        let catalog = self.project_catalog.as_ref().ok_or_else(|| {
            ProjectChatDeleteError::Failed("Project Catalog is unavailable".to_string())
        })?;
        if catalog.lifecycle_transition_in_progress(project_id) {
            return Err(ProjectChatDeleteError::Failed(
                "Project lifecycle transition is in progress".to_string(),
            ));
        }
        let project = catalog
            .get(project_id)
            .map_err(|error| ProjectChatDeleteError::Failed(error.to_string()))?
            .ok_or_else(|| {
                ProjectChatDeleteError::NotFound(format!("Project not found: {project_id}"))
            })?;
        if project.lifecycle != rua_project::ProjectLifecycle::Active {
            return Err(ProjectChatDeleteError::Failed(
                "Archived Project cannot delete Chats".to_string(),
            ));
        }
        let bindings = self.project_bindings.as_ref().ok_or_else(|| {
            ProjectChatDeleteError::Failed(
                "Project Resource Binding Store is unavailable".to_string(),
            )
        })?;
        let binding = bindings
            .get(project_id)
            .map_err(|error| ProjectChatDeleteError::Failed(error.to_string()))?
            .ok_or_else(|| {
                ProjectChatDeleteError::NotFound(format!(
                    "Project Resource Binding not found: {project_id}"
                ))
            })?;
        if binding.resource_device_id != project.resource_device_id {
            return Err(ProjectChatDeleteError::Failed(
                "Project Resource Binding Device does not match the Project".to_string(),
            ));
        }

        let address = ChatAddress::project(project_id.clone(), chat_id);
        let scope_root = ChatScopeStore::resolve(data_dir, address.clone())
            .root()
            .to_path_buf();
        let deleting = BTreeSet::from([chat_id.to_string()]);
        let storage_snapshot = crate::services::chat_storage::scan_scope_for_deletion(
            &scope_root,
            address.scope().clone(),
            project.display_name.clone(),
            &deleting,
        )
        .await
        .map_err(ProjectChatDeleteError::Failed)?;
        let storage_entry = storage_snapshot
            .entries
            .iter()
            .find(|entry| entry.address.chat_id().as_str() == chat_id)
            .ok_or_else(|| {
                ProjectChatDeleteError::NotFound(format!("Project Chat not found: {chat_id}"))
            })?;
        if storage_entry.pinned {
            return Err(ProjectChatDeleteError::Pinned(format!(
                "Pinned Project Chat '{chat_id}' cannot be deleted; Unpin it first"
            )));
        }
        let storage_bytes = storage_entry
            .session_bytes
            .saturating_add(storage_entry.task_bytes);
        let header = rua_session::WorkspaceSessionStore::new(&scope_root)
            .load_chat_header(chat_id)
            .await
            .map_err(|error| ProjectChatDeleteError::Failed(error.to_string()))?;
        let selection = header
            .as_ref()
            .map(crate::services::project_chat::selection_from_header)
            .transpose()
            .map_err(ProjectChatDeleteError::Failed)?;
        let authority = self.conversation_authority.as_ref().ok_or_else(|| {
            ProjectChatDeleteError::Failed("Conversation Authority is unavailable".to_string())
        })?;
        let approval_authority = self.approval_authority.as_ref().ok_or_else(|| {
            ProjectChatDeleteError::Failed("Approval Authority is unavailable".to_string())
        })?;
        let active = authority.active_turn(&address).await.is_some();
        let queued = !authority.queue_snapshot(&address).await.is_empty();
        let approval = !approval_authority
            .list_pending_for_address(&address)
            .await
            .is_empty();
        let prompt = rua_tool::extended::ask_user::AskUserService::global()
            .get_pending_by_session(chat_id)
            .await
            .is_some();
        let acp = header.as_ref().is_some_and(|header| {
            crate::services::conversation_authority_placement::chat_acp_binding_active(
                &serde_json::Value::Object(header.extra.clone()),
            )
        });
        let was_busy = active || queued || approval || prompt || acp;
        if was_busy && !force {
            return Err(ProjectChatDeleteError::Busy(format!(
                "Project Chat '{chat_id}' has active or pending work"
            )));
        }

        let current = crate::services::chat_index::get(&scope_root, chat_id)
            .await
            .map_err(ProjectChatDeleteError::Failed)?
            .ok_or_else(|| {
                ProjectChatDeleteError::NotFound(format!("Project Chat not found: {chat_id}"))
            })?;
        if current.pinned {
            return Err(ProjectChatDeleteError::Pinned(format!(
                "Pinned Project Chat '{chat_id}' cannot be deleted; Unpin it first"
            )));
        }
        crate::services::chat_deletion::ChatDeletionCoordinator::from_rua_root(data_dir)
            .commit_intent(&address)
            .map_err(ProjectChatDeleteError::Failed)?;

        if active {
            let _ = authority
                .request_stop(Some(self.streams.as_ref()), CONSOLE_CHANNEL_ID, &address)
                .await;
        }
        authority.clear_queue(&address).await;
        approval_authority.clear_by_address(&address).await;
        rua_tool::extended::ask_user::AskUserService::global()
            .clear_by_session(chat_id)
            .await;
        if let Some(selection) = selection {
            self.cancel_acp_turn(&selection.agent_id, chat_id).await;
        }
        if let Some(task_runtime) = self.task_runtime.as_ref() {
            task_runtime
                .remove_owner(&rua_task::TaskOwner::from_address(address.clone()))
                .await
                .map_err(|error| ProjectChatDeleteError::Failed(error.to_string()))?;
        }
        if active {
            let stopped = tokio::time::timeout(LOCAL_STOP_CONFIRM_TIMEOUT, async {
                while authority.active_turn(&address).await.is_some() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await;
            if stopped.is_err() {
                tracing::warn!(
                    project_id = %project_id,
                    chat_id,
                    "forcing removal of stale Active Turn state during Project Chat delete"
                );
                let _ = authority.finish_active_turn(&address).await;
            }
        }

        let (removable_media, _) =
            crate::services::chat_storage::removable_media(&storage_snapshot, &deleting);
        let media_owner = rua_sync::media_adapter::MediaOwner::Project(project_id.to_string());
        for media_id in &removable_media {
            self.session_sync
                .emit_media_deleted_for_checked(media_owner.clone(), media_id)
                .await
                .map_err(ProjectChatDeleteError::Failed)?;
        }
        self.session_sync
            .emit_chat_deleted_for_checked(&scope_root, &address)
            .await
            .map_err(ProjectChatDeleteError::Failed)?;
        let mut attachment_cleanup = crate::services::chat_storage::remove_local_attachments(
            &scope_root,
            &storage_snapshot,
            &deleting,
            &removable_media,
        )
        .map_err(ProjectChatDeleteError::Failed)?;

        crate::services::project_chat::remove_project_chat_storage(&scope_root, &address).await?;
        approval_authority
            .delete_chat_rules(&address)
            .await
            .map_err(ProjectChatDeleteError::Failed)?;
        self.remote_input_delivery
            .spawn_priority_sync_to_device(source_device_id);
        Ok(crate::services::project_chat::ProjectChatDeleteResult {
            chat_id: chat_id.to_string(),
            was_busy,
            freed_bytes: storage_bytes.saturating_add(attachment_cleanup.freed_bytes),
            shared_media_preserved: attachment_cleanup
                .shared_media_preserved
                .remove(chat_id)
                .unwrap_or_default(),
        })
    }

    pub async fn inspect_project_lifecycle(
        &self,
        project_id: &ProjectId,
    ) -> Result<ProjectLifecyclePreview, String> {
        let (project, scope_root, chats) = self.project_lifecycle_chats(project_id).await?;
        let impact = self.project_lifecycle_impact(&scope_root, &chats).await?;
        Ok(ProjectLifecyclePreview {
            project,
            impact: Some(impact),
            resource_device_revoked: false,
        })
    }

    pub async fn archive_project(
        &self,
        project_id: &ProjectId,
    ) -> Result<ProjectArchiveResult, String> {
        let catalog = self
            .project_catalog
            .as_ref()
            .ok_or_else(|| "Project Catalog is unavailable".to_string())?;
        catalog
            .begin_lifecycle_transition(project_id)
            .map_err(|error| error.to_string())?;
        let result = async {
            let (project, scope_root, chats) = self.project_lifecycle_chats(project_id).await?;
            if project.lifecycle == rua_project::ProjectLifecycle::Archived {
                return Ok(ProjectArchiveResult {
                    project,
                    impact: Some(ProjectLifecycleImpact::default()),
                    resource_device_revoked: false,
                    changed: false,
                });
            }
            let impact = self.project_lifecycle_impact(&scope_root, &chats).await?;
            self.stop_project_runtime(&chats).await?;
            let project = catalog
                .set_lifecycle(project_id, rua_project::ProjectLifecycle::Archived)
                .map_err(|error| error.to_string())?;
            Ok(ProjectArchiveResult {
                project,
                impact: Some(impact),
                resource_device_revoked: false,
                changed: true,
            })
        }
        .await;
        catalog.end_lifecycle_transition(project_id);
        result
    }

    pub async fn reopen_project(
        &self,
        project_id: &ProjectId,
    ) -> Result<ProjectReopenResult, String> {
        let catalog = self
            .project_catalog
            .as_ref()
            .ok_or_else(|| "Project Catalog is unavailable".to_string())?;
        catalog
            .begin_lifecycle_transition(project_id)
            .map_err(|error| error.to_string())?;
        let result = async {
            let (project, _scope_root, chats) = self.project_lifecycle_chats(project_id).await?;
            if project.lifecycle == rua_project::ProjectLifecycle::Active {
                return Ok(ProjectReopenResult {
                    project,
                    changed: false,
                });
            }
            let bindings = self
                .project_bindings
                .as_ref()
                .ok_or_else(|| "Project Resource Binding Store is unavailable".to_string())?;
            bindings
                .validate_resource_root(project_id)
                .map_err(|error| format!("Project resource cannot be reopened: {error}"))?;
            self.stop_project_runtime(&chats).await?;
            let project = catalog
                .set_lifecycle(project_id, rua_project::ProjectLifecycle::Active)
                .map_err(|error| error.to_string())?;
            Ok(ProjectReopenResult {
                project,
                changed: true,
            })
        }
        .await;
        catalog.end_lifecycle_transition(project_id);
        result
    }

    pub async fn project_delete_scope(
        &self,
        project_id: &ProjectId,
    ) -> Result<(PathBuf, Vec<ChatAddress>), String> {
        let data_dir = self
            .data_dir
            .as_ref()
            .ok_or_else(|| "Project Chat runtime is unavailable".to_string())?;
        let scope_root = ChatScopeStore::resolve(
            data_dir,
            ChatAddress::project(project_id.clone(), "00000000-0000-4000-8000-000000000000"),
        )
        .root()
        .to_path_buf();
        let mut chat_ids = BTreeSet::new();
        for chat in crate::services::chat_index::list_all(&scope_root).await? {
            chat_ids.insert(chat.id);
        }
        let sessions_dir = scope_root.join("sessions");
        if sessions_dir.is_dir() {
            for entry in std::fs::read_dir(&sessions_dir).map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                if entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    if let Some(chat_id) = entry.file_name().to_str() {
                        chat_ids.insert(chat_id.to_string());
                    }
                }
            }
        }
        let prompt_state_dir = scope_root.join("prompt-state");
        if prompt_state_dir.is_dir() {
            for entry in std::fs::read_dir(&prompt_state_dir).map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) == Some("json") {
                    if let Some(chat_id) = path.file_stem().and_then(|value| value.to_str()) {
                        chat_ids.insert(chat_id.to_string());
                    }
                }
            }
        }
        Ok((
            scope_root,
            chat_ids
                .into_iter()
                .map(|chat_id| ChatAddress::project(project_id.clone(), chat_id))
                .collect(),
        ))
    }

    pub async fn cleanup_permanently_deleted_project(
        &self,
        project_id: &ProjectId,
        scope_root: &Path,
        addresses: &[ChatAddress],
    ) -> Result<ProjectPermanentDeleteResult, String> {
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        let approvals = self
            .approval_authority
            .as_ref()
            .ok_or_else(|| "Approval Authority is unavailable".to_string())?;
        let store = rua_session::WorkspaceSessionStore::new(scope_root);

        for address in addresses {
            if authority.active_turn(address).await.is_some() {
                let _ = authority
                    .request_stop(Some(self.streams.as_ref()), CONSOLE_CHANNEL_ID, address)
                    .await;
            }
            authority.clear_queue(address).await;
            approvals.clear_by_address(address).await;
            rua_tool::extended::ask_user::AskUserService::global()
                .clear_by_session(address.chat_id().as_str())
                .await;
            if let Some(header) = store
                .load_chat_header(address.chat_id().as_str())
                .await
                .map_err(|error| error.to_string())?
            {
                if let Ok(selection) = crate::services::project_chat::selection_from_header(&header)
                {
                    self.cancel_acp_turn(&selection.agent_id, address.chat_id().as_str())
                        .await;
                    if crate::services::conversation_authority_placement::chat_acp_binding_active(
                        &serde_json::Value::Object(header.extra),
                    ) {
                        authority
                            .exit_acp(None, scope_root, address, &selection.agent_id)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
            if let Some(runtime) = self.task_runtime.as_ref() {
                runtime
                    .remove_owner(&rua_task::TaskOwner::from_address(address.clone()))
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }

        tokio::time::timeout(LOCAL_STOP_CONFIRM_TIMEOUT, async {
            loop {
                if futures::future::join_all(
                    addresses
                        .iter()
                        .map(|address| authority.active_turn(address)),
                )
                .await
                .into_iter()
                .all(|turn| turn.is_none())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| "timed out clearing Project runtime before Permanent Delete".to_string())?;

        approvals
            .delete_project_rules(project_id)
            .await
            .map_err(|error| error.to_string())?;
        match tokio::fs::remove_dir_all(scope_root).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        Ok(ProjectPermanentDeleteResult {
            project_id: project_id.clone(),
            deleted_chat_count: addresses.len(),
            changed: true,
        })
    }

    async fn project_lifecycle_chats(
        &self,
        project_id: &ProjectId,
    ) -> Result<
        (
            rua_project::ProjectDescriptor,
            PathBuf,
            Vec<(ChatAddress, rua_session::ChatSessionHeader)>,
        ),
        String,
    > {
        crate::services::project_lifecycle::load_project_chats(
            self.data_dir
                .as_deref()
                .ok_or("Project Chat runtime is unavailable")?,
            self.project_catalog
                .as_deref()
                .ok_or("Project Catalog is unavailable")?,
            self.project_bindings
                .as_deref()
                .ok_or("Project Resource Binding Store is unavailable")?,
            project_id,
        )
        .await
    }

    async fn project_lifecycle_impact(
        &self,
        _scope_root: &Path,
        chats: &[(ChatAddress, rua_session::ChatSessionHeader)],
    ) -> Result<ProjectLifecycleImpact, String> {
        crate::services::project_lifecycle::inspect_project_work(
            self.conversation_authority
                .as_deref()
                .ok_or("Conversation Authority is unavailable")?,
            self.approval_authority
                .as_deref()
                .ok_or("Approval Authority is unavailable")?,
            self.task_runtime.as_deref(),
            chats,
        )
        .await
    }

    async fn stop_project_runtime(
        &self,
        chats: &[(ChatAddress, rua_session::ChatSessionHeader)],
    ) -> Result<(), String> {
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        let approvals = self
            .approval_authority
            .as_ref()
            .ok_or_else(|| "Approval Authority is unavailable".to_string())?;
        for (address, header) in chats {
            if authority.active_turn(address).await.is_some() {
                let _ = authority
                    .request_stop(Some(self.streams.as_ref()), CONSOLE_CHANNEL_ID, address)
                    .await;
            }
            authority.clear_queue(address).await;
            approvals.clear_by_address(address).await;
            rua_tool::extended::ask_user::AskUserService::global()
                .clear_by_session(address.chat_id().as_str())
                .await;
            let selection = crate::services::project_chat::selection_from_header(header)?;
            self.cancel_acp_turn(&selection.agent_id, address.chat_id().as_str())
                .await;
            if let Some(runtime) = self.task_runtime.as_ref() {
                runtime
                    .cancel_owner(&rua_task::TaskOwner::from_address(address.clone()))
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }
        tokio::time::timeout(LOCAL_STOP_CONFIRM_TIMEOUT, async {
            loop {
                if futures::future::join_all(
                    chats
                        .iter()
                        .map(|(address, _)| authority.active_turn(address)),
                )
                .await
                .into_iter()
                .all(|turn| turn.is_none())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| "timed out stopping Project runtime before lifecycle commit".to_string())?;
        Ok(())
    }

    pub async fn cleanup_deleted_project_agent(&self, agent_id: &str) -> Result<usize, String> {
        let data_dir = self
            .data_dir
            .as_ref()
            .ok_or_else(|| "Project Chat runtime is unavailable".to_string())?;
        let catalog = self
            .project_catalog
            .as_ref()
            .ok_or_else(|| "Project Catalog is unavailable".to_string())?;
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        let approval_authority = self
            .approval_authority
            .as_ref()
            .ok_or_else(|| "Approval Authority is unavailable".to_string())?;
        let mut affected = 0usize;

        for project in catalog.list().map_err(|error| error.to_string())? {
            let scope_root = ChatScopeStore::resolve(
                data_dir,
                ChatAddress::project(
                    project.project_id.clone(),
                    "00000000-0000-4000-8000-000000000000",
                ),
            )
            .root()
            .to_path_buf();
            if !scope_root.join("chat-index.sqlite").is_file() {
                continue;
            }
            let mut offset = 0u64;
            loop {
                let page = crate::services::chat_index::list(
                    &scope_root,
                    crate::services::chat_index::ChatIndexFilter::default(),
                    offset,
                    256,
                )
                .await?;
                if page.entries.is_empty() {
                    break;
                }
                let total = page.total;
                for chat in page.entries {
                    let address = ChatAddress::project(project.project_id.clone(), chat.id.clone());
                    let store = rua_session::WorkspaceSessionStore::new(&scope_root);
                    let Some(header) = store
                        .load_chat_header(address.chat_id().as_str())
                        .await
                        .map_err(|error| error.to_string())?
                    else {
                        continue;
                    };
                    let Ok(selection) =
                        crate::services::project_chat::selection_from_header(&header)
                    else {
                        continue;
                    };
                    if selection.agent_id != agent_id {
                        continue;
                    }
                    affected += 1;
                    let _ = authority
                        .request_stop(Some(self.streams.as_ref()), CONSOLE_CHANNEL_ID, &address)
                        .await;
                    authority.clear_queue(&address).await;
                    approval_authority.clear_by_address(&address).await;
                    rua_tool::extended::ask_user::AskUserService::global()
                        .clear_by_session(address.chat_id().as_str())
                        .await;
                    self.cancel_acp_turn(agent_id, address.chat_id().as_str())
                        .await;
                    if crate::services::conversation_authority_placement::chat_acp_binding_active(
                        &serde_json::Value::Object(header.extra.clone()),
                    ) {
                        authority
                            .exit_acp(None, &scope_root, &address, agent_id)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    if let Some(task_runtime) = self.task_runtime.as_ref() {
                        task_runtime
                            .interrupt_owner(&rua_task::TaskOwner::from_address(address.clone()))
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    tokio::time::timeout(Duration::from_secs(8), async {
                        while authority.active_turn(&address).await.is_some() {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                    })
                    .await
                    .map_err(|_| {
                        format!(
                            "timed out stopping Project Chat {} before deleting Agent '{}'",
                            address.chat_id(),
                            agent_id
                        )
                    })?;
                }
                offset = offset.saturating_add(256);
                if offset >= total {
                    break;
                }
            }
        }
        if let Some(registry) = self.acp_chat_registry.as_ref() {
            registry.remove(agent_id);
        }
        Ok(affected)
    }

    pub async fn request_local_stop(&self, agent_id: &str, chat_id: &str) -> bool {
        let address = ChatAddress::agent(agent_id, chat_id);
        self.request_local_stop_for(&address).await
    }

    pub(crate) async fn collaboration_active_turn(
        &self,
        address: &ChatAddress,
    ) -> Option<ConversationActiveTurn> {
        self.conversation_authority
            .as_ref()?
            .active_turn(address)
            .await
    }

    pub(crate) async fn stop_collaboration_turn(
        &self,
        address: &ChatAddress,
        turn_id: &str,
        authority_epoch: ConversationAuthorityEpoch,
    ) -> Result<Option<Vec<String>>, String> {
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        let gate = authority.transition_gate_at(address);
        let _transition = gate.lock().await;
        let stopped = authority
            .stop_collaboration_locked(
                self.streams.as_ref(),
                CONSOLE_CHANNEL_ID,
                address,
                turn_id,
                authority_epoch,
            )
            .await;
        if stopped.is_some() {
            let scope = ChatScopeStore::resolve(
                self.data_dir
                    .as_ref()
                    .ok_or("Conversation data directory is unavailable")?,
                address.clone(),
            );
            if let Some(agent_id) = rua_session::WorkspaceSessionStore::new(scope.root())
                .load_chat_header(address.chat_id().as_str())
                .await
                .map_err(|error| error.to_string())?
                .and_then(|header| {
                    header
                        .extra
                        .get("selected_agent_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                        .or(Some(header.agent_id))
                })
            {
                self.cancel_acp_turn(&agent_id, address.chat_id().as_str())
                    .await;
            }
        }
        Ok(stopped)
    }

    async fn stop_observed_console_turn(
        &self,
        target: &ConversationControlTarget,
        turn_id: &str,
        authority_epoch: ConversationAuthorityEpoch,
    ) -> Result<bool, String> {
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or("Conversation Authority is unavailable")?;
        let gate = authority.transition_gate_at(&target.address);
        let _transition = gate.lock().await;
        if authority
            .request_stop_matching_locked(
                Some(self.streams.as_ref()),
                CONSOLE_CHANNEL_ID,
                &target.address,
                Some(turn_id),
                Some(authority_epoch),
            )
            .await
            .is_none()
        {
            return Ok(false);
        }
        self.clear_approvals_for_address(&target.address).await;
        self.cancel_acp_turn(
            &target.execution_agent_id,
            target.address.chat_id().as_str(),
        )
        .await;
        Ok(true)
    }

    pub(crate) async fn apply_inbound_chat_delete(
        &self,
        address: &ChatAddress,
    ) -> Result<(), String> {
        let _ = self.request_local_stop_for(address).await;
        if let Some(authority) = self.conversation_authority.as_ref() {
            authority.clear_queue(address).await;
            let _ = authority.finish_active_turn(address).await;
        }
        self.clear_approvals_for_address(address).await;
        rua_tool::extended::ask_user::AskUserService::global()
            .clear_by_session(address.chat_id().as_str())
            .await;
        if let Some(task_runtime) = self.task_runtime.as_ref() {
            task_runtime
                .remove_owner(&rua_task::TaskOwner::from_address(address.clone()))
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    async fn request_local_stop_for(&self, address: &ChatAddress) -> bool {
        let Some(authority) = self.conversation_authority.as_ref() else {
            return false;
        };
        let Some(active_turn) = authority.active_turn(address).await else {
            return authority.pause_queued_inputs(address).await;
        };
        let mut control_events = authority.subscribe_control_events(address).await;
        let stopped = authority
            .request_stop_matching(
                Some(self.streams.as_ref()),
                CONSOLE_CHANNEL_ID,
                address,
                Some(&active_turn.turn_id),
                Some(active_turn.authority_epoch),
            )
            .await;
        if stopped.is_none() {
            return false;
        }
        authority
            .wait_for_turn_handoff(
                &mut control_events,
                address,
                &active_turn.turn_id,
                LOCAL_STOP_CONFIRM_TIMEOUT,
            )
            .await
    }

    pub(crate) async fn clear_approvals_for_address(&self, address: &ChatAddress) {
        if let Some(approval_authority) = self.approval_authority.as_ref() {
            approval_authority.clear_by_address(address).await;
        }
    }

    pub async fn request_local_interjection(
        &self,
        agent_id: &str,
        chat_id: &str,
    ) -> Option<ConversationActiveTurn> {
        self.conversation_authority
            .as_ref()?
            .request_interjection(&rua_core::ChatAddress::agent(agent_id, chat_id))
            .await
    }

    pub async fn submit_delivery(
        &self,
        agent_id: &str,
        mut request: ConsoleChatRequest,
    ) -> ConversationInputDeliveryOutcome {
        let raw_request_chat_id = request
            .chat_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let workspace_dir = match self.registry.get_or_create(agent_id).await {
            Ok(runtime) => Some(runtime.workspace_dir()),
            Err(error) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: format!("workspace unavailable: {error}"),
                    target_device_id: None,
                    failure_kind: None,
                };
            }
        };
        let chat_id = match resolve_chat_id(&raw_request_chat_id) {
            Ok(id) => id,
            Err(err) => {
                tracing::error!(
                    target: "diag.session",
                    agent_id = %agent_id,
                    chat_id = %raw_request_chat_id,
                    error = %err,
                    "[DIAG-session] submit_chat rejected non-UUID chat_id",
                );
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: format!("invalid chat_id: {err}"),
                    target_device_id: None,
                    failure_kind: None,
                };
            }
        };

        let local_device_id = current_sync_device_id(&self.sync_device);
        let address = rua_core::ChatAddress::agent(agent_id, &chat_id);
        let first_submission = if let Some(workspace_dir) = workspace_dir.as_deref() {
            if let Err(detail) =
                crate::services::conversation_authority_target::validate_chat_submit_materialization(
                    workspace_dir,
                    &chat_id,
                    request
                        .extra
                        .get("expected_authority_epoch")
                        .and_then(serde_json::Value::as_str),
                )
                .await
            {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail,
                    target_device_id: None,
                    failure_kind: Some("chat_not_ready".to_string()),
                };
            }
            match crate::services::chat_index::get(workspace_dir, &chat_id).await {
                Ok(indexed) => {
                    indexed.is_none()
                        && rua_session::WorkspaceSessionStore::new(workspace_dir)
                            .load_chat_header(&chat_id)
                            .await
                            .ok()
                            .flatten()
                            .is_none()
                }
                Err(error) => {
                    return ConversationInputDeliveryOutcome::Rejected {
                        detail: format!("chat index unavailable: {error}"),
                        target_device_id: None,
                        failure_kind: None,
                    };
                }
            }
        } else {
            false
        };
        if first_submission {
            if let Some(authority) = self.conversation_authority.as_ref() {
                if authority.active_turn(&address).await.is_some() {
                    return ConversationInputDeliveryOutcome::Rejected {
                        detail:
                            "active turn exists without durable Conversation Authority Placement"
                                .to_string(),
                        target_device_id: None,
                        failure_kind: Some("business".to_string()),
                    };
                }
            }
        }
        let first_submission_placement = if first_submission {
            let Some(local_device_id) = local_device_id.as_deref() else {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: "source Device identity is unavailable".to_string(),
                    target_device_id: None,
                    failure_kind: Some("delivery".to_string()),
                };
            };
            let target_device_id = request
                .extra
                .get("target_device_id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(local_device_id)
                .to_string();
            Some(ConversationAuthorityPlacement {
                device_id: target_device_id.clone(),
                authority_epoch: ConversationAuthorityEpoch::next_for_device(
                    &target_device_id,
                    None,
                ),
                base_manifest_hash:
                    match crate::services::conversation_manifest::initial_conversation_manifest_hash(
                        &address,
                    ) {
                        Ok(hash) => hash,
                        Err(error) => {
                            return ConversationInputDeliveryOutcome::Rejected {
                                detail: error.to_string(),
                                target_device_id: None,
                                failure_kind: None,
                            };
                        }
                    },
            })
        } else {
            None
        };
        if let Some(placement) = first_submission_placement.as_ref() {
            request.extra.insert(
                "target_device_id".to_string(),
                serde_json::json!(placement.device_id),
            );
            request.extra.insert(
                "draft_authority_epoch".to_string(),
                serde_json::json!(placement.authority_epoch),
            );
            request.extra.insert(
                "draft_base_manifest_hash".to_string(),
                serde_json::json!(placement.base_manifest_hash),
            );
        }
        let captured_assignment = if let Some(root) = workspace_dir.as_deref() {
            let header = match rua_session::WorkspaceSessionStore::new(root)
                .load_chat_header(&chat_id)
                .await
            {
                Ok(header) => header,
                Err(error) => {
                    return ConversationInputDeliveryOutcome::Rejected {
                        detail: error.to_string(),
                        target_device_id: None,
                        failure_kind: Some("invalid_request".to_string()),
                    }
                }
            };
            match header.and_then(|header| {
                header
                    .extra
                    .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
                    .cloned()
            }) {
                Some(value) => {
                    match crate::services::product_hooks::ChatHookConfiguration::from_persisted(
                        value,
                    ) {
                        Ok(configuration) => configuration.agent_assignment,
                        Err(error) => {
                            return ConversationInputDeliveryOutcome::Rejected {
                                detail: error,
                                target_device_id: None,
                                failure_kind: Some("invalid_request".to_string()),
                            }
                        }
                    }
                }
                None => None,
            }
        } else {
            None
        };
        let assignment = match match captured_assignment {
            Some(assignment) => Ok(assignment),
            None => self.resolve_agent_turn_assignment(agent_id).await,
        } {
            Ok(assignment) => assignment,
            Err(error) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: error,
                    target_device_id: None,
                    failure_kind: Some("invalid_request".to_string()),
                };
            }
        };
        request.extra.insert(
            rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
            serde_json::to_value(assignment).expect("Turn Agent Assignment is serializable"),
        );
        if !first_submission {
            let scope_root = workspace_dir
                .as_deref()
                .expect("Agent Chat workspace was resolved");
            let scope_store =
                match rua_core::ChatScopeStore::for_agent_workspace(scope_root, address.clone()) {
                    Ok(scope_store) => scope_store,
                    Err(error) => {
                        return ConversationInputDeliveryOutcome::Rejected {
                            detail: error.to_string(),
                            target_device_id: None,
                            failure_kind: None,
                        };
                    }
                };
            let chat_service = rua_chat::ChatService::for_scope_store(scope_store);
            let chat = match chat_service.load(&address).await {
                Ok(Some(chat)) => chat,
                Ok(None) => {
                    return ConversationInputDeliveryOutcome::Rejected {
                        detail: "Chat disappeared after materialization".to_string(),
                        target_device_id: None,
                        failure_kind: None,
                    };
                }
                Err(error) => {
                    return ConversationInputDeliveryOutcome::Rejected {
                        detail: error.to_string(),
                        target_device_id: None,
                        failure_kind: None,
                    };
                }
            };
            let (turn_model, turn_thinking) =
                rua_chat::capture_turn_model_and_thinking(chat.model(), chat.thinking());
            if let Some(turn_model) = turn_model {
                request
                    .extra
                    .insert(rua_chat::TURN_MODEL_KEY.to_string(), turn_model);
            }
            if let Some(turn_thinking) = turn_thinking {
                request
                    .extra
                    .insert(rua_chat::TURN_THINKING_KEY.to_string(), turn_thinking);
            }
        }
        let route = ConversationAuthorityRouter
            .resolve_submit_route(ConversationSubmitRouteRequest {
                workspace_dir: workspace_dir.as_deref(),
                address: &address,
                request: &request,
                execution_agent_id: agent_id,
                local_device_id: local_device_id.as_deref(),
                acp_chat_registry: self.acp_chat_registry.as_ref(),
            })
            .await;

        match route {
            ConversationSubmitRoute::LocalAuthority {
                authority_device_id,
                authority_epoch,
                base_manifest_hash,
                materialize_conversation,
            } => {
                let first_submission = if materialize_conversation {
                    let placement = match first_submission_placement {
                        Some(placement)
                            if placement.device_id == authority_device_id
                                && placement.authority_epoch == authority_epoch
                                && placement.base_manifest_hash == base_manifest_hash =>
                        {
                            placement
                        }
                        _ => {
                            return ConversationInputDeliveryOutcome::Rejected {
                                detail: "draft Agent Chat target changed before delivery"
                                    .to_string(),
                                target_device_id: Some(authority_device_id),
                                failure_kind: Some("business".to_string()),
                            };
                        }
                    };
                    let initial_model = match self
                        .resolve_initial_authority_chat_model(
                            request.extra.get("chat_model_slot").cloned(),
                        )
                        .await
                    {
                        Ok(model) => model,
                        Err(detail) => {
                            return ConversationInputDeliveryOutcome::Rejected {
                                detail,
                                target_device_id: Some(authority_device_id),
                                failure_kind: Some("invalid_request".to_string()),
                            };
                        }
                    };
                    let Some(hooks) = self.product_hooks.as_ref() else {
                        return ConversationInputDeliveryOutcome::Rejected {
                            detail: "Chat configuration service unavailable".to_string(),
                            target_device_id: Some(authority_device_id),
                            failure_kind: Some("business".to_string()),
                        };
                    };
                    let configuration = match async {
                        let collaboration_mode: rua_core::AgentCollaborationMode = request.extra.get("collaboration_mode").cloned()
                            .map(serde_json::from_value).transpose().map_err(|error| error.to_string())?.unwrap_or_default();
                        let assignment = self.resolve_agent_turn_assignment(agent_id).await?;
                        let running = self
                            .authority_running_config_typed(agent_id)
                            .await
                            .map_err(|error| error.to_string())?;
                        hooks
                            .resolve_complete_chat_configuration(
                                &address,
                                self.capture_chat_configuration(
                                    assignment,
                                    rua_config::ChatRunningConfig::from_agent_defaults(&running),
                                    None,
                                    crate::services::chat_configuration_capture::ChatConfigurationSelection {
                                        model: initial_model.clone(),
                                        thinking: None,
                                        collaboration_mode: serde_json::to_value(collaboration_mode).map_err(|error| error.to_string())?,
                                        harness_configuration: request.extra.get("harness_configuration").cloned(),
                                        authority_device_id: placement.device_id.clone(),
                                    },
                                )
                                .await?,
                            )
                            .await
                    }
                    .await
                    {
                        Ok(configuration) => configuration,
                        Err(detail) => {
                            return ConversationInputDeliveryOutcome::Rejected {
                                detail,
                                target_device_id: Some(authority_device_id),
                                failure_kind: Some("invalid_request".to_string()),
                            };
                        }
                    };
                    Some(FirstAgentSubmission {
                        placement,
                        initial_model,
                        configuration,
                    })
                } else {
                    None
                };
                self.deliver_local_authority(
                    agent_id,
                    request,
                    chat_id,
                    workspace_dir,
                    AuthorityRouteTarget {
                        device_id: &authority_device_id,
                        epoch: authority_epoch,
                    },
                    first_submission,
                )
                .await
            }
            ConversationSubmitRoute::RemoteAuthority {
                source_device_id,
                target_device_id,
                authority_epoch,
                base_manifest_hash,
                materialize_conversation,
            } => {
                if !materialize_conversation {
                    return self
                        .deliver_remote_authority(
                            agent_id,
                            &address,
                            &request,
                            workspace_dir,
                            &source_device_id,
                            AuthorityRouteTarget {
                                device_id: &target_device_id,
                                epoch: authority_epoch,
                            },
                        )
                        .await;
                }
                let params = RemoteConversationInputParams {
                    address: &address,
                    execution_agent_id: agent_id,
                    request: &request,
                    source_device_id: &source_device_id,
                    target_device_id: &target_device_id,
                    authority_epoch,
                    base_manifest_hash: &base_manifest_hash,
                    workspace_dir: workspace_dir.as_deref(),
                    materialize_conversation,
                };
                match self
                    .remote_input_delivery
                    .build_remote_conversation_input_request(&params)
                    .await
                {
                    Ok(prepared) => match self
                        .remote_input_delivery
                        .deliver_prepared_conversation_input(&prepared, "remote conversation input")
                        .await
                    {
                        Ok(outcome) => ConversationInputDeliveryOutcome::remote(chat_id, outcome),
                        Err(error) => ConversationInputDeliveryOutcome::Rejected {
                            detail: error.to_string(),
                            target_device_id: Some(target_device_id),
                            failure_kind: Some(error.failure_kind().to_string()),
                        },
                    },
                    Err(detail) => ConversationInputDeliveryOutcome::Rejected {
                        detail,
                        target_device_id: Some(target_device_id),
                        failure_kind: Some("invalid_request".to_string()),
                    },
                }
            }
            ConversationSubmitRoute::Rejected {
                detail,
                target_device_id,
            } => ConversationInputDeliveryOutcome::Rejected {
                detail,
                target_device_id,
                failure_kind: None,
            },
        }
    }

    pub async fn submit_project_delivery(
        &self,
        project_id: &ProjectId,
        execution_agent_id: &str,
        request: ConsoleChatRequest,
    ) -> ConversationInputDeliveryOutcome {
        let Some(data_dir) = self.data_dir.as_ref() else {
            return ConversationInputDeliveryOutcome::Rejected {
                detail: "Project Chat runtime is unavailable".to_string(),
                target_device_id: None,
                failure_kind: Some("business".to_string()),
            };
        };
        let Some(catalog) = self.project_catalog.as_ref() else {
            return ConversationInputDeliveryOutcome::Rejected {
                detail: "Project Catalog is unavailable".to_string(),
                target_device_id: None,
                failure_kind: Some("business".to_string()),
            };
        };
        if catalog.lifecycle_transition_in_progress(project_id) {
            return ConversationInputDeliveryOutcome::Rejected {
                detail: "Project lifecycle transition is in progress".to_string(),
                target_device_id: None,
                failure_kind: Some("business".to_string()),
            };
        }
        let project = match catalog.get(project_id) {
            Ok(Some(project)) if project.lifecycle == rua_project::ProjectLifecycle::Active => {
                project
            }
            Ok(Some(_)) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: "Archived Project cannot accept new work".to_string(),
                    target_device_id: None,
                    failure_kind: Some("business".to_string()),
                };
            }
            Ok(None) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: format!("Project not found: {project_id}"),
                    target_device_id: None,
                    failure_kind: Some("business".to_string()),
                };
            }
            Err(error) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: error.to_string(),
                    target_device_id: None,
                    failure_kind: Some("business".to_string()),
                };
            }
        };
        let Some(raw_chat_id) = request.chat_id.as_deref() else {
            return ConversationInputDeliveryOutcome::Rejected {
                detail: "Project Chat id is required".to_string(),
                target_device_id: Some(project.resource_device_id),
                failure_kind: Some("invalid_request".to_string()),
            };
        };
        let chat_id = match resolve_chat_id(raw_chat_id) {
            Ok(chat_id) => chat_id,
            Err(error) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: error,
                    target_device_id: Some(project.resource_device_id),
                    failure_kind: Some("invalid_request".to_string()),
                };
            }
        };
        if execution_agent_id.trim().is_empty() {
            return ConversationInputDeliveryOutcome::Rejected {
                detail: "execution Agent id is required".to_string(),
                target_device_id: Some(project.resource_device_id),
                failure_kind: Some("invalid_request".to_string()),
            };
        }
        let Some(source_device_id) = current_sync_device_id(&self.sync_device) else {
            return ConversationInputDeliveryOutcome::Rejected {
                detail: "source Device identity is unavailable".to_string(),
                target_device_id: Some(project.resource_device_id),
                failure_kind: Some("delivery".to_string()),
            };
        };

        let address = ChatAddress::project(project_id.clone(), &chat_id);
        let scope_root = ChatScopeStore::resolve(data_dir, address.clone())
            .root()
            .to_path_buf();
        let mut selected_agent_id =
            match crate::services::project_chat::load_project_chat_agent_selection(
                &scope_root,
                &chat_id,
            )
            .await
            {
                Ok(selection) => selection.agent_id,
                Err(_) => execution_agent_id.trim().to_string(),
            };
        let (placement, materialize_conversation) = match crate::services::conversation_authority_target::load_chat_authority_placement_target(
            &scope_root,
            &chat_id,
        )
        .await
        {
            Ok(placement) => {
                if placement.device_id != project.resource_device_id {
                    return ConversationInputDeliveryOutcome::Rejected {
                        detail: "Project Chat authority does not match its resource Device"
                            .to_string(),
                        target_device_id: Some(project.resource_device_id),
                        failure_kind: Some("business".to_string()),
                    };
                }
                (
                    crate::services::conversation_authority_placement::ConversationAuthorityPlacement {
                        device_id: placement.device_id,
                        authority_epoch: placement.authority_epoch,
                        base_manifest_hash: placement.base_manifest_hash,
                    },
                    false,
                )
            }
            Err(_) => {
                let discovered = if project.resource_device_id != source_device_id {
                    match crate::services::conversation_control_dispatcher::dispatch_conversation_control(self, ConversationControlDeviceCommand {
                            command_id: format!(
                                "project-submit-discover-{}",
                                uuid::Uuid::new_v4()
                            ),
                            agent_id: selected_agent_id.clone(),
                            address: address.clone(),
                            source_device_id: source_device_id.clone(),
                            target_device_id: project.resource_device_id.clone(),
                            authority_epoch: ConversationAuthorityEpoch::from_parts(0, 0, [0; 8]),
                            action: ConversationControlDeviceAction::Discover,
                        })
                        .await
                    {
                        Ok(ConversationControlDeviceResponse::View {
                            execution_agent_id: Some(execution_agent_id),
                            authority_placement: Some(placement),
                            ..
                        }) => {
                            if placement.device_id != project.resource_device_id {
                                return ConversationInputDeliveryOutcome::Rejected {
                                    detail: "Project Chat authority does not match its resource Device"
                                        .to_string(),
                                    target_device_id: Some(project.resource_device_id),
                                    failure_kind: Some("business".to_string()),
                                };
                            }
                            if execution_agent_id.trim().is_empty() {
                                return ConversationInputDeliveryOutcome::Rejected {
                                    detail: "Project control discovery returned an empty selected Agent"
                                        .to_string(),
                                    target_device_id: Some(project.resource_device_id),
                                    failure_kind: Some("protocol_error".to_string()),
                                };
                            }
                            Some((execution_agent_id, placement))
                        }
                        Ok(ConversationControlDeviceResponse::View { .. }) => {
                            return ConversationInputDeliveryOutcome::Rejected {
                                detail: "Project control discovery returned incomplete authority state"
                                    .to_string(),
                                target_device_id: Some(project.resource_device_id),
                                failure_kind: Some("protocol_error".to_string()),
                            };
                        }
                        Ok(_) => {
                            return ConversationInputDeliveryOutcome::Rejected {
                                detail:
                                    "Project control discovery returned an unexpected response"
                                        .to_string(),
                                target_device_id: Some(project.resource_device_id),
                                failure_kind: Some("protocol_error".to_string()),
                            };
                        }
                        Err(DeviceCommandDeliveryError::Rejected { reason, .. })
                            if reason == "project_chat_not_found" =>
                        {
                            None
                        }
                        Err(error) => {
                            return ConversationInputDeliveryOutcome::Rejected {
                                detail: error.to_string(),
                                target_device_id: Some(project.resource_device_id),
                                failure_kind: Some(error.failure_kind().to_string()),
                            };
                        }
                    }
                } else {
                    None
                };
                if let Some((execution_agent_id, placement)) = discovered {
                    selected_agent_id = execution_agent_id;
                    (placement, false)
                } else {
                let base_manifest_hash =
                    match crate::services::conversation_manifest::initial_conversation_manifest_hash(
                        &address,
                    ) {
                        Ok(hash) => hash,
                        Err(error) => {
                            return ConversationInputDeliveryOutcome::Rejected {
                                detail: error.to_string(),
                                target_device_id: Some(project.resource_device_id),
                                failure_kind: Some("business".to_string()),
                            };
                        }
                    };
                (
                    crate::services::conversation_authority_placement::ConversationAuthorityPlacement {
                        device_id: project.resource_device_id.clone(),
                        authority_epoch: ConversationAuthorityEpoch::next_for_device(
                            &project.resource_device_id,
                            None,
                        ),
                        base_manifest_hash,
                    },
                    true,
                )
                }
            }
        };
        let route =
            ConversationAuthorityRouter.route_fixed_placement(&placement, Some(&source_device_id));
        match route {
            ConversationSubmitRoute::LocalAuthority {
                authority_device_id,
                authority_epoch,
                ..
            } => {
                self.deliver_local_project_authority(
                    project_id,
                    &selected_agent_id,
                    request,
                    address,
                    scope_root,
                    &source_device_id,
                    ConversationAuthorityPlacement {
                        device_id: authority_device_id,
                        authority_epoch,
                        base_manifest_hash: placement.base_manifest_hash,
                    },
                )
                .await
            }
            ConversationSubmitRoute::RemoteAuthority {
                source_device_id,
                target_device_id,
                authority_epoch,
                ..
            } => {
                let params = RemoteConversationInputParams {
                    address: &address,
                    execution_agent_id: &selected_agent_id,
                    request: &request,
                    source_device_id: &source_device_id,
                    target_device_id: &target_device_id,
                    authority_epoch,
                    base_manifest_hash: &placement.base_manifest_hash,
                    workspace_dir: Some(&scope_root),
                    materialize_conversation,
                };
                let prepared = match self
                    .remote_input_delivery
                    .build_remote_conversation_input_request(&params)
                    .await
                {
                    Ok(request) => request,
                    Err(detail) => {
                        return ConversationInputDeliveryOutcome::Rejected {
                            detail,
                            target_device_id: Some(project.resource_device_id),
                            failure_kind: Some("invalid_request".to_string()),
                        };
                    }
                };
                match self
                    .remote_input_delivery
                    .deliver_prepared_conversation_input(&prepared, "Project Chat input")
                    .await
                {
                    Ok(outcome) => ConversationInputDeliveryOutcome::remote(chat_id, outcome),
                    Err(error) => ConversationInputDeliveryOutcome::Rejected {
                        detail: error.to_string(),
                        target_device_id: Some(project.resource_device_id),
                        failure_kind: Some(error.failure_kind().to_string()),
                    },
                }
            }
            ConversationSubmitRoute::Rejected {
                detail,
                target_device_id,
            } => ConversationInputDeliveryOutcome::Rejected {
                detail,
                target_device_id,
                failure_kind: Some("delivery".to_string()),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn deliver_local_project_authority(
        &self,
        project_id: &ProjectId,
        requested_agent_id: &str,
        mut request: ConsoleChatRequest,
        address: ChatAddress,
        scope_root: PathBuf,
        submit_device_id: &str,
        placement: ConversationAuthorityPlacement,
    ) -> ConversationInputDeliveryOutcome {
        let reject = |detail: String| ConversationInputDeliveryOutcome::Rejected {
            detail,
            target_device_id: Some(placement.device_id.clone()),
            failure_kind: Some("business".to_string()),
        };
        let Some(catalog) = self.project_catalog.as_ref() else {
            return reject("Project catalog unavailable".into());
        };
        let _admission = match catalog.begin_admission(project_id) {
            Ok(guard) => guard,
            Err(error) => return reject(error.to_string()),
        };
        let Some(bindings) = self.project_bindings.as_ref() else {
            return reject("Project Resource Binding Store is unavailable".to_string());
        };
        let binding = match bindings.get(project_id) {
            Ok(Some(binding)) => binding,
            Ok(None) => {
                return reject(format!("Project Resource Binding not found: {project_id}"));
            }
            Err(error) => return reject(error.to_string()),
        };
        if binding.resource_device_id != placement.device_id {
            return reject("Project Chat authority does not match its resource Device".to_string());
        }
        let _project = match self
            .project_catalog
            .as_ref()
            .ok_or_else(|| "Project Catalog is unavailable".to_string())
            .and_then(|catalog| catalog.get(project_id).map_err(|error| error.to_string()))
        {
            Ok(Some(project)) => project,
            Ok(None) => return reject(format!("Project not found: {project_id}")),
            Err(error) => return reject(error),
        };
        let indexed =
            match crate::services::chat_index::get(&scope_root, address.chat_id().as_str()).await {
                Ok(indexed) => indexed,
                Err(error) => return reject(error),
            };
        let header = match rua_session::WorkspaceSessionStore::new(&scope_root)
            .load_chat_header(address.chat_id().as_str())
            .await
        {
            Ok(header) => header,
            Err(error) => return reject(error.to_string()),
        };
        let first_submission = indexed.is_none() && header.is_none();
        let directory_key = crate::services::project_chat::PROJECT_WORKING_DIRECTORY_META_KEY;
        let directory_id = header
            .as_ref()
            .and_then(|header| header.extra.get(directory_key))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(if first_submission {
                request
                    .extra
                    .get(directory_key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&binding.resources.primary_directory_id)
            } else {
                rua_project::INITIAL_PROJECT_DIRECTORY_ID
            })
            .to_string();
        request
            .extra
            .insert(directory_key.into(), serde_json::json!(directory_id));
        let working_dir = match crate::services::project_chat::working_directory(
            &binding,
            header.as_ref(),
            Some(&directory_id),
        ) {
            Ok(path) => path,
            Err(error) => return reject(error),
        };

        if indexed.is_none() && header.is_some() {
            return reject(
                "unindexed Project Chat storage does not match this submission".to_string(),
            );
        }

        let initial_model = if first_submission {
            match self
                .resolve_initial_authority_chat_model(request.extra.get("chat_model_slot").cloned())
                .await
            {
                Ok(model) => model,
                Err(error) => return reject(error),
            }
        } else {
            None
        };
        let selected_agent_id = if first_submission {
            requested_agent_id.to_string()
        } else {
            let Some(header) = header.as_ref() else {
                return reject("Project Chat header is unavailable".to_string());
            };
            match crate::services::project_chat::selection_from_header(header) {
                Ok(selection) => selection.agent_id,
                Err(error) => return reject(error),
            }
        };
        if !first_submission && requested_agent_id != selected_agent_id {
            return reject(format!(
                "stale Project Chat Agent Selection: requested '{}', current '{}'",
                requested_agent_id, selected_agent_id
            ));
        }
        if let Err(error) = self.resolve_project_turn_agent(&selected_agent_id).await {
            return reject(error);
        }
        let captured_assignment = match self
            .load_chat_configuration(&address, &selected_agent_id, &scope_root)
            .await
        {
            Ok(configuration) => configuration.and_then(|config| config.agent_assignment),
            Err(error) => return reject(error),
        };
        let assignment = match match captured_assignment {
            Some(assignment) => Ok(assignment),
            None => self.resolve_project_turn_agent(&selected_agent_id).await,
        } {
            Ok(assignment) => assignment,
            Err(error) => return reject(error),
        };
        if let Err(error) = self.registry.get_or_create(&selected_agent_id).await {
            return reject(format!("workspace unavailable: {error}"));
        }
        request.extra.insert(
            rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
            match serde_json::to_value(&assignment) {
                Ok(value) => value,
                Err(error) => return reject(error.to_string()),
            },
        );
        let (turn_model, turn_thinking) = if first_submission {
            rua_chat::capture_turn_model_and_thinking(initial_model.as_ref(), None)
        } else {
            let current_chat =
                match rua_chat::ChatService::for_resolved_scope(&scope_root, address.clone())
                    .load(&address)
                    .await
                {
                    Ok(Some(chat)) => chat,
                    Ok(None) => {
                        return reject("Project Chat Session File is unavailable".to_string())
                    }
                    Err(error) => return reject(error.to_string()),
                };
            rua_chat::capture_turn_model_and_thinking(current_chat.model(), current_chat.thinking())
        };
        if let Some(turn_model) = turn_model {
            request
                .extra
                .insert(rua_chat::TURN_MODEL_KEY.to_string(), turn_model);
        }
        if let Some(turn_thinking) = turn_thinking {
            request
                .extra
                .insert(rua_chat::TURN_THINKING_KEY.to_string(), turn_thinking);
        }

        let Some(authority) = self.conversation_authority.as_ref() else {
            return reject("Conversation Authority is unavailable".to_string());
        };
        let key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
        let (driver, driver_detail) = self
            .conversation_driver_for(
                &selected_agent_id,
                address.chat_id().as_str(),
                Some(scope_root.as_path()),
            )
            .await;
        let start = ConversationActiveTurnStart {
            address: address.clone(),
            driver,
            authority_device_id: placement.device_id.clone(),
            authority_epoch: placement.authority_epoch,
            driver_detail,
        };
        let mut pending = build_authority_pending_message(
            &request,
            &placement.device_id,
            placement.authority_epoch,
        );
        if first_submission {
            if let Err(error) = self.validate_initial_acp(
                &selected_agent_id,
                &address,
                &working_dir,
                &pending.extra,
            ) {
                return reject(error);
            }
        } else {
            pending.extra.remove(INITIAL_ACP_KEY);
        }
        if let Err(error) = self
            .attach_chat_configuration(&scope_root, &address, &selected_agent_id, &mut pending)
            .await
        {
            return reject(error);
        }
        if pending
            .extra
            .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
            .is_none()
        {
            if let Some(product_hooks) = self.product_hooks.as_ref() {
                let running = match self
                    .authority_running_config_typed(&assignment.agent_id)
                    .await
                {
                    Ok(running) => rua_config::ChatRunningConfig::from_agent_defaults(&running),
                    Err(error) => return reject(error.to_string()),
                };
                let materialization = match self
                    .capture_chat_configuration(
                        assignment.clone(),
                        running,
                        Some(&working_dir),
                        crate::services::chat_configuration_capture::ChatConfigurationSelection {
                            model: initial_model.clone(),
                            thinking: request.extra.get(rua_chat::TURN_THINKING_KEY).cloned(),
                            collaboration_mode: request
                                .extra
                                .get("collaboration_mode")
                                .or_else(|| {
                                    request
                                        .extra
                                        .get(rua_core::TURN_COLLABORATION_MODE_OVERRIDE_KEY)
                                })
                                .cloned()
                                .unwrap_or_else(|| {
                                    serde_json::json!(rua_core::AgentCollaborationMode::default())
                                }),
                            harness_configuration: request
                                .extra
                                .get("harness_configuration")
                                .cloned(),
                            authority_device_id: placement.device_id.clone(),
                        },
                    )
                    .await
                {
                    Ok(configuration) => configuration,
                    Err(error) => return reject(error),
                };
                let configuration = match product_hooks
                    .resolve_complete_chat_configuration(&address, materialization)
                    .await
                {
                    Ok(configuration) => configuration,
                    Err(error) => return reject(error),
                };
                pending.extra.insert(
                    crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY.to_string(),
                    serde_json::to_value(configuration)
                        .expect("Chat Hook Configuration serializes"),
                );
            }
        }
        if first_submission {
            pending.extra.insert(
                crate::services::conversation_authority::FIRST_CHAT_SUBMISSION_CANDIDATE_KEY
                    .to_string(),
                serde_json::Value::Bool(true),
            );
        }
        let persist = if first_submission {
            let scope_root_for_persist = scope_root.clone();
            let address_for_persist = address.clone();
            let placement_for_persist = placement.clone();
            Some(Box::new(
                move |pending,
                      prepared|
                      -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<(), String>> + Send>,
                > {
                    Box::pin(async move {
                        crate::services::project_chat::persist_first_project_submission(
                            &scope_root_for_persist,
                            &address_for_persist,
                            &assignment,
                            &pending,
                            &prepared,
                            &placement_for_persist,
                            crate::services::project_chat::ProjectChatMaterializationSnapshot {
                                initial_model,
                                configuration: crate::services::product_hooks::ChatHookConfiguration::from_persisted(pending.extra[crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY].clone())?,
                            },
                        )
                        .await
                    })
                },
            ) as ConversationSubmissionPersistCallback)
        } else {
            None
        };
        let acceptance = match authority
            .accept_submission(
                self.streams.as_ref(),
                &key,
                start,
                pending,
                Some(submit_device_id.to_string()),
                persist,
            )
            .await
        {
            Ok(acceptance) => acceptance,
            Err(error) => return reject(error),
        };
        let outcome = self
            .finish_local_submission(
                acceptance,
                key,
                ConversationExecutionContext {
                    address: address.clone(),
                    execution_agent_id: selected_agent_id,
                    scope_root: scope_root.clone(),
                    working_dir: working_dir.clone(),
                },
                truncate_chat_name(&request.message),
                true,
                true,
            )
            .await;
        if first_submission {
            if let Ok(Some(entry)) =
                crate::services::chat_index::get(&scope_root, address.chat_id().as_str()).await
            {
                self.session_sync
                    .emit_chat_index_upsert_for(
                        &address,
                        crate::services::console_chat::chat_index_entry_to_json(&entry),
                    )
                    .await;
            }
            self.session_sync
                .emit_evergreen_chat_snapshot(&scope_root, &address)
                .await;
        }
        outcome
    }

    pub async fn send_now(
        &self,
        agent_id: &str,
        chat_id: &str,
        input_id: &str,
        authority_epoch: ConversationAuthorityEpoch,
        command_id: &str,
    ) -> std::result::Result<ConversationInputEntrySnapshot, String> {
        let runtime = self
            .registry
            .get_or_create(agent_id)
            .await
            .map_err(|error| format!("workspace unavailable: {error}"))?;
        let workspace_dir = runtime.workspace_dir();
        let placement =
            crate::services::conversation_authority_target::load_chat_authority_placement_target(
                &workspace_dir,
                chat_id,
            )
            .await?;
        let target = ConversationControlTarget {
            address: ChatAddress::agent(agent_id, chat_id),
            execution_agent_id: agent_id.to_string(),
            scope_root: runtime.workspace_dir(),
            working_dir: runtime.workspace_dir(),
            placement: ConversationAuthorityPlacement {
                device_id: placement.device_id,
                authority_epoch: placement.authority_epoch,
                base_manifest_hash: placement.base_manifest_hash,
            },
        };
        self.send_now_for(&target, input_id, authority_epoch, command_id)
            .await
    }

    async fn send_now_for(
        &self,
        target: &ConversationControlTarget,
        input_id: &str,
        authority_epoch: ConversationAuthorityEpoch,
        command_id: &str,
    ) -> std::result::Result<ConversationInputEntrySnapshot, String> {
        let (driver, driver_detail) = self
            .conversation_driver_for(
                &target.execution_agent_id,
                target.address.chat_id().as_str(),
                Some(target.scope_root.as_path()),
            )
            .await;
        if driver == ConversationDriverKind::Acp {
            self.cancel_acp_turn(
                &target.execution_agent_id,
                target.address.chat_id().as_str(),
            )
            .await;
        }
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        let key = StreamKey::new(CONSOLE_CHANNEL_ID, target.address.clone(), Priority::Normal);
        match authority
            .request_send_now(
                self.streams.as_ref(),
                CONSOLE_CHANNEL_ID,
                &target.address,
                input_id,
                authority_epoch,
                command_id,
                ConversationActiveTurnStart {
                    address: target.address.clone(),
                    driver,
                    authority_device_id: target.placement.device_id.clone(),
                    authority_epoch: target.placement.authority_epoch,
                    driver_detail,
                },
            )
            .await?
        {
            ConversationSendNowOutcome::Started {
                input,
                producer_id,
                cancel_rx,
            } => {
                self.spawn_console_producer(
                    key,
                    producer_id,
                    cancel_rx,
                    ConversationExecutionContext {
                        address: target.address.clone(),
                        execution_agent_id: target.execution_agent_id.clone(),
                        scope_root: target.scope_root.clone(),
                        working_dir: target.working_dir.clone(),
                    },
                );
                Ok(input)
            }
            ConversationSendNowOutcome::InterjectionRequested { input }
            | ConversationSendNowOutcome::AlreadyApplied { input } => Ok(input),
        }
    }

    pub async fn observe_authority_redirect(
        &self,
        requested_epoch: ConversationAuthorityEpoch,
        redirect: &ConversationAuthorityRedirect,
    ) -> Result<
        crate::services::conversation_authority_placement::ConversationAuthorityPlacement,
        String,
    > {
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        verify_and_observe_authority_redirect(
            &self.registry,
            authority,
            self.remote_input_delivery.device_network(),
            &self.sync_device,
            requested_epoch,
            redirect,
        )
        .await
    }

    pub async fn enter_authority_acp(
        &self,
        agent_id: &str,
        chat_id: &str,
        binding: AcpBindingMetadata,
    ) -> Result<ChatMetadataRecord, String> {
        let runtime = self
            .registry
            .get_or_create(agent_id)
            .await
            .map_err(|error| error.to_string())?;
        self.enter_authority_acp_for(
            &ChatAddress::agent(agent_id, chat_id),
            agent_id,
            &runtime.workspace_dir(),
            binding,
        )
        .await
    }

    pub(crate) async fn enter_authority_acp_for(
        &self,
        address: &ChatAddress,
        agent_id: &str,
        scope_root: &Path,
        binding: AcpBindingMetadata,
    ) -> Result<ChatMetadataRecord, String> {
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        let record = authority
            .enter_acp(None, scope_root, address, agent_id, binding)
            .await
            .map_err(|error| error.to_string())?;
        self.publish_authority_chat_metadata_for(scope_root, address, &record)
            .await;
        Ok(record)
    }

    pub(crate) fn validate_acp_project_resources(
        &self,
        project_id: &ProjectId,
    ) -> Result<(), String> {
        let binding = self
            .project_bindings
            .as_ref()
            .ok_or("Project bindings unavailable")?
            .get(project_id)
            .map_err(|error| error.to_string())?
            .ok_or("Project binding missing")?;
        if binding.resources.directories.len() > 1 {
            return Err("ACP runners currently support one Project resource directory; choose the built-in Agent for this multi-directory Project".into());
        }
        Ok(())
    }

    fn validate_initial_acp(
        &self,
        agent_id: &str,
        address: &ChatAddress,
        working_dir: &Path,
        extra: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Option<InitialAcpSelection>, String> {
        let Some(value) = extra.get(INITIAL_ACP_KEY) else {
            return Ok(None);
        };
        if let Some(project_id) = address.project_id() {
            self.validate_acp_project_resources(project_id)?;
        }
        let selection: InitialAcpSelection = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid initial ACP selection: {error}"))?;
        if selection.runner.trim().is_empty() {
            return Err("initial ACP runner is required".into());
        }
        let handle = self
            .acp_chat_registry
            .as_ref()
            .and_then(|registry| registry.get(agent_id))
            .ok_or("ACP chat runtime is unavailable")?;
        handle.refresh_config();
        let config = handle
            .service
            .agent_config(&selection.runner)
            .ok_or_else(|| format!("ACP runner '{}' is not configured", selection.runner))?;
        if !config.enabled {
            return Err(format!("ACP runner '{}' is disabled", selection.runner));
        }
        if selection
            .cwd
            .as_ref()
            .is_some_and(|cwd| cwd.trim().is_empty())
            || selection
                .resume_session_id
                .as_ref()
                .is_some_and(|id| id.trim().is_empty())
        {
            return Err("initial ACP cwd and resume Session must not be empty".into());
        }
        let cwd = if address.project_id().is_some() {
            working_dir
        } else {
            selection
                .cwd
                .as_deref()
                .map(Path::new)
                .unwrap_or(working_dir)
        };
        if !cwd.is_dir() {
            return Err(format!(
                "initial ACP working directory does not exist: {}",
                cwd.display()
            ));
        }
        Ok(Some(selection))
    }

    pub(crate) async fn enter_collaboration_acp(
        &self,
        address: &ChatAddress,
        execution_agent_id: &str,
        scope_root: &Path,
        working_dir: &Path,
        runner: &str,
    ) -> Result<(), String> {
        let _resource_admission = self.project_work_admission(address)?;
        if let Some(project_id) = address.project_id() {
            self.validate_acp_project_resources(project_id)?;
        }
        let registry = self
            .acp_chat_registry
            .as_ref()
            .ok_or("ACP chat runtime is unavailable")?;
        let handle = registry.get(execution_agent_id).ok_or_else(|| {
            format!("ACP chat runtime for Agent '{execution_agent_id}' is unavailable")
        })?;
        handle.refresh_config();
        let config = handle
            .service
            .agent_config(runner)
            .ok_or_else(|| format!("ACP runner '{runner}' is unavailable"))?;
        if !config.enabled {
            return Err(format!("ACP runner '{runner}' is disabled"));
        }
        let chat_id = address.chat_id().to_string();
        let runtime_session_id = rua_core::id::SessionId::new(chat_id.clone());
        if let Some(session) = handle.manager.get(&chat_id).await {
            if session.runner != runner {
                return Err("Collaboration ACP runner is fixed; create another Chat".into());
            }
            if !handle
                .service
                .chat_session_exists(&runtime_session_id, runner)
                .await
            {
                let _ = handle.controller.exit(&chat_id).await;
                handle.service.bind_chat_address(&chat_id, address.clone());
                let cwd = working_dir.to_string_lossy().to_string();
                let message = handle.controller.enter(&chat_id, runner, Some(&cwd)).await;
                if handle.manager.get(&chat_id).await.is_none() {
                    return Err(format!(
                        "ACP runner '{runner}' failed to restart: {message}"
                    ));
                }
            }
        } else {
            handle.service.bind_chat_address(&chat_id, address.clone());
            let cwd = working_dir.to_string_lossy().to_string();
            let message = handle.controller.enter(&chat_id, runner, Some(&cwd)).await;
            let Some(session) = handle.manager.get(&chat_id).await else {
                return Err(format!("ACP runner '{runner}' failed to start: {message}"));
            };
            if session.runner != runner {
                let _ = handle.controller.exit(&chat_id).await;
                return Err("ACP runtime started a different runner".into());
            }
        }
        let session = handle
            .manager
            .get(&chat_id)
            .await
            .ok_or("ACP session disappeared before binding")?;
        if let Err(error) = self
            .enter_authority_acp_for(
                address,
                execution_agent_id,
                scope_root,
                AcpBindingMetadata {
                    target_device_id: Some(
                        crate::services::device_context::current_sync_device_id(&self.sync_device)
                            .ok_or("local Device identity is unavailable")?,
                    ),
                    runner: runner.to_string(),
                    cwd: Some(session.cwd.to_string_lossy().to_string()),
                    runner_session_id: session.runner_session_id,
                },
            )
            .await
        {
            let _ = handle.controller.exit(&chat_id).await;
            return Err(error);
        }
        Ok(())
    }

    pub async fn exit_authority_acp(
        &self,
        agent_id: &str,
        chat_id: &str,
    ) -> Result<Option<ChatMetadataRecord>, String> {
        let runtime = self
            .registry
            .get_or_create(agent_id)
            .await
            .map_err(|error| error.to_string())?;
        self.exit_authority_acp_for(
            &ChatAddress::agent(agent_id, chat_id),
            agent_id,
            &runtime.workspace_dir(),
        )
        .await
    }

    async fn exit_authority_acp_for(
        &self,
        address: &ChatAddress,
        agent_id: &str,
        scope_root: &Path,
    ) -> Result<Option<ChatMetadataRecord>, String> {
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        let record = authority
            .exit_acp(None, scope_root, address, agent_id)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(record) = record.as_ref() {
            self.publish_authority_chat_metadata_for(scope_root, address, record)
                .await;
        }
        Ok(record)
    }

    async fn publish_authority_chat_metadata_for(
        &self,
        scope_root: &Path,
        address: &ChatAddress,
        record: &ChatMetadataRecord,
    ) {
        self.session_sync
            .emit_chat_index_upsert_for(address, record.to_json_value())
            .await;
        self.session_sync
            .emit_evergreen_chat_snapshot(scope_root, address)
            .await;
    }

    pub async fn update_authority_chat_model(
        &self,
        agent_id: &str,
        chat_id: &str,
        model: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let runtime = self
            .registry
            .get_or_create(agent_id)
            .await
            .map_err(|error| error.to_string())?;
        let scope_root = runtime.workspace_dir();
        self.update_authority_chat_model_for(
            &ChatAddress::agent(agent_id, chat_id),
            agent_id,
            &scope_root,
            model,
        )
        .await
    }

    async fn update_authority_chat_model_for(
        &self,
        address: &ChatAddress,
        agent_id: &str,
        scope_root: &Path,
        model: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let model = self.normalize_authority_chat_model(model).await?;
        ChatConfigurationUpdate::begin(self, address, agent_id, scope_root)
            .await?
            .update_model(model)
            .await
    }

    pub async fn authority_chat_model(
        &self,
        agent_id: &str,
        chat_id: &str,
    ) -> Result<serde_json::Value, String> {
        let runtime = self
            .registry
            .get_or_create(agent_id)
            .await
            .map_err(|error| error.to_string())?;
        self.authority_chat_model_for(&runtime.workspace_dir(), chat_id)
            .await
    }

    pub(crate) async fn authority_chat_model_for(
        &self,
        scope_root: &Path,
        chat_id: &str,
    ) -> Result<serde_json::Value, String> {
        let header = rua_session::WorkspaceSessionStore::new(scope_root)
            .load_chat_header(chat_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "target chat header is unavailable".to_string())?;
        rua_chat::model_slot_from_header(&header)
            .ok_or_else(|| "target Chat has no selected model".to_string())
    }

    pub async fn resolve_initial_authority_chat_model(
        &self,
        requested_model: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, String> {
        match requested_model {
            Some(model) => self.normalize_authority_chat_model(model).await.map(Some),
            None => Ok(self.default_authority_chat_model().await),
        }
    }

    async fn default_authority_chat_model(&self) -> Option<serde_json::Value> {
        let default_model = self.provider_manager.as_ref()?.default_model().await?;
        if default_model.provider_id.trim().is_empty() || default_model.model.trim().is_empty() {
            return None;
        }
        let config = match self.provider_cells.as_ref() {
            Some(provider_cells) => provider_cells
                .cell(&default_model.provider_id)
                .read()
                .await
                .ok()
                .flatten(),
            None => None,
        };
        let entry = config.as_ref().and_then(|config| {
            config
                .all_models()
                .into_iter()
                .find(|entry| entry.id == default_model.model)
        });
        let knowledge =
            rua_provider::model_knowledge::lookup(&default_model.provider_id, &default_model.model);
        let thinking_param_style = entry
            .as_ref()
            .and_then(|entry| entry.thinking_param_style.clone())
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|config| config.thinking_param_style.clone())
            })
            .or_else(|| {
                knowledge.and_then(|knowledge| {
                    knowledge.reasoning_style.map(|style| match style {
                        rua_provider::model_knowledge::ReasoningStyle::Effort => {
                            "effort".to_string()
                        }
                        rua_provider::model_knowledge::ReasoningStyle::Budget => {
                            "budget".to_string()
                        }
                    })
                })
            })
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|config| config.protocol_thinking_param_style().map(str::to_string))
            });
        let thinking_budget_range = entry
            .as_ref()
            .and_then(|entry| entry.thinking_budget_range)
            .or_else(|| {
                config
                    .as_ref()
                    .and_then(|config| config.thinking_budget_range)
            })
            .or_else(|| knowledge.and_then(|knowledge| knowledge.thinking_budget_range));
        let reasoning_effort_options = entry
            .as_ref()
            .map(|entry| entry.reasoning_effort_options.clone())
            .filter(|options| !options.is_empty())
            .or_else(|| {
                config.as_ref().and_then(|config| {
                    (!config.reasoning_effort_options.is_empty())
                        .then(|| config.reasoning_effort_options.clone())
                })
            })
            .or_else(|| {
                knowledge.and_then(|knowledge| {
                    (!knowledge.reasoning_effort_options.is_empty()).then(|| {
                        knowledge
                            .reasoning_effort_options
                            .iter()
                            .map(|option| (*option).to_string())
                            .collect()
                    })
                })
            })
            .unwrap_or_default();
        Some(serde_json::json!({
            "provider_id": default_model.provider_id,
            "model": default_model.model,
            "thinking_strength": "auto",
            "thinking_enabled": null,
            "thinking_budget": null,
            "reasoning_effort": null,
            "thinking_param_style": thinking_param_style,
            "thinking_budget_range": thinking_budget_range,
            "reasoning_effort_options": reasoning_effort_options,
        }))
    }

    pub async fn normalize_authority_chat_model(
        &self,
        model: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.validate_authority_chat_model(&model)?;
        let mut slot = model
            .as_object()
            .cloned()
            .ok_or_else(|| "chat model must be an object".to_string())?;
        let provider_id = slot
            .get("provider_id")
            .and_then(serde_json::Value::as_str)
            .expect("validated provider_id");
        let model_id = slot
            .get("model")
            .and_then(serde_json::Value::as_str)
            .expect("validated model id");
        let provider_cells = self
            .provider_cells
            .as_ref()
            .ok_or_else(|| "target Provider configuration is unavailable".to_string())?;
        let config = provider_cells
            .cell(provider_id)
            .read()
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!("provider '{provider_id}' is not configured on target Device")
            })?;
        let entry = config
            .all_models()
            .into_iter()
            .find(|entry| entry.id == model_id);
        let knowledge = rua_provider::model_knowledge::lookup(provider_id, model_id);

        let mut style = entry
            .as_ref()
            .and_then(|entry| entry.thinking_param_style.clone())
            .or_else(|| config.thinking_param_style.clone())
            .or_else(|| {
                knowledge.and_then(|knowledge| {
                    knowledge.reasoning_style.map(|style| match style {
                        rua_provider::model_knowledge::ReasoningStyle::Effort => {
                            "effort".to_string()
                        }
                        rua_provider::model_knowledge::ReasoningStyle::Budget => {
                            "budget".to_string()
                        }
                    })
                })
            })
            .or_else(|| config.protocol_thinking_param_style().map(str::to_string));
        let budget_range = entry
            .as_ref()
            .and_then(|entry| entry.thinking_budget_range)
            .or(config.thinking_budget_range)
            .or_else(|| knowledge.and_then(|knowledge| knowledge.thinking_budget_range));
        let effort_options = entry
            .as_ref()
            .map(|entry| entry.reasoning_effort_options.clone())
            .filter(|options| !options.is_empty())
            .or_else(|| {
                (!config.reasoning_effort_options.is_empty())
                    .then(|| config.reasoning_effort_options.clone())
            })
            .or_else(|| {
                knowledge.and_then(|knowledge| {
                    (!knowledge.reasoning_effort_options.is_empty()).then(|| {
                        knowledge
                            .reasoning_effort_options
                            .iter()
                            .map(|option| (*option).to_string())
                            .collect()
                    })
                })
            })
            .unwrap_or_default();
        if style.is_none() {
            style = if !effort_options.is_empty() {
                Some("effort".to_string())
            } else if budget_range.is_some() {
                Some("budget".to_string())
            } else {
                None
            };
        }

        let strength = slot
            .get("thinking_strength")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                if slot
                    .get("thinking_enabled")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                {
                    Some("off".to_string())
                } else {
                    slot.get("reasoning_effort")
                        .and_then(serde_json::Value::as_str)
                        .filter(|value| matches!(*value, "low" | "medium" | "high"))
                        .map(str::to_string)
                }
            })
            .unwrap_or_else(|| "auto".to_string());
        if !matches!(
            strength.as_str(),
            "off" | "auto" | "low" | "medium" | "high"
        ) {
            return Err(format!("unsupported thinking_strength '{strength}'"));
        }
        if matches!(strength.as_str(), "low" | "medium" | "high") {
            match style.as_deref() {
                Some("effort") => {
                    let supported = effort_options.is_empty()
                        || match strength.as_str() {
                            "high" => effort_options
                                .iter()
                                .any(|value| matches!(value.as_str(), "high" | "xhigh" | "max")),
                            value => effort_options.iter().any(|option| option == value),
                        };
                    if !supported {
                        return Err(format!(
                            "thinking strength '{strength}' is not supported by target model '{provider_id}/{model_id}'"
                        ));
                    }
                }
                Some("budget") => {
                    if !budget_range.is_some_and(|[min, max]| min >= 0 && max > 0 && min <= max) {
                        return Err(format!(
                            "target model '{provider_id}/{model_id}' has no valid thinking budget range"
                        ));
                    }
                }
                _ => {
                    return Err(format!(
                        "target model '{provider_id}/{model_id}' does not support Reasoning Control"
                    ));
                }
            }
        }

        match style {
            Some(style) => {
                slot.insert(
                    "thinking_param_style".to_string(),
                    serde_json::Value::String(style),
                );
            }
            None => {
                slot.remove("thinking_param_style");
            }
        }
        match budget_range {
            Some(range) => {
                slot.insert(
                    "thinking_budget_range".to_string(),
                    serde_json::json!(range),
                );
            }
            None => {
                slot.remove("thinking_budget_range");
            }
        }
        if effort_options.is_empty() {
            slot.remove("reasoning_effort_options");
        } else {
            slot.insert(
                "reasoning_effort_options".to_string(),
                serde_json::json!(effort_options),
            );
        }
        slot.insert(
            "thinking_strength".to_string(),
            serde_json::Value::String(strength),
        );
        slot.remove("thinking_enabled");
        slot.remove("thinking_budget");
        slot.remove("reasoning_effort");
        Ok(serde_json::Value::Object(slot))
    }

    pub fn validate_authority_chat_model(&self, model: &serde_json::Value) -> Result<(), String> {
        let model = model
            .as_object()
            .ok_or_else(|| "chat model must be an object".to_string())?;
        let provider_id = model
            .get("provider_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "chat model provider_id is required".to_string())?;
        let model_id = model
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "chat model id is required".to_string())?;
        let provider_manager = self
            .provider_manager
            .as_ref()
            .ok_or_else(|| "target provider manager is unavailable".to_string())?;
        let provider = provider_manager
            .get(provider_id)
            .ok_or_else(|| format!("provider '{provider_id}' is not available on target Device"))?;
        if !provider.supports_model(model_id) {
            return Err(format!(
                "model '{model_id}' is not available for provider '{provider_id}' on target Device"
            ));
        }
        provider.create_chat_model(model_id).map_err(|error| {
            format!("target model '{provider_id}/{model_id}' is invalid: {error}")
        })?;
        Ok(())
    }

    pub async fn validate_authority_adoption_model(
        &self,
        agent_id: &str,
        chat_id: &str,
    ) -> Result<(), String> {
        let runtime = self
            .registry
            .get_or_create(agent_id)
            .await
            .map_err(|error| error.to_string())?;
        let header = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir())
            .load_chat_header(chat_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "target chat header is unavailable".to_string())?;
        let model = rua_chat::model_slot_from_header(&header)
            .ok_or_else(|| "target Chat has no selected model".to_string())?;
        self.normalize_authority_chat_model(model).await.map(|_| ())
    }

    pub async fn authority_running_config(
        &self,
        agent_id: &str,
    ) -> rua_core::Result<serde_json::Value> {
        let agent_service =
            self.agent_service
                .as_ref()
                .ok_or_else(|| rua_core::RuaError::Runtime {
                    message: "target Agent service is unavailable".to_string(),
                })?;
        let detail = agent_service.get_agent(agent_id).await?.ok_or_else(|| {
            rua_core::RuaError::NotFound {
                resource: "agent".to_string(),
                id: agent_id.to_string(),
            }
        })?;
        Ok(detail.running.unwrap_or_else(|| {
            serde_json::to_value(rua_config::AgentsRunningConfig::default()).unwrap_or_default()
        }))
    }

    async fn authority_running_config_typed(
        &self,
        agent_id: &str,
    ) -> rua_core::Result<rua_config::AgentsRunningConfig> {
        serde_json::from_value(self.authority_running_config(agent_id).await?).map_err(|error| {
            rua_core::RuaError::Validation {
                message: format!("invalid Agent Running Config: {error}"),
            }
        })
    }

    pub async fn authority_chat_running_config(
        &self,
        address: &ChatAddress,
        agent_id: &str,
        scope_root: &Path,
    ) -> rua_core::Result<rua_config::ChatRunningConfig> {
        self.load_chat_configuration(address, agent_id, scope_root)
            .await
            .map_err(|message| rua_core::RuaError::Validation { message })?
            .map(|configuration| configuration.running_config)
            .ok_or_else(|| rua_core::RuaError::NotFound {
                resource: "chat".to_string(),
                id: address.chat_id().to_string(),
            })
    }

    async fn load_chat_configuration(
        &self,
        address: &ChatAddress,
        agent_id: &str,
        scope_root: &Path,
    ) -> Result<Option<crate::services::product_hooks::ChatHookConfiguration>, String> {
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or("Conversation Authority unavailable")?;
        let gate = authority.transition_gate(address);
        let _transition = gate.lock().await;
        self.load_chat_configuration_locked(address, agent_id, scope_root)
            .await
    }

    async fn attach_chat_configuration(
        &self,
        scope_root: &Path,
        address: &ChatAddress,
        agent_id: &str,
        pending: &mut ConversationInputPayload,
    ) -> Result<(), String> {
        if let Some(service) = self
            .collaboration
            .as_ref()
            .and_then(|binding| binding.get())
            .and_then(std::sync::Weak::upgrade)
        {
            service.ensure_configuration_ready(address).await?;
        }
        let key = crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY;
        pending.extra.remove(key);
        if let Some(configuration) = self
            .load_chat_configuration(address, agent_id, scope_root)
            .await?
        {
            pending.extra.insert(
                key.to_string(),
                serde_json::to_value(configuration).map_err(|error| error.to_string())?,
            );
        }
        Ok(())
    }

    async fn load_chat_configuration_locked(
        &self,
        address: &ChatAddress,
        agent_id: &str,
        scope_root: &Path,
    ) -> Result<Option<crate::services::product_hooks::ChatHookConfiguration>, String> {
        use crate::services::product_hooks::{
            ChatHookConfiguration, CHAT_HOOK_CONFIGURATION_META_KEY,
        };
        let store = rua_session::WorkspaceSessionStore::new(scope_root);
        let Some(header) = store
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        let value = header
            .extra
            .get(CHAT_HOOK_CONFIGURATION_META_KEY)
            .ok_or("Chat configuration snapshot is unavailable")?;
        let configuration = ChatHookConfiguration::from_persisted(value.clone())?;
        if configuration.selected_agent_id != agent_id {
            return Err("Chat configuration belongs to a different Agent".into());
        }
        Ok(Some(configuration))
    }

    async fn chat_configuration_materialization(
        &self,
        address: &ChatAddress,
        agent_id: &str,
        header: &rua_session::ChatSessionHeader,
        running_config: rua_config::ChatRunningConfig,
    ) -> Result<crate::services::product_hooks::ChatConfigurationMaterialization, String> {
        let assignment = self.resolve_agent_turn_assignment(agent_id).await?;
        let project_root = match address.project_id() {
            Some(project_id) => {
                let binding = self
                    .project_bindings
                    .as_ref()
                    .ok_or("Project Resource Binding Store unavailable")?
                    .get(project_id)
                    .map_err(|error| error.to_string())?
                    .ok_or("Project Resource Binding missing")?;
                Some(crate::services::project_chat::working_directory(
                    &binding,
                    Some(header),
                    None,
                )?)
            }
            None => None,
        };
        let current = header
            .extra
            .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY);
        let field = |name: &str| {
            current
                .and_then(|value| value.get(name))
                .or_else(|| header.extra.get(name))
                .filter(|value| !value.is_null())
                .cloned()
        };
        let authority_device_id = header
            .extra
            .get("authority_placement")
            .and_then(|value| value.get("device_id"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or("Chat Authority Device missing")?
            .to_string();
        self.capture_chat_configuration(
            assignment,
            running_config,
            project_root.as_deref(),
            crate::services::chat_configuration_capture::ChatConfigurationSelection {
                model: match field("model") {
                    Some(model) => Some(model),
                    None => self.default_authority_chat_model().await,
                },
                thinking: field("thinking"),
                collaboration_mode: field("collaboration_mode")
                    .or_else(|| {
                        header
                            .extra
                            .get(rua_core::CHAT_COLLABORATION_MODE_META_KEY)
                            .cloned()
                    })
                    .unwrap_or_else(|| {
                        serde_json::json!(rua_core::AgentCollaborationMode::default())
                    }),
                harness_configuration: field("harness_configuration"),
                authority_device_id,
            },
        )
        .await
    }

    pub async fn chat_configuration_control(
        &self,
        address: &ChatAddress,
        agent_id: &str,
        scope_root: &Path,
        update: Option<(&str, &str)>,
    ) -> Result<serde_json::Value, String> {
        use crate::services::product_hooks::ChatConfigurationPreview;

        self.conversation_authority
            .as_ref()
            .ok_or("Conversation Authority unavailable")?;
        let hooks = self
            .product_hooks
            .as_ref()
            .ok_or("Chat configuration service unavailable")?;
        let operation = ChatConfigurationUpdate::begin(self, address, agent_id, scope_root).await?;
        let current = operation.current().cloned().ok_or("Chat not found")?;
        let store = rua_session::WorkspaceSessionStore::new(scope_root);
        let is_collaboration_child = store
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
            .is_some_and(|header| {
                header
                    .extra
                    .contains_key(rua_session::collaboration::COLLABORATION_META_KEY)
            });
        let already_applied_retry = update
            .map(|(_, candidate_epoch)| current.epoch == candidate_epoch)
            .unwrap_or(false);
        let mut candidate = if let Some((expected_epoch, candidate_epoch)) = update {
            if already_applied_retry {
                current.clone()
            } else {
                if current.epoch != expected_epoch {
                    return Err(
                        "Chat configuration changed; reload the preview before updating"
                            .to_string(),
                    );
                }
                let preview = hooks
                    .configuration_preview(address, candidate_epoch)
                    .await
                    .ok_or("Configuration preview expired; preview again before updating")?;
                if preview.current != current {
                    return Err(
                        "Chat configuration changed; reload the preview before updating"
                            .to_string(),
                    );
                }
                if preview.candidate.selected_agent_id != current.selected_agent_id {
                    return Err("Configuration preview belongs to a different Agent".to_string());
                }
                preview.candidate
            }
        } else {
            let header = store
                .load_chat_header(address.chat_id().as_str())
                .await
                .map_err(|error| error.to_string())?
                .ok_or("Chat not found")?;
            let defaults = rua_config::ChatRunningConfig::from_agent_defaults(
                &self
                    .authority_running_config_typed(agent_id)
                    .await
                    .map_err(|error| error.to_string())?,
            );
            let mut materialization = self
                .chat_configuration_materialization(address, agent_id, &header, defaults)
                .await?;
            // Updating defaults must keep the complete snapshot's choices,
            // including absence, rather than resurrect legacy header fields.
            materialization.model = current.model.clone();
            materialization.thinking = current.thinking.clone();
            materialization.collaboration_mode = current.collaboration_mode.clone();
            materialization.harness_configuration = current.harness_configuration.clone();
            let mut candidate = hooks
                .resolve_complete_chat_configuration(address, materialization)
                .await?;
            candidate.extra = current.extra.clone();
            if is_collaboration_child {
                candidate.security_config = current.security_config.clone();
                candidate.collaboration_mode = current.collaboration_mode.clone();
            }
            candidate.validate()?;
            hooks
                .store_configuration_preview(
                    address,
                    ChatConfigurationPreview {
                        current: current.clone(),
                        candidate: candidate.clone(),
                        applied: false,
                        collaboration: None,
                    },
                )
                .await;
            candidate
        };
        if is_collaboration_child {
            candidate.security_config = current.security_config.clone();
            candidate.collaboration_mode = current.collaboration_mode.clone();
        }
        candidate.validate()?;
        if update.is_some() {
            operation.replace(&candidate).await?;
        }
        let collaboration = store
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
            .and_then(|header| {
                header
                    .extra
                    .get(rua_session::collaboration::COLLABORATION_META_KEY)
                    .cloned()
            })
            .map(serde_json::from_value::<rua_session::collaboration::CollaborationDescriptor>)
            .transpose()
            .map_err(|error| format!("Invalid collaboration configuration state: {error}"))?
            .map(|descriptor| descriptor.configuration_view());
        serde_json::to_value(ChatConfigurationPreview {
            current,
            candidate,
            applied: update.is_some(),
            collaboration,
        })
        .map_err(|error| error.to_string())
    }

    pub async fn update_authority_chat_running_config(
        &self,
        address: &ChatAddress,
        agent_id: &str,
        scope_root: &Path,
        running: serde_json::Value,
    ) -> rua_core::Result<rua_config::ChatRunningConfig> {
        let running_config = serde_json::from_value::<rua_config::ChatRunningConfig>(running)
            .map_err(|error| rua_core::RuaError::Validation {
                message: format!("invalid Chat Running Config: {error}"),
            })?;
        running_config
            .validate()
            .map_err(|message| rua_core::RuaError::Validation { message })?;
        ChatConfigurationUpdate::begin(self, address, agent_id, scope_root)
            .await
            .map_err(|message| rua_core::RuaError::Validation { message })?
            .update_running_config(running_config)
            .await
    }

    pub async fn update_authority_running_config(
        &self,
        agent_id: &str,
        chat_id: &str,
        running: serde_json::Value,
    ) -> rua_core::Result<AppliedAgentRunningConfig> {
        let scope_root = self
            .registry
            .get_or_create(agent_id)
            .await
            .map_err(|error| rua_core::RuaError::Runtime {
                message: error.to_string(),
            })?
            .workspace_dir();
        self.update_authority_running_config_for(
            &ChatAddress::agent(agent_id, chat_id),
            agent_id,
            &scope_root,
            running,
        )
        .await
    }

    async fn update_authority_running_config_for(
        &self,
        address: &ChatAddress,
        agent_id: &str,
        scope_root: &Path,
        running: serde_json::Value,
    ) -> rua_core::Result<AppliedAgentRunningConfig> {
        let agent_service =
            self.agent_service
                .as_ref()
                .ok_or_else(|| rua_core::RuaError::Runtime {
                    message: "target Agent service is unavailable".to_string(),
                })?;
        let detail = agent_service
            .update_agent(
                agent_id,
                crate::endpoints::dto::AgentUpdate {
                    running: Some(running),
                    name: None,
                    description: None,
                    enabled: None,
                    language: None,
                    system_prompt_files: None,
                    default_model: None,
                    channels: None,
                    mcp: None,
                    heartbeat: None,
                    llm_routing: None,
                    tools: None,
                    plan: None,
                    security: None,
                    skill_names: None,
                    extra: serde_json::Map::new(),
                },
            )
            .await?;
        let running = detail.running.clone().unwrap_or_else(|| {
            serde_json::to_value(rua_config::AgentsRunningConfig::default()).unwrap_or_default()
        });
        let applied = AppliedAgentRunningConfig {
            running,
            agent: serde_json::to_value(detail).unwrap_or_default(),
        };
        if let Some(authority) = self.conversation_authority.as_ref() {
            let meta = crate::services::conversation_authority_target::load_chat_control_meta(
                scope_root,
                address.chat_id().as_str(),
            )
            .await
            .map_err(|error| rua_core::RuaError::Runtime {
                message: error.to_string(),
            })?
            .unwrap_or_else(|| serde_json::json!({}));
            authority
                .publish_running_config_snapshot(address, &meta, applied.running.clone())
                .await;
        }
        self.session_sync
            .emit_agent_config_committed(agent_id, applied.agent.clone())
            .await;
        Ok(applied)
    }

    pub async fn adopt_conversation_authority(
        &self,
        agent_id: &str,
        chat_id: &str,
        target: crate::services::conversation_authority_placement::ConversationAuthorityPlacement,
    ) -> Result<crate::services::chat_metadata_store::ChatMetadataRecord, String> {
        let runtime = self
            .registry
            .get_or_create(agent_id)
            .await
            .map_err(|error| error.to_string())?;
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        let workspace_dir = runtime.workspace_dir();
        let record = authority
            .switch_authority(
                None,
                &workspace_dir,
                &rua_core::ChatAddress::agent(agent_id, chat_id),
                target,
            )
            .await
            .map_err(|error| error.to_string())?;
        self.session_sync.spawn_chat_state_publication_for(
            &workspace_dir,
            &rua_core::ChatAddress::agent(agent_id, chat_id),
            Some(record.to_json_value()),
        );
        Ok(record)
    }

    async fn cancel_acp_turn(&self, agent_id: &str, chat_id: &str) -> bool {
        let Some(registry) = self.acp_chat_registry.as_ref() else {
            return false;
        };
        let Some(handle) = registry.get(agent_id) else {
            return false;
        };
        let Some(session) = handle.manager.get(chat_id).await else {
            return false;
        };
        let runtime_session_id = rua_core::id::SessionId::new(chat_id.to_string());
        match handle
            .service
            .cancel_turn(&runtime_session_id, &session.runner)
            .await
        {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    agent_id,
                    chat_id,
                    runner = %session.runner,
                    error = %error,
                    "failed to cancel ACP turn"
                );
                false
            }
        }
    }

    async fn deliver_remote_authority(
        &self,
        execution_agent_id: &str,
        address: &rua_core::ChatAddress,
        request: &ConsoleChatRequest,
        workspace_dir: Option<PathBuf>,
        source_device_id: &str,
        authority: AuthorityRouteTarget<'_>,
    ) -> ConversationInputDeliveryOutcome {
        let authority_placement = match workspace_dir.as_deref() {
            Some(workspace_dir) => {
                match crate::services::conversation_authority_target::load_chat_authority_placement_target(
                    workspace_dir,
                    address.chat_id().as_str(),
                )
                .await
                {
                    Ok(placement)
                        if placement.device_id == authority.device_id
                            && placement.authority_epoch == authority.epoch =>
                    {
                        crate::services::conversation_authority_placement::ConversationAuthorityPlacement {
                            device_id: placement.device_id,
                            authority_epoch: placement.authority_epoch,
                            base_manifest_hash: placement.base_manifest_hash,
                        }
                    }
                    Ok(_) => {
                        return ConversationInputDeliveryOutcome::Rejected {
                            detail: "conversation authority changed before delivery".to_string(),
                            target_device_id: Some(authority.device_id.to_string()),
                            failure_kind: Some("business".to_string()),
                        };
                    }
                    Err(error) => {
                        return ConversationInputDeliveryOutcome::Rejected {
                            detail: error,
                            target_device_id: Some(authority.device_id.to_string()),
                            failure_kind: Some("business".to_string()),
                        };
                    }
                }
            }
            None => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: "conversation workspace is unavailable".to_string(),
                    target_device_id: Some(authority.device_id.to_string()),
                    failure_kind: Some("business".to_string()),
                };
            }
        };
        if let Some(workspace_dir) = workspace_dir.as_deref() {
            if let Err(error) = self
                .remote_input_delivery
                .clear_source_pending_if_stable_locally(workspace_dir, address)
                .await
            {
                tracing::warn!(
                    chat_address = %address,
                    error = %error,
                    "failed to clear finalized source-side pending remote conversation input marker"
                );
            }
        }
        let params = RemoteConversationInputParams {
            address,
            execution_agent_id,
            request,
            source_device_id,
            target_device_id: authority.device_id,
            authority_epoch: authority.epoch,
            base_manifest_hash: &authority_placement.base_manifest_hash,
            workspace_dir: workspace_dir.as_deref(),
            materialize_conversation: false,
        };
        let mut prepared = match self
            .remote_input_delivery
            .build_remote_conversation_input_request(&params)
            .await
        {
            Ok(prepared) => prepared,
            Err(detail) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail,
                    target_device_id: Some(authority.device_id.to_string()),
                    failure_kind: Some("invalid_request".to_string()),
                };
            }
        };
        let first = self
            .remote_input_delivery
            .deliver_prepared_conversation_input(&prepared, "remote conversation input")
            .await;
        let outcome = match first {
            Ok(outcome) => outcome,
            Err(DeviceCommandDeliveryError::AuthorityRedirect(redirect)) => {
                let replacement = match self
                    .observe_authority_redirect(authority.epoch, &redirect)
                    .await
                {
                    Ok(replacement) => replacement,
                    Err(detail) => {
                        return ConversationInputDeliveryOutcome::Rejected {
                            detail,
                            target_device_id: Some(redirect.placement.device_id),
                            failure_kind: Some("authority_redirect".to_string()),
                        };
                    }
                };
                prepared.target_device_id = replacement.device_id;
                prepared.authority_epoch = replacement.authority_epoch;
                prepared.base_manifest_hash = replacement.base_manifest_hash;
                match self
                    .remote_input_delivery
                    .deliver_prepared_conversation_input(
                        &prepared,
                        "redirected remote conversation input",
                    )
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return ConversationInputDeliveryOutcome::Rejected {
                            detail: error.to_string(),
                            target_device_id: Some(prepared.target_device_id),
                            failure_kind: Some(error.failure_kind().to_string()),
                        };
                    }
                }
            }
            Err(error) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: error.to_string(),
                    target_device_id: Some(prepared.target_device_id),
                    failure_kind: Some(error.failure_kind().to_string()),
                };
            }
        };
        ConversationInputDeliveryOutcome::remote(address.chat_id().to_string(), outcome)
    }

    async fn deliver_local_authority(
        &self,
        agent_id: &str,
        request: ConsoleChatRequest,
        chat_id: String,
        workspace_dir: Option<PathBuf>,
        route_target: AuthorityRouteTarget<'_>,
        first_submission: Option<FirstAgentSubmission>,
    ) -> ConversationInputDeliveryOutcome {
        let address = rua_core::ChatAddress::agent(agent_id, &chat_id);
        let key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
        let Some(authority) = self.conversation_authority.as_ref() else {
            return ConversationInputDeliveryOutcome::Rejected {
                detail: "Conversation Authority is unavailable".to_string(),
                target_device_id: Some(route_target.device_id.to_string()),
                failure_kind: Some("business".to_string()),
            };
        };
        let (driver, driver_detail) = if let Some(first) = first_submission.as_ref() {
            (
                ConversationDriverKind::Rua,
                active_turn_driver_detail(None, first.initial_model.clone()),
            )
        } else {
            self.conversation_driver_for(agent_id, &chat_id, workspace_dir.as_deref())
                .await
        };
        let mut pending =
            build_authority_pending_message(&request, route_target.device_id, route_target.epoch);
        if first_submission.is_some() {
            if let Err(detail) = self.validate_initial_acp(
                agent_id,
                &address,
                workspace_dir.as_deref().expect("Agent workspace resolved"),
                &pending.extra,
            ) {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail,
                    target_device_id: Some(route_target.device_id.to_string()),
                    failure_kind: Some("invalid_request".into()),
                };
            }
        } else {
            pending.extra.remove(INITIAL_ACP_KEY);
        }
        let persist = if let Some(first) = first_submission {
            pending.extra.insert(
                crate::services::conversation_authority::FIRST_CHAT_SUBMISSION_CANDIDATE_KEY
                    .to_string(),
                serde_json::Value::Bool(true),
            );
            pending.extra.insert(
                crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY.to_string(),
                serde_json::to_value(&first.configuration)
                    .expect("Chat Hook Configuration serializes"),
            );
            pending = match crate::services::first_chat_submission::input_with_configuration(
                &pending,
                &first.configuration,
            ) {
                Ok(pending) => pending,
                Err(detail) => {
                    return ConversationInputDeliveryOutcome::Rejected {
                        detail,
                        target_device_id: Some(route_target.device_id.to_string()),
                        failure_kind: Some("invalid_request".to_string()),
                    };
                }
            };
            let workspace_dir_for_persist = workspace_dir
                .clone()
                .expect("Agent Chat workspace was resolved");
            let address_for_persist = address.clone();
            let agent_id_for_persist = agent_id.to_string();
            let placement_for_persist = first.placement.clone();
            let initial_model = first.initial_model.clone();
            let configuration = first.configuration.clone();
            Some(Box::new(
                move |pending,
                      prepared|
                      -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<(), String>> + Send>,
                > {
                    Box::pin(async move {
                        crate::services::first_chat_submission::persist_first_agent_submission(
                            &workspace_dir_for_persist,
                            &address_for_persist,
                            &agent_id_for_persist,
                            &pending,
                            &prepared,
                            &placement_for_persist,
                            crate::services::first_chat_submission::AgentChatMaterializationSnapshot {
                                initial_model,
                                configuration,
                            },
                        )
                        .await
                    })
                },
            ) as ConversationSubmissionPersistCallback)
        } else if let Some(workspace_dir) = workspace_dir.as_deref() {
            if let Err(detail) = self
                .attach_chat_configuration(workspace_dir, &address, agent_id, &mut pending)
                .await
            {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail,
                    target_device_id: Some(route_target.device_id.to_string()),
                    failure_kind: Some("invalid_request".to_string()),
                };
            }
            None
        } else {
            None
        };
        let acceptance = match authority
            .accept_submission(
                self.streams.as_ref(),
                &key,
                ConversationActiveTurnStart {
                    address,
                    driver,
                    authority_device_id: route_target.device_id.to_string(),
                    authority_epoch: route_target.epoch,
                    driver_detail,
                },
                pending,
                current_sync_device_id(&self.sync_device),
                persist,
            )
            .await
        {
            Ok(acceptance) => acceptance,
            Err(detail) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail,
                    target_device_id: Some(route_target.device_id.to_string()),
                    failure_kind: Some("business".to_string()),
                };
            }
        };
        let workspace_dir = workspace_dir.expect("Agent Chat workspace was resolved");
        self.finish_local_submission(
            acceptance,
            key,
            ConversationExecutionContext {
                address: rua_core::ChatAddress::agent(agent_id, &chat_id),
                execution_agent_id: agent_id.to_string(),
                scope_root: workspace_dir.clone(),
                working_dir: workspace_dir,
            },
            truncate_chat_name(&request.message),
            false,
            true,
        )
        .await
    }

    pub(crate) fn validate_collaboration_resource(
        &self,
        address: &ChatAddress,
        device_id: &str,
        working_dir: &Path,
    ) -> Result<(), String> {
        if let Some(project_id) = address.project_id() {
            let catalog = self
                .project_catalog
                .as_ref()
                .ok_or("Project Catalog unavailable")?;
            if catalog.lifecycle_transition_in_progress(project_id) {
                return Err("Project lifecycle transition in progress".into());
            }
            let project = catalog
                .get(project_id)
                .map_err(|e| e.to_string())?
                .ok_or("Project not found")?;
            if project.lifecycle != rua_project::ProjectLifecycle::Active {
                return Err("Project is not active".into());
            }
            let binding = self
                .project_bindings
                .as_ref()
                .ok_or("Project bindings unavailable")?
                .get(project_id)
                .map_err(|e| e.to_string())?
                .ok_or("Project binding missing")?;
            if project.resource_device_id != device_id
                || binding.resource_device_id != device_id
                || !binding
                    .resources
                    .directories
                    .iter()
                    .any(|directory| directory.resource_root == working_dir)
            {
                return Err("Collaboration must retain the Project resource binding".into());
            }
        }
        Ok(())
    }

    pub(crate) async fn accept_collaboration_input(
        &self,
        execution: ConversationExecutionContext,
        placement: ConversationAuthorityPlacement,
        mut pending: ConversationInputPayload,
        persist: ConversationSubmissionPersistCallback,
    ) -> Result<ConversationInputDeliveryOutcome, String> {
        let _resource_admission = self.project_work_admission(&execution.address)?;
        self.validate_collaboration_resource(
            &execution.address,
            &placement.device_id,
            &execution.working_dir,
        )?;
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or("Conversation Authority unavailable")?;
        let address = execution.address.clone();
        let key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
        self.attach_chat_configuration(
            &execution.scope_root,
            &address,
            &execution.execution_agent_id,
            &mut pending,
        )
        .await?;
        let title = truncate_chat_name(&pending.message);
        let (driver, driver_detail) = self
            .conversation_driver_for(
                &execution.execution_agent_id,
                address.chat_id().as_str(),
                Some(&execution.scope_root),
            )
            .await;
        let start = ConversationActiveTurnStart {
            address: address.clone(),
            driver,
            authority_device_id: placement.device_id,
            authority_epoch: placement.authority_epoch,
            driver_detail,
        };
        let acceptance = authority
            .accept_interjecting_submission(
                &self.streams,
                &key,
                start,
                pending,
                None,
                Some(persist),
            )
            .await?;
        Ok(self
            .finish_local_submission(acceptance, key, execution, title, false, false)
            .await)
    }

    async fn finish_local_submission(
        &self,
        acceptance: ConversationSubmissionAcceptance,
        key: StreamKey,
        execution: ConversationExecutionContext,
        first_input_title: String,
        include_control: bool,
        resume_collaboration_wake: bool,
    ) -> ConversationInputDeliveryOutcome {
        let address = execution.address.clone();
        let scope_root = execution.scope_root.clone();
        if resume_collaboration_wake {
            if let Err(error) = rua_session::WorkspaceSessionStore::new(&scope_root)
                .set_collaboration_wake_suppressed(address.chat_id().as_str(), false)
                .await
            {
                tracing::warn!(address = %address, error = %error, "failed to resume collaboration wake after user input");
            }
            if let Err(error) = self.resume_task_results(Some(&address)).await {
                tracing::warn!(%error, "failed to resume pending Task results");
            }
            if let Some(service) = self
                .collaboration
                .as_ref()
                .and_then(|binding| binding.get())
                .and_then(std::sync::Weak::upgrade)
            {
                let source = address.clone();
                tokio::spawn(async move {
                    if let Err(error) = service.resume_deferred_results(source.clone()).await {
                        tracing::warn!(address = %source, error = %error, "failed to resume deferred collaboration results");
                    }
                });
            }
        }
        match acceptance {
            ConversationSubmissionAcceptance::Started {
                input,
                producer_id,
                cancel_rx,
            } => {
                let chat_service =
                    rua_chat::ChatService::for_resolved_scope(scope_root.clone(), address.clone());
                if chat_service
                    .replace_placeholder_title(&address, first_input_title)
                    .await
                    .is_ok()
                {
                    if let Ok(Some(entry)) =
                        crate::services::chat_index::get(&scope_root, address.chat_id().as_str())
                            .await
                    {
                        self.session_sync
                            .emit_chat_index_upsert_for(
                                &address,
                                crate::services::console_chat::chat_index_entry_to_json(&entry),
                            )
                            .await;
                    }
                }
                self.spawn_console_producer(key, producer_id, cancel_rx, execution);
                let outcome = ConversationInputDeliveryOutcome::local_started(
                    address.chat_id().to_string(),
                    Some(input.input_id),
                    Some(input.submission_id),
                );
                if include_control {
                    outcome.with_control(self.authority_control_view(&scope_root, &address).await)
                } else {
                    outcome
                }
            }
            ConversationSubmissionAcceptance::Queued { input } => {
                let outcome = ConversationInputDeliveryOutcome::local_queued(
                    address.chat_id().to_string(),
                    Some(input.input_id),
                    Some(input.submission_id),
                );
                if include_control {
                    outcome.with_control(self.authority_control_view(&scope_root, &address).await)
                } else {
                    outcome
                }
            }
        }
    }

    fn spawn_console_producer(
        &self,
        key: StreamKey,
        producer_id: uuid::Uuid,
        cancel_rx: tokio::sync::watch::Receiver<rua_core::RunControl>,
        execution: ConversationExecutionContext,
    ) {
        let streams = Arc::clone(&self.streams);
        let panic_streams = Arc::clone(&self.streams);
        let chat_service = Arc::clone(&self.chat_service);
        let acp_chat_registry = self.acp_chat_registry.clone();
        let workspace_registry = Arc::clone(&self.registry);
        let panic_workspace_registry = Arc::clone(&self.registry);
        let session_sync = self.session_sync.clone();
        let conversation_authority = self.conversation_authority.clone();
        let panic_conversation_authority = self.conversation_authority.clone();
        let active_stream_broadcast_hub = self.active_stream_broadcast_hub.clone();
        let session_sync_for_producer = Some(self.session_sync.clone());
        let remote_input_delivery = Some(self.remote_input_delivery.clone());
        let candidate_completion_runtime = self.candidate_completion_runtime.clone();
        let task_watch_wake_receipts = self.task_watch_wake_receipts.clone();
        let panic_task_watch_wake_receipts = self.task_watch_wake_receipts.clone();
        let agent_id = execution.execution_agent_id.clone();
        let chat_id = execution.address.chat_id().to_string();
        let panic_scope_root = execution.scope_root.clone();
        let panic_agent_id = agent_id.clone();
        let panic_chat_id = chat_id.clone();
        let panic_key = key.clone();
        let panic_address = panic_key.address.clone();
        tokio::spawn(async move {
            let result = AssertUnwindSafe(drive_conversation_producer(
                key,
                producer_id,
                cancel_rx,
                streams,
                chat_service,
                workspace_registry,
                acp_chat_registry,
                execution,
                active_stream_broadcast_hub,
                conversation_authority,
                candidate_completion_runtime,
                session_sync_for_producer,
                remote_input_delivery,
                task_watch_wake_receipts,
            ))
            .catch_unwind()
            .await;
            if let Err(payload) = result {
                let detail = panic_payload_message(payload.as_ref());
                finalize_panicked_console_producer(PanickedConsoleProducerContext {
                    panic_key,
                    producer_id,
                    panic_streams,
                    panic_workspace_registry,
                    session_sync,
                    panic_conversation_authority,
                    panic_agent_id,
                    panic_chat_id,
                    panic_scope_root,
                    detail,
                })
                .await;
                panic_task_watch_wake_receipts.complete_all_for_address(&panic_address);
            }
        });
    }

    async fn authority_control_view(
        &self,
        scope_root: &std::path::Path,
        address: &ChatAddress,
    ) -> Option<Box<crate::services::conversation_authority::ConversationControlView>> {
        let authority = self.conversation_authority.as_ref()?;
        let meta = crate::services::conversation_authority_target::load_chat_control_meta(
            scope_root,
            address.chat_id().as_str(),
        )
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| serde_json::json!({}));
        Some(Box::new(authority.control_view(address, &meta).await))
    }

    pub async fn accept_remote_conversation_input(
        &self,
        mut request: RemoteConversationInputRequest,
        accepted_at: String,
    ) -> std::result::Result<RemoteConversationInputAck, String> {
        let _project_admission = match request.address.project_id() {
            Some(project_id) => Some(
                self.project_catalog
                    .as_ref()
                    .ok_or("Project catalog unavailable")?
                    .begin_admission(project_id)
                    .map_err(|error| error.to_string())?,
            ),
            None => None,
        };
        let (
            scope_root,
            working_dir,
            placement,
            requires_atomic_materialization,
            turn_assignment,
            initial_model,
        ) = match request.address.scope() {
            rua_core::ChatScope::Agent { .. } => {
                let runtime = self
                    .registry
                    .get_or_create(&request.agent_id)
                    .await
                    .map_err(|error| format!("workspace unavailable: {error}"))?;
                let agent_workspace_dir = runtime.workspace_dir();
                let indexed =
                    crate::services::chat_index::get(&agent_workspace_dir, request.chat_id())
                        .await?;
                let header = rua_session::WorkspaceSessionStore::new(&agent_workspace_dir)
                    .load_chat_header(request.chat_id())
                    .await
                    .map_err(|error| error.to_string())?;
                let first_submission = indexed.is_none() && header.is_none();
                if indexed.is_none() && header.is_some() {
                    return Err(
                        "unindexed Agent Chat storage does not match this submission".to_string(),
                    );
                }
                let placement = if first_submission {
                    let expected_hash =
                        crate::services::conversation_manifest::initial_conversation_manifest_hash(
                            &request.address,
                        )
                        .map_err(|error| error.to_string())?;
                    if request.base_manifest_hash != expected_hash {
                        return Err(
                            "Agent Chat initial manifest hash does not match its address"
                                .to_string(),
                        );
                    }
                    ConversationAuthorityPlacement {
                        device_id: request.target_device_id.clone(),
                        authority_epoch: request.authority_epoch,
                        base_manifest_hash: expected_hash,
                    }
                } else {
                    let placement =
                        crate::services::conversation_authority_target::load_chat_authority_placement_target(
                            &agent_workspace_dir,
                            request.chat_id(),
                        )
                        .await?;
                    ConversationAuthorityPlacement {
                        device_id: placement.device_id,
                        authority_epoch: placement.authority_epoch,
                        base_manifest_hash: placement.base_manifest_hash,
                    }
                };
                let initial_model = if first_submission {
                    self.resolve_initial_authority_chat_model(
                        request.metadata.get("chat_model_slot").cloned(),
                    )
                    .await?
                } else {
                    None
                };
                let assignment = if first_submission {
                    let assignment = self
                        .resolve_agent_turn_assignment(&request.agent_id)
                        .await?;
                    request.metadata.insert(
                        rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
                        serde_json::to_value(&assignment).map_err(|error| error.to_string())?,
                    );
                    Some(assignment)
                } else {
                    None
                };
                (
                    agent_workspace_dir.clone(),
                    agent_workspace_dir,
                    placement,
                    first_submission,
                    assignment,
                    initial_model,
                )
            }
            rua_core::ChatScope::Project { project_id, .. } => {
                let data_dir = self
                    .data_dir
                    .as_ref()
                    .ok_or_else(|| "Project Chat runtime is unavailable".to_string())?;
                let catalog = self
                    .project_catalog
                    .as_ref()
                    .ok_or_else(|| "Project Catalog is unavailable".to_string())?;
                if catalog.lifecycle_transition_in_progress(project_id) {
                    return Err("Project lifecycle transition is in progress".to_string());
                }
                let project = catalog
                    .get(project_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("Project not found: {project_id}"))?;
                if project.lifecycle != rua_project::ProjectLifecycle::Active {
                    return Err("Archived Project cannot accept new work".to_string());
                }
                let bindings = self
                    .project_bindings
                    .as_ref()
                    .ok_or_else(|| "Project Resource Binding Store is unavailable".to_string())?;
                let binding = bindings
                    .get(project_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("Project Resource Binding not found: {project_id}"))?;
                if binding.resource_device_id != request.target_device_id {
                    return Err(
                        "Project Chat target does not match its resource Device".to_string()
                    );
                }

                let scope_root = ChatScopeStore::resolve(data_dir, request.address.clone())
                    .root()
                    .to_path_buf();
                let indexed =
                    crate::services::chat_index::get(&scope_root, request.chat_id()).await?;
                let header = rua_session::WorkspaceSessionStore::new(&scope_root)
                    .load_chat_header(request.chat_id())
                    .await
                    .map_err(|error| error.to_string())?;
                let first_submission = indexed.is_none() && header.is_none();
                let directory_key =
                    crate::services::project_chat::PROJECT_WORKING_DIRECTORY_META_KEY;
                let directory_id = header
                    .as_ref()
                    .and_then(|header| header.extra.get(directory_key))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(if first_submission {
                        request
                            .metadata
                            .get(directory_key)
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(&binding.resources.primary_directory_id)
                    } else {
                        rua_project::INITIAL_PROJECT_DIRECTORY_ID
                    })
                    .to_string();
                request
                    .metadata
                    .insert(directory_key.into(), serde_json::json!(directory_id));
                let working_dir = crate::services::project_chat::working_directory(
                    &binding,
                    header.as_ref(),
                    Some(&directory_id),
                )?;

                let initial_model = if first_submission {
                    self.resolve_initial_authority_chat_model(
                        request.metadata.get("chat_model_slot").cloned(),
                    )
                    .await?
                } else {
                    None
                };
                let selected_agent_id = if first_submission {
                    request.agent_id.clone()
                } else {
                    let header = header
                        .as_ref()
                        .ok_or_else(|| "Project Chat header is unavailable".to_string())?;
                    crate::services::project_chat::selection_from_header(header)?.agent_id
                };
                if !first_submission && request.agent_id != selected_agent_id {
                    return Err(format!(
                        "stale Project Chat Agent Selection: requested '{}', current '{}'",
                        request.agent_id, selected_agent_id
                    ));
                }
                let validated_assignment =
                    self.resolve_project_turn_agent(&selected_agent_id).await?;
                self.registry
                    .get_or_create(&selected_agent_id)
                    .await
                    .map_err(|error| format!("workspace unavailable: {error}"))?;
                request.agent_id = selected_agent_id;
                let assignment = first_submission.then_some(validated_assignment);
                if let Some(assignment) = assignment.as_ref() {
                    request.metadata.insert(
                        rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
                        serde_json::to_value(assignment).map_err(|error| error.to_string())?,
                    );
                }
                let placement = match indexed {
                    Some(_) => {
                        let placement =
                                crate::services::conversation_authority_target::load_chat_authority_placement_target(
                                    &scope_root,
                                    request.chat_id(),
                                )
                                .await?;
                        if placement.device_id != binding.resource_device_id {
                            return Err(
                                "Project Chat authority does not match its resource Device"
                                    .to_string(),
                            );
                        }
                        crate::services::conversation_authority_placement::ConversationAuthorityPlacement {
                                device_id: placement.device_id,
                                authority_epoch: placement.authority_epoch,
                                base_manifest_hash: placement.base_manifest_hash,
                            }
                    }
                    None => {
                        let expected_hash =
                                crate::services::conversation_manifest::initial_conversation_manifest_hash(
                                    &request.address,
                                )
                                .map_err(|error| error.to_string())?;
                        if request.base_manifest_hash != expected_hash {
                            return Err(
                                "Project Chat initial manifest hash does not match its address"
                                    .to_string(),
                            );
                        }
                        crate::services::conversation_authority_placement::ConversationAuthorityPlacement {
                                device_id: binding.resource_device_id.clone(),
                                authority_epoch: request.authority_epoch,
                                base_manifest_hash: expected_hash,
                            }
                    }
                };
                (
                    scope_root,
                    working_dir,
                    placement,
                    first_submission,
                    assignment,
                    initial_model,
                )
            }
        };
        if placement.device_id != request.target_device_id
            || placement.authority_epoch != request.authority_epoch
            || placement.base_manifest_hash != request.base_manifest_hash
        {
            return Err("stale conversation authority Epoch".to_string());
        }
        if let Some(acceptance) = device_delivery_acceptance_locally(&scope_root, &request).await? {
            if request.address.project_id().is_some() {
                rua_chat::ChatService::for_resolved_scope(&scope_root, request.address.clone())
                    .repair_projection(&request.address)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            if acceptance.turn_status == "in_progress" {
                if let Some(authority) = self.conversation_authority.as_ref() {
                    if authority.active_turn(&request.address).await.is_none() {
                        let key = StreamKey::new(
                            CONSOLE_CHANNEL_ID,
                            request.address.clone(),
                            Priority::Normal,
                        );
                        let (driver, driver_detail) = self
                            .conversation_driver_for(
                                &request.agent_id,
                                request.chat_id(),
                                Some(scope_root.as_path()),
                            )
                            .await;
                        let mut pending = pending_from_remote_request(&request);
                        pending.extra.insert(
                            crate::services::conversation_authority::SUBMITTED_USER_TURN_PERSISTED_KEY
                                .to_string(),
                            serde_json::Value::Bool(true),
                        );
                        pending.extra.insert(
                            INPUT_ID_KEY.to_string(),
                            serde_json::json!(acceptance.input_id.clone()),
                        );
                        pending.extra.insert(
                            ACCEPTANCE_MODE_KEY.to_string(),
                            serde_json::json!(acceptance.mode),
                        );
                        pending.extra.insert(
                            "turn_id".to_string(),
                            serde_json::json!(acceptance.turn_id.clone()),
                        );
                        if request.address.project_id().is_some() {
                            let assignment = rua_chat::ChatService::for_resolved_scope(
                                &scope_root,
                                request.address.clone(),
                            )
                            .load_turn_agent_assignment(&request.address, &acceptance.turn_id)
                            .await
                            .map_err(|error| error.to_string())?;
                            pending.extra.insert(
                                rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
                                serde_json::to_value(assignment)
                                    .map_err(|error| error.to_string())?,
                            );
                        }
                        if let ConversationSubmissionAcceptance::Started {
                            producer_id,
                            cancel_rx,
                            ..
                        } = authority
                            .accept_submission(
                                self.streams.as_ref(),
                                &key,
                                ConversationActiveTurnStart {
                                    address: request.address.clone(),
                                    driver,
                                    authority_device_id: request.target_device_id.clone(),
                                    authority_epoch: request.authority_epoch,
                                    driver_detail,
                                },
                                pending,
                                Some(request.source_device_id.clone()),
                                None,
                            )
                            .await?
                        {
                            self.spawn_console_producer(
                                key,
                                producer_id,
                                cancel_rx,
                                ConversationExecutionContext {
                                    address: request.address.clone(),
                                    execution_agent_id: request.agent_id.clone(),
                                    scope_root: scope_root.clone(),
                                    working_dir: working_dir.clone(),
                                },
                            );
                        }
                    }
                }
            }
            self.session_sync
                .emit_evergreen_chat_snapshot(&scope_root, &request.address)
                .await;
            if request.source_device_id != request.target_device_id {
                self.sync_committed_state_after_foreground(
                    &request.address,
                    &request.source_device_id,
                )
                .await;
            }
            return Ok(conversation_input_ack(
                &request,
                accepted_at,
                acceptance.input_id,
                acceptance.mode,
                self.authority_control_view(&scope_root, &request.address)
                    .await,
            ));
        }
        if request.address.project_id().is_some()
            && crate::services::chat_index::get(&scope_root, request.chat_id())
                .await?
                .is_none()
            && rua_session::WorkspaceSessionStore::new(&scope_root)
                .load_chat_header(request.chat_id())
                .await
                .map_err(|error| error.to_string())?
                .is_some()
        {
            return Err(
                "unindexed Project Chat storage does not match this submission".to_string(),
            );
        }

        let input = self.materialize_remote_media(&scope_root, &request).await?;
        request.input = input;

        let key = StreamKey::new(
            CONSOLE_CHANNEL_ID,
            request.address.clone(),
            Priority::Normal,
        );
        if let Some(authority) = self.conversation_authority.as_ref() {
            let (driver, driver_detail) = self
                .conversation_driver_for(
                    &request.agent_id,
                    request.chat_id(),
                    Some(scope_root.as_path()),
                )
                .await;
            let mut start = ConversationActiveTurnStart {
                address: request.address.clone(),
                driver,
                authority_device_id: request.target_device_id.clone(),
                authority_epoch: request.authority_epoch,
                driver_detail,
            };
            let mut pending = pending_from_remote_request(&request);
            if requires_atomic_materialization {
                self.validate_initial_acp(
                    &request.agent_id,
                    &request.address,
                    &working_dir,
                    &pending.extra,
                )?;
            } else {
                pending.extra.remove(INITIAL_ACP_KEY);
            }
            self.attach_chat_configuration(
                &scope_root,
                &request.address,
                &request.agent_id,
                &mut pending,
            )
            .await?;
            if requires_atomic_materialization {
                if let (Some(hooks), Some(assignment)) =
                    (self.product_hooks.as_ref(), turn_assignment.as_ref())
                {
                    let collaboration_mode: rua_core::AgentCollaborationMode = pending
                        .extra
                        .get("collaboration_mode")
                        .cloned()
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|error| error.to_string())?
                        .unwrap_or_default();
                    let running = self
                        .authority_running_config_typed(&assignment.agent_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    let configuration = hooks
                        .resolve_complete_chat_configuration(
                            &request.address,
                            self.capture_chat_configuration(
                                assignment.clone(),
                                rua_config::ChatRunningConfig::from_agent_defaults(&running),
                                request
                                    .address
                                    .project_id()
                                    .map(|_| working_dir.as_path()),
                                crate::services::chat_configuration_capture::ChatConfigurationSelection {
                                    model: initial_model.clone(),
                                    thinking: None,
                                    collaboration_mode: serde_json::to_value(collaboration_mode).map_err(|error| error.to_string())?,
                                    harness_configuration: pending.extra.get("harness_configuration").cloned(),
                                    authority_device_id: placement.device_id.clone(),
                                },
                            ).await?,
                        )
                        .await?;
                    pending.extra.insert(
                        crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY
                            .to_string(),
                        serde_json::to_value(&configuration).map_err(|error| error.to_string())?,
                    );
                    pending = crate::services::first_chat_submission::input_with_configuration(
                        &pending,
                        &configuration,
                    )?;
                    start.driver_detail = active_turn_driver_detail(None, initial_model.clone());
                }
                pending.extra.insert(
                    crate::services::conversation_authority::FIRST_CHAT_SUBMISSION_CANDIDATE_KEY
                        .to_string(),
                    serde_json::Value::Bool(true),
                );
            }
            let persist = if requires_atomic_materialization {
                let scope_root_for_persist = scope_root.clone();
                let address_for_persist = request.address.clone();
                let assignment = turn_assignment
                    .clone()
                    .ok_or_else(|| "Project Turn Agent Assignment is unavailable".to_string())?;
                let placement_for_persist = placement.clone();
                if request.address.project_id().is_some() {
                    Some(Box::new(
                        move |pending: ConversationInputPayload,
                              prepared|
                              -> std::pin::Pin<
                            Box<dyn std::future::Future<Output = Result<(), String>> + Send>,
                        > {
                            Box::pin(async move {
                                crate::services::project_chat::persist_first_project_submission(
                                &scope_root_for_persist,
                                &address_for_persist,
                                &assignment,
                                &pending,
                                &prepared,
                                &placement_for_persist,
                                crate::services::project_chat::ProjectChatMaterializationSnapshot {
                                    initial_model,
                                    configuration: crate::services::product_hooks::ChatHookConfiguration::from_persisted(pending.extra[crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY].clone())?,
                                },
                            )
                            .await
                            })
                        },
                    )
                        as ConversationSubmissionPersistCallback)
                } else {
                    let agent_id = request.agent_id.clone();
                    Some(Box::new(
                        move |pending: ConversationInputPayload,
                              prepared: crate::services::conversation_authority::FirstConversationSubmissionPersistenceContext|
                              -> std::pin::Pin<
                            Box<dyn std::future::Future<Output = Result<(), String>> + Send>,
                        > {
                            Box::pin(async move {
                                let configuration =
                                    crate::services::product_hooks::ChatHookConfiguration::from_persisted(
                                        pending.extra[crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY]
                                            .clone(),
                                    )?;
                                crate::services::first_chat_submission::persist_first_agent_submission(
                                    &scope_root_for_persist,
                                    &address_for_persist,
                                    &agent_id,
                                    &pending,
                                    &prepared,
                                    &placement_for_persist,
                                    crate::services::first_chat_submission::AgentChatMaterializationSnapshot {
                                        initial_model,
                                        configuration,
                                    },
                                )
                                .await
                            })
                        },
                    ) as ConversationSubmissionPersistCallback)
                }
            } else {
                None
            };
            let acceptance = authority
                .accept_submission(
                    self.streams.as_ref(),
                    &key,
                    start,
                    pending,
                    Some(request.source_device_id.clone()),
                    persist,
                )
                .await?;
            let (input, mode) = match acceptance {
                ConversationSubmissionAcceptance::Started {
                    input,
                    producer_id,
                    cancel_rx,
                } => {
                    self.spawn_console_producer(
                        key,
                        producer_id,
                        cancel_rx,
                        ConversationExecutionContext {
                            address: request.address.clone(),
                            execution_agent_id: request.agent_id.clone(),
                            scope_root: scope_root.clone(),
                            working_dir,
                        },
                    );
                    (input, ConversationInputAcceptanceMode::Started)
                }
                ConversationSubmissionAcceptance::Queued { input } => {
                    (input, ConversationInputAcceptanceMode::Queued)
                }
            };
            if requires_atomic_materialization {
                if let Some(entry) =
                    crate::services::chat_index::get(&scope_root, request.chat_id()).await?
                {
                    self.session_sync
                        .emit_chat_index_upsert_for(
                            &request.address,
                            crate::services::console_chat::chat_index_entry_to_json(&entry),
                        )
                        .await;
                }
                self.session_sync
                    .emit_evergreen_chat_snapshot(&scope_root, &request.address)
                    .await;
                if request.source_device_id != request.target_device_id {
                    self.remote_input_delivery
                        .spawn_priority_sync_to_device(&request.source_device_id);
                }
            }
            return Ok(conversation_input_ack(
                &request,
                accepted_at,
                input.input_id,
                mode,
                self.authority_control_view(&scope_root, &request.address)
                    .await,
            ));
        }
        Err("Conversation Authority is unavailable".to_string())
    }

    async fn materialize_remote_media(
        &self,
        workspace_dir: &Path,
        request: &RemoteConversationInputRequest,
    ) -> std::result::Result<Option<Vec<serde_json::Value>>, String> {
        if request.media_requirements.is_empty() {
            return Ok(request.input.clone());
        }

        for media in &request.media_requirements {
            self.ensure_remote_media_file(workspace_dir, media).await?;
            if !media.sha256.is_empty()
                || request
                    .metadata
                    .get("first_submission")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
            {
                let path = remote_media_path(workspace_dir, &media.filename)?;
                let (size, digest) =
                    crate::services::remote_conversation_input::media_fingerprint(&path).await?;
                if size != media.byte_size || digest != media.sha256 {
                    return Err(format!(
                        "Attachment integrity mismatch: {}",
                        media.media_ref
                    ));
                }
            }
        }

        Ok(request.input.clone())
    }

    async fn sync_committed_state_after_foreground(
        &self,
        address: &ChatAddress,
        source_device_id: &str,
    ) {
        if address.project_id().is_some() {
            self.remote_input_delivery
                .spawn_priority_sync_to_device(source_device_id);
        } else {
            self.remote_input_delivery
                .priority_sync_to_device(source_device_id)
                .await;
        }
    }

    async fn ensure_remote_media_file(
        &self,
        workspace_dir: &Path,
        media: &RemoteConversationInputMediaRequirement,
    ) -> std::result::Result<(), String> {
        let path = remote_media_path(workspace_dir, &media.filename)?;
        if path.is_file() {
            return Ok(());
        }

        Err(format!(
            "required Chat Media is unavailable: {}",
            media.media_ref
        ))
    }

    async fn conversation_driver_for(
        &self,
        agent_id: &str,
        chat_id: &str,
        workspace_dir: Option<&std::path::Path>,
    ) -> (ConversationDriverKind, Option<serde_json::Value>) {
        let model_slot = chat_model_slot_for_authority(workspace_dir, chat_id).await;
        if let Some(handle) = self
            .acp_chat_registry
            .as_ref()
            .and_then(|registry| registry.get(agent_id))
        {
            if let Some(session) = handle.manager.get(chat_id).await {
                return (
                    ConversationDriverKind::Acp,
                    active_turn_driver_detail(Some(session.runner), model_slot),
                );
            }
        }
        let Some(workspace_dir) = workspace_dir else {
            return (
                ConversationDriverKind::Rua,
                active_turn_driver_detail(None, model_slot),
            );
        };
        let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
        match store.load_chat_header(chat_id).await {
            Ok(Some(header)) => {
                let acp_binding = header
                    .extra
                    .get("acp_binding")
                    .and_then(|value| value.as_object());
                if let Some(binding) = acp_binding {
                    (
                        ConversationDriverKind::Acp,
                        active_turn_driver_detail(
                            binding
                                .get("runner")
                                .and_then(|value| value.as_str())
                                .map(str::to_string),
                            model_slot,
                        ),
                    )
                } else {
                    (
                        ConversationDriverKind::Rua,
                        active_turn_driver_detail(None, model_slot),
                    )
                }
            }
            _ => (
                ConversationDriverKind::Rua,
                active_turn_driver_detail(None, model_slot),
            ),
        }
    }
}

async fn acp_status_for_target(
    handle: &crate::services::acp_chat_registry::AcpChatRuntimeHandle,
    chat_id: &str,
    workspace_dir: &Path,
    target_device_id: &str,
) -> AcpChatDeviceStatus {
    match handle.manager.get(chat_id).await {
        Some(session) => AcpChatDeviceStatus {
            active: true,
            runner: Some(session.runner),
            session_id: session.runner_session_id,
            cwd: Some(session.cwd.to_string_lossy().to_string()),
            chat_id: chat_id.to_string(),
            workspace_dir: workspace_dir.to_string_lossy().to_string(),
            target_device_id: target_device_id.to_string(),
        },
        None => AcpChatDeviceStatus {
            active: false,
            runner: None,
            session_id: None,
            cwd: None,
            chat_id: chat_id.to_string(),
            workspace_dir: workspace_dir.to_string_lossy().to_string(),
            target_device_id: target_device_id.to_string(),
        },
    }
}

fn expand_control_tilde(path: &str) -> String {
    let home = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok());
    let Some(home) = home else {
        return path.to_string();
    };
    if path == "~" {
        return home;
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        return format!("{home}/{rest}");
    }
    path.to_string()
}

async fn chat_model_slot_for_authority(
    workspace_dir: Option<&std::path::Path>,
    chat_id: &str,
) -> Option<serde_json::Value> {
    let workspace_dir = workspace_dir?;
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let header = store.load_chat_header(chat_id).await.ok().flatten()?;
    rua_chat::model_slot_from_header(&header)
}

fn active_turn_driver_detail(
    runner: Option<String>,
    chat_model_slot: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    if runner.is_none() && chat_model_slot.is_none() {
        return None;
    }

    let mut detail = serde_json::Map::new();
    if let Some(runner) = runner {
        detail.insert("runner".to_string(), serde_json::json!(runner));
    }
    if let Some(slot) = chat_model_slot {
        detail.insert(
            "metadata".to_string(),
            serde_json::json!({
                "chat_model_slot": slot,
            }),
        );
    }
    Some(serde_json::Value::Object(detail))
}

#[doc(hidden)]
pub struct PanickedConsoleProducerContext {
    pub panic_key: StreamKey,
    pub producer_id: uuid::Uuid,
    pub panic_streams: Arc<ActiveStreamRegistry>,
    pub panic_workspace_registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
    pub session_sync: SessionSyncEmitter,
    pub panic_conversation_authority: Option<Arc<ConversationAuthority>>,
    pub panic_agent_id: String,
    pub panic_chat_id: String,
    pub panic_scope_root: PathBuf,
    pub detail: String,
}

#[doc(hidden)]
pub async fn finalize_panicked_console_producer(ctx: PanickedConsoleProducerContext) {
    let PanickedConsoleProducerContext {
        panic_key,
        producer_id,
        panic_streams,
        panic_workspace_registry: _panic_workspace_registry,
        session_sync,
        panic_conversation_authority,
        panic_agent_id,
        panic_chat_id,
        panic_scope_root,
        detail,
    } = ctx;
    let error = format!("conversation producer panicked: {detail}");
    tracing::error!(
        producer_id = %producer_id,
        agent_id = %panic_agent_id,
        chat_id = %panic_chat_id,
        error = %error,
        "console producer panicked"
    );
    if let Some(turn_id) = panic_streams
        .active_turn_id_for_chat(CONSOLE_CHANNEL_ID, &panic_key.address)
        .await
    {
        if let Err(err) = persist_conversation_runtime_failure_to_session(
            &panic_scope_root,
            &panic_agent_id,
            &panic_chat_id,
            &turn_id,
            &error,
            None,
        )
        .await
        {
            tracing::warn!(
                agent_id = %panic_agent_id,
                chat_id = %panic_chat_id,
                turn_id = %turn_id,
                error = %err,
                "failed to persist panic terminal fact for conversation producer"
            );
        } else {
            session_sync
                .emit_evergreen_chat_snapshot(&panic_scope_root, &panic_key.address)
                .await;
        }
    }
    if let Some(authority) = panic_conversation_authority {
        authority
            .finish_producer(
                panic_streams.as_ref(),
                &panic_key,
                producer_id,
                &panic_key.address,
                true,
            )
            .await;
    } else {
        panic_streams.mark_completed(&panic_key, producer_id).await;
    }
    panic_streams
        .push_event_keyed(
            &panic_key,
            serde_json::json!({
                "type": "error",
                "detail": error,
            })
            .to_string(),
        )
        .await;
    panic_streams
        .push_event_keyed(
            &panic_key,
            rua_channel::core::console::make_response_done_json(&panic_chat_id, Some("error")),
        )
        .await;
}

pub(crate) fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .map(|message| message.as_str())
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic")
        .to_string()
}

fn runtime_turn_agent_assignment(agent_id: &str) -> rua_chat::TurnAgentAssignment {
    rua_chat::TurnAgentAssignment {
        agent_id: agent_id.to_string(),
        agent_name: agent_id.to_string(),
        assigned_at: chrono::Utc::now().to_rfc3339(),
        system_prompt: None,
        system_prompt_files: Vec::new(),
        disabled_tools: Vec::new(),
    }
}

pub(crate) fn build_input_payload(request: &ConsoleChatRequest) -> ConversationInputPayload {
    let attachments: Vec<serde_json::Value> = request
        .attachments
        .iter()
        .map(|a| {
            serde_json::json!({
                "url": a.url,
                "name": a.name,
                "mime_type": a.mime_type,
                "stored_name": a.stored_name,
                "size": a.size,
            })
        })
        .collect();
    ConversationInputPayload {
        message: request.message.clone(),
        model_override: request.model.clone(),
        input: request.input.clone(),
        messages: request.messages.clone(),
        extra: request.extra.clone(),
        attachments,
        enqueued_at: std::time::Instant::now(),
    }
}

pub(crate) fn user_input_message_from_payload(
    input: &ConversationInputPayload,
) -> Result<AgentInputMessage, String> {
    let attachments = input
        .attachments
        .iter()
        .map(|attachment| {
            let object = attachment
                .as_object()
                .ok_or_else(|| "invalid chat attachment: expected an object".to_string())?;
            let required = |field: &str| {
                object
                    .get(field)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| format!("invalid chat attachment: {field} is required"))
            };
            Ok(rua_channel::core::console::AttachmentInfo {
                url: required("url")?,
                name: required("name")?,
                mime_type: object
                    .get("mime_type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                stored_name: object
                    .get("stored_name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                size: object.get("size").and_then(serde_json::Value::as_u64),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(AgentInputMessage {
        role: "user".to_string(),
        content: rua_channel::core::console::ordered_user_content_parts(
            &input.message,
            input.input.as_ref(),
            input.messages.as_ref(),
            &attachments,
        ),
    })
}

async fn find_project_chat_owner_on_target(
    data_dir: &Path,
    catalog: &rua_project::ProjectCatalog,
    requested_project_id: &ProjectId,
    chat_id: &str,
) -> Result<Option<ProjectId>, String> {
    for project in catalog.list().map_err(|error| error.to_string())? {
        if project.project_id == *requested_project_id {
            continue;
        }
        let address = ChatAddress::project(project.project_id.clone(), chat_id);
        let scope_root = ChatScopeStore::resolve(data_dir, address)
            .root()
            .to_path_buf();
        if crate::services::chat_index::get(&scope_root, chat_id)
            .await?
            .is_some()
            || rua_session::WorkspaceSessionStore::new(&scope_root)
                .load_chat_header(chat_id)
                .await
                .map_err(|error| error.to_string())?
                .is_some()
        {
            return Ok(Some(project.project_id));
        }
    }
    Ok(None)
}

fn build_authority_pending_message(
    request: &ConsoleChatRequest,
    authority_device_id: &str,
    authority_epoch: ConversationAuthorityEpoch,
) -> ConversationInputPayload {
    let mut pending = build_input_payload(request);
    pending
        .extra
        .remove(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY);
    pending.model_override = None;
    pending.extra.remove("chat_model_slot");
    pending.extra.insert(
        "authority_device_id".to_string(),
        serde_json::json!(authority_device_id),
    );
    pending.extra.insert(
        "authority_epoch".to_string(),
        serde_json::json!(authority_epoch),
    );
    pending
}

fn pending_from_remote_request(
    request: &RemoteConversationInputRequest,
) -> ConversationInputPayload {
    ConversationInputPayload {
        message: request.text.clone(),
        model_override: None,
        input: request.input.clone(),
        messages: None,
        extra: pending_extra_from_remote_request(request),
        attachments: Vec::new(),
        enqueued_at: std::time::Instant::now(),
    }
}

fn pending_extra_from_remote_request(
    request: &RemoteConversationInputRequest,
) -> serde_json::Map<String, serde_json::Value> {
    let mut extra = request.metadata.clone();
    extra.remove(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY);
    extra.remove(CHAT_MODEL_META_KEY);
    extra.remove("chat_model_slot");
    extra.insert(
        crate::services::conversation_authority::SUBMISSION_ID_KEY.to_string(),
        serde_json::json!(request.submission_id),
    );
    extra.insert(
        "authority_device_id".to_string(),
        serde_json::json!(request.target_device_id),
    );
    extra.insert(
        "authority_epoch".to_string(),
        serde_json::json!(request.authority_epoch),
    );
    extra.insert(
        "device_delivery".to_string(),
        serde_json::json!({
            "device_delivery_id": request.device_delivery_id,
            "device_delivery_sequence": request.device_delivery_sequence,
            "source_device_id": request.source_device_id,
            "target_device_id": request.target_device_id,
        }),
    );
    extra
}

fn conversation_input_ack(
    request: &RemoteConversationInputRequest,
    accepted_at: String,
    input_id: String,
    mode: ConversationInputAcceptanceMode,
    control: Option<Box<crate::services::conversation_authority::ConversationControlView>>,
) -> RemoteConversationInputAck {
    RemoteConversationInputAck {
        submission_id: request.submission_id.clone(),
        input_id,
        mode,
        device_delivery_id: request.device_delivery_id.clone(),
        target_device_id: request.target_device_id.clone(),
        accepted_at,
        control,
    }
}

fn remote_media_path(workspace_dir: &Path, filename: &str) -> std::result::Result<PathBuf, String> {
    if filename.is_empty()
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains("..")
        || filename == "."
        || filename == ".."
    {
        return Err(format!("invalid remote media filename: {filename}"));
    }
    Ok(workspace_dir.join("media").join(filename))
}

struct PersistedRemoteInputAcceptance {
    input_id: String,
    mode: ConversationInputAcceptanceMode,
    turn_id: String,
    turn_status: String,
}

async fn device_delivery_acceptance_locally(
    workspace_dir: &Path,
    request: &RemoteConversationInputRequest,
) -> std::result::Result<Option<PersistedRemoteInputAcceptance>, String> {
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let turns = store
        .read_turn_range(request.chat_id(), 1, usize::MAX)
        .await
        .map_err(|error| error.to_string())?;
    for turn in turns {
        let messages = store
            .read_turn_messages(request.chat_id(), &turn.turn_id)
            .await
            .map_err(|error| error.to_string())?;
        let acceptance = messages.iter().find_map(|message| {
            let belongs_to_device_delivery = message
                .metadata
                .get("device_delivery_id")
                .and_then(|value| value.as_str())
                == Some(request.device_delivery_id.as_str());
            if !belongs_to_device_delivery {
                return None;
            }
            let input_id = message.metadata.get(INPUT_ID_KEY)?.as_str()?.to_string();
            let mode =
                serde_json::from_value(message.metadata.get(ACCEPTANCE_MODE_KEY)?.clone()).ok()?;
            Some(PersistedRemoteInputAcceptance {
                input_id,
                mode,
                turn_id: turn.turn_id.clone(),
                turn_status: turn.lifecycle.status.clone(),
            })
        });
        if acceptance.is_some() {
            return Ok(acceptance);
        }
    }
    Ok(None)
}
