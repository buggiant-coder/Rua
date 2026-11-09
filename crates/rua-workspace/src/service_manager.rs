use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::error::WorkspaceError;
use crate::health::ComponentState;
use crate::lifecycle::InitPhase;

pub type ServicePriority = u32;

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct ImPlatformConfigs {
    pub dingtalk: Option<rua_config::DingTalkConfig>,
    pub wecom: Option<rua_config::WecomConfig>,
    pub feishu: Option<rua_config::FeishuConfig>,
    pub telegram: Option<rua_config::TelegramConfig>,
    pub discord: Option<rua_config::DiscordConfig>,
    pub onebot: Option<rua_config::OneBotConfig>,
    pub mattermost: Option<rua_config::MattermostConfig>,
    pub slack: Option<rua_config::SlackConfig>,
    pub sip: Option<rua_config::SipConfig>,
    pub qq: Option<rua_config::QqConfig>,
    pub matrix: Option<rua_config::MatrixConfig>,
    pub mqtt: Option<rua_config::MqttConfig>,
    pub wechat: Option<rua_config::WechatConfig>,
    pub xiaoyi: Option<rua_config::XiaoyiConfig>,
    pub yuanbao: Option<rua_config::YuanbaoConfig>,
    pub imessage: Option<rua_config::IMessageConfig>,
}

#[derive(Debug, Clone)]
pub struct ServiceDescriptor {
    pub name: String,

    pub priority: ServicePriority,

    pub phase: InitPhase,
}

#[async_trait]
pub trait ServiceLifecycle: Send + Sync {
    async fn start(&self) -> Result<(), WorkspaceError>;

    async fn stop(&self) -> Result<(), WorkspaceError>;
}

#[derive(Clone)]
struct ServiceEntry {
    descriptor: ServiceDescriptor,
    lifecycle: Option<Arc<dyn ServiceLifecycle>>,
    state: ComponentState,

    /// A real `start` was observed on this entry.
    ///
    /// `NotInitialized` alone cannot answer "was anything started here?", because
    /// it is both the state before a start and the state after a stop. Without
    /// this flag a second shutdown would re-stop an idle component, and a stop
    /// pass could skip a half-open one that never reached `Ready`.
    started_once: bool,
}

/// What a component stop pass actually did.
///
/// `stopped`, `failed` and `timed_out` are disjoint observations of the same
/// pass: a component appears in `stopped` only once its own `stop` returned
/// `Ok`, and in `timed_out` only when nothing confirmed it closed.
#[derive(Debug, Clone, Default)]
pub struct ServiceStopOutcome {
    /// Capabilities whose stop was invoked.
    pub attempted: u64,

    /// Capabilities confirmed stopped.
    pub stopped: Vec<String>,

    /// Capabilities that reported a failure or exceeded their bound.
    pub failed: Vec<String>,

    /// Capabilities that exceeded their bound and remain owned.
    pub timed_out: Vec<String>,
}

impl ServiceStopOutcome {
    /// Every capability that was not confirmed stopped.
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }
}

pub struct ServiceManager {
    operation_gate: tokio::sync::Mutex<()>,
    services: RwLock<Vec<ServiceEntry>>,
}

impl ServiceManager {
    pub fn new() -> Self {
        Self {
            operation_gate: tokio::sync::Mutex::new(()),
            services: RwLock::new(Vec::new()),
        }
    }

    /// Builds the descriptor table without any lifecycle. Descriptors describe
    /// which capabilities the workspace declares; they are not evidence that a
    /// resource exists. Only a registered lifecycle can start real work.
    pub fn with_descriptors(entries: Vec<(ServiceDescriptor, ComponentState)>) -> Self {
        let mut services: Vec<ServiceEntry> = entries
            .into_iter()
            .map(|(descriptor, state)| ServiceEntry {
                descriptor,
                lifecycle: None,
                state,
                started_once: false,
            })
            .collect();
        services.sort_by_key(|e| e.descriptor.priority);
        Self {
            operation_gate: tokio::sync::Mutex::new(()),
            services: RwLock::new(services),
        }
    }

    pub async fn start_phase(&self, phase: InitPhase) -> Result<(), WorkspaceError> {
        let _operation = self.operation_gate.lock().await;
        let mut services = self.services.read().await.clone();
        let mut first_error: Option<WorkspaceError> = None;

        for entry in services.iter_mut() {
            if entry.descriptor.phase != phase {
                continue;
            }
            if matches!(
                entry.state,
                ComponentState::Ready | ComponentState::Disabled
            ) {
                continue;
            }

            if entry.lifecycle.is_none() {
                tracing::debug!(
                    service = %entry.descriptor.name,
                    "Service is not assembled; leaving state untouched"
                );
                continue;
            }

            entry.state = ComponentState::Initializing;
            self.publish_entry(entry).await;
            let result = Self::start_entry(entry).await;
            self.publish_entry(entry).await;
            if let Err(error) = result {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        if let Some(e) = first_error {
            Err(e)
        } else {
            Ok(())
        }
    }

    /// Stops every assembled capability this Agent owns.
    ///
    /// A capability that failed to start may still hold a half-open resource, so
    /// it is stopped too: `Failed` means "not usable", never "nothing to clean
    /// up". Capabilities that were never assembled (no lifecycle), were excluded
    /// by the platform profile, or are back in `NotInitialized` are skipped: the
    /// first two were never started, and the third has already been stopped, so
    /// stopping it again would be a second stop of one resource.
    pub async fn stop_all(&self) -> Vec<String> {
        self.stop_owned(tokio::time::Instant::now() + std::time::Duration::from_secs(30))
            .await
            .failed
    }

    /// Stops every owned capability, bounding each `stop` call.
    ///
    /// The whole set is attempted even after a stop fails, and an entry whose
    /// stop exceeds its bound is kept in a state that a later shutdown retries
    /// rather than being dropped as if it were closed.
    pub async fn stop_owned(&self, deadline: tokio::time::Instant) -> ServiceStopOutcome {
        let _operation = match tokio::time::timeout_at(deadline, self.operation_gate.lock()).await {
            Ok(operation) => operation,
            Err(_) => {
                return ServiceStopOutcome {
                    failed: vec!["service_operation_timeout".to_string()],
                    timed_out: vec!["service_operation_timeout".to_string()],
                    ..ServiceStopOutcome::default()
                }
            }
        };
        let mut services = self.services.read().await.clone();
        let mut outcome = ServiceStopOutcome::default();

        for entry in services.iter_mut().rev() {
            // Only a component that was actually started (or that is in a state
            // proving it holds something) is stopped. `NotInitialized` after a
            // real start means the previous stop already succeeded.
            let holds_resource = entry.started_once
                || matches!(
                    entry.state,
                    ComponentState::Ready
                        | ComponentState::Initializing
                        | ComponentState::Degraded { .. }
                        | ComponentState::Failed { .. }
                );
            if !holds_resource {
                continue;
            }
            let Some(lifecycle) = entry.lifecycle.as_ref() else {
                continue;
            };
            outcome.attempted += 1;
            tracing::debug!(service = %entry.descriptor.name, "Stopping service");
            let per_component = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout_at(deadline, lifecycle.stop()).await {
                Ok(Ok(())) => {
                    entry.state = ComponentState::NotInitialized;
                    // The resource is confirmed closed, so a repeat shutdown must
                    // not stop it a second time.
                    entry.started_once = false;
                    outcome.stopped.push(entry.descriptor.name.clone());
                    tracing::info!(service = %entry.descriptor.name, "Service stopped");
                }
                Ok(Err(error)) => {
                    entry.state = ComponentState::Failed {
                        reason: error.message.clone(),
                    };
                    outcome.failed.push(entry.descriptor.name.clone());
                    tracing::error!(
                        service = %entry.descriptor.name,
                        error = %error.message,
                        "Service failed to stop"
                    );
                }
                Err(_) => {
                    // Not stopped, not closed, and still owned: keep it visible so a
                    // later shutdown can retry instead of leaking it silently.
                    entry.state = ComponentState::Failed {
                        reason: format!(
                            "stop did not finish within {}s",
                            per_component.as_secs_f64()
                        ),
                    };
                    outcome.timed_out.push(entry.descriptor.name.clone());
                    outcome.failed.push(entry.descriptor.name.clone());
                    tracing::error!(
                        service = %entry.descriptor.name,
                        "Service stop exceeded its bound"
                    );
                }
            }
        }

        for entry in &services {
            self.publish_entry(entry).await;
        }
        outcome
    }

    pub async fn service_state(&self, name: &str) -> Option<ComponentState> {
        let services = self.services.read().await;
        services
            .iter()
            .find(|e| e.descriptor.name == name)
            .map(|e| e.state.clone())
    }

    pub async fn component_states(&self) -> HashMap<String, ComponentState> {
        let services = self.services.read().await;
        services
            .iter()
            .map(|e| (e.descriptor.name.clone(), e.state.clone()))
            .collect()
    }

    /// Whether any assembled capability might still be holding a resource.
    ///
    /// A capability counts as still-held unless its stop was confirmed (which
    /// leaves it `NotInitialized`) or the platform profile excluded it. This is
    /// the evidence behind `cleanup_completed`: a component that failed to stop
    /// or exceeded its bound keeps showing up here so a later shutdown retries.
    pub async fn has_unstopped(&self) -> bool {
        let services = self.services.read().await;
        services.iter().any(|entry| {
            entry.lifecycle.is_some()
                && entry.started_once
                && !matches!(
                    entry.state,
                    ComponentState::NotInitialized | ComponentState::Disabled
                )
        })
    }

    pub async fn failed_count(&self) -> usize {
        let services = self.services.read().await;
        services
            .iter()
            .filter(|e| matches!(e.state, ComponentState::Failed { .. }))
            .count()
    }

    async fn publish_entry(&self, entry: &ServiceEntry) {
        if let Some(current) = self
            .services
            .write()
            .await
            .iter_mut()
            .find(|current| current.descriptor.name == entry.descriptor.name)
        {
            *current = entry.clone();
        }
    }

    async fn start_entry(entry: &mut ServiceEntry) -> Result<(), WorkspaceError> {
        let Some(lifecycle) = entry.lifecycle.as_ref() else {
            return Ok(());
        };
        entry.state = ComponentState::Initializing;
        entry.started_once = true;
        tracing::debug!(service = %entry.descriptor.name, "Starting service");

        match lifecycle.start().await {
            Ok(()) => {
                entry.state = ComponentState::Ready;
                tracing::info!(service = %entry.descriptor.name, "Service started");
                Ok(())
            }
            Err(error) => {
                let reason = error.message.clone();
                entry.state = ComponentState::Failed {
                    reason: reason.clone(),
                };
                tracing::error!(
                    service = %entry.descriptor.name,
                    error = %reason,
                    "Service failed to start"
                );
                Err(error)
            }
        }
    }

    /// Attaches the real lifecycle of a capability that physically exists.
    ///
    /// Disabled capabilities keep their `Disabled` state: attaching a lifecycle
    /// does not resurrect a capability the platform profile excluded.
    pub async fn replace_lifecycle(&self, name: &str, lifecycle: Arc<dyn ServiceLifecycle>) {
        let _operation = self.operation_gate.lock().await;
        let previous = self
            .services
            .read()
            .await
            .iter()
            .find(|entry| entry.descriptor.name == name)
            .cloned();
        if let Some(previous) = previous.filter(|entry| entry.started_once) {
            if let Some(old) = previous.lifecycle.as_ref() {
                if !matches!(
                    tokio::time::timeout(std::time::Duration::from_secs(30), old.stop()).await,
                    Ok(Ok(()))
                ) {
                    let mut failed = previous;
                    failed.state = ComponentState::Failed {
                        reason: "previous lifecycle could not be stopped for replacement"
                            .to_string(),
                    };
                    self.publish_entry(&failed).await;
                    return;
                }
            }
        }
        let mut services = self.services.write().await;
        match services.iter_mut().find(|e| e.descriptor.name == name) {
            Some(entry) => {
                entry.lifecycle = Some(lifecycle);
                entry.started_once = false;
                if matches!(entry.state, ComponentState::Disabled) {
                    // A disabled capability stays disabled.
                } else if !entry.started_once {
                    entry.state = ComponentState::NotInitialized;
                }
                // Otherwise keep the observed state: attaching a lifecycle does
                // not undo a start that already happened.
            }
            None => {
                tracing::warn!(
                    service = %name,
                    "replace_lifecycle: no matching service registered"
                );
            }
        }
    }

    /// Attaches a lifecycle only when the capability has none yet.
    ///
    /// `WorkspaceRuntime` uses this for the resources it assembles itself, so an
    /// owner that already registered a lifecycle (a test double or an embedding
    /// composition root) keeps control of that capability.
    pub async fn register_lifecycle_if_absent(
        &self,
        name: &str,
        lifecycle: Arc<dyn ServiceLifecycle>,
    ) -> bool {
        let _operation = self.operation_gate.lock().await;
        let mut services = self.services.write().await;
        let Some(entry) = services.iter_mut().find(|e| e.descriptor.name == name) else {
            tracing::warn!(
                service = %name,
                "register_lifecycle_if_absent: no matching service registered"
            );
            return false;
        };
        if entry.lifecycle.is_some() {
            return false;
        }
        entry.lifecycle = Some(lifecycle);
        if matches!(entry.state, ComponentState::Disabled) {
            return true;
        }
        entry.state = ComponentState::NotInitialized;
        true
    }

    /// Registers the per-Agent agent lifecycle.
    pub async fn apply_channels<F, Fut>(&self, apply: F) -> Result<(), WorkspaceError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<(), WorkspaceError>> + Send,
    {
        let _operation = self.operation_gate.lock().await;
        let mut entry = self
            .services
            .read()
            .await
            .iter()
            .find(|entry| entry.descriptor.name == "channels")
            .cloned()
            .ok_or_else(|| {
                WorkspaceError::component_init_failed(
                    "channels",
                    "Channel lifecycle is unavailable",
                )
            })?;
        if matches!(entry.state, ComponentState::Disabled) {
            return Err(WorkspaceError::mobile_policy_denied(
                "Channel runtime is disabled on this platform",
            ));
        }
        entry.state = ComponentState::Initializing;
        entry.started_once = true;
        self.publish_entry(&entry).await;
        let result = apply().await;
        entry.state = if result.is_ok() {
            ComponentState::Ready
        } else {
            ComponentState::Failed {
                reason: "Saved Channel configuration could not be applied".to_string(),
            }
        };
        self.publish_entry(&entry).await;
        result
    }

    pub async fn apply_mcp(
        &self,
        manager: Arc<dyn mcp_manager::manager::McpManager>,
        config: mcp_manager::types::McpConfig,
    ) -> Result<mcp_manager::types::McpReloadReport, WorkspaceError> {
        let _operation = self.operation_gate.lock().await;
        let mut entry = self
            .services
            .read()
            .await
            .iter()
            .find(|entry| entry.descriptor.name == "mcp")
            .cloned()
            .ok_or_else(|| {
                WorkspaceError::component_init_failed("mcp", "MCP lifecycle is unavailable")
            })?;
        entry.state = ComponentState::Initializing;
        entry.started_once = true;
        self.publish_entry(&entry).await;
        let result = manager.reload(config).await;
        match result {
            Ok(report) => {
                entry.state = if report.errors.is_empty() {
                    ComponentState::Ready
                } else {
                    ComponentState::Failed {
                        reason: "Saved MCP configuration could not be applied".to_string(),
                    }
                };
                self.publish_entry(&entry).await;
                Ok(report)
            }
            Err(error) => {
                entry.state = ComponentState::Failed {
                    reason: "Saved MCP configuration could not be applied".to_string(),
                };
                self.publish_entry(&entry).await;
                Err(WorkspaceError::component_init_failed(
                    "mcp",
                    error.to_string(),
                ))
            }
        }
    }

    ///
    /// `provider` has no lifecycle: it is a process-shared resource owned by
    /// the composition root, so this workspace never starts or stops it.
    pub async fn register_agent_service(
        &self,
        agent_id: rua_core::id::AgentId,
        agent_runtime: Arc<dyn rua_contract::agent::AgentRuntime>,
    ) {
        let agent_adapter: Arc<dyn ServiceLifecycle> = Arc::new(
            crate::services::AgentServiceAdapter::new(agent_runtime, agent_id),
        );
        self.replace_lifecycle("agent", agent_adapter).await;
    }

    /// Registers the resources with real per-Agent start/stop behaviour.
    ///
    /// The toolkit has no lifecycle of its own: it is constructed as part of the
    /// React runtime, so declaring `tools` Ready would not prove anything.
    pub async fn register_runtime_services(
        &self,
        mcp_manager: Arc<dyn mcp_manager::manager::McpManager>,
        mcp_initial_config: Option<mcp_manager::types::McpConfig>,
        memory_manager: Arc<dyn rua_agent::memory_manager::MemoryManager>,
    ) {
        let mcp_adapter: Arc<dyn ServiceLifecycle> = Arc::new(
            crate::services::McpServiceAdapter::new(mcp_manager, mcp_initial_config),
        );
        let memory_adapter: Arc<dyn ServiceLifecycle> =
            Arc::new(crate::services::MemoryServiceAdapter::new(memory_manager));

        self.replace_lifecycle("mcp", mcp_adapter).await;
        self.replace_lifecycle("memory", memory_adapter).await;
    }

    /// Registers the desktop platform capabilities owned by this workspace.
    ///
    /// Registration is not startup: callers still drive `start_phase`. `cron`
    /// stays unassembled because its scheduler loop belongs to the process
    /// composition root, not to a single Agent.
    pub async fn register_platform_services(
        &self,
        profile: crate::lifecycle::PlatformProfile,
        skills_service: std::sync::Arc<rua_skills::SkillService>,
        channel_manager: std::sync::Arc<rua_channel::core::ChannelManager>,
        im_configs: ImPlatformConfigs,
        react_runtime: Option<std::sync::Arc<dyn rua_contract::agent::AgentRuntime>>,
    ) {
        let ImPlatformConfigs {
            dingtalk: dingtalk_config,
            wecom: wecom_config,
            feishu: feishu_config,
            telegram: telegram_config,
            discord: discord_config,
            onebot: onebot_config,
            mattermost: mattermost_config,
            slack: slack_config,
            sip: sip_config,
            qq: qq_config,
            matrix: matrix_config,
            mqtt: mqtt_config,
            wechat: wechat_config,
            xiaoyi: xiaoyi_config,
            yuanbao: yuanbao_config,
            imessage: imessage_config,
        } = im_configs;

        if profile != crate::lifecycle::PlatformProfile::Desktop {
            tracing::debug!(
                ?profile,
                "register_platform_services skipped on non-desktop profile"
            );
            return;
        }

        if let Some(ref react_rt) = react_runtime {
            channel_manager
                .set_process(Arc::new(
                    crate::channel_process_adapter::ChannelProcessAdapter::new(Arc::clone(
                        react_rt,
                    ))
                    .with_workspace_dir(skills_service.workspace_dir().to_path_buf())
                    .with_memory_write_available(true),
                ))
                .await;
        }
        let mut adapter = crate::services::ChannelsServiceAdapter::new(channel_manager);
        if let Some(cfg) = dingtalk_config {
            adapter = adapter.with_dingtalk_config(cfg);
        }
        if let Some(cfg) = wecom_config {
            adapter = adapter.with_wecom_config(cfg);
        }
        if let Some(cfg) = feishu_config {
            adapter = adapter.with_feishu_config(cfg);
        }
        if let Some(cfg) = telegram_config {
            adapter = adapter.with_telegram_config(cfg);
        }
        if let Some(cfg) = discord_config {
            adapter = adapter.with_discord_config(cfg);
        }
        if let Some(cfg) = onebot_config {
            adapter = adapter.with_onebot_config(cfg);
        }
        if let Some(cfg) = mattermost_config {
            adapter = adapter.with_mattermost_config(cfg);
        }
        if let Some(cfg) = slack_config {
            adapter = adapter.with_slack_config(cfg);
        }
        if let Some(cfg) = sip_config {
            adapter = adapter.with_sip_config(cfg);
        }
        if let Some(cfg) = qq_config {
            adapter = adapter.with_qq_config(cfg);
        }
        if let Some(cfg) = matrix_config {
            adapter = adapter.with_matrix_config(cfg);
        }
        if let Some(cfg) = mqtt_config {
            adapter = adapter.with_mqtt_config(cfg);
        }
        if let Some(cfg) = wechat_config {
            adapter = adapter.with_wechat_config(cfg);
        }
        if let Some(cfg) = xiaoyi_config {
            adapter = adapter.with_xiaoyi_config(cfg);
        }
        if let Some(cfg) = yuanbao_config {
            adapter = adapter.with_yuanbao_config(cfg);
        }
        if let Some(cfg) = imessage_config {
            adapter = adapter.with_imessage_config(cfg);
        }
        let channels_adapter: Arc<dyn ServiceLifecycle> = Arc::new(adapter);
        let skills_adapter: Arc<dyn ServiceLifecycle> =
            Arc::new(crate::services::SkillsServiceAdapter::new(skills_service));

        self.replace_lifecycle("channels", channels_adapter).await;
        self.replace_lifecycle("skills", skills_adapter).await;
    }
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ActiveTaskTracker {
    tasks: RwLock<HashMap<String, TrackedTask>>,
    count: std::sync::atomic::AtomicUsize,
}

struct TrackedTask {
    name: String,
    cancel: CancellationToken,
}

impl ActiveTaskTracker {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub async fn register(&self, task_id: String, name: String) -> CancellationToken {
        let mut tasks = self.tasks.write().await;
        if let Some(existing) = tasks.get(&task_id) {
            return existing.cancel.clone();
        }

        let cancel = CancellationToken::new();
        tasks.insert(
            task_id,
            TrackedTask {
                name,
                cancel: cancel.clone(),
            },
        );
        self.count
            .store(tasks.len(), std::sync::atomic::Ordering::Relaxed);
        cancel
    }

    pub async fn complete(&self, task_id: &str) {
        let mut tasks = self.tasks.write().await;
        tasks.remove(task_id);
        self.count
            .store(tasks.len(), std::sync::atomic::Ordering::Relaxed);
    }

    pub async fn cancel(&self, task_id: &str) -> bool {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.get(task_id) {
            task.cancel.cancel();
            true
        } else {
            false
        }
    }

    pub async fn cancel_all(&self) -> usize {
        let tasks = self.tasks.read().await;
        let count = tasks.len();
        for task in tasks.values() {
            task.cancel.cancel();
        }
        count
    }

    pub fn active_count(&self) -> usize {
        self.count.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub async fn active_task_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tasks
            .read()
            .await
            .values()
            .map(|t| t.name.clone())
            .collect();
        names.sort();
        names
    }

    pub async fn drain(&self, timeout: std::time::Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.active_count() == 0 {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}

impl Default for ActiveTaskTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Descriptors for the capabilities an Agent workspace accounts for.
///
/// A descriptor only declares that the capability exists and where it sits in
/// the phase order. It never implies that a resource has been started: only a
/// registered [`ServiceLifecycle`] can move a state to `Ready`.
pub fn default_service_descriptors() -> Vec<ServiceDescriptor> {
    vec![
        ServiceDescriptor {
            name: "agent".into(),
            priority: 25,
            // The per-Agent runtime is assembled during the Runtime phase, so
            // warmup belongs to that phase rather than to Core.
            phase: InitPhase::Runtime,
        },
        ServiceDescriptor {
            name: "mcp".into(),
            priority: 40,
            phase: InitPhase::Runtime,
        },
        ServiceDescriptor {
            name: "memory".into(),
            priority: 51,
            phase: InitPhase::Runtime,
        },
    ]
}

pub fn platform_service_descriptors() -> Vec<ServiceDescriptor> {
    vec![
        ServiceDescriptor {
            name: "channels".into(),
            priority: 45,
            phase: InitPhase::Runtime,
        },
        ServiceDescriptor {
            name: "skills".into(),
            priority: 48,
            phase: InitPhase::Runtime,
        },
    ]
}
