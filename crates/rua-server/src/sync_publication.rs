use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use rua_core::ChatAddress;
use rua_sync::event::SyncMeta;
use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use rua_sync::opaque_replica_log::SyncChange;
use tracing::warn;

use crate::state::AppState;
use crate::sync_network::{BoundSyncNetwork, BoundSyncNetworkError};

#[derive(Debug, Clone)]
pub enum SyncPublicationFact {
    AgentPromptFilesCommitted {
        agent_id: String,
        changes: Vec<rua_prompts::agent_files::AgentPromptFileChange>,
    },
    ProviderConfigCommitted {
        provider_id: String,
        config: serde_json::Value,
    },
    ProviderConfigDeleted {
        provider_id: String,
    },
    ChatIndexUpserted {
        address: ChatAddress,
        chat: serde_json::Value,
    },
    ChatIndexDeleted {
        address: ChatAddress,
    },
    ChatDeleted {
        scope_root: PathBuf,
        address: ChatAddress,
    },
    MediaDeleted {
        owner: rua_sync::media_adapter::MediaOwner,
        media_id: String,
    },
    PromptLibraryFilesCommitted {
        changes: Vec<rua_prompts::file_store::PromptFileChange>,
    },
    AgentTemplateCommitted {
        template_id: String,
        template: serde_json::Value,
    },
    AgentTemplateDeleted {
        template_id: String,
    },
    SubagentTemplatesCommitted {
        templates: serde_json::Value,
    },
    RootSettingsCommitted {
        settings: serde_json::Value,
    },
    RootAgentRegistryCommitted {
        agents: serde_json::Value,
    },
    RootUserTimezoneCommitted {
        timezone: serde_json::Value,
    },
    RootProcessSecurityCommitted {
        security: serde_json::Value,
    },
    AgentConfigCommitted {
        agent_id: String,
        config: serde_json::Value,
    },
    AgentConfigDeleted {
        agent_id: String,
    },
    AgentSecurityCommitted {
        agent_id: String,
        config: serde_json::Value,
    },
    AgentSecurityDeleted {
        agent_id: String,
    },
    SharedMcpCatalogCommitted {
        catalog: serde_json::Value,
    },
    SharedMcpClientDeleted {
        client_key: String,
    },
    StructuredMemoryRecordCommitted {
        record: Box<rua_memory::MemoryRecord>,
    },
    StructuredMemoryVisibilityCommitted {
        visibility: Box<rua_memory::MemoryVisibility>,
    },
    StructuredMemoryRecordDeleted {
        agent_id: String,
        memory_id: rua_memory::MemoryId,
    },
    SkillPoolCommitted {
        skill_name: String,
    },
    SkillPoolDeleted {
        skill_name: String,
    },
    WorkspaceSkillCommitted {
        agent_id: String,
        skill_name: String,
    },
    WorkspaceSkillDeleted {
        agent_id: String,
        skill_name: String,
    },
    DeviceListEntrySnapshot {
        entry: rua_sync::device_list::DeviceEntry,
    },
    DeviceListDeviceRemoved {
        device_id_hex: String,
    },
    DeviceListDeviceRenamed {
        device_id_hex: String,
        friendly_name: String,
    },
    ProjectSidebarRecordsCommitted {
        records: Vec<rua_project::ProjectSidebarRecord>,
    },
    ProjectDescriptorCommitted {
        descriptor: rua_project::ProjectDescriptor,
    },
    ProjectPermanentlyDeleted {
        project_id: rua_core::ProjectId,
        scope_root: PathBuf,
        chat_addresses: Vec<ChatAddress>,
    },
    StableConversationState {
        scope_root: PathBuf,
        address: ChatAddress,
    },
    ConversationStateDeleted {
        scope_root: PathBuf,
        address: ChatAddress,
    },
}

impl SyncPublicationFact {
    fn is_device_list_fact(&self) -> bool {
        matches!(
            self,
            Self::DeviceListEntrySnapshot { .. }
                | Self::DeviceListDeviceRemoved { .. }
                | Self::DeviceListDeviceRenamed { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationStatus {
    Published,
    SkippedDisabled,
    SkippedUnbound,
    AppendFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationResult {
    pub status: PublicationStatus,
    pub appended: usize,
    pub errors: Vec<String>,
}

impl PublicationResult {
    fn published(appended: usize) -> Self {
        Self {
            status: PublicationStatus::Published,
            appended,
            errors: Vec::new(),
        }
    }

    fn skipped(status: PublicationStatus) -> Self {
        Self {
            status,
            appended: 0,
            errors: Vec::new(),
        }
    }

    fn append_failed(appended: usize, errors: Vec<String>) -> Self {
        Self {
            status: PublicationStatus::AppendFailed,
            appended,
            errors,
        }
    }
}

#[derive(Clone)]
pub struct SyncPublicationRuntime {
    data_dir: PathBuf,
    sync_dir: PathBuf,
    sync_enabled: Arc<AtomicBool>,
    sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    sync_network_secret: Arc<std::sync::RwLock<Option<Arc<NetworkSecret>>>>,
    sync_device: Arc<std::sync::RwLock<Option<Arc<DeviceIdentity>>>>,
    sync_hlc_clock: Option<crate::state::SyncHlcClockCell>,
    sync_trigger: Arc<tokio::sync::RwLock<Option<rua_sync::trigger::SyncTriggerHandle>>>,
    sync_stage_recorder: Arc<rua_sync::stage_diagnostics::SyncStageRecorder>,
}

impl SyncPublicationRuntime {
    pub fn from_app_state(state: &AppState) -> Self {
        Self {
            data_dir: state.rua_root.clone(),
            sync_dir: state.rua_root.join("sync"),
            sync_enabled: Arc::clone(&state.sync_enabled),
            sync_node: Arc::clone(&state.sync_node),
            sync_network_secret: Arc::clone(&state.sync_network_secret),
            sync_device: Arc::clone(&state.sync_device),
            sync_hlc_clock: Some(Arc::clone(&state.sync_hlc_clock)),
            sync_trigger: Arc::clone(&state.sync_trigger),
            sync_stage_recorder: Arc::clone(&state.sync_stage_recorder),
        }
    }

    pub fn new(
        sync_dir: PathBuf,
        sync_enabled: Arc<AtomicBool>,
        sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
        sync_network_secret: Arc<std::sync::RwLock<Option<Arc<NetworkSecret>>>>,
        sync_device: Arc<std::sync::RwLock<Option<Arc<DeviceIdentity>>>>,
        sync_hlc_clock: Option<crate::state::SyncHlcClockCell>,
        sync_trigger: Arc<tokio::sync::RwLock<Option<rua_sync::trigger::SyncTriggerHandle>>>,
    ) -> Self {
        let data_dir = sync_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| sync_dir.clone());
        Self {
            data_dir,
            sync_dir,
            sync_enabled,
            sync_node,
            sync_network_secret,
            sync_device,
            sync_hlc_clock,
            sync_trigger,
            sync_stage_recorder: Arc::new(rua_sync::stage_diagnostics::SyncStageRecorder::default()),
        }
    }
}

pub async fn publish_sync_fact(state: &AppState, fact: SyncPublicationFact) -> PublicationResult {
    let runtime = SyncPublicationRuntime::from_app_state(state);
    publish_sync_fact_with_runtime(&runtime, fact).await
}

pub async fn publish_agent_config_committed(
    state: &AppState,
    agent_id: &str,
    config: serde_json::Value,
) -> PublicationResult {
    let portable =
        match normalize_agent_config_for_publication(&config, state.device_key.as_bytes()) {
            Ok(value) => value,
            Err(error) => {
                warn!(
                    error = %error,
                    agent_id,
                    "[sync-publication] skip agent config publication: secret normalization failed"
                );
                return PublicationResult::append_failed(0, vec![error.to_string()]);
            }
        };
    publish_sync_fact(
        state,
        SyncPublicationFact::AgentConfigCommitted {
            agent_id: agent_id.to_string(),
            config: portable,
        },
    )
    .await
}

pub async fn publish_agent_config_deleted(state: &AppState, agent_id: &str) -> PublicationResult {
    publish_sync_fact(
        state,
        SyncPublicationFact::AgentConfigDeleted {
            agent_id: agent_id.to_string(),
        },
    )
    .await
}

pub async fn publish_project_descriptor_committed(
    state: &AppState,
    descriptor: rua_project::ProjectDescriptor,
) -> PublicationResult {
    publish_sync_fact(
        state,
        SyncPublicationFact::ProjectDescriptorCommitted { descriptor },
    )
    .await
}

pub async fn publish_project_permanently_deleted(
    state: &AppState,
    project_id: rua_core::ProjectId,
    scope_root: PathBuf,
    chat_addresses: Vec<ChatAddress>,
) -> PublicationResult {
    publish_sync_fact(
        state,
        SyncPublicationFact::ProjectPermanentlyDeleted {
            project_id,
            scope_root,
            chat_addresses,
        },
    )
    .await
}

pub async fn publish_agent_config_file_committed(
    state: &AppState,
    agent_id: &str,
    path: &Path,
    label: &'static str,
) -> PublicationResult {
    let mut value = match std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
    {
        Some(value) => value,
        None => serde_json::json!({ "id": agent_id }),
    };
    if let Some(obj) = value.as_object_mut() {
        obj.entry("id")
            .or_insert_with(|| serde_json::Value::String(agent_id.to_string()));
    }
    let result = publish_agent_config_committed(state, agent_id, value).await;
    if result.status == PublicationStatus::AppendFailed {
        warn!(
            agent_id,
            label,
            errors = ?result.errors,
            "[sync-publication] agent config file publication failed"
        );
    }
    result
}

pub async fn publish_agent_security_committed(
    state: &AppState,
    agent_id: &str,
    config: serde_json::Value,
) -> PublicationResult {
    publish_sync_fact(
        state,
        SyncPublicationFact::AgentSecurityCommitted {
            agent_id: agent_id.to_string(),
            config,
        },
    )
    .await
}

pub async fn publish_agent_security_deleted(state: &AppState, agent_id: &str) -> PublicationResult {
    publish_sync_fact(
        state,
        SyncPublicationFact::AgentSecurityDeleted {
            agent_id: agent_id.to_string(),
        },
    )
    .await
}

pub async fn publish_shared_mcp_catalog_committed(
    state: &AppState,
    catalog: serde_json::Value,
) -> PublicationResult {
    let logical = match normalize_mcp_config_for_publication(&catalog, state.device_key.as_bytes())
    {
        Ok(value) => value,
        Err(error) => {
            warn!(
                error = %error,
                "[sync-publication] skip shared MCP publication: secret normalization failed"
            );
            return PublicationResult::append_failed(0, vec![error.to_string()]);
        }
    };
    publish_sync_fact(
        state,
        SyncPublicationFact::SharedMcpCatalogCommitted { catalog: logical },
    )
    .await
}

pub async fn publish_shared_mcp_client_deleted(
    state: &AppState,
    client_key: &str,
) -> PublicationResult {
    publish_sync_fact(
        state,
        SyncPublicationFact::SharedMcpClientDeleted {
            client_key: client_key.to_string(),
        },
    )
    .await
}

pub fn normalize_mcp_config_for_publication(
    config: &serde_json::Value,
    device_key: &[u8; 32],
) -> std::io::Result<serde_json::Value> {
    rua_mcp::secret_field_io::decrypt_client_value(config, Some(device_key))
}

pub async fn publish_skill_pool_committed(state: &AppState, skill_name: &str) -> PublicationResult {
    publish_sync_fact(
        state,
        SyncPublicationFact::SkillPoolCommitted {
            skill_name: skill_name.to_string(),
        },
    )
    .await
}

pub async fn publish_skill_pool_deleted(state: &AppState, skill_name: &str) -> PublicationResult {
    publish_sync_fact(
        state,
        SyncPublicationFact::SkillPoolDeleted {
            skill_name: skill_name.to_string(),
        },
    )
    .await
}

pub async fn publish_workspace_skill_committed(
    state: &AppState,
    agent_id: &str,
    skill_name: &str,
) -> PublicationResult {
    publish_sync_fact(
        state,
        SyncPublicationFact::WorkspaceSkillCommitted {
            agent_id: agent_id.to_string(),
            skill_name: skill_name.to_string(),
        },
    )
    .await
}

pub async fn publish_workspace_skill_deleted(
    state: &AppState,
    agent_id: &str,
    skill_name: &str,
) -> PublicationResult {
    publish_sync_fact(
        state,
        SyncPublicationFact::WorkspaceSkillDeleted {
            agent_id: agent_id.to_string(),
            skill_name: skill_name.to_string(),
        },
    )
    .await
}

pub fn normalize_agent_config_for_publication(
    config: &serde_json::Value,
    device_key: &[u8; 32],
) -> std::io::Result<serde_json::Value> {
    let mut config = config.clone();
    if let Some(mcp_value) = config.get("mcp").cloned() {
        let mcp_value =
            rua_mcp::secret_field_io::decrypt_client_value(&mcp_value, Some(device_key))?;
        if let Some(obj) = config.as_object_mut() {
            obj.insert("mcp".to_string(), mcp_value);
        }
    }
    if let Some(obj) = config.as_object_mut() {
        obj.remove("channels");
        obj.remove("security");
        obj.remove("running");
        obj.remove("default_model");
    }
    strip_device_local_acp_runner_fields(&mut config);
    Ok(config)
}

fn strip_device_local_acp_runner_fields(config: &mut serde_json::Value) {
    let Some(runners) = config
        .get_mut("acp")
        .and_then(|value| value.get_mut("agents"))
        .and_then(|value| value.as_object_mut())
    else {
        return;
    };
    for runner in runners.values_mut() {
        let Some(runner_obj) = runner.as_object_mut() else {
            continue;
        };
        for field in ["command", "args", "env"] {
            runner_obj.remove(field);
        }
    }
}

pub async fn publish_sync_fact_with_runtime(
    runtime: &SyncPublicationRuntime,
    fact: SyncPublicationFact,
) -> PublicationResult {
    let commit_observed_at_ms = rua_sync::stage_diagnostics::wall_now_ms();
    let commit_observed = Instant::now();
    if !runtime
        .sync_enabled
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return PublicationResult::skipped(PublicationStatus::SkippedDisabled);
    }

    let (bound_network, device) = match bound_sync_network(runtime).await {
        Ok(value) => value,
        Err(error) => {
            tracing::debug!(error = %error, "[sync-publication] skipped: bound sync network unavailable");
            return PublicationResult::skipped(PublicationStatus::SkippedUnbound);
        }
    };

    let sync_meta = build_sync_meta(&device, runtime.sync_hlc_clock.as_ref());
    let blob_context = Some(bound_network.blob_context());
    let is_device_list_fact = fact.is_device_list_fact();
    let changes = match crate::sync_domain_catalog::build_publication_changes(
        fact,
        sync_meta,
        crate::sync_domain_catalog::PublicationChangeBuildContext {
            data_dir: &runtime.data_dir,
            blob_context,
        },
    )
    .await
    {
        Ok(changes) => changes,
        Err(error) => {
            warn!(error = %error, "[sync-publication] build publication changes failed");
            runtime.sync_stage_recorder.record_failure(
                rua_sync::stage_diagnostics::SyncStageFailure {
                    correlation_id: None,
                    entry_fingerprint: None,
                    stage: rua_sync::stage_diagnostics::SyncFailureStage::Publication,
                    cause: "change_build_failed".to_string(),
                    observed_at_ms: rua_sync::stage_diagnostics::wall_now_ms(),
                },
            );
            return PublicationResult::append_failed(0, vec![error]);
        }
    };

    let publication_started_at_ms = rua_sync::stage_diagnostics::wall_now_ms();
    let publication_started = Instant::now();
    let result = append_changes(
        bound_network.opaque_replica_log(),
        changes,
        &runtime.sync_stage_recorder,
        rua_sync::stage_diagnostics::SyncSourceTiming {
            commit_observed_at_ms,
            publication_started_at_ms,
        },
        commit_observed,
        publication_started,
    )
    .await;
    if result.appended > 0 {
        notify_write_coalesced_sync(runtime).await;
        if is_device_list_fact {
            notify_device_list_all_peer_sync(runtime);
        }
    }
    result
}

async fn bound_sync_network(
    runtime: &SyncPublicationRuntime,
) -> Result<(BoundSyncNetwork, Arc<DeviceIdentity>), BoundSyncNetworkError> {
    let network_secret = runtime
        .sync_network_secret
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(Arc::clone));
    let device = runtime
        .sync_device
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(Arc::clone));
    let node = runtime.sync_node.lock().await.as_ref().cloned();
    let bound = BoundSyncNetwork::from_optional_parts(
        &runtime.sync_dir,
        node.as_ref(),
        network_secret,
        device.clone(),
    )?;
    let device = device.ok_or(BoundSyncNetworkError::NotPaired)?;
    Ok((bound, device))
}

fn build_sync_meta(
    device: &DeviceIdentity,
    hlc_clock: Option<&crate::state::SyncHlcClockCell>,
) -> SyncMeta {
    let device_id_bytes = device.device_id_short();
    let hlc = if let Some(clock_cell) = hlc_clock {
        let mut guard = match clock_cell.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                warn!("[sync-publication] sync_hlc_clock Mutex poisoned");
                poisoned.into_inner()
            }
        };
        let reset_clock = guard
            .as_ref()
            .map(|(current_device, _)| *current_device != device_id_bytes)
            .unwrap_or(true);
        if reset_clock {
            *guard = Some((
                device_id_bytes,
                rua_sync::hlc::HlcClock::new(device_id_bytes),
            ));
        }
        guard
            .as_mut()
            .map(|(_, clock)| clock.now())
            .unwrap_or_else(|| rua_sync::hlc::HlcClock::new(device_id_bytes).now())
    } else {
        rua_sync::hlc::HlcClock::new(device_id_bytes).now()
    };

    SyncMeta {
        device_id: hex::encode(device_id_bytes),
        hlc,
        modified_at: chrono::Utc::now().to_rfc3339(),
    }
}

async fn notify_write_coalesced_sync(runtime: &SyncPublicationRuntime) {
    let Some(handle) = runtime.sync_trigger.read().await.clone() else {
        tracing::debug!("[sync-publication] write trigger skipped: sync trigger unavailable");
        return;
    };
    handle.notify_write();
}

fn notify_device_list_all_peer_sync(runtime: &SyncPublicationRuntime) {
    let sync_node = Arc::clone(&runtime.sync_node);
    let sync_dir = runtime.sync_dir.clone();
    tokio::spawn(async move {
        match crate::sync_cycle::run_sync_cycle_to_all_device_list_peers(&sync_node, &sync_dir)
            .await
        {
            Ok(result) => {
                tracing::info!(
                    attempted = result.attempted,
                    succeeded = result.succeeded,
                    failed = result.failed,
                    events_sent = result.events_sent,
                    events_received = result.events_received,
                    already_syncing_retries = result.already_syncing_retries,
                    "[sync-publication] device-list all-peer docs sync completed"
                );
            }
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "[sync-publication] device-list all-peer docs sync skipped"
                );
            }
        }
    });
}

async fn append_changes(
    log: rua_sync::opaque_replica_log::OpaqueReplicaLog,
    changes: Vec<SyncChange>,
    stage_recorder: &rua_sync::stage_diagnostics::SyncStageRecorder,
    source_timing: rua_sync::stage_diagnostics::SyncSourceTiming,
    commit_observed: Instant,
    publication_started: Instant,
) -> PublicationResult {
    let mut appended = 0usize;
    let mut errors = Vec::new();
    for mut change in changes {
        change.source_timing = Some(source_timing.clone());
        let append_started = Instant::now();
        match log.append_change(&change).await {
            Ok(_) => {
                appended += 1;
                stage_recorder.record_publication(
                    rua_sync::stage_diagnostics::SyncPublicationStageSample {
                        correlation_id: change.op_id.clone(),
                        commit_observed_at_ms: source_timing.commit_observed_at_ms,
                        publication_started_at_ms: source_timing.publication_started_at_ms,
                        publication_completed_at_ms: rua_sync::stage_diagnostics::wall_now_ms(),
                        commit_to_publication_ms: rua_sync::stage_diagnostics::millis(
                            publication_started
                                .checked_duration_since(commit_observed)
                                .unwrap_or_default(),
                        ),
                        publication_ms: rua_sync::stage_diagnostics::millis(
                            append_started.elapsed(),
                        ),
                        status: rua_sync::stage_diagnostics::SyncStageStatus::Success,
                        failure_cause: None,
                    },
                );
            }
            Err(error) => {
                warn!(
                    error = %error,
                    domain = %change.domain,
                    record_id = %change.record_id,
                    "[sync-publication] append v4 change failed"
                );
                stage_recorder.record_publication(
                    rua_sync::stage_diagnostics::SyncPublicationStageSample {
                        correlation_id: change.op_id.clone(),
                        commit_observed_at_ms: source_timing.commit_observed_at_ms,
                        publication_started_at_ms: source_timing.publication_started_at_ms,
                        publication_completed_at_ms: rua_sync::stage_diagnostics::wall_now_ms(),
                        commit_to_publication_ms: rua_sync::stage_diagnostics::millis(
                            publication_started
                                .checked_duration_since(commit_observed)
                                .unwrap_or_default(),
                        ),
                        publication_ms: rua_sync::stage_diagnostics::millis(
                            append_started.elapsed(),
                        ),
                        status: rua_sync::stage_diagnostics::SyncStageStatus::Failure,
                        failure_cause: Some("append_failed".to_string()),
                    },
                );
                errors.push(format!("{}:{}: {error}", change.domain, change.record_id));
            }
        }
    }

    if errors.is_empty() {
        PublicationResult::published(appended)
    } else {
        PublicationResult::append_failed(appended, errors)
    }
}
