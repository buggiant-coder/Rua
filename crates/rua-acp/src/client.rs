use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    ContentBlock, EnvVariable, InitializeRequest, ListSessionsRequest, LoadSessionRequest,
    McpServer, McpServerStdio, NewSessionRequest, NewSessionResponse, RequestPermissionRequest,
    RequestPermissionResponse, SessionInfo, SessionNotification, SessionUpdate,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::util::MatchDispatch;
use agent_client_protocol::{
    on_receive_request, ActiveSession, Agent, Client, Dispatch, SessionMessage,
};
use rua_config::types::acp::AcpAgentConfig;
use rua_core::event::RuntimeEvent;
use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchPlan, RuntimeLaunchRequest, ToolchainIntent,
};
use tokio::sync::{mpsc, oneshot};
use tracing;

use crate::core::{AcpError, SessionInit};
use crate::managed_agent::ManagedAcpAgent;
use crate::permissions::{
    cancelled_response, select_unambiguous_allow_once, selected_response, AcpPermissionRelay,
    AcpPermissionRequestContext, PermissionDecision, PermissionEvaluator,
    PermissionRelayResolution,
};
use crate::tool_projection::{AcpToolProjectionUpdate, SharedAcpToolProjection};

#[derive(Debug)]
pub enum TurnCommand {
    SendPrompt {
        text: String,
        event_tx: mpsc::UnboundedSender<RuntimeEvent>,
        started_tx: oneshot::Sender<()>,
    },
    Close,
}

pub struct AcpClientAdapter;

pub(crate) struct AcpConnectionContext {
    pub evaluator: Option<Arc<dyn PermissionEvaluator>>,
    pub permission_relay: Arc<AcpPermissionRelay>,
    pub permission_context: AcpPermissionRequestContext,
    pub trusted: bool,
    pub permission_scope_id: String,
    pub cancel_gate: Arc<std::sync::Mutex<tokio_util::sync::CancellationToken>>,
    pub active_turn: Arc<AtomicBool>,
}

enum HistoryPreviewEvent {
    Text(String),
    Thinking(String),
    ToolCall(String),
    ToolResult(String),
}

const ACP_HISTORY_PREVIEW_MAX_BYTES: usize = 1024 * 1024;
const ACP_HISTORY_PREVIEW_TRIM_THRESHOLD_BYTES: usize = ACP_HISTORY_PREVIEW_MAX_BYTES * 2;
const ACP_HISTORY_PREVIEW_TRUNCATION_MARKER: &str = "[Earlier ACP history omitted]\n\n";
const ACP_PERMISSION_RELAY_TIMEOUT: Duration = Duration::from_secs(300);
const ACP_PERMISSION_TOOL_CORRELATION_TIMEOUT: Duration = Duration::from_secs(1);

impl AcpClientAdapter {
    pub fn build_acp_agent(config: &AcpAgentConfig) -> Result<ManagedAcpAgent, AcpError> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::build_acp_agent_for_cwd(config, &cwd)
    }

    pub fn build_acp_agent_for_cwd(
        config: &AcpAgentConfig,
        cwd: &Path,
    ) -> Result<ManagedAcpAgent, AcpError> {
        let cache_dir = default_runtime_cache_dir();
        let plan = acp_runtime_launch_plan(config, cwd, &cache_dir);
        Self::build_acp_agent_from_plan(&plan)
    }

    pub fn build_acp_agent_for_runtime(
        config: &AcpAgentConfig,
        cwd: &Path,
        runtime_cache_dir: &Path,
    ) -> Result<ManagedAcpAgent, AcpError> {
        let plan = acp_runtime_launch_plan(config, cwd, runtime_cache_dir);
        Self::build_acp_agent_from_plan(&plan)
    }

    pub fn build_acp_agent_from_plan(
        plan: &RuntimeLaunchPlan,
    ) -> Result<ManagedAcpAgent, AcpError> {
        let process = plan
            .process()
            .map_err(|error| AcpError::Connection(error.to_string()))?;
        let env = process
            .env
            .iter()
            .map(|(key, value)| EnvVariable::new(key, value))
            .collect();
        let command = process.command.clone();
        let name = command
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("agent")
            .to_string();
        let agent = ManagedAcpAgent::new(McpServer::Stdio(
            McpServerStdio::new(name, command)
                .args(process.args.clone())
                .env(env),
        ))
        .map_err(|error| AcpError::Connection(error.to_string()))?;

        let agent = agent.with_process_context(
            process.cwd.clone(),
            plan.runtime_kind == rua_core::runtime_layout::RuntimeKind::LinuxCompat,
        );
        Ok(agent.with_debug(|line, direction| {
            tracing::trace!(?direction, line, "acp stdio");
        }))
    }

    pub(crate) async fn run_connection(
        acp_agent: ManagedAcpAgent,
        session_init: SessionInit,
        mut cmd_rx: mpsc::UnboundedReceiver<TurnCommand>,
        startup_tx: oneshot::Sender<Result<String, String>>,
        context: AcpConnectionContext,
    ) -> Result<(), AcpError> {
        let handler_evaluator = context.evaluator.clone();
        let handler_permission_context = context.permission_context.clone();
        let handler_permission_scope_id = context.permission_scope_id.clone();
        let current_permission_event_tx: Arc<
            std::sync::Mutex<Option<mpsc::UnboundedSender<RuntimeEvent>>>,
        > = Arc::new(std::sync::Mutex::new(None));
        let handler_permission_event_tx = Arc::clone(&current_permission_event_tx);
        let tool_projection = Arc::new(SharedAcpToolProjection::new());
        let handler_tool_projection = Arc::clone(&tool_projection);
        let permission_relay = Arc::clone(&context.permission_relay);
        let handler_permission_relay = Arc::clone(&context.permission_relay);
        let handler_cancel_gate = Arc::clone(&context.cancel_gate);
        let cancel_gate = Arc::clone(&context.cancel_gate);
        let trusted = context.trusted;
        let permission_scope_id = context.permission_scope_id;
        let startup_tx = Arc::new(std::sync::Mutex::new(Some(startup_tx)));
        let connection_startup_tx = Arc::clone(&startup_tx);

        let result = Client
            .builder()
            .name("rua")
            .on_receive_request_from(
                Agent,
                async move |req: RequestPermissionRequest,
                            responder: agent_client_protocol::Responder<
                    RequestPermissionResponse,
                >,
                            _cx| {
                    let pending_event_tx = handler_permission_event_tx.lock().unwrap().clone();
                    let turn_cancel = handler_cancel_gate.lock().unwrap().clone();
                    let response = evaluate_permission_request(
                        PermissionRequestEvaluation {
                            evaluator: handler_evaluator.as_ref(),
                            permission_relay: &handler_permission_relay,
                            permission_context: &handler_permission_context,
                            trusted,
                            permission_scope_id: &handler_permission_scope_id,
                            pending_event_tx: pending_event_tx.as_ref(),
                            tool_projection: &handler_tool_projection,
                            turn_cancel,
                        },
                        &req,
                    )
                    .await;
                    responder.respond(response)
                },
                on_receive_request!(),
            )
            .connect_with(acp_agent, async move |cx| {
                cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;

                tracing::info!("ACP connection initialized");

                let mut active_session: ActiveSession<'static, _> = match session_init {
                    SessionInit::New { cwd } => {
                        let resp = cx
                            .send_request(NewSessionRequest::new(std::path::PathBuf::from(cwd)))
                            .block_task()
                            .await?;
                        cx.attach_session(resp, Vec::new())?
                    }
                    SessionInit::Load {
                        session_id,
                        cwd,
                        history_tx,
                    } => {
                        let load_resp = cx
                            .send_request(LoadSessionRequest::new(
                                session_id.clone(),
                                std::path::PathBuf::from(cwd),
                            ))
                            .block_task()
                            .await?;

                        let synthetic = NewSessionResponse::new(session_id)
                            .modes(load_resp.modes)
                            .meta(load_resp.meta);
                        let mut session = cx.attach_session(synthetic, Vec::new())?;
                        let history =
                            Self::drain_loaded_history_preview(&mut session, &tool_projection)
                                .await;
                        if let Some(tx) = history_tx {
                            let _ = tx.send(history);
                        }
                        session
                    }
                };
                if let Some(tx) = connection_startup_tx.lock().unwrap().take() {
                    let _ = tx.send(Ok(active_session.session_id().to_string()));
                }

                tracing::info!(
                    session_id = %active_session.session_id(),
                    "ACP session established, entering turn loop"
                );

                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        TurnCommand::SendPrompt {
                            text,
                            event_tx,
                            started_tx,
                        } => {
                            let turn_cancel = tokio_util::sync::CancellationToken::new();
                            {
                                let mut slot = cancel_gate.lock().unwrap();
                                *slot = turn_cancel.clone();
                            }
                            {
                                let mut slot = current_permission_event_tx.lock().unwrap();
                                *slot = Some(event_tx.clone());
                            }
                            context.active_turn.store(true, Ordering::Release);
                            let _ = started_tx.send(());
                            let turn_result = Self::run_turn_persistent(
                                &mut active_session,
                                &text,
                                &event_tx,
                                turn_cancel,
                                &tool_projection,
                            )
                            .await;
                            context.active_turn.store(false, Ordering::Release);
                            {
                                let mut slot = current_permission_event_tx.lock().unwrap();
                                *slot = None;
                            }

                            if let Err(e) = turn_result {
                                tracing::error!(error = %e, "ACP turn failed");
                                let _ = event_tx.send(RuntimeEvent::Error {
                                    error: rua_core::error::RuaError::Runtime {
                                        message: e.to_string(),
                                    },
                                });
                            }
                            let _ = event_tx.send(RuntimeEvent::End {
                                finish_reason: Some("stop".to_string()),
                            });
                        }
                        TurnCommand::Close => {
                            tracing::info!("ACP session close requested");
                            break;
                        }
                    }
                }

                Ok(())
            })
            .await;

        if let Err(error) = &result {
            if let Some(tx) = startup_tx.lock().unwrap().take() {
                let _ = tx.send(Err(error.to_string()));
            }
        }
        permission_relay.cancel_scope(&permission_scope_id).await;
        result.map_err(|e| AcpError::Connection(e.to_string()))
    }

    async fn drain_loaded_history_preview<Link>(
        active_session: &mut ActiveSession<'static, Link>,
        tool_projection: &Arc<SharedAcpToolProjection>,
    ) -> String
    where
        Link: agent_client_protocol::Role
            + agent_client_protocol::role::HasPeer<Agent>
            + Send
            + Sync
            + 'static,
    {
        const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(1);

        let mut preview = String::new();
        let mut preview_truncated = false;
        loop {
            let update = match tokio::time::timeout(
                INACTIVITY_TIMEOUT,
                active_session.read_update(),
            )
            .await
            {
                Ok(Ok(update)) => update,
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "failed to read ACP load replay update");
                    break;
                }
                Err(_) => break,
            };
            let should_stop = match update {
                SessionMessage::SessionMessage(dispatch) => {
                    Self::append_dispatch_preview(dispatch, &mut preview, tool_projection).await;
                    if preview.len() > ACP_HISTORY_PREVIEW_TRIM_THRESHOLD_BYTES {
                        preview_truncated |= Self::retain_history_preview_tail(
                            &mut preview,
                            ACP_HISTORY_PREVIEW_MAX_BYTES,
                        );
                    }
                    false
                }
                SessionMessage::StopReason(_) => true,
                _ => false,
            };
            if should_stop {
                break;
            }
        }
        Self::finish_history_preview(preview, preview_truncated)
    }

    fn finish_history_preview(preview: String, mut truncated: bool) -> String {
        let mut preview = preview.trim().to_string();
        let content_limit =
            ACP_HISTORY_PREVIEW_MAX_BYTES - ACP_HISTORY_PREVIEW_TRUNCATION_MARKER.len();
        truncated |= Self::retain_history_preview_tail(&mut preview, content_limit);

        if !truncated {
            return preview;
        }

        let mut output =
            String::with_capacity(ACP_HISTORY_PREVIEW_TRUNCATION_MARKER.len() + preview.len());
        output.push_str(ACP_HISTORY_PREVIEW_TRUNCATION_MARKER);
        output.push_str(preview.trim_start());
        output
    }

    fn retain_history_preview_tail(preview: &mut String, max_bytes: usize) -> bool {
        if preview.len() <= max_bytes {
            return false;
        }

        let mut start = preview.len() - max_bytes;
        while !preview.is_char_boundary(start) {
            start += 1;
        }
        preview.drain(..start);
        true
    }

    async fn append_dispatch_preview(
        dispatch: Dispatch,
        preview: &mut String,
        tool_projection: &Arc<SharedAcpToolProjection>,
    ) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        Self::dispatch_to_history_preview_events(dispatch, &tx, tool_projection).await;
        drop(tx);

        while let Ok(event) = rx.try_recv() {
            match event {
                HistoryPreviewEvent::Text(text) => {
                    preview.push_str(&text);
                }
                HistoryPreviewEvent::Thinking(text) if !text.trim().is_empty() => {
                    if !preview.ends_with('\n') && !preview.is_empty() {
                        preview.push('\n');
                    }
                    preview.push_str("[thinking] ");
                    preview.push_str(&text);
                    preview.push('\n');
                }
                HistoryPreviewEvent::ToolCall(name) => {
                    if !preview.ends_with('\n') && !preview.is_empty() {
                        preview.push('\n');
                    }
                    preview.push_str("[tool] ");
                    preview.push_str(&name);
                    preview.push('\n');
                }
                HistoryPreviewEvent::ToolResult(text) => {
                    if !preview.ends_with('\n') && !preview.is_empty() {
                        preview.push('\n');
                    }
                    preview.push_str("[tool result] ");
                    preview.push_str(&text);
                    preview.push('\n');
                }
                _ => {}
            }
        }
    }

    async fn dispatch_to_history_preview_events(
        dispatch: Dispatch,
        event_tx: &mpsc::UnboundedSender<HistoryPreviewEvent>,
        tool_projection: &Arc<SharedAcpToolProjection>,
    ) {
        let _ = MatchDispatch::new(dispatch)
            .if_notification(async |notif: SessionNotification| {
                match notif.update {
                    SessionUpdate::UserMessageChunk(chunk)
                    | SessionUpdate::AgentMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            let _ = event_tx.send(HistoryPreviewEvent::Text(text.text));
                        }
                    }
                    SessionUpdate::AgentThoughtChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            let _ = event_tx.send(HistoryPreviewEvent::Thinking(text.text));
                        }
                    }
                    SessionUpdate::ToolCall(tool_call) => {
                        let update = tool_projection.apply_tool_call(tool_call);
                        emit_history_projection_update(update, event_tx);
                    }
                    SessionUpdate::ToolCallUpdate(update) => {
                        if let Some(update) = tool_projection.apply_tool_call_update(update) {
                            emit_history_projection_update(update, event_tx);
                        }
                    }
                    SessionUpdate::AvailableCommandsUpdate(_) => {}
                    _ => {}
                }
                Ok(())
            })
            .await
            .otherwise_ignore();
    }

    pub async fn probe_initialize(config: &AcpAgentConfig) -> Result<(), AcpError> {
        let acp_agent = Self::build_acp_agent(config)?;
        Self::probe_initialize_with_agent(acp_agent).await
    }

    pub async fn probe_initialize_for_runtime(
        config: &AcpAgentConfig,
        cwd: &Path,
        runtime_cache_dir: &Path,
    ) -> Result<(), AcpError> {
        let acp_agent = Self::build_acp_agent_for_runtime(config, cwd, runtime_cache_dir)?;
        Self::probe_initialize_with_agent(acp_agent).await
    }

    async fn probe_initialize_with_agent(acp_agent: ManagedAcpAgent) -> Result<(), AcpError> {
        let result = Client
            .builder()
            .name("rua")
            .connect_with(acp_agent, async |cx| {
                cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                Ok(())
            })
            .await;

        result.map_err(|e| AcpError::Connection(e.to_string()))
    }

    pub async fn probe_initialize_for_runtime_with_timeout(
        config: &AcpAgentConfig,
        cwd: &Path,
        runtime_cache_dir: &Path,
        timeout: Duration,
    ) -> Result<(), AcpError> {
        tokio::time::timeout(
            timeout,
            Self::probe_initialize_for_runtime(config, cwd, runtime_cache_dir),
        )
        .await
        .map_err(|_| AcpError::Timeout(timeout))?
    }

    pub async fn list_sessions(
        config: &AcpAgentConfig,
        cwd: Option<String>,
    ) -> Result<Vec<SessionInfo>, AcpError> {
        let runtime_cache_dir = default_runtime_cache_dir();
        Self::list_sessions_for_runtime(config, cwd, &runtime_cache_dir).await
    }

    pub async fn list_sessions_for_runtime(
        config: &AcpAgentConfig,
        cwd: Option<String>,
        runtime_cache_dir: &Path,
    ) -> Result<Vec<SessionInfo>, AcpError> {
        let plan_cwd = cwd
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let acp_agent = Self::build_acp_agent_for_runtime(config, &plan_cwd, runtime_cache_dir)?;
        let sessions_out = Arc::new(tokio::sync::Mutex::new(Vec::<SessionInfo>::new()));
        let sessions_capture = sessions_out.clone();
        let result = Client
            .builder()
            .name("rua")
            .connect_with(acp_agent, async move |cx| {
                cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let req = match cwd {
                    Some(dir) => ListSessionsRequest::new().cwd(std::path::PathBuf::from(dir)),
                    None => ListSessionsRequest::new(),
                };
                let resp = cx.send_request(req).block_task().await?;
                let mut guard = sessions_capture.lock().await;
                *guard = resp.sessions;
                Ok(())
            })
            .await;
        result.map_err(|e| AcpError::Connection(e.to_string()))?;
        let guard = sessions_out.lock().await;
        Ok(guard.clone())
    }

    pub async fn probe_session_start(config: &AcpAgentConfig, cwd: String) -> Result<(), AcpError> {
        let runtime_cache_dir = default_runtime_cache_dir();
        Self::probe_session_start_for_runtime(config, cwd, &runtime_cache_dir).await
    }

    pub async fn probe_session_start_for_runtime(
        config: &AcpAgentConfig,
        cwd: String,
        runtime_cache_dir: &Path,
    ) -> Result<(), AcpError> {
        let acp_agent =
            Self::build_acp_agent_for_runtime(config, Path::new(&cwd), runtime_cache_dir)?;
        Self::probe_session_start_with_agent(acp_agent, cwd).await
    }

    async fn probe_session_start_with_agent(
        acp_agent: ManagedAcpAgent,
        cwd: String,
    ) -> Result<(), AcpError> {
        let result = Client
            .builder()
            .name("rua")
            .connect_with(acp_agent, async |cx| {
                cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                cx.build_session(Path::new(&cwd))
                    .block_task()
                    .run_until(async |_session| Ok(()))
                    .await?;
                Ok(())
            })
            .await;

        result.map_err(|e| AcpError::Connection(e.to_string()))
    }

    pub async fn probe_session_start_with_timeout(
        config: &AcpAgentConfig,
        cwd: String,
        timeout: Duration,
    ) -> Result<(), AcpError> {
        tokio::time::timeout(timeout, Self::probe_session_start(config, cwd))
            .await
            .map_err(|_| AcpError::Timeout(timeout))?
    }

    pub async fn probe_session_start_for_runtime_with_timeout(
        config: &AcpAgentConfig,
        cwd: String,
        runtime_cache_dir: &Path,
        timeout: Duration,
    ) -> Result<(), AcpError> {
        tokio::time::timeout(
            timeout,
            Self::probe_session_start_for_runtime(config, cwd, runtime_cache_dir),
        )
        .await
        .map_err(|_| AcpError::Timeout(timeout))?
    }

    async fn run_turn_persistent<Link>(
        active_session: &mut ActiveSession<'static, Link>,
        prompt: &str,
        event_tx: &mpsc::UnboundedSender<RuntimeEvent>,
        cancel: tokio_util::sync::CancellationToken,
        tool_projection: &Arc<SharedAcpToolProjection>,
    ) -> Result<(), agent_client_protocol::Error>
    where
        Link: agent_client_protocol::Role
            + agent_client_protocol::role::HasPeer<Agent>
            + Send
            + Sync
            + 'static,
    {
        active_session.send_prompt(prompt)?;

        let session_id = active_session.session_id().clone();
        let connection = active_session.connection();
        let mut cancel_sent = false;

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if !cancel_sent => {




                    cancel_sent = true;
                    tracing::info!(%session_id, "sending session/cancel to runner");
                    if let Err(e) = connection.send_notification_to(
                        Agent,
                        agent_client_protocol::schema::v1::CancelNotification::new(session_id.clone()),
                    ) {
                        tracing::warn!(error = %e, "failed to send CancelNotification");
                        break;
                    }
                }
                update = active_session.read_update() => {
                    match update? {
                        SessionMessage::SessionMessage(dispatch) => {
                            Self::dispatch_to_events(dispatch, event_tx, tool_projection).await;
                        }
                        SessionMessage::StopReason(_) => break,
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }

    async fn dispatch_to_events(
        dispatch: Dispatch,
        event_tx: &mpsc::UnboundedSender<RuntimeEvent>,
        tool_projection: &Arc<SharedAcpToolProjection>,
    ) {
        let _ = MatchDispatch::new(dispatch)
            .if_notification(async |notif: SessionNotification| {
                match notif.update {
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            let _ = event_tx.send(RuntimeEvent::TextDelta {
                                delta: text.text,
                                message_id: None,
                            });
                        }
                    }
                    SessionUpdate::AgentThoughtChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            let _ = event_tx.send(RuntimeEvent::ThinkingDelta {
                                delta: text.text,
                                signature: None,
                                message_id: None,
                            });
                        }
                    }
                    SessionUpdate::ToolCall(tool_call) => {
                        let update = tool_projection.apply_tool_call(tool_call);
                        emit_runtime_projection_update(update, event_tx);
                    }
                    SessionUpdate::ToolCallUpdate(update) => {
                        if let Some(update) = tool_projection.apply_tool_call_update(update) {
                            emit_runtime_projection_update(update, event_tx);
                        }
                    }
                    _ => {
                        tracing::trace!("unhandled ACP session update");
                    }
                }
                Ok(())
            })
            .await
            .otherwise_ignore();
    }
}

fn default_runtime_cache_dir() -> PathBuf {
    rua_config::dirs::Paths::new(None).cache_dir
}

pub fn acp_runtime_launch_plan(
    config: &AcpAgentConfig,
    cwd: &Path,
    runtime_cache_dir: &Path,
) -> RuntimeLaunchPlan {
    RuntimeAuthority::from_current_process(None, cwd, runtime_cache_dir).plan(
        ProcessShape::Stdio,
        match config.command.as_str() {
            "python" | "python3" => ToolchainIntent::Python,
            "node" => ToolchainIntent::Node,
            "npx" => ToolchainIntent::Npx,
            _ => ToolchainIntent::NodeRunner {
                command: config.command.clone(),
            },
        },
        RuntimeLaunchRequest {
            args: config.args.clone(),
            env: config.env.clone().into_iter().collect(),
            cwd: Some(cwd.to_path_buf()),
            ..Default::default()
        },
    )
}

fn emit_runtime_projection_update(
    update: AcpToolProjectionUpdate,
    event_tx: &mpsc::UnboundedSender<RuntimeEvent>,
) {
    for event in update.into_runtime_events() {
        let _ = event_tx.send(event);
    }
}

fn emit_history_projection_update(
    update: AcpToolProjectionUpdate,
    event_tx: &mpsc::UnboundedSender<HistoryPreviewEvent>,
) {
    if update.should_emit_tool_call() {
        let _ = event_tx.send(HistoryPreviewEvent::ToolCall(
            update.snapshot().tool_call.title.clone(),
        ));
    }
    if let Some(preview) = update.result_preview() {
        if !preview.is_empty() {
            let _ = event_tx.send(HistoryPreviewEvent::ToolResult(preview));
        }
    }
}

struct PermissionRequestEvaluation<'a> {
    evaluator: Option<&'a Arc<dyn PermissionEvaluator>>,
    permission_relay: &'a Arc<AcpPermissionRelay>,
    permission_context: &'a AcpPermissionRequestContext,
    trusted: bool,
    permission_scope_id: &'a str,
    pending_event_tx: Option<&'a mpsc::UnboundedSender<RuntimeEvent>>,
    tool_projection: &'a Arc<SharedAcpToolProjection>,
    turn_cancel: tokio_util::sync::CancellationToken,
}

async fn evaluate_permission_request(
    context: PermissionRequestEvaluation<'_>,
    request: &RequestPermissionRequest,
) -> RequestPermissionResponse {
    let enriched_tool_call = context
        .tool_projection
        .wait_for_enriched_tool_call_update(
            &request.tool_call,
            ACP_PERMISSION_TOOL_CORRELATION_TIMEOUT,
        )
        .await;
    let mut enriched_request = request.clone();
    if let Some(tool_call) = enriched_tool_call.clone() {
        enriched_request.tool_call = tool_call.into();
    }

    if let Some(evaluator) = context.evaluator {
        match evaluator
            .evaluate(
                &context.permission_context.agent_id,
                &context.permission_context.chat_id,
                &enriched_request,
            )
            .await
        {
            PermissionDecision::Denied { reason } => {
                tracing::info!(%reason, "ACP permission denied by host evaluator");
                return cancelled_response();
            }
            PermissionDecision::Relay => {}
        }
    }

    if context.trusted {
        if let Some(option_id) = select_unambiguous_allow_once(&enriched_request) {
            return selected_response(option_id);
        }
    };

    let (pending, resolution_rx) = context
        .permission_relay
        .create_request_in_scope(
            context.permission_scope_id.to_string(),
            context.permission_context.clone(),
            &enriched_request,
            enriched_tool_call.as_ref(),
        )
        .await;
    if let Some(event_tx) = context.pending_event_tx {
        let _ = event_tx.send(RuntimeEvent::ToolApprovalPending {
            call_id: pending.tool_call_id.clone(),
            tool_name: pending.tool_name.clone(),
            request_id: Some(pending.id.clone()),
            prompt: Some(permission_prompt(&pending)),
        });
    }

    let resolution = tokio::select! {
        _ = context.turn_cancel.cancelled() => {
            context.permission_relay.cancel_request(&pending.id).await;
            PermissionRelayResolution::Cancelled
        }
        result = tokio::time::timeout(ACP_PERMISSION_RELAY_TIMEOUT, resolution_rx) => {
            match result {
                Ok(Ok(resolution)) => resolution,
                Ok(Err(_)) => PermissionRelayResolution::Cancelled,
                Err(_) => {
                    context.permission_relay.cancel_request(&pending.id).await;
                    PermissionRelayResolution::Cancelled
                }
            }
        }
    };

    match resolution {
        PermissionRelayResolution::Selected(option_id) => selected_response(option_id),
        PermissionRelayResolution::Cancelled => cancelled_response(),
    }
}

fn permission_prompt(request: &crate::permissions::AcpPermissionRequest) -> String {
    if request
        .tool_input
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        format!(
            "ACP runner `{}` requests permission for {}",
            request.runner, request.tool_name
        )
    } else {
        format!(
            "ACP runner `{}` requests permission for {}: {}",
            request.runner,
            request.tool_name,
            serde_json::to_string(&request.tool_input).unwrap_or_default()
        )
    }
}
