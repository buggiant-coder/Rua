use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};

use crate::state::AppState;
use crate::sync_network::{BoundSyncNetwork, BoundSyncNetworkError};

const MAX_INBOUND_DRAIN_PER_PASS: usize = 64;
const MAX_INBOUND_DRAIN_READ_CONCURRENCY: usize = 4;
const INBOUND_READ_TIMEOUT: Duration = Duration::from_millis(1_500);
const INBOUND_ACTIVE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(45);
const INBOUND_DRAIN_TIME_BUDGET: Duration = Duration::from_secs(90);
const MAX_AUTO_APPLY_ATTEMPTS: u32 = 3;

#[derive(Debug)]
pub enum LocalSyncCompletionError {
    RepairScan(rua_sync::SyncError),
    InboxDrain(rua_sync::SyncError),
}

impl std::fmt::Display for LocalSyncCompletionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RepairScan(error) => write!(f, "Repair Scan failed: {error}"),
            Self::InboxDrain(error) => write!(f, "Inbound Inbox drain failed: {error}"),
        }
    }
}

impl std::error::Error for LocalSyncCompletionError {}

#[derive(Clone)]
struct ResourceApplicationContext {
    mcp_cache: Arc<crate::mcp_tools_cache::McpToolsCache>,
    voice: Arc<crate::voice_runtime::VoiceRuntime>,
    voice_context: crate::voice_runtime::VoiceApplicationContext,
}

#[derive(Clone)]
pub struct InboundApplyRuntime {
    resource_application: Option<ResourceApplicationContext>,
    sidecar_store: Arc<rua_sync::sidecar::SidecarStore>,
    sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    sync_network_secret: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::NetworkSecret>>>>,
    sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    provider_manager: Arc<rua_provider::manager::ProviderManager>,
    project_catalog: Arc<rua_project::ProjectCatalog>,
    security: Arc<rua_security::SecurityModule>,
    registry: Option<Arc<dyn rua_workspace::WorkspaceRegistry>>,
    conversation_input_intake:
        Option<crate::services::conversation_input_intake::ConversationInputIntake>,
    device_key: [u8; 32],
    data_dir: PathBuf,
    secret_dir: PathBuf,
    inbound_inbox: Arc<rua_sync::inbound_inbox::InboundInbox>,
    sync_stage_recorder: Arc<rua_sync::stage_diagnostics::SyncStageRecorder>,
    apply_lock: Arc<tokio::sync::Mutex<()>>,
    active_stream_broadcast_hub:
        Option<Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>>,
    chat_activity_wake_hub: Option<Arc<crate::services::chat_activity_wake::ChatActivityWakeHub>>,
}

impl InboundApplyRuntime {
    pub async fn apply_agent_configuration_resources(
        &self,
        change: &rua_sync::opaque_replica_log::SyncChange,
        result: &rua_sync::sync_registry::RegistryApplyResult,
    ) -> Result<(), String> {
        apply_agent_resources_after_inbound_config_inner(
            &self.registry,
            &self.conversation_input_intake,
            &self.device_key,
            self.resource_application.as_ref(),
            change,
            result,
        )
        .await
    }

    pub fn from_state(state: &AppState) -> Self {
        Self {
            resource_application: Some(ResourceApplicationContext {
                mcp_cache: state.mcp_tools_cache.clone(),
                voice: state.voice_runtime.clone(),
                voice_context: crate::voice_runtime::VoiceApplicationContext::from_state(state),
            }),
            sidecar_store: Arc::clone(&state.sidecar_store),
            sync_node: Arc::clone(&state.sync_node),
            sync_network_secret: Arc::clone(&state.sync_network_secret),
            sync_device: Arc::clone(&state.sync_device),
            provider_manager: Arc::clone(&state.provider_manager),
            project_catalog: Arc::clone(&state.project_catalog),
            security: Arc::clone(&state.security),
            registry: Some(Arc::clone(&state.registry)),
            conversation_input_intake: Some(state.conversation_input_intake.clone()),
            device_key: *state.device_key.as_bytes(),
            data_dir: state.rua_root.clone(),
            secret_dir: state.secret_dir.clone(),
            inbound_inbox: Arc::clone(&state.inbound_inbox),
            sync_stage_recorder: Arc::clone(&state.sync_stage_recorder),
            apply_lock: Arc::clone(&state.inbound_apply_lock),
            active_stream_broadcast_hub: Some(Arc::clone(&state.active_stream_broadcast_hub)),
            chat_activity_wake_hub: Some(Arc::clone(&state.chat_activity_wake_hub)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        sidecar_store: Arc<rua_sync::sidecar::SidecarStore>,
        sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
        sync_network_secret: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::NetworkSecret>>>>,
        sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
        provider_manager: Arc<rua_provider::manager::ProviderManager>,
        security: Arc<rua_security::SecurityModule>,
        registry: Option<Arc<dyn rua_workspace::WorkspaceRegistry>>,
        device_key: [u8; 32],
        data_dir: PathBuf,
        secret_dir: PathBuf,
        inbound_inbox: Arc<rua_sync::inbound_inbox::InboundInbox>,
        apply_lock: Arc<tokio::sync::Mutex<()>>,
        project_catalog: Arc<rua_project::ProjectCatalog>,
    ) -> Self {
        Self {
            resource_application: None,
            sidecar_store,
            sync_node,
            sync_network_secret,
            sync_device,
            provider_manager,
            project_catalog,
            security,
            registry,
            conversation_input_intake: None,
            device_key,
            data_dir,
            secret_dir,
            inbound_inbox,
            sync_stage_recorder: Arc::new(rua_sync::stage_diagnostics::SyncStageRecorder::default()),
            apply_lock,
            active_stream_broadcast_hub: None,
            chat_activity_wake_hub: None,
        }
    }

    pub async fn repair_scan_and_drain(
        &self,
    ) -> Result<rua_sync::inbound_inbox::RepairScanReport, rua_sync::SyncError> {
        self.repair_scan_and_drain_staged()
            .await
            .map_err(|error| match error {
                LocalSyncCompletionError::RepairScan(error)
                | LocalSyncCompletionError::InboxDrain(error) => error,
            })
    }

    pub async fn repair_scan_and_drain_staged(
        &self,
    ) -> Result<rua_sync::inbound_inbox::RepairScanReport, LocalSyncCompletionError> {
        let _guard = self.apply_lock.lock().await;
        let bound_network = self.bound_sync_network().await.map_err(|e| {
            LocalSyncCompletionError::RepairScan(rua_sync::SyncError::Store(format!(
                "bound sync network: {e}"
            )))
        })?;
        let report = self
            .inbound_inbox
            .repair_scan_doc_entries(bound_network.doc())
            .await
            .map_err(LocalSyncCompletionError::RepairScan)?;
        self.drain_once_inner()
            .await
            .map_err(LocalSyncCompletionError::InboxDrain)?;
        let retryable = self
            .inbound_inbox
            .count_by_state(rua_sync::inbound_inbox::InboxState::Retryable)
            .map_err(LocalSyncCompletionError::InboxDrain)?;
        let failed = self
            .inbound_inbox
            .count_by_state(rua_sync::inbound_inbox::InboxState::Failed)
            .map_err(LocalSyncCompletionError::InboxDrain)?;
        if retryable > 0 || failed > 0 {
            return Err(LocalSyncCompletionError::InboxDrain(
                rua_sync::SyncError::Store(format!(
                    "Inbound Inbox drain incomplete: {retryable} retryable, {failed} failed"
                )),
            ));
        }
        Ok(report)
    }

    pub async fn drain_once(&self) -> Result<(), rua_sync::SyncError> {
        let _guard = self.apply_lock.lock().await;
        self.drain_once_inner().await
    }

    async fn drain_once_inner(&self) -> Result<(), rua_sync::SyncError> {
        let merge_store =
            rua_sync::merge_store::MergeStore::from_shared(Arc::clone(&self.sidecar_store));
        let bound_network = self
            .bound_sync_network()
            .await
            .map_err(|e| rua_sync::SyncError::Store(format!("bound sync network: {e}")))?;
        let device_id_hex = bound_network.local_device_id_hex();
        let local_author = bound_network.local_author();
        let v4_log = bound_network.opaque_replica_log();
        let active_blob_download =
            build_active_blob_download(&self.sync_node, &self.data_dir).await;
        let device_session_manager = self
            .sync_node
            .lock()
            .await
            .as_ref()
            .map(|node| node.network_runtime().session_manager().clone());

        let result = drain_inbound_inbox(InboundDrainContext {
            resource_application: self.resource_application.clone(),
            inbox: &self.inbound_inbox,
            data_dir: &self.data_dir,
            secret_dir: &self.secret_dir,
            local_device_id: &device_id_hex,
            local_author: Some(local_author),
            v4_log: Some(&v4_log),
            active_blob_download: active_blob_download.as_ref(),
            merge_store: &merge_store,
            provider_manager: Arc::clone(&self.provider_manager),
            project_catalog: Arc::clone(&self.project_catalog),
            security: Arc::clone(&self.security),
            registry: self.registry.clone(),
            conversation_input_intake: self.conversation_input_intake.clone(),
            device_key: self.device_key,
            active_stream_broadcast_hub: self.active_stream_broadcast_hub.clone(),
            chat_activity_wake_hub: self.chat_activity_wake_hub.clone(),
            device_session_manager,
            sync_stage_recorder: Arc::clone(&self.sync_stage_recorder),
        })
        .await;
        if let Some(error) = result.error.as_ref() {
            return Err(rua_sync::SyncError::Store(error.clone()));
        }
        if result.retryable > 0 || result.discarded > 0 {
            return Err(rua_sync::SyncError::Store(format!(
                "Inbound Inbox drain incomplete: {} retryable, {} discarded",
                result.retryable, result.discarded
            )));
        }
        Ok(())
    }

    pub async fn apply_liveactor_mode(&self, mode: rua_sync::liveactor_apply::ApplyMode) {
        let _guard = self.apply_lock.lock().await;
        let merge_store =
            rua_sync::merge_store::MergeStore::from_shared(Arc::clone(&self.sidecar_store));

        match mode {
            rua_sync::liveactor_apply::ApplyMode::RepairScan { force } => {
                let bound_network = match self.bound_sync_network().await {
                    Ok(bound_network) => bound_network,
                    Err(e) => {
                        tracing::warn!(error = %e, "[liveactor] bound sync network");
                        return;
                    }
                };
                self.run_repair_scan_if_needed(bound_network.doc(), force, "repair_request")
                    .await;
                self.drain_bound_network(bound_network, &merge_store).await;
            }
            rua_sync::liveactor_apply::ApplyMode::Incremental(entries, content_ready) => {
                for hash in content_ready {
                    match self
                        .inbound_inbox
                        .reactivate_dormant_content(hash, rua_sync::inbound_inbox::now_ms())
                    {
                        Ok(reactivated) if reactivated > 0 => tracing::info!(
                            %hash,
                            reactivated,
                            "[liveactor] reactivated dormant entries for ready content"
                        ),
                        Ok(_) => {}
                        Err(error) => tracing::warn!(
                            %hash,
                            %error,
                            "[liveactor] reactivate dormant ready content"
                        ),
                    }
                }
                for entry in entries {
                    self.sync_stage_recorder
                        .record_transport_arrival(entry.author, &entry.key);
                    if let Err(e) = self.inbound_inbox.record_remote_insert(
                        entry.author,
                        &entry.key,
                        entry.content_hash,
                    ) {
                        tracing::warn!(error = %e, "[liveactor] record inbound entry");
                        self.sync_stage_recorder.record_failure(
                            rua_sync::stage_diagnostics::SyncStageFailure {
                                correlation_id: None,
                                entry_fingerprint: Some(
                                    rua_sync::stage_diagnostics::entry_fingerprint(
                                        entry.author,
                                        &entry.key,
                                    ),
                                ),
                                stage: rua_sync::stage_diagnostics::SyncFailureStage::InboundInbox,
                                cause: "record_failed".to_string(),
                                observed_at_ms: rua_sync::stage_diagnostics::wall_now_ms(),
                            },
                        );
                    } else {
                        self.sync_stage_recorder
                            .record_inbox_recorded(entry.author, &entry.key);
                    }
                }
                let bound_network = match self.bound_sync_network().await {
                    Ok(bound_network) => bound_network,
                    Err(e) => {
                        tracing::warn!(error = %e, "[liveactor] bound sync network");
                        return;
                    }
                };
                self.run_repair_scan_if_needed(bound_network.doc(), false, "incremental")
                    .await;
                self.drain_bound_network(bound_network, &merge_store).await;
            }
            rua_sync::liveactor_apply::ApplyMode::Drain(content_ready) => {
                for hash in content_ready {
                    match self
                        .inbound_inbox
                        .reactivate_dormant_content(hash, rua_sync::inbound_inbox::now_ms())
                    {
                        Ok(reactivated) if reactivated > 0 => tracing::info!(
                            %hash,
                            reactivated,
                            "[liveactor] reactivated dormant entries for ready content"
                        ),
                        Ok(_) => {}
                        Err(error) => tracing::warn!(
                            %hash,
                            %error,
                            "[liveactor] reactivate dormant ready content"
                        ),
                    }
                }
                let bound_network = match self.bound_sync_network().await {
                    Ok(bound_network) => bound_network,
                    Err(e) => {
                        tracing::warn!(error = %e, "[liveactor] bound sync network");
                        return;
                    }
                };
                self.run_repair_scan_if_needed(bound_network.doc(), false, "drain")
                    .await;
                self.drain_bound_network(bound_network, &merge_store).await;
            }
        }
    }

    async fn run_repair_scan_if_needed(
        &self,
        doc: &iroh_docs::api::Doc,
        force: bool,
        trigger: &'static str,
    ) {
        let should_scan = if force {
            true
        } else {
            match self
                .inbound_inbox
                .repair_scan_due(rua_sync::inbound_inbox::now_ms())
            {
                Ok(due) => due,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        trigger,
                        "[liveactor] failed to read Repair Scan state; scanning"
                    );
                    true
                }
            }
        };
        if !should_scan {
            return;
        }
        match self.inbound_inbox.repair_scan_doc_entries(doc).await {
            Ok(report) => tracing::debug!(
                scanned = report.scanned,
                recorded = report.recorded,
                already_applied = report.already_applied,
                errors = report.errors.len(),
                duration_ms = report.duration_ms,
                force,
                trigger,
                "[liveactor] Repair Scan completed"
            ),
            Err(error) => {
                tracing::warn!(%error, force, trigger, "[liveactor] Repair Scan failed")
            }
        }
    }

    async fn drain_bound_network(
        &self,
        bound_network: BoundSyncNetwork,
        merge_store: &rua_sync::merge_store::MergeStore,
    ) {
        let v4_log = bound_network.opaque_replica_log();
        let device_id_hex = bound_network.local_device_id_hex();
        let local_author = bound_network.local_author();
        let active_blob_download =
            build_active_blob_download(&self.sync_node, &self.data_dir).await;
        let device_session_manager = self
            .sync_node
            .lock()
            .await
            .as_ref()
            .map(|node| node.network_runtime().session_manager().clone());
        let result = drain_inbound_inbox(InboundDrainContext {
            resource_application: self.resource_application.clone(),
            inbox: &self.inbound_inbox,
            data_dir: &self.data_dir,
            secret_dir: &self.secret_dir,
            local_device_id: &device_id_hex,
            local_author: Some(local_author),
            v4_log: Some(&v4_log),
            active_blob_download: active_blob_download.as_ref(),
            merge_store,
            provider_manager: Arc::clone(&self.provider_manager),
            project_catalog: Arc::clone(&self.project_catalog),
            security: Arc::clone(&self.security),
            registry: self.registry.clone(),
            conversation_input_intake: self.conversation_input_intake.clone(),
            device_key: self.device_key,
            active_stream_broadcast_hub: self.active_stream_broadcast_hub.clone(),
            chat_activity_wake_hub: self.chat_activity_wake_hub.clone(),
            device_session_manager,
            sync_stage_recorder: Arc::clone(&self.sync_stage_recorder),
        })
        .await;
        if let Some(error) = result.error.as_ref() {
            tracing::warn!(%error, "[liveactor] inbound drain stopped");
        }
    }

    async fn bound_sync_network(&self) -> Result<BoundSyncNetwork, BoundSyncNetworkError> {
        let sync_dir = self.data_dir.join("sync");
        let network_secret = self
            .sync_network_secret
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(Arc::clone));
        let device = self
            .sync_device
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(Arc::clone));
        let node = self.sync_node.lock().await.as_ref().cloned();
        BoundSyncNetwork::from_optional_parts(&sync_dir, node.as_ref(), network_secret, device)
    }
}

struct InboundDrainContext<'a> {
    resource_application: Option<ResourceApplicationContext>,
    inbox: &'a rua_sync::inbound_inbox::InboundInbox,
    data_dir: &'a Path,
    secret_dir: &'a Path,
    local_device_id: &'a str,
    local_author: Option<rua_sync::opaque_replica_log::AuthorId>,
    v4_log: Option<&'a rua_sync::opaque_replica_log::OpaqueReplicaLog>,
    active_blob_download: Option<&'a ActiveBlobDownload>,
    merge_store: &'a rua_sync::merge_store::MergeStore,
    provider_manager: Arc<rua_provider::manager::ProviderManager>,
    project_catalog: Arc<rua_project::ProjectCatalog>,
    security: Arc<rua_security::SecurityModule>,
    registry: Option<Arc<dyn rua_workspace::WorkspaceRegistry>>,
    conversation_input_intake:
        Option<crate::services::conversation_input_intake::ConversationInputIntake>,
    device_key: [u8; 32],
    active_stream_broadcast_hub:
        Option<Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>>,
    chat_activity_wake_hub: Option<Arc<crate::services::chat_activity_wake::ChatActivityWakeHub>>,
    device_session_manager: Option<rua_device_network::DeviceSessionManager>,
    sync_stage_recorder: Arc<rua_sync::stage_diagnostics::SyncStageRecorder>,
}

#[derive(Default)]
struct InboundDrainResult {
    retryable: usize,
    discarded: usize,
    error: Option<String>,
}

async fn drain_inbound_inbox(args: InboundDrainContext<'_>) -> InboundDrainResult {
    let InboundDrainContext {
        resource_application,
        inbox,
        data_dir,
        secret_dir,
        local_device_id,
        local_author,
        v4_log,
        active_blob_download,
        merge_store,
        provider_manager,
        project_catalog,
        security,
        registry,
        conversation_input_intake,
        device_key,
        active_stream_broadcast_hub,
        chat_activity_wake_hub,
        device_session_manager,
        sync_stage_recorder,
    } = args;
    let started = Instant::now();

    match local_author {
        Some(local_author) => match inbox.mark_author_applied(local_author) {
            Ok(applied) if applied > 0 => {
                tracing::info!(
                    applied,
                    "[liveactor] marked self-authored sync inbox entries applied"
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "[liveactor] mark self-authored inbox applied"),
        },
        None => tracing::warn!("[liveactor] local author id is unavailable"),
    }

    let blob_context = v4_log.map(|log| log.blob_context());
    let blob_download =
        active_blob_download.map(|download| crate::sync_domain_catalog::BlobDownloadContext {
            downloader: download.downloader.clone(),
            providers: download.providers.clone(),
        });
    let v4_registry = match crate::sync_domain_catalog::build_apply_registry(
        data_dir.to_path_buf(),
        secret_dir.to_path_buf(),
        local_device_id.to_string(),
        device_key,
        provider_manager,
        Arc::clone(&security),
        blob_context,
        blob_download,
        Arc::clone(&project_catalog),
    ) {
        Ok(registry) => registry,
        Err(e) => {
            tracing::warn!(error = %e, "[liveactor] build v4 apply domain catalog");
            return InboundDrainResult {
                error: Some(format!("build v4 apply domain catalog: {e}")),
                ..Default::default()
            };
        }
    };

    let mut total_applied = 0usize;
    let mut total_retryable = 0usize;
    let mut total_discarded = 0usize;
    let mut passes = 0usize;
    loop {
        if started.elapsed() >= INBOUND_DRAIN_TIME_BUDGET {
            tracing::debug!(
                passes,
                total_applied,
                total_retryable,
                total_discarded,
                "[liveactor] inbound drain stopped at time budget"
            );
            return InboundDrainResult {
                retryable: total_retryable,
                discarded: total_discarded,
                error: Some("Inbound Inbox drain reached its time budget".to_string()),
            };
        }

        let now = rua_sync::inbound_inbox::now_ms();
        let entries = match inbox.pending_ready_limited(now, MAX_INBOUND_DRAIN_PER_PASS) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(error = %e, "[liveactor] read inbound inbox");
                return InboundDrainResult {
                    retryable: total_retryable,
                    discarded: total_discarded,
                    error: Some(format!("read Inbound Inbox: {e}")),
                };
            }
        };
        if entries.is_empty() {
            break;
        }
        passes += 1;

        let pass_result = drain_inbound_batch(InboundBatchContext {
            resource_application: resource_application.clone(),
            device_key: &device_key,
            entries,
            inbox,
            data_dir,
            local_device_id,
            v4_log,
            active_blob_download,
            v4_registry: &v4_registry,
            merge_store,
            project_catalog: Arc::clone(&project_catalog),
            security: Arc::clone(&security),
            registry: registry.clone(),
            conversation_input_intake: conversation_input_intake.clone(),
            active_stream_broadcast_hub: active_stream_broadcast_hub.clone(),
            chat_activity_wake_hub: chat_activity_wake_hub.clone(),
            device_session_manager: device_session_manager.clone(),
            sync_stage_recorder: Arc::clone(&sync_stage_recorder),
        })
        .await;
        total_applied += pass_result.applied;
        total_retryable += pass_result.retryable;
        total_discarded += pass_result.discarded;
    }

    tracing::info!(
        passes,
        total_applied,
        total_retryable,
        total_discarded,
        "[liveactor] inbound drain completed"
    );
    InboundDrainResult {
        retryable: total_retryable,
        discarded: total_discarded,
        error: None,
    }
}

struct InboundBatchContext<'a> {
    resource_application: Option<ResourceApplicationContext>,
    device_key: &'a [u8; 32],
    entries: Vec<rua_sync::inbound_inbox::InboxEntry>,
    inbox: &'a rua_sync::inbound_inbox::InboundInbox,
    data_dir: &'a Path,
    local_device_id: &'a str,
    v4_log: Option<&'a rua_sync::opaque_replica_log::OpaqueReplicaLog>,
    active_blob_download: Option<&'a ActiveBlobDownload>,
    v4_registry: &'a rua_sync::sync_registry::SyncRegistry,
    merge_store: &'a rua_sync::merge_store::MergeStore,
    project_catalog: Arc<rua_project::ProjectCatalog>,
    security: Arc<rua_security::SecurityModule>,
    registry: Option<Arc<dyn rua_workspace::WorkspaceRegistry>>,
    conversation_input_intake:
        Option<crate::services::conversation_input_intake::ConversationInputIntake>,
    active_stream_broadcast_hub:
        Option<Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>>,
    chat_activity_wake_hub: Option<Arc<crate::services::chat_activity_wake::ChatActivityWakeHub>>,
    device_session_manager: Option<rua_device_network::DeviceSessionManager>,
    sync_stage_recorder: Arc<rua_sync::stage_diagnostics::SyncStageRecorder>,
}

#[derive(Default)]
struct InboundBatchResult {
    applied: usize,
    retryable: usize,
    failed: usize,
    dormant: usize,
    discarded: usize,
}

struct ReadableInboundChange {
    entry: rua_sync::inbound_inbox::InboxEntry,
    changes: Vec<rua_sync::opaque_replica_log::SyncChange>,
    read_diagnostics: rua_sync::opaque_replica_log::ReplicaLogReadDiagnostics,
    stage_start: rua_sync::stage_diagnostics::InboundStageStart,
    read_started: Instant,
}

struct InboundApplyContext<'a> {
    resource_application: &'a Option<ResourceApplicationContext>,
    device_key: &'a [u8; 32],
    inbox: &'a rua_sync::inbound_inbox::InboundInbox,
    data_dir: &'a Path,
    local_device_id: &'a str,
    v4_registry: &'a rua_sync::sync_registry::SyncRegistry,
    merge_store: &'a rua_sync::merge_store::MergeStore,
    project_catalog: &'a Arc<rua_project::ProjectCatalog>,
    security: &'a rua_security::SecurityModule,
    registry: &'a Option<Arc<dyn rua_workspace::WorkspaceRegistry>>,
    conversation_input_intake:
        &'a Option<crate::services::conversation_input_intake::ConversationInputIntake>,
    active_stream_broadcast_hub:
        &'a Option<Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>>,
    chat_activity_wake_hub:
        &'a Option<Arc<crate::services::chat_activity_wake::ChatActivityWakeHub>>,
    device_session_manager: &'a Option<rua_device_network::DeviceSessionManager>,
    sync_stage_recorder: &'a rua_sync::stage_diagnostics::SyncStageRecorder,
}

struct PendingInboundStageSample {
    correlation_id: String,
    source_commit_observed_at_ms: Option<u64>,
    domain_apply_completed_at_ms: u64,
    cache_refresh_completed_at_ms: u64,
    domain_apply_ms: u64,
    cache_refresh_ms: u64,
}

async fn drain_inbound_batch(args: InboundBatchContext<'_>) -> InboundBatchResult {
    let InboundBatchContext {
        resource_application,
        device_key,
        entries,
        inbox,
        data_dir,
        local_device_id,
        v4_log,
        active_blob_download,
        v4_registry,
        merge_store,
        project_catalog,
        security,
        registry,
        conversation_input_intake,
        active_stream_broadcast_hub,
        chat_activity_wake_hub,
        device_session_manager,
        sync_stage_recorder,
    } = args;
    let Some(log) = v4_log else {
        let mut result = InboundBatchResult::default();
        for entry in entries {
            record_limited_retry(
                &mut result,
                mark_inbound_retryable(
                    inbox,
                    &entry,
                    "v4 change not readable yet",
                    rua_sync::inbound_inbox::RetryExhaustion::Dormant,
                ),
            );
        }
        return result;
    };

    let recorder_for_reads = Arc::clone(&sync_stage_recorder);
    let read_results =
        stream::iter(entries.into_iter().map(move |entry| {
            let recorder = Arc::clone(&recorder_for_reads);
            async move {
                read_inbound_frame_fast(log, active_blob_download, recorder.as_ref(), entry).await
            }
        }))
        .buffer_unordered(MAX_INBOUND_DRAIN_READ_CONCURRENCY);
    tokio::pin!(read_results);

    let apply_context = InboundApplyContext {
        resource_application: &resource_application,
        device_key,
        inbox,
        data_dir,
        local_device_id,
        v4_registry,
        merge_store,
        project_catalog: &project_catalog,
        security: security.as_ref(),
        registry: &registry,
        conversation_input_intake: &conversation_input_intake,
        active_stream_broadcast_hub: &active_stream_broadcast_hub,
        chat_activity_wake_hub: &chat_activity_wake_hub,
        device_session_manager: &device_session_manager,
        sync_stage_recorder: sync_stage_recorder.as_ref(),
    };
    let mut result = InboundBatchResult::default();
    while let Some(read) = read_results.next().await {
        match read {
            InboundReadResult::Readable(change) => {
                apply_readable_inbound_change(*change, &apply_context, &mut result).await;
            }
            InboundReadResult::Missing(entry) => match inbox.discard(entry.author, &entry.key) {
                Ok(true) => {
                    apply_context
                        .sync_stage_recorder
                        .clear_inbound_entry(entry.author, &entry.key);
                    result.discarded += 1;
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "[liveactor] discard obsolete inbound inbox entry"
                    );
                }
            },
            InboundReadResult::Retryable { entry, error } => {
                apply_context.sync_stage_recorder.record_failure(
                    rua_sync::stage_diagnostics::SyncStageFailure {
                        correlation_id: None,
                        entry_fingerprint: Some(rua_sync::stage_diagnostics::entry_fingerprint(
                            entry.author,
                            &entry.key,
                        )),
                        stage: inbound_read_failure_stage(&error),
                        cause: "read_failed".to_string(),
                        observed_at_ms: rua_sync::stage_diagnostics::wall_now_ms(),
                    },
                );
                record_limited_retry(
                    &mut result,
                    mark_inbound_retryable(
                        inbox,
                        &entry,
                        format!("v4 read/decrypt failed: {error}"),
                        rua_sync::inbound_inbox::RetryExhaustion::Failed,
                    ),
                );
            }
            InboundReadResult::Dormant { entry, error } => {
                apply_context.sync_stage_recorder.record_failure(
                    rua_sync::stage_diagnostics::SyncStageFailure {
                        correlation_id: None,
                        entry_fingerprint: Some(rua_sync::stage_diagnostics::entry_fingerprint(
                            entry.author,
                            &entry.key,
                        )),
                        stage: rua_sync::stage_diagnostics::SyncFailureStage::Content,
                        cause: "source_content_unavailable".to_string(),
                        observed_at_ms: rua_sync::stage_diagnostics::wall_now_ms(),
                    },
                );
                record_limited_retry(
                    &mut result,
                    mark_inbound_retryable(
                        inbox,
                        &entry,
                        format!("v4 source content unavailable: {error}"),
                        rua_sync::inbound_inbox::RetryExhaustion::Dormant,
                    ),
                );
            }
        }
    }

    result
}

async fn apply_readable_inbound_change(
    item: ReadableInboundChange,
    context: &InboundApplyContext<'_>,
    result: &mut InboundBatchResult,
) {
    let entry = item.entry;
    let mut changes = item.changes;
    changes.sort_by_key(|change| domain_apply_priority(&change.domain));
    let mut commits = Vec::new();
    let mut stage_samples = Vec::new();
    for change in changes {
        if change.device_id == context.local_device_id {
            continue;
        }
        let local_chat_deletion = if change.domain == rua_sync::chat_adapter::CHAT_DOMAIN
            && change.kind == rua_sync::chat_adapter::CHAT_KIND_INDEX_DELETE
        {
            match rua_sync::chat_adapter::parse_chat_record_id(&change.record_id) {
                Ok(address) => {
                    match crate::services::chat_deletion::LocalChatDeletionPlan::prepare(
                        &context.data_dir,
                        address,
                    )
                    .await
                    {
                        Ok(plan) => Some(plan),
                        Err(error) => {
                            record_limited_retry(
                                result,
                                mark_inbound_retryable(
                                    context.inbox,
                                    &entry,
                                    format!("prepare local Chat delete failed: {error}"),
                                    rua_sync::inbound_inbox::RetryExhaustion::Failed,
                                ),
                            );
                            return;
                        }
                    }
                }
                Err(error) => {
                    record_limited_retry(
                        result,
                        mark_inbound_retryable(
                            context.inbox,
                            &entry,
                            format!("parse Chat delete identity failed: {error}"),
                            rua_sync::inbound_inbox::RetryExhaustion::Failed,
                        ),
                    );
                    return;
                }
            }
        } else {
            None
        };
        let domain_apply_started = Instant::now();
        match context
            .v4_registry
            .prepare_change(&change, context.merge_store)
            .await
        {
            Ok(prepared) => {
                let domain_apply_ms =
                    rua_sync::stage_diagnostics::millis(domain_apply_started.elapsed());
                let domain_apply_completed_at_ms = rua_sync::stage_diagnostics::wall_now_ms();
                let apply_result = prepared.result;
                let pending_commit = prepared.commit;
                let cache_refresh_started = Instant::now();
                if apply_result.domain == rua_sync::device_list_adapter::DEVICE_LIST_DOMAIN
                    && matches!(
                        apply_result.status,
                        rua_sync::sync_registry::RegistryApplyStatus::Applied
                            | rua_sync::sync_registry::RegistryApplyStatus::AlreadyApplied
                    )
                {
                    if let Some(session_manager) = context.device_session_manager {
                        match session_manager.reconcile_peer_directory() {
                            Ok(invalidated) if !invalidated.is_empty() => {
                                tracing::info!(
                                    invalidated = ?invalidated,
                                    "reconciled Device Sessions after Device List apply"
                                );
                            }
                            Ok(_) => {}
                            Err(error) => {
                                tracing::warn!(
                                    error = %error,
                                    "failed to reconcile Device Sessions after Device List apply"
                                );
                            }
                        }
                    }
                }
                if apply_result.domain == rua_sync::agent_security_adapter::AGENT_SECURITY_DOMAIN
                    && matches!(
                        apply_result.status,
                        rua_sync::sync_registry::RegistryApplyStatus::Applied
                    )
                {
                    context.security.invalidate(&apply_result.record_id);
                }
                if apply_result.domain == rua_sync::skill_adapter::SKILL_BLOB_DOMAIN
                    && matches!(
                        apply_result.status,
                        rua_sync::sync_registry::RegistryApplyStatus::Applied
                    )
                {
                    if let (Some(registry), Some(target)) = (
                        context.registry.as_ref(),
                        apply_result.record_id.strip_prefix("workspace/"),
                    ) {
                        if let Some((agent_id, _)) = target.split_once('/') {
                            if let Some(runtime) = registry.get(agent_id).await {
                                if let Some(react) = runtime.react_runtime().await {
                                    react.bump_prepared_generation();
                                }
                            }
                        }
                    }
                }
                if apply_agent_resources_after_inbound_config_inner(
                    context.registry,
                    context.conversation_input_intake,
                    context.device_key,
                    context.resource_application.as_ref(),
                    &change,
                    &apply_result,
                )
                .await
                .is_err()
                {
                    context.sync_stage_recorder.record_failure(
                        rua_sync::stage_diagnostics::SyncStageFailure {
                            correlation_id: Some(change.op_id.clone()),
                            entry_fingerprint: Some(
                                rua_sync::stage_diagnostics::entry_fingerprint(
                                    entry.author,
                                    &entry.key,
                                ),
                            ),
                            stage: rua_sync::stage_diagnostics::SyncFailureStage::CacheRefresh,
                            cause: "agent_resource_application_failed".to_string(),
                            observed_at_ms: rua_sync::stage_diagnostics::wall_now_ms(),
                        },
                    );
                }
                if let Err(error) = cleanup_project_after_inbound_delete(
                    context.conversation_input_intake,
                    context.project_catalog,
                    &change,
                    &apply_result,
                )
                .await
                {
                    context.sync_stage_recorder.record_failure(
                        rua_sync::stage_diagnostics::SyncStageFailure {
                            correlation_id: Some(change.op_id.clone()),
                            entry_fingerprint: Some(
                                rua_sync::stage_diagnostics::entry_fingerprint(
                                    entry.author,
                                    &entry.key,
                                ),
                            ),
                            stage: rua_sync::stage_diagnostics::SyncFailureStage::CacheRefresh,
                            cause: "project_cleanup_failed".to_string(),
                            observed_at_ms: rua_sync::stage_diagnostics::wall_now_ms(),
                        },
                    );
                    record_limited_retry(
                        result,
                        mark_inbound_retryable(
                            context.inbox,
                            &entry,
                            format!("Project tombstone cleanup failed: {error}"),
                            rua_sync::inbound_inbox::RetryExhaustion::Failed,
                        ),
                    );
                    return;
                }
                if let Some(plan) = local_chat_deletion {
                    if let Some(intake) = context.conversation_input_intake.as_ref() {
                        if let Err(error) = intake.apply_inbound_chat_delete(plan.address()).await {
                            record_limited_retry(
                                result,
                                mark_inbound_retryable(
                                    context.inbox,
                                    &entry,
                                    format!("stop local Chat activity failed: {error}"),
                                    rua_sync::inbound_inbox::RetryExhaustion::Failed,
                                ),
                            );
                            return;
                        }
                    }
                    if let Err(error) = plan.apply().await {
                        record_limited_retry(
                            result,
                            mark_inbound_retryable(
                                context.inbox,
                                &entry,
                                format!("apply local Chat delete failed: {error}"),
                                rua_sync::inbound_inbox::RetryExhaustion::Failed,
                            ),
                        );
                        return;
                    }
                }
                if let Some(commit) = pending_commit {
                    commits.push(commit);
                }
                publish_evergreen_chat_sync_wake(
                    context.active_stream_broadcast_hub,
                    &apply_result,
                )
                .await;
                publish_chat_placement_sync_wake(
                    context.chat_activity_wake_hub,
                    &change,
                    &apply_result,
                )
                .await;
                let cache_refresh_ms =
                    rua_sync::stage_diagnostics::millis(cache_refresh_started.elapsed());
                let cache_refresh_completed_at_ms = rua_sync::stage_diagnostics::wall_now_ms();
                stage_samples.push(PendingInboundStageSample {
                    correlation_id: change.op_id.clone(),
                    source_commit_observed_at_ms: change
                        .source_timing
                        .as_ref()
                        .map(|timing| timing.commit_observed_at_ms),
                    domain_apply_completed_at_ms,
                    cache_refresh_completed_at_ms,
                    domain_apply_ms,
                    cache_refresh_ms,
                });
                tracing::debug!(
                    domain = %apply_result.domain,
                    record_id = %apply_result.record_id,
                    status = ?apply_result.status,
                    "[liveactor] applied v4 change"
                );
            }
            Err(e) => {
                let permanent_error = e.is_permanent();
                context.sync_stage_recorder.record_failure(
                    rua_sync::stage_diagnostics::SyncStageFailure {
                        correlation_id: Some(change.op_id.clone()),
                        entry_fingerprint: Some(rua_sync::stage_diagnostics::entry_fingerprint(
                            entry.author,
                            &entry.key,
                        )),
                        stage: rua_sync::stage_diagnostics::SyncFailureStage::DomainApply,
                        cause: "domain_apply_failed".to_string(),
                        observed_at_ms: rua_sync::stage_diagnostics::wall_now_ms(),
                    },
                );
                if permanent_error {
                    match context.inbox.mark_discarded(
                        entry.author,
                        &entry.key,
                        format!("v4 invalid payload discarded: {e}"),
                    ) {
                        Ok(()) => {
                            context
                                .sync_stage_recorder
                                .clear_inbound_entry(entry.author, &entry.key);
                            result.discarded += 1;
                        }
                        Err(mark_error) => {
                            tracing::warn!(
                                error = %mark_error,
                                "[liveactor] mark invalid inbound payload discarded"
                            );
                            record_limited_retry(
                                result,
                                mark_inbound_retryable(
                                    context.inbox,
                                    &entry,
                                    format!("mark invalid payload discarded failed: {mark_error}"),
                                    rua_sync::inbound_inbox::RetryExhaustion::Failed,
                                ),
                            );
                        }
                    }
                    return;
                }
                record_limited_retry(
                    result,
                    mark_inbound_retryable(
                        context.inbox,
                        &entry,
                        format!("v4 apply failed: {e}"),
                        rua_sync::inbound_inbox::RetryExhaustion::Failed,
                    ),
                );
                return;
            }
        }
    }

    let canonical_visibility_started = Instant::now();
    match context.merge_store.commit_inbound_apply(
        entry.author,
        &entry.key,
        rua_sync::inbound_inbox::now_ms(),
        &commits,
    ) {
        Ok(_) => {
            let canonical_visible_at_ms = rua_sync::stage_diagnostics::wall_now_ms();
            let canonical_visibility_ms =
                rua_sync::stage_diagnostics::millis(canonical_visibility_started.elapsed());
            for sample in stage_samples {
                context.sync_stage_recorder.record_inbound(
                    rua_sync::stage_diagnostics::SyncInboundStageSample {
                        correlation_id: sample.correlation_id,
                        source_commit_observed_at_ms: sample.source_commit_observed_at_ms,
                        transport_arrival_at_ms: item.stage_start.transport_arrival_at_ms,
                        inbox_recorded_at_ms: item.stage_start.inbox_recorded_at_ms,
                        content_ready_at_ms: item.read_diagnostics.content_ready_at_ms,
                        decrypt_read_completed_at_ms: item
                            .read_diagnostics
                            .decrypt_read_completed_at_ms,
                        domain_apply_completed_at_ms: sample.domain_apply_completed_at_ms,
                        cache_refresh_completed_at_ms: sample.cache_refresh_completed_at_ms,
                        canonical_visible_at_ms,
                        transport_to_inbox_ms: item.stage_start.transport_to_inbox_ms(),
                        inbox_wait_ms: item.stage_start.inbox_wait_ms(item.read_started),
                        content_wait_ms: item.read_diagnostics.content_wait_ms,
                        content_read_ms: item.read_diagnostics.content_read_ms,
                        decrypt_ms: item.read_diagnostics.decrypt_ms,
                        domain_apply_ms: sample.domain_apply_ms,
                        cache_refresh_ms: sample.cache_refresh_ms,
                        canonical_visibility_ms,
                        receiver_pipeline_ms: item.stage_start.receiver_pipeline_ms(),
                        end_to_end_wall_ms: sample
                            .source_commit_observed_at_ms
                            .map(|started| canonical_visible_at_ms.saturating_sub(started)),
                        used_peer_download: item.read_diagnostics.used_peer_download,
                        status: rua_sync::stage_diagnostics::SyncStageStatus::Success,
                    },
                );
            }
            context
                .sync_stage_recorder
                .clear_inbound_entry(entry.author, &entry.key);
            result.applied += 1;
        }
        Err(error) => {
            context.sync_stage_recorder.record_failure(
                rua_sync::stage_diagnostics::SyncStageFailure {
                    correlation_id: stage_samples
                        .first()
                        .map(|sample| sample.correlation_id.clone()),
                    entry_fingerprint: Some(rua_sync::stage_diagnostics::entry_fingerprint(
                        entry.author,
                        &entry.key,
                    )),
                    stage: rua_sync::stage_diagnostics::SyncFailureStage::CanonicalVisibility,
                    cause: "apply_commit_failed".to_string(),
                    observed_at_ms: rua_sync::stage_diagnostics::wall_now_ms(),
                },
            );
            record_limited_retry(
                result,
                mark_inbound_retryable(
                    context.inbox,
                    &entry,
                    format!("commit inbound apply bookkeeping: {error}"),
                    rua_sync::inbound_inbox::RetryExhaustion::Failed,
                ),
            );
        }
    }
}

async fn cleanup_project_after_inbound_delete(
    conversation_input_intake: &Option<
        crate::services::conversation_input_intake::ConversationInputIntake,
    >,
    project_catalog: &Arc<rua_project::ProjectCatalog>,
    change: &rua_sync::opaque_replica_log::SyncChange,
    apply_result: &rua_sync::sync_registry::RegistryApplyResult,
) -> Result<(), String> {
    if change.domain != rua_sync::project_adapter::PROJECT_CATALOG_DOMAIN
        || change.kind != rua_sync::project_adapter::PROJECT_DELETE_KIND
        || !matches!(
            apply_result.status,
            rua_sync::sync_registry::RegistryApplyStatus::Applied
                | rua_sync::sync_registry::RegistryApplyStatus::AlreadyApplied
        )
    {
        return Ok(());
    }
    let project_id = rua_core::ProjectId::new(apply_result.record_id.clone());
    let Some(intake) = conversation_input_intake else {
        return Err(format!(
            "inbound Project tombstone for {project_id} has no Project cleanup runtime"
        ));
    };
    let (scope_root, addresses) = intake.project_delete_scope(&project_id).await?;
    intake
        .cleanup_permanently_deleted_project(&project_id, &scope_root, &addresses)
        .await?;
    project_catalog
        .finalize_permanent_delete(&project_id)
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn publish_chat_placement_sync_wake(
    hub: &Option<Arc<crate::services::chat_activity_wake::ChatActivityWakeHub>>,
    change: &rua_sync::opaque_replica_log::SyncChange,
    apply_result: &rua_sync::sync_registry::RegistryApplyResult,
) {
    if apply_result.domain != rua_sync::evergreen_chat_adapter::EVERGREEN_CHAT_DOMAIN
        || !matches!(
            change.kind.as_str(),
            rua_sync::evergreen_chat_adapter::EVERGREEN_CHAT_KIND_FILE_UPSERT
                | rua_sync::evergreen_chat_adapter::EVERGREEN_CHAT_KIND_FILE_SNAPSHOT
        )
        || !matches!(
            apply_result.status,
            rua_sync::sync_registry::RegistryApplyStatus::Applied
        )
    {
        return;
    }
    let Some(hub) = hub else {
        return;
    };
    let Ok((address, relative)) =
        rua_sync::evergreen_chat_adapter::parse_file_record_id(&apply_result.record_id)
    else {
        return;
    };
    if relative != "session" && relative != "session.json" {
        return;
    }
    let Ok(next) =
        crate::services::conversation_authority_placement::parse_conversation_authority_placement(
            &change.payload,
        )
    else {
        return;
    };
    hub.publish(
        crate::services::chat_activity_wake::ChatLiveWake::placement_changed(
            address,
            next.device_id,
            next.authority_epoch,
            next.base_manifest_hash,
        ),
    );
}

async fn apply_agent_resources_after_inbound_config_inner(
    registry: &Option<Arc<dyn rua_workspace::WorkspaceRegistry>>,
    conversation_input_intake: &Option<
        crate::services::conversation_input_intake::ConversationInputIntake,
    >,
    device_key: &[u8; 32],
    resources: Option<&ResourceApplicationContext>,
    change: &rua_sync::opaque_replica_log::SyncChange,
    apply_result: &rua_sync::sync_registry::RegistryApplyResult,
) -> Result<(), String> {
    if change.domain != rua_sync::agent_adapter::AGENT_CONFIG_DOMAIN
        || !matches!(
            apply_result.status,
            rua_sync::sync_registry::RegistryApplyStatus::Applied
        )
    {
        return Ok(());
    }

    let Some(registry) = registry else {
        return Ok(());
    };
    if change.kind == rua_sync::agent_adapter::AGENT_CONFIG_KIND_DELETE {
        if let Some(intake) = conversation_input_intake {
            if let Err(error) = intake
                .cleanup_deleted_project_agent(&apply_result.record_id)
                .await
            {
                tracing::error!(
                    agent_id = %apply_result.record_id,
                    error = %error,
                    "[liveactor] failed to stop Project work after inbound Agent deletion"
                );
                return Err(error);
            }
        }
        if registry.get(&apply_result.record_id).await.is_some() {
            if let Err(error) = registry.remove(&apply_result.record_id).await {
                tracing::error!(
                    agent_id = %apply_result.record_id,
                    error = %error,
                    "[liveactor] failed to remove Agent runtime after inbound deletion"
                );
                return Err(error.to_string());
            }
        }
        return Ok(());
    }
    if registry.get(&apply_result.record_id).await.is_none() {
        return Ok(());
    }

    let Some(runtime) = registry.get(&apply_result.record_id).await else {
        return Ok(());
    };
    let agent_id = &apply_result.record_id;
    let cells = rua_mcp::AgentMcpCells::new(
        Arc::new(cell_store::FileSystemCellStore::new()),
        runtime.rua_root().join("workspaces"),
    );
    let mcp = crate::mcp_reload::apply_loaded(&runtime, &cells, device_key, agent_id).await;
    if let Some(resources) = resources {
        if mcp.changed {
            resources.mcp_cache.invalidate_agent(agent_id).await;
        }
    }
    let channels = runtime
        .apply_channels(|| async {
            runtime
                .ctx()
                .agent_cells
                .section::<serde_json::Value>(agent_id, "channels")
                .read()
                .await
                .map(|value| value.unwrap_or_else(|| serde_json::json!({})))
                .map_err(|error| rua_workspace::WorkspaceError::config_invalid(error.to_string()))
        })
        .await;
    let voice = match resources {
        Some(resources) if agent_id == resources.voice_context.voice.agent_id() => resources
            .voice
            .apply(&resources.voice_context)
            .await
            .map_err(|error| error.to_string()),
        _ => Ok(()),
    };
    let embedding = runtime.apply_embedding_configuration().await;
    let maintenance = runtime.apply_maintenance_configuration().await;
    if mcp.status == "failed"
        || channels.is_err()
        || embedding.is_err()
        || maintenance.is_err()
        || voice.is_err()
    {
        tracing::warn!(agent_id, "Inbound Agent configuration saved, but one or more runtime resources could not be applied");
        return Err(
            "Inbound Agent configuration saved, but resource application failed".to_string(),
        );
    }
    Ok(())
}

async fn publish_evergreen_chat_sync_wake(
    hub: &Option<Arc<crate::services::active_stream_broadcast::ActiveStreamBroadcastHub>>,
    apply_result: &rua_sync::sync_registry::RegistryApplyResult,
) {
    if apply_result.domain != rua_sync::evergreen_chat_adapter::EVERGREEN_CHAT_DOMAIN
        || !matches!(
            apply_result.status,
            rua_sync::sync_registry::RegistryApplyStatus::Applied
        )
    {
        return;
    }
    let Some(hub) = hub else {
        return;
    };
    let Ok((address, _relative)) =
        rua_sync::evergreen_chat_adapter::parse_file_record_id(&apply_result.record_id)
    else {
        return;
    };
    let stream_id = format!("sync-wake-{address}-{}", uuid::Uuid::new_v4());
    let payload = match address.agent_id() {
        Some(agent_id) => serde_json::json!({
            "type": "chat_sync_wake",
            "agent_id": agent_id,
            "chat_id": address.chat_id(),
        }),
        None => serde_json::json!({
            "type": "chat_sync_wake",
            "address": address,
        }),
    }
    .to_string();
    hub.publish_event(&stream_id, &address, payload).await;
    hub.finish(&stream_id, &address).await;
}

enum InboundReadResult {
    Readable(Box<ReadableInboundChange>),
    Missing(rua_sync::inbound_inbox::InboxEntry),
    Retryable {
        entry: rua_sync::inbound_inbox::InboxEntry,
        error: String,
    },
    Dormant {
        entry: rua_sync::inbound_inbox::InboxEntry,
        error: String,
    },
}

async fn read_inbound_frame_fast(
    log: &rua_sync::opaque_replica_log::OpaqueReplicaLog,
    active_blob_download: Option<&ActiveBlobDownload>,
    sync_stage_recorder: &rua_sync::stage_diagnostics::SyncStageRecorder,
    entry: rua_sync::inbound_inbox::InboxEntry,
) -> InboundReadResult {
    let read_started = Instant::now();
    let stage_start =
        sync_stage_recorder.inbound_stage_start(entry.author, &entry.key, entry.seen_at_ms);
    let should_try_active_download = entry.attempts < MAX_AUTO_APPLY_ATTEMPTS;

    let should_use_active_download = should_try_active_download && active_blob_download.is_some();
    let read_future = async {
        if let Some(download) = active_blob_download.filter(|_| should_try_active_download) {
            if fetch_inbound_blob_via_device_session(download, &entry.content_hash).await {
                return log
                    .read_frame_by_content_hash_with_diagnostics(&entry.content_hash)
                    .await
                    .map(Some);
            }
            return log
                .read_frame_by_content_hash_with_peer_download_diagnostics(
                    &entry.content_hash,
                    &download.downloader,
                    download.providers.clone(),
                )
                .await
                .map(Some);
        }
        log.read_frame_by_content_hash_with_diagnostics(&entry.content_hash)
            .await
            .map(Some)
    };

    let timeout = if should_use_active_download {
        INBOUND_ACTIVE_DOWNLOAD_TIMEOUT
    } else {
        INBOUND_READ_TIMEOUT
    };

    match tokio::time::timeout(timeout, read_future).await {
        Ok(Ok(Some((frame, read_diagnostics)))) => {
            InboundReadResult::Readable(Box::new(ReadableInboundChange {
                entry,
                changes: frame.into_changes(),
                read_diagnostics,
                stage_start,
                read_started,
            }))
        }
        Ok(Ok(None)) => InboundReadResult::Missing(entry),
        Ok(Err(error @ rua_sync::SyncError::ReplicaLogContentUnavailable(_)))
            if entry.attempts.saturating_add(1) >= MAX_AUTO_APPLY_ATTEMPTS =>
        {
            InboundReadResult::Dormant {
                entry,
                error: error.to_string(),
            }
        }
        Ok(Err(error)) => InboundReadResult::Retryable {
            entry,
            error: error.to_string(),
        },
        Err(_) => InboundReadResult::Retryable {
            entry,
            error: format!("v4 read/decrypt timed out after {:?}", timeout),
        },
    }
}

fn inbound_read_failure_stage(error: &str) -> rua_sync::stage_diagnostics::SyncFailureStage {
    if error.contains("content")
        || error.contains("blob")
        || error.contains("download")
        || error.contains("timed out")
    {
        rua_sync::stage_diagnostics::SyncFailureStage::Content
    } else {
        rua_sync::stage_diagnostics::SyncFailureStage::DecryptRead
    }
}

fn mark_inbound_retryable(
    inbox: &rua_sync::inbound_inbox::InboundInbox,
    entry: &rua_sync::inbound_inbox::InboxEntry,
    error: impl Into<String>,
    exhaustion: rua_sync::inbound_inbox::RetryExhaustion,
) -> Option<rua_sync::inbound_inbox::InboxState> {
    let retry_after = rua_sync::inbound_inbox::next_retry_after_ms(
        entry.attempts,
        rua_sync::inbound_inbox::now_ms(),
    );
    match inbox.mark_retryable_limited(
        entry.author,
        &entry.key,
        error,
        retry_after,
        MAX_AUTO_APPLY_ATTEMPTS,
        exhaustion,
    ) {
        Ok(state) => Some(state),
        Err(mark_err) => {
            tracing::warn!(error = %mark_err, "[liveactor] mark inbound retryable");
            None
        }
    }
}

fn record_limited_retry(
    result: &mut InboundBatchResult,
    state: Option<rua_sync::inbound_inbox::InboxState>,
) {
    match state {
        Some(rua_sync::inbound_inbox::InboxState::Retryable) => result.retryable += 1,
        Some(rua_sync::inbound_inbox::InboxState::Failed) => result.failed += 1,
        Some(rua_sync::inbound_inbox::InboxState::Dormant) => result.dormant += 1,
        _ => {}
    }
}

fn domain_apply_priority(domain: &str) -> u8 {
    match domain {
        rua_sync::provider_adapter::PROVIDER_SECRET_DOMAIN
        | rua_sync::root_config_adapter::ROOT_CONFIG_DOMAIN
        | rua_sync::agent_adapter::AGENT_CONFIG_DOMAIN
        | rua_sync::agent_security_adapter::AGENT_SECURITY_DOMAIN
        | rua_sync::chat_adapter::CHAT_DOMAIN
        | rua_sync::device_list_adapter::DEVICE_LIST_DOMAIN
        | rua_sync::project_adapter::PROJECT_CATALOG_DOMAIN
        | rua_sync::project_sidebar_adapter::PROJECT_SIDEBAR_DOMAIN
        | rua_sync::prompt_adapter::PROMPT_DOMAIN
        | rua_sync::shared_mcp_adapter::SHARED_MCP_DOMAIN => 0,
        rua_sync::agent_template_adapter::AGENT_TEMPLATE_DOMAIN
        | rua_sync::evergreen_chat_adapter::EVERGREEN_CHAT_DOMAIN
        | rua_sync::agent_prompt_file_adapter::AGENT_PROMPT_FILE_DOMAIN => 1,
        rua_sync::skill_adapter::SKILL_BLOB_DOMAIN
        | rua_sync::media_adapter::MEDIA_DOMAIN
        | rua_sync::structured_memory_adapter::STRUCTURED_MEMORY_DOMAIN => 2,
        _ => 3,
    }
}

#[derive(Clone)]
struct ActiveBlobDownload {
    downloader: rua_sync::sync_blob_store::StatefulBlobDownloader,
    blobs_store: rua_sync::endpoint::BlobsApiStore,
    session_manager: rua_device_network::DeviceSessionManager,
    providers: Vec<iroh::EndpointId>,
}

#[derive(Default, serde::Deserialize)]
struct BlobPeerHealthState {
    #[serde(default)]
    peers: std::collections::HashMap<String, BlobPeerHealthEntry>,
}

#[derive(Clone, Default, serde::Deserialize)]
struct BlobPeerHealthEntry {
    #[serde(default)]
    last_success_ms: Option<u64>,
    #[serde(default)]
    consecutive_failures: u32,
}

async fn fetch_inbound_blob_via_device_session(download: &ActiveBlobDownload, hash: &str) -> bool {
    let Ok(hash) = hash.parse::<rua_sync::endpoint::BlobHash>() else {
        return false;
    };
    for provider in &download.providers {
        let target_device_id = hex::encode(&provider.as_bytes()[..8]);
        match crate::services::sync_transfer::read_sync_blob_via_session(
            &download.session_manager,
            &target_device_id,
            hash,
        )
        .await
        {
            Ok(bytes) => {
                match rua_sync::opaque_replica_log::store_encrypted_blob_bytes(
                    &download.blobs_store,
                    bytes::Bytes::from(bytes),
                )
                .await
                {
                    Ok(actual) if actual == hash => return true,
                    Ok(actual) => tracing::warn!(
                        expected = %hash,
                        %actual,
                        "shared Device Session returned mismatched Sync Blob"
                    ),
                    Err(error) => tracing::debug!(
                        peer = %provider.fmt_short(),
                        %error,
                        "store shared-session Sync Blob failed"
                    ),
                }
            }
            Err(error) => tracing::debug!(
                peer = %provider.fmt_short(),
                %error,
                "shared Device Session Sync Blob read failed; falling back to Iroh Blob transfer"
            ),
        }
    }
    false
}

async fn build_active_blob_download(
    sync_node: &Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    data_dir: &Path,
) -> Option<ActiveBlobDownload> {
    let (downloader, blobs_store, self_pubkey, network_runtime) = {
        let node = sync_node.lock().await.as_ref().cloned()?;
        (
            node.blob_downloader().clone(),
            node.blobs_store().clone(),
            node.node_id(),
            node.network_runtime().clone(),
        )
    };

    let device_list_path = data_dir.join("sync").join("device_list.json");
    let device_list: rua_sync::device_list::DeviceList = std::fs::read_to_string(&device_list_path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    let self_pubkey_bytes = self_pubkey.as_bytes();
    let peer_health: BlobPeerHealthState =
        std::fs::read_to_string(data_dir.join("sync").join("peer_health.json"))
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
    let mut providers = device_list
        .devices
        .iter()
        .filter(|device| &device.public_key != self_pubkey_bytes)
        .filter_map(
            |device| match iroh::EndpointId::from_bytes(&device.public_key) {
                Ok(peer) => {
                    let peer_key = peer.to_string();
                    let health = peer_health
                        .peers
                        .get(&peer_key)
                        .cloned()
                        .unwrap_or_default();
                    Some((
                        peer,
                        health.last_success_ms.unwrap_or(0),
                        health.consecutive_failures,
                        device.last_seen_ms.max(device.added_at_ms),
                    ))
                }
                Err(e) => {
                    tracing::warn!(error = %e, "[sync-v4] invalid blob provider public key");
                    None
                }
            },
        )
        .collect::<Vec<_>>();
    providers.sort_by_key(
        |(_, last_success_ms, consecutive_failures, last_seen_or_added_ms)| {
            (
                std::cmp::Reverse(*last_success_ms),
                *consecutive_failures,
                std::cmp::Reverse(*last_seen_or_added_ms),
            )
        },
    );
    let providers = providers
        .into_iter()
        .map(|(peer, _, _, _)| peer)
        .collect::<Vec<_>>();

    if providers.is_empty() {
        return None;
    }
    for provider in &providers {
        if let Err(error) = network_runtime.resolve_peer_addr(*provider).await {
            tracing::debug!(
                peer = %provider.fmt_short(),
                %error,
                "[sync-v4] Blob provider address resolution failed"
            );
        }
    }

    Some(ActiveBlobDownload {
        downloader,
        blobs_store,
        session_manager: network_runtime.session_manager().clone(),
        providers,
    })
}
