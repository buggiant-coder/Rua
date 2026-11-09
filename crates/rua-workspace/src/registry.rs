use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::context::WorkspaceContext;
use crate::error::WorkspaceError;
use crate::lifecycle::{AgentWorkAuthority, PlatformProfile, ShutdownPolicy, ShutdownReport};
use crate::root::RuaRoot;
use crate::runtime::{WorkspaceRuntime, WorkspaceSummary};

#[async_trait]
pub trait WorkspaceRegistry: Send + Sync {
    async fn get_or_create(&self, agent_id: &str) -> Result<Arc<WorkspaceRuntime>, WorkspaceError>;

    async fn get(&self, agent_id: &str) -> Option<Arc<WorkspaceRuntime>>;

    async fn list(&self) -> Result<Vec<WorkspaceSummary>, WorkspaceError>;

    async fn reload_agent(&self, agent_id: &str) -> Result<(), WorkspaceError>;

    async fn remove(&self, agent_id: &str) -> Result<(), WorkspaceError>;

    /// Removes one Agent under an explicit policy and returns what the shutdown
    /// observed.
    ///
    /// A registry must state its outcome rather than implying that detaching the
    /// index entry stopped the resources, so this is part of the contract, not an
    /// optional refinement.
    async fn remove_with_policy(
        &self,
        agent_id: &str,
        policy: ShutdownPolicy,
    ) -> Result<ShutdownReport, WorkspaceError>;

    async fn shutdown_all(&self, policy: ShutdownPolicy) -> Result<ShutdownReport, WorkspaceError>;
}

#[derive(Debug, Clone, Default)]
pub struct AgentResolveRequest {
    pub explicit_agent_id: Option<String>,

    pub channel: Option<String>,

    pub session_id: Option<String>,

    pub user_id: Option<String>,
}

pub struct AgentResolver {
    active_agent_id: Arc<RwLock<String>>,

    channel_bindings: Arc<RwLock<HashMap<String, String>>>,

    session_bindings: Arc<RwLock<HashMap<String, String>>>,
}

impl AgentResolver {
    pub fn new(active_agent_id: impl Into<String>) -> Self {
        Self {
            active_agent_id: Arc::new(RwLock::new(active_agent_id.into())),
            channel_bindings: Arc::new(RwLock::new(HashMap::new())),
            session_bindings: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn resolve(&self, request: &AgentResolveRequest) -> Result<String, WorkspaceError> {
        if let Some(ref agent_id) = request.explicit_agent_id {
            if !agent_id.is_empty() {
                return Ok(agent_id.clone());
            }
        }

        if let Some(ref session_id) = request.session_id {
            let bindings = self.session_bindings.read().await;
            if let Some(agent_id) = bindings.get(session_id) {
                return Ok(agent_id.clone());
            }
        }

        if let Some(ref channel) = request.channel {
            let bindings = self.channel_bindings.read().await;
            if let Some(agent_id) = bindings.get(channel) {
                return Ok(agent_id.clone());
            }
        }

        let active = self.active_agent_id.read().await;
        if !active.is_empty() {
            return Ok(active.clone());
        }

        Err(WorkspaceError::agent_not_found(
            "No agent could be resolved",
        ))
    }

    pub async fn set_active(&self, agent_id: impl Into<String>) {
        *self.active_agent_id.write().await = agent_id.into();
    }

    pub async fn active_agent_id(&self) -> String {
        self.active_agent_id.read().await.clone()
    }

    pub async fn bind_session(&self, session_id: impl Into<String>, agent_id: impl Into<String>) {
        self.session_bindings
            .write()
            .await
            .insert(session_id.into(), agent_id.into());
    }

    pub async fn unbind_session(&self, session_id: &str) {
        self.session_bindings.write().await.remove(session_id);
    }

    pub async fn bind_channel(&self, channel: impl Into<String>, agent_id: impl Into<String>) {
        self.channel_bindings
            .write()
            .await
            .insert(channel.into(), agent_id.into());
    }

    pub async fn unbind_channel(&self, channel: &str) {
        self.channel_bindings.write().await.remove(channel);
    }
}

type CreationResults = Arc<std::sync::Mutex<HashMap<String, (u64, Option<WorkspaceError>)>>>;

pub struct DefaultWorkspaceRegistry {
    channel_managers: std::sync::RwLock<HashMap<String, Arc<rua_channel::core::ChannelManager>>>,
    creation_results: CreationResults,
    operation_gates: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    runtimes: Arc<RwLock<HashMap<String, Arc<WorkspaceRuntime>>>>,

    root: RuaRoot,

    profile: PlatformProfile,

    resolver: AgentResolver,

    ctx: Arc<WorkspaceContext>,

    known_agents: RwLock<Vec<(String, String)>>,

    drain_timeout: std::time::Duration,

    /// The process-composition owner of per-Agent Conversation and Task work.
    /// Every runtime this registry builds receives it, so an Agent that was
    /// created lazily still reports real work at shutdown.
    work_authority: std::sync::RwLock<Option<Arc<dyn AgentWorkAuthority>>>,
}

impl DefaultWorkspaceRegistry {
    pub fn new(
        root: RuaRoot,
        profile: PlatformProfile,
        active_agent_id: impl Into<String>,
        ctx: Arc<WorkspaceContext>,
    ) -> Self {
        let active = active_agent_id.into();
        Self {
            channel_managers: std::sync::RwLock::new(HashMap::new()),
            creation_results: Arc::new(std::sync::Mutex::new(HashMap::new())),
            operation_gates: std::sync::Mutex::new(HashMap::new()),
            runtimes: Arc::new(RwLock::new(HashMap::new())),
            root,
            profile,
            resolver: AgentResolver::new(&active),
            ctx,
            known_agents: RwLock::new(Vec::new()),
            drain_timeout: std::time::Duration::from_secs(60),
            work_authority: std::sync::RwLock::new(None),
        }
    }

    pub async fn register_agent(&self, agent_id: impl Into<String>, name: impl Into<String>) {
        self.known_agents
            .write()
            .await
            .push((agent_id.into(), name.into()));
    }

    fn operation_gate(&self, agent_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.operation_gates
                .lock()
                .expect("Registry operation gates poisoned")
                .entry(agent_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    pub fn resolver(&self) -> &AgentResolver {
        &self.resolver
    }

    pub fn ctx(&self) -> &Arc<WorkspaceContext> {
        &self.ctx
    }

    pub fn set_drain_timeout(&mut self, timeout: std::time::Duration) {
        self.drain_timeout = timeout;
    }

    /// Installs the work authority used by every runtime this registry creates
    /// and by every runtime it already holds.
    ///
    /// Registration is synchronous so a composition root can install the seam
    /// while it is still assembling the process, before any Agent exists. The
    /// index lock is only held for the duration of the swap, never across work.
    pub async fn set_work_authority(&self, authority: Arc<dyn AgentWorkAuthority>) {
        let runtimes: Vec<Arc<WorkspaceRuntime>> =
            self.runtimes.read().await.values().cloned().collect();
        for rt in runtimes {
            rt.set_work_authority(Arc::clone(&authority));
        }
        *self
            .work_authority
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(authority);
    }

    /// Installs the work authority for every runtime this registry creates later.
    ///
    /// Unlike [`Self::set_work_authority`] this needs no async context, so a
    /// composition root can install the seam while it is still assembling the
    /// process synchronously. It only records the default: runtimes that already
    /// exist keep the authority they were given, which is what makes
    /// [`Self::set_work_authority`] the tool for retrofitting live Agents.
    pub fn install_work_authority(&self, authority: Arc<dyn AgentWorkAuthority>) {
        *self
            .work_authority
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(authority);
    }

    pub fn install_channel_manager(
        &self,
        agent_id: &str,
        manager: Arc<rua_channel::core::ChannelManager>,
    ) {
        self.channel_managers
            .write()
            .expect("Channel composition poisoned")
            .insert(agent_id.to_string(), manager);
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_runtime(
        root: &RuaRoot,
        profile: PlatformProfile,
        ctx: Arc<WorkspaceContext>,
        authority: Option<Arc<dyn AgentWorkAuthority>>,
        channel_manager: Option<Arc<rua_channel::core::ChannelManager>>,
        runtimes: &RwLock<HashMap<String, Arc<WorkspaceRuntime>>>,
        drain_timeout: std::time::Duration,
        agent_id: &str,
        agent_name: &str,
    ) -> Result<Arc<WorkspaceRuntime>, WorkspaceError> {
        let workspace_root = root
            .agent_workspace(agent_id)
            .map_err(|error| WorkspaceError::path_invalid(error.to_string()))?;
        let rt = WorkspaceRuntime::new(agent_id, agent_name, workspace_root, profile, ctx);
        if let Some(manager) = channel_manager {
            rt.set_channel_manager(manager).await;
        }
        if let Some(authority) = authority {
            rt.set_work_authority(authority);
        }
        let rt = Arc::new(rt);
        let initialized = async {
            rt.initialize_for_platform().await?;
            if let Some(authority) = rt.work_authority_for_registry() {
                authority.resume(agent_id).await?;
            }
            Ok::<(), WorkspaceError>(())
        }
        .await;
        if let Err(error) = initialized {
            let report = rt
                .shutdown(ShutdownPolicy::CancelActive {
                    timeout: drain_timeout,
                })
                .await;
            if !report.cleanup_completed {
                runtimes
                    .write()
                    .await
                    .insert(agent_id.to_string(), Arc::clone(&rt));
            }
            return Err(error);
        }
        Ok(rt)
    }

    async fn find_agent_name(&self, agent_id: &str) -> Option<String> {
        let agents = self.known_agents.read().await;
        agents
            .iter()
            .find(|(id, _)| id == agent_id)
            .map(|(_, name)| name.clone())
    }
}

#[async_trait]
impl WorkspaceRegistry for DefaultWorkspaceRegistry {
    async fn get_or_create(&self, agent_id: &str) -> Result<Arc<WorkspaceRuntime>, WorkspaceError> {
        let observed = self
            .creation_results
            .lock()
            .expect("Registry creation results poisoned")
            .get(agent_id)
            .map_or(0, |(generation, _)| *generation);
        let gate = self.operation_gate(agent_id);
        let operation = Arc::clone(&gate).lock_owned().await;
        if let Some((generation, error)) = self
            .creation_results
            .lock()
            .expect("Registry creation results poisoned")
            .get(agent_id)
        {
            if *generation > observed {
                if let Some(error) = error {
                    return Err(error.clone());
                }
            }
        }
        {
            let runtimes = self.runtimes.read().await;
            if let Some(rt) = runtimes.get(agent_id) {
                if matches!(
                    rt.lifecycle_state().await,
                    crate::lifecycle::WorkspaceLifecycleState::Stopped
                        | crate::lifecycle::WorkspaceLifecycleState::Draining
                        | crate::lifecycle::WorkspaceLifecycleState::Failed
                ) {
                    return Err(WorkspaceError::component_init_failed(
                        agent_id,
                        "Runtime is unavailable; finish cleanup before creating another instance",
                    ));
                }
                return Ok(rt.clone());
            }
        }

        let name = self
            .find_agent_name(agent_id)
            .await
            .unwrap_or_else(|| agent_id.to_string());

        let root = self.root.clone();
        let profile = self.profile;
        let ctx = Arc::clone(&self.ctx);
        let authority = self
            .work_authority
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let runtimes = Arc::clone(&self.runtimes);
        let results = Arc::clone(&self.creation_results);
        let channel_manager = self
            .channel_managers
            .read()
            .expect("Channel composition poisoned")
            .get(agent_id)
            .cloned();
        let timeout = self.drain_timeout;
        let agent_id = agent_id.to_string();
        tokio::spawn(async move {
            let _operation = operation;
            let result = Self::build_runtime(
                &root,
                profile,
                ctx,
                authority,
                channel_manager,
                &runtimes,
                timeout,
                &agent_id,
                &name,
            )
            .await;
            results
                .lock()
                .expect("Registry creation results poisoned")
                .insert(
                    agent_id.clone(),
                    (observed + 1, result.as_ref().err().cloned()),
                );
            if let Ok(runtime) = result.as_ref() {
                runtimes.write().await.insert(agent_id, Arc::clone(runtime));
            }
            result
        })
        .await
        .map_err(|error| {
            WorkspaceError::internal(format!("Runtime initialization task failed: {error}"))
        })?
    }

    async fn get(&self, agent_id: &str) -> Option<Arc<WorkspaceRuntime>> {
        self.runtimes.read().await.get(agent_id).cloned()
    }

    async fn list(&self) -> Result<Vec<WorkspaceSummary>, WorkspaceError> {
        let runtimes = self.runtimes.read().await;
        let active_id = self.resolver.active_agent_id().await;

        let mut summaries = Vec::new();
        for (agent_id, rt) in runtimes.iter() {
            let is_active = *agent_id == active_id;
            summaries.push(rt.summary(is_active).await);
        }

        summaries.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        Ok(summaries)
    }

    async fn reload_agent(&self, agent_id: &str) -> Result<(), WorkspaceError> {
        let gate = self.operation_gate(agent_id);
        let operation = Arc::clone(&gate).lock_owned().await;
        {
            let runtimes = self.runtimes.read().await;
            if !runtimes.contains_key(agent_id) {
                return Err(WorkspaceError::agent_not_found(agent_id));
            }
        }

        let name = self
            .find_agent_name(agent_id)
            .await
            .unwrap_or_else(|| agent_id.to_string());

        let old_rt = self
            .get(agent_id)
            .await
            .ok_or_else(|| WorkspaceError::agent_not_found(agent_id))?;
        let channel_manager = self
            .channel_managers
            .read()
            .expect("Channel composition poisoned")
            .get(agent_id)
            .cloned();
        let root = self.root.clone();
        let profile = self.profile;
        let ctx = Arc::clone(&self.ctx);
        let authority = self
            .work_authority
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let runtimes = Arc::clone(&self.runtimes);
        let timeout = self.drain_timeout;
        let agent_id = agent_id.to_string();
        tokio::spawn(async move {
            let _operation = operation;
            let report = old_rt.shutdown(ShutdownPolicy::Graceful { timeout }).await;
            if !report.is_clean() {
                return Err(WorkspaceError::component_shutdown_failed(
                    &agent_id,
                    "old Runtime did not shut down cleanly",
                ));
            }
            let runtime = Self::build_runtime(
                &root,
                profile,
                ctx,
                authority,
                channel_manager,
                &runtimes,
                timeout,
                &agent_id,
                &name,
            )
            .await?;
            runtimes.write().await.insert(agent_id, runtime);
            Ok(())
        })
        .await
        .map_err(|error| {
            WorkspaceError::internal(format!("Runtime rebuild task failed: {error}"))
        })?
    }

    /// Removes one Agent and stops every resource it owns.
    ///
    /// Returns an error when the resources did not stop cleanly, because a caller
    /// that only deletes the index entry would otherwise believe the Agent is
    /// gone while its processes, sessions and work are still alive.
    async fn remove(&self, agent_id: &str) -> Result<(), WorkspaceError> {
        let report = self
            .remove_with_policy(agent_id, ShutdownPolicy::default())
            .await?;
        if report.is_clean() {
            Ok(())
        } else {
            Err(WorkspaceError::component_shutdown_failed(
                "workspace",
                format!(
                    "Agent '{agent_id}' did not stop cleanly: failed=[{}] unresolved_turns={} unresolved_tasks={} authority_confirmed={}",
                    report.failed_components.join(", "),
                    report.turns_unresolved,
                    report.tasks_unresolved,
                    report.authority_confirmed
                ),
            ))
        }
    }

    /// Removes one Agent and stops every resource it owns under `policy`.
    ///
    /// Shutdown runs outside the index lock under the Agent operation gate.
    /// Failed cleanup retains the owner for retry; only confirmed cleanup removes
    /// the instance. Other Agents remain independent.
    async fn remove_with_policy(
        &self,
        agent_id: &str,
        policy: ShutdownPolicy,
    ) -> Result<ShutdownReport, WorkspaceError> {
        let deadline = policy.deadline();
        let gate = self.operation_gate(agent_id);
        let _operation = tokio::time::timeout_at(deadline, gate.lock())
            .await
            .map_err(|_| {
                WorkspaceError::component_shutdown_failed(
                    agent_id,
                    "Timed out waiting for Agent operation",
                )
            })?;
        let rt = {
            let runtimes = self.runtimes.read().await;
            runtimes
                .get(agent_id)
                .cloned()
                .ok_or_else(|| WorkspaceError::agent_not_found(agent_id))?
        };

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let policy = match policy {
            ShutdownPolicy::Graceful { .. } => ShutdownPolicy::Graceful { timeout: remaining },
            ShutdownPolicy::CancelActive { .. } => {
                ShutdownPolicy::CancelActive { timeout: remaining }
            }
            ShutdownPolicy::Immediate => ShutdownPolicy::Immediate,
        };
        let report = rt.shutdown(policy).await;

        if report.is_clean() {
            let mut runtimes = self.runtimes.write().await;
            if runtimes
                .get(agent_id)
                .is_some_and(|current| Arc::ptr_eq(current, &rt))
            {
                runtimes.remove(agent_id);
            }
            tracing::info!(
                agent_id,
                turns_drained = report.turns_drained,
                turns_cancelled = report.turns_cancelled,
                tasks_cancelled = report.tasks_cancelled,
                "Agent workspace removed"
            );
        } else {
            tracing::warn!(
                agent_id,
                failed = ?report.failed_components,
                authority_confirmed = report.authority_confirmed,
                unresolved_turns = report.turns_unresolved,
                unresolved_tasks = report.tasks_unresolved,
                "Agent workspace removed without a clean shutdown"
            );
        }

        Ok(report)
    }

    /// Stops every Agent runtime under `policy`, continuing past failures.
    ///
    /// Resource shutdown does not hold the index lock. Failed owners remain
    /// indexed for retry. The returned report aggregates only observed outcomes: a
    /// component that failed, work the owner still holds, or a shutdown that ran
    /// past its deadline each keep the aggregate from reading as clean.
    async fn shutdown_all(&self, policy: ShutdownPolicy) -> Result<ShutdownReport, WorkspaceError> {
        let deadline = policy.deadline();
        let detached: Vec<(String, Arc<WorkspaceRuntime>)> = {
            let runtimes = self.runtimes.read().await;
            runtimes
                .iter()
                .map(|(id, runtime)| (id.clone(), Arc::clone(runtime)))
                .collect()
        };

        let mut report = ShutdownReport::default();
        let mut authority_failures = 0usize;
        // An empty registry is a clean aggregate; starting from `true` is what
        // makes that honest instead of vacuously false.
        let mut all_within_timeout = true;
        let mut all_cleanup_completed = true;

        for (agent_id, rt) in detached {
            rt.begin_drain().await;
            let gate = self.operation_gate(&agent_id);
            let _operation = match tokio::time::timeout_at(deadline, gate.lock()).await {
                Ok(operation) => operation,
                Err(_) => {
                    report.fail(format!("{agent_id}:operation_timeout"));
                    authority_failures += 1;
                    all_within_timeout = false;
                    all_cleanup_completed = false;
                    continue;
                }
            };
            if !self
                .runtimes
                .read()
                .await
                .get(&agent_id)
                .is_some_and(|current| Arc::ptr_eq(current, &rt))
            {
                continue;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let agent_policy = match policy {
                ShutdownPolicy::Graceful { .. } => ShutdownPolicy::Graceful { timeout: remaining },
                ShutdownPolicy::CancelActive { .. } => {
                    ShutdownPolicy::CancelActive { timeout: remaining }
                }
                ShutdownPolicy::Immediate => ShutdownPolicy::CancelActive { timeout: remaining },
            };
            let agent_report = rt.shutdown(agent_policy).await;
            if agent_report.is_clean() {
                let mut runtimes = self.runtimes.write().await;
                if runtimes
                    .get(&agent_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &rt))
                {
                    runtimes.remove(&agent_id);
                }
            }

            report.turns_drained += agent_report.turns_drained;
            report.turns_cancelled += agent_report.turns_cancelled;
            report.turns_unresolved += agent_report.turns_unresolved;
            report.tasks_cancelled += agent_report.tasks_cancelled;
            report.tasks_unresolved += agent_report.tasks_unresolved;
            report.queued_inputs_discarded += agent_report.queued_inputs_discarded;
            report.queued_inputs_unresolved += agent_report.queued_inputs_unresolved;
            all_within_timeout &= agent_report.finished_within_timeout;
            all_cleanup_completed &= agent_report.cleanup_completed;
            if agent_report
                .failed_components
                .iter()
                .any(|c| c == "work_authority")
            {
                authority_failures += 1;
            }
            for component in agent_report.failed_components {
                report.fail(format!("{agent_id}:{component}"));
            }

            tracing::info!(
                agent_id = %agent_id,
                turns_drained = agent_report.turns_drained,
                turns_cancelled = agent_report.turns_cancelled,
                tasks_cancelled = agent_report.tasks_cancelled,
                authority_confirmed = agent_report.authority_confirmed,
                "Agent workspace shut down"
            );
        }

        // An authority failure or leftover work in any Agent withholds the
        // confirmation for the whole shutdown; an empty registry holds nothing
        // and is therefore genuinely confirmed.
        report.authority_confirmed = authority_failures == 0
            && report.turns_unresolved == 0
            && report.tasks_unresolved == 0
            && report.queued_inputs_unresolved == 0;
        report.cleanup_completed = all_cleanup_completed;
        report.finished_within_timeout =
            all_within_timeout && tokio::time::Instant::now() <= deadline;

        Ok(report)
    }
}
