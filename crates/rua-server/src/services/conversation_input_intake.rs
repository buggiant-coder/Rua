use std::collections::{BTreeSet, HashMap};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures::FutureExt;
use rua_acp::client::AcpClientAdapter;
use rua_channel::core::console::ConsoleChatRequest;
use rua_contract::agent::AgentInputMessage;
use rua_core::{ChatAddress, ChatStore, ProjectId};

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
use crate::services::conversation_authority_target::load_optional_chat_authority_placement;
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
    project_catalog: Option<Arc<rua_project::ProjectCatalog>>,
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
    pub project_member: bool,
    pub chat_agent_state: Option<crate::services::project_chat::ChatAgentState>,
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
    Unconfirmed {
        chat_id: String,
        detail: String,
        submission_id: String,
        device_delivery_id: String,
        target_device_id: String,
        device_delivery_sequence: u64,
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
    fn remote_error(
        request: &RemoteConversationInputRequest,
        error: DeviceCommandDeliveryError,
    ) -> Self {
        let unconfirmed = matches!(error.failure_kind(), "device_offline" | "protocol_error")
            || matches!(&error, DeviceCommandDeliveryError::Rejected { reason, .. } if reason == "unexpected_response");
        if unconfirmed {
            Self::Unconfirmed {
                chat_id: request.chat_id().to_string(),
                detail: error.to_string(),
                submission_id: request.submission_id.clone(),
                device_delivery_id: request.device_delivery_id.clone(),
                target_device_id: request.target_device_id.clone(),
                device_delivery_sequence: request.device_delivery_sequence,
            }
        } else {
            Self::Rejected {
                detail: error.to_string(),
                target_device_id: Some(request.target_device_id.clone()),
                failure_kind: Some(error.failure_kind().to_string()),
            }
        }
    }

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
    pub(crate) async fn optional_chat_header(
        &self,
        address: &ChatAddress,
    ) -> Result<Option<rua_session::ChatSessionHeader>, String> {
        let root = self.session_sync.rua_root();
        rua_session::WorkspaceSessionStore::new(root.join("chats"))
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn chat_header(
        &self,
        address: &ChatAddress,
    ) -> Result<rua_session::ChatSessionHeader, String> {
        self.optional_chat_header(address)
            .await?
            .ok_or_else(|| format!("Chat not found: {}", address.chat_id()))
    }

    async fn chat_selection(
        &self,
        address: &ChatAddress,
    ) -> Result<(rua_core::AgentId, Option<rua_core::ProjectId>), String> {
        let header = self.chat_header(address).await?;
        Ok((
            rua_core::AgentId::new(header.agent_id),
            header.project_id.map(rua_core::ProjectId::new),
        ))
    }

    pub(crate) async fn chat_project_id(
        &self,
        address: &ChatAddress,
    ) -> Result<Option<rua_core::ProjectId>, String> {
        Ok(self
            .optional_chat_header(address)
            .await?
            .and_then(|header| header.project_id)
            .map(rua_core::ProjectId::new))
    }

    pub(crate) async fn live_execution_agent(
        &self,
        address: &ChatAddress,
    ) -> Result<Option<String>, String> {
        if let Some(header) = self.optional_chat_header(address).await? {
            return Ok(Some(header.agent_id));
        }
        Ok(self
            .remote_input_delivery
            .read_source_pending(address)
            .await?
            .map(|pending| pending.agent_id))
    }

    pub(crate) async fn project_work_admission(
        &self,
        address: &ChatAddress,
    ) -> Result<Option<rua_project::ProjectAdmissionGuard<'_>>, String> {
        match self.chat_project_id(address).await?.as_ref() {
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

    async fn submission_directory(
        &self,
        header: Option<&rua_session::ChatSessionHeader>,
        address: &ChatAddress,
        device_id: &str,
        extra: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<rua_session::ChatExecutionDirectory, String> {
        if let Some(header) = header {
            if let Some(directory) =
                crate::services::chat_context::saved_directory(header, device_id)?
            {
                crate::services::execution_directory::validate(
                    &directory.path,
                    self.session_sync.rua_root(),
                )
                .await?;
                return Ok(directory);
            }
            return crate::services::chat_context::select_execution_directory(
                self.session_sync.rua_root(),
                address,
                device_id,
                None,
            )
            .await;
        }
        crate::services::chat_context::initial_execution_directory(
            self.session_sync.rua_root(),
            address,
            device_id,
            extra,
        )
        .await
    }

    async fn execution_working_directory(
        &self,
        chat_id: &str,
        agent_id: &str,
        device_id: &str,
    ) -> Result<PathBuf, String> {
        let runtime = self
            .registry
            .get_or_create(agent_id)
            .await
            .map_err(|error| error.to_string())?;
        crate::services::chat_context::working_directory_on_device(
            &runtime.chat_workspace_dir(),
            chat_id,
            device_id,
            runtime.workspace_dir(),
        )
        .await
    }

    pub(crate) async fn resolve_collaboration_working_dir(
        &self,
        address: &ChatAddress,
        local_device_id: &str,
    ) -> Result<PathBuf, String> {
        let (agent_id, project_id) = self.chat_selection(address).await?;
        if project_id.is_some() {
            self.resolve_project_conversation_scope_target(address, local_device_id)
                .await?;
        }
        self.execution_working_directory(
            address.chat_id().as_str(),
            agent_id.as_str(),
            local_device_id,
        )
        .await
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
            project_catalog: None,
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

    pub fn with_project_runtime(mut self, catalog: Arc<rua_project::ProjectCatalog>) -> Self {
        self.project_catalog = Some(catalog);
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
        let root = self.session_sync.rua_root();
        let Some(authority) = self.conversation_authority.as_ref() else {
            return;
        };
        let scope = ChatStore::resolve(root, address.clone());
        if let Ok(Some(header)) = rua_session::WorkspaceSessionStore::new(scope.root())
            .load_chat_header(address.chat_id().as_str())
            .await
        {
            if authority.is_draining(&header.agent_id).await {
                return;
            }
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
        if notification.projection == rua_task::TaskLiveOutputProjection::ActivityOnly {
            let (source, _) = self
                .runtime_observation_execution(&address, &local_device_id)
                .await?;
            let store = rua_session::WorkspaceSessionStore::new(&source.scope_root);
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
        self.deliver_runtime_observation(address, local_device_id, runtime_note, extra)
            .await
    }

    pub(crate) async fn deliver_task_result(
        &self,
        record: rua_task::TaskRecord,
        tasks: Arc<rua_task::TaskRuntime>,
    ) -> Result<(), String> {
        let root = self.session_sync.rua_root();
        let scope = ChatStore::resolve(root, record.spec.owner.chat_address.clone());
        let store = rua_session::WorkspaceSessionStore::new(scope.root());
        let note = rua_agent::task_runtime_notes::task_terminal_note(&record, &store)
            .await
            .map_err(|error| error.to_string())?;
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
        let root = self.session_sync.rua_root();
        let scope = ChatStore::resolve(root, address.clone());
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
        if crate::services::chat_context::load_live_chat_header(root, address.chat_id().as_str())
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
            let agent_id = header.agent_id.clone();
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
        let mut extra = serde_json::Map::new();
        extra.insert(
            crate::services::background_work::BACKGROUND_RESULT_KEY.into(),
            serde_json::Value::Bool(true),
        );
        self.deliver_runtime_observation(address, local_device_id, note, extra)
            .await
    }

    async fn runtime_observation_execution(
        &self,
        address: &ChatAddress,
        local_device_id: &str,
    ) -> Result<(ConversationExecutionContext, rua_session::ChatSessionHeader), String> {
        let header = self.chat_header(address).await?;
        let execution_agent_id =
            crate::services::project_chat::selection_from_header(&header)?.agent_id;
        let runtime = self
            .registry
            .get_or_create(&execution_agent_id)
            .await
            .map_err(|error| error.to_string())?;
        let scope_root = runtime.chat_workspace_dir();
        if header.project_id.is_some() {
            self.resolve_project_conversation_scope_target(address, local_device_id)
                .await?;
        }
        let working_dir =
            crate::services::chat_context::saved_working_directory(Some(&header), local_device_id)?
                .unwrap_or_else(|| runtime.workspace_dir());
        Ok((
            ConversationExecutionContext {
                address: address.clone(),
                execution_agent_id,
                scope_root,
                working_dir,
            },
            header,
        ))
    }

    async fn deliver_runtime_observation(
        &self,
        address: ChatAddress,
        local_device_id: String,
        note: rua_session::RuntimeNote,
        mut extra: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
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
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or_else(|| "Conversation Authority is unavailable".to_string())?;
        // A Task may outlive its executor's selection. Choose its result Turn's
        // Agent under the same gate that accepts the input and switches the Chat.
        let gate = authority.transition_gate(&address);
        let transition = gate.lock().await;
        let (execution, header) = self
            .runtime_observation_execution(&address, &local_device_id)
            .await?;
        let scope_root = &execution.scope_root;
        let execution_agent_id = execution.execution_agent_id.as_str();
        let placement =
            crate::services::conversation_authority_target::load_chat_authority_placement_target(
                scope_root,
                address.chat_id().as_str(),
            )
            .await?;
        if placement.device_id != local_device_id {
            return Err("Runtime observation is not on the current Conversation Authority".into());
        }
        let (driver, driver_detail) = self
            .conversation_driver_for(
                execution_agent_id,
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
        let configuration_key = crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY;
        let configuration = header.extra.get(configuration_key);
        if let Some(configuration) = configuration {
            extra.insert(configuration_key.into(), configuration.clone());
        }
        let assignment = match configuration
            .and_then(|c| c.get("agent_assignment"))
            .filter(|a| !a.is_null())
        {
            Some(assignment) => assignment.clone(),
            None => serde_json::to_value(
                self.resolve_agent_turn_assignment(execution_agent_id)
                    .await?,
            )
            .map_err(|error| error.to_string())?,
        };
        extra.insert(rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.into(), assignment);
        let pending = ConversationInputPayload {
            message: String::new(),
            model_override: None,
            input: None,
            messages: None,
            extra,
            attachments: Vec::new(),
            enqueued_at: std::time::Instant::now(),
        };
        let acceptance = authority
            .accept_submission_inner_locked(
                &transition,
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
                true,
            )
            .await?;
        drop(transition);
        match acceptance {
            ConversationSubmissionAcceptance::Started {
                producer_id,
                cancel_rx,
                ..
            } => {
                self.spawn_console_producer(key, producer_id, cancel_rx, execution);
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
        _local_device_id: &str,
    ) -> Result<ProjectConversationScopeTarget, String> {
        let project = self.chat_project_id(address).await?;
        let project_id = project
            .as_ref()
            .ok_or_else(|| "Project Conversation scope requires a Project id".to_string())?;
        let data_dir = self.session_sync.rua_root();
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
        Ok(ProjectConversationScopeTarget {
            scope_root: ChatStore::resolve(data_dir, address.clone())
                .root()
                .to_path_buf(),
        })
    }

    pub(crate) async fn resolve_initial_conversation_control_target(
        &self,
        address: &ChatAddress,
        local_device_id: &str,
    ) -> Result<InitialConversationControlTarget, String> {
        let project_member = self.chat_header(address).await?.project_id.is_some();
        let scope_root = if project_member {
            self.resolve_project_conversation_scope_target(address, local_device_id)
                .await?
                .scope_root
        } else {
            ChatStore::resolve(self.session_sync.rua_root(), address.clone())
                .root()
                .to_path_buf()
        };
        Ok(InitialConversationControlTarget {
            scope_root,
            project_member,
            chat_agent_state: self.chat_agent_state(address).await.ok(),
        })
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
        let (agent_id, project_id) = self
            .chat_selection(&command.address)
            .await
            .map_err(|detail| reject("chat_unavailable", detail))?;

        if matches!(&command.action, ConversationControlDeviceAction::Discover)
            && project_id.as_ref().is_none()
        {
            return Err(reject(
                "wrong_chat_scope",
                "Conversation control discovery is available only for Project Chat".to_string(),
            ));
        }
        let selecting_agent = matches!(
            command.action,
            ConversationControlDeviceAction::ViewAgentSelection
                | ConversationControlDeviceAction::SwitchAgent { .. }
        );
        let needs_working_directory = !selecting_agent
            && !matches!(
                &command.action,
                ConversationControlDeviceAction::SetExecutionDirectory { .. }
            );
        let project_discovery = project_id.as_ref().is_some()
            && matches!(&command.action, ConversationControlDeviceAction::Discover);
        let (execution_agent_id, scope_root, working_dir) = match project_id.as_ref() {
            None => {
                if !selecting_agent && agent_id.as_str() != command.agent_id {
                    return Err(reject(
                        "wrong_agent_scope",
                        "Conversation control Agent does not match its Chat Address".to_string(),
                    ));
                }
                let root =
                    ChatStore::resolve(self.session_sync.rua_root(), command.address.clone())
                        .root()
                        .to_path_buf();
                let cwd = if !needs_working_directory {
                    // Selection uses durable configuration and requires no current Agent runtime.
                    root.clone()
                } else {
                    let runtime = self
                        .registry
                        .get_or_create(agent_id.as_str())
                        .await
                        .map_err(|e| reject("workspace_unavailable", e.to_string()))?;
                    crate::services::chat_context::working_directory_on_device(
                        &root,
                        chat_id,
                        &command.target_device_id,
                        runtime.workspace_dir(),
                    )
                    .await
                    .map_err(|detail| reject("chat_unavailable", detail))?
                };
                (agent_id.to_string(), root, cwd)
            }
            Some(project_id) => {
                let data_dir = self.session_sync.rua_root();
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
                if project.lifecycle != rua_project::ProjectLifecycle::Active
                    && !matches!(
                        command.action,
                        ConversationControlDeviceAction::ViewAgentSelection
                    )
                {
                    let detail = if selecting_agent {
                        "Chat Agent switch blocked by archived_project"
                    } else {
                        "Archived Project cannot accept Chat controls"
                    };
                    return Err(reject("project_archived", detail.to_owned()));
                }
                let scope_root = ChatStore::resolve(data_dir, command.address.clone())
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
                if command.agent_id != selected_agent_id && !project_discovery && !selecting_agent {
                    return Err(reject(
                        "stale_project_agent_selection",
                        format!(
                            "Project Chat Agent Selection changed from '{}' to '{}'",
                            command.agent_id, selected_agent_id
                        ),
                    ));
                }
                let cwd = if !needs_working_directory {
                    scope_root.clone()
                } else {
                    self.execution_working_directory(
                        chat_id,
                        &selected_agent_id,
                        &command.target_device_id,
                    )
                    .await
                    .map_err(|detail| reject("workspace_unavailable", detail))?
                };
                (selected_agent_id, scope_root, cwd)
            }
        };
        let placement =
            crate::services::conversation_authority_target::load_chat_authority_placement_target(
                &scope_root,
                chat_id,
            )
            .await
            .map_err(|detail| reject("authority_placement_unavailable", detail))?;
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
            if project_id.as_ref().is_some() {
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
        let project_id = self
            .chat_project_id(&target.address)
            .await
            .map_err(|detail| reject("chat_unavailable", detail))?;

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
            ConversationControlDeviceAction::ViewAgentSelection => {
                let state = self
                    .chat_agent_state(&target.address)
                    .await
                    .map_err(|e| reject("chat_agent_state_unavailable", e))?;
                Ok(ConversationControlDeviceResponse::AgentSelection { state })
            }
            ConversationControlDeviceAction::SetExecutionDirectory { device_id, path } => {
                let directory = self
                    .set_execution_directory(&target.address, &device_id, path.as_deref())
                    .await
                    .map_err(|detail| reject("execution_directory_change_rejected", detail))?;
                Ok(ConversationControlDeviceResponse::ExecutionDirectory { directory })
            }
            ConversationControlDeviceAction::SwitchAgent {
                expected_agent_id,
                agent_id,
            } => {
                let state = self
                    .switch_chat_agent(
                        &target.address,
                        &expected_agent_id,
                        &agent_id,
                        &command.source_device_id,
                        &target.placement,
                    )
                    .await
                    .map_err(|e| reject("chat_agent_switch_rejected", e))?;
                Ok(ConversationControlDeviceResponse::AgentSelection { state })
            }
            ConversationControlDeviceAction::Discover | ConversationControlDeviceAction::View => {
                let mut control = authority.control_view(&target.address, &meta).await;
                control.chat_agent_state = self.chat_agent_state(&target.address).await.ok();
                Ok(ConversationControlDeviceResponse::View {
                    control,
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
                if project_id.as_ref().is_none() {
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
                let roots = configuration
                    .capability_configuration
                    .as_ref()
                    .map(|capabilities| capabilities.project_write_roots())
                    .unwrap_or_default();
                let config = configuration.security_config;
                let policy = config
                    .extra
                    .contains_key(rua_security::PROJECT_RESOURCE_POLICY_KEY)
                    .then(|| {
                        rua_security::EffectiveProjectResourcePolicy::resolve_with_roots(
                            &config,
                            &target.working_dir,
                            &roots,
                        )
                    });
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
                let common_chat = rua_chat::ChatService::from_workspace(
                    rua_core::WorkspaceRoot::new(&target.scope_root),
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
                let chat_service = rua_chat::ChatService::from_workspace(
                    rua_core::WorkspaceRoot::new(&target.scope_root),
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
                let chat_service = rua_chat::ChatService::from_workspace(
                    rua_core::WorkspaceRoot::new(&target.scope_root),
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
                let branch_address = ChatAddress::new(&chat.id);
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
                if request.owner.as_chat_address() != Some(&target.address) {
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
                    .decide_for_agent(&request.agent_id, &approval_id, decision)
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
                    if let Some(security) = self.security.as_ref() {
                        match security.config_store().load(&request.agent_id) {
                            Ok(config) => self.session_sync.emit_sync_fact(
                                crate::sync_publication::SyncPublicationFact::AgentSecurityCommitted {
                                    agent_id: request.agent_id.clone(),
                                    config: serde_json::to_value(config).unwrap_or_default(),
                                },
                            ).await,
                            Err(error) => tracing::warn!(agent_id = %request.agent_id, %error, "failed to load approved Agent security configuration"),
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
            .await
            .map_err(|error| reject("project_resources_changing", error))?;
        let draft_discovery = if matches!(
            &command.action,
            AcpChatDeviceAction::ListRunners | AcpChatDeviceAction::ListSessions { .. }
        ) {
            let runtime = self
                .registry
                .get_or_create(&command.agent_id)
                .await
                .map_err(|error| reject("workspace_unavailable", error.to_string()))?;
            rua_session::WorkspaceSessionStore::new(runtime.chat_workspace_dir())
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
                    None => Some(handle.workspace_dir.to_string_lossy().into_owned()),
                };
                let data_dir = self.session_sync.rua_root();
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
        let project_id = self
            .chat_project_id(&target.address)
            .await
            .map_err(|detail| reject("chat_unavailable", detail))?;

        if matches!(
            &command.action,
            AcpChatDeviceAction::Enter { .. } | AcpChatDeviceAction::ListSessions { .. }
        ) {
            if let Some(project_id) = project_id.as_ref() {
                self.require_existing_project(project_id)
                    .map_err(|error| reject("project_unavailable", error))?;
            }
        }
        let project_cwd = project_id
            .as_ref()
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
                let requested_cwd = project_cwd
                    .or_else(|| {
                        cwd.as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(expand_control_tilde)
                    })
                    .or_else(|| Some(target.working_dir.to_string_lossy().into_owned()));
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
                        None => Some(target.working_dir.to_string_lossy().into_owned()),
                    }
                };
                let data_dir = self.session_sync.rua_root();
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

    pub async fn set_execution_directory(
        &self,
        address: &ChatAddress,
        device_id: &str,
        path: Option<&Path>,
    ) -> Result<rua_session::ChatExecutionDirectory, String> {
        if current_sync_device_id(&self.sync_device).as_deref() != Some(device_id) {
            return Err("Execution directory must be changed on the Chat authority Device".into());
        }
        let _project = self.project_work_admission(address).await?;
        let scope = ChatStore::resolve(self.session_sync.rua_root(), address.clone());
        let agent = self.chat_header(address).await?.agent_id;
        let update = ChatConfigurationUpdate::begin(self, address, &agent, scope.root()).await?;
        let approvals = self
            .approval_authority
            .as_ref()
            .ok_or("Approval Authority is unavailable")?;
        let _registration = approvals.hold_chat_registrations(address).await;
        let header = self.chat_header(address).await?;
        let placement = crate::services::conversation_authority_placement::parse_conversation_authority_placement(
            &serde_json::Value::Object(header.extra.clone())
        ).map_err(|error| error.to_string())?;
        if placement.device_id != device_id {
            return Err("Changing the execution Device requires an authority handoff".into());
        }
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or("Conversation Authority is unavailable")?;
        use crate::services::project_chat::ChatAgentSwitchBlocker;
        let mut state = crate::services::project_chat::chat_agent_state_from_header(
            address, &header, true, authority, approvals,
        )
        .await?;
        state.blockers.retain(|blocker| {
            matches!(
                blocker,
                ChatAgentSwitchBlocker::ActiveTurn
                    | ChatAgentSwitchBlocker::QueuedInput
                    | ChatAgentSwitchBlocker::UnresolvedApproval
                    | ChatAgentSwitchBlocker::ArchivedChat
            )
        });
        if let Some(id) = &header.project_id {
            let catalog = self
                .project_catalog
                .as_ref()
                .ok_or("Project catalog unavailable")?;
            if catalog
                .get(&ProjectId::new(id))
                .map_err(|error| error.to_string())?
                .is_none_or(|project| project.lifecycle != rua_project::ProjectLifecycle::Active)
            {
                state.blockers.push(ChatAgentSwitchBlocker::ArchivedProject);
            }
        }
        if !state.blockers.is_empty() {
            return Err(format!(
                "Execution directory change blocked by {}",
                serde_json::json!(state.blockers)
            ));
        }
        let directory = crate::services::chat_context::select_execution_directory(
            self.session_sync.rua_root(),
            address,
            device_id,
            path,
        )
        .await?;
        let mut next = update
            .current()
            .ok_or("Chat configuration is unavailable")?
            .clone();
        let capabilities = next
            .capability_configuration
            .as_mut()
            .ok_or("Chat configuration requires captured capabilities")?;
        let value = serde_json::json!(directory);
        let changed = capabilities.extra.get(rua_session::EXECUTION_DIRECTORY_KEY) != Some(&value);
        if changed {
            capabilities
                .extra
                .insert(rua_session::EXECUTION_DIRECTORY_KEY.into(), value);
            next.epoch = format!("config-{}", uuid::Uuid::new_v4());
            next.validate()?;
        }
        let created = crate::services::chat_context::create_managed_directory(&directory).await?;
        if changed {
            if let Err(error) = update.replace(&next).await {
                // replace can fail after it commits (for example, during sync).
                // Preserve the directory if readers already observe the new head.
                if let Some(path) = created {
                    if let Ok(header) = self.chat_header(address).await {
                        if header.extra.get(rua_session::EXECUTION_DIRECTORY_KEY)
                            != Some(&serde_json::json!(directory))
                        {
                            let _ = tokio::fs::remove_dir(path).await;
                        }
                    }
                }
                return Err(error);
            }
        }
        rua_chat::ChatService::from_workspace(rua_core::WorkspaceRoot::new(scope.root()))
            .repair_projection(address)
            .await
            .map_err(|error| error.to_string())?;
        Ok(directory)
    }

    pub async fn switch_chat_agent(
        &self,
        address: &ChatAddress,
        expected_agent_id: &str,
        selected_agent_id: &str,
        source_device_id: &str,
        placement: &ConversationAuthorityPlacement,
    ) -> Result<crate::services::project_chat::ChatAgentState, String> {
        let _resource_admission = self.project_work_admission(address).await?;
        let chat_id = address.chat_id().as_str();
        let scope_root = ChatStore::resolve(self.session_sync.rua_root(), address.clone())
            .root()
            .to_path_buf();
        let authority = self
            .conversation_authority
            .as_ref()
            .ok_or("Conversation Authority is unavailable")?;
        let approvals = self
            .approval_authority
            .as_ref()
            .ok_or("Approval Authority is unavailable")?;
        let operation = crate::services::project_chat::ChatAgentSwitch::begin(
            &scope_root,
            address,
            expected_agent_id,
            selected_agent_id,
            authority,
            approvals,
        )
        .await?;
        let saved = crate::services::conversation_authority_placement::parse_conversation_authority_placement(&serde_json::Value::Object(operation.header.extra.clone())).map_err(|e| e.to_string())?;
        if placement.device_id != saved.device_id
            || placement.authority_epoch != saved.authority_epoch
        {
            return Err("Chat authority changed before the Agent switch".into());
        }
        if let Some(project_id) = operation.header.project_id.as_ref() {
            let project = self
                .project_catalog
                .as_ref()
                .ok_or("Project Catalog unavailable")?
                .get(&ProjectId::new(project_id))
                .map_err(|e| e.to_string())?
                .ok_or("Project not found")?;
            if project.lifecycle != rua_project::ProjectLifecycle::Active {
                return Err("Chat Agent switch blocked by archived_project".into());
            }
        }
        let selected_agent = self
            .agent_service
            .as_ref()
            .ok_or("Agent configuration service is unavailable")?
            .get_agent(selected_agent_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Agent not found: {selected_agent_id}"))?;
        if !selected_agent.enabled {
            return Err(format!("Agent '{selected_agent_id}' is disabled"));
        }
        if operation.state.selection.agent_id == selected_agent_id {
            return Ok(operation.state.clone());
        }
        let hooks = self
            .product_hooks
            .as_ref()
            .ok_or("Chat configuration service unavailable")?;
        let running = rua_config::ChatRunningConfig::from_agent_defaults(
            &self
                .authority_running_config_typed(selected_agent_id)
                .await
                .map_err(|e| e.to_string())?,
        );
        let current = crate::services::product_hooks::ChatHookConfiguration::from_persisted(
            operation
                .header
                .extra
                .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
                .ok_or("Chat configuration snapshot is unavailable")?
                .clone(),
        )?;
        let materialization = self
            .chat_configuration_materialization(
                crate::services::console_chat::turn_agent_assignment_from_agent(&selected_agent)?,
                &operation.header,
                running,
                &current,
            )
            .await?;
        let configuration = hooks
            .resolve_complete_chat_configuration(address, materialization)
            .await?;
        // Placement, model and ACP stay fixed across the switch, so capture the control
        // fields before the durable commit. Publication failures cannot undo a committed selection.
        let meta = crate::services::conversation_authority_target::load_chat_control_meta(
            &scope_root,
            chat_id,
        )
        .await?
        .ok_or("Chat disappeared before Agent selection")?;
        let state = operation.commit(&configuration, source_device_id).await?;
        hooks.adopt_chat_configuration(address, configuration).await;
        authority
            .publish_chat_agent_state_snapshot(address, &meta, state.clone())
            .await;
        match crate::services::chat_metadata_store::load_chat_record(&scope_root, chat_id).await {
            Ok(Some(record)) => {
                self.publish_authority_chat_metadata_for(&scope_root, address, &record)
                    .await
            }
            Ok(None) => {
                tracing::debug!(%address, "Chat deleted before Agent switch Sync publication")
            }
            Err(error) => {
                tracing::warn!(%address, %error, "Could not publish switched Chat metadata")
            }
        }
        self.remote_input_delivery
            .spawn_priority_sync_to_device(source_device_id);
        Ok(state)
    }

    pub async fn chat_agent_state(
        &self,
        address: &ChatAddress,
    ) -> Result<crate::services::project_chat::ChatAgentState, String> {
        let header = self.chat_header(address).await?;
        let selected_agent_available = match self.agent_service.as_ref() {
            Some(service) => service
                .get_agent(&header.agent_id)
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
        let mut state = crate::services::project_chat::chat_agent_state_from_header(
            address,
            &header,
            selected_agent_available,
            authority,
            approval_authority,
        )
        .await?;
        if let Some(project_id) = header.project_id.map(ProjectId::new) {
            if self
                .project_catalog
                .as_ref()
                .and_then(|catalog| catalog.get(&project_id).ok().flatten())
                .is_some_and(|project| project.lifecycle == rua_project::ProjectLifecycle::Archived)
            {
                state
                    .blockers
                    .push(crate::services::project_chat::ChatAgentSwitchBlocker::ArchivedProject);
                state.switchable = false;
            }
        }
        Ok(state)
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
        let data_dir = self.session_sync.rua_root();
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
        let scope_root = ChatStore::resolve(
            data_dir,
            ChatAddress::new("00000000-0000-4000-8000-000000000000"),
        )
        .root()
        .to_path_buf();
        // Snapshot before mutation; never paginate a list that is shrinking as we archive.
        let ids = match chat_ids {
            Some(ids) => ids,
            None => crate::services::chat_index::list_all(&scope_root)
                .await?
                .into_iter()
                .filter(|chat| {
                    !chat.archived
                        && chat
                            .meta
                            .get("project_id")
                            .and_then(serde_json::Value::as_str)
                            == Some(project_id.as_str())
                })
                .map(|chat| chat.id)
                .collect(),
        };
        let mut seen = std::collections::HashSet::new();
        let mut items = Vec::new();
        for chat_id in ids.into_iter().filter(|id| seen.insert(id.clone())) {
            let outcome = self
                .archive_idle_project_chat(project_id, &scope_root, &chat_id, source_device_id)
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
        source_device_id: &str,
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
        let admission = match catalog.begin_exclusive_admission(project_id) {
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
        let address = ChatAddress::new(chat_id);
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
        if header.project_id.as_deref() != Some(project_id.as_str()) {
            return Err("Chat does not belong to this Project".into());
        }
        if header.metadata.archived {
            return Ok(None);
        }
        let local_device_id = current_sync_device_id(&self.sync_device)
            .ok_or("Local Device identity is unavailable")?;
        let placement = crate::services::conversation_authority_placement::parse_conversation_authority_placement(
            &serde_json::Value::Object(header.extra.clone()),
        ).map_err(|e| e.to_string())?;
        if placement.device_id != local_device_id {
            if source_device_id != local_device_id {
                return Err("Chat authority placement changed; retry after sync".into());
            }
            drop(transition);
            drop(admission);
            use crate::services::device_command_bus::{
                deliver_project_device_command_via_session, ProjectDeviceAction,
                ProjectDeviceCommand, ProjectDeviceResponse,
            };
            let manager = crate::services::device_session_access::current_session_manager(
                self.device_network(),
            )
            .await?;
            let response = deliver_project_device_command_via_session(
                &manager,
                &ProjectDeviceCommand {
                    command_id: format!("project-archive-chat-{}", uuid::Uuid::new_v4()),
                    source_device_id: local_device_id,
                    target_device_id: placement.device_id,
                    action: ProjectDeviceAction::ArchiveIdleChats {
                        project_id: project_id.clone(),
                        chat_ids: Some(vec![chat_id.to_string()]),
                    },
                },
                std::time::Duration::from_secs(10),
            )
            .await
            .map_err(|e| e.to_string())?;
            let ProjectDeviceResponse::ChatsArchived { result } = response else {
                return Err("Invalid response to Chat archive".into());
            };
            let [item] = result.items.as_slice() else {
                return Err("Invalid number of Chat archive results".into());
            };
            if item.chat_id != chat_id {
                return Err("Chat archive result belongs to another Chat".into());
            }
            use crate::services::project_chat::ProjectChatArchiveStatus;
            return match item.status {
                ProjectChatArchiveStatus::Archived => Ok(None),
                ProjectChatArchiveStatus::Skipped => Ok(Some(
                    item.reason
                        .clone()
                        .unwrap_or_else(|| "Chat archive skipped".into()),
                )),
                ProjectChatArchiveStatus::Failed => Err(item
                    .reason
                    .clone()
                    .unwrap_or_else(|| "Chat archive failed".into())),
            };
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
        let data_dir = self.session_sync.rua_root();
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
        let address = ChatAddress::new(chat_id);
        if self.chat_project_id(&address).await?.as_ref() != Some(project_id) {
            return Err(format!("Chat not found in Project: {chat_id}"));
        }
        let scope_root = ChatStore::resolve(data_dir, address.clone())
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
            let selection =
                crate::services::project_chat::load_chat_agent_selection(&scope_root, chat_id)
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

        let data_dir = self.session_sync.rua_root();
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
        let address = ChatAddress::new(chat_id);
        let scope_root = ChatStore::resolve(data_dir, address.clone())
            .root()
            .to_path_buf();
        if self
            .chat_project_id(&address)
            .await
            .map_err(ProjectChatDeleteError::Failed)?
            .as_ref()
            != Some(project_id)
        {
            return Err(ProjectChatDeleteError::NotFound(format!(
                "Chat not found in Project: {chat_id}"
            )));
        }
        let deleting = BTreeSet::from([chat_id.to_string()]);
        let storage_snapshot =
            crate::services::chat_storage::scan_scope_for_deletion(&scope_root, &deleting)
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
        for media_id in &removable_media {
            self.session_sync
                .emit_media_deleted_for_checked(media_id)
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
        let data_dir = self.session_sync.rua_root();
        let workspace = rua_core::RuaRoot::new(data_dir).chat_workspace();
        let store = rua_session::WorkspaceSessionStore::new(workspace.as_path());
        let mut addresses = Vec::new();
        let sessions = workspace.sessions_dir();
        if sessions.is_dir() {
            let mut entries = tokio::fs::read_dir(sessions)
                .await
                .map_err(|e| e.to_string())?;
            while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
                let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if uuid::Uuid::parse_str(&id).is_err()
                    || !entry.file_type().await.map_err(|e| e.to_string())?.is_dir()
                {
                    continue;
                }
                let header = store
                    .load_chat_header(&id)
                    .await
                    .map_err(|e| e.to_string())?;
                if header
                    .as_ref()
                    .and_then(|header| header.project_id.as_deref())
                    == Some(project_id.as_str())
                {
                    addresses.push(ChatAddress::new(id));
                }
            }
        }
        Ok((workspace.as_path().to_path_buf(), addresses))
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

        let mut matched = Vec::new();
        for address in addresses {
            let header = store
                .load_chat_header(address.chat_id().as_str())
                .await
                .map_err(|error| error.to_string())?;
            if header
                .as_ref()
                .and_then(|header| header.project_id.as_deref())
                != Some(project_id.as_str())
            {
                continue;
            }
            matched.push(address);
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
                    matched.iter().map(|address| authority.active_turn(address)),
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

        let ids = matched
            .iter()
            .map(|address| address.chat_id().to_string())
            .collect();
        let snapshot =
            crate::services::chat_storage::scan_scope_for_deletion(scope_root, &ids).await?;
        let (removable_media, _) = crate::services::chat_storage::removable_media(&snapshot, &ids);
        crate::services::chat_storage::remove_local_attachments(
            scope_root,
            &snapshot,
            &ids,
            &removable_media,
        )?;
        for address in &matched {
            approvals
                .delete_chat_rules(address)
                .await
                .map_err(|error| error.to_string())?;
            store
                .delete_chat_storage_by_chat_id(address.chat_id().as_str())
                .await
                .map_err(|error| error.to_string())?;
            crate::services::chat_index::delete(scope_root, address.chat_id().as_str()).await?;
        }
        Ok(ProjectPermanentDeleteResult {
            project_id: project_id.clone(),
            deleted_chat_count: matched.len(),
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
            self.session_sync.rua_root(),
            self.project_catalog
                .as_deref()
                .ok_or("Project Catalog is unavailable")?,
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
        let data_dir = self.session_sync.rua_root();
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
            let scope_root = ChatStore::resolve(
                data_dir,
                ChatAddress::new("00000000-0000-4000-8000-000000000000"),
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
                    crate::services::chat_index::ChatIndexFilter {
                        project_id: Some(project.project_id.to_string()),
                        agent_id: Some(agent_id.to_string()),
                        ..Default::default()
                    },
                    offset,
                    256,
                )
                .await?;
                if page.entries.is_empty() {
                    break;
                }
                let total = page.total;
                for chat in page.entries {
                    let address = ChatAddress::new(chat.id.clone());
                    let store = rua_session::WorkspaceSessionStore::new(&scope_root);
                    let Some(header) = store
                        .load_chat_header(address.chat_id().as_str())
                        .await
                        .map_err(|error| error.to_string())?
                    else {
                        continue;
                    };
                    if header.project_id.as_deref() != Some(project.project_id.as_str()) {
                        continue;
                    }
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

    pub async fn request_local_stop(&self, _agent_id: &str, chat_id: &str) -> bool {
        let address = ChatAddress::new(chat_id);
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
            let scope = ChatStore::resolve(self.session_sync.rua_root(), address.clone());
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
        _agent_id: &str,
        chat_id: &str,
    ) -> Option<ConversationActiveTurn> {
        self.conversation_authority
            .as_ref()?
            .request_interjection(&rua_core::ChatAddress::new(chat_id))
            .await
    }

    pub(crate) async fn decide_channel_approval(
        &self,
        address: &ChatAddress,
        agent_id: &str,
        approval_id: Option<&str>,
        approve: bool,
        decided_by: &str,
    ) -> Result<bool, String> {
        let root = self.session_sync.rua_root();
        let workspace = rua_core::RuaRoot::new(root).chat_workspace();
        let send = |action| {
            crate::endpoints::chats::dispatch_saved_chat_control(
                self,
                current_sync_device_id(&self.sync_device),
                agent_id,
                workspace.as_path(),
                address.chat_id().as_str(),
                action,
            )
        };
        let approval_id = match approval_id {
            Some(id) => id.to_owned(),
            None => {
                let ConversationControlDeviceResponse::ApprovalSnapshot { snapshot } =
                    send(ConversationControlDeviceAction::ApprovalSnapshot)
                        .await
                        .map_err(|e| e.to_string())?
                else {
                    return Err("Unexpected Chat approval snapshot".into());
                };
                let snapshot: rua_security::approval_authority::ApprovalSnapshot =
                    serde_json::from_value(snapshot).map_err(|error| error.to_string())?;
                let Some(pending) = snapshot
                    .pending
                    .into_iter()
                    .min_by_key(|item| item.created_at)
                else {
                    return Ok(false);
                };
                pending.id
            }
        };
        let decided_by = decided_by.to_owned();
        let decision = if approve {
            ConversationApprovalDecision::RunOnce { decided_by }
        } else {
            ConversationApprovalDecision::Reject { decided_by }
        };
        let result = send(ConversationControlDeviceAction::DecideApproval {
            approval_id,
            decision,
        })
        .await
        .map_err(|e| e.to_string())?;
        Ok(matches!(
            result,
            ConversationControlDeviceResponse::ApprovalDecided { .. }
        ))
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
        let data_dir = self.session_sync.rua_root();
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

        let workspace = rua_core::RuaRoot::new(data_dir).chat_workspace();
        let workspace_dir = workspace.as_path();
        if let Err(error) =
            crate::services::chat_context::ensure_chat_not_deleted(data_dir, &chat_id)
        {
            return ConversationInputDeliveryOutcome::Rejected {
                detail: error.to_string(),
                target_device_id: None,
                failure_kind: Some("chat_not_found".into()),
            };
        }
        let saved = match rua_session::WorkspaceSessionStore::new(workspace_dir)
            .load_chat_header(&raw_request_chat_id)
            .await
        {
            Ok(header) => header,
            Err(error) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: error.to_string(),
                    target_device_id: None,
                    failure_kind: None,
                }
            }
        };
        let selected_agent = saved
            .as_ref()
            .map(|header| header.agent_id.clone())
            .unwrap_or_else(|| agent_id.to_string());
        let agent_id = selected_agent.as_str();
        let requested_project =
            crate::services::chat_context::requested_project(request.extra.get("project_id"));
        let project_id = match requested_project.and_then(|requested| {
            if let Some(header) = saved.as_ref() {
                crate::services::chat_context::check_project_membership(
                    header.project_id.as_deref(),
                    requested,
                )?;
                Ok(header.project_id.clone())
            } else {
                Ok(requested.flatten().map(str::to_string))
            }
        }) {
            Ok(project_id) => project_id,
            Err(error) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: error.to_string(),
                    target_device_id: None,
                    failure_kind: Some("invalid_request".into()),
                }
            }
        };
        request
            .extra
            .insert("agent_id".into(), serde_json::json!(agent_id));
        request
            .extra
            .insert("project_id".into(), serde_json::json!(project_id));
        if let Some(project_id) = project_id {
            return self
                .submit_project_delivery(&rua_core::ProjectId::new(project_id), agent_id, request)
                .await;
        }
        let local_device_id = current_sync_device_id(&self.sync_device);
        let address = rua_core::ChatAddress::new(&chat_id);
        if saved.is_none() {
            match self
                .remote_input_delivery
                .read_source_pending(&address)
                .await
            {
                Ok(Some(pending)) => {
                    let Some(local) = local_device_id.as_deref() else {
                        return ConversationInputDeliveryOutcome::Rejected {
                            detail: "Source Device identity is unavailable".into(),
                            target_device_id: Some(pending.target_device_id),
                            failure_kind: Some("delivery".into()),
                        };
                    };
                    let expected = request
                        .extra
                        .get("expected_authority_epoch")
                        .and_then(serde_json::Value::as_str);
                    if expected.is_some_and(|value| value != pending.authority_epoch.to_string())
                        || pending.base_manifest_hash.is_none()
                    {
                        return ConversationInputDeliveryOutcome::Rejected {
                            detail:
                                "Chat authority placement is not ready; retry after synchronization"
                                    .into(),
                            target_device_id: Some(pending.target_device_id),
                            failure_kind: Some("chat_not_ready".into()),
                        };
                    }
                    return self
                        .send_routed_remote_input(RemoteConversationInputParams {
                            address: &address,
                            execution_agent_id: &pending.agent_id,
                            request: &request,
                            source_device_id: local,
                            target_device_id: &pending.target_device_id,
                            authority_epoch: pending.authority_epoch,
                            base_manifest_hash: pending
                                .base_manifest_hash
                                .as_deref()
                                .expect("validated receipt hash"),
                            workspace_dir: Some(workspace_dir),
                            materialize_conversation: false,
                        })
                        .await;
                }
                Err(detail) => {
                    return ConversationInputDeliveryOutcome::Rejected {
                        detail,
                        target_device_id: None,
                        failure_kind: Some("chat_not_ready".into()),
                    }
                }
                Ok(None) => {}
            }
        }
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
                failure_kind: Some("chat_not_ready".into()),
            };
        }
        let first_submission = match crate::services::chat_index::get(workspace_dir, &chat_id).await
        {
            Ok(indexed) => indexed.is_none() && saved.is_none(),
            Err(error) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: format!("chat index unavailable: {error}"),
                    target_device_id: None,
                    failure_kind: None,
                }
            }
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
            let target_device_id = match crate::services::execution_directory::select_draft(
                &mut request.extra,
                None,
                local_device_id,
            ) {
                Ok(device) => device,
                Err(detail) => {
                    return ConversationInputDeliveryOutcome::Rejected {
                        detail,
                        target_device_id: None,
                        failure_kind: Some("invalid_request".into()),
                    }
                }
            };
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
        let captured_assignment = match saved.and_then(|header| {
            header
                .extra
                .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
                .cloned()
        }) {
            Some(value) => {
                match crate::services::product_hooks::ChatHookConfiguration::from_persisted(value) {
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
            let chat_service = rua_chat::ChatService::from_workspace(workspace.clone());
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
                workspace_dir: Some(workspace_dir),
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
                        let directory = self.submission_directory(None,&address,&placement.device_id,&request.extra).await?;
                        request.extra.insert(rua_session::EXECUTION_DIRECTORY_KEY.into(),serde_json::json!(directory));
                        hooks
                            .resolve_complete_chat_configuration(
                                &address,
                                self.capture_chat_configuration(
                                    assignment,
                                    rua_config::ChatRunningConfig::from_agent_defaults(&running),
                                    None,
                                    crate::services::chat_configuration_capture::ChatConfigurationSelection {
                                        chat_id: address.chat_id().clone(),
                                        execution_directory: directory,
                                        project_id: None,
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
                    Some(workspace_dir.to_owned()),
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
                            Some(workspace_dir.to_owned()),
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
                    workspace_dir: Some(workspace_dir),
                    materialize_conversation,
                };
                self.send_routed_remote_input(params).await
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

    async fn send_routed_remote_input(
        &self,
        params: RemoteConversationInputParams<'_>,
    ) -> ConversationInputDeliveryOutcome {
        let request = match self
            .remote_input_delivery
            .build_remote_conversation_input_request(&params)
            .await
        {
            Ok(request) => request,
            Err(detail) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail,
                    target_device_id: Some(params.target_device_id.to_owned()),
                    failure_kind: Some("invalid_request".into()),
                }
            }
        };
        match self
            .remote_input_delivery
            .deliver_prepared_conversation_input(&request, "remote conversation input")
            .await
        {
            Ok(outcome) => ConversationInputDeliveryOutcome::remote(
                params.address.chat_id().to_string(),
                outcome,
            ),
            Err(error) => ConversationInputDeliveryOutcome::remote_error(&request, error),
        }
    }

    pub async fn submit_project_delivery(
        &self,
        project_id: &ProjectId,
        execution_agent_id: &str,
        mut request: ConsoleChatRequest,
    ) -> ConversationInputDeliveryOutcome {
        request
            .extra
            .insert("project_id".into(), serde_json::json!(project_id));
        request
            .extra
            .insert("agent_id".into(), serde_json::json!(execution_agent_id));
        let data_dir = self.session_sync.rua_root();
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
                target_device_id: None,
                failure_kind: Some("invalid_request".to_string()),
            };
        };
        let chat_id = match resolve_chat_id(raw_chat_id) {
            Ok(chat_id) => chat_id,
            Err(error) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: error,
                    target_device_id: None,
                    failure_kind: Some("invalid_request".to_string()),
                };
            }
        };
        if let Err(error) =
            crate::services::chat_context::ensure_chat_not_deleted(data_dir, &chat_id)
        {
            return ConversationInputDeliveryOutcome::Rejected {
                detail: error.to_string(),
                target_device_id: None,
                failure_kind: Some("chat_not_found".into()),
            };
        }
        if execution_agent_id.trim().is_empty() {
            return ConversationInputDeliveryOutcome::Rejected {
                detail: "execution Agent id is required".to_string(),
                target_device_id: None,
                failure_kind: Some("invalid_request".to_string()),
            };
        }
        let Some(source_device_id) = current_sync_device_id(&self.sync_device) else {
            return ConversationInputDeliveryOutcome::Rejected {
                detail: "source Device identity is unavailable".to_string(),
                target_device_id: None,
                failure_kind: Some("delivery".to_string()),
            };
        };

        let address = ChatAddress::new(&chat_id);
        let scope_root = ChatStore::resolve(data_dir, address.clone())
            .root()
            .to_path_buf();
        let saved = match self.optional_chat_header(&address).await {
            Ok(header) => header,
            Err(detail) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail,
                    target_device_id: None,
                    failure_kind: Some("chat_not_ready".into()),
                }
            }
        };
        let selected_agent_id = match saved.as_ref() {
            Some(header) => {
                if header.project_id.as_deref() != Some(project_id.as_str()) {
                    return ConversationInputDeliveryOutcome::Rejected {
                        detail: "Chat Project membership is fixed".into(),
                        target_device_id: None,
                        failure_kind: Some("invalid_request".into()),
                    };
                }
                match crate::services::project_chat::selection_from_header(header) {
                    Ok(selection) => selection.agent_id,
                    Err(detail) => {
                        return ConversationInputDeliveryOutcome::Rejected {
                            detail,
                            target_device_id: None,
                            failure_kind: Some("chat_not_ready".into()),
                        }
                    }
                }
            }
            None => execution_agent_id.trim().to_owned(),
        };
        let acknowledged = match load_optional_chat_authority_placement(&scope_root, &chat_id).await
        {
            Ok(Some(_)) => None,
            Ok(None) => match self
                .remote_input_delivery
                .read_source_pending(&address)
                .await
            {
                Ok(pending) => pending,
                Err(detail) => {
                    return ConversationInputDeliveryOutcome::Rejected {
                        detail,
                        target_device_id: None,
                        failure_kind: Some("chat_not_ready".into()),
                    }
                }
            },
            Err(detail) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail,
                    target_device_id: None,
                    failure_kind: Some("chat_not_ready".into()),
                }
            }
        };
        if saved.is_none() && acknowledged.is_none() {
            if let Err(detail) = crate::services::execution_directory::select_draft(
                &mut request.extra,
                project.execution_default.as_ref(),
                &source_device_id,
            ) {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail,
                    target_device_id: None,
                    failure_kind: Some("invalid_request".into()),
                };
            }
        }
        // A source-side receipt can arrive before the authoritative Chat header.
        // Route this interval to the Device that already accepted the Chat.
        let route = if let Some(pending) = acknowledged {
            let expected = request
                .extra
                .get("expected_authority_epoch")
                .and_then(serde_json::Value::as_str);
            if expected.is_some_and(|value| value != pending.authority_epoch.to_string())
                || pending.base_manifest_hash.is_none()
            {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: "Chat authority placement is not ready; retry after synchronization"
                        .into(),
                    target_device_id: Some(pending.target_device_id),
                    failure_kind: Some("chat_not_ready".into()),
                };
            }
            ConversationAuthorityRouter.route_fixed_placement(
                &ConversationAuthorityPlacement {
                    device_id: pending.target_device_id,
                    authority_epoch: pending.authority_epoch,
                    base_manifest_hash: pending.base_manifest_hash.expect("validated receipt hash"),
                },
                Some(&source_device_id),
            )
        } else {
            ConversationAuthorityRouter
                .resolve_submit_route(ConversationSubmitRouteRequest {
                    workspace_dir: Some(&scope_root),
                    address: &address,
                    request: &request,
                    execution_agent_id: &selected_agent_id,
                    local_device_id: Some(&source_device_id),
                    acp_chat_registry: self.acp_chat_registry.as_ref(),
                })
                .await
        };
        match route {
            ConversationSubmitRoute::LocalAuthority {
                authority_device_id,
                authority_epoch,
                base_manifest_hash,
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
                        base_manifest_hash,
                    },
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
                let params = RemoteConversationInputParams {
                    address: &address,
                    execution_agent_id: &selected_agent_id,
                    request: &request,
                    source_device_id: &source_device_id,
                    target_device_id: &target_device_id,
                    authority_epoch,
                    base_manifest_hash: &base_manifest_hash,
                    workspace_dir: Some(&scope_root),
                    materialize_conversation,
                };
                self.send_routed_remote_input(params).await
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
        let directory = match self
            .submission_directory(
                header.as_ref(),
                &address,
                &placement.device_id,
                &request.extra,
            )
            .await
        {
            Ok(directory) => directory,
            Err(error) => return reject(error),
        };
        let working_dir = match directory.host_path() {
            Ok(path) => path,
            Err(error) => return reject(error.to_string()),
        };
        request.extra.insert(
            rua_session::EXECUTION_DIRECTORY_KEY.into(),
            serde_json::json!(directory),
        );
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
            let current_chat = match rua_chat::ChatService::from_workspace(
                rua_core::WorkspaceRoot::new(&scope_root),
            )
            .load(&address)
            .await
            {
                Ok(Some(chat)) => chat,
                Ok(None) => return reject("Project Chat Session File is unavailable".to_string()),
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
                            chat_id: address.chat_id().clone(),
                            execution_directory: directory.clone(),
                            project_id: Some(project_id.to_string()),
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
                                project_id: pending.extra.get("project_id").and_then(serde_json::Value::as_str)
                                    .ok_or("Project identity is required for initial submission")?.to_string(),
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
        let workspace_dir = runtime.chat_workspace_dir();
        let placement =
            crate::services::conversation_authority_target::load_chat_authority_placement_target(
                &workspace_dir,
                chat_id,
            )
            .await?;
        let target = ConversationControlTarget {
            address: ChatAddress::new(chat_id),
            execution_agent_id: agent_id.to_string(),
            scope_root: runtime.chat_workspace_dir(),
            working_dir: crate::services::chat_context::working_directory_on_device(
                &workspace_dir,
                chat_id,
                &placement.device_id,
                runtime.workspace_dir(),
            )
            .await?,
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
            &self.session_sync.rua_root().join("chats"),
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
            &ChatAddress::new(chat_id),
            agent_id,
            &runtime.chat_workspace_dir(),
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

    pub(crate) fn require_existing_project(&self, project_id: &ProjectId) -> Result<(), String> {
        self.project_catalog
            .as_ref()
            .ok_or("Project catalog unavailable")?
            .get(project_id)
            .map_err(|e| e.to_string())?
            .ok_or("Project not found")?;
        Ok(())
    }

    fn validate_initial_acp(
        &self,
        agent_id: &str,
        _address: &ChatAddress,
        working_dir: &Path,
        extra: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Option<InitialAcpSelection>, String> {
        let Some(value) = extra.get(INITIAL_ACP_KEY) else {
            return Ok(None);
        };
        let project_id = extra
            .get("project_id")
            .and_then(serde_json::Value::as_str)
            .map(rua_core::ProjectId::new);
        if let Some(project_id) = project_id.as_ref() {
            self.require_existing_project(project_id)?;
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
        let managed = extra
            .get(rua_session::EXECUTION_DIRECTORY_KEY)
            .and_then(|directory| directory.get("managed"))
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        if !managed && !working_dir.is_dir() {
            return Err(format!(
                "initial ACP working directory does not exist: {}",
                working_dir.display()
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
        let _resource_admission = self.project_work_admission(address).await?;
        if let Some(project_id) = self.chat_project_id(address).await?.as_ref() {
            self.require_existing_project(project_id)?;
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
            &ChatAddress::new(chat_id),
            agent_id,
            &runtime.chat_workspace_dir(),
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
        let scope_root = runtime.chat_workspace_dir();
        self.update_authority_chat_model_for(
            &ChatAddress::new(chat_id),
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
        self.authority_chat_model_for(&runtime.chat_workspace_dir(), chat_id)
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
        let header = rua_session::WorkspaceSessionStore::new(runtime.chat_workspace_dir())
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
        assignment: rua_chat::TurnAgentAssignment,
        header: &rua_session::ChatSessionHeader,
        running_config: rua_config::ChatRunningConfig,
        current: &crate::services::product_hooks::ChatHookConfiguration,
    ) -> Result<crate::services::product_hooks::ChatConfigurationMaterialization, String> {
        let authority_device_id = header
            .extra
            .get("authority_placement")
            .and_then(|value| value.get("device_id"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or("Chat Authority Device missing")?
            .to_string();
        let directory =
            match crate::services::chat_context::saved_directory(header, &authority_device_id)? {
                Some(directory) => directory,
                None => {
                    self.submission_directory(
                        Some(header),
                        &ChatAddress::new(header.chat_id.as_str()),
                        &authority_device_id,
                        &Default::default(),
                    )
                    .await?
                }
            };
        let cwd = directory.host_path().map_err(|e| e.to_string())?;
        self.capture_chat_configuration(
            assignment,
            running_config,
            header.project_id.as_ref().map(|_| cwd.as_path()),
            crate::services::chat_configuration_capture::ChatConfigurationSelection {
                chat_id: rua_core::ChatId::new(header.chat_id.as_str()),
                execution_directory: directory,
                project_id: header.project_id.clone(),
                model: current.model.clone(),
                thinking: current.thinking.clone(),
                collaboration_mode: current.collaboration_mode.clone(),
                harness_configuration: current.harness_configuration.clone(),
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
            let materialization = self
                .chat_configuration_materialization(
                    self.resolve_agent_turn_assignment(agent_id).await?,
                    &header,
                    defaults,
                    &current,
                )
                .await?;
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
            .chat_workspace_dir();
        self.update_authority_running_config_for(
            &ChatAddress::new(chat_id),
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
        let workspace_dir = runtime.chat_workspace_dir();
        let record = authority
            .switch_authority(
                None,
                &workspace_dir,
                &rua_core::ChatAddress::new(chat_id),
                target,
            )
            .await
            .map_err(|error| error.to_string())?;
        self.session_sync.spawn_chat_state_publication_for(
            &workspace_dir,
            &rua_core::ChatAddress::new(chat_id),
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
                        return ConversationInputDeliveryOutcome::remote_error(&prepared, error);
                    }
                }
            }
            Err(error) => {
                return ConversationInputDeliveryOutcome::remote_error(&prepared, error);
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
        let address = rua_core::ChatAddress::new(&chat_id);
        let directory = async {
            if let Some(first) = first_submission.as_ref() {
                let value = first
                    .configuration
                    .capability_configuration
                    .as_ref()
                    .and_then(|capabilities| {
                        capabilities.extra.get(rua_session::EXECUTION_DIRECTORY_KEY)
                    })
                    .ok_or("Chat configuration requires an execution directory")?;
                let directory: rua_session::ChatExecutionDirectory =
                    serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
                return directory.host_path().map_err(|e| e.to_string());
            }
            let header = self.optional_chat_header(&address).await?;
            self.submission_directory(
                header.as_ref(),
                &address,
                route_target.device_id,
                &request.extra,
            )
            .await?
            .host_path()
            .map_err(|e| e.to_string())
        }
        .await;
        let working_dir = match directory {
            Ok(directory) => directory,
            Err(error) => {
                return ConversationInputDeliveryOutcome::Rejected {
                    detail: error.to_string(),
                    target_device_id: Some(route_target.device_id.to_string()),
                    failure_kind: None,
                }
            }
        };
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
            if let Err(detail) =
                self.validate_initial_acp(agent_id, &address, &working_dir, &pending.extra)
            {
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
                address: rua_core::ChatAddress::new(&chat_id),
                execution_agent_id: agent_id.to_string(),
                scope_root: workspace_dir.clone(),
                working_dir,
            },
            truncate_chat_name(&request.message),
            false,
            true,
        )
        .await
    }

    pub(crate) async fn validate_collaboration_project(
        &self,
        address: &ChatAddress,
    ) -> Result<(), String> {
        if let Some(project_id) = self.chat_project_id(address).await?.as_ref() {
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
        let _resource_admission = self.project_work_admission(&execution.address).await?;
        self.validate_collaboration_project(&execution.address)
            .await?;
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
                let chat_service = rua_chat::ChatService::from_workspace(
                    rua_core::WorkspaceRoot::new(scope_root.clone()),
                );
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
        let rua_root = self.session_sync.rua_root();
        crate::services::chat_context::ensure_chat_not_deleted(rua_root, request.chat_id())
            .map_err(|error| error.to_string())?;
        let header = self.optional_chat_header(&request.address).await?;
        let project_id = match header.as_ref() {
            Some(header) => {
                let requested = crate::services::chat_context::requested_project(
                    request.metadata.get("project_id"),
                )
                .map_err(|error| error.to_string())?;
                crate::services::chat_context::check_project_membership(
                    header.project_id.as_deref(),
                    requested,
                )
                .map_err(|error| error.to_string())?;
                request.agent_id = header.agent_id.clone();
                header.project_id.clone()
            }
            None => request
                .metadata
                .get("project_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        }
        .map(rua_core::ProjectId::new);
        request
            .metadata
            .insert("project_id".into(), serde_json::json!(project_id));
        let _project_admission = match project_id.as_ref() {
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
        ) = match project_id.as_ref() {
            None => {
                let runtime = self
                    .registry
                    .get_or_create(&request.agent_id)
                    .await
                    .map_err(|error| format!("workspace unavailable: {error}"))?;
                let agent_workspace_dir = runtime.chat_workspace_dir();
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
                let directory = self
                    .submission_directory(
                        header.as_ref(),
                        &request.address,
                        &request.target_device_id,
                        &request.metadata,
                    )
                    .await?;
                let working_dir = directory.host_path().map_err(|e| e.to_string())?;
                request.metadata.insert(
                    rua_session::EXECUTION_DIRECTORY_KEY.into(),
                    serde_json::json!(directory),
                );
                (
                    agent_workspace_dir,
                    working_dir,
                    placement,
                    first_submission,
                    assignment,
                    initial_model,
                )
            }
            Some(project_id) => {
                let data_dir = self.session_sync.rua_root();
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
                let scope_root = ChatStore::resolve(data_dir, request.address.clone())
                    .root()
                    .to_path_buf();
                let indexed =
                    crate::services::chat_index::get(&scope_root, request.chat_id()).await?;
                let header = rua_session::WorkspaceSessionStore::new(&scope_root)
                    .load_chat_header(request.chat_id())
                    .await
                    .map_err(|error| error.to_string())?;
                let first_submission = indexed.is_none() && header.is_none();
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
                let directory = self
                    .submission_directory(
                        header.as_ref(),
                        &request.address,
                        &request.target_device_id,
                        &request.metadata,
                    )
                    .await?;
                let working_dir = directory.host_path().map_err(|e| e.to_string())?;
                request.metadata.insert(
                    rua_session::EXECUTION_DIRECTORY_KEY.into(),
                    serde_json::json!(directory),
                );
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
                                device_id: request.target_device_id.clone(),
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
            if project_id.as_ref().is_some() {
                rua_chat::ChatService::from_workspace(rua_core::WorkspaceRoot::new(&scope_root))
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
                        if project_id.as_ref().is_some() {
                            let assignment = rua_chat::ChatService::from_workspace(
                                rua_core::WorkspaceRoot::new(&scope_root),
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
            self.schedule_accepted_remote_input_publication(&scope_root, &request, false)
                .await;
            return Ok(conversation_input_ack(
                &request,
                accepted_at,
                acceptance.input_id,
                acceptance.mode,
                self.remote_input_ack_control(&scope_root, &request.address)
                    .await,
            ));
        }
        if project_id.as_ref().is_some()
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
                                project_id.as_ref()
                                    .map(|_| working_dir.as_path()),
                                crate::services::chat_configuration_capture::ChatConfigurationSelection {
                                    chat_id: request.address.chat_id().clone(),
                                    execution_directory: serde_json::from_value(pending.extra[rua_session::EXECUTION_DIRECTORY_KEY].clone()).map_err(|e|e.to_string())?,
                                    project_id: project_id.as_ref().map(ToString::to_string),
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
                if project_id.as_ref().is_some() {
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
                                project_id: pending.extra.get("project_id").and_then(serde_json::Value::as_str)
                                    .ok_or("Project identity is required for initial submission")?.to_string(),
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
                self.schedule_accepted_remote_input_publication(&scope_root, &request, true)
                    .await;
            }
            return Ok(conversation_input_ack(
                &request,
                accepted_at,
                input.input_id,
                mode,
                self.remote_input_ack_control(&scope_root, &request.address)
                    .await,
            ));
        }
        Err("Conversation Authority is unavailable".to_string())
    }

    async fn remote_input_ack_control(
        &self,
        scope_root: &Path,
        address: &ChatAddress,
    ) -> Option<Box<crate::services::conversation_authority::ConversationControlView>> {
        // This read-only enrichment is optional. Durable acceptance must not
        // depend on metadata or a Control View becoming available promptly.
        tokio::time::timeout(
            Duration::from_millis(100),
            self.authority_control_view(scope_root, address),
        )
        .await
        .ok()
        .flatten()
    }

    async fn schedule_accepted_remote_input_publication(
        &self,
        scope_root: &Path,
        request: &RemoteConversationInputRequest,
        include_index: bool,
    ) {
        let Some(runtime) = self.registry.get(&request.agent_id).await else {
            return;
        };
        let task_id = format!(
            "remote-input-publication:{}:{}",
            request.device_delivery_id,
            uuid::Uuid::new_v4()
        );
        let cancellation = runtime
            .task_tracker()
            .register(task_id.clone(), "Publish accepted remote input".to_string())
            .await;
        // Register before checking lifecycle: shutdown either observes this token
        // in cancel_all, or this check rejects a registration made after it.
        if !runtime.can_accept_requests().await {
            runtime.task_tracker().complete(&task_id).await;
            return;
        }
        let scope_root = scope_root.to_path_buf();
        let request = request.clone();
        let session_sync = self.session_sync.clone();
        let delivery = self.remote_input_delivery.clone();
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {},
                result = tokio::time::timeout(Duration::from_secs(30), async {
                    if include_index {
                        if let Ok(Some(entry)) = crate::services::chat_index::get(&scope_root, request.chat_id()).await {
                            session_sync.emit_chat_index_upsert_for(
                                &request.address,
                                crate::services::console_chat::chat_index_entry_to_json(&entry),
                            ).await;
                        }
                    }
                    session_sync.emit_evergreen_chat_snapshot(&scope_root, &request.address).await;
                    if request.source_device_id != request.target_device_id {
                        delivery.priority_sync_to_device(&request.source_device_id).await;
                    }
                }) => {
                    if result.is_err() {
                        tracing::warn!(chat_address = %request.address, "accepted remote input publication timed out; canonical state remains committed");
                    }
                },
            }
            runtime.task_tracker().complete(&task_id).await;
        });
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
        if self
            .chat_project_id(address)
            .await
            .is_ok_and(|id| id.is_some())
        {
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
