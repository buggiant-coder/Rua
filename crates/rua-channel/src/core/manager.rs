use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::core::channel::{Channel, Process};
use crate::core::command_registry::CommandRegistry;
use crate::core::content::{
    AgentChannelRequest, ChannelTarget, IncomingContentPart, OutgoingContentPart, OutgoingMessage,
};
use crate::core::control_handlers::{self, ControlContext};
use crate::core::health::{ChannelLifecycle, ChannelStatus};
use crate::core::queue::UnifiedQueueManager;
use rua_core::RuntimeEvent;

fn text_message(channel_id: &str, target: &ChannelTarget, text: String) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: channel_id.to_string(),
        conversation_id: target.conversation_id.clone(),
        receiver_id: target.sender_id.clone(),
        content: vec![OutgoingContentPart::Text { text }],
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

fn approval_message(
    channel_id: &str,
    target: &ChannelTarget,
    prompt: &crate::core::im::ApprovalPrompt,
) -> OutgoingMessage {
    OutgoingMessage {
        metadata: serde_json::json!({
            "message_type": "tool_guard_approval",
            "approval_request_id": prompt.request_id,
            "tool_name": prompt.tool_name,
        }),
        ..text_message(channel_id, target, prompt.render_text())
    }
}

pub fn extract_query_text(payload: &serde_json::Value) -> String {
    let raw = payload
        .get("text")
        .or_else(|| payload.get("content"))
        .or_else(|| payload.get("query"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    first_line(raw).to_string()
}

pub fn extract_query_text_from_request(req: &AgentChannelRequest) -> String {
    let text: String = req
        .input
        .iter()
        .filter_map(|part| match part {
            IncomingContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    first_line(&text).to_string()
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

fn extract_outcome_text(outcome: &rua_command::CommandOutcome) -> Option<String> {
    let text: String = outcome
        .events
        .iter()
        .filter_map(|e| match e {
            agent_types::AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn target_from_payload(session_id: &str, payload: &serde_json::Value) -> ChannelTarget {
    ChannelTarget {
        agent_id: payload
            .get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or("default")
            .to_string(),
        conversation_id: payload
            .get("conversation_id")
            .and_then(|v| v.as_str())
            .unwrap_or(session_id)
            .to_string(),
        sender_id: payload
            .get("sender_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

pub struct RoutingProcess {
    command_registry: Arc<CommandRegistry>,
    inner: Arc<dyn Process>,
}

impl RoutingProcess {
    pub fn new(command_registry: Arc<CommandRegistry>, inner: Arc<dyn Process>) -> Self {
        Self {
            command_registry,
            inner,
        }
    }
}

#[async_trait]
impl Process for RoutingProcess {
    async fn handle(&self, req: AgentChannelRequest) -> BoxStream<'static, RuntimeEvent> {
        let query = extract_query_text_from_request(&req);
        let level = self.command_registry.get_priority_level(&query);

        if self.command_registry.is_control_command(&query) {
            tracing::info!(
                channel_id = %req.channel_id,
                session_id = %req.session_id,
                command = %query,
                priority = level,
                "control command short-circuited via RoutingProcess (dispatch_control stub)"
            );
            return Box::pin(futures::stream::empty());
        }

        tracing::debug!(
            channel_id = %req.channel_id,
            session_id = %req.session_id,
            priority = level,
            "normal message, forwarding to inner process"
        );
        self.inner.handle(req).await
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushMessage {
    pub id: String,

    #[serde(rename = "type")]
    pub msg_type: String,

    pub content: String,

    pub created_at: DateTime<Utc>,

    pub session_id: String,

    pub channel_id: String,

    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushMessagesResponse {
    pub messages: Vec<PushMessage>,
}

pub struct ConsoleMessageSubscription {
    rx: broadcast::Receiver<PushMessage>,
}

impl ConsoleMessageSubscription {
    pub async fn recv(&mut self) -> Option<PushMessage> {
        match self.rx.recv().await {
            Ok(msg) => Some(msg),
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(
                    lagged = n,
                    "push message subscriber lagged, some messages dropped"
                );

                self.rx.recv().await.ok()
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}

const DEFAULT_PUSH_BUFFER_CAPACITY: usize = 100;

const DEFAULT_BROADCAST_CAPACITY: usize = 256;

const DEFAULT_REFRESH_INTERVAL_SECS: u64 = 1800;

fn lifecycle_result(errors: Vec<String>) -> rua_core::Result<()> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(rua_core::RuaError::Runtime {
            message: errors.join("; "),
        })
    }
}

pub struct ChannelManager {
    channels: Arc<RwLock<HashMap<String, RegisteredChannel>>>,

    process: RwLock<Option<Arc<dyn Process>>>,

    push_buffers: RwLock<HashMap<String, Vec<PushMessage>>>,

    push_senders: RwLock<HashMap<String, broadcast::Sender<PushMessage>>>,

    push_buffer_capacity: usize,

    queue_manager: Option<UnifiedQueueManager>,

    command_registry: Arc<CommandRegistry>,

    refresh_interval_secs: u64,

    lifecycle: Mutex<Option<RefreshTask>>,

    control_ctx: RwLock<Option<Arc<ControlContext>>>,

    command_dispatch: RwLock<Option<CommandDispatchConfig>>,

    session_targets: RwLock<HashMap<String, ChannelTarget>>,
}

#[derive(Clone)]
struct RegisteredChannel {
    channel: Arc<dyn Channel>,
    status: Arc<RwLock<ChannelStatus>>,
}

struct RefreshTask {
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

struct CommandDispatchConfig {
    dispatcher: Arc<rua_command::CommandDispatcher>,
    session_canceller: Arc<dyn rua_command::SessionCanceller>,
    approval_service: Arc<dyn rua_command::ApprovalService>,
    queue_clearer: Arc<dyn rua_command::QueueClearer>,
    skills_lister: Arc<dyn rua_command::SkillsLister>,
    data_dir: std::path::PathBuf,
    started_at: std::time::Instant,
    workspace_reloader: Arc<dyn rua_command::WorkspaceReloader>,
    exit_fn: Option<Arc<dyn Fn(i32) + Send + Sync>>,
}

struct NoopChannelAcpController;

#[async_trait]
impl rua_command::AcpChatController for NoopChannelAcpController {
    async fn enter(&self, _chat_id: &str, _runner: &str, _cwd: Option<&str>) -> String {
        String::new()
    }
    async fn exit(&self, _chat_id: &str) -> String {
        String::new()
    }
    async fn list_sessions(&self, _chat_id: &str, _runner: &str, _cwd: Option<&str>) -> String {
        String::new()
    }
    async fn resume(
        &self,
        _chat_id: &str,
        _runner: &str,
        _acp_session_id: &str,
        _cwd: Option<&str>,
    ) -> String {
        String::new()
    }
}

fn channel_noop_acp() -> (
    Arc<rua_command::AcpChatSessionManager>,
    Arc<dyn rua_command::AcpChatController>,
) {
    (
        Arc::new(rua_command::AcpChatSessionManager::new(
            std::time::Duration::from_secs(0),
        )),
        Arc::new(NoopChannelAcpController),
    )
}

impl ChannelManager {
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
            process: RwLock::new(None),
            push_buffers: RwLock::new(HashMap::new()),
            push_senders: RwLock::new(HashMap::new()),
            push_buffer_capacity: DEFAULT_PUSH_BUFFER_CAPACITY,
            queue_manager: None,
            command_registry: Arc::new(CommandRegistry::new()),
            refresh_interval_secs: DEFAULT_REFRESH_INTERVAL_SECS,
            lifecycle: Mutex::new(None),
            control_ctx: RwLock::new(None),
            command_dispatch: RwLock::new(None),
            session_targets: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_command_registry(mut self, registry: CommandRegistry) -> Self {
        self.command_registry = Arc::new(registry);
        self
    }

    pub fn command_registry(&self) -> &CommandRegistry {
        &self.command_registry
    }

    pub async fn set_control_context(&self, ctx: Arc<ControlContext>) {
        *self.control_ctx.write().await = Some(ctx);
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn set_command_dispatcher(
        &self,
        dispatcher: Arc<rua_command::CommandDispatcher>,
        session_canceller: Arc<dyn rua_command::SessionCanceller>,
        approval_service: Arc<dyn rua_command::ApprovalService>,
        queue_clearer: Arc<dyn rua_command::QueueClearer>,
        skills_lister: Arc<dyn rua_command::SkillsLister>,
        data_dir: std::path::PathBuf,
        started_at: std::time::Instant,
        workspace_reloader: Arc<dyn rua_command::WorkspaceReloader>,
        exit_fn: Option<Arc<dyn Fn(i32) + Send + Sync>>,
    ) {
        *self.command_dispatch.write().await = Some(CommandDispatchConfig {
            dispatcher,
            session_canceller,
            approval_service,
            queue_clearer,
            skills_lister,
            data_dir,
            started_at,
            workspace_reloader,
            exit_fn,
        });
    }

    pub async fn register_session_target(&self, session_id: &str, target: ChannelTarget) {
        self.session_targets
            .write()
            .await
            .insert(session_id.to_string(), target);
    }

    pub async fn unregister_session_target(&self, session_id: &str) {
        self.session_targets.write().await.remove(session_id);
    }

    pub async fn target_for_session(&self, session_id: &str) -> Option<ChannelTarget> {
        self.session_targets.read().await.get(session_id).cloned()
    }

    pub fn with_push_buffer_capacity(mut self, capacity: usize) -> Self {
        self.push_buffer_capacity = capacity;
        self
    }

    pub fn with_queue_manager(mut self, qm: UnifiedQueueManager) -> Self {
        self.queue_manager = Some(qm);
        self
    }

    pub fn with_refresh_interval_secs(mut self, secs: u64) -> Self {
        self.refresh_interval_secs = secs;
        self
    }

    pub async fn set_process(&self, process: Arc<dyn Process>) {
        *self.process.write().await = Some(process);
    }

    pub async fn set_process_if_absent(&self, process: Arc<dyn Process>) {
        let mut current = self.process.write().await;
        if current.is_none() {
            *current = Some(process);
        }
    }

    pub async fn register(&self, channel: Arc<dyn Channel>) -> rua_core::Result<()> {
        let _lifecycle = self.lifecycle.lock().await;
        let id = channel.id().clone();
        tracing::info!(channel_id = %id, "registering channel");
        let mut channels = self.channels.write().await;
        if let Some(existing) = channels.get(&id) {
            if !matches!(
                existing.status.read().await.lifecycle,
                ChannelLifecycle::Registered | ChannelLifecycle::Stopped
            ) {
                return Err(rua_core::RuaError::Validation {
                    message: format!("Channel '{id}' must be stopped before replacement"),
                });
            }
        }
        channels.insert(
            id,
            RegisteredChannel {
                channel,
                status: Arc::new(RwLock::new(ChannelStatus {
                    lifecycle: ChannelLifecycle::Registered,
                    refresh_error: None,
                })),
            },
        );
        Ok(())
    }

    pub async fn start_all(&self) -> rua_core::Result<()> {
        let mut refresh_task = self.lifecycle.lock().await;
        let channels = self.channels.read().await.clone();
        if channels.is_empty() {
            return Ok(());
        }
        let process =
            self.process
                .read()
                .await
                .clone()
                .ok_or_else(|| rua_core::RuaError::Validation {
                    message: "Channel start requires a Process".into(),
                })?;
        let process: Arc<dyn Process> = Arc::new(
            crate::core::debouncer::NoTextDebounceProcess::new(Arc::new(RoutingProcess::new(
                Arc::clone(&self.command_registry),
                process,
            ))),
        );
        let mut errors = Vec::new();
        for (id, entry) in &channels {
            let status = entry.status.read().await.clone();
            if status.is_running() {
                continue;
            }
            if matches!(status.lifecycle, ChannelLifecycle::Failed { .. }) {
                errors.push(format!(
                    "{id}: stop the failed instance before starting again"
                ));
                continue;
            }
            entry.status.write().await.lifecycle = ChannelLifecycle::Starting;
            let lifecycle = match entry.channel.start(process.clone()).await {
                Ok(()) => ChannelLifecycle::Running,
                Err(error) => {
                    let reason = format!("{id}: start failed: {error}");
                    errors.push(reason.clone());
                    ChannelLifecycle::Failed { reason }
                }
            };
            *entry.status.write().await = ChannelStatus {
                lifecycle,
                refresh_error: None,
            };
        }
        if refresh_task.is_none() {
            if let Some(qm) = &self.queue_manager {
                qm.start_cleanup_loop();
            }
            let channels = Arc::clone(&self.channels);
            let interval = std::time::Duration::from_secs(self.refresh_interval_secs);
            let cancel = CancellationToken::new();
            let token = cancel.clone();
            let handle = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = token.cancelled() => break,
                        _ = tokio::time::sleep(interval) => {}
                    }
                    let entries = channels.read().await.clone();
                    for entry in entries.values() {
                        if !entry.status.read().await.is_running() {
                            continue;
                        }
                        let result = tokio::select! {
                            _ = token.cancelled() => return,
                            result = entry.channel.refresh_webhook_or_token() => result,
                        };
                        entry.status.write().await.refresh_error = result
                            .err()
                            .map(|error| format!("refresh_webhook_or_token failed: {error}"));
                    }
                }
            });
            *refresh_task = Some(RefreshTask { cancel, handle });
        }
        lifecycle_result(errors)
    }

    pub async fn stop_all(&self) -> rua_core::Result<()> {
        let mut refresh_task = self.lifecycle.lock().await;
        let mut errors = Vec::new();
        if let Some(task) = refresh_task.take() {
            task.cancel.cancel();
            if let Err(error) = task.handle.await {
                errors.push(format!("Channel refresh task failed: {error}"));
            }
        }
        let channels = self.channels.read().await.clone();
        for (id, entry) in &channels {
            if matches!(
                entry.status.read().await.lifecycle,
                ChannelLifecycle::Registered | ChannelLifecycle::Stopped
            ) {
                *entry.status.write().await = ChannelStatus {
                    lifecycle: ChannelLifecycle::Stopped,
                    refresh_error: None,
                };
                continue;
            }
            entry.status.write().await.lifecycle = ChannelLifecycle::Stopping;
            let lifecycle = match entry.channel.stop().await {
                Ok(()) => ChannelLifecycle::Stopped,
                Err(error) => {
                    let reason = format!("{id}: stop failed: {error}");
                    errors.push(reason.clone());
                    ChannelLifecycle::Failed { reason }
                }
            };
            *entry.status.write().await = ChannelStatus {
                lifecycle,
                refresh_error: None,
            };
        }
        if let Some(qm) = &self.queue_manager {
            qm.stop_all().await;
        }
        lifecycle_result(errors)
    }

    pub async fn enqueue(
        &self,
        channel_id: String,
        session_id: String,
        priority: u32,
        payload: serde_json::Value,
    ) -> rua_core::Result<()> {
        let qm = self
            .queue_manager
            .as_ref()
            .ok_or_else(|| rua_core::RuaError::Runtime {
                message: "queue manager not configured".into(),
            })?;
        qm.enqueue(channel_id, session_id, priority, payload).await
    }

    pub async fn remove_channel(&self, id: &str) -> rua_core::Result<()> {
        let _operation = self.lifecycle.lock().await;
        let entry = self.channels.read().await.get(id).cloned();
        if let Some(entry) = entry {
            entry.status.write().await.lifecycle = ChannelLifecycle::Stopping;
            if let Err(error) = entry.channel.stop().await {
                entry.status.write().await.lifecycle = ChannelLifecycle::Failed {
                    reason: "Channel could not stop for reconfiguration".to_string(),
                };
                return Err(error);
            }
            self.channels.write().await.remove(id);
        }
        Ok(())
    }

    pub async fn handle_incoming(
        &self,
        channel_id: String,
        session_id: String,
        payload: serde_json::Value,
    ) -> rua_core::Result<()> {
        if let Some(entry) = self.channels.read().await.get(&channel_id) {
            if matches!(
                entry.status.read().await.lifecycle,
                ChannelLifecycle::Failed { .. }
                    | ChannelLifecycle::Stopping
                    | ChannelLifecycle::Stopped
            ) {
                return Err(rua_core::RuaError::Runtime {
                    message: "Channel configuration is not applied; incoming work is unavailable"
                        .to_string(),
                });
            }
        }
        let query = extract_query_text(&payload);
        let level = self.command_registry.get_priority_level(&query);

        if crate::core::im::is_im_channel(&channel_id)
            && !query.starts_with("/approve")
            && !query.starts_with("/deny")
        {
            self.reject_pending_if_any(&session_id).await;
        }

        if self.command_registry.is_control_command(&query) {
            return self
                .dispatch_control(&channel_id, &session_id, &query, &payload)
                .await;
        }

        self.enqueue(channel_id, session_id, level, payload).await
    }

    async fn dispatch_control(
        &self,
        channel_id: &str,
        session_id: &str,
        query: &str,
        payload: &serde_json::Value,
    ) -> rua_core::Result<()> {
        if let Some(text) = self
            .try_command_dispatcher(channel_id, session_id, query)
            .await
        {
            let target = self
                .target_for_session(session_id)
                .await
                .unwrap_or_else(|| target_from_payload(session_id, payload));
            if let Err(e) = self.send_text(channel_id, target, text).await {
                tracing::warn!(
                    %channel_id, %session_id, error = %e,
                    "failed to send command dispatcher response"
                );
            }

            return Ok(());
        }

        let ctx_guard = self.control_ctx.read().await;
        let Some(ctx) = ctx_guard.as_ref() else {
            tracing::warn!(
                %channel_id,
                %session_id,
                command = %query,
                "control command received but no ControlContext set"
            );
            return Ok(());
        };

        let result =
            control_handlers::dispatch_control_command(ctx, channel_id, session_id, query, payload)
                .await?;

        if let Some(text) = result.response_text {
            let target = self
                .target_for_session(session_id)
                .await
                .unwrap_or_else(|| target_from_payload(session_id, payload));

            if let Err(e) = self.send_text(channel_id, target, text).await {
                tracing::warn!(
                    %channel_id,
                    %session_id,
                    error = %e,
                    "failed to send control command response"
                );
            }
        }

        Ok(())
    }

    async fn try_command_dispatcher(
        &self,
        channel_id: &str,
        session_id: &str,
        query: &str,
    ) -> Option<String> {
        let config_guard = self.command_dispatch.read().await;
        let config = config_guard.as_ref()?;

        let (cmd_name, args) = rua_command::parse_command(query)?;

        if let Some(cmd) = config.dispatcher.control.get(cmd_name) {
            let (acp_mgr, acp_ctrl) = channel_noop_acp();
            let ctrl_ctx = rua_command::ControlContext {
                session_id: session_id.to_string(),
                user_id: String::new(),
                channel: channel_id.to_string(),
                session_canceller: Arc::clone(&config.session_canceller),
                approval_service: Arc::clone(&config.approval_service),
                queue_clearer: Arc::clone(&config.queue_clearer),
                skills_lister: Arc::clone(&config.skills_lister),
                acp_chat_manager: acp_mgr,
                acp_chat_controller: acp_ctrl,
                locale: "zh".into(),
            };
            let outcome = cmd.execute(&ctrl_ctx, args).await;
            return extract_outcome_text(&outcome);
        }

        if let Some(cmd) = config.dispatcher.daemon.get(cmd_name) {
            let daemon_ctx = rua_command::DaemonContext {
                data_dir: config.data_dir.clone(),
                started_at: config.started_at,
                workspace_reloader: Arc::clone(&config.workspace_reloader),
                exit_fn: config.exit_fn.clone(),
                approval_service: Arc::clone(&config.approval_service),
                locale: "zh".into(),
            };
            let outcome = cmd.execute(&daemon_ctx, args).await;
            return extract_outcome_text(&outcome);
        }

        None
    }

    #[allow(clippy::unnecessary_wraps)]
    pub async fn send_text(
        &self,
        channel_id: &str,
        target: ChannelTarget,
        text: String,
    ) -> rua_core::Result<()> {
        let channels = self.channels.read().await;
        let channel = channels
            .get(channel_id)
            .ok_or_else(|| rua_core::RuaError::NotFound {
                resource: "channel".into(),
                id: channel_id.into(),
            })?;
        let message = text_message(channel_id, &target, text);
        if matches!(
            channel.status.read().await.lifecycle,
            ChannelLifecycle::Failed { .. }
                | ChannelLifecycle::Stopping
                | ChannelLifecycle::Stopped
        ) {
            return Err(rua_core::RuaError::Runtime {
                message: "Channel configuration is not applied; sending is unavailable".to_string(),
            });
        }
        channel.channel.send(target, message).await
    }

    pub async fn send_stream_event(
        &self,
        channel_id: &str,
        target: ChannelTarget,
        event: RuntimeEvent,
    ) -> rua_core::Result<()> {
        let channel = self
            .channels
            .read()
            .await
            .get(channel_id)
            .cloned()
            .ok_or_else(|| rua_core::RuaError::NotFound {
                resource: "channel".into(),
                id: channel_id.into(),
            })?;
        if matches!(
            channel.status.read().await.lifecycle,
            ChannelLifecycle::Failed { .. }
                | ChannelLifecycle::Stopping
                | ChannelLifecycle::Stopped
        ) {
            return Err(rua_core::RuaError::Runtime {
                message: "Channel configuration is not applied; event delivery is unavailable"
                    .to_string(),
            });
        }
        if let Some(prompt) = crate::core::im::extract_tool_approval(&event) {
            return channel
                .channel
                .send(
                    target.clone(),
                    approval_message(channel_id, &target, &prompt),
                )
                .await;
        }
        if matches!(
            event,
            RuntimeEvent::AskUser { .. } | RuntimeEvent::ToolApprovalRequest { .. }
        ) {
            return Err(rua_core::RuaError::Runtime {
                message:
                    "Channel event delivery requires a supported interaction/approval control path"
                        .into(),
            });
        }
        let content = crate::core::renderer::MessageRenderer::new(
            crate::core::renderer::RenderStyle::Structured,
        )
        .render_event(&event);
        if content.is_empty() {
            return Ok(());
        }
        let message = OutgoingMessage {
            content,
            ..text_message(channel_id, &target, String::new())
        };
        channel.channel.send(target, message).await
    }

    pub async fn push_console_message(
        &self,
        agent_id: &str,
        message: PushMessage,
    ) -> rua_core::Result<()> {
        {
            let mut buffers = self.push_buffers.write().await;
            let buffer = buffers.entry(agent_id.to_string()).or_insert_with(Vec::new);
            buffer.push(message.clone());

            if buffer.len() > self.push_buffer_capacity {
                let overflow = buffer.len() - self.push_buffer_capacity;
                buffer.drain(..overflow);
                tracing::debug!(
                    agent_id = %agent_id,
                    dropped = overflow,
                    "push message buffer overflow, dropped oldest"
                );
            }
        }

        {
            let senders = self.push_senders.read().await;
            if let Some(sender) = senders.get(agent_id) {
                let _ = sender.send(message);
            }
        }

        Ok(())
    }

    pub async fn recent_console_messages(&self, agent_id: &str, limit: usize) -> Vec<PushMessage> {
        let buffers = self.push_buffers.read().await;
        match buffers.get(agent_id) {
            Some(buffer) => {
                let start = buffer.len().saturating_sub(limit);
                buffer[start..].to_vec()
            }
            None => vec![],
        }
    }

    pub async fn subscribe_console_messages(&self, agent_id: &str) -> ConsoleMessageSubscription {
        let mut senders = self.push_senders.write().await;
        let sender = senders.entry(agent_id.to_string()).or_insert_with(|| {
            let (tx, _) = broadcast::channel(DEFAULT_BROADCAST_CAPACITY);
            tx
        });
        ConsoleMessageSubscription {
            rx: sender.subscribe(),
        }
    }

    pub async fn broadcast_text(
        &self,
        text: &str,
    ) -> Vec<(String, Result<(), rua_core::RuaError>)> {
        let channels = self.channels.read().await;
        let mut results = Vec::with_capacity(channels.len());
        let target = ChannelTarget {
            agent_id: "system".into(),
            conversation_id: String::new(),
            sender_id: None,
            reply_to: None,
            metadata: serde_json::json!({}),
        };
        for (id, channel) in channels.iter() {
            let res = channel
                .channel
                .send(target.clone(), text_message(id, &target, text.to_string()))
                .await;
            if let Err(ref e) = res {
                tracing::warn!(channel_id = %id, error = %e, "broadcast_text: channel failed");
            }
            results.push((id.clone(), res));
        }
        results
    }

    pub async fn statuses(&self) -> HashMap<String, ChannelStatus> {
        let channels = self.channels.read().await.clone();
        let mut status = HashMap::new();
        for (id, entry) in channels {
            status.insert(id, entry.status.read().await.clone());
        }
        status
    }

    pub async fn channel_ids(&self) -> Vec<String> {
        self.channels.read().await.keys().cloned().collect()
    }

    pub async fn subscriber_count(&self, agent_id: &str) -> usize {
        let senders = self.push_senders.read().await;
        match senders.get(agent_id) {
            Some(sender) => sender.receiver_count(),
            None => 0,
        }
    }

    async fn reject_pending_if_any(&self, session_id: &str) {
        let config_guard = self.command_dispatch.read().await;
        if let Some(config) = config_guard.as_ref() {
            let rejected = config.approval_service.reject_pending(session_id).await;
            if rejected {
                tracing::info!(
                    %session_id,
                    "IM non-approve-reject: rejected pending approval"
                );
            }
        }
    }
}

impl Default for ChannelManager {
    fn default() -> Self {
        Self::new()
    }
}
