use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use rua_core::{ChatAddress, ChatScope, ChatScopeStore};
use rua_session::collaboration::{
    CollaborationConfigurationState, CollaborationConfigurationStatus, CollaborationDeferredClaim,
    CollaborationDeliveryClaim, CollaborationDeliveryState, CollaborationDescriptor,
    CollaborationOutcome, CollaborationRequest, COLLABORATION_META_KEY,
};
use rua_session::{ChatMetadata, ChatSessionHeader, WorkspaceSessionStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

const PROGRESS_ACTIVITY_MAX_BYTES: usize = 12_000;

use super::agent_manager::AgentManager;
use super::collaboration_progress::{
    CollaborationProgressStatus, CollaborationProgressSummarizer, CollaborationProgressSummary,
};
use super::console_chat::ConversationExecutionContext;
use super::conversation_authority::{ConversationInputPayload, SUBMISSION_ID_KEY};
use super::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement, CHAT_AUTHORITY_PLACEMENT_META_KEY,
};
use super::conversation_input_intake::{ConversationInputDeliveryOutcome, ConversationInputIntake};
use super::product_hooks::{
    ChatConfigurationMaterialization, ChatHookConfiguration, ProductHookService,
    CHAT_HOOK_CONFIGURATION_META_KEY,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollaborationCaller {
    pub address: ChatAddress,
    pub turn_id: String,
    pub tool_call_id: String,
    pub configuration_epoch: String,
    pub working_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollaborationTurnControl {
    pub turn_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,
    pub phase: super::conversation_authority::ConversationActiveTurnPhase,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollaborationReply {
    pub chat: ChatAddress,
    pub request_id: Option<String>,
    pub state: String,
    pub input_id: Option<String>,
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_turn: Option<CollaborationTurnControl>,
    pub outcome: Option<CollaborationOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<rua_session::collaboration::CollaborationResultCursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<CollaborationProgressSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationStopStatus {
    StopRequested,
    NoActiveTurn,
    TargetChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollaborationStopReply {
    pub chat: ChatAddress,
    pub turn_id: String,
    pub stopped: bool,
    pub status: CollaborationStopStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollaborationRecoveryReport {
    pub recovered_terminal: usize,
    pub interrupted: usize,
    pub redelivered: usize,
    pub failed: usize,
}

#[derive(Debug, Clone)]
struct CollaborationProgressCacheEntry {
    summary: CollaborationProgressSummary,
    covered_sequence: u64,
}

pub struct CollaborationService {
    root: PathBuf,
    registry: Weak<dyn rua_workspace::WorkspaceRegistry>,
    agents: Arc<AgentManager>,
    hooks: Arc<ProductHookService>,
    intake: ConversationInputIntake,
    device_id: String,
    max_in_flight: usize,
    source_gates: Mutex<HashMap<ChatAddress, Arc<Mutex<()>>>>,
    in_flight: Mutex<HashMap<ChatAddress, Vec<(ChatAddress, String)>>>,
    progress_summarizer: std::sync::RwLock<Option<Arc<dyn CollaborationProgressSummarizer>>>,
    progress_timeout: std::sync::RwLock<Duration>,
    progress_cache: Mutex<HashMap<ChatAddress, CollaborationProgressCacheEntry>>,
    progress_gates: Mutex<HashMap<ChatAddress, Arc<Mutex<()>>>>,
    device_network: Option<Arc<crate::services::device_session_access::DeviceNetworkAccess>>,
    sync_device: Option<Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>>,
}

impl CollaborationService {
    pub async fn inventory(&self) -> Result<serde_json::Value, String> {
        super::console_chat::collaboration_agent_inventory(&self.agents).await
    }

    pub fn new(
        root: PathBuf,
        registry: Weak<dyn rua_workspace::WorkspaceRegistry>,
        agents: Arc<AgentManager>,
        hooks: Arc<ProductHookService>,
        intake: ConversationInputIntake,
        device_id: String,
        max_in_flight: usize,
    ) -> Self {
        Self {
            root,
            registry,
            agents,
            hooks,
            intake,
            device_id,
            max_in_flight: max_in_flight.max(1),
            source_gates: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashMap::new()),
            progress_summarizer: std::sync::RwLock::new(None),
            progress_timeout: std::sync::RwLock::new(Duration::from_secs(15)),
            progress_cache: Mutex::new(HashMap::new()),
            progress_gates: Mutex::new(HashMap::new()),
            device_network: None,
            sync_device: None,
        }
    }

    pub fn with_progress_summarizer(
        self,
        summarizer: Arc<dyn CollaborationProgressSummarizer>,
    ) -> Self {
        *self
            .progress_summarizer
            .write()
            .expect("collaboration progress summarizer lock poisoned") = Some(summarizer);
        self
    }

    pub fn install_progress_summarizer(
        &self,
        summarizer: Arc<dyn CollaborationProgressSummarizer>,
    ) {
        *self
            .progress_summarizer
            .write()
            .expect("collaboration progress summarizer lock poisoned") = Some(summarizer);
    }

    pub fn with_device_network(
        mut self,
        device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    ) -> Self {
        self.device_network = Some(device_network);
        self
    }

    pub fn with_sync_device(
        mut self,
        sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    ) -> Self {
        self.sync_device = Some(sync_device);
        self
    }

    fn current_device_id(&self) -> String {
        self.sync_device
            .as_ref()
            .and_then(crate::services::device_context::current_sync_device_id)
            .unwrap_or_else(|| self.device_id.clone())
    }

    async fn route_collaboration_command(
        &self,
        route_address: &ChatAddress,
        command_id: String,
        action: crate::services::device_command_bus::CollaborationDeviceAction,
    ) -> Result<Option<crate::services::device_command_bus::CollaborationDeviceResponse>, String>
    {
        let scope = ChatScopeStore::resolve(&self.root, route_address.clone());
        let placement =
            crate::services::conversation_authority_target::load_optional_chat_authority_placement(
                scope.root(),
                route_address.chat_id().as_str(),
            )
            .await?;
        let Some(placement) = placement else {
            return Err("Conversation Authority placement is unavailable".into());
        };
        let current_device_id = self.current_device_id();
        if placement.device_id == current_device_id {
            return Ok(None);
        }
        let network = self
            .device_network
            .as_ref()
            .ok_or("Device network is unavailable for remote collaboration")?;
        let session_manager =
            crate::services::device_session_access::current_session_manager(network)
                .await
                .map_err(|error| error.to_string())?;
        let response =
            crate::services::device_command_bus::deliver_collaboration_device_command_via_session(
                &session_manager,
                &crate::services::device_command_bus::CollaborationDeviceCommand {
                    command_id,
                    source_device_id: current_device_id,
                    target_device_id: placement.device_id,
                    route_address: route_address.clone(),
                    authority_epoch: placement.authority_epoch,
                    action,
                },
                Duration::from_secs(35),
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(Some(response))
    }

    pub fn set_progress_summary_timeout(&self, timeout: Duration) {
        *self
            .progress_timeout
            .write()
            .expect("collaboration progress timeout lock poisoned") = timeout;
    }

    async fn progress_gate(&self, target: &ChatAddress) -> Arc<Mutex<()>> {
        self.progress_gates
            .lock()
            .await
            .entry(target.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn public_activity_snapshot(
        &self,
        target: &ChatAddress,
        after_sequence: Option<u64>,
    ) -> Result<(String, String, u64), String> {
        let store = self.store(target);
        let turns = store
            .read_turn_range(target.chat_id().as_str(), 1, usize::MAX)
            .await
            .map_err(|error| error.to_string())?;
        let latest_sequence = turns
            .iter()
            .map(|turn| turn.last_sequence)
            .max()
            .unwrap_or(0);
        let mut version_hasher = Sha256::new();
        for turn in &turns {
            version_hasher.update(turn.turn_id.as_bytes());
            version_hasher.update([0]);
            version_hasher.update(turn.last_sequence.to_le_bytes());
            version_hasher.update([0]);
            version_hasher.update(turn.lifecycle.status.as_bytes());
            version_hasher.update([b'\n']);
        }
        let version = format!("{:x}", version_hasher.finalize());
        let mut items = Vec::new();
        for turn in turns
            .into_iter()
            .filter(|turn| after_sequence.is_none_or(|sequence| turn.last_sequence > sequence))
            .rev()
            .take(24)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            let messages = store
                .read_turn_messages(target.chat_id().as_str(), &turn.turn_id)
                .await
                .map_err(|error| error.to_string())?;
            let mut assistant = Vec::new();
            let mut tools = Vec::new();
            for message in messages {
                if message.role == "assistant" {
                    let text = message
                        .content
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|part| {
                            part.get("type").and_then(serde_json::Value::as_str) == Some("text")
                        })
                        .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n")
                        .chars()
                        .take(512)
                        .collect::<String>();
                    if !text.trim().is_empty() {
                        assistant.push(text);
                    }
                }
                if let Some(tool) = message.tool {
                    tools.push(format!("{} ({})", tool.tool_name, message.status));
                }
            }
            items.push(format!(
                "turn status={} last_sequence={}\npublic_assistant={}\ntools={}",
                turn.lifecycle.status,
                turn.last_sequence,
                if assistant.is_empty() {
                    "unknown".into()
                } else {
                    assistant.join(" | ")
                },
                if tools.is_empty() {
                    "none observed".into()
                } else {
                    tools.join(", ")
                }
            ));
        }
        let full = items.join("\n---\n");
        let activity = if full.len() <= PROGRESS_ACTIVITY_MAX_BYTES {
            full
        } else {
            const OMITTED: &str = "[earlier public activity omitted]\n";
            let retained = PROGRESS_ACTIVITY_MAX_BYTES.saturating_sub(OMITTED.len());
            let mut start = full.len() - retained;
            while !full.is_char_boundary(start) {
                start += 1;
            }
            format!("{OMITTED}{}", &full[start..])
        };
        Ok((version, activity, latest_sequence))
    }

    async fn progress_summary(
        &self,
        target: &ChatAddress,
        model_slot: Option<&serde_json::Value>,
    ) -> CollaborationProgressSummary {
        let summarizer = self
            .progress_summarizer
            .read()
            .expect("collaboration progress summarizer lock poisoned")
            .clone();
        let Some(summarizer) = summarizer else {
            return CollaborationProgressSummary {
                status: CollaborationProgressStatus::Unavailable,
                activity_version: String::new(),
                summary: None,
                covered_at: chrono::Utc::now().to_rfc3339(),
                stale: false,
                error: Some("Collaboration progress summarizer is unavailable".into()),
            };
        };
        let gate = self.progress_gate(target).await;
        let _guard = gate.lock().await;
        let (version, activity, latest_sequence) =
            match self.public_activity_snapshot(target, None).await {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return CollaborationProgressSummary {
                        status: CollaborationProgressStatus::Unavailable,
                        activity_version: String::new(),
                        summary: None,
                        covered_at: chrono::Utc::now().to_rfc3339(),
                        stale: false,
                        error: Some(error),
                    }
                }
            };
        let previous = self.progress_cache.lock().await.get(target).cloned();
        if previous
            .as_ref()
            .is_some_and(|entry| entry.summary.activity_version == version)
        {
            return previous.expect("checked progress cache").summary;
        }
        let incremental_activity = if let Some(previous) = previous.as_ref() {
            match self
                .public_activity_snapshot(target, Some(previous.covered_sequence))
                .await
            {
                Ok((_, incremental, _)) if !incremental.trim().is_empty() => incremental,
                _ => activity.clone(),
            }
        } else {
            activity.clone()
        };
        let timeout = *self
            .progress_timeout
            .read()
            .expect("collaboration progress timeout lock poisoned");
        let generated = tokio::time::timeout(
            timeout,
            summarizer.summarize(
                &incremental_activity,
                previous
                    .as_ref()
                    .and_then(|entry| entry.summary.summary.as_deref()),
                model_slot,
            ),
        )
        .await;
        let summary = match generated {
            Ok(Ok(text)) => CollaborationProgressSummary {
                status: CollaborationProgressStatus::Available,
                activity_version: version,
                summary: Some(text.chars().take(512).collect()),
                covered_at: chrono::Utc::now().to_rfc3339(),
                stale: false,
                error: None,
            },
            Ok(Err(error)) => CollaborationProgressSummary {
                status: CollaborationProgressStatus::Unavailable,
                activity_version: version,
                summary: previous
                    .as_ref()
                    .and_then(|entry| entry.summary.summary.clone()),
                covered_at: previous
                    .as_ref()
                    .map(|entry| entry.summary.covered_at.clone())
                    .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
                stale: previous.is_some(),
                error: Some(error),
            },
            Err(_) => CollaborationProgressSummary {
                status: CollaborationProgressStatus::Unavailable,
                activity_version: version,
                summary: previous
                    .as_ref()
                    .and_then(|entry| entry.summary.summary.clone()),
                covered_at: previous
                    .as_ref()
                    .map(|entry| entry.summary.covered_at.clone())
                    .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
                stale: previous.is_some(),
                error: Some("Collaboration progress summary timed out".into()),
            },
        };
        if summary.status == CollaborationProgressStatus::Available {
            self.progress_cache.lock().await.insert(
                target.clone(),
                CollaborationProgressCacheEntry {
                    summary: summary.clone(),
                    covered_sequence: latest_sequence,
                },
            );
        }
        summary
    }

    async fn cached_progress_summary(&self, target: &ChatAddress) -> CollaborationProgressSummary {
        let cached = self.progress_cache.lock().await.get(target).cloned();
        if let Some(mut cached) = cached {
            cached.summary.stale = self
                .public_activity_snapshot(target, None)
                .await
                .map(|(version, _, _)| version != cached.summary.activity_version)
                .unwrap_or(true);
            return cached.summary;
        }
        CollaborationProgressSummary {
            status: CollaborationProgressStatus::Unavailable,
            activity_version: String::new(),
            summary: None,
            covered_at: chrono::Utc::now().to_rfc3339(),
            stale: false,
            error: Some(
                "No cached progress summary; read with a positive wait to generate one".into(),
            ),
        }
    }

    async fn bounded_progress_summary(
        &self,
        target: &ChatAddress,
        model_slot: Option<&serde_json::Value>,
        deadline: Instant,
    ) -> CollaborationProgressSummary {
        if Instant::now() >= deadline {
            return self.cached_progress_summary(target).await;
        }
        match tokio::time::timeout_at(deadline.into(), self.progress_summary(target, model_slot))
            .await
        {
            Ok(summary) => summary,
            Err(_) => {
                let previous = self
                    .progress_cache
                    .try_lock()
                    .ok()
                    .and_then(|cache| cache.get(target).map(|entry| entry.summary.clone()));
                CollaborationProgressSummary {
                    status: CollaborationProgressStatus::Unavailable,
                    activity_version: previous
                        .as_ref()
                        .map(|summary| summary.activity_version.clone())
                        .unwrap_or_default(),
                    summary: previous
                        .as_ref()
                        .and_then(|summary| summary.summary.clone()),
                    covered_at: previous
                        .as_ref()
                        .map(|summary| summary.covered_at.clone())
                        .unwrap_or_default(),
                    stale: previous.is_some(),
                    error: Some("Progress summary wait timed out; execution continues".into()),
                }
            }
        }
    }

    fn store(&self, address: &ChatAddress) -> WorkspaceSessionStore {
        WorkspaceSessionStore::new(ChatScopeStore::resolve(&self.root, address.clone()).root())
    }

    async fn workspace_roots(&self) -> Result<Vec<PathBuf>, String> {
        let mut roots = Vec::new();
        for parent in [self.root.join("workspaces"), self.root.join("projects")] {
            let mut entries = match tokio::fs::read_dir(&parent).await {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("read {}: {error}", parent.display())),
            };
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|error| format!("read {} entry: {error}", parent.display()))?
            {
                if entry
                    .file_type()
                    .await
                    .map(|kind| kind.is_dir())
                    .unwrap_or(false)
                {
                    roots.push(entry.path());
                }
            }
        }
        roots.sort();
        Ok(roots)
    }

    pub async fn reconcile_startup(&self) -> Result<CollaborationRecoveryReport, String> {
        let mut report = CollaborationRecoveryReport::default();
        let mut deferred_sources = HashSet::new();
        for root in self.workspace_roots().await? {
            let store = WorkspaceSessionStore::new(&root);
            for header in store
                .list_chat_headers(0, usize::MAX)
                .await
                .map_err(|error| error.to_string())?
            {
                let Some(value) = header.extra.get(COLLABORATION_META_KEY).cloned() else {
                    continue;
                };
                let descriptor: CollaborationDescriptor = serde_json::from_value(value)
                    .map_err(|error| format!("invalid collaboration descriptor: {error}"))?;
                let target = match descriptor.source.scope() {
                    ChatScope::Agent { .. } => {
                        ChatAddress::agent(&header.agent_id, header.chat_id.clone())
                    }
                    ChatScope::Project { project_id, .. } => {
                        ChatAddress::project(project_id.clone(), header.chat_id.clone())
                    }
                };
                for persisted in store
                    .list_collaboration_requests(target.chat_id().as_str())
                    .await
                    .map_err(|error| error.to_string())?
                {
                    let mut record = if persisted.outcome.is_some() {
                        persisted
                    } else {
                        let refreshed = self.refresh(&target, &persisted.request_id).await?;
                        if refreshed.outcome.is_some() {
                            report.recovered_terminal += 1;
                            refreshed
                        } else {
                            let evidence = if refreshed.turn_id.is_some() {
                                "The accepted collaboration Turn was interrupted by process restart; partial side effects may have occurred and execution was not replayed"
                            } else if refreshed.input_id.is_some() {
                                "The accepted collaboration input was interrupted before a durable Turn completed; execution was not replayed"
                            } else {
                                "The collaboration request was recorded but acceptance remained unconfirmed at process restart; execution was not replayed"
                            };
                            let mut interrupted = refreshed;
                            interrupted.outcome = Some(outcome(
                                &interrupted.request_id,
                                "interrupted",
                                evidence.to_string(),
                                true,
                            ));
                            if interrupted.delivery == CollaborationDeliveryState::Undecided {
                                interrupted.delivery = CollaborationDeliveryState::Deferred;
                            }
                            store
                                .save_collaboration_request(&interrupted)
                                .await
                                .map_err(|error| error.to_string())?;
                            let saved = store
                                .load_collaboration_request(
                                    target.chat_id().as_str(),
                                    &interrupted.request_id,
                                )
                                .await
                                .map_err(|error| error.to_string())?
                                .ok_or("reconciled collaboration request disappeared")?;
                            if saved
                                .outcome
                                .as_ref()
                                .is_some_and(|result| result.status == "interrupted")
                            {
                                report.interrupted += 1;
                            }
                            saved
                        }
                    };
                    if record.delivery == CollaborationDeliveryState::Undecided
                        && record.outcome.is_some()
                    {
                        record = store
                            .claim_collaboration_delivery(
                                target.chat_id().as_str(),
                                &record.request_id,
                                CollaborationDeliveryClaim::Deferred,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    if record.delivery == CollaborationDeliveryState::Deferred {
                        let source_exists = self
                            .store(&record.source)
                            .load_chat_header(record.source.chat_id().as_str())
                            .await
                            .map_err(|error| error.to_string())?
                            .is_some();
                        if source_exists {
                            deferred_sources.insert(record.source.clone());
                        } else {
                            match self.deliver_persisted_result(record, None).await {
                                Ok(true) => report.redelivered += 1,
                                Ok(false) => {}
                                Err(error) => {
                                    report.failed += 1;
                                    tracing::warn!(target = %target, error = %error, "failed to retain recovered collaboration result for deleted source");
                                }
                            }
                        }
                    } else {
                        match self.deliver_persisted_result(record, None).await {
                            Ok(true) => report.redelivered += 1,
                            Ok(false) => {}
                            Err(error) => {
                                report.failed += 1;
                                tracing::warn!(target = %target, error = %error, "failed to redeliver recovered collaboration result");
                            }
                        }
                    }
                }
            }
        }
        for source in deferred_sources {
            match self.resume_deferred_results(source.clone()).await {
                Ok(delivered) => report.redelivered += delivered,
                Err(error) => {
                    report.failed += 1;
                    tracing::warn!(source = %source, error = %error, "failed to redeliver recovered collaboration result batch");
                }
            }
        }
        self.in_flight.lock().await.clear();
        Ok(report)
    }

    async fn caller_configuration(
        &self,
        caller: &CollaborationCaller,
    ) -> Result<ChatHookConfiguration, String> {
        let store = self.store(&caller.address);
        let header = store
            .load_chat_header(caller.address.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Source Chat is unavailable")?;
        if header.extra.contains_key(COLLABORATION_META_KEY) {
            return Err("Collaboration Chats cannot delegate another level".into());
        }
        let current_epoch = header
            .extra
            .get("hook_configuration")
            .and_then(|configuration| configuration.get("epoch"))
            .and_then(serde_json::Value::as_str)
            .ok_or("Source configuration head is unavailable")?;
        if current_epoch != caller.configuration_epoch {
            return Err(format!(
                "Source Chat configuration changed from epoch {} to {current_epoch}; start a new Turn before creating collaboration work",
                caller.configuration_epoch
            ));
        }
        let configuration: ChatHookConfiguration = serde_json::from_value(
            store
                .load_configuration_snapshot(
                    caller.address.chat_id().as_str(),
                    &caller.configuration_epoch,
                )
                .await
                .map_err(|e| e.to_string())?
                .ok_or("Source configuration is unavailable")?,
        )
        .map_err(|e| e.to_string())?;
        configuration.validate()?;
        let placement =
            super::conversation_authority_placement::parse_conversation_authority_placement(
                &serde_json::Value::Object(header.extra),
            )
            .map_err(|e| e.to_string())?;
        let current_device_id = self.current_device_id();
        if placement.device_id != current_device_id {
            return Err("Source Chat is not authoritative on this Device".into());
        }
        self.intake.validate_collaboration_resource(
            &caller.address,
            &current_device_id,
            &caller.working_dir,
        )?;
        Ok(configuration)
    }

    pub async fn request_progress(
        &self,
        source: &ChatAddress,
    ) -> Result<serde_json::Value, String> {
        let pending = self
            .in_flight
            .lock()
            .await
            .get(source)
            .cloned()
            .unwrap_or_default();
        let mut progress = Vec::new();
        for (target, request_id) in pending {
            let record = self.refresh(&target, &request_id).await?;
            if record.source != *source || record.outcome.is_some() {
                continue;
            }
            let active = self.active_turn_control(&target).await;
            let phase = if record.turn_id.is_some() {
                "started"
            } else {
                match active.as_ref().map(|turn| turn.phase) {
                    Some(
                        super::conversation_authority::ConversationActiveTurnPhase::RunningTool,
                    ) => "waiting_for_tool",
                    Some(
                        super::conversation_authority::ConversationActiveTurnPhase::WaitingApproval,
                    ) => "waiting_for_approval",
                    Some(_) => "handing_off",
                    None => "queued_without_active_turn",
                }
            };
            progress.push(serde_json::json!({"chat": target, "request_id": request_id, "phase": phase, "active_turn": active}));
        }
        Ok(serde_json::Value::Array(progress))
    }

    async fn gate(&self, source: &ChatAddress) -> Arc<Mutex<()>> {
        self.source_gates
            .lock()
            .await
            .entry(source.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn inherited_child_epoch(
        source: &ChatAddress,
        target: &ChatAddress,
        child_epoch: &str,
        source_epoch: &str,
    ) -> String {
        let digest = Sha256::digest(format!(
            "{source}\0{target}\0{child_epoch}\0{source_epoch}\0collaboration-inheritance-v1"
        ));
        format!("collaboration-{}", hex::encode(&digest[..16]))
    }

    async fn save_descriptor(
        &self,
        target: &ChatAddress,
        descriptor: &CollaborationDescriptor,
    ) -> Result<(), String> {
        let store = self.store(target);
        let mut header = store
            .load_chat_header(target.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
            .ok_or("Target Chat not found")?;
        header.extra.insert(
            COLLABORATION_META_KEY.into(),
            serde_json::to_value(descriptor).map_err(|error| error.to_string())?,
        );
        store
            .save_chat_header(&header)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn ensure_configuration_ready(&self, target: &ChatAddress) -> Result<(), String> {
        let store = self.store(target);
        let Some(header) = store
            .load_chat_header(target.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let Some(value) = header.extra.get(COLLABORATION_META_KEY).cloned() else {
            return Ok(());
        };
        let descriptor: CollaborationDescriptor =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        if descriptor.configuration.is_ready() {
            Ok(())
        } else {
            Err(format!(
                "Collaboration Chat configuration is not ready for source epoch {}{}",
                descriptor.configuration.desired_source_epoch,
                descriptor
                    .configuration
                    .error
                    .as_deref()
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default()
            ))
        }
    }

    pub async fn propagate_source_configuration(
        &self,
        source: ChatAddress,
        source_configuration: ChatHookConfiguration,
    ) -> Result<(), String> {
        source_configuration.validate()?;
        let gate = self.gate(&source).await;
        let _guard = gate.lock().await;
        let targets = self
            .store(&source)
            .load_collaboration_targets(source.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?;
        let mut failures = Vec::new();
        for target in targets {
            let store = self.store(&target);
            let Some(header) = store
                .load_chat_header(target.chat_id().as_str())
                .await
                .map_err(|error| error.to_string())?
            else {
                continue;
            };
            let Some(value) = header.extra.get(COLLABORATION_META_KEY).cloned() else {
                continue;
            };
            let mut descriptor: CollaborationDescriptor =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            if descriptor.source != source {
                continue;
            }
            if descriptor.configuration.is_ready()
                && descriptor.configuration.applied_source_epoch == source_configuration.epoch
            {
                continue;
            }
            descriptor.configuration = CollaborationConfigurationState {
                desired_source_epoch: source_configuration.epoch.clone(),
                applied_source_epoch: descriptor.configuration.applied_source_epoch.clone(),
                status: CollaborationConfigurationStatus::Applying,
                error: None,
            };
            self.save_descriptor(&target, &descriptor).await?;

            let result = async {
                let child_value = store
                    .load_configuration_snapshot(
                        target.chat_id().as_str(),
                        header
                            .extra
                            .get("hook_configuration")
                            .and_then(|value| value.get("epoch"))
                            .and_then(serde_json::Value::as_str)
                            .ok_or("Child configuration epoch is unavailable")?,
                    )
                    .await
                    .map_err(|error| error.to_string())?
                    .ok_or("Child configuration snapshot is unavailable")?;
                let mut child = ChatHookConfiguration::from_persisted(child_value)?;
                if child
                    .extra
                    .get("collaboration_source_epoch")
                    .and_then(serde_json::Value::as_str)
                    == Some(source_configuration.epoch.as_str())
                {
                    self.hooks.adopt_chat_configuration(&target, child).await;
                    return Ok(());
                }
                let previous_epoch = child.epoch.clone();
                child.security_config = source_configuration.security_config.clone();
                child.collaboration_mode = source_configuration.collaboration_mode.clone();
                child.extra.insert(
                    "collaboration_configuration_source".into(),
                    serde_json::to_value(&source).map_err(|error| error.to_string())?,
                );
                child.extra.insert(
                    "collaboration_source_epoch".into(),
                    serde_json::json!(source_configuration.epoch),
                );
                child.epoch = Self::inherited_child_epoch(
                    &source,
                    &target,
                    &previous_epoch,
                    &source_configuration.epoch,
                );
                child.validate()?;
                store
                    .replace_chat_configuration(
                        target.chat_id().as_str(),
                        &previous_epoch,
                        serde_json::to_value(&child).map_err(|error| error.to_string())?,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                self.hooks.adopt_chat_configuration(&target, child).await;
                Ok::<(), String>(())
            }
            .await;
            match result {
                Ok(()) => {
                    descriptor.configuration =
                        CollaborationConfigurationState::ready(source_configuration.epoch.clone());
                    self.save_descriptor(&target, &descriptor).await?;
                }
                Err(error) => {
                    descriptor.configuration.status = CollaborationConfigurationStatus::Failed;
                    descriptor.configuration.error = Some(error.clone());
                    self.save_descriptor(&target, &descriptor).await?;
                    failures.push(format!("{target}: {error}"));
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Collaboration configuration propagation failed: {}",
                failures.join("; ")
            ))
        }
    }

    pub async fn create(
        &self,
        caller: CollaborationCaller,
        agent_id: String,
        message: Option<String>,
        yield_seconds: u64,
    ) -> Result<CollaborationReply, String> {
        if let Some(response) = self
            .route_collaboration_command(
                &caller.address,
                format!("collaboration-create-{}", operation_id(&caller, "create")),
                crate::services::device_command_bus::CollaborationDeviceAction::CreateAgent {
                    caller: caller.clone(),
                    agent_id: agent_id.clone(),
                    message: message.clone(),
                    yield_seconds,
                },
            )
            .await?
        {
            return match response {
                crate::services::device_command_bus::CollaborationDeviceResponse::Reply {
                    reply,
                    ..
                } => Ok(*reply),
                _ => Err("Remote collaboration returned an unexpected response".into()),
            };
        }
        self.create_with_executor(
            caller,
            rua_session::collaboration::CollaborationExecutor::Agent,
            Some(agent_id),
            message,
            yield_seconds,
        )
        .await
    }

    pub async fn create_acp(
        &self,
        caller: CollaborationCaller,
        runner: String,
        message: Option<String>,
        yield_seconds: u64,
    ) -> Result<CollaborationReply, String> {
        if runner.trim().is_empty() {
            return Err("ACP runner is required".into());
        }
        if let Some(response) = self
            .route_collaboration_command(
                &caller.address,
                format!("collaboration-create-{}", operation_id(&caller, "create")),
                crate::services::device_command_bus::CollaborationDeviceAction::CreateAcp {
                    caller: caller.clone(),
                    runner: runner.clone(),
                    message: message.clone(),
                    yield_seconds,
                },
            )
            .await?
        {
            return match response {
                crate::services::device_command_bus::CollaborationDeviceResponse::Reply {
                    reply,
                    ..
                } => Ok(*reply),
                _ => Err("Remote collaboration returned an unexpected response".into()),
            };
        }
        self.create_with_executor(
            caller,
            rua_session::collaboration::CollaborationExecutor::Acp { runner },
            None,
            message,
            yield_seconds,
        )
        .await
    }

    async fn create_with_executor(
        &self,
        caller: CollaborationCaller,
        executor: rua_session::collaboration::CollaborationExecutor,
        requested_agent_id: Option<String>,
        message: Option<String>,
        yield_seconds: u64,
    ) -> Result<CollaborationReply, String> {
        let _resource_admission = self.intake.project_work_admission(&caller.address)?;
        if matches!(
            &executor,
            rua_session::collaboration::CollaborationExecutor::Acp { .. }
        ) {
            if let Some(project_id) = caller.address.project_id() {
                self.intake.validate_acp_project_resources(project_id)?;
            }
        }
        let gate = self.gate(&caller.address).await;
        let guard = gate.lock().await;
        let source = self.caller_configuration(&caller).await?;
        let agent_id = requested_agent_id.unwrap_or_else(|| source.selected_agent_id.clone());
        let id = operation_id(&caller, "create");
        let target = match caller.address.scope() {
            ChatScope::Project { project_id, .. } => ChatAddress::project(project_id.clone(), &id),
            ChatScope::Agent { .. } => ChatAddress::agent(&agent_id, &id),
        };
        let store = self.store(&target);
        let mut created = false;
        if let Some(header) = store
            .load_chat_header(&id)
            .await
            .map_err(|e| e.to_string())?
        {
            let descriptor: CollaborationDescriptor = serde_json::from_value(
                header
                    .extra
                    .get(COLLABORATION_META_KEY)
                    .cloned()
                    .ok_or("Chat identity collision")?,
            )
            .map_err(|e| e.to_string())?;
            if descriptor.source != caller.address
                || descriptor.executor_agent_id != agent_id
                || descriptor.executor != executor
            {
                return Err("Create identity reused with a different executor".into());
            }
        } else {
            created = true;
            if message.is_some() {
                self.check_capacity(&caller.address).await?;
            }
            let agent = self
                .agents
                .get_agent(&agent_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("Target Agent not found")?;
            let assignment = super::console_chat::turn_agent_assignment_from_agent(&agent)?;
            let registry = self
                .registry
                .upgrade()
                .ok_or("Workspace registry unavailable")?;
            let runtime = registry
                .get_or_create(&agent_id)
                .await
                .map_err(|e| e.to_string())?;
            let running = agent
                .running
                .clone()
                .map(serde_json::from_value::<rua_config::AgentsRunningConfig>)
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            let configuration = self
                .hooks
                .resolve_complete_chat_configuration(
                    &target,
                    ChatConfigurationMaterialization {
                        selected_agent_id: agent_id.clone(),
                        running_config: rua_config::ChatRunningConfig::from_agent_defaults(
                            &running,
                        ),
                        security_config: source.security_config,
                        prompt_file_content: super::console_chat::prompt_file_fragment_for_entries(
                            &runtime.workspace_dir(),
                            &assignment.system_prompt_files,
                        ),
                        tool_config_snapshot: super::console_chat::snapshot_tool_configuration(
                            agent.tools.as_ref(),
                        ),
                        capability_configuration: Some(
                            runtime
                                .capability_configuration(
                                    caller
                                        .address
                                        .project_id()
                                        .map(|_| caller.working_dir.as_path()),
                                )
                                .await
                                .map_err(|e| e.to_string())?,
                        ),
                        agent_assignment: Some(assignment),
                        model: agent
                            .default_model
                            .filter(|model| !model.is_null())
                            .or(source.model),
                        thinking: None,
                        collaboration_mode: source.collaboration_mode,
                        harness_configuration: None,
                        authority_device_id: self.current_device_id(),
                    },
                )
                .await?;
            let now = chrono::Utc::now().to_rfc3339();
            let mut header = ChatSessionHeader::new(ChatMetadata {
                chat_id: id.clone(),
                agent_id: agent_id.clone(),
                title: "New Chat".to_string(),
                created_at: now.clone(),
                last_activity_at: now,
                pinned: false,
                archived: false,
                preview: None,
            });
            header.extra.insert(
                CHAT_HOOK_CONFIGURATION_META_KEY.into(),
                serde_json::to_value(&configuration).map_err(|e| e.to_string())?,
            );
            header
                .extra
                .insert("selected_agent_id".into(), serde_json::json!(agent_id));
            if let Some(project_id) = target.project_id() {
                header
                    .extra
                    .insert("project_id".into(), serde_json::json!(project_id));
                let source_scope = ChatScopeStore::resolve(&self.root, caller.address.clone());
                let source_header = WorkspaceSessionStore::new(source_scope.root())
                    .load_chat_header(caller.address.chat_id().as_str())
                    .await
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "Source Project Chat header is missing".to_string())?;
                let directory_key = super::project_chat::PROJECT_WORKING_DIRECTORY_META_KEY;
                header.extra.insert(
                    directory_key.into(),
                    source_header
                        .extra
                        .get(directory_key)
                        .cloned()
                        .unwrap_or_else(|| {
                            serde_json::json!(rua_project::INITIAL_PROJECT_DIRECTORY_ID)
                        }),
                );
            }
            if let Some(model) = configuration.model.as_ref() {
                header.extra.insert("model".into(), model.clone());
            }
            header.extra.insert(
                CHAT_AUTHORITY_PLACEMENT_META_KEY.into(),
                serde_json::to_value(ConversationAuthorityPlacement {
                    device_id: self.current_device_id(),
                    authority_epoch: ConversationAuthorityEpoch::next_for_device(
                        &self.current_device_id(),
                        None,
                    ),
                    base_manifest_hash:
                        super::conversation_manifest::initial_conversation_manifest_hash(&target)
                            .map_err(|e| e.to_string())?,
                })
                .map_err(|e| e.to_string())?,
            );
            header.extra.insert(
                COLLABORATION_META_KEY.into(),
                serde_json::to_value(CollaborationDescriptor {
                    source: caller.address.clone(),
                    executor: executor.clone(),
                    executor_agent_id: agent_id.clone(),
                    authority_device_id: self.current_device_id(),
                    working_dir: caller.working_dir.clone(),
                    creation_id: id.clone(),
                    configuration: CollaborationConfigurationState::ready(source.epoch.clone()),
                    extra: Default::default(),
                })
                .map_err(|e| e.to_string())?,
            );
            store
                .save_chat_header(&header)
                .await
                .map_err(|e| e.to_string())?;
            let scope = ChatScopeStore::resolve(&self.root, target.clone());
            rua_chat::ChatService::for_resolved_scope(scope.root(), target.clone())
                .repair_projection(&target)
                .await
                .map_err(|e| e.to_string())?;
            self.hooks
                .adopt_chat_configuration(&target, configuration)
                .await;
        }
        if let rua_session::collaboration::CollaborationExecutor::Acp { runner } = &executor {
            let scope = ChatScopeStore::resolve(&self.root, target.clone());
            if let Err(error) = self
                .intake
                .enter_collaboration_acp(
                    &target,
                    &agent_id,
                    scope.root(),
                    &caller.working_dir,
                    runner,
                )
                .await
            {
                if created {
                    let _ = store
                        .delete_chat_storage_by_chat_id(target.chat_id().as_str())
                        .await;
                }
                return Err(error);
            }
        }
        if created {
            self.store(&caller.address)
                .add_collaboration_target(caller.address.chat_id().as_str(), target.clone())
                .await
                .map_err(|error| error.to_string())?;
        }
        drop(guard);
        match message {
            Some(message) => match self
                .send(caller, target.clone(), message, yield_seconds)
                .await
            {
                Ok(reply) => Ok(reply),
                Err(error) => Ok(CollaborationReply {
                    chat: target,
                    request_id: None,
                    state: "submission_failed".into(),
                    result_ref: None,
                    input_id: None,
                    turn_id: None,
                    active_turn: None,
                    outcome: None,
                    progress: None,
                    error: Some(error),
                }),
            },
            None => Ok(CollaborationReply {
                result_ref: None,
                active_turn: self.active_turn_control(&target).await,
                chat: target,
                request_id: None,
                state: "created".into(),
                input_id: None,
                turn_id: None,
                outcome: None,
                progress: None,
                error: None,
            }),
        }
    }

    async fn check_capacity(&self, source: &ChatAddress) -> Result<(), String> {
        let pending = self
            .in_flight
            .lock()
            .await
            .get(source)
            .cloned()
            .unwrap_or_default();
        let mut active = Vec::new();
        for (chat, id) in pending {
            if self.refresh(&chat, &id).await?.outcome.is_none() {
                active.push((chat, id));
            }
        }
        let full = active.len() >= self.max_in_flight;
        self.in_flight.lock().await.insert(source.clone(), active);
        if full {
            Err("Collaboration in-flight limit reached; wait for existing requests".into())
        } else {
            Ok(())
        }
    }

    pub async fn send(
        &self,
        caller: CollaborationCaller,
        target: ChatAddress,
        message: String,
        yield_seconds: u64,
    ) -> Result<CollaborationReply, String> {
        if message.trim().is_empty() {
            return Err("Collaboration message must not be empty".into());
        }
        if let Some(response) = self
            .route_collaboration_command(
                &target,
                format!(
                    "collaboration-send-{}",
                    operation_id(&caller, &target.to_string())
                ),
                crate::services::device_command_bus::CollaborationDeviceAction::Send {
                    caller: caller.clone(),
                    target: target.clone(),
                    message: message.clone(),
                    yield_seconds,
                },
            )
            .await?
        {
            return match response {
                crate::services::device_command_bus::CollaborationDeviceResponse::Reply {
                    reply,
                    ..
                } => Ok(*reply),
                _ => Err("Remote collaboration returned an unexpected response".into()),
            };
        }
        let gate = self.gate(&caller.address).await;
        let guard = gate.lock().await;
        self.caller_configuration(&caller).await?;
        let store = self.store(&target);
        let header = store
            .load_chat_header(target.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Target Chat not found")?;
        let descriptor: CollaborationDescriptor = serde_json::from_value(
            header
                .extra
                .get(COLLABORATION_META_KEY)
                .cloned()
                .ok_or("Target is not a collaboration Chat")?,
        )
        .map_err(|e| e.to_string())?;
        if descriptor.source != caller.address {
            return Err("Target collaboration belongs to another source Chat".into());
        }
        self.ensure_configuration_ready(&target).await?;
        if descriptor.authority_device_id != self.current_device_id() {
            return Err("Target collaboration requires another Authority Device".into());
        }
        let placement =
            super::conversation_authority_placement::parse_conversation_authority_placement(
                &serde_json::Value::Object(header.extra),
            )
            .map_err(|e| e.to_string())?;
        let id = operation_id(&caller, &target.to_string());
        let digest = format!("{:x}", Sha256::digest(message.as_bytes()));
        if let Some(existing) = store
            .load_collaboration_request(target.chat_id().as_str(), &id)
            .await
            .map_err(|e| e.to_string())?
        {
            if existing.input_digest != digest {
                return Err("Request identity reused with different input".into());
            }
            if existing.input_id.is_none() && existing.outcome.is_none() {
                return Err(
                    "Collaboration acceptance is unconfirmed; the request was not resubmitted"
                        .into(),
                );
            }
        } else {
            self.check_capacity(&caller.address).await?;
            let executor = self
                .agents
                .get_agent(&descriptor.executor_agent_id)
                .await
                .map_err(|error| error.to_string())?
                .ok_or("Collaboration executor Agent is unavailable")?;
            if !executor.enabled {
                return Err("Collaboration executor Agent is disabled".into());
            }
            let mut record = CollaborationRequest {
                request_id: id.clone(),
                source: caller.address.clone(),
                target: target.clone(),
                origin_turn_id: caller.turn_id,
                origin_tool_call_id: caller.tool_call_id,
                input_digest: digest,
                created_at: chrono::Utc::now().to_rfc3339(),
                input_id: None,
                turn_id: None,
                outcome: None,
                delivery: Default::default(),
                extra: Default::default(),
            };
            store
                .save_collaboration_request(&record)
                .await
                .map_err(|e| e.to_string())?;
            let scope = ChatScopeStore::resolve(&self.root, target.clone());
            let persist_store = self.store(&target);
            let persist_scope_root = scope.root().to_path_buf();
            let mut persisted_record = record.clone();
            let pending = ConversationInputPayload {
                message: message.clone(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::from_iter([(
                    SUBMISSION_ID_KEY.into(),
                    serde_json::json!(id),
                )]),
                attachments: Vec::new(),
                enqueued_at: Instant::now(),
            };
            let persist_placement = placement.clone();
            let accepted = self.intake.accept_collaboration_input(ConversationExecutionContext {
                address: target.clone(), execution_agent_id: descriptor.executor_agent_id,
                scope_root: scope.root().to_path_buf(), working_dir: descriptor.working_dir,
            }, placement, pending, Box::new(move |pending, accepted| Box::pin(async move {
                persisted_record.input_id = Some(accepted.input_id.clone());
                if matches!(accepted.mode, super::conversation_authority::ConversationInputAcceptanceMode::Started) {
                    persisted_record.turn_id = Some(accepted.turn_id.clone());
                    let input = super::conversation_input_intake::user_input_message_from_payload(&pending)?;
                    let configuration: ChatHookConfiguration = serde_json::from_value(pending.extra.get(CHAT_HOOK_CONFIGURATION_META_KEY).cloned().ok_or("Missing accepted configuration")?).map_err(|e| e.to_string())?;
                    let assignment = serde_json::to_value(&configuration.agent_assignment).map_err(|e| e.to_string())?;
                    let (model, thinking) = rua_chat::capture_turn_model_and_thinking(configuration.model.as_ref(), configuration.thinking.as_ref());
                    let (turn, _) = super::console_chat::build_submitted_user_turn(&configuration.selected_agent_id, &input, &persist_placement.device_id, super::console_chat::SubmittedUserTurnMetadata {
                        turn_id: Some(&accepted.turn_id), input_id: Some(&accepted.input_id), acceptance_mode: Some("started"),
                        submission_id: Some(&persisted_record.request_id), device_delivery: None, acp_origin_runner: None,
                        memory_signal: None, turn_agent_assignment: Some(&assignment), turn_model: model.as_ref(), turn_thinking: thinking.as_ref(),
                        collaboration_mode: serde_json::from_value(configuration.collaboration_mode).map_err(|e| e.to_string())?,
                        authority_device_id: &persist_placement.device_id, authority_epoch: persist_placement.authority_epoch,
                    }).map_err(|e| e.to_string())?.ok_or("Empty collaboration input")?;
                    rua_chat::ChatService::for_resolved_scope(
                        persist_scope_root, persisted_record.target.clone(),
                    ).replace_placeholder_title(
                        &persisted_record.target, super::console_chat::truncate_chat_name(&pending.message),
                    ).await.map_err(|e| e.to_string())?;
                    persist_store.append_conversation_turn_for_address(&persisted_record.target, turn).await.map_err(|e| e.to_string())?;
                }
                persist_store.save_collaboration_request(&persisted_record).await.map_err(|e| e.to_string())
            }))).await;
            match accepted {
                Ok(ConversationInputDeliveryOutcome::Delivered {
                    accepted: true,
                    input_id,
                    ..
                }) => {
                    record = store
                        .load_collaboration_request(target.chat_id().as_str(), &id)
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or("Accepted request record disappeared")?;
                    record.input_id = input_id;
                    store
                        .save_collaboration_request(&record)
                        .await
                        .map_err(|e| e.to_string())?;
                    self.in_flight
                        .lock()
                        .await
                        .entry(caller.address)
                        .or_default()
                        .push((target.clone(), id.clone()));
                }
                Ok(ConversationInputDeliveryOutcome::Delivered {
                    accepted: false,
                    detail,
                    ..
                }) => {
                    return Err(
                        detail.unwrap_or_else(|| "Collaboration acceptance unconfirmed".into())
                    );
                }
                Ok(ConversationInputDeliveryOutcome::Rejected { detail, .. }) | Err(detail) => {
                    record.outcome = Some(outcome(&id, "rejected", detail, false));
                    store
                        .save_collaboration_request(&record)
                        .await
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        drop(guard);
        let deadline = Instant::now() + Duration::from_secs(yield_seconds.min(30));
        loop {
            let refreshed = self.refresh(&target, &id).await?;
            let claim = if refreshed.outcome.is_some() {
                Some(CollaborationDeliveryClaim::Direct)
            } else if Instant::now() >= deadline {
                Some(CollaborationDeliveryClaim::Deferred)
            } else {
                None
            };
            if let Some(claim) = claim {
                let record = store
                    .claim_collaboration_delivery(target.chat_id().as_str(), &id, claim)
                    .await
                    .map_err(|error| error.to_string())?;
                let outcome = if record.delivery == CollaborationDeliveryState::Direct {
                    record.outcome
                } else {
                    None
                };
                return Ok(CollaborationReply {
                    result_ref: None,
                    active_turn: self.active_turn_control(&target).await,
                    chat: target,
                    request_id: Some(id),
                    state: outcome
                        .as_ref()
                        .map(|r| r.status.clone())
                        .unwrap_or_else(|| {
                            if record.turn_id.is_some() {
                                "running"
                            } else {
                                "queued"
                            }
                            .into()
                        }),
                    input_id: record.input_id,
                    turn_id: record.turn_id,
                    outcome,
                    progress: None,
                    error: None,
                });
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn read(
        &self,
        caller: CollaborationCaller,
        target: ChatAddress,
        yield_seconds: u64,
    ) -> Result<CollaborationReply, String> {
        // Zero yield disables waiting for progress, not the IO required to read state.
        let io_budget = Duration::from_secs(if yield_seconds == 0 {
            5
        } else {
            yield_seconds.min(30)
        });
        let deadline = Instant::now() + io_budget;
        tokio::time::timeout_at(deadline.into(), self.read_with_deadline(caller, target, yield_seconds, deadline))
            .await
            .map_err(|_| "Collaboration read deadline exceeded; state is unconfirmed, execution was not cancelled. Retry reading the same Chat.".to_string())?
    }

    async fn read_with_deadline(
        &self,
        caller: CollaborationCaller,
        target: ChatAddress,
        yield_seconds: u64,
        deadline: Instant,
    ) -> Result<CollaborationReply, String> {
        if let Some(response) = self
            .route_collaboration_command(
                &target,
                format!(
                    "collaboration-read-{}",
                    operation_id(&caller, &format!("read:{target}"))
                ),
                crate::services::device_command_bus::CollaborationDeviceAction::Read {
                    caller: caller.clone(),
                    target: target.clone(),
                    yield_seconds,
                    result: None,
                },
            )
            .await?
        {
            return match response {
                crate::services::device_command_bus::CollaborationDeviceResponse::Reply {
                    reply,
                    ..
                } => Ok(*reply),
                _ => Err("Remote collaboration returned an unexpected response".into()),
            };
        }
        self.caller_configuration(&caller).await?;
        let store = self.store(&target);
        let header = store
            .load_chat_header(target.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
            .ok_or("Target Chat not found")?;
        let descriptor: CollaborationDescriptor = serde_json::from_value(
            header
                .extra
                .get(COLLABORATION_META_KEY)
                .cloned()
                .ok_or("Target is not a collaboration Chat")?,
        )
        .map_err(|error| error.to_string())?;
        if descriptor.source != caller.address {
            return Err("Target collaboration belongs to another source Chat".into());
        }
        if descriptor.authority_device_id != self.current_device_id() {
            return Err("Target collaboration requires another Authority Device".into());
        }
        let Some(mut record) = store
            .list_collaboration_requests(target.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
            .pop()
        else {
            return Ok(CollaborationReply {
                result_ref: None,
                active_turn: self.active_turn_control(&target).await,
                chat: target,
                request_id: None,
                state: "created".into(),
                input_id: None,
                turn_id: None,
                outcome: None,
                progress: None,
                error: None,
            });
        };
        let read_id = operation_id(&caller, &format!("read:{}:{}", target, record.request_id));
        let summary_deadline = if yield_seconds == 0 {
            Instant::now()
        } else {
            deadline
                .checked_sub(Duration::from_millis(50))
                .unwrap_or(deadline)
        };
        let mut observed_progress = None;
        loop {
            record = self.refresh(&target, &record.request_id).await?;
            if record.outcome.is_some() {
                if record.delivery == CollaborationDeliveryState::Deferred {
                    record = store
                        .claim_deferred_collaboration_delivery(
                            target.chat_id().as_str(),
                            &record.request_id,
                            CollaborationDeferredClaim::Read {
                                read_id: read_id.clone(),
                            },
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                }
                let outcome = matches!(
                    &record.delivery,
                    CollaborationDeliveryState::Read { read_id: claimed }
                        if claimed == &read_id
                )
                .then(|| record.outcome.clone())
                .flatten();
                return Ok(CollaborationReply {
                    result_ref: if outcome.is_none() {
                        record.outcome.as_ref().map(|result| {
                            rua_session::collaboration::CollaborationResultCursor {
                                request_id: record.request_id.clone(),
                                result_id: result.result_id.clone(),
                                offset: 0,
                                extra: Default::default(),
                            }
                        })
                    } else {
                        None
                    },
                    active_turn: self.active_turn_control(&target).await,
                    chat: target,
                    request_id: Some(record.request_id),
                    state: record
                        .outcome
                        .as_ref()
                        .map(|result| result.status.clone())
                        .unwrap_or_else(|| "running".into()),
                    input_id: record.input_id,
                    turn_id: record.turn_id,
                    outcome,
                    progress: None,
                    error: match &record.delivery {
                        CollaborationDeliveryState::DeliveryFailed { error, .. } => {
                            Some(error.clone())
                        }
                        _ => None,
                    },
                });
            }
            if let Some(progress) = observed_progress.take() {
                return Ok(CollaborationReply {
                    result_ref: None,
                    active_turn: self.active_turn_control(&target).await,
                    chat: target,
                    request_id: Some(record.request_id),
                    state: if record.turn_id.is_some() {
                        "running"
                    } else {
                        "queued"
                    }
                    .into(),
                    input_id: record.input_id,
                    turn_id: record.turn_id,
                    outcome: None,
                    progress: Some(progress),
                    error: None,
                });
            }
            let model_slot = header
                .extra
                .get(CHAT_HOOK_CONFIGURATION_META_KEY)
                .and_then(|configuration| configuration.get("model"));
            observed_progress = Some(
                self.bounded_progress_summary(&target, model_slot, summary_deadline)
                    .await,
            );
        }
    }

    pub async fn read_result(
        &self,
        caller: CollaborationCaller,
        target: ChatAddress,
        cursor: rua_session::collaboration::CollaborationResultCursor,
    ) -> Result<CollaborationReply, String> {
        tokio::time::timeout(Duration::from_secs(5), self.read_result_inner(caller, target, cursor))
            .await
            .map_err(|_| "Collaboration result read deadline exceeded; retry the same result cursor. Execution and delivery were not changed.".to_string())?
    }

    async fn read_result_inner(
        &self,
        caller: CollaborationCaller,
        target: ChatAddress,
        cursor: rua_session::collaboration::CollaborationResultCursor,
    ) -> Result<CollaborationReply, String> {
        if let Some(response) = self
            .route_collaboration_command(
                &target,
                format!(
                    "collaboration-result-read-{}",
                    operation_id(
                        &caller,
                        &format!(
                            "{}:{}:{}",
                            cursor.request_id, cursor.result_id, cursor.offset
                        )
                    )
                ),
                crate::services::device_command_bus::CollaborationDeviceAction::Read {
                    caller: caller.clone(),
                    target: target.clone(),
                    yield_seconds: 0,
                    result: Some(cursor.clone()),
                },
            )
            .await?
        {
            return match response {
                crate::services::device_command_bus::CollaborationDeviceResponse::Reply {
                    reply,
                    ..
                } => Ok(*reply),
                _ => Err("Remote collaboration returned an unexpected response".into()),
            };
        }
        self.caller_configuration(&caller).await?;
        let store = self.store(&target);
        let header = store
            .load_chat_header(target.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Target Chat not found")?;
        let descriptor: CollaborationDescriptor = serde_json::from_value(
            header
                .extra
                .get(COLLABORATION_META_KEY)
                .cloned()
                .ok_or("Target is not a collaboration Chat")?,
        )
        .map_err(|e| e.to_string())?;
        if descriptor.source != caller.address {
            return Err("Target collaboration belongs to another source Chat".into());
        }
        if descriptor.authority_device_id != self.current_device_id() {
            return Err("Target collaboration requires another Authority Device".into());
        }
        let record = store
            .load_collaboration_request(target.chat_id().as_str(), &cursor.request_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Collaboration request not found")?;
        if record.source != caller.address || record.target != target {
            return Err("Result does not belong to this collaboration".into());
        }
        let outcome = record
            .outcome
            .ok_or("Collaboration result is not available")?;
        if outcome.result_id != cursor.result_id {
            return Err("Result cursor does not match the requested result".into());
        }
        rua_session::collaboration::collaboration_result_page(
            &cursor.request_id,
            &cursor.result_id,
            &outcome.status,
            &outcome.output,
            outcome.partial,
            cursor.offset,
        )?;
        Ok(CollaborationReply {
            chat: target,
            request_id: Some(record.request_id),
            result_ref: None,
            state: outcome.status.clone(),
            input_id: None,
            turn_id: None,
            active_turn: None,
            outcome: Some(outcome),
            progress: None,
            error: None,
        })
    }

    async fn active_turn_control(&self, target: &ChatAddress) -> Option<CollaborationTurnControl> {
        let active = self.intake.collaboration_active_turn(target).await?;
        Some(CollaborationTurnControl {
            turn_id: active.turn_id,
            authority_epoch: active.authority_epoch,
            phase: active.phase,
        })
    }

    pub async fn stop_turn(
        &self,
        caller: CollaborationCaller,
        target: ChatAddress,
        turn_id: String,
        authority_epoch: ConversationAuthorityEpoch,
    ) -> Result<CollaborationStopReply, String> {
        if let Some(response) = self
            .route_collaboration_command(
                &target,
                format!(
                    "collaboration-stop-{}",
                    operation_id(&caller, &format!("stop:{target}:{turn_id}"))
                ),
                crate::services::device_command_bus::CollaborationDeviceAction::StopTurn {
                    caller: caller.clone(),
                    target: target.clone(),
                    turn_id: turn_id.clone(),
                    turn_authority_epoch: authority_epoch,
                },
            )
            .await?
        {
            return match response {
                crate::services::device_command_bus::CollaborationDeviceResponse::Stopped {
                    reply,
                    ..
                } => Ok(reply),
                _ => Err("Remote collaboration returned an unexpected response".into()),
            };
        }
        self.caller_configuration(&caller).await?;
        let store = self.store(&target);
        let header = store
            .load_chat_header(target.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
            .ok_or("Target Chat not found")?;
        let descriptor: CollaborationDescriptor = serde_json::from_value(
            header
                .extra
                .get(COLLABORATION_META_KEY)
                .cloned()
                .ok_or("Target is not a collaboration Chat")?,
        )
        .map_err(|error| error.to_string())?;
        if descriptor.source != caller.address {
            return Err("Target collaboration belongs to another source Chat".into());
        }
        if descriptor.authority_device_id != self.current_device_id() {
            return Err("Target collaboration requires another Authority Device".into());
        }
        let gate = self.gate(&caller.address).await;
        let _guard = gate.lock().await;
        let cancelled_requests = self
            .intake
            .stop_collaboration_turn(&target, &turn_id, authority_epoch)
            .await?;
        let stopped = cancelled_requests.is_some();
        for request_id in cancelled_requests.into_iter().flatten() {
            let Some(mut record) = store
                .load_collaboration_request(target.chat_id().as_str(), &request_id)
                .await
                .map_err(|error| error.to_string())?
            else {
                continue;
            };
            if record.outcome.is_some() || record.turn_id.is_some() {
                continue;
            }
            record.outcome = Some(outcome(
                &request_id,
                "cancelled",
                "Request cancelled before execution by collaboration Stop".into(),
                false,
            ));
            store
                .save_collaboration_request(&record)
                .await
                .map_err(|error| error.to_string())?;
            if let Some(requests) = self.in_flight.lock().await.get_mut(&record.source) {
                requests.retain(|(chat, id)| chat != &target || id != &request_id);
            }
            if matches!(record.delivery, CollaborationDeliveryState::Deferred) {
                self.deliver_persisted_result(record, None).await?;
            }
        }
        Ok(CollaborationStopReply {
            status: if stopped {
                CollaborationStopStatus::StopRequested
            } else if self.active_turn_control(&target).await.is_some() {
                CollaborationStopStatus::TargetChanged
            } else {
                CollaborationStopStatus::NoActiveTurn
            },
            chat: target,
            turn_id,
            stopped,
        })
    }

    async fn refresh(
        &self,
        target: &ChatAddress,
        id: &str,
    ) -> Result<CollaborationRequest, String> {
        let store = self.store(target);
        let mut record = store
            .load_collaboration_request(target.chat_id().as_str(), id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Collaboration request not found")?;
        if record.outcome.is_some() {
            return Ok(record);
        }
        for turn in store
            .read_turn_range(target.chat_id().as_str(), 1, usize::MAX)
            .await
            .map_err(|e| e.to_string())?
        {
            if let Some(turn_id) = record.turn_id.as_ref() {
                if &turn.turn_id != turn_id {
                    continue;
                }
            }
            let messages = store
                .read_turn_messages(target.chat_id().as_str(), &turn.turn_id)
                .await
                .map_err(|e| e.to_string())?;
            if record.turn_id.is_none()
                && !messages.iter().any(|message| {
                    message
                        .metadata
                        .get(SUBMISSION_ID_KEY)
                        .and_then(serde_json::Value::as_str)
                        == Some(id)
                })
            {
                continue;
            }
            record.turn_id = Some(turn.turn_id.clone());
            if !matches!(turn.lifecycle.status.as_str(), "in_progress" | "running") {
                let text = messages
                    .iter()
                    .rev()
                    .find(|message| message.role == "assistant" && message.tool.is_none())
                    .map(|message| {
                        message
                            .content
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                record.outcome = Some(outcome(
                    id,
                    &turn.lifecycle.status,
                    text,
                    turn.lifecycle.status != "completed",
                ));
            }
            store
                .save_collaboration_request(&record)
                .await
                .map_err(|e| e.to_string())?;
            break;
        }
        Ok(record)
    }

    pub async fn settle_terminal_result(
        &self,
        fact: &super::conversation_authority::ConversationTurnTerminalFact,
    ) -> Result<Option<String>, String> {
        let store = self.store(&fact.address);
        let messages = store
            .read_turn_messages(fact.address.chat_id().as_str(), &fact.turn_id)
            .await
            .map_err(|error| error.to_string())?;
        let Some(request_id) = messages.iter().find_map(|message| {
            message
                .metadata
                .get(SUBMISSION_ID_KEY)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        let Some(mut record) = store
            .load_collaboration_request(fact.address.chat_id().as_str(), &request_id)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        if record.target != fact.address
            || record
                .turn_id
                .as_deref()
                .is_some_and(|turn_id| turn_id != fact.turn_id)
        {
            return Ok(None);
        }
        let output = messages
            .iter()
            .rev()
            .find(|message| message.role == "assistant" && message.tool.is_none())
            .map(|message| {
                message
                    .content
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        record.turn_id = Some(fact.turn_id.clone());
        record.outcome = Some(outcome(
            &request_id,
            &fact.status,
            output,
            fact.status != "completed",
        ));
        store
            .save_collaboration_request(&record)
            .await
            .map_err(|error| error.to_string())?;
        self.intake.publish_background_work(&record.source).await;
        Ok(Some(request_id))
    }

    pub async fn deliver_terminal_result(
        &self,
        fact: &super::conversation_authority::ConversationTurnTerminalFact,
    ) -> Result<(), String> {
        let Some(request_id) = self.settle_terminal_result(fact).await? else {
            return Ok(());
        };
        self.deliver_settled_terminal_result(fact, request_id).await
    }

    pub(crate) async fn deliver_settled_terminal_result(
        &self,
        fact: &super::conversation_authority::ConversationTurnTerminalFact,
        request_id: String,
    ) -> Result<(), String> {
        let store = self.store(&fact.address);
        let mut record = store
            .load_collaboration_request(fact.address.chat_id().as_str(), &request_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("Settled collaboration request disappeared during result delivery")?;
        let deadline = Instant::now() + Duration::from_secs(31);
        while record.delivery == CollaborationDeliveryState::Undecided && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
            record = store
                .load_collaboration_request(fact.address.chat_id().as_str(), &request_id)
                .await
                .map_err(|error| error.to_string())?
                .ok_or("Collaboration request disappeared during result delivery")?;
        }
        match record.delivery {
            CollaborationDeliveryState::Direct
            | CollaborationDeliveryState::Automatic { .. }
            | CollaborationDeliveryState::Read { .. }
            | CollaborationDeliveryState::DeliveryFailed { .. }
            | CollaborationDeliveryState::Delivered { .. } => return Ok(()),
            CollaborationDeliveryState::Undecided => {
                return Err("Collaboration result delivery was not decided before timeout".into())
            }
            CollaborationDeliveryState::Deferred => {}
        }
        self.deliver_persisted_result(record, fact.completed_at.clone())
            .await?;
        Ok(())
    }

    async fn deliver_persisted_result(
        &self,
        mut record: CollaborationRequest,
        created_at: Option<String>,
    ) -> Result<bool, String> {
        let result = record
            .outcome
            .clone()
            .ok_or("Terminal collaboration request is missing its outcome")?;
        let source_store = self.store(&record.source);
        if source_store
            .load_collaboration_wake_state(record.source.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
            .suppressed
        {
            return Ok(false);
        }
        let store = self.store(&record.target);
        let observation_id = match &record.delivery {
            CollaborationDeliveryState::Deferred => result.result_id.clone(),
            CollaborationDeliveryState::Automatic { observation_id }
            | CollaborationDeliveryState::DeliveryFailed { observation_id, .. } => {
                observation_id.clone()
            }
            CollaborationDeliveryState::Undecided => {
                return Err("Collaboration result delivery is still undecided".into())
            }
            CollaborationDeliveryState::Direct
            | CollaborationDeliveryState::Read { .. }
            | CollaborationDeliveryState::Delivered { .. } => return Ok(false),
        };
        record = match record.delivery {
            CollaborationDeliveryState::Deferred => store
                .claim_deferred_collaboration_delivery(
                    record.target.chat_id().as_str(),
                    &record.request_id,
                    CollaborationDeferredClaim::Automatic {
                        observation_id: observation_id.clone(),
                    },
                )
                .await
                .map_err(|error| error.to_string())?,
            CollaborationDeliveryState::DeliveryFailed { .. } => store
                .retry_failed_collaboration_delivery(
                    record.target.chat_id().as_str(),
                    &record.request_id,
                )
                .await
                .map_err(|error| error.to_string())?,
            _ => record,
        };
        if !matches!(
            record.delivery,
            CollaborationDeliveryState::Automatic {
                observation_id: ref claimed
            }
                if claimed == &observation_id
        ) {
            return Ok(false);
        }
        if let Err(error) = self
            .intake
            .deliver_background_result(
                record.source.clone(),
                rua_session::RuntimeNote {
                    runtime_note_id: observation_id.clone(),
                    created_at: created_at.unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
                    payload: rua_session::RuntimeNotePayload::CollaborationResult(
                        rua_session::CollaborationResultRuntimeNote {
                            request_id: record.request_id.clone(),
                            result_id: result.result_id,
                            target: record.target.clone(),
                            status: result.status,
                            output: result.output,
                            partial: result.partial,
                            results: Vec::new(),
                        },
                    ),
                },
            )
            .await
        {
            store
                .mark_collaboration_delivery_failed(
                    record.target.chat_id().as_str(),
                    &record.request_id,
                    observation_id,
                    error.clone(),
                )
                .await
                .map_err(|persist_error| persist_error.to_string())?;
            return Err(error);
        }
        store
            .mark_collaboration_delivered(
                record.target.chat_id().as_str(),
                &record.request_id,
                observation_id,
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        self.intake.publish_background_work(&record.source).await;
        Ok(true)
    }

    pub async fn resume_deferred_results(&self, source: ChatAddress) -> Result<usize, String> {
        let source_store = self.store(&source);
        if source_store
            .load_collaboration_wake_state(source.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?
            .suppressed
        {
            return Ok(0);
        }
        let targets = source_store
            .load_collaboration_targets(source.chat_id().as_str())
            .await
            .map_err(|error| error.to_string())?;
        let observation_id = format!("collaboration-resume-{}", uuid::Uuid::new_v4());
        let mut claimed = Vec::new();
        for target in targets {
            let store = self.store(&target);
            for record in store
                .list_collaboration_requests(target.chat_id().as_str())
                .await
                .map_err(|error| error.to_string())?
            {
                if record.source != source
                    || record.outcome.is_none()
                    || record.delivery != CollaborationDeliveryState::Deferred
                {
                    continue;
                }
                let claimed_record = store
                    .claim_deferred_collaboration_delivery(
                        target.chat_id().as_str(),
                        &record.request_id,
                        CollaborationDeferredClaim::Automatic {
                            observation_id: observation_id.clone(),
                        },
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                if matches!(
                    claimed_record.delivery,
                    CollaborationDeliveryState::Automatic {
                        observation_id: ref claimed_id
                    } if claimed_id == &observation_id
                ) {
                    claimed.push(claimed_record);
                }
            }
        }
        if claimed.is_empty() {
            return Ok(0);
        }
        let delivered_count = claimed.len();

        let mut results = Vec::with_capacity(claimed.len());
        let mut result_ids = Vec::with_capacity(claimed.len());
        for record in &claimed {
            let result = record.outcome.as_ref().expect("claimed terminal result");
            result_ids.push(result.result_id.clone());
            results.push(rua_session::runtime_note::CollaborationResultReference {
                chat: record.target.clone(),
                result: rua_session::collaboration::CollaborationResultCursor {
                    request_id: record.request_id.clone(),
                    result_id: result.result_id.clone(),
                    offset: 0,
                    extra: Default::default(),
                },
            });
        }
        let partial = claimed.iter().any(|record| {
            record
                .outcome
                .as_ref()
                .is_some_and(|result| result.partial || result.status != "completed")
        });
        let note = rua_session::RuntimeNote {
            runtime_note_id: observation_id.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            payload: rua_session::RuntimeNotePayload::CollaborationResult(
                rua_session::CollaborationResultRuntimeNote {
                    request_id: format!("batch:{}", claimed.len()),
                    result_id: result_ids.join(","),
                    target: source.clone(),
                    status: if partial { "partial" } else { "completed" }.into(),
                    output: "Deferred results are ready. Read each reference with chat_read."
                        .into(),
                    partial,
                    results,
                },
            ),
        };
        if let Err(error) = self.intake.deliver_background_result(source, note).await {
            for record in claimed {
                self.store(&record.target)
                    .mark_collaboration_delivery_failed(
                        record.target.chat_id().as_str(),
                        &record.request_id,
                        observation_id.clone(),
                        error.clone(),
                    )
                    .await
                    .map_err(|persist_error| persist_error.to_string())?;
            }
            return Err(error);
        }
        for record in claimed {
            self.store(&record.target)
                .mark_collaboration_delivered(
                    record.target.chat_id().as_str(),
                    &record.request_id,
                    observation_id.clone(),
                    None,
                )
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(delivered_count)
    }
}

fn outcome(id: &str, status: &str, output: String, partial: bool) -> CollaborationOutcome {
    CollaborationOutcome {
        result_id: format!("collaboration-result-{id}"),
        status: status.into(),
        output,
        partial,
        extra: Default::default(),
    }
}

fn operation_id(caller: &CollaborationCaller, operation: &str) -> String {
    let bytes = Sha256::digest(format!(
        "{}\0{}\0{}\0{operation}",
        caller.address, caller.turn_id, caller.tool_call_id
    ));
    let mut id = [0u8; 16];
    id.copy_from_slice(&bytes[..16]);
    uuid::Uuid::from_bytes(id).to_string()
}
