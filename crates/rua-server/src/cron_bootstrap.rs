use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use rua_channel::core::{ChannelManager, ChannelTarget};
use rua_config::types::agents::HeartbeatConfig as AgentHeartbeatConfig;
use rua_config::types::cron::CronJobSpec;
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime};
use rua_contract::cron::SystemScheduler;
use rua_core::event::RuntimeEvent;
use rua_core::id::{AgentId, SessionId};
use rua_core::message::ContentPart;
use rua_core::RuaError;
use rua_cron::{
    compute_next_run_for_schedule, decide_misfire, spec_to_cron_job, CronExecutor,
    JsonCronJobRepository, MisfireDecision, RuntimeEventStream, TokioScheduler,
};
use rua_workspace::WorkspaceRegistry;

use crate::services::cron_dispatch_history::record_cron_dispatch_result;

pub struct WorkspaceCronChannelSender {
    registry: Arc<dyn WorkspaceRegistry>,
    push_store: Option<crate::endpoints::messages::PushMessageStore>,
}

impl WorkspaceCronChannelSender {
    pub fn new(registry: Arc<dyn WorkspaceRegistry>) -> Self {
        Self {
            registry,
            push_store: None,
        }
    }

    pub fn with_push_store(
        mut self,
        push_store: crate::endpoints::messages::PushMessageStore,
    ) -> Self {
        self.push_store = Some(push_store);
        self
    }

    async fn channel_manager_for(
        &self,
        agent_id: &str,
        operation: &str,
    ) -> std::result::Result<Arc<ChannelManager>, RuaError> {
        let runtime = self
            .registry
            .get_or_create(agent_id)
            .await
            .map_err(|e| RuaError::Cron {
                job_id: format!("channel:{agent_id}"),
                message: format!("failed to obtain workspace runtime for cron {operation}: {e}"),
            })?;
        runtime
            .channel_manager()
            .await
            .ok_or_else(|| RuaError::Cron {
                job_id: format!("channel:{agent_id}"),
                message: format!("channel manager unavailable for agent {agent_id}"),
            })
    }
}

#[async_trait::async_trait]
impl rua_cron::CronChannelSender for WorkspaceCronChannelSender {
    async fn send_text(
        &self,
        agent_id: &str,
        channel: &str,
        user_id: &str,
        session_id: &str,
        text: &str,
    ) -> std::result::Result<(), rua_core::RuaError> {
        if channel == "console" {
            return self
                .save_result_to_inbox(agent_id, channel, user_id, session_id, "", text)
                .await;
        }
        let channel_manager = self.channel_manager_for(agent_id, "text delivery").await?;
        let target = ChannelTarget {
            agent_id: agent_id.to_string(),
            conversation_id: session_id.to_string(),
            sender_id: Some(user_id.to_string()),
            reply_to: None,
            metadata: serde_json::Value::Null,
        };
        channel_manager
            .send_text(channel, target, text.to_string())
            .await
    }

    async fn send_event(
        &self,
        agent_id: &str,
        channel: &str,
        user_id: &str,
        session_id: &str,
        event: &rua_core::RuntimeEvent,
    ) -> std::result::Result<(), rua_core::RuaError> {
        if channel == "console" {
            match event {
                RuntimeEvent::TextDelta { delta, .. } if !delta.is_empty() => {
                    return self
                        .send_text(agent_id, channel, user_id, session_id, delta)
                        .await;
                }
                RuntimeEvent::Error { error } => {
                    return self
                        .send_text(agent_id, channel, user_id, session_id, &error.to_string())
                        .await;
                }
                RuntimeEvent::AskUser { .. }
                | RuntimeEvent::ToolApprovalRequest { .. }
                | RuntimeEvent::ToolApprovalPending { .. } => {
                    return Err(RuaError::Cron { job_id: format!("channel:{agent_id}"), message: "Cron Console notifications do not provide an interactive approval/control surface".into() });
                }
                _ => return Ok(()),
            }
        }
        let channel_manager = self.channel_manager_for(agent_id, "event delivery").await?;
        let target = ChannelTarget {
            agent_id: agent_id.to_string(),
            conversation_id: session_id.to_string(),
            sender_id: Some(user_id.to_string()),
            reply_to: None,
            metadata: serde_json::Value::Null,
        };
        channel_manager
            .send_stream_event(channel, target, event.clone())
            .await
    }

    async fn save_result_to_inbox(
        &self,
        agent_id: &str,
        channel: &str,
        user_id: &str,
        session_id: &str,
        job_name: &str,
        text: &str,
    ) -> std::result::Result<(), rua_core::RuaError> {
        let store = self.push_store.as_ref().ok_or_else(|| RuaError::Cron {
            job_id: format!("channel:{agent_id}"),
            message: "Cron inbox storage is unavailable".into(),
        })?;
        let mut extra = serde_json::Map::new();
        extra.insert(
            "agent_id".into(),
            serde_json::Value::String(agent_id.to_string()),
        );
        extra.insert(
            "channel".into(),
            serde_json::Value::String(channel.to_string()),
        );
        extra.insert(
            "session_id".into(),
            serde_json::Value::String(session_id.to_string()),
        );
        extra.insert(
            "user_id".into(),
            serde_json::Value::String(user_id.to_string()),
        );
        extra.insert(
            "source".into(),
            serde_json::Value::String("cron".to_string()),
        );
        extra.insert(
            "job_name".into(),
            serde_json::Value::String(job_name.to_string()),
        );
        store
            .push(crate::endpoints::dto::PushMessage {
                id: format!("cron:{agent_id}:{session_id}:{}", uuid::Uuid::new_v4()),
                content: text.to_string(),
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
                extra,
            })
            .await;
        Ok(())
    }
}

pub(crate) struct WorkspaceCronAgentRunner {
    registry: Arc<dyn WorkspaceRegistry>,
}

impl WorkspaceCronAgentRunner {
    pub(crate) fn new(registry: Arc<dyn WorkspaceRegistry>) -> Self {
        Self { registry }
    }

    async fn runtime_for_agent(
        &self,
        agent_id: &str,
    ) -> std::result::Result<(Arc<dyn AgentRuntime>, PathBuf), RuaError> {
        let workspace =
            self.registry
                .get_or_create(agent_id)
                .await
                .map_err(|e| RuaError::Cron {
                    job_id: format!("agent:{agent_id}"),
                    message: format!("failed to obtain workspace runtime for cron agent job: {e}"),
                })?;
        let workspace_dir = workspace.workspace_dir();
        let runtime = workspace
            .react_runtime()
            .await
            .ok_or_else(|| RuaError::Cron {
                job_id: format!("agent:{agent_id}"),
                message: format!("react runtime unavailable for agent {agent_id}"),
            })?;
        Ok((runtime, workspace_dir))
    }

    fn build_request(
        agent_id: &str,
        user_id: &str,
        session_id: &str,
        input: &str,
        workspace_dir: PathBuf,
        options: rua_cron::CronAgentRunOptions,
    ) -> AgentRequest {
        let mut metadata = HashMap::new();
        metadata.insert(
            "source".to_string(),
            serde_json::Value::String("cron".to_string()),
        );
        metadata.insert(
            "cron_tool_safety".to_string(),
            serde_json::Value::Bool(options.tool_safety),
        );
        metadata.insert(
            "cron_approval_mode".to_string(),
            serde_json::Value::String(if options.tool_safety { "auto" } else { "off" }.to_string()),
        );
        AgentRequest {
            session_id: SessionId::new(session_id),
            user_id: user_id.to_string(),
            agent_id: AgentId::new(agent_id),
            messages: vec![AgentInputMessage {
                role: "user".to_string(),
                content: vec![ContentPart::Text {
                    text: input.to_string(),
                }],
            }],
            model_override: None,
            tool_mode: "auto".to_string(),
            attachments: vec![],
            display_preferences: None,
            disabled_tools: Vec::new(),
            metadata,
            cancel: None,
            run_control: None,
            workspace_dir: Some(workspace_dir),
            conversation_store_dir: None,
            working_dir: None,
        }
    }
}

#[async_trait::async_trait]
impl rua_cron::CronAgentRunner for WorkspaceCronAgentRunner {
    async fn run_query(
        &self,
        agent_id: &str,
        user_id: &str,
        session_id: &str,
        input: &str,
    ) -> std::result::Result<String, RuaError> {
        let mut stream = self
            .stream_query(agent_id, user_id, session_id, input)
            .await?;
        let mut response = String::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(RuntimeEvent::TextDelta { delta, .. }) => response.push_str(&delta),
                Ok(RuntimeEvent::Error { error }) => return Err(error),
                Ok(_) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(response)
    }

    async fn stream_query(
        &self,
        agent_id: &str,
        user_id: &str,
        session_id: &str,
        input: &str,
    ) -> std::result::Result<RuntimeEventStream, RuaError> {
        let (runtime, workspace_dir) = self.runtime_for_agent(agent_id).await?;
        let request = Self::build_request(
            agent_id,
            user_id,
            session_id,
            input,
            workspace_dir,
            rua_cron::CronAgentRunOptions { tool_safety: false },
        );
        runtime.stream(request).await
    }

    async fn stream_query_with_options(
        &self,
        agent_id: &str,
        user_id: &str,
        session_id: &str,
        input: &str,
        options: rua_cron::CronAgentRunOptions,
    ) -> std::result::Result<RuntimeEventStream, RuaError> {
        let (runtime, workspace_dir) = self.runtime_for_agent(agent_id).await?;
        let request =
            Self::build_request(agent_id, user_id, session_id, input, workspace_dir, options);
        runtime.stream(request).await
    }
}

pub(crate) fn spawn_cron_scheduler(
    data_dir: PathBuf,
    registry: Arc<dyn WorkspaceRegistry>,
    scheduler: Arc<TokioScheduler>,
    fire_rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    push_store: crate::endpoints::messages::PushMessageStore,
) {
    tokio::spawn(async move {
        run_cron_scheduler(data_dir, registry, scheduler, fire_rx, push_store).await;
    });
}

#[derive(Clone)]
enum ScheduledJob {
    User(CronJobSpec),
    Heartbeat(CronJobSpec),
    Dream(CronJobSpec),
}

impl ScheduledJob {
    fn spec(&self) -> &CronJobSpec {
        match self {
            Self::User(spec) | Self::Heartbeat(spec) | Self::Dream(spec) => spec,
        }
    }
}

fn load_schedulable_jobs(data_dir: &std::path::Path) -> Result<Vec<ScheduledJob>, String> {
    let mut jobs = Vec::new();
    let timezone = load_user_timezone(data_dir)?;
    let root = rua_config::load_config(&data_dir.join("config.json"), data_dir)
        .map_err(|e| e.to_string())?;
    for agent_id in root.agents.profiles.keys() {
        let config =
            rua_config::load_agent_config(agent_id, data_dir).map_err(|e| e.to_string())?;
        let path = std::path::PathBuf::from(&config.workspace_dir);
        let jobs_path = path.join("jobs.json");
        let repo = JsonCronJobRepository::new(&jobs_path);
        {
            let loaded = repo
                .load_jobs()
                .map_err(|e| format!("load jobs {}: {e}", jobs_path.display()))?;
            for job in loaded {
                if job.enabled {
                    if job.agent_id.as_deref().unwrap_or_default().is_empty() {
                        return Err(format!("Cron job '{}' has no agent_id", job.id));
                    }
                    jobs.push(ScheduledJob::User(job));
                }
            }
        }

        {
            if let Some(heartbeat) = config
                .heartbeat
                .clone()
                .map(serde_json::from_value::<AgentHeartbeatConfig>)
                .transpose()
                .map_err(|e| format!("Agent '{agent_id}' heartbeat config: {e}"))?
            {
                if let Some(job) = rua_cron::build_heartbeat_job_spec(
                    &config.id,
                    &heartbeat,
                    &path,
                    &timezone,
                    config.last_dispatch.as_ref(),
                ) {
                    jobs.push(ScheduledJob::Heartbeat(job));
                }
            }

            if let Some(job) =
                rua_cron::build_dream_job_spec(&config.id, &config.running, &timezone)
            {
                jobs.push(ScheduledJob::Dream(job));
            }
        }
    }
    Ok(jobs)
}

fn load_user_timezone(data_dir: &std::path::Path) -> Result<String, String> {
    Ok(
        rua_config::loader::load_config(&data_dir.join("config.json"), data_dir)
            .map_err(|e| e.to_string())?
            .user_timezone
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(crate::user_timezone::detected_user_timezone),
    )
}

async fn run_cron_scheduler(
    data_dir: PathBuf,
    registry: Arc<dyn WorkspaceRegistry>,
    scheduler: Arc<TokioScheduler>,
    mut fire_rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    push_store: crate::endpoints::messages::PushMessageStore,
) {
    let ws_dir = data_dir.join("workspaces");

    let channel_sender: Arc<dyn rua_cron::CronChannelSender> = Arc::new(
        WorkspaceCronChannelSender::new(Arc::clone(&registry)).with_push_store(push_store),
    );
    let agent_runner: Arc<dyn rua_cron::CronAgentRunner> =
        Arc::new(WorkspaceCronAgentRunner::new(Arc::clone(&registry)));
    let executor = Arc::new(CronExecutor::new(channel_sender, agent_runner));

    let mut jobs_map: std::collections::HashMap<String, ScheduledJob> =
        std::collections::HashMap::new();

    let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::channel::<()>(1);
    let _watcher = setup_cron_watcher(&ws_dir, watcher_tx);

    if let Err(error) = reload_jobs(&data_dir, &ws_dir, &mut jobs_map, &scheduler).await {
        tracing::error!(%error, "Cron initialization failed");
        return;
    }
    if !jobs_map.is_empty() {
        tracing::info!(count = jobs_map.len(), "Cron scheduler started");
    }

    let mut reload_interval = tokio::time::interval(std::time::Duration::from_secs(5));
    reload_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            Some(job_id) = fire_rx.recv() => {
                if let Err(error) = reload_jobs(&data_dir, &ws_dir, &mut jobs_map, &scheduler).await {
                    tracing::error!(%error, "Cron dispatch stopped by configuration error");
                    continue;
                }
                if let Some(job) = jobs_map.get(&job_id) {
                    let job = job.clone();
                    let spec = job.spec();
                    let scheduled_at = scheduler.scheduled_at(&job_id).unwrap_or_else(chrono::Utc::now);
                    let grace_seconds = spec
                        .runtime
                        .as_ref()
                        .map(|runtime| runtime.misfire_grace_seconds)
                        .unwrap_or(600);
                    let missed_policy = spec.missed_policy.as_deref().unwrap_or("run_once");
                    let should_run = decide_misfire(
                        scheduled_at,
                        chrono::Utc::now(),
                        grace_seconds,
                        missed_policy,
                    ) == MisfireDecision::RunNow;

                    if should_run {
                        let result = execute_scheduled_job(&job, &executor, &registry).await;
                        if result.success {
                            tracing::info!(
                                job_id = %job_id,
                                duration_ms = result.duration_ms,
                                "Cron job executed successfully"
                            );
                        } else {
                            tracing::warn!(
                                job_id = %job_id,
                                error = ?result.error,
                                duration_ms = result.duration_ms,
                                "Cron job execution failed"
                            );
                        }
                        if is_channel_dispatch_job(spec) {
                            record_cron_dispatch_result(
                                &data_dir,
                                spec,
                                result.success,
                                result.error.clone(),
                                result.duration_ms,
                            );
                        }
                    } else {
                        tracing::warn!(
                            job_id = %job_id,
                            scheduled_at = %scheduled_at,
                            grace_seconds,
                            missed_policy,
                            "Cron job skipped because scheduled run missed grace window"
                        );
                        if is_channel_dispatch_job(spec) {
                            record_cron_dispatch_result(
                                &data_dir,
                                spec,
                                false,
                                Some(format!(
                                    "missed scheduled run at {}: grace={}s policy={}",
                                    scheduled_at.to_rfc3339(),
                                    grace_seconds,
                                    missed_policy
                                )),
                                0,
                            );
                        }
                    }
                    if spec.enabled && !spec.paused {
                        if let Some(next) = compute_next_run_for_schedule(&spec.schedule) {
                            let cron_job = spec_to_cron_job(spec);
                            let _ = scheduler.register_next_run(&cron_job, next).await;
                        }
                    }
                }
            }
            Some(()) = watcher_rx.recv() => {
                tracing::debug!("Cron watcher triggered reload");
                if let Err(error) = reload_jobs(&data_dir, &ws_dir, &mut jobs_map, &scheduler).await {
                    tracing::error!(%error, "Cron reload failed");
                }
            }
            _ = reload_interval.tick() => {
                if let Err(error) = reload_jobs(&data_dir, &ws_dir, &mut jobs_map, &scheduler).await {
                    tracing::error!(%error, "Cron reload failed");
                }
            }
        }
    }
}

fn is_channel_dispatch_job(spec: &CronJobSpec) -> bool {
    spec.dispatch
        .as_ref()
        .map(|dispatch| dispatch.r#type.is_empty() || dispatch.r#type == "channel")
        .unwrap_or(true)
}

async fn execute_scheduled_job(
    job: &ScheduledJob,
    executor: &Arc<CronExecutor>,
    registry: &Arc<dyn WorkspaceRegistry>,
) -> rua_cron::JobExecutionResult {
    match job {
        ScheduledJob::User(spec) => executor.execute(spec).await,
        ScheduledJob::Heartbeat(spec) => {
            if rua_cron::should_run_heartbeat_job(spec) {
                executor.execute(spec).await
            } else {
                rua_cron::JobExecutionResult {
                    job_id: spec.id.clone(),
                    success: true,
                    error: None,
                    duration_ms: 0,
                }
            }
        }
        ScheduledJob::Dream(spec) => execute_dream_job(spec, registry).await,
    }
}

async fn execute_dream_job(
    job: &CronJobSpec,
    registry: &Arc<dyn WorkspaceRegistry>,
) -> rua_cron::JobExecutionResult {
    let start = std::time::Instant::now();
    let agent_id = job.agent_id.as_deref().unwrap_or("default");
    let result = async {
        let runtime = registry
            .get_or_create(agent_id)
            .await
            .map_err(|e| RuaError::Cron {
                job_id: job.id.clone(),
                message: format!("failed to obtain workspace runtime for dream: {e}"),
            })?;
        let memory_manager = runtime
            .memory_manager()
            .await
            .ok_or_else(|| RuaError::Cron {
                job_id: job.id.clone(),
                message: format!("memory manager unavailable for agent {agent_id}"),
            })?;
        memory_manager
            .dream_memory(rua_agent::memory_manager::DreamMemoryRequest {
                agent_id: agent_id.to_string(),
            })
            .await
            .map_err(|e| RuaError::Cron {
                job_id: job.id.clone(),
                message: e.to_string(),
            })?;
        Ok::<(), RuaError>(())
    }
    .await;

    let duration_ms = start.elapsed().as_millis() as u64;
    match result {
        Ok(()) => rua_cron::JobExecutionResult {
            job_id: job.id.clone(),
            success: true,
            error: None,
            duration_ms,
        },
        Err(error) => rua_cron::JobExecutionResult {
            job_id: job.id.clone(),
            success: false,
            error: Some(error.to_string()),
            duration_ms,
        },
    }
}

fn setup_cron_watcher(
    ws_dir: &std::path::Path,
    watcher_tx: tokio::sync::mpsc::Sender<()>,
) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};

    let tx = watcher_tx;
    let mut watcher = match notify::recommended_watcher(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let dominated_by_json = event
                    .paths
                    .iter()
                    .any(|p| p.extension().is_some_and(|ext| ext == "json"));
                // Linux also reports reads; reloading on those would observe our own
                // config reads and continuously reload, starving the runtime.
                let changed = matches!(
                    event.kind,
                    notify::EventKind::Create(_)
                        | notify::EventKind::Modify(_)
                        | notify::EventKind::Remove(_)
                );
                if changed && dominated_by_json {
                    let _ = tx.try_send(());
                }
            }
        },
    ) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to create cron file watcher, relying on periodic reload");
            return None;
        }
    };

    if let Err(e) = watcher.watch(ws_dir, RecursiveMode::Recursive) {
        tracing::warn!(error = %e, path = %ws_dir.display(), "Failed to watch cron directory, relying on periodic reload");
        return None;
    }

    tracing::info!(path = %ws_dir.display(), "Cron file watcher active");
    Some(watcher)
}

async fn reload_jobs(
    data_dir: &std::path::Path,
    ws_dir: &std::path::Path,
    jobs_map: &mut std::collections::HashMap<String, ScheduledJob>,
    scheduler: &Arc<TokioScheduler>,
) -> Result<(), String> {
    let fresh = load_schedulable_jobs(data_dir)?;
    let fresh_ids: std::collections::HashSet<String> =
        fresh.iter().map(|j| j.spec().id.clone()).collect();
    let old_ids: std::collections::HashSet<String> = jobs_map.keys().cloned().collect();

    for removed_id in old_ids.difference(&fresh_ids) {
        jobs_map.remove(removed_id);
        if let Err(e) = scheduler.cancel(removed_id).await {
            tracing::debug!(job_id = %removed_id, error = %e, "Failed to cancel removed cron job");
        }
    }

    for job in fresh {
        let id = job.spec().id.clone();
        let should_register = jobs_map
            .get(&id)
            .map(|old| schedule_changed(old.spec(), job.spec()))
            .unwrap_or(true);
        if should_register {
            if let Some(next) = compute_next_run_for_schedule(&job.spec().schedule) {
                let cron_job = spec_to_cron_job(job.spec());
                if let Err(e) = scheduler.register_next_run(&cron_job, next).await {
                    tracing::warn!(job_id = %id, error = %e, "Failed to register cron job");
                } else {
                    tracing::info!(job_id = %id, "Cron job registered");
                }
            } else if matches!(&job, ScheduledJob::User(_))
                && (job.spec().schedule.r#type.is_empty() || job.spec().schedule.r#type == "cron")
            {
                auto_disable_invalid_user_job(ws_dir, job.spec());
            }
        }
        jobs_map.insert(id, job);
    }
    Ok(())
}

fn auto_disable_invalid_user_job(ws_dir: &std::path::Path, spec: &CronJobSpec) {
    let Some(agent_id) = spec
        .agent_id
        .as_deref()
        .filter(|agent_id| !agent_id.is_empty())
    else {
        return;
    };
    let mut disabled = spec.clone();
    disabled.enabled = false;
    let repo = JsonCronJobRepository::new(ws_dir.join(agent_id).join("jobs.json"));
    match repo.upsert_job(disabled) {
        Ok(_) => tracing::warn!(
            job_id = %spec.id,
            agent_id,
            cron = %spec.schedule.cron,
            "Auto-disabled invalid cron job during scheduler reload"
        ),
        Err(e) => tracing::warn!(
            job_id = %spec.id,
            agent_id,
            error = %e,
            "Failed to auto-disable invalid cron job during scheduler reload"
        ),
    }
}

fn schedule_changed(old: &CronJobSpec, new: &CronJobSpec) -> bool {
    old.schedule.cron != new.schedule.cron
        || old.schedule.r#type != new.schedule.r#type
        || old.schedule.run_at != new.schedule.run_at
        || old.schedule.timezone != new.schedule.timezone
        || old.schedule.repeat_every_days != new.schedule.repeat_every_days
        || old.schedule.repeat_end_type != new.schedule.repeat_end_type
        || old.schedule.repeat_until != new.schedule.repeat_until
        || old.schedule.repeat_count != new.schedule.repeat_count
        || old.enabled != new.enabled
        || old.paused != new.paused
}
