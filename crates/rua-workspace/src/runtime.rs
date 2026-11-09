use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use mcp_manager::manager::McpManager;
use rua_agent::compaction::ConversationCompactor;
use rua_agent::context_manager::ContextManager;
use rua_agent::memory_manager::MemoryManager;
use rua_agent::runtime::{build_default_toolkit, ReactAgentRuntime};
use rua_channel::core::ChannelManager;
use rua_contract::agent::AgentRuntime;
use rua_security::SecurityModule;

use crate::context::WorkspaceContext;
use crate::error::WorkspaceError;
use crate::health::{ComponentState, WorkspaceHealth, WorkspaceHealthBuilder};
use crate::lifecycle::{
    AgentWork, AgentWorkAuthority, InitPhase, PlatformProfile, ShutdownPolicy, ShutdownReport,
    WorkKind, WorkUnit, WorkspaceLifecycleState,
};
use crate::root::WorkspaceRoot;
use crate::runner::{ActiveSessionTracker, SessionAcquireResult, SessionConcurrencyPolicy};
use crate::service_manager::{
    default_service_descriptors, platform_service_descriptors, ActiveTaskTracker,
    ServiceDescriptor, ServiceManager,
};

/// Bound on one authority observation or terminal confirmation.
///
/// The authority lives in the process composition layer and can block on its own
/// locks; without this, its stall would outlive every shutdown deadline.
const AUTHORITY_OBSERVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Ceiling for a `Graceful` drain, independent of the policy's own timeout.
const DRAIN_CEILING: std::time::Duration = std::time::Duration::from_secs(30);

/// Bound on one cancellation request.
const CANCELLATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Bound on confirming the maintenance loop exited.
const MAINTENANCE_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[path = "session_tracker.rs"]
mod session_tracker;
pub use session_tracker::SessionGuard;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub agent_id: String,

    pub agent_name: String,

    pub is_active: bool,

    pub lifecycle: WorkspaceLifecycleState,

    pub active_sessions: usize,
}

pub struct WorkspaceRuntime {
    maintenance_interval: std::sync::Mutex<Option<std::time::Duration>>,
    memory_embedder: std::sync::RwLock<Option<rua_agent::memory_manager::EmbedderSlot>>,
    applied_embedding: tokio::sync::RwLock<Option<rua_config::EmbeddingConfig>>,
    operation_gate: tokio::sync::Mutex<()>,
    applied_channels: tokio::sync::RwLock<serde_json::Map<String, serde_json::Value>>,
    pub agent_id: String,

    pub agent_name: String,

    pub root: WorkspaceRoot,

    pub profile: PlatformProfile,

    ctx: Arc<WorkspaceContext>,

    state: Arc<RwLock<WorkspaceLifecycleState>>,

    service_manager: Arc<ServiceManager>,

    task_tracker: Arc<ActiveTaskTracker>,

    session_tracker: Arc<ActiveSessionTracker>,

    active_sessions: Arc<AtomicUsize>,

    maintenance_handle: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,

    /// Cancels the maintenance loop so `stop` can confirm the task really exited
    /// instead of dropping the handle and hoping.
    maintenance_cancel: std::sync::Mutex<tokio_util::sync::CancellationToken>,

    /// The real owner of Conversation turns and Task attempts for this Agent.
    /// Absent for embedders that never dispatch such work; absence is reported,
    /// never assumed to mean "nothing outstanding".
    work_authority: std::sync::RwLock<Option<Arc<dyn AgentWorkAuthority>>>,

    maintenance_tick_count: Arc<AtomicUsize>,

    mcp_manager: Arc<std::sync::RwLock<Option<Arc<dyn McpManager>>>>,

    memory_manager: RwLock<Option<Arc<dyn MemoryManager>>>,

    context_manager: RwLock<Option<Arc<dyn ContextManager>>>,

    channel_manager: RwLock<Option<Arc<ChannelManager>>>,

    react_runtime: RwLock<Option<Arc<dyn AgentRuntime>>>,
    capability_resolver: RwLock<Option<rua_agent::runtime::EffectiveCapabilityResolver>>,

    task_toolkit: RwLock<Option<Arc<react_agent::AgentToolkit>>>,

    compactor: RwLock<Option<Arc<ConversationCompactor>>>,
}

impl WorkspaceRuntime {
    pub fn new(
        agent_id: impl Into<String>,
        agent_name: impl Into<String>,
        root: WorkspaceRoot,
        profile: PlatformProfile,
        ctx: Arc<WorkspaceContext>,
    ) -> Self {
        Self::with_session_policy(
            agent_id,
            agent_name,
            root,
            profile,
            ctx,
            SessionConcurrencyPolicy::default(),
        )
    }

    pub fn with_session_policy(
        agent_id: impl Into<String>,
        agent_name: impl Into<String>,
        root: WorkspaceRoot,
        profile: PlatformProfile,
        ctx: Arc<WorkspaceContext>,
        policy: SessionConcurrencyPolicy,
    ) -> Self {
        let mobile_disabled = ["mcp", "skills", "channels"];

        // Declared capabilities start `NotInitialized`, or `Disabled` when the
        // platform profile excludes them. Nothing is `Ready` before a real
        // resource has been assembled and started.
        let entries: Vec<(ServiceDescriptor, ComponentState)> = default_service_descriptors()
            .into_iter()
            .chain(platform_service_descriptors())
            .map(|descriptor| {
                let disabled = profile == PlatformProfile::Mobile
                    && mobile_disabled.contains(&descriptor.name.as_str());
                let state = if disabled {
                    ComponentState::Disabled
                } else {
                    ComponentState::NotInitialized
                };
                (descriptor, state)
            })
            .collect();

        let service_manager = Arc::new(ServiceManager::with_descriptors(entries));
        let task_tracker = Arc::new(ActiveTaskTracker::new());
        let session_tracker = Arc::new(ActiveSessionTracker::new(policy));

        Self {
            maintenance_interval: std::sync::Mutex::new(None),
            memory_embedder: std::sync::RwLock::new(None),
            applied_embedding: tokio::sync::RwLock::new(None),
            operation_gate: tokio::sync::Mutex::new(()),
            applied_channels: tokio::sync::RwLock::new(serde_json::Map::new()),
            agent_id: agent_id.into(),
            agent_name: agent_name.into(),
            root,
            profile,
            ctx,
            state: Arc::new(RwLock::new(WorkspaceLifecycleState::New)),
            service_manager,
            task_tracker,
            session_tracker,
            active_sessions: Arc::new(AtomicUsize::new(0)),
            maintenance_handle: std::sync::Mutex::new(None),
            maintenance_cancel: std::sync::Mutex::new(tokio_util::sync::CancellationToken::new()),
            work_authority: std::sync::RwLock::new(None),
            maintenance_tick_count: Arc::new(AtomicUsize::new(0)),
            mcp_manager: Arc::new(std::sync::RwLock::new(None)),
            memory_manager: RwLock::new(None),
            context_manager: RwLock::new(None),
            channel_manager: RwLock::new(None),
            react_runtime: RwLock::new(None),
            capability_resolver: RwLock::new(None),
            task_toolkit: RwLock::new(None),
            compactor: RwLock::new(None),
        }
    }

    pub async fn initialize_core(&self) -> Result<(), WorkspaceError> {
        let _operation = self.operation_gate.lock().await;
        self.initialize_core_inner().await
    }

    async fn initialize_core_inner(&self) -> Result<(), WorkspaceError> {
        let state = self.state.read().await.clone();
        if matches!(
            state,
            WorkspaceLifecycleState::Stopped
                | WorkspaceLifecycleState::Draining
                | WorkspaceLifecycleState::Failed
        ) {
            return Err(WorkspaceError::component_init_failed(
                "workspace",
                "Runtime is not available for initialization",
            ));
        }
        if state != WorkspaceLifecycleState::New {
            return Ok(());
        }

        tracing::info!(agent_id = %self.agent_id, "Initializing core components");

        if let Err(e) = self.root.ensure() {
            tracing::error!(agent_id = %self.agent_id, error = %e, "Failed to create agent dir");
            *self.state.write().await = WorkspaceLifecycleState::Failed;
            return Err(WorkspaceError::component_init_failed(
                "directories",
                e.to_string(),
            ));
        }

        if let Err(e) = self.service_manager.start_phase(InitPhase::Core).await {
            tracing::error!(agent_id = %self.agent_id, error = %e, "Core service start failed");
            *self.state.write().await = WorkspaceLifecycleState::Failed;
            return Err(e);
        }

        *self.state.write().await = WorkspaceLifecycleState::CoreReady;

        tracing::info!(agent_id = %self.agent_id, "Core components ready");
        Ok(())
    }

    pub async fn initialize_metadata(&self) -> Result<(), WorkspaceError> {
        let _operation = self.operation_gate.lock().await;
        self.initialize_metadata_inner().await
    }

    async fn initialize_metadata_inner(&self) -> Result<(), WorkspaceError> {
        let state = self.state.read().await;
        if *state != WorkspaceLifecycleState::CoreReady {
            if *state == WorkspaceLifecycleState::MetadataReady
                || *state == WorkspaceLifecycleState::RuntimeReady
            {
                return Ok(());
            }
            return Err(WorkspaceError::component_init_failed(
                "metadata",
                "Core not ready",
            ));
        }
        drop(state);

        tracing::info!(agent_id = %self.agent_id, "Initializing metadata components");

        // No metadata-phase resource is assembled by default; an embedding
        // composition root can attach one and it will start here.
        if let Err(e) = self.service_manager.start_phase(InitPhase::Metadata).await {
            tracing::error!(agent_id = %self.agent_id, error = %e, "Metadata service start failed");
        }

        *self.state.write().await = WorkspaceLifecycleState::MetadataReady;

        tracing::info!(agent_id = %self.agent_id, "Metadata components ready");
        Ok(())
    }

    pub async fn initialize_runtime(&self) -> Result<(), WorkspaceError> {
        let _operation = self.operation_gate.lock().await;
        self.initialize_runtime_inner().await
    }

    async fn initialize_runtime_inner(&self) -> Result<(), WorkspaceError> {
        let state = self.state.read().await;
        if *state != WorkspaceLifecycleState::MetadataReady {
            if *state == WorkspaceLifecycleState::RuntimeReady {
                return Ok(());
            }
            return Err(WorkspaceError::component_init_failed(
                "runtime",
                "Metadata not ready",
            ));
        }
        drop(state);

        tracing::info!(agent_id = %self.agent_id, "Initializing runtime components");

        self.init_memory_manager().await;

        if self.profile != PlatformProfile::Mobile {
            self.lazy_init_mcp().await;
        }

        self.lazy_init_react_runtime().await;

        self.set_compactor(Arc::clone(&self.ctx.compactor)).await;

        self.ensure_agent_service().await;

        // Start the resources assembled above; a failure is reported through the
        // component state of the failing capability, and the workspace keeps the
        // other independent resources running.
        if let Err(e) = self.service_manager.start_phase(InitPhase::Runtime).await {
            tracing::error!(agent_id = %self.agent_id, error = %e, "Runtime service start failed");
        }

        *self.state.write().await = WorkspaceLifecycleState::RuntimeReady;

        let interval = {
            let config =
                crate::config::LayeredConfig::for_workspace(&self.ctx.rua_root, &self.root);
            let hours = config
                .merged()
                .ok()
                .and_then(|v| v.get("maintenance_interval_hours")?.as_f64())
                .unwrap_or(6.0);
            if !hours.is_finite() || hours <= 0.0 {
                *self.state.write().await = WorkspaceLifecycleState::Failed;
                return Err(WorkspaceError::config_invalid(
                    "Maintenance interval must be positive and finite",
                ));
            }
            std::time::Duration::try_from_secs_f64(hours * 3600.0)
                .map_err(|error| WorkspaceError::config_invalid(error.to_string()))?
        };
        self.spawn_maintenance_task(interval);

        tracing::info!(agent_id = %self.agent_id, "Runtime components ready");
        Ok(())
    }

    pub async fn initialize_for_platform(&self) -> Result<(), WorkspaceError> {
        let _operation = self.operation_gate.lock().await;
        self.initialize_core_inner().await?;

        if self.profile.max_blocking_phase() >= InitPhase::Metadata {
            self.initialize_metadata_inner().await?;
        }

        if self.profile.max_blocking_phase() >= InitPhase::Runtime {
            self.initialize_runtime_inner().await?;
        }

        Ok(())
    }

    pub fn ctx(&self) -> &Arc<WorkspaceContext> {
        &self.ctx
    }

    pub async fn lifecycle_state(&self) -> WorkspaceLifecycleState {
        self.state.read().await.clone()
    }

    pub async fn can_accept_requests(&self) -> bool {
        self.state.read().await.can_accept_requests()
    }

    pub fn service_manager(&self) -> &ServiceManager {
        &self.service_manager
    }

    pub fn task_tracker(&self) -> &ActiveTaskTracker {
        &self.task_tracker
    }

    pub async fn acquire_session(
        &self,
        session_id: &str,
        request_id: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<SessionGuard, WorkspaceError> {
        let result = self
            .session_tracker
            .try_acquire(session_id, request_id, &self.agent_id, cancel)
            .await?;

        match result {
            SessionAcquireResult::Acquired
            | SessionAcquireResult::CancelledPrevious { .. }
            | SessionAcquireResult::Queued { .. } => {
                self.active_sessions
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(SessionGuard::new(
                    Arc::clone(&self.session_tracker),
                    Arc::clone(&self.active_sessions),
                    session_id,
                    request_id,
                ))
            }
            SessionAcquireResult::Rejected => Err(WorkspaceError::session_busy(format!(
                "Session {session_id} is busy"
            ))),
        }
    }

    pub fn session_tracker(&self) -> &Arc<ActiveSessionTracker> {
        &self.session_tracker
    }

    pub fn active_session_count(&self) -> usize {
        self.active_sessions
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn background_task_count(&self) -> usize {
        self.task_tracker.active_count()
    }

    pub fn has_active_work(&self) -> bool {
        self.active_session_count() > 0 || self.background_task_count() > 0
    }

    pub fn spawn_maintenance_task(&self, interval: std::time::Duration) {
        *self
            .maintenance_interval
            .lock()
            .expect("Maintenance interval poisoned") = Some(interval);
        let workspace_dir = self.workspace_dir();
        let sessions_dir = self.root.sessions_dir();
        let agent_id = self.agent_id.clone();
        let tick_count = Arc::clone(&self.maintenance_tick_count);

        let retention_days = std::time::Duration::from_secs(
            Self::load_retention_days(&workspace_dir) as u64 * 24 * 3600,
        );
        let session_max_age = std::time::Duration::from_secs(
            Self::load_session_ttl_days(&workspace_dir) as u64 * 24 * 3600,
        );

        // Re-spawning replaces the loop; the previous one must observe its own
        // cancellation token rather than being silently orphaned.
        let cancel = tokio_util::sync::CancellationToken::new();
        if let Ok(mut guard) = self.maintenance_cancel.lock() {
            guard.cancel();
            *guard = cancel.clone();
        }

        let handle = tokio::spawn(async move {
            let ws_config = rua_cron::WorkspaceMaintenanceConfig {
                retention_days,
                ..Default::default()
            };
            let session_config = rua_cron::SessionCleanupConfig {
                max_age: session_max_age,
            };

            let chats_store = crate::chats_cleanup_store::ChatIndexSessionCleanupStore::new(
                workspace_dir.clone(),
            );

            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::debug!(agent_id = %agent_id, "maintenance loop observed cancellation");
                        break;
                    }
                    _ = ticker.tick() => {}
                }
                tracing::info!(agent_id = %agent_id, "Running periodic maintenance");
                rua_cron::run_workspace_maintenance(&workspace_dir, &ws_config).await;
                rua_cron::run_session_cleanup(&sessions_dir, &session_config, &chats_store).await;
                tick_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        });

        if let Ok(mut guard) = self.maintenance_handle.lock() {
            if let Some(old) = guard.take() {
                old.abort();
            }
            *guard = Some(handle);
        }
    }

    /// Confirms the periodic maintenance loop is gone.
    ///
    /// The token is the request; only the joined handle is evidence that the
    /// loop stopped. A loop that does not finish inside `timeout` is reported as
    /// a failure rather than waited on forever.
    async fn stop_maintenance(&self, timeout: std::time::Duration) -> bool {
        match self.maintenance_cancel.lock() {
            Ok(guard) => guard.cancel(),
            Err(_) => return false,
        }

        let handle = match self.maintenance_handle.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => return false,
        };
        let Some(mut handle) = handle else {
            // No loop was ever spawned: its absence is confirmed.
            return true;
        };

        match tokio::time::timeout(timeout, &mut handle).await {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                tracing::error!(agent_id = %self.agent_id, error = %error, "maintenance task ended abnormally");
                false
            }
            Err(_) => {
                tracing::error!(
                    agent_id = %self.agent_id,
                    "maintenance task did not exit within its shutdown bound"
                );
                handle.abort();
                false
            }
        }
    }

    /// Installs the real work authority for this Agent.
    ///
    /// The composition root owns Conversation and Task state, so it supplies the
    /// seam. Leaving it unset is allowed, but then shutdown cannot confirm that
    /// work ended and must not claim it did.
    pub fn set_work_authority(&self, authority: Arc<dyn AgentWorkAuthority>) {
        *self
            .work_authority
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(authority);
    }

    pub fn has_work_authority(&self) -> bool {
        self.work_authority
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }

    fn work_authority(&self) -> Option<Arc<dyn AgentWorkAuthority>> {
        self.work_authority
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn load_retention_days(workspace_dir: &std::path::Path) -> u32 {
        let agent_json = workspace_dir.join("agent.json");
        std::fs::read(&agent_json)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|v| {
                v.get("running")?
                    .get("tool_result_compact")?
                    .get("retention_days")?
                    .as_u64()
            })
            .map(|d| d as u32)
            .unwrap_or(5)
    }

    fn load_session_ttl_days(workspace_dir: &std::path::Path) -> u32 {
        let agent_json = workspace_dir.join("agent.json");
        std::fs::read(&agent_json)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|v| {
                v.get("running")
                    .and_then(|running| running.get("session_ttl_days"))
                    .or_else(|| v.get("session_ttl_days"))
                    .and_then(|days| days.as_u64())
            })
            .map(|d| d as u32)
            .unwrap_or(30)
    }

    pub fn maintenance_tick_count(&self) -> usize {
        self.maintenance_tick_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn begin_drain(&self) {
        let mut state = self.state.write().await;
        if state.can_accept_requests() {
            *state = WorkspaceLifecycleState::Draining;
        }
    }

    pub async fn wait_drain(&self, timeout: std::time::Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.active_session_count() == 0 && self.background_task_count() == 0 {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Stops this Agent under the default `Graceful` policy.
    ///
    /// Returns the components that failed to stop. Callers that need the full
    /// outcome use [`Self::shutdown`].
    pub async fn stop(&self) -> Vec<String> {
        self.shutdown(ShutdownPolicy::default())
            .await
            .failed_components
    }

    /// Stops every resource this Agent owns and reports what actually happened.
    ///
    /// Ordering is what makes the report truthful:
    ///
    /// 1. The Agent stops accepting new work, so a submission cannot slip in
    ///    between "nothing outstanding" and "resources closed".
    /// 2. The work authority is asked to drain (or to cancel, per policy), and
    ///    the result is re-observed afterwards instead of counted on request.
    /// 3. Owned components stop, and cleanup continues past failures.
    ///
    /// A single deadline formed from the policy governs the whole shutdown. Every
    /// authority call and component stop is wrapped in a real timeout derived
    /// from that deadline, so no step can wait forever; cleanup that must not be
    /// skipped is still attempted after it expires, and the report then says the
    /// shutdown did not finish inside its bound.
    pub async fn shutdown(&self, policy: ShutdownPolicy) -> ShutdownReport {
        let deadline = policy.deadline();
        self.begin_drain().await;
        let _operation = match tokio::time::timeout_at(deadline, self.operation_gate.lock()).await {
            Ok(operation) => operation,
            Err(_) => {
                let mut report = ShutdownReport::default();
                report.fail("runtime_operation_timeout");
                return report;
            }
        };
        let mut report = ShutdownReport::default();
        // Stops this Agent from accepting new submissions for the whole
        // shutdown, independently of whether its resources were ever assembled.
        self.begin_drain().await;

        let remaining = || deadline.saturating_duration_since(tokio::time::Instant::now());

        // Real work is whatever the owner holds, never a mirrored counter.
        let authority = self.work_authority();
        if let Some(authority) = authority.as_ref() {
            match tokio::time::timeout_at(deadline, authority.begin_shutdown(&self.agent_id)).await
            {
                Ok(Ok(())) => {}
                _ => report.fail("work_authority_admission"),
            }
        }
        let before = match authority.as_ref() {
            Some(authority) => {
                let bound = remaining().min(AUTHORITY_OBSERVE_TIMEOUT);
                match tokio::time::timeout(bound, authority.outstanding_work(&self.agent_id)).await
                {
                    Ok(Ok(work)) => work,
                    Ok(Err(error)) => {
                        tracing::error!(
                            agent_id = %self.agent_id,
                            error = %error,
                            "work authority could not report Agent work"
                        );
                        report.fail("work_authority");
                        AgentWork::default()
                    }
                    Err(_) => {
                        tracing::error!(
                            agent_id = %self.agent_id,
                            "work authority did not report Agent work within its bound"
                        );
                        report.fail("work_authority");
                        AgentWork::default()
                    }
                }
            }
            None => AgentWork::default(),
        };

        let mut cancelled = false;
        if let Some(authority) = authority.as_ref() {
            match policy {
                ShutdownPolicy::Graceful { .. } => {
                    // Reserve time for cancellation and cleanup inside the same deadline.
                    let drain_budget = remaining().div_f64(2.0).min(DRAIN_CEILING);
                    let drained = self
                        .wait_work_drained(authority, &before, drain_budget)
                        .await;
                    if drained {
                        report.turns_drained = before.count(WorkKind::Turn);
                    } else {
                        // The drain bound expired: ask for cancellation and keep the
                        // two outcomes distinct.
                        cancelled = true;
                        self.cancel_bounded(authority, &mut report, deadline).await;
                    }
                }
                ShutdownPolicy::CancelActive { .. } | ShutdownPolicy::Immediate => {
                    cancelled = true;
                    self.cancel_bounded(authority, &mut report, deadline).await;
                }
            }
        }

        if cancelled {
            if let Some(authority) = authority.as_ref() {
                self.wait_work_drained(authority, &before, remaining().div_f64(2.0))
                    .await;
            }
        }
        // Observe the outcome rather than trusting the request: work the owner
        // still holds is unresolved, and a cancellation is only a termination
        // when the owner confirms a terminal fact for that exact unit.
        let after = match authority.as_ref() {
            Some(authority) => {
                let bound = remaining().min(AUTHORITY_OBSERVE_TIMEOUT);
                match tokio::time::timeout(bound, authority.outstanding_work(&self.agent_id)).await
                {
                    Ok(Ok(work)) => {
                        report.authority_confirmed = work.is_empty();
                        work
                    }
                    Ok(Err(error)) => {
                        tracing::error!(
                            agent_id = %self.agent_id,
                            error = %error,
                            "work authority could not confirm Agent work ended"
                        );
                        report.fail("work_authority");
                        before.clone()
                    }
                    Err(_) => {
                        tracing::error!(
                            agent_id = %self.agent_id,
                            "work authority did not confirm Agent work ended within its bound"
                        );
                        report.fail("work_authority");
                        before.clone()
                    }
                }
            }
            None => {
                // No owner means no evidence. Never fall back to a fake report.
                tracing::warn!(
                    agent_id = %self.agent_id,
                    "no work authority attached; shutdown cannot confirm work termination"
                );
                report.fail("work_authority");
                before.clone()
            }
        };

        if tokio::time::timeout_at(
            deadline,
            self.record_work_outcome(authority.as_ref(), &mut report, &before, &after, cancelled),
        )
        .await
        .is_err()
        {
            report.fail("work_authority_confirmation");
            report.authority_confirmed = false;
        }

        // Tracked background tasks are Agent-owned resources: they must be
        // cancelled here, or the Agent is reported stopped while its work keeps
        // running. Cancellation is requested before the resources they use are
        // closed, so a task cannot observe a half-stopped workspace.
        let cancelled_tasks = self.task_tracker.cancel_all().await;
        if cancelled_tasks > 0 {
            tracing::info!(
                agent_id = %self.agent_id,
                cancelled_tasks,
                "cancelled tracked background tasks"
            );
        }

        // The maintenance loop is the Agent's own background resource: confirm it
        // exited by joining its handle rather than only cancelling its token.
        if !self
            .stop_maintenance(remaining().min(MAINTENANCE_STOP_TIMEOUT))
            .await
        {
            report.fail("maintenance");
        }

        // Component stops are independent of each other and of the deadline:
        // skipping them would leak real resources, so the whole set is attempted.
        let stops = self.service_manager.stop_owned(deadline).await;
        for component in stops.failed {
            report.fail(component);
        }
        report.timed_out_components = stops.timed_out;

        *self.state.write().await = WorkspaceLifecycleState::Stopped;

        report.cleanup_completed = report.failed_components.is_empty()
            && report.timed_out_components.is_empty()
            && !self.service_manager.has_unstopped().await;
        report.finished_within_timeout = tokio::time::Instant::now() <= deadline;

        report
    }

    /// Waits for the authority to report no outstanding work, bounded by `timeout`.
    ///
    /// Per-Chat Turns and Task Attempts are observed through the owner, so this is
    /// a real drain and not a wait on local session counters. Each observation is
    /// itself bounded, so a stalled authority cannot outlive the drain.
    async fn wait_work_drained(
        &self,
        authority: &Arc<dyn AgentWorkAuthority>,
        before: &AgentWork,
        timeout: std::time::Duration,
    ) -> bool {
        if before.is_empty() {
            return true;
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let bound = remaining.min(AUTHORITY_OBSERVE_TIMEOUT);
            match tokio::time::timeout(bound, authority.outstanding_work(&self.agent_id)).await {
                Ok(Ok(work)) if work.is_empty() => return true,
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(
                        agent_id = %self.agent_id,
                        error = %error,
                        "work authority failed during drain; escalating"
                    );
                    return false;
                }
                Err(_) => {
                    tracing::warn!(
                        agent_id = %self.agent_id,
                        "work authority stopped answering during drain; escalating"
                    );
                    return false;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    /// Asks the owner to end this Agent's work within a real bound.
    ///
    /// A cancel request that does not return in time is not evidence that work
    /// stopped, so it is reported as a failure and the following observation is
    /// what decides the counts.
    async fn cancel_bounded(
        &self,
        authority: &Arc<dyn AgentWorkAuthority>,
        report: &mut ShutdownReport,
        deadline: tokio::time::Instant,
    ) {
        let bound = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .min(CANCELLATION_TIMEOUT);
        match tokio::time::timeout(bound, authority.cancel_work(&self.agent_id)).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::error!(
                    agent_id = %self.agent_id,
                    error = %error,
                    "work authority failed to cancel Agent work"
                );
                report.fail("work_authority");
            }
            Err(_) => {
                tracing::error!(
                    agent_id = %self.agent_id,
                    "work authority did not finish cancelling Agent work within its bound"
                );
                report.fail("work_authority");
            }
        }
    }

    /// Records the observed work outcome, counting only evidenced terminations.
    ///
    /// A unit is counted as cancelled only when the owner confirms it terminal;
    /// anything still held, and anything cancelled but never confirmed, stays in
    /// the unresolved fields so a caller cannot read a request as a result.
    async fn record_work_outcome(
        &self,
        authority: Option<&Arc<dyn AgentWorkAuthority>>,
        report: &mut ShutdownReport,
        before: &AgentWork,
        after: &AgentWork,
        cancelled: bool,
    ) {
        let turns_before = before.count(WorkKind::Turn);
        let tasks_before = before.count(WorkKind::Task);
        let turns_after = after.count(WorkKind::Turn);
        let tasks_after = after.count(WorkKind::Task);

        report.queued_inputs_unresolved = after.queued_inputs;
        report.queued_inputs_discarded = before.queued_inputs.saturating_sub(after.queued_inputs);
        report.turns_unresolved = turns_after;
        report.tasks_unresolved = tasks_after;

        if !cancelled || before.units.is_empty() {
            // A drain reports its own outcome; nothing was retired by this call.
            return;
        }

        let Some(authority) = authority else {
            return;
        };
        let retired: Vec<WorkUnit> = before
            .units
            .iter()
            .filter(|unit| !after.units.contains(unit))
            .cloned()
            .collect();
        // Nothing disappeared, so every unit is still observable and unresolved.
        // Asking the owner again would only re-observe the same live work.
        if retired.is_empty() {
            return;
        }

        let confirmed = match tokio::time::timeout(
            AUTHORITY_OBSERVE_TIMEOUT,
            authority.terminated_units(&retired),
        )
        .await
        {
            Ok(Ok(units)) => units,
            Ok(Err(error)) => {
                tracing::error!(
                    agent_id = %self.agent_id,
                    error = %error,
                    "work authority could not confirm terminal state for retired work"
                );
                report.fail("work_authority");
                Vec::new()
            }
            Err(_) => {
                tracing::error!(
                    agent_id = %self.agent_id,
                    "work authority did not confirm terminal state within its bound"
                );
                report.fail("work_authority");
                Vec::new()
            }
        };

        let confirmed_turns = confirmed
            .iter()
            .filter(|unit| unit.kind == WorkKind::Turn)
            .count()
            .min(turns_before.saturating_sub(turns_after));
        let confirmed_tasks = confirmed
            .iter()
            .filter(|unit| unit.kind == WorkKind::Task)
            .count()
            .min(tasks_before.saturating_sub(tasks_after));

        report.turns_cancelled = confirmed_turns;
        report.tasks_cancelled = confirmed_tasks;
        // A unit that vanished without a terminal fact is not a termination: it
        // stays unresolved instead of being rounded up to a clean stop.
        report.turns_unresolved +=
            (turns_before.saturating_sub(turns_after)).saturating_sub(confirmed_turns);
        report.tasks_unresolved +=
            (tasks_before.saturating_sub(tasks_after)).saturating_sub(confirmed_tasks);
    }

    pub(crate) fn work_authority_for_registry(&self) -> Option<Arc<dyn AgentWorkAuthority>> {
        self.work_authority()
    }

    pub async fn health(&self) -> WorkspaceHealth {
        let components = self.service_manager.component_states().await;
        let lifecycle = self.state.read().await.clone();

        let mut builder = WorkspaceHealthBuilder::new(&self.agent_id)
            .lifecycle(lifecycle)
            .active_sessions(self.active_session_count())
            .background_tasks(self.task_tracker.active_count());

        for (name, state) in components.iter() {
            builder = builder.component(name.clone(), state.clone());
        }

        builder.build()
    }

    pub fn workspace_dir(&self) -> std::path::PathBuf {
        self.root.as_path().to_path_buf()
    }

    pub(crate) fn task_runtime(&self) -> Arc<rua_task::TaskRuntime> {
        Arc::clone(&self.ctx.task_runtime)
    }

    pub fn rua_root(&self) -> std::path::PathBuf {
        self.ctx.rua_root.clone()
    }

    pub fn secret_root(&self) -> std::path::PathBuf {
        self.ctx.secret_dir.clone()
    }

    pub fn runtime_cache_dir(&self) -> std::path::PathBuf {
        self.ctx.cache_dir.clone()
    }

    pub async fn resolved_agent(&self) -> Result<rua_agent::ResolvedAgent, WorkspaceError> {
        if self.agent_id != "default"
            && self
                .ctx
                .agent_cells
                .full(&self.agent_id)
                .read()
                .await
                .map_err(|error| WorkspaceError::config_invalid(error.to_string()))?
                .is_none()
        {
            return Err(WorkspaceError::agent_not_found(&self.agent_id));
        }
        let config = self
            .ctx
            .agent_profile_store
            .load_agent_config_via_cells(&self.ctx.agent_cells, Some(&self.agent_id))
            .await
            .map_err(|error| WorkspaceError::config_invalid(error.to_string()))?;
        Ok(rua_agent::ResolvedAgent::from_config(&config))
    }

    pub async fn mcp_manager(&self) -> Option<Arc<dyn McpManager>> {
        self.mcp_manager.read().expect("MCP owner poisoned").clone()
    }

    pub async fn memory_manager(&self) -> Option<Arc<dyn MemoryManager>> {
        self.memory_manager.read().await.clone()
    }

    pub async fn context_manager(&self) -> Option<Arc<dyn ContextManager>> {
        self.context_manager.read().await.clone()
    }

    pub fn session_replication_hook(
        &self,
    ) -> Option<Arc<dyn rua_contract::agent::SessionReplicationHook>> {
        self.ctx.session_replication_hook.get().cloned()
    }

    pub async fn channel_manager(&self) -> Option<Arc<ChannelManager>> {
        {
            let guard = self.channel_manager.read().await;
            if guard.is_some() {
                return guard.clone();
            }
        }
        let mut guard = self.channel_manager.write().await;
        if guard.is_some() {
            return guard.clone();
        }
        let cm = Arc::new(ChannelManager::new());
        *guard = Some(Arc::clone(&cm));
        Some(cm)
    }

    pub async fn security_module(&self) -> Option<Arc<SecurityModule>> {
        Some(Arc::clone(&self.ctx.security))
    }

    pub async fn react_runtime(&self) -> Option<Arc<dyn AgentRuntime>> {
        self.react_runtime.read().await.clone()
    }

    pub async fn task_toolkit(&self) -> Option<Arc<react_agent::AgentToolkit>> {
        self.task_toolkit.read().await.clone()
    }

    pub async fn capability_configuration(
        &self,
        project_root: Option<&std::path::Path>,
    ) -> rua_core::Result<rua_agent::runtime::ChatCapabilityConfiguration> {
        let resolver = self
            .capability_resolver
            .read()
            .await
            .clone()
            .ok_or_else(|| rua_core::RuaError::Runtime {
                message: "Capability resolver is unavailable".to_string(),
            })?;
        let mut configuration = resolver
            .configuration(&self.workspace_dir(), project_root, "console")
            .await?;
        let enabled = self
            .ctx
            .tool_config_store
            .list_tools(Some(&self.agent_id))
            .await
            .map_err(|error| rua_core::RuaError::Config {
                message: error.to_string(),
            })?
            .into_iter()
            .filter(|tool| tool.enabled)
            .map(|tool| tool.name)
            .collect::<std::collections::HashSet<_>>();
        configuration.tools.retain(|name| enabled.contains(name));
        let tools = self
            .ctx
            .agent_cells
            .section::<serde_json::Value>(&self.agent_id, "tools")
            .read()
            .await
            .map_err(|error| rua_core::RuaError::Config {
                message: error.to_string(),
            })?;
        if tools
            .as_ref()
            .and_then(|tools| tools.get("default_enabled"))
            .and_then(serde_json::Value::as_bool)
            == Some(false)
        {
            let explicit_mcp = self
                .ctx
                .agent_cells
                .section::<serde_json::Value>(&self.agent_id, "mcp")
                .read()
                .await
                .map_err(|error| rua_core::RuaError::Config {
                    message: error.to_string(),
                })?;
            let explicit_mcp = match explicit_mcp {
                Some(value) => {
                    rua_mcp::McpConfigLoader::load_from_value(value).map_err(|error| {
                        rua_core::RuaError::Config {
                            message: error.to_string(),
                        }
                    })?
                }
                None => mcp_manager::types::McpConfig::default(),
            };
            configuration.mcp_servers.retain(|id, _| {
                explicit_mcp
                    .clients
                    .iter()
                    .any(|(key, config)| key.as_str() == id && config.enabled)
            });
            configuration.plugin_ids.clear();
        }
        Ok(configuration)
    }

    pub async fn compactor(&self) -> Option<Arc<ConversationCompactor>> {
        self.compactor.read().await.clone()
    }

    async fn lazy_init_mcp(&self) {
        // One manager, including concurrent first use; never replace an existing owner.
        if self.mcp_manager().await.is_some() {
            return;
        }
        let agent_json_path = self.root.as_path().join("agent.json");

        let mcp_value = match std::fs::read(&agent_json_path) {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(root) => root.get("mcp").filter(|v| !v.is_null()).cloned(),
                Err(e) => {
                    tracing::warn!(
                        agent_id = %self.agent_id,
                        path = %agent_json_path.display(),
                        error = %e,
                        "failed to parse agent.json for MCP config"
                    );
                    None
                }
            },
            Err(_) => None,
        };

        let mcp_config = match mcp_value {
            Some(val) => match rua_mcp::McpConfigLoader::load_from_value(val) {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::warn!(
                        agent_id = %self.agent_id,
                        error = %e,
                        "invalid MCP config in agent.json, skipping"
                    );
                    return;
                }
            },
            None => mcp_manager::types::McpConfig::default(),
        };
        self.assemble_mcp(mcp_config).await;
    }

    async fn assemble_mcp(&self, mcp_config: mcp_manager::types::McpConfig) {
        let authority = rua_core::runtime_authority::RuntimeAuthority::from_current_process(
            None,
            self.root.as_path(),
            &self.ctx.cache_dir,
        );
        let bundled_browser =
            rua_mcp::bundled_browser::BundledBrowser::from_runtime(&authority, &self.ctx.cache_dir);
        let bundled_computer = rua_mcp::bundled_computer::BundledComputer::from_runtime(
            &authority,
            &self.ctx.cache_dir,
        );
        if mcp_config.clients.is_empty() && bundled_browser.is_none() && bundled_computer.is_none()
        {
            return;
        }

        let mcp_profile = match self.profile {
            PlatformProfile::Desktop => rua_mcp::PlatformProfile::Desktop,
            PlatformProfile::Mobile => rua_mcp::PlatformProfile::MobileDefault,
        };
        let mut manager_impl =
            rua_mcp::manager::McpManagerImpl::new(rua_mcp::McpMobilePolicy::new(mcp_profile))
                .with_runtime_dirs(self.root.as_path(), &self.ctx.cache_dir);
        if let Some(browser) = bundled_browser {
            manager_impl = manager_impl.with_bundled_browser(browser);
        }
        if let Some(computer) = bundled_computer {
            manager_impl = manager_impl.with_bundled_computer(computer);
        }
        let manager: Arc<dyn McpManager> = Arc::new(manager_impl);

        let client_count = mcp_config.clients.len();

        // The configuration load is the MCP start: the adapter applies it and
        // reports failures through the component state, so a manager whose
        // configuration could not be applied is never recorded as started.
        self.service_manager
            .register_lifecycle_if_absent(
                "mcp",
                Arc::new(crate::services::McpServiceAdapter::new(
                    manager.clone(),
                    Some(mcp_config),
                )),
            )
            .await;
        *self.mcp_manager.write().expect("MCP owner poisoned") = Some(manager);
        tracing::info!(
            agent_id = %self.agent_id,
            servers = client_count,
            "per-agent MCP manager assembled; configuration is applied by its lifecycle"
        );
    }

    #[doc(hidden)]
    pub async fn set_mcp_manager(&self, mgr: Arc<dyn McpManager>) {
        *self.mcp_manager.write().expect("MCP owner poisoned") = Some(mgr);
    }

    pub async fn apply_mcp_configuration<F, Fut>(
        &self,
        read_configuration: F,
    ) -> Result<mcp_manager::types::McpReloadReport, WorkspaceError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<mcp_manager::types::McpConfig, WorkspaceError>>
            + Send,
    {
        let _operation = self.operation_gate.lock().await;
        let config = match read_configuration().await {
            Ok(config) => config,
            Err(error) => {
                if let Some(manager) = self.mcp_manager().await {
                    manager.suspend_new_calls().await.map_err(|failure| {
                        WorkspaceError::component_init_failed("mcp", failure.to_string())
                    })?;
                }
                return Err(error);
            }
        };
        if matches!(
            self.lifecycle_state().await,
            WorkspaceLifecycleState::Stopped
                | WorkspaceLifecycleState::Draining
                | WorkspaceLifecycleState::Failed
        ) {
            return Err(WorkspaceError::component_init_failed(
                "mcp",
                "Workspace is unavailable",
            ));
        }
        if self.mcp_manager().await.is_none() {
            self.assemble_mcp(config.clone()).await;
        }
        let manager = self.mcp_manager().await.ok_or_else(|| {
            WorkspaceError::component_init_failed("mcp", "MCP manager could not be assembled")
        })?;
        let report = self.service_manager.apply_mcp(manager, config).await?;
        if let Some(runtime) = self.react_runtime().await {
            if !report.added.is_empty() || !report.removed.is_empty() || !report.updated.is_empty()
            {
                runtime.bump_prepared_generation();
            }
        }
        Ok(report)
    }

    pub async fn set_memory_manager(&self, mgr: Arc<dyn MemoryManager>) {
        *self.memory_manager.write().await = Some(mgr);
    }

    pub async fn set_context_manager(&self, mgr: Arc<dyn ContextManager>) {
        *self.context_manager.write().await = Some(mgr);
    }

    pub async fn set_channel_manager(&self, mgr: Arc<ChannelManager>) {
        *self.channel_manager.write().await = Some(mgr);
    }

    pub async fn record_initial_channels(&self, value: serde_json::Value) {
        *self.applied_channels.write().await = value.as_object().cloned().unwrap_or_default();
    }

    pub async fn apply_channels<F, Fut>(&self, read: F) -> Result<(), WorkspaceError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<serde_json::Value, WorkspaceError>> + Send,
    {
        self.apply_channels_for(read, None).await
    }

    pub async fn apply_channels_for<F, Fut>(
        &self,
        read: F,
        restart: Option<&str>,
    ) -> Result<(), WorkspaceError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<serde_json::Value, WorkspaceError>> + Send,
    {
        let _operation = self.operation_gate.lock().await;
        if !self.can_accept_requests().await {
            return Err(WorkspaceError::component_init_failed(
                "channels",
                "Workspace is unavailable",
            ));
        }
        let value = read().await?;
        let new = value.as_object().cloned().unwrap_or_default();
        let previous = self.applied_channels.read().await.clone();
        if previous == new && restart.is_none() {
            return Ok(());
        }
        serde_json::from_value::<crate::service_manager::ImPlatformConfigs>(
            serde_json::Value::Object(new.clone()),
        )
        .map_err(|error| WorkspaceError::config_invalid(error.to_string()))?;
        let manager = self.channel_manager().await.ok_or_else(|| {
            WorkspaceError::component_init_failed("channels", "Channel manager unavailable")
        })?;
        self.service_manager
            .register_lifecycle_if_absent(
                "channels",
                Arc::new(crate::services::ChannelsServiceAdapter::new(Arc::clone(
                    &manager,
                ))),
            )
            .await;
        self.service_manager
            .apply_channels(|| async {
                if let Some(runtime) = self.react_runtime().await {
                    manager
                        .set_process_if_absent(Arc::new(
                            crate::channel_process_adapter::ChannelProcessAdapter::new(runtime)
                                .with_workspace_dir(self.workspace_dir())
                                .with_memory_write_available(self.memory_manager().await.is_some()),
                        ))
                        .await;
                }
                let mut applied = previous.clone();
                let mut failures = Vec::new();
                let mut keys = std::collections::BTreeSet::new();
                keys.extend(previous.keys().cloned());
                keys.extend(new.keys().cloned());
                for key in keys {
                    if previous.get(&key) == new.get(&key) && restart != Some(key.as_str()) {
                        continue;
                    }
                    if matches!(key.as_str(), "console" | "voice") {
                        continue;
                    }
                    let result = async {
                        manager.remove_channel(&key).await.map_err(|error| {
                            WorkspaceError::component_shutdown_failed("channels", error.to_string())
                        })?;
                        let mut changed = serde_json::Map::new();
                        if let Some(config) = new.get(&key) {
                            changed.insert(key.clone(), config.clone());
                        }
                        let configs = serde_json::from_value::<
                            crate::service_manager::ImPlatformConfigs,
                        >(serde_json::Value::Object(changed))
                        .map_err(|error| WorkspaceError::config_invalid(error.to_string()))?;
                        let adapter = crate::services::ChannelsServiceAdapter::from_configs(
                            Arc::clone(&manager),
                            configs,
                        );
                        crate::service_manager::ServiceLifecycle::start(&adapter).await
                    }
                    .await;
                    if result.is_err() {
                        failures.push(key);
                        continue;
                    }
                    if let Some(config) = new.get(&key) {
                        applied.insert(key, config.clone());
                    } else {
                        applied.remove(&key);
                    }
                }
                *self.applied_channels.write().await = applied;
                if !failures.is_empty() {
                    return Err(WorkspaceError::component_init_failed(
                        "channels",
                        format!("Channel application failed: {}", failures.join(", ")),
                    ));
                }
                for key in ["console"] {
                    if let Some(value) = new.get(key) {
                        self.applied_channels
                            .write()
                            .await
                            .insert(key.to_string(), value.clone());
                    }
                }
                Ok(())
            })
            .await
    }

    pub async fn set_react_runtime(&self, rt: Arc<dyn AgentRuntime>) {
        *self.react_runtime.write().await = Some(rt);
    }

    /// Registers the agent lifecycle for the runtime assembled by React init.
    ///
    /// Until the runtime exists there is nothing to warm up, so the `agent`
    /// capability stays `NotInitialized` instead of being reported as started.
    async fn ensure_agent_service(&self) {
        let Some(runtime) = self.react_runtime().await else {
            tracing::debug!(
                agent_id = %self.agent_id,
                "React runtime is not assembled yet; agent lifecycle stays unregistered"
            );
            return;
        };
        self.service_manager
            .register_lifecycle_if_absent(
                "agent",
                Arc::new(crate::services::AgentServiceAdapter::new(
                    runtime,
                    rua_core::id::AgentId::new(self.agent_id.clone()),
                )),
            )
            .await;
    }

    pub async fn set_compactor(&self, c: Arc<ConversationCompactor>) {
        *self.compactor.write().await = Some(c);
    }

    pub async fn summary(&self, is_active: bool) -> WorkspaceSummary {
        WorkspaceSummary {
            agent_id: self.agent_id.clone(),
            agent_name: self.agent_name.clone(),
            is_active,
            lifecycle: self.state.read().await.clone(),
            active_sessions: self.active_session_count(),
        }
    }

    async fn init_memory_manager(&self) {
        let summary_gen: Arc<dyn rua_agent::compaction::SummaryGenerator> =
            Arc::new(ProviderSummaryGenerator {
                provider_manager: Arc::clone(&self.ctx.provider_manager),
                usage_recorder: self.ctx.usage_recorder.clone(),
            });

        let ws_dir = self.workspace_dir();
        let _ = std::fs::create_dir_all(&ws_dir);
        let vec_db_path = ws_dir.join("memory_vec.db");
        let agent_cfg = std::fs::read(ws_dir.join("agent.json"))
            .ok()
            .and_then(|bytes| {
                serde_json::from_slice::<rua_config::AgentProfileConfig>(&bytes).ok()
            });

        let embedder: Arc<dyn rua_memory::Embedder> = {
            match agent_cfg.as_ref() {
                Some(agent_cfg)
                    if !agent_cfg.running.embedding_config.base_url.is_empty()
                        && !agent_cfg.running.embedding_config.model_name.is_empty() =>
                {
                    let ec = &agent_cfg.running.embedding_config;
                    let api_key = if ec.api_key.is_empty() {
                        None
                    } else {
                        Some(ec.api_key.clone())
                    };
                    let transport = Arc::new(rua_memory::HttpEmbeddingTransport::new(
                        &ec.base_url,
                        api_key,
                    ));
                    tracing::info!(
                        agent_id = %self.agent_id,
                        model = %ec.model_name,
                        base_url = %ec.base_url,
                        dimensions = ec.dimensions,
                        "per-agent embedding transport: http"
                    );
                    Arc::new(
                        rua_memory::LlmEmbedder::new(
                            transport,
                            &ec.model_name,
                            ec.dimensions as usize,
                        )
                        .with_backend_identity(&ec.backend),
                    )
                }
                _ => {
                    tracing::info!(
                        agent_id = %self.agent_id,
                        "no embedding config, falling back to BM25-only"
                    );
                    Arc::new(rua_memory::NullEmbedder)
                }
            }
        };

        let mut mm = rua_agent::memory_manager::LlmMemoryManager::new(summary_gen)
            .with_workspace_memory_root(self.ctx.rua_root.join("workspaces"))
            .with_publication_hook_cell(Arc::clone(&self.ctx.memory_publication_hook));
        match rua_memory::StructuredMemoryStore::open(ws_dir.join("memory.db")) {
            Ok(store) => {
                mm = mm.with_structured_memory_store(store);
            }
            Err(e) => {
                tracing::warn!(
                    agent_id = %self.agent_id,
                    error = %e,
                    "Failed to open structured memory store, memory_write disabled"
                );
            }
        }

        match sqlite_vecstore::VecStore::open(
            &vec_db_path,
            sqlite_vecstore::VecStoreConfig::default(),
        ) {
            Ok(store) => {
                mm = mm.with_vec_store(store, embedder);
            }
            Err(e) => {
                tracing::warn!(
                    agent_id = %self.agent_id,
                    path = %vec_db_path.display(),
                    error = %e,
                    "Failed to open memory vec store, vector search disabled"
                );
            }
        }

        let manager = Arc::new(mm);
        *self
            .memory_embedder
            .write()
            .expect("Memory embedder owner poisoned") = Some(manager.embedder_slot());
        *self.applied_embedding.write().await = agent_cfg
            .as_ref()
            .map(|config| config.running.embedding_config.clone());
        let memory_manager: Arc<dyn MemoryManager> = manager.clone();
        let context_manager: Arc<dyn ContextManager> = manager;
        self.set_memory_manager(Arc::clone(&memory_manager)).await;
        self.set_context_manager(context_manager).await;
        self.service_manager
            .register_lifecycle_if_absent(
                "memory",
                Arc::new(crate::services::MemoryServiceAdapter::new(memory_manager)),
            )
            .await;
    }

    pub async fn apply_embedding_configuration(&self) -> Result<(), WorkspaceError> {
        let _operation = self.operation_gate.lock().await;
        if !self.can_accept_requests().await {
            return Err(WorkspaceError::component_init_failed(
                "memory",
                "Workspace is unavailable",
            ));
        }
        let slot = self
            .memory_embedder
            .read()
            .expect("Memory embedder owner poisoned")
            .clone()
            .ok_or_else(|| {
                WorkspaceError::component_init_failed("memory", "Memory owner is unavailable")
            })?;
        let config = match self.resolved_agent().await {
            Ok(agent) => agent.running.embedding_config.clone(),
            Err(error) => {
                *slot.write().expect("Memory embedder poisoned") = None;
                return Err(error);
            }
        };
        if self.applied_embedding.read().await.as_ref() == Some(&config)
            && slot.read().expect("Memory embedder poisoned").is_some()
        {
            return Ok(());
        }
        let embedder: Arc<dyn rua_memory::Embedder> = if config.base_url.is_empty()
            && config.model_name.is_empty()
        {
            Arc::new(rua_memory::NullEmbedder)
        } else {
            if config.base_url.is_empty() || config.model_name.is_empty() || config.dimensions == 0
            {
                *slot.write().expect("Memory embedder poisoned") = None;
                return Err(WorkspaceError::config_invalid(
                    "Embedding requires base URL, model and nonzero dimensions",
                ));
            }
            if !reqwest::Url::parse(&config.base_url)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
            {
                *slot.write().expect("Memory embedder poisoned") = None;
                return Err(WorkspaceError::config_invalid(
                    "Embedding base URL must be HTTP or HTTPS",
                ));
            }
            let transport = Arc::new(rua_memory::HttpEmbeddingTransport::new(
                &config.base_url,
                (!config.api_key.is_empty()).then(|| config.api_key.clone()),
            ));
            Arc::new(
                rua_memory::LlmEmbedder::new(
                    transport,
                    &config.model_name,
                    config.dimensions as usize,
                )
                .with_backend_identity(&config.backend),
            )
        };
        *slot.write().expect("Memory embedder poisoned") = None;
        if embedder.dimension() > 0 {
            let probe = vec!["Embedding configuration readiness check".to_string()];
            match tokio::time::timeout(std::time::Duration::from_secs(10), embedder.embed(&probe))
                .await
            {
                Ok(Ok(vectors))
                    if vectors.len() == 1
                        && vectors[0].len() == embedder.dimension()
                        && vectors[0].iter().all(|value| value.is_finite()) => {}
                _ => {
                    return Err(WorkspaceError::component_init_failed(
                        "embedding",
                        "Saved Embedding configuration could not produce a valid vector",
                    ))
                }
            }
        }
        *slot.write().expect("Memory embedder poisoned") = Some(embedder);
        *self.applied_embedding.write().await = Some(config);
        Ok(())
    }

    pub async fn apply_maintenance_configuration(&self) -> Result<(), WorkspaceError> {
        let _operation = self.operation_gate.lock().await;
        if !self.can_accept_requests().await {
            return Err(WorkspaceError::component_init_failed(
                "maintenance",
                "Workspace is unavailable",
            ));
        }
        let value = crate::config::LayeredConfig::for_workspace(&self.ctx.rua_root, &self.root)
            .merged()
            .map_err(|error| WorkspaceError::config_invalid(error.to_string()))?;
        let hours = value
            .get("maintenance_interval_hours")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(6.0);
        if !hours.is_finite() || hours <= 0.0 {
            return Err(WorkspaceError::config_invalid(
                "Maintenance interval must be positive and finite",
            ));
        }
        let interval = std::time::Duration::try_from_secs_f64(hours * 3600.0)
            .map_err(|error| WorkspaceError::config_invalid(error.to_string()))?;
        if *self
            .maintenance_interval
            .lock()
            .expect("Maintenance interval poisoned")
            == Some(interval)
        {
            return Ok(());
        }
        if !self.stop_maintenance(MAINTENANCE_STOP_TIMEOUT).await {
            return Err(WorkspaceError::component_shutdown_failed(
                "maintenance",
                "Previous maintenance loop did not exit",
            ));
        }
        self.spawn_maintenance_task(interval);
        Ok(())
    }

    async fn lazy_init_react_runtime(&self) {
        let ws_dir = self.workspace_dir();
        let toolkit = build_default_toolkit().create_filtered(&[]);
        let toolkit = Arc::new(toolkit);
        *self.task_toolkit.write().await = Some(Arc::clone(&toolkit));

        let mut rt = ReactAgentRuntime::new(Arc::clone(&self.ctx.provider_manager))
            .with_mcp_manager_slot(Arc::clone(&self.mcp_manager))
            .with_rua_root(self.ctx.rua_root.clone())
            .with_toolkit(toolkit)
            .with_task_runtime(Arc::clone(&self.ctx.task_runtime))
            .with_collaboration_runtime(Arc::clone(&self.ctx.collaboration_runtime))
            .with_sync_transfer_runtime(Arc::clone(&self.ctx.sync_transfer_runtime))
            .with_plugin_registry(Arc::clone(&self.ctx.plugin_registry))
            .with_active_plugin_registry(Arc::clone(&self.ctx.active_plugin_registry));
        if let Some(ref usage_recorder) = self.ctx.usage_recorder {
            rt = rt.with_usage_recorder(Arc::clone(usage_recorder));
        }
        if let Some(boundary) = self.ctx.tool_before_boundary.clone() {
            rt = rt.with_tool_before_boundary(boundary);
        }
        if let Some(boundary) = self.ctx.tool_observer_boundary.clone() {
            rt = rt.with_tool_observer_boundary(boundary);
        }
        if let Some(boundary) = self.ctx.permission_request_boundary.clone() {
            rt = rt.with_permission_request_boundary(boundary);
        }

        if let Ok(resolved) = self.resolved_agent().await {
            rt = rt.with_running_config(resolved.running);
        }

        if let Some(security) = self.security_module().await {
            rt = rt.with_security(security);
        }

        if let Some(mm) = self.memory_manager().await {
            rt = rt.with_memory_manager(mm);
        }
        if let Some(cm) = self.context_manager().await {
            rt = rt.with_context_manager(cm);
        }
        if let Some(hook) = self.ctx.session_replication_hook.get() {
            rt = rt.with_session_replication_hook(Arc::clone(hook));
        }

        let rt = Arc::new(rt);

        if let Some(mcp_mgr) = self.mcp_manager().await {
            rt.set_mcp_manager(mcp_mgr);
        }

        if let Some(factory) = self.ctx.command_runtime_factory.get() {
            let cfg = factory(&self.agent_id, &ws_dir);
            rt.set_command_runtime(cfg);
        }

        *self.capability_resolver.write().await = Some(rt.capability_resolver());
        *self.react_runtime.write().await = Some(rt);

        tracing::info!(
            agent_id = %self.agent_id,
            "per-agent react_runtime initialized (toolkit + mcp + memory + approval)"
        );
    }
}

struct ProviderSummaryGenerator {
    provider_manager: Arc<rua_provider::manager::ProviderManager>,
    usage_recorder: Option<Arc<dyn rua_contract::UsageRecorder>>,
}

impl ProviderSummaryGenerator {
    async fn build_model(
        &self,
        model: Option<&rua_session::CheckpointModelIdentity>,
    ) -> Result<Arc<dyn llm_multiplex::ChatModel>, rua_agent::error::AgentError> {
        let (provider_id, raw_model) = match model {
            Some(model) => {
                let raw_model = self
                    .provider_manager
                    .chat_model(&model.provider, &model.model)
                    .map_err(|e| {
                        rua_agent::error::AgentError::new(
                            rua_agent::error::AgentErrorKind::Provider,
                            format!("active Chat model unavailable for context compaction: {e}"),
                        )
                    })?;
                (model.provider.clone(), raw_model)
            }
            None => {
                let active = self.provider_manager.default_model().await.ok_or_else(|| {
                    rua_agent::error::AgentError::new(
                        rua_agent::error::AgentErrorKind::Provider,
                        "no active model available for context compaction",
                    )
                })?;
                let raw_model = self
                    .provider_manager
                    .default_chat_model()
                    .await
                    .map_err(|e| {
                        rua_agent::error::AgentError::new(
                            rua_agent::error::AgentErrorKind::Provider,
                            format!("no model available for context compaction: {e}"),
                        )
                    })?;
                (active.provider_id, raw_model)
            }
        };
        let mut factory = rua_provider::ModelFactory::new();
        if let Some(ref recorder) = self.usage_recorder {
            factory = factory.with_usage_recorder(Arc::clone(recorder));
        }
        Ok(Arc::from(factory.build(raw_model, &provider_id)))
    }
}

#[async_trait::async_trait]
impl rua_agent::compaction::SummaryGenerator for ProviderSummaryGenerator {
    async fn summarize(
        &self,
        messages: &[agent_types::ModelDialogueItem],
        previous_summary: Option<&str>,
        max_tokens: Option<usize>,
        model: Option<&rua_session::CheckpointModelIdentity>,
    ) -> Result<String, rua_agent::error::AgentError> {
        self.summarize_with_context(messages, previous_summary, max_tokens, model, None)
            .await
    }

    async fn summarize_with_context(
        &self,
        messages: &[agent_types::ModelDialogueItem],
        previous_summary: Option<&str>,
        max_tokens: Option<usize>,
        model: Option<&rua_session::CheckpointModelIdentity>,
        model_call_context: Option<&rua_agent::model_call_context::ModelCallContext>,
    ) -> Result<String, rua_agent::error::AgentError> {
        let model = self.build_model(model).await?;
        let gen = rua_agent::llm_summary::LlmSummaryGenerator::new(model);
        gen.summarize_with_context(
            messages,
            previous_summary,
            max_tokens,
            None,
            model_call_context,
        )
        .await
    }

    async fn generate_history_replacement(
        &self,
        request: &rua_agent::context_manager::HistoryReplacementGenerationRequest,
    ) -> Result<
        rua_agent::context_manager::HistoryReplacementGenerationResult,
        rua_agent::error::AgentError,
    > {
        let chat_model = self.build_model(Some(&request.model)).await?;
        let generator = rua_agent::llm_summary::LlmSummaryGenerator::new(chat_model);
        generator.generate_history_replacement(request).await
    }
}
