use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use rua_core::{event::RuntimeEvent, ChatAddress};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch, Mutex as AsyncMutex, RwLock};

use crate::services::chat_metadata_store::{
    apply_confirmed_authority_placement as persist_confirmed_authority_placement,
    clear_acp_binding, persist_acp_binding, update_authority_placement, AcpBindingMetadata,
    ChatMetadataRecord, ChatMetadataStoreError,
};
use crate::services::conversation_authority_placement::{
    chat_acp_binding_active, parse_conversation_authority_placement, ConversationAuthorityEpoch,
    ConversationAuthorityPlacement, CHAT_ACP_BINDING_META_KEY, CHAT_MODEL_META_KEY,
};
use crate::services::foreground_operation::{ForegroundOperationKind, ForegroundOperationRegistry};
use crate::state::AppState;
use crate::streaming::{ActiveStreamRegistry, StreamKey};

pub const SUBMISSION_ID_KEY: &str = "submission_id";
pub const INPUT_ID_KEY: &str = "input_id";
pub const ACCEPTANCE_MODE_KEY: &str = "acceptance_mode";
pub const REGENERATE_TURN_ID_KEY: &str = "regenerate_turn_id";
pub const SUBMITTED_USER_TURN_PERSISTED_KEY: &str = "submitted_user_turn_persisted";
pub(crate) const FIRST_CHAT_SUBMISSION_CANDIDATE_KEY: &str = "first_chat_submission_candidate";

async fn enforce_authority_movement_policy(
    workspace_dir: &std::path::Path,
    address: &ChatAddress,
) -> Result<(), ChatMetadataStoreError> {
    let policy = rua_chat::ChatService::for_resolved_scope(workspace_dir, address.clone())
        .policy(address)
        .await
        .map_err(ChatMetadataStoreError::Session)?;
    if policy.authority_placement == rua_chat::AuthorityPlacementPolicy::ResourceBound {
        return Err(ChatMetadataStoreError::Conflict(
            "Chat authority is fixed by its Resource-bound Authority Policy".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationInputAcceptanceMode {
    Started,
    Queued,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationControlView {
    #[serde(default)]
    pub background_work: crate::services::background_work::BackgroundWorkView,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_device_id: Option<String>,
    pub placement: ConversationAuthorityPlacementView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<ConversationActiveTurn>,
    pub queue: ConversationInputQueueView,
    pub acp: ConversationAcpControlView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_chat_agent_state: Option<crate::services::project_chat::ProjectChatAgentState>,
    pub allowed_controls: ConversationAllowedControls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationActiveTurnPhase {
    Preparing,
    Streaming,
    WaitingApproval,
    RunningTool,
    Committing,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationDriverKind {
    Rua,
    Acp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationActiveTurn {
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_device_id: Option<String>,
    pub phase: ConversationActiveTurnPhase,
    pub authority_device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub driver: ConversationDriverKind,
    pub started_at: String,
    #[serde(default)]
    pub queued_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver_detail: Option<serde_json::Value>,
}

impl ConversationActiveTurn {
    pub fn status_hint(&self) -> &'static str {
        "running"
    }
}

#[derive(Debug, Clone)]
pub struct ConversationActiveTurnStart {
    pub address: ChatAddress,
    pub driver: ConversationDriverKind,
    pub authority_device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub driver_detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct ConversationTurnBinding {
    pub address: ChatAddress,
    pub turn_id: String,
    pub user_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationTurnTerminalFact {
    pub address: ChatAddress,
    pub turn_id: String,
    pub status: String,
    pub completed_at: Option<String>,
    pub authority_device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub device_delivery_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationAuthorityPlacementLockReason {
    ActiveTurn,
    AcpPassthrough,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationAuthorityPlacementView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_epoch: Option<ConversationAuthorityEpoch>,
    pub switchable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_reason: Option<ConversationAuthorityPlacementLockReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationInputQueueView {
    #[serde(default)]
    pub entries: Vec<ConversationInputEntrySnapshot>,
    #[serde(default)]
    pub paused: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationInputEntryState {
    Queued,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationInputEntrySnapshot {
    pub input_id: String,
    pub submission_id: String,
    pub state: ConversationInputEntryState,
    pub text: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_delivery_id: Option<String>,
    #[serde(default)]
    pub editable: bool,
    #[serde(default)]
    pub deletable: bool,
}

#[derive(Debug, Clone)]
pub struct ConversationInputPayload {
    pub message: String,
    pub model_override: Option<String>,
    pub input: Option<Vec<serde_json::Value>>,
    pub messages: Option<Vec<serde_json::Value>>,
    pub extra: serde_json::Map<String, serde_json::Value>,
    pub attachments: Vec<serde_json::Value>,
    pub enqueued_at: std::time::Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationAcpControlView {
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationAllowedControls {
    pub stop: bool,
    pub queue: bool,
    pub interject: bool,
    pub edit_queued_input: bool,
    pub delete_queued_input: bool,
    pub switch_authority: bool,
    pub enter_acp: bool,
    pub exit_acp: bool,
}

pub struct ConversationAuthority {
    background_work: std::sync::OnceLock<Arc<crate::services::background_work::BackgroundWork>>,
    active_turns: RwLock<HashMap<ChatAddress, ConversationActiveTurn>>,
    placement_projections: RwLock<HashMap<ChatAddress, ConversationAuthorityPlacementProjection>>,
    queue: RwLock<HashMap<ChatAddress, VecDeque<ConversationInputEntry>>>,
    accepted_submissions: RwLock<HashMap<ChatAddress, VecDeque<ConversationInputEntrySnapshot>>>,
    paused_queue_drain: RwLock<HashSet<ChatAddress>>,
    /// Agents that have begun shutdown.
    ///
    /// This is process-wide owner state, not a Workspace queue: it is the same
    /// authority that starts Turns, so gating here is what actually stops a new
    /// Turn from being created after an Agent was reported stopped. Checking
    /// lifecycle state at each HTTP call site instead would leave the internal
    /// queue drain and Task continuations able to start work.
    draining_agents: RwLock<HashSet<String>>,
    draining_chats: RwLock<HashSet<ChatAddress>>,
    processed_control_commands: RwLock<HashMap<(ChatAddress, String), String>>,
    interjected_turns: RwLock<HashMap<ChatAddress, String>>,
    revisions: RwLock<HashMap<ChatAddress, u64>>,
    transition_gates: StdMutex<HashMap<ChatAddress, Weak<AsyncMutex<()>>>>,
    control_events: RwLock<HashMap<ChatAddress, broadcast::Sender<String>>>,
    global_control_events: broadcast::Sender<ConversationControlEvent>,
    history_replacement: crate::services::history_replacement::HistoryReplacementCoordinator,
    foreground_operations: Option<Arc<ForegroundOperationRegistry>>,
    product_hooks: Arc<crate::services::product_hooks::ProductHookService>,
    terminal_observer_runtime:
        std::sync::RwLock<Option<Arc<crate::services::terminal_observer::TerminalObserverRuntime>>>,
    terminal_observer_cancellation: tokio_util::sync::CancellationToken,
    chat_completion_notifier: std::sync::RwLock<
        Option<Arc<crate::services::chat_completion_notification::ChatCompletionNotifier>>,
    >,
    active_stream_broadcast_hub: std::sync::RwLock<
        Option<Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>>,
    >,
    gossip_broadcast: std::sync::RwLock<Option<GossipBroadcastHandles>>,
}

#[derive(Clone)]
struct GossipBroadcastHandles {
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    sync_network_secret: crate::services::chat_activity_wake::SyncNetworkSecretCell,
}

#[derive(Debug, Clone)]
pub struct ConversationDequeuedInput {
    pub snapshot: ConversationInputEntrySnapshot,
    pub pending: ConversationInputPayload,
}

#[derive(Debug, Clone)]
pub struct ConversationTurnHandoff {
    pub terminal_fact: ConversationTurnTerminalFact,
    pub next_input: Option<ConversationDequeuedInput>,
}

pub enum ConversationSubmissionAcceptance {
    Started {
        input: ConversationInputEntrySnapshot,
        producer_id: uuid::Uuid,
        cancel_rx: watch::Receiver<rua_core::runtime::RunControl>,
    },
    Queued {
        input: ConversationInputEntrySnapshot,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstConversationSubmissionPersistenceContext {
    pub turn_id: String,
    pub input_id: String,
    pub submission_id: String,
    pub mode: ConversationInputAcceptanceMode,
}

pub type ConversationSubmissionPersistCallback = Box<
    dyn FnOnce(
            ConversationInputPayload,
            FirstConversationSubmissionPersistenceContext,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        + Send,
>;

pub enum ConversationSendNowOutcome {
    InterjectionRequested {
        input: ConversationInputEntrySnapshot,
    },
    Started {
        input: ConversationInputEntrySnapshot,
        producer_id: uuid::Uuid,
        cancel_rx: watch::Receiver<rua_core::runtime::RunControl>,
    },
    AlreadyApplied {
        input: ConversationInputEntrySnapshot,
    },
}

#[derive(Debug, Clone)]
pub struct ConversationControlEvent {
    pub address: ChatAddress,
    pub payload: String,
}

#[derive(Debug, Clone)]
struct ConversationInputEntry {
    snapshot: ConversationInputEntrySnapshot,
    pending: ConversationInputPayload,
    runtime_only: bool,
}

#[derive(Debug, Clone)]
struct ConversationAuthorityPlacementProjection {
    device_id: String,
    authority_epoch: ConversationAuthorityEpoch,
}

impl ConversationAuthority {
    pub async fn ensure_initial_placement(
        &self,
        state: &AppState,
        address: &ChatAddress,
    ) -> Result<ConversationAuthorityPlacement, ChatMetadataStoreError> {
        let scope_root =
            rua_core::chat_store::ChatScopeStore::resolve(&state.rua_root, address.clone());
        if let Some(placement) =
            crate::services::conversation_authority_target::load_optional_chat_authority_placement(
                scope_root.root(),
                address.chat_id().as_str(),
            )
            .await
            .map_err(ChatMetadataStoreError::Internal)?
        {
            return Ok(placement);
        }
        let local_device_id =
            crate::services::device_context::current_sync_device_id(&state.sync_device)
                .ok_or_else(|| {
                    ChatMetadataStoreError::Internal(
                        "local Device identity is unavailable".to_string(),
                    )
                })?;
        let target_device_id = match address.scope() {
            rua_core::ChatScope::Agent { .. } => local_device_id,
            rua_core::ChatScope::Project { project_id, .. } => {
                state
                    .project_catalog
                    .get(&rua_core::ProjectId::new(project_id.as_str()))
                    .map_err(|error| ChatMetadataStoreError::Internal(error.to_string()))?
                    .ok_or_else(|| {
                        ChatMetadataStoreError::Internal(format!(
                            "Project '{}' not found",
                            project_id.as_str()
                        ))
                    })?
                    .resource_device_id
            }
        };
        let placement = ConversationAuthorityPlacement {
            device_id: target_device_id.clone(),
            authority_epoch: ConversationAuthorityEpoch::next_for_device(&target_device_id, None),
            base_manifest_hash:
                crate::services::conversation_manifest::initial_conversation_manifest_hash(address)
                    .map_err(|error| ChatMetadataStoreError::Internal(error.to_string()))?,
        };
        self.update_authority_placement_guarded(
            Some(state),
            scope_root.root(),
            address,
            placement.clone(),
        )
        .await?;
        Ok(placement)
    }

    pub fn new() -> Self {
        Self::build(
            None,
            Arc::new(crate::services::product_hooks::ProductHookService::new()),
        )
    }

    pub fn with_foreground_operations(
        foreground_operations: Arc<ForegroundOperationRegistry>,
    ) -> Self {
        Self::build(
            Some(foreground_operations),
            Arc::new(crate::services::product_hooks::ProductHookService::new()),
        )
    }

    pub fn with_product_hooks(
        foreground_operations: Arc<ForegroundOperationRegistry>,
        product_hooks: Arc<crate::services::product_hooks::ProductHookService>,
    ) -> Self {
        Self::build(Some(foreground_operations), product_hooks)
    }

    fn build(
        foreground_operations: Option<Arc<ForegroundOperationRegistry>>,
        product_hooks: Arc<crate::services::product_hooks::ProductHookService>,
    ) -> Self {
        let (global_control_events, _) = broadcast::channel(256);
        Self {
            active_turns: RwLock::new(HashMap::new()),
            background_work: std::sync::OnceLock::new(),
            placement_projections: RwLock::new(HashMap::new()),
            queue: RwLock::new(HashMap::new()),
            accepted_submissions: RwLock::new(HashMap::new()),
            paused_queue_drain: RwLock::new(HashSet::new()),
            draining_agents: RwLock::new(HashSet::new()),
            draining_chats: RwLock::new(HashSet::new()),
            processed_control_commands: RwLock::new(HashMap::new()),
            interjected_turns: RwLock::new(HashMap::new()),
            revisions: RwLock::new(HashMap::new()),
            transition_gates: StdMutex::new(HashMap::new()),
            control_events: RwLock::new(HashMap::new()),
            global_control_events,
            history_replacement:
                crate::services::history_replacement::HistoryReplacementCoordinator::new(),
            foreground_operations,
            product_hooks,
            terminal_observer_runtime: std::sync::RwLock::new(None),
            terminal_observer_cancellation: tokio_util::sync::CancellationToken::new(),
            chat_completion_notifier: std::sync::RwLock::new(None),
            active_stream_broadcast_hub: std::sync::RwLock::new(None),
            gossip_broadcast: std::sync::RwLock::new(None),
        }
    }

    pub fn set_terminal_observer_runtime(
        &self,
        runtime: Arc<crate::services::terminal_observer::TerminalObserverRuntime>,
    ) {
        *self
            .terminal_observer_runtime
            .write()
            .expect("terminal observer runtime lock poisoned") = Some(runtime);
    }

    pub fn set_chat_completion_notifier(
        &self,
        notifier: Arc<crate::services::chat_completion_notification::ChatCompletionNotifier>,
    ) {
        *self
            .chat_completion_notifier
            .write()
            .expect("chat completion notifier lock poisoned") = Some(notifier);
    }

    pub fn install_background_work(
        &self,
        work: Arc<crate::services::background_work::BackgroundWork>,
    ) {
        if let Some(notifier) = self
            .chat_completion_notifier
            .read()
            .expect("chat completion notifier lock poisoned")
            .as_ref()
        {
            notifier.install_background_work(work.clone());
        }
        let _ = self.background_work.set(work);
    }

    pub async fn background_work_view(
        &self,
        address: &ChatAddress,
    ) -> crate::services::background_work::BackgroundWorkView {
        match self.background_work.get() {
            Some(work) => work.view(address).await,
            None => Default::default(),
        }
    }

    pub fn set_active_stream_broadcast_hub(
        &self,
        hub: Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>,
    ) {
        *self
            .active_stream_broadcast_hub
            .write()
            .expect("Conversation Authority activity hub lock poisoned") = Some(hub);
    }

    pub fn set_gossip_broadcast(
        &self,
        device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
        sync_network_secret: crate::services::chat_activity_wake::SyncNetworkSecretCell,
    ) {
        *self
            .gossip_broadcast
            .write()
            .expect("Conversation Authority gossip broadcast lock poisoned") =
            Some(GossipBroadcastHandles {
                device_network,
                sync_network_secret,
            });
    }

    async fn broadcast_placement_changed_over_gossip(
        &self,
        address: &ChatAddress,
        placement: ConversationAuthorityPlacement,
    ) {
        let handles = self
            .gossip_broadcast
            .read()
            .expect("Conversation Authority gossip broadcast lock poisoned")
            .clone();
        let Some(handles) = handles else {
            return;
        };
        let runtime = handles.device_network.lock().await.as_ref().cloned();
        let Some(runtime) = runtime else {
            return;
        };
        crate::services::chat_activity_wake::broadcast_placement_changed(
            &runtime,
            &handles.sync_network_secret,
            address.clone(),
            placement,
        )
        .await;
    }

    fn sync_execution_residency_at(
        &self,
        address: &ChatAddress,
        phase: Option<ConversationActiveTurnPhase>,
    ) {
        let Some(registry) = self.foreground_operations.as_ref() else {
            return;
        };
        let lease_id = format!("conversation-execution:{address}");
        if phase.is_some_and(|phase| phase != ConversationActiveTurnPhase::WaitingApproval) {
            registry.acquire(
                lease_id,
                format!("conversation:{address}"),
                ForegroundOperationKind::ConversationExecution,
            );
        } else {
            registry.release(&lease_id);
        }
    }

    pub async fn refresh_execution_residency(&self, address: &ChatAddress) {
        let phase = self
            .active_turns
            .read()
            .await
            .get(address)
            .map(|turn| turn.phase);
        self.sync_execution_residency_at(address, phase);
    }

    pub async fn transition_active_turn_phase(
        &self,
        address: &ChatAddress,
        expected_turn_id: Option<&str>,
        phase: ConversationActiveTurnPhase,
    ) -> Option<ConversationActiveTurn> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let mut active_turns = self.active_turns.write().await;
        let active_turn = active_turns.get_mut(address)?;
        if expected_turn_id.is_some_and(|turn_id| active_turn.turn_id != turn_id) {
            return None;
        }
        if active_turn.phase == ConversationActiveTurnPhase::Stopping {
            return Some(active_turn.clone());
        }
        let changed = active_turn.phase != phase;
        active_turn.phase = phase;
        let snapshot = active_turn.clone();
        drop(active_turns);
        self.sync_execution_residency_at(address, Some(phase));
        if changed {
            self.publish_control_snapshot(
                address,
                "active_turn_phase_changed",
                &serde_json::json!({ "phase": phase }),
            )
            .await;
        }
        Some(snapshot)
    }

    pub async fn observe_runtime_event(
        &self,
        address: &ChatAddress,
        expected_turn_id: Option<&str>,
        event: &RuntimeEvent,
    ) -> Option<ConversationActiveTurn> {
        let phase = match event {
            RuntimeEvent::ToolApprovalPending { .. } | RuntimeEvent::ToolApprovalRequest { .. } => {
                ConversationActiveTurnPhase::WaitingApproval
            }
            RuntimeEvent::ToolCall { .. } | RuntimeEvent::ToolResultDelta { .. } => {
                ConversationActiveTurnPhase::RunningTool
            }
            RuntimeEvent::Error { .. } | RuntimeEvent::End { .. } => {
                ConversationActiveTurnPhase::Committing
            }
            RuntimeEvent::Start { .. }
            | RuntimeEvent::TextDelta { .. }
            | RuntimeEvent::ThinkingDelta { .. }
            | RuntimeEvent::ModelCallReset { .. }
            | RuntimeEvent::ToolCallDelta { .. }
            | RuntimeEvent::ToolResult { .. }
            | RuntimeEvent::ContextCompaction { .. }
            | RuntimeEvent::IterationStart { .. }
            | RuntimeEvent::IterationEnd { .. }
            | RuntimeEvent::AskUser { .. } => ConversationActiveTurnPhase::Streaming,
            RuntimeEvent::Usage { .. } => return self.active_turn(address).await,
        };
        self.transition_active_turn_phase(address, expected_turn_id, phase)
            .await
    }

    pub(crate) fn schedule_history_replacement(
        &self,
        input: crate::services::history_replacement::HistoryReplacementJobContext,
    ) {
        self.history_replacement.schedule(input);
    }

    #[doc(hidden)]
    pub fn disable_history_replacement_for_agent(&self, agent_id: &str) {
        self.history_replacement.disable_agent(agent_id);
    }

    #[doc(hidden)]
    pub fn begin_history_replacement_foreground(
        &self,
        model: &rua_session::CheckpointModelIdentity,
    ) -> crate::services::history_replacement::HistoryReplacementForegroundGuard {
        self.history_replacement.foreground(model)
    }

    pub(crate) fn transition_gate_at(&self, address: &ChatAddress) -> Arc<AsyncMutex<()>> {
        let key = address.clone();
        let mut gates = self
            .transition_gates
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
            return gate;
        }
        if gates.len() >= 256 {
            gates.retain(|_, gate| gate.strong_count() > 0);
        }
        let gate = Arc::new(AsyncMutex::new(()));
        gates.insert(key, Arc::downgrade(&gate));
        gate
    }

    pub(crate) fn transition_gate(&self, address: &ChatAddress) -> Arc<AsyncMutex<()>> {
        self.transition_gate_at(address)
    }

    pub async fn start_active_turn(
        &self,
        start: ConversationActiveTurnStart,
    ) -> ConversationActiveTurn {
        let gate = self.transition_gate_at(&start.address);
        let _transition = gate.lock().await;
        self.start_active_turn_locked(start).await
    }

    async fn start_active_turn_locked(
        &self,
        start: ConversationActiveTurnStart,
    ) -> ConversationActiveTurn {
        self.paused_queue_drain.write().await.remove(&start.address);
        self.start_active_turn_with_id_locked(start, None, None)
            .await
    }

    async fn start_active_turn_for_input_locked(
        &self,
        start: ConversationActiveTurnStart,
        pending: &mut ConversationInputPayload,
        submit_device_id: Option<String>,
    ) -> ConversationActiveTurn {
        let configuration_key =
            crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY.to_string();
        let configuration = match self.product_hooks.chat_configuration(&start.address).await {
            Some(configuration) => Some(configuration),
            None => pending
                .extra
                .get(&configuration_key)
                .cloned()
                .and_then(|value| {
                    serde_json::from_value::<crate::services::product_hooks::ChatHookConfiguration>(
                        value,
                    )
                    .ok()
                }),
        };
        if let Some(configuration) = configuration {
            if pending.model_override.is_none() {
                let (model, thinking) = rua_chat::capture_turn_model_and_thinking(
                    configuration.model.as_ref(),
                    configuration.thinking.as_ref(),
                );
                if let Some(model) = model {
                    pending
                        .extra
                        .insert(rua_chat::TURN_MODEL_KEY.to_string(), model);
                } else {
                    pending.extra.remove(rua_chat::TURN_MODEL_KEY);
                }
                if let Some(thinking) = thinking {
                    pending
                        .extra
                        .insert(rua_chat::TURN_THINKING_KEY.to_string(), thinking);
                } else {
                    pending.extra.remove(rua_chat::TURN_THINKING_KEY);
                }
            }
            if let Some(assignment) = configuration.agent_assignment.as_ref() {
                pending.extra.insert(
                    rua_chat::TURN_AGENT_ASSIGNMENT_META_KEY.to_string(),
                    serde_json::to_value(assignment).expect("Agent assignment serializes"),
                );
            }
            self.product_hooks
                .adopt_chat_configuration(&start.address, configuration.clone())
                .await;
            pending.extra.insert(
                configuration_key,
                serde_json::to_value(configuration).expect("Chat configuration serializes"),
            );
        }
        let turn_id = pending
            .extra
            .get("turn_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .or_else(|| regenerate_turn_id(pending));
        if pending
            .extra
            .get(crate::services::background_work::BACKGROUND_RESULT_KEY)
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        {
            self.paused_queue_drain.write().await.remove(&start.address);
        }
        self.start_active_turn_with_id_locked(start, turn_id, submit_device_id)
            .await
    }

    async fn start_active_turn_with_id_locked(
        &self,
        start: ConversationActiveTurnStart,
        turn_id: Option<String>,
        submit_device_id: Option<String>,
    ) -> ConversationActiveTurn {
        let address = start.address.clone();
        let placement = self
            .observe_placement_projection_at(
                &address,
                &start.authority_device_id,
                start.authority_epoch,
            )
            .await;
        let active_turn = ConversationActiveTurn {
            turn_id: turn_id.unwrap_or_else(|| format!("turn-{}", uuid::Uuid::new_v4())),
            user_message_id: None,
            submit_device_id,
            phase: ConversationActiveTurnPhase::Preparing,
            authority_device_id: placement.device_id,
            authority_epoch: placement.authority_epoch,
            driver: start.driver,
            started_at: chrono::Utc::now().to_rfc3339(),
            queued_count: self.user_input_queue_snapshot(&address).await.len(),
            driver_detail: start.driver_detail,
        };
        self.active_turns
            .write()
            .await
            .insert(address.clone(), active_turn.clone());
        self.product_hooks
            .freeze_turn(&address, &active_turn.turn_id)
            .await;
        self.sync_execution_residency_at(&address, Some(ConversationActiveTurnPhase::Preparing));
        self.publish_control_snapshot(&address, "active_turn_started", &serde_json::json!({}))
            .await;
        active_turn
    }

    pub async fn bind_turn(
        &self,
        binding: ConversationTurnBinding,
    ) -> Option<ConversationActiveTurn> {
        let address = binding.address.clone();
        let gate = self.transition_gate_at(&address);
        let _transition = gate.lock().await;
        let mut active_turns = self.active_turns.write().await;
        let active_turn = active_turns.get_mut(&address)?;
        if active_turn.turn_id != binding.turn_id {
            return None;
        }
        active_turn.user_message_id = binding.user_message_id;
        if active_turn.phase != ConversationActiveTurnPhase::Stopping {
            active_turn.phase = ConversationActiveTurnPhase::Streaming;
        }
        let snapshot = active_turn.clone();
        drop(active_turns);
        self.sync_execution_residency_at(&address, Some(snapshot.phase));
        self.publish_control_snapshot(&address, "turn_bound", &serde_json::json!({}))
            .await;
        Some(snapshot)
    }

    pub async fn finish_active_turn(
        &self,
        address: &ChatAddress,
    ) -> Option<ConversationActiveTurn> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        self.finish_active_turn_locked(address).await
    }

    async fn finish_active_turn_locked(
        &self,
        address: &ChatAddress,
    ) -> Option<ConversationActiveTurn> {
        let snapshot = self.active_turns.write().await.remove(address);
        if snapshot.is_some() {
            self.sync_execution_residency_at(address, None);
        }
        self.publish_control_snapshot(address, "active_turn_finished", &serde_json::json!({}))
            .await;
        snapshot
    }

    pub async fn clear_queue(&self, address: &ChatAddress) {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        self.clear_queue_locked(address).await;
    }

    pub async fn discard_unclaimed_inputs(&self, address: &ChatAddress) {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        if let Some(queue) = self.queue.write().await.get_mut(address) {
            queue.retain(|entry| {
                entry
                    .pending
                    .extra
                    .get(ACCEPTANCE_MODE_KEY)
                    .and_then(serde_json::Value::as_str)
                    == Some("started")
            });
        }
        self.set_queued_count_from_queue(address).await;
    }

    /// Stops this Agent from starting any new Turn or accepting new input.
    ///
    /// Shutdown calls this before it observes work so that a submission racing
    /// the shutdown cannot become a fresh Turn after the drain looked empty.
    pub async fn begin_draining(&self, agent_id: &str) {
        self.draining_agents
            .write()
            .await
            .insert(agent_id.to_string());
    }

    /// Whether this Agent has begun shutdown.
    pub async fn is_draining(&self, agent_id: &str) -> bool {
        self.draining_agents.read().await.contains(agent_id)
    }

    pub async fn begin_chat_shutdown(&self, address: &ChatAddress) {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        self.draining_chats.write().await.insert(address.clone());
    }

    pub async fn resume_agent(&self, agent_id: &str, addresses: &[ChatAddress]) {
        for address in addresses {
            let gate = self.transition_gate_at(address);
            let _transition = gate.lock().await;
            self.draining_chats.write().await.remove(address);
        }
        self.draining_agents.write().await.remove(agent_id);
    }

    async fn clear_queue_locked(&self, address: &ChatAddress) {
        self.queue.write().await.remove(address);
        self.set_queued_count(address, 0).await;
        self.publish_control_snapshot(address, "queue_cleared", &serde_json::json!({}))
            .await;
    }

    pub async fn request_stop(
        &self,
        streams: Option<&ActiveStreamRegistry>,
        channel_id: &str,
        address: &ChatAddress,
    ) -> Option<ConversationActiveTurn> {
        self.request_stop_matching(streams, channel_id, address, None, None)
            .await
    }

    pub async fn request_stop_matching(
        &self,
        streams: Option<&ActiveStreamRegistry>,
        channel_id: &str,
        address: &ChatAddress,
        expected_turn_id: Option<&str>,
        expected_authority_epoch: Option<ConversationAuthorityEpoch>,
    ) -> Option<ConversationActiveTurn> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        self.request_stop_matching_locked(
            streams,
            channel_id,
            address,
            expected_turn_id,
            expected_authority_epoch,
        )
        .await
    }

    pub(crate) async fn request_stop_matching_locked(
        &self,
        streams: Option<&ActiveStreamRegistry>,
        channel_id: &str,
        address: &ChatAddress,
        expected_turn_id: Option<&str>,
        expected_authority_epoch: Option<ConversationAuthorityEpoch>,
    ) -> Option<ConversationActiveTurn> {
        self.stop_turn_locked(
            streams,
            channel_id,
            address,
            expected_turn_id,
            expected_authority_epoch,
        )
        .await
    }

    pub(crate) async fn stop_collaboration_locked(
        &self,
        streams: &ActiveStreamRegistry,
        channel_id: &str,
        address: &ChatAddress,
        turn_id: &str,
        epoch: ConversationAuthorityEpoch,
    ) -> Option<Vec<String>> {
        let active = self.active_turn(address).await?;
        if active.turn_id != turn_id || active.authority_epoch != epoch {
            return None;
        }
        let requests = self
            .queue
            .read()
            .await
            .get(address)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                entry
                    .pending
                    .extra
                    .get(SUBMISSION_ID_KEY)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        self.clear_queue_locked(address).await;
        self.paused_queue_drain.write().await.remove(address);
        self.stop_turn_locked(
            Some(streams),
            channel_id,
            address,
            Some(turn_id),
            Some(epoch),
        )
        .await?;
        Some(requests)
    }

    #[allow(clippy::too_many_arguments)]
    async fn stop_turn_locked(
        &self,
        streams: Option<&ActiveStreamRegistry>,
        channel_id: &str,
        address: &ChatAddress,
        expected_turn_id: Option<&str>,
        expected_authority_epoch: Option<ConversationAuthorityEpoch>,
    ) -> Option<ConversationActiveTurn> {
        let mut active_turns = self.active_turns.write().await;
        let active_turn = active_turns.get_mut(address)?;
        if expected_turn_id.is_some_and(|expected| active_turn.turn_id != expected)
            || expected_authority_epoch
                .is_some_and(|expected| active_turn.authority_epoch != expected)
        {
            return None;
        }
        active_turn.phase = ConversationActiveTurnPhase::Stopping;
        let snapshot = active_turn.clone();
        drop(active_turns);
        self.sync_execution_residency_at(address, Some(ConversationActiveTurnPhase::Stopping));
        let key = address.clone();
        let queue_has_entries = self.queue.read().await.get(&key).is_some_and(|queue| {
            queue.iter().any(|entry| {
                !entry.runtime_only
                    && entry
                        .pending
                        .extra
                        .get(ACCEPTANCE_MODE_KEY)
                        .and_then(serde_json::Value::as_str)
                        != Some("started")
            })
        });
        let mut paused_queue_drain = self.paused_queue_drain.write().await;
        if queue_has_entries {
            paused_queue_drain.insert(key.clone());
        } else {
            paused_queue_drain.remove(&key);
        }
        drop(paused_queue_drain);
        if let Some(streams) = streams {
            let _ = streams.cancel_all_priorities(channel_id, address).await;
        }
        self.publish_control_snapshot(address, "stop_requested", &serde_json::json!({}))
            .await;
        Some(snapshot)
    }

    pub async fn pause_queued_inputs(&self, address: &ChatAddress) -> bool {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        self.pause_queued_inputs_locked(address).await
    }

    pub(crate) async fn pause_queued_inputs_locked(&self, address: &ChatAddress) -> bool {
        if self.user_input_queue_snapshot(address).await.is_empty() {
            return false;
        }
        self.paused_queue_drain
            .write()
            .await
            .insert(address.clone());
        self.publish_control_snapshot(address, "queue_paused", &serde_json::json!({}))
            .await;
        true
    }

    pub async fn request_interjection(
        &self,
        address: &ChatAddress,
    ) -> Option<ConversationActiveTurn> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let snapshot = self.active_turn(address).await;
        self.publish_control_snapshot(address, "interjection_requested", &serde_json::json!({}))
            .await;
        snapshot
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn request_send_now(
        &self,
        streams: &ActiveStreamRegistry,
        channel_id: &str,
        address: &ChatAddress,
        input_id: &str,
        authority_epoch: ConversationAuthorityEpoch,
        command_id: &str,
        start: ConversationActiveTurnStart,
    ) -> Result<ConversationSendNowOutcome, String> {
        let command_id = command_id.trim();
        if command_id.is_empty() {
            return Err("command_id is required".to_string());
        }
        if &start.address != address {
            return Err("Active Turn start address does not match Send Now address".to_string());
        }
        let key = address.clone();
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;

        if let Some(processed_input_id) = self
            .processed_control_commands
            .read()
            .await
            .get(&(key.clone(), command_id.to_string()))
            .cloned()
        {
            if processed_input_id != input_id {
                return Err("command_id was already used for another queued input".to_string());
            }
            let input = self
                .accepted_input_by_id(&key, input_id)
                .await
                .ok_or_else(|| "queued input is no longer available".to_string())?;
            return Ok(ConversationSendNowOutcome::AlreadyApplied { input });
        }

        let placement = self
            .observe_placement_projection_at(
                address,
                &start.authority_device_id,
                start.authority_epoch,
            )
            .await;
        if placement.authority_epoch != authority_epoch {
            return Err("stale conversation authority Epoch".to_string());
        }
        let active_turn = self.active_turns.read().await.get(&key).cloned();
        let (head, regenerate_turn_id, submit_device_id) = {
            let mut queues = self.queue.write().await;
            let queue = queues
                .get_mut(&key)
                .ok_or_else(|| "queued input is missing".to_string())?;
            let position = queue
                .iter()
                .position(|entry| entry.snapshot.input_id == input_id)
                .ok_or_else(|| "queued input is missing".to_string())?;
            if active_turn.is_some() && position > 0 {
                return Err("queued input is not the authority queue head".to_string());
            }
            if position > 0 {
                let selected = queue
                    .remove(position)
                    .ok_or_else(|| "queued input is missing".to_string())?;
                queue.push_front(selected);
            }
            let entry = queue
                .front()
                .ok_or_else(|| "queued input is missing".to_string())?;
            (
                entry.snapshot.clone(),
                regenerate_turn_id(&entry.pending),
                entry.snapshot.submit_device_id.clone(),
            )
        };

        self.processed_control_commands
            .write()
            .await
            .insert((key.clone(), command_id.to_string()), input_id.to_string());
        self.paused_queue_drain.write().await.remove(&key);
        if let Some(active_turn) = active_turn {
            self.interjected_turns
                .write()
                .await
                .insert(key.clone(), active_turn.turn_id);
            let _ = streams.interject_all_priorities(channel_id, &key).await;
            self.publish_control_snapshot(
                address,
                "send_now_requested",
                &serde_json::json!({"input_id": input_id}),
            )
            .await;
            return Ok(ConversationSendNowOutcome::InterjectionRequested { input: head });
        }

        let (producer_id, cancel_rx) = streams
            .register_active_keyed(&StreamKey::new(
                channel_id,
                key.clone(),
                crate::streaming::Priority::Normal,
            ))
            .await;
        let active_turn = self
            .start_active_turn_with_id_locked(start, regenerate_turn_id, submit_device_id)
            .await;
        let mut queues = self.queue.write().await;
        let queued = queues
            .get_mut(&key)
            .and_then(|queue| queue.front_mut())
            .ok_or_else(|| "queued input is missing".to_string())?;
        queued.pending.extra.insert(
            "turn_id".to_string(),
            serde_json::json!(active_turn.turn_id.clone()),
        );
        drop(queues);
        let _ = streams
            .set_active_turn_id(
                channel_id,
                &key,
                crate::streaming::Priority::Normal,
                active_turn.turn_id,
            )
            .await;
        self.set_queued_count_from_queue(address).await;
        self.publish_control_snapshot(
            address,
            "send_now_started",
            &serde_json::json!({"input_id": input_id}),
        )
        .await;
        Ok(ConversationSendNowOutcome::Started {
            input: head,
            producer_id,
            cancel_rx,
        })
    }

    pub async fn commit_turn_terminal(
        &self,
        address: &ChatAddress,
        status: impl Into<String>,
        completed_at: Option<String>,
        device_delivery_id: Option<String>,
    ) -> Option<ConversationTurnTerminalFact> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let mut active_turns = self.active_turns.write().await;
        let active_turn = active_turns.get_mut(address)?;
        active_turn.phase = ConversationActiveTurnPhase::Committing;
        let active_turn = active_turn.clone();
        drop(active_turns);
        self.sync_execution_residency_at(address, Some(ConversationActiveTurnPhase::Committing));
        let fact = Some(ConversationTurnTerminalFact {
            address: address.clone(),
            turn_id: active_turn.turn_id,
            status: status.into(),
            completed_at,
            authority_device_id: active_turn.authority_device_id,
            authority_epoch: active_turn.authority_epoch,
            device_delivery_id,
        });
        self.publish_control_snapshot(address, "turn_terminal", &serde_json::json!({}))
            .await;
        fact
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn complete_active_turn_and_drain_queue<F, Fut>(
        &self,
        streams: &ActiveStreamRegistry,
        stream_key: &StreamKey,
        address: &ChatAddress,
        turn_id: &str,
        status: impl Into<String>,
        completed_at: Option<String>,
        device_delivery_id: Option<String>,
        drain_queue: bool,
        publish_terminal: F,
    ) -> Option<ConversationTurnHandoff>
    where
        F: FnOnce(ConversationTurnTerminalFact) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        if &stream_key.address != address {
            return None;
        }
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let key = address.clone();
        let current = self.active_turns.read().await.get(&key).cloned()?;
        if current.turn_id != turn_id {
            return None;
        }

        let status = if self
            .interjected_turns
            .write()
            .await
            .remove(&key)
            .is_some_and(|interjected_turn_id| interjected_turn_id == current.turn_id)
        {
            "interjected".to_string()
        } else {
            status.into()
        };
        let terminal_fact = ConversationTurnTerminalFact {
            address: address.clone(),
            turn_id: current.turn_id,
            status,
            completed_at,
            authority_device_id: current.authority_device_id,
            authority_epoch: current.authority_epoch,
            device_delivery_id,
        };
        // Capture and retire this epoch before a queued regeneration reuses its Turn id.
        let frozen_configuration = self
            .product_hooks
            .frozen_turn_configuration(address, turn_id)
            .await;
        self.product_hooks
            .observe_terminal(address, turn_id, terminal_fact.completed_at.as_deref())
            .await;
        publish_terminal(terminal_fact.clone()).await;
        self.active_turns.write().await.remove(&key);
        self.sync_execution_residency_at(address, None);
        self.publish_control_snapshot(address, "turn_completed", &serde_json::json!({}))
            .await;

        let mut next_input = if drain_queue || self.is_queue_drain_paused(address).await {
            self.pop_next_input_state(&key).await
        } else {
            None
        };
        if let Some(next) = next_input.as_mut() {
            let placement = self
                .placement_projection(address)
                .await
                .expect("queue drain must retain Conversation Authority Placement");
            let next_turn = self
                .start_active_turn_for_input_locked(
                    ConversationActiveTurnStart {
                        address: key,
                        driver: current.driver,
                        authority_device_id: placement.device_id,
                        authority_epoch: placement.authority_epoch,
                        driver_detail: current.driver_detail,
                    },
                    &mut next.pending,
                    next.snapshot.submit_device_id.clone(),
                )
                .await;
            next.pending.extra.insert(
                "turn_id".to_string(),
                serde_json::json!(next_turn.turn_id.clone()),
            );
            let _ = streams
                .set_active_turn_id(
                    &stream_key.channel_id,
                    &stream_key.address,
                    stream_key.priority,
                    next_turn.turn_id,
                )
                .await;
        }

        drop(_transition);

        let terminal_observer_runtime = self
            .terminal_observer_runtime
            .read()
            .expect("terminal observer runtime lock poisoned")
            .clone();
        let chat_completion_notifier = self
            .chat_completion_notifier
            .read()
            .expect("chat completion notifier lock poisoned")
            .clone();
        let product_hooks = Arc::clone(&self.product_hooks);
        let cancellation = self.terminal_observer_cancellation.child_token();
        let observed_terminal = terminal_fact.clone();
        // Post-terminal work must not retain the producer or delay the next Turn.
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {},
                _ = async {
                    if let (Some(runtime), Some(configuration)) =
                        (terminal_observer_runtime, frozen_configuration)
                    {
                        let diagnostics = runtime
                            .observe_frozen(&observed_terminal, &configuration, cancellation.clone())
                            .await;
                        product_hooks
                            .record_terminal_observer_diagnostics(&observed_terminal, &diagnostics)
                            .await;
                    }
                    if let Some(notifier) = chat_completion_notifier {
                        notifier.notify(&observed_terminal).await;
                    }
                } => {},
            }
        });

        Some(ConversationTurnHandoff {
            terminal_fact,
            next_input,
        })
    }

    pub async fn active_turn(&self, address: &ChatAddress) -> Option<ConversationActiveTurn> {
        self.active_turns.read().await.get(address).cloned()
    }

    /// Every Chat Address this authority currently holds an Active Turn for,
    /// limited to one Agent when `agent_id` is given.
    ///
    /// This is the real enumeration an Agent shutdown needs: it reads the same
    /// map the turn lifecycle mutates, so it cannot drift from a mirrored count.
    /// A Project-Scoped Chat is owned by the Project, not by the Agent executing
    /// its turns, so it is never attributed to an Agent here.
    pub async fn active_turn_addresses(&self, agent_id: Option<&str>) -> Vec<ChatAddress> {
        let active_turns = self.active_turns.read().await;
        let mut addresses: Vec<ChatAddress> = active_turns
            .keys()
            .filter(|address| match agent_id {
                None => true,
                Some(agent) => address
                    .agent_id()
                    .is_some_and(|owner| owner.as_str() == agent),
            })
            .cloned()
            .collect();
        addresses.sort_by_key(ChatAddress::identity_key);
        addresses
    }

    /// Every Chat Address holding queued input for one Agent, in stable order.
    ///
    /// Queued input is real Agent work even with no Active Turn, so a shutdown
    /// that only looked at turns would leave a submission able to start later.
    pub async fn queued_input_addresses(&self, agent_id: &str) -> Vec<ChatAddress> {
        let queue = self.queue.read().await;
        let mut addresses: Vec<ChatAddress> = queue
            .iter()
            .filter(|(address, entries)| {
                address
                    .agent_id()
                    .is_some_and(|owner| owner.as_str() == agent_id)
                    && !entries.is_empty()
            })
            .map(|(address, _)| address.clone())
            .collect();
        addresses.sort_by_key(ChatAddress::identity_key);
        addresses
    }

    /// Queued inputs the authority holds for one Agent.
    pub async fn queued_input_count(&self, agent_id: &str) -> usize {
        self.queued_input_addresses(agent_id).await.len()
    }

    /// Accounts for the queued input the authority holds for one Agent.
    pub async fn outstanding_queued_input(&self, agent_id: &str) -> usize {
        self.queue
            .read()
            .await
            .iter()
            .filter(|(address, _)| {
                address
                    .agent_id()
                    .is_some_and(|owner| owner.as_str() == agent_id)
            })
            .map(|(_, entries)| entries.len())
            .sum()
    }

    pub async fn accept_submission(
        &self,
        streams: &ActiveStreamRegistry,
        stream_key: &StreamKey,
        start: ConversationActiveTurnStart,
        pending: ConversationInputPayload,
        submit_device_id: Option<String>,
        persist: Option<ConversationSubmissionPersistCallback>,
    ) -> Result<ConversationSubmissionAcceptance, String> {
        self.accept_submission_inner(
            streams,
            stream_key,
            start,
            pending,
            submit_device_id,
            persist,
            true,
        )
        .await
    }

    pub(crate) async fn accept_interjecting_submission(
        &self,
        streams: &ActiveStreamRegistry,
        stream_key: &StreamKey,
        start: ConversationActiveTurnStart,
        pending: ConversationInputPayload,
        submit_device_id: Option<String>,
        persist: Option<ConversationSubmissionPersistCallback>,
    ) -> Result<ConversationSubmissionAcceptance, String> {
        self.accept_submission_inner(
            streams,
            stream_key,
            start,
            pending,
            submit_device_id,
            persist,
            false,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn accept_submission_inner(
        &self,
        streams: &ActiveStreamRegistry,
        stream_key: &StreamKey,
        start: ConversationActiveTurnStart,
        pending: ConversationInputPayload,
        submit_device_id: Option<String>,
        persist: Option<ConversationSubmissionPersistCallback>,
        continue_active_turn_at_boundary: bool,
    ) -> Result<ConversationSubmissionAcceptance, String> {
        let address = start.address.clone();
        assert_eq!(
            stream_key.address, address,
            "Conversation submission Stream Key must match Active Turn address"
        );
        let gate = self.transition_gate_at(&address);
        let _transition = gate.lock().await;
        self.accept_submission_inner_locked(
            streams,
            stream_key,
            start,
            pending,
            submit_device_id,
            persist,
            continue_active_turn_at_boundary,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn accept_submission_inner_locked(
        &self,
        streams: &ActiveStreamRegistry,
        stream_key: &StreamKey,
        mut start: ConversationActiveTurnStart,
        mut pending: ConversationInputPayload,
        submit_device_id: Option<String>,
        persist: Option<ConversationSubmissionPersistCallback>,
        continue_active_turn_at_boundary: bool,
    ) -> Result<ConversationSubmissionAcceptance, String> {
        let address = start.address.clone();
        assert_eq!(
            stream_key.address, address,
            "Conversation submission Stream Key must match Active Turn address"
        );
        let submission_id = ensure_submission_id(&mut pending);
        if pending
            .extra
            .get(FIRST_CHAT_SUBMISSION_CANDIDATE_KEY)
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            if let Some(placement) = self.placement_projection(&address).await {
                pending
                    .extra
                    .remove(crate::services::conversation_input_intake::INITIAL_ACP_KEY);
                start.authority_device_id = placement.device_id.clone();
                start.authority_epoch = placement.authority_epoch;
                pending.extra.insert(
                    "authority_device_id".to_string(),
                    serde_json::json!(placement.device_id),
                );
                pending.extra.insert(
                    "authority_epoch".to_string(),
                    serde_json::json!(placement.authority_epoch),
                );
                if let Some(persisted) = self.product_hooks.chat_configuration(&address).await {
                    pending = crate::services::first_chat_submission::input_with_configuration(
                        &pending, &persisted,
                    )?;
                    pending.extra.insert(
                        crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY
                            .to_string(),
                        serde_json::to_value(persisted)
                            .expect("Chat Hook Configuration serializes"),
                    );
                }
            }
        }
        // Read under the same gate the turn lifecycle mutates, so a submission
        // cannot observe "not draining" and then start a Turn while shutdown is
        // already terminating the Agent.
        if self.draining_chats.read().await.contains(&address) {
            return Err("Chat execution is shutting down and cannot accept new input".to_string());
        }
        if let Some(agent_id) = address.agent_id() {
            if self.is_draining(agent_id.as_str()).await {
                return Err(format!(
                    "Agent '{}' is shutting down and cannot accept new input",
                    agent_id.as_str()
                ));
            }
        }

        let background_result = pending
            .extra
            .get(crate::services::background_work::BACKGROUND_RESULT_KEY)
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        if background_result {
            let work = self
                .background_work
                .get()
                .ok_or("Background continuation runtime is unavailable")?;
            if work.suppressed(&address).await? {
                return Err("Background result retained: automatic continuation is stopped".into());
            }
        }

        let active_turn = self.active_turn(&address).await;
        let has_active_turn = active_turn.is_some();
        let queue_drain_paused = self.is_queue_drain_paused(&address).await && !background_result;
        if let Some(input) = self.accepted_submission(&address, &submission_id).await {
            return Ok(ConversationSubmissionAcceptance::Queued { input });
        }

        let configuration = match self.product_hooks.chat_configuration(&address).await {
            Some(configuration) => Some(configuration),
            None => pending
                .extra
                .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
                .cloned()
                .map(
                    serde_json::from_value::<crate::services::product_hooks::ChatHookConfiguration>,
                )
                .transpose()
                .map_err(|error| format!("invalid Chat configuration: {error}"))?,
        };
        if let Some(configuration) = configuration {
            configuration.validate()?;
            if self.is_draining(&configuration.selected_agent_id).await {
                return Err(
                    "Selected Agent is shutting down and cannot accept new input".to_string(),
                );
            }
            if let Some(agent_id) = address.agent_id() {
                if configuration.selected_agent_id != agent_id.as_str() {
                    return Err("Chat configuration belongs to a different Agent".to_string());
                }
            }
        }

        if !has_active_turn && queue_drain_paused {
            pending.extra.insert(
                ACCEPTANCE_MODE_KEY.to_string(),
                serde_json::json!(ConversationInputAcceptanceMode::Queued),
            );
            let input = self
                .enqueue_input_locked(&address, pending, submit_device_id)
                .await
                .expect("paused Conversation Authority queue must accept input");
            return Ok(ConversationSubmissionAcceptance::Queued { input });
        }

        if has_active_turn {
            pending.extra.insert(
                ACCEPTANCE_MODE_KEY.to_string(),
                serde_json::json!(ConversationInputAcceptanceMode::Queued),
            );
            if let Some(input) = self
                .enqueue_input_locked(&address, pending.clone(), submit_device_id.clone())
                .await
            {
                if matches!(
                    active_turn.as_ref().map(|turn| turn.driver),
                    Some(ConversationDriverKind::Rua)
                ) {
                    if continue_active_turn_at_boundary {
                        let _ = streams
                            .continue_all_priorities_at_boundary(&stream_key.channel_id, &address)
                            .await;
                    } else {
                        let _ = streams
                            .interject_after_tool_all_priorities(&stream_key.channel_id, &address)
                            .await;
                    }
                }
                return Ok(ConversationSubmissionAcceptance::Queued { input });
            }
        }

        pending.extra.insert(
            ACCEPTANCE_MODE_KEY.to_string(),
            serde_json::json!(ConversationInputAcceptanceMode::Started),
        );
        if let Some(persist) = persist {
            let persistence_context = FirstConversationSubmissionPersistenceContext {
                turn_id: pending
                    .extra
                    .get("turn_id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("turn-{}", uuid::Uuid::new_v4())),
                input_id: format!("input-{}", uuid::Uuid::new_v4()),
                submission_id,
                mode: ConversationInputAcceptanceMode::Started,
            };
            pending.extra.insert(
                "turn_id".to_string(),
                serde_json::json!(persistence_context.turn_id),
            );
            pending.extra.insert(
                INPUT_ID_KEY.to_string(),
                serde_json::json!(persistence_context.input_id),
            );
            persist(pending.clone(), persistence_context).await?;
            pending.extra.insert(
                SUBMITTED_USER_TURN_PERSISTED_KEY.to_string(),
                serde_json::Value::Bool(true),
            );
        }

        let (producer_id, cancel_rx) = streams.register_active_keyed(stream_key).await;
        let active_turn = self
            .start_active_turn_for_input_locked(start, &mut pending, submit_device_id.clone())
            .await;
        pending.extra.insert(
            "turn_id".to_string(),
            serde_json::json!(active_turn.turn_id),
        );
        let input = self
            .enqueue_input_locked(&address, pending, submit_device_id)
            .await
            .expect("newly started Conversation Authority must accept its first input");
        Ok(ConversationSubmissionAcceptance::Started {
            input,
            producer_id,
            cancel_rx,
        })
    }

    pub async fn claim_next_or_finish(
        &self,
        streams: &ActiveStreamRegistry,
        stream_key: &StreamKey,
        producer_id: uuid::Uuid,
        address: &ChatAddress,
    ) -> Option<ConversationDequeuedInput> {
        if &stream_key.address != address {
            return None;
        }
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        // Draining blocks the queue drain from turning queued input into a new
        // Turn. The queue itself is discarded by shutdown under this same gate,
        // and the producer still has to finish through the normal tail so the
        // Turn and its stream are not left dangling.
        let draining = self.draining_chats.read().await.contains(address)
            || match address.agent_id() {
                Some(agent_id) => self.is_draining(agent_id.as_str()).await,
                None => false,
            };
        if let Some(mut next) = self.pop_next_input_state(address).await {
            if !draining || next.pending.extra.contains_key("turn_id") {
                self.set_queued_count_from_queue(address).await;
                if !next.pending.extra.contains_key("turn_id") {
                    let current = self.active_turn(address).await?;
                    let placement = self
                        .placement_projection(address)
                        .await
                        .expect("Active Turn must retain Conversation Authority Placement");
                    let next_turn = self
                        .start_active_turn_for_input_locked(
                            ConversationActiveTurnStart {
                                address: address.clone(),
                                driver: current.driver,
                                authority_device_id: placement.device_id,
                                authority_epoch: placement.authority_epoch,
                                driver_detail: current.driver_detail,
                            },
                            &mut next.pending,
                            next.snapshot.submit_device_id.clone(),
                        )
                        .await;
                    next.pending
                        .extra
                        .insert("turn_id".to_string(), serde_json::json!(next_turn.turn_id));
                }
                return Some(next);
            }
        }

        streams.mark_completed(stream_key, producer_id).await;
        let _ = self.finish_active_turn_locked(address).await;
        None
    }

    pub async fn finish_producer(
        &self,
        streams: &ActiveStreamRegistry,
        stream_key: &StreamKey,
        producer_id: uuid::Uuid,
        address: &ChatAddress,
        clear_queue: bool,
    ) {
        if &stream_key.address != address {
            return;
        }
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        if clear_queue {
            self.clear_queue_locked(address).await;
        }
        streams.mark_completed(stream_key, producer_id).await;
        let _ = self.finish_active_turn_locked(address).await;
    }

    #[doc(hidden)]
    pub async fn enqueue_input(
        &self,
        address: &ChatAddress,
        pending: ConversationInputPayload,
        submit_device_id: Option<String>,
    ) -> Option<ConversationInputEntrySnapshot> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        if self.draining_chats.read().await.contains(address)
            || match address.agent_id() {
                Some(agent) => self.is_draining(agent.as_str()).await,
                None => false,
            }
        {
            return None;
        }
        self.enqueue_input_locked(address, pending, submit_device_id)
            .await
    }

    async fn enqueue_input_locked(
        &self,
        address: &ChatAddress,
        mut pending: ConversationInputPayload,
        submit_device_id: Option<String>,
    ) -> Option<ConversationInputEntrySnapshot> {
        if self.active_turn(address).await.is_none() && !self.is_queue_drain_paused(address).await {
            return None;
        }
        pending
            .extra
            .entry(ACCEPTANCE_MODE_KEY.to_string())
            .or_insert_with(|| serde_json::json!(ConversationInputAcceptanceMode::Queued));
        let submission_id = ensure_submission_id(&mut pending);
        let now = chrono::Utc::now().to_rfc3339();
        let input_id = pending
            .extra
            .get(INPUT_ID_KEY)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("input-{}", uuid::Uuid::new_v4()));
        let runtime_only = pending
            .extra
            .contains_key(crate::services::conversation_input_intake::RUNTIME_OBSERVATION_META_KEY);
        let snapshot = ConversationInputEntrySnapshot {
            input_id,
            submission_id,
            state: ConversationInputEntryState::Queued,
            text: pending.message.clone(),
            created_at: now.clone(),
            updated_at: now,
            submit_device_id,
            device_delivery_id: device_delivery_id_from_pending(&pending),
            editable: !runtime_only,
            deletable: !runtime_only,
        };
        pending.extra.insert(
            INPUT_ID_KEY.to_string(),
            serde_json::json!(snapshot.input_id.clone()),
        );
        let key = address.clone();
        let mut accepted = self.accepted_submissions.write().await;
        if let Some(existing) = accepted
            .get(&key)
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.submission_id == snapshot.submission_id)
            })
            .cloned()
        {
            return Some(existing);
        }
        let mut queues = self.queue.write().await;
        let entries = accepted.entry(key.clone()).or_default();
        entries.push_back(snapshot.clone());
        while entries.len() > 256 {
            entries.pop_front();
        }
        queues
            .entry(key)
            .or_default()
            .push_back(ConversationInputEntry {
                snapshot: snapshot.clone(),
                pending,
                runtime_only,
            });
        drop(queues);
        drop(accepted);
        self.set_queued_count_from_queue(address).await;
        self.publish_control_snapshot(address, "queue_accepted", &serde_json::json!({}))
            .await;
        Some(snapshot)
    }

    pub async fn switch_authority(
        &self,
        state: Option<&AppState>,
        workspace_dir: &std::path::Path,
        address: &ChatAddress,
        target: ConversationAuthorityPlacement,
    ) -> Result<ChatMetadataRecord, ChatMetadataStoreError> {
        enforce_authority_movement_policy(workspace_dir, address).await?;
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let broadcast_placement = target.clone();
        let record = self
            .update_authority_placement_guarded(state, workspace_dir, address, target)
            .await?;
        self.broadcast_placement_changed_over_gossip(address, broadcast_placement)
            .await;
        Ok(record)
    }

    pub async fn apply_confirmed_authority_placement(
        &self,
        workspace_dir: &std::path::Path,
        address: &ChatAddress,
        target: ConversationAuthorityPlacement,
    ) -> Result<ChatMetadataRecord, ChatMetadataStoreError> {
        enforce_authority_movement_policy(workspace_dir, address).await?;
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let record = persist_confirmed_authority_placement(workspace_dir, address, target).await?;
        self.publish_control_snapshot(address, "authority_placement_adopted", &record.meta)
            .await;
        Ok(record)
    }

    pub async fn apply_authority_supersession(
        &self,
        workspace_dir: &std::path::Path,
        address: &ChatAddress,
        replacement: ConversationAuthorityPlacement,
    ) -> Result<(ChatMetadataRecord, Vec<String>), ChatMetadataStoreError> {
        enforce_authority_movement_policy(workspace_dir, address).await?;
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let key = address.clone();
        let superseded = self
            .queue
            .write()
            .await
            .remove(&key)
            .unwrap_or_default()
            .into_iter()
            .map(|entry| entry.snapshot)
            .collect::<Vec<_>>();
        let superseded_input_ids = superseded
            .iter()
            .map(|entry| entry.input_id.clone())
            .collect::<Vec<_>>();
        if !superseded.is_empty() {
            let superseded_submissions = superseded
                .iter()
                .map(|entry| entry.submission_id.as_str())
                .collect::<std::collections::HashSet<_>>();
            let mut accepted = self.accepted_submissions.write().await;
            if let Some(entries) = accepted.get_mut(&key) {
                entries
                    .retain(|entry| !superseded_submissions.contains(entry.submission_id.as_str()));
                if entries.is_empty() {
                    accepted.remove(&key);
                }
            }
        }
        self.paused_queue_drain.write().await.remove(&key);
        self.set_queued_count(address, 0).await;
        let record =
            persist_confirmed_authority_placement(workspace_dir, address, replacement.clone())
                .await?;
        self.observe_placement_projection_at(
            address,
            &replacement.device_id,
            replacement.authority_epoch,
        )
        .await;
        self.publish_authority_redirect(address, replacement, superseded_input_ids.clone())
            .await;
        Ok((record, superseded_input_ids))
    }

    async fn update_authority_placement_guarded(
        &self,
        state: Option<&AppState>,
        workspace_dir: &std::path::Path,
        address: &ChatAddress,
        target: ConversationAuthorityPlacement,
    ) -> Result<ChatMetadataRecord, ChatMetadataStoreError> {
        if self.active_turn(address).await.is_some() {
            return Err(ChatMetadataStoreError::Conflict(
                "cannot change authority placement while the chat has an active turn".to_string(),
            ));
        }
        if let Some(state) = state {
            rua_session::WorkspaceSessionStore::new(workspace_dir)
                .set_collaboration_wake_suppressed(address.chat_id().as_str(), true)
                .await
                .map_err(|error| ChatMetadataStoreError::Internal(error.to_string()))?;
            state
                .task_runtime
                .interrupt_owner(&rua_task::TaskOwner::from_address(address.clone()))
                .await
                .map_err(|error| {
                    ChatMetadataStoreError::Internal(format!(
                        "failed to interrupt active Tasks before authority transfer: {error}"
                    ))
                })?;
        }
        let record = update_authority_placement(state, workspace_dir, address, target).await?;
        self.publish_control_snapshot(address, "authority_placement_updated", &record.meta)
            .await;
        Ok(record)
    }

    pub(crate) async fn bind_initial_acp_turn(
        &self,
        workspace_dir: &std::path::Path,
        address: &ChatAddress,
        execution_agent_id: &str,
        expected_turn_id: &str,
        expected_epoch: ConversationAuthorityEpoch,
        binding: AcpBindingMetadata,
    ) -> Result<ChatMetadataRecord, ChatMetadataStoreError> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let active = self.active_turn(address).await.ok_or_else(|| {
            ChatMetadataStoreError::Conflict("initial ACP Turn is no longer active".into())
        })?;
        if active.turn_id != expected_turn_id || active.authority_epoch != expected_epoch {
            return Err(ChatMetadataStoreError::Conflict(
                "initial ACP Turn identity changed".into(),
            ));
        }
        let header = rua_session::WorkspaceSessionStore::new(workspace_dir)
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|error| ChatMetadataStoreError::Internal(error.to_string()))?
            .ok_or_else(|| ChatMetadataStoreError::Conflict("accepted Chat is missing".into()))?;
        if chat_acp_binding_active(&serde_json::Value::Object(header.extra)) {
            return Err(ChatMetadataStoreError::Conflict(
                "Chat already has an ACP binding".into(),
            ));
        }
        let record =
            persist_acp_binding(None, workspace_dir, address, execution_agent_id, binding).await?;
        if let Some(turn) = self.active_turns.write().await.get_mut(address) {
            turn.driver = ConversationDriverKind::Acp;
        }
        self.publish_control_snapshot(address, "acp_entered", &record.meta)
            .await;
        Ok(record)
    }

    pub async fn enter_acp(
        &self,
        state: Option<&AppState>,
        workspace_dir: &std::path::Path,
        address: &ChatAddress,
        execution_agent_id: &str,
        binding: AcpBindingMetadata,
    ) -> Result<ChatMetadataRecord, ChatMetadataStoreError> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        if self.active_turn(address).await.is_some() {
            return Err(ChatMetadataStoreError::Conflict(
                "cannot enter ACP mode while the chat has an active turn".to_string(),
            ));
        }
        let header = rua_session::WorkspaceSessionStore::new(workspace_dir)
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|error| ChatMetadataStoreError::Internal(error.to_string()))?;
        let mut refresh_collaboration_binding = false;
        if let Some(value) = header.as_ref().and_then(|header| {
            header
                .extra
                .get(rua_session::collaboration::COLLABORATION_META_KEY)
        }) {
            let descriptor: rua_session::collaboration::CollaborationDescriptor =
                serde_json::from_value(value.clone()).map_err(|error| {
                    ChatMetadataStoreError::Internal(format!(
                        "invalid collaboration descriptor: {error}"
                    ))
                })?;
            match descriptor.executor {
                rua_session::collaboration::CollaborationExecutor::Acp { runner }
                    if runner == binding.runner =>
                {
                    refresh_collaboration_binding = header.as_ref().is_some_and(|header| {
                            header.extra.contains_key(
                                crate::services::conversation_authority_placement::CHAT_ACP_BINDING_META_KEY,
                            )
                        });
                }
                _ => {
                    return Err(ChatMetadataStoreError::Conflict(
                        "Collaboration driver is fixed; create another Chat for ACP".into(),
                    ))
                }
            }
        }
        if refresh_collaboration_binding {
            clear_acp_binding(state, workspace_dir, address, execution_agent_id).await?;
        }
        let record =
            persist_acp_binding(state, workspace_dir, address, execution_agent_id, binding).await?;
        self.publish_control_snapshot(address, "acp_entered", &record.meta)
            .await;
        Ok(record)
    }

    pub async fn exit_acp(
        &self,
        state: Option<&AppState>,
        workspace_dir: &std::path::Path,
        address: &ChatAddress,
        execution_agent_id: &str,
    ) -> Result<Option<ChatMetadataRecord>, ChatMetadataStoreError> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        if rua_session::WorkspaceSessionStore::new(workspace_dir)
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|error| ChatMetadataStoreError::Internal(error.to_string()))?
            .is_some_and(|header| {
                header
                    .extra
                    .contains_key(rua_session::collaboration::COLLABORATION_META_KEY)
            })
        {
            return Err(ChatMetadataStoreError::Conflict(
                "Collaboration executor is fixed; create another Chat to change drivers".into(),
            ));
        }
        let record = clear_acp_binding(state, workspace_dir, address, execution_agent_id).await?;
        if let Some(record) = record.as_ref() {
            self.publish_control_snapshot(address, "acp_exited", &record.meta)
                .await;
        }
        Ok(record)
    }

    pub async fn edit_queued_input(
        &self,
        address: &ChatAddress,
        input_id: &str,
        text: &str,
    ) -> Option<ConversationInputEntrySnapshot> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let mut queues = self.queue.write().await;
        let entry = queues
            .get_mut(address)?
            .iter_mut()
            .find(|entry| entry.snapshot.input_id == input_id)?;
        entry.pending.message = text.to_string();
        entry.snapshot.text = text.to_string();
        entry.snapshot.updated_at = chrono::Utc::now().to_rfc3339();
        let snapshot = entry.snapshot.clone();
        drop(queues);
        self.update_accepted_submission(address, &snapshot).await;
        self.publish_control_snapshot(address, "queue_edited", &serde_json::json!({}))
            .await;
        Some(snapshot)
    }

    pub async fn delete_queued_input(
        &self,
        address: &ChatAddress,
        input_id: &str,
    ) -> Option<ConversationInputEntrySnapshot> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let key = address.clone();
        let deleted = {
            let mut queues = self.queue.write().await;
            let queue = queues.get_mut(&key)?;
            let position = queue
                .iter()
                .position(|entry| entry.snapshot.input_id == input_id)?;
            let entry = queue.remove(position)?;
            if queue.is_empty() {
                queues.remove(&key);
            }
            entry.snapshot
        };
        {
            let mut accepted = self.accepted_submissions.write().await;
            if let Some(entries) = accepted.get_mut(&key) {
                entries.retain(|entry| entry.submission_id != deleted.submission_id);
                if entries.is_empty() {
                    accepted.remove(&key);
                }
            }
        }
        self.set_queued_count_from_queue(address).await;
        self.publish_control_snapshot(address, "queue_deleted", &serde_json::json!({}))
            .await;
        Some(deleted)
    }

    pub async fn take_queued_input(
        &self,
        address: &ChatAddress,
        input_id: &str,
    ) -> Option<ConversationDequeuedInput> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let key = address.clone();
        let dequeued = {
            let mut queues = self.queue.write().await;
            let queue = queues.get_mut(&key)?;
            let position = queue
                .iter()
                .position(|entry| entry.snapshot.input_id == input_id)?;
            let entry = queue.remove(position)?;
            if queue.is_empty() {
                queues.remove(&key);
            }
            ConversationDequeuedInput {
                snapshot: entry.snapshot,
                pending: entry.pending,
            }
        };
        self.set_queued_count_from_queue(address).await;
        self.publish_control_snapshot(address, "queue_consumed", &serde_json::json!({}))
            .await;
        Some(dequeued)
    }

    pub async fn promote_queued_input_to_front(
        &self,
        address: &ChatAddress,
        input_id: &str,
    ) -> Option<ConversationInputEntrySnapshot> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let key = address.clone();
        let promoted = {
            let mut queues = self.queue.write().await;
            let queue = queues.get_mut(&key)?;
            let position = queue
                .iter()
                .position(|entry| entry.snapshot.input_id == input_id)?;
            if position == 0 {
                queue.front()?.snapshot.clone()
            } else {
                let mut entry = queue.remove(position)?;
                entry.snapshot.updated_at = chrono::Utc::now().to_rfc3339();
                let snapshot = entry.snapshot.clone();
                queue.push_front(entry);
                snapshot
            }
        };
        self.update_accepted_submission(address, &promoted).await;
        self.publish_control_snapshot(address, "queue_promoted", &serde_json::json!({}))
            .await;
        Some(promoted)
    }

    pub async fn pop_next_input(&self, address: &ChatAddress) -> Option<ConversationDequeuedInput> {
        let gate = self.transition_gate_at(address);
        let _transition = gate.lock().await;
        let dequeued = self.pop_next_input_state(address).await?;
        self.set_queued_count_from_queue(address).await;
        self.publish_control_snapshot(address, "queue_consumed", &serde_json::json!({}))
            .await;
        Some(dequeued)
    }

    async fn pop_next_input_state(&self, key: &ChatAddress) -> Option<ConversationDequeuedInput> {
        let paused = self.is_queue_drain_paused(key).await;
        let mut queues = self.queue.write().await;
        let queue = queues.get_mut(key)?;
        let position = if paused {
            queue.iter().position(|entry| {
                entry.runtime_only || entry.pending.extra.contains_key("turn_id")
            })?
        } else {
            0
        };
        let entry = queue.remove(position)?;
        if queue.is_empty() {
            queues.remove(key);
        }
        Some(ConversationDequeuedInput {
            snapshot: entry.snapshot,
            pending: entry.pending,
        })
    }

    pub async fn queued_device_delivery_head(&self, address: &ChatAddress) -> Option<String> {
        self.queue
            .read()
            .await
            .get(address)?
            .front()
            .and_then(|entry| entry.snapshot.device_delivery_id.clone())
    }

    async fn accepted_submission(
        &self,
        address: &ChatAddress,
        submission_id: &str,
    ) -> Option<ConversationInputEntrySnapshot> {
        self.accepted_submissions
            .read()
            .await
            .get(address)?
            .iter()
            .find(|entry| entry.submission_id == submission_id)
            .cloned()
    }

    async fn accepted_input_by_id(
        &self,
        key: &ChatAddress,
        input_id: &str,
    ) -> Option<ConversationInputEntrySnapshot> {
        self.accepted_submissions
            .read()
            .await
            .get(key)?
            .iter()
            .find(|entry| entry.input_id == input_id)
            .cloned()
    }

    async fn update_accepted_submission(
        &self,
        address: &ChatAddress,
        snapshot: &ConversationInputEntrySnapshot,
    ) {
        let mut accepted = self.accepted_submissions.write().await;
        let Some(entries) = accepted.get_mut(address) else {
            return;
        };
        if let Some(entry) = entries
            .iter_mut()
            .find(|entry| entry.submission_id == snapshot.submission_id)
        {
            *entry = snapshot.clone();
        }
    }

    pub async fn subscribe_control_events(
        &self,
        address: &ChatAddress,
    ) -> broadcast::Receiver<String> {
        let mut events = self.control_events.write().await;
        events
            .entry(address.clone())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(128);
                tx
            })
            .subscribe()
    }

    pub async fn wait_for_turn_handoff(
        &self,
        control_events: &mut broadcast::Receiver<String>,
        address: &ChatAddress,
        target_turn_id: &str,
        timeout: Duration,
    ) -> bool {
        tokio::time::timeout(timeout, async {
            loop {
                if self
                    .active_turn(address)
                    .await
                    .as_ref()
                    .map(|turn| turn.turn_id != target_turn_id)
                    .unwrap_or(true)
                {
                    return;
                }
                match control_events.recv().await {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
        .await
        .is_ok()
    }

    pub fn subscribe_global_control_events(&self) -> broadcast::Receiver<ConversationControlEvent> {
        self.global_control_events.subscribe()
    }

    pub async fn publish_control_snapshot(
        &self,
        address: &ChatAddress,
        reason: &str,
        meta: &serde_json::Value,
    ) {
        let revision = self.next_revision(address).await;
        let control = self.control_view_at_revision(address, meta, revision).await;
        self.publish_control_view(address, reason, control).await;
        if let Some(work) = self.background_work.get() {
            match work.collaboration_source(address).await {
                Ok(Some((source, meta))) if source != *address => {
                    let revision = self.next_revision(&source).await;
                    let control = self
                        .control_view_at_revision(&source, &meta, revision)
                        .await;
                    self.publish_control_view(&source, "background_work_changed", control)
                        .await;
                }
                Err(error) => {
                    tracing::warn!(%address, %error, "Could not refresh collaboration source activity")
                }
                _ => {}
            }
        }
    }

    pub async fn publish_running_config_snapshot(
        &self,
        address: &ChatAddress,
        meta: &serde_json::Value,
        running_config: serde_json::Value,
    ) {
        let revision = self.next_revision(address).await;
        let mut control = self.control_view_at_revision(address, meta, revision).await;
        control.running_config = Some(running_config);
        self.publish_control_view(address, "running_config_updated", control)
            .await;
    }

    pub async fn publish_project_agent_state_snapshot(
        &self,
        address: &ChatAddress,
        meta: &serde_json::Value,
        state: crate::services::project_chat::ProjectChatAgentState,
    ) {
        let revision = self.next_revision(address).await;
        let mut control = self.control_view_at_revision(address, meta, revision).await;
        control.project_chat_agent_state = Some(state);
        self.publish_control_view(address, "project_agent_selection_updated", control)
            .await;
    }

    pub async fn publish_history_mutation(
        &self,
        address: &ChatAddress,
        authority_epoch: ConversationAuthorityEpoch,
        mutation: serde_json::Value,
    ) {
        let payload = conversation_history_mutation_payload(address, mutation);
        let hub = self
            .active_stream_broadcast_hub
            .read()
            .expect("Conversation Authority activity hub lock poisoned")
            .clone();
        if let Some(hub) = hub {
            hub.publish_conversation_event(
                &format!("history:{address}"),
                address,
                authority_epoch,
                payload,
            )
            .await;
        }
    }

    async fn publish_control_view(
        &self,
        address: &ChatAddress,
        reason: &str,
        control: ConversationControlView,
    ) {
        let authority_epoch = control.placement.authority_epoch;
        let payload = conversation_control_payload(address, reason, control);
        let hub = self
            .active_stream_broadcast_hub
            .read()
            .expect("Conversation Authority activity hub lock poisoned")
            .clone();
        if let (Some(hub), Some(authority_epoch)) = (hub, authority_epoch) {
            hub.publish_conversation_event(
                &format!("control:{address}"),
                address,
                authority_epoch,
                payload.clone(),
            )
            .await;
        }
        if let Some(tx) = self.control_event_sender(address).await {
            let _ = tx.send(payload.clone());
        }
        let _ = self.global_control_events.send(ConversationControlEvent {
            address: address.clone(),
            payload,
        });
    }

    async fn publish_authority_redirect(
        &self,
        address: &ChatAddress,
        placement: ConversationAuthorityPlacement,
        superseded_input_ids: Vec<String>,
    ) {
        let hub = self
            .active_stream_broadcast_hub
            .read()
            .expect("Conversation Authority activity hub lock poisoned")
            .clone();
        if let Some(hub) = hub {
            hub.publish_authority_redirect(address, placement.clone())
                .await;
        }
        let payload = authority_redirect_payload(address, placement, superseded_input_ids);
        if let Some(tx) = self.control_event_sender(address).await {
            let _ = tx.send(payload.clone());
        }
        let _ = self.global_control_events.send(ConversationControlEvent {
            address: address.clone(),
            payload,
        });
        self.control_events.write().await.remove(address);
    }

    async fn control_event_sender(
        &self,
        address: &ChatAddress,
    ) -> Option<broadcast::Sender<String>> {
        self.control_events.read().await.get(address).cloned()
    }

    pub async fn control_view(
        &self,
        address: &ChatAddress,
        meta: &serde_json::Value,
    ) -> ConversationControlView {
        let revision = self.revision(address).await;
        self.control_view_at_revision(address, meta, revision).await
    }

    async fn control_view_at_revision(
        &self,
        address: &ChatAddress,
        meta: &serde_json::Value,
        revision: u64,
    ) -> ConversationControlView {
        let mut active_turn = self.active_turn(address).await;
        let queue_entries = self.user_input_queue_snapshot(address).await;
        if let Some(active_turn) = active_turn.as_mut() {
            active_turn.queued_count = queue_entries.len();
        }
        let authority_placement = parse_conversation_authority_placement(meta).ok();
        let placement_projection = match authority_placement.as_ref() {
            Some(placement) => Some(
                self.observe_placement_projection(
                    address,
                    &placement.device_id,
                    placement.authority_epoch,
                )
                .await,
            ),
            None => self.placement_projection(address).await,
        };
        let acp_binding = meta.get(CHAT_ACP_BINDING_META_KEY).cloned();
        let acp_active = chat_acp_binding_active(meta);
        let active_turn_present = active_turn.is_some();
        let authority_resource_bound = rua_chat::ChatPolicy::from_metadata(address.scope(), meta)
            .authority_placement
            == rua_chat::AuthorityPlacementPolicy::ResourceBound;
        let authority_device_id = placement_projection
            .as_ref()
            .map(|placement| placement.device_id.clone());
        let authority_epoch = placement_projection
            .as_ref()
            .map(|placement| placement.authority_epoch);
        let placement = conversation_authority_placement_view(
            authority_device_id.clone(),
            authority_epoch,
            active_turn_present,
            acp_active,
            authority_resource_bound,
        );

        let background_work = self.background_work_view(address).await;
        let can_pause_queue =
            !queue_entries.is_empty() && !self.is_queue_drain_paused(address).await;
        ConversationControlView {
            background_work,
            revision,
            authority_device_id,
            placement,
            active_turn,
            queue: ConversationInputQueueView {
                entries: queue_entries,
                paused: self.is_queue_drain_paused(address).await,
            },
            acp: ConversationAcpControlView {
                active: acp_active,
                binding: acp_binding,
            },
            model: meta.get(CHAT_MODEL_META_KEY).cloned(),
            running_config: None,
            project_chat_agent_state: None,
            allowed_controls: ConversationAllowedControls {
                stop: active_turn_present || can_pause_queue,
                queue: active_turn_present,
                interject: active_turn_present,
                edit_queued_input: false,
                delete_queued_input: false,
                switch_authority: !authority_resource_bound && !active_turn_present && !acp_active,
                enter_acp: !active_turn_present && !acp_active,
                exit_acp: acp_active,
            },
        }
    }

    async fn observe_placement_projection(
        &self,
        address: &ChatAddress,
        device_id: &str,
        authority_epoch: ConversationAuthorityEpoch,
    ) -> ConversationAuthorityPlacementProjection {
        self.observe_placement_projection_at(address, device_id, authority_epoch)
            .await
    }

    async fn observe_placement_projection_at(
        &self,
        address: &ChatAddress,
        device_id: &str,
        authority_epoch: ConversationAuthorityEpoch,
    ) -> ConversationAuthorityPlacementProjection {
        let key = address.clone();
        let mut placements = self.placement_projections.write().await;
        if let Some(current) = placements.get(&key) {
            if current.authority_epoch >= authority_epoch {
                return current.clone();
            }
        }
        let placement = ConversationAuthorityPlacementProjection {
            device_id: device_id.to_string(),
            authority_epoch,
        };
        placements.insert(key, placement.clone());
        placement
    }

    async fn placement_projection(
        &self,
        address: &ChatAddress,
    ) -> Option<ConversationAuthorityPlacementProjection> {
        self.placement_projections
            .read()
            .await
            .get(address)
            .cloned()
    }

    pub async fn queue_snapshot(
        &self,
        address: &ChatAddress,
    ) -> Vec<ConversationInputEntrySnapshot> {
        self.queue
            .read()
            .await
            .get(address)
            .map(|queue| queue.iter().map(|entry| entry.snapshot.clone()).collect())
            .unwrap_or_default()
    }

    async fn user_input_queue_snapshot(
        &self,
        address: &ChatAddress,
    ) -> Vec<ConversationInputEntrySnapshot> {
        self.queue
            .read()
            .await
            .get(address)
            .map(|queue| {
                queue
                    .iter()
                    .filter(|entry| !entry.runtime_only)
                    .map(|entry| entry.snapshot.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn revision(&self, address: &ChatAddress) -> u64 {
        self.revisions
            .read()
            .await
            .get(address)
            .copied()
            .unwrap_or(0)
    }

    async fn next_revision(&self, address: &ChatAddress) -> u64 {
        let mut revisions = self.revisions.write().await;
        let revision = revisions.entry(address.clone()).or_default();
        *revision = revision.saturating_add(1);
        *revision
    }

    pub(crate) async fn is_queue_drain_paused(&self, address: &ChatAddress) -> bool {
        self.paused_queue_drain.read().await.contains(address)
    }

    async fn set_queued_count_from_queue(&self, address: &ChatAddress) {
        let queued_count = self.user_input_queue_snapshot(address).await.len();
        self.set_queued_count(address, queued_count).await;
    }

    async fn set_queued_count(&self, address: &ChatAddress, queued_count: usize) {
        if let Some(active_turn) = self.active_turns.write().await.get_mut(address) {
            active_turn.queued_count = queued_count;
        }
    }
}

impl Drop for ConversationAuthority {
    fn drop(&mut self) {
        self.terminal_observer_cancellation.cancel();
    }
}

impl Default for ConversationAuthority {
    fn default() -> Self {
        Self::new()
    }
}

fn conversation_control_payload(
    address: &ChatAddress,
    reason: &str,
    control: ConversationControlView,
) -> String {
    match address.agent_id() {
        Some(agent_id) => serde_json::json!({
            "type": "conversation_control",
            "agent_id": agent_id,
            "chat_id": address.chat_id(),
            "reason": reason,
            "control": control,
        }),
        None => serde_json::json!({
            "type": "conversation_control",
            "address": address,
            "reason": reason,
            "control": control,
        }),
    }
    .to_string()
}

fn conversation_history_mutation_payload(
    address: &ChatAddress,
    mutation: serde_json::Value,
) -> String {
    let mut payload = mutation.as_object().cloned().unwrap_or_default();
    payload.insert(
        "type".to_string(),
        serde_json::Value::String("conversation_history_mutated".to_string()),
    );
    match address.agent_id() {
        Some(agent_id) => {
            payload.insert(
                "agent_id".to_string(),
                serde_json::Value::String(agent_id.to_string()),
            );
            payload.insert(
                "chat_id".to_string(),
                serde_json::Value::String(address.chat_id().to_string()),
            );
        }
        None => {
            payload.insert(
                "address".to_string(),
                serde_json::to_value(address).expect("Chat Address is serializable"),
            );
        }
    }
    serde_json::Value::Object(payload).to_string()
}

fn authority_redirect_payload(
    address: &ChatAddress,
    placement: ConversationAuthorityPlacement,
    superseded_input_ids: Vec<String>,
) -> String {
    match address.agent_id() {
        Some(agent_id) => serde_json::json!({
            "type": "authority_redirect",
            "agent_id": agent_id,
            "chat_id": address.chat_id(),
            "reason": "authority_changed",
            "placement": placement,
            "superseded_input_ids": superseded_input_ids,
        }),
        None => serde_json::json!({
            "type": "authority_redirect",
            "address": address,
            "reason": "authority_changed",
            "placement": placement,
            "superseded_input_ids": superseded_input_ids,
        }),
    }
    .to_string()
}

fn conversation_authority_placement_view(
    authority_device_id: Option<String>,
    authority_epoch: Option<ConversationAuthorityEpoch>,
    active_turn_present: bool,
    acp_active: bool,
    resource_bound: bool,
) -> ConversationAuthorityPlacementView {
    let lock_reason = if acp_active {
        Some(ConversationAuthorityPlacementLockReason::AcpPassthrough)
    } else if active_turn_present {
        Some(ConversationAuthorityPlacementLockReason::ActiveTurn)
    } else {
        None
    };
    ConversationAuthorityPlacementView {
        authority_device_id,
        authority_epoch,
        switchable: !resource_bound && lock_reason.is_none(),
        lock_reason,
    }
}

fn device_delivery_id_from_pending(pending: &ConversationInputPayload) -> Option<String> {
    pending
        .extra
        .get("device_delivery")
        .and_then(|value| value.get("device_delivery_id"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn regenerate_turn_id(pending: &ConversationInputPayload) -> Option<String> {
    pending
        .extra
        .get(REGENERATE_TURN_ID_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|turn_id| !turn_id.is_empty())
        .map(str::to_string)
}

fn ensure_submission_id(pending: &mut ConversationInputPayload) -> String {
    if let Some(submission_id) = pending
        .extra
        .get(SUBMISSION_ID_KEY)
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
    {
        return submission_id;
    }
    let submission_id = format!("submission-{}", uuid::Uuid::new_v4());
    pending.extra.insert(
        SUBMISSION_ID_KEY.to_string(),
        serde_json::json!(submission_id.clone()),
    );
    submission_id
}

pub type SharedConversationAuthority = Arc<ConversationAuthority>;
