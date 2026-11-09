use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;

use rua_channel::core::console::{
    AttachmentInfo, ConsoleChatRequest, ConsoleSubmitKind, ConsoleSubmitOutcome, SseOutputStream,
};
use rua_core::{ChatAddress, ChatScopeStore};

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::services::active_stream_broadcast::{ActiveStreamBroadcastFrame, ActiveStreamOwner};
use crate::services::chat_index;
use crate::services::console_chat::resolve_chat_id;
use crate::services::conversation_authority::ConversationControlView;
use crate::state::AppState;

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/chats/{chatId}/llm-debug-capture",
            get(agent_llm_debug_capture_status)
                .post(agent_llm_debug_capture_start)
                .delete(agent_llm_debug_capture_stop),
        )
        .route(
            "/agents/{agentId}/chats/{chatId}/console/chat",
            post(agent_console_chat_cli),
        )
        .route(
            "/agents/{agentId}/chats/{chatId}/console/submit",
            post(agent_console_chat_submit),
        )
        .route(
            "/agents/{agentId}/chats/{chatId}/console/live",
            post(agent_console_chat_live),
        )
        .route(
            "/agents/{agentId}/chats/{chatId}/console/activity",
            get(agent_console_chat_activity),
        )
        .route(
            "/agents/{agentId}/chats/{chatId}/console/stop",
            post(agent_console_stop),
        )
        .route(
            "/agents/{agentId}/console/push-messages",
            get(agent_console_push_messages),
        )
        .route(
            "/agents/{agentId}/console/push-messages/receive",
            post(agent_console_push_messages_receive),
        )
        .route(
            "/agents/{agentId}/console/recent_messages",
            get(agent_console_recent_messages),
        )
}

#[derive(Serialize)]
struct LlmDebugCaptureStatus {
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<std::path::PathBuf>,
}

impl LlmDebugCaptureStatus {
    fn inactive() -> Self {
        Self {
            active: false,
            capture_id: None,
            path: None,
        }
    }

    fn active(info: crate::services::llm_debug_capture::LlmDebugCaptureInfo) -> Self {
        Self {
            active: true,
            capture_id: Some(info.capture_id),
            path: Some(info.path),
        }
    }
}

fn agent_chat_address(agent_id: String, chat_id: String) -> ChatAddress {
    ChatAddress::agent(agent_id, chat_id)
}

async fn agent_llm_debug_capture_status(
    State(state): State<AppState>,
    Path((agent_id, chat_id)): Path<(String, String)>,
) -> Json<LlmDebugCaptureStatus> {
    let address = agent_chat_address(agent_id, chat_id);
    let scope_root = ChatScopeStore::resolve(&state.rua_root, address.clone());
    Json(
        state
            .llm_debug_captures
            .get(scope_root.root(), &address)
            .ok()
            .flatten()
            .map(LlmDebugCaptureStatus::active)
            .unwrap_or_else(LlmDebugCaptureStatus::inactive),
    )
}

async fn agent_llm_debug_capture_start(
    State(state): State<AppState>,
    Path((agent_id, chat_id)): Path<(String, String)>,
) -> Json<LlmDebugCaptureStatus> {
    let address = agent_chat_address(agent_id, chat_id);
    let scope_root = ChatScopeStore::resolve(&state.rua_root, address.clone());
    let status = match state.llm_debug_captures.start(scope_root.root(), address) {
        Ok(info) => LlmDebugCaptureStatus::active(info),
        Err(error) => {
            tracing::warn!(%error, "failed to start local LLM debug capture");
            LlmDebugCaptureStatus::inactive()
        }
    };
    Json(status)
}

async fn agent_llm_debug_capture_stop(
    State(state): State<AppState>,
    Path((agent_id, chat_id)): Path<(String, String)>,
) -> Json<LlmDebugCaptureStatus> {
    let address = agent_chat_address(agent_id, chat_id);
    let scope_root = ChatScopeStore::resolve(&state.rua_root, address.clone());
    if let Err(error) = state.llm_debug_captures.stop(scope_root.root(), &address) {
        tracing::warn!(%error, "failed to stop local LLM debug capture");
    }
    Json(LlmDebugCaptureStatus::inactive())
}

fn contains_approval_response(
    input: &Option<Vec<serde_json::Value>>,
    messages: &Option<Vec<serde_json::Value>>,
) -> bool {
    for arr in [input, messages].into_iter().flatten() {
        for msg in arr {
            if msg.get("type").and_then(|t| t.as_str()) == Some("tool_approval_response") {
                return true;
            }
        }
    }
    false
}

fn is_approve_or_deny(msg: &str) -> Option<bool> {
    let trimmed = msg.trim();
    if trimmed.eq_ignore_ascii_case("/approve") {
        Some(true)
    } else if trimmed.eq_ignore_ascii_case("/deny") {
        Some(false)
    } else {
        None
    }
}

async fn resolve_pending_ask_user(
    state: &AppState,
    agent_id: &str,
    chat_id: &str,
    answer: &str,
) -> Result<(), ApiError> {
    let svc = rua_tool::extended::ask_user::AskUserService::global();
    let Some(pending) = svc.get_pending_by_session(chat_id).await else {
        return Ok(());
    };
    if svc.resolve(&pending.request_id, answer).await.is_none() {
        return Ok(());
    }

    let runtime = state
        .registry
        .get_or_create(agent_id)
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    crate::services::console_chat::clear_pending_action_snapshot(&runtime.workspace_dir(), chat_id)
        .await
        .map_err(ApiError::from)
}

#[derive(Debug, Serialize)]
struct ConsoleSubmitResponse {
    accepted: bool,
    mode: &'static str,
    detail: Option<String>,
    chat_id: Option<String>,
    input_id: Option<String>,
    submission_id: Option<String>,
    device_delivery_id: Option<String>,
    target_device_id: Option<String>,
    device_delivery_sequence: Option<u64>,
    accepted_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_kind: Option<String>,
    control: Option<ConversationControlView>,
}

async fn console_submit_response(
    state: &AppState,
    agent_id: &str,
    outcome: ConsoleSubmitOutcome,
) -> ConsoleSubmitResponse {
    let control = match outcome
        .control
        .as_ref()
        .and_then(|value| serde_json::from_value(value.clone()).ok())
    {
        Some(control) => Some(control),
        None => match outcome.chat_id.as_deref() {
            Some(chat_id) => {
                let meta = match state.registry.get_or_create(agent_id).await {
                    Ok(runtime) => match crate::services::conversation_authority_target::load_chat_control_meta(
                        &runtime.workspace_dir(),
                        chat_id,
                    )
                    .await
                    {
                        Ok(Some(meta)) => meta,
                        Ok(None) | Err(_) => serde_json::json!({}),
                    },
                    Err(_) => serde_json::json!({}),
                };
                Some(
                    state
                        .conversation_authority
                        .control_view(&rua_core::ChatAddress::agent(agent_id, chat_id), &meta)
                        .await,
                )
            }
            None => None,
        },
    };
    ConsoleSubmitResponse {
        accepted: outcome.accepted,
        mode: match outcome.kind {
            ConsoleSubmitKind::Started => "started",
            ConsoleSubmitKind::Queued => "queued",
            ConsoleSubmitKind::Rejected => "rejected",
        },
        detail: outcome.detail,
        chat_id: outcome.chat_id,
        input_id: outcome.input_id,
        submission_id: outcome.submission_id,
        device_delivery_id: outcome.device_delivery_id,
        target_device_id: outcome.target_device_id,
        device_delivery_sequence: outcome.device_delivery_sequence,
        accepted_at: outcome.accepted_at,
        failure_kind: outcome.failure_kind,
        control,
    }
}

async fn agent_console_chat_cli(
    State(state): State<AppState>,
    Path((agent_id, chat_id)): Path<(String, String)>,
    Json(mut body): Json<dto::ChatRequest>,
) -> Response {
    let chat_id = match bind_agent_chat_request_to_path(&mut body, &chat_id) {
        Ok(chat_id) => chat_id,
        Err(error) => return error.into_response(),
    };
    let request = match prepare_agent_console_request(&state, &agent_id, body, false).await {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };

    state.usage_stats.bump_agent(&agent_id).await;
    crate::endpoints::chat_cli::submit_single_turn(
        &state,
        ChatAddress::agent(&agent_id, chat_id),
        &agent_id,
        request,
    )
    .await
}

async fn agent_console_chat_submit(
    State(state): State<AppState>,
    Path((agent_id, chat_id)): Path<(String, String)>,
    Json(mut body): Json<dto::ChatRequest>,
) -> Response {
    if let Err(error) = bind_agent_chat_request_to_path(&mut body, &chat_id) {
        return error.into_response();
    }
    let request = match prepare_agent_console_request(&state, &agent_id, body, true).await {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };

    state.usage_stats.bump_agent(&agent_id).await;
    let outcome = state.console_facade.submit_chat(&agent_id, request).await;
    Json(console_submit_response(&state, &agent_id, outcome).await).into_response()
}

fn bind_agent_chat_request_to_path(
    body: &mut dto::ChatRequest,
    path_chat_id: &str,
) -> Result<String, ApiError> {
    let chat_id = resolve_chat_id(path_chat_id).map_err(ApiError::BadRequest)?;
    if let Some(body_chat_id) = body.effective_chat_id() {
        if body_chat_id != chat_id {
            return Err(ApiError::BadRequest(
                "Agent Chat path and request body identify different Chats".to_string(),
            ));
        }
    }
    body.chat_id = Some(chat_id.clone());
    Ok(chat_id)
}

async fn prepare_agent_console_request(
    state: &AppState,
    agent_id: &str,
    mut body: dto::ChatRequest,
    apply_webui_submit_preparation: bool,
) -> Result<ConsoleChatRequest, ApiError> {
    let effective_msg = body.effective_message();
    let text_approval = is_approve_or_deny(&effective_msg);
    let structured_approval = contains_approval_response(&body.input, &body.messages);

    if text_approval.is_some() || structured_approval {
        return Err(ApiError::BadRequest(
            "approval decisions must use the approval control endpoint".to_string(),
        ));
    }

    if !body.has_user_content() {
        return Err(ApiError::BadRequest("message is required".to_string()));
    }

    let attachments = body
        .attachments
        .iter()
        .map(attachment_info_from_dto)
        .collect();

    let chat_message = effective_msg;
    let chat_id = body.effective_chat_id();
    let requested_model = body.structured_model().cloned();
    let requested_model_id = body.prepare_model_for_console();

    if let Some(ref chat_id) = chat_id {
        let resolved_chat_id = resolve_chat_id(chat_id).map_err(ApiError::BadRequest)?;
        if apply_webui_submit_preparation {
            let runtime = state
                .registry
                .get_or_create(agent_id)
                .await
                .map_err(|err| ApiError::Internal(format!("workspace unavailable: {err}")))?;
            crate::services::conversation_authority_target::validate_chat_submit_materialization(
                &runtime.workspace_dir(),
                &resolved_chat_id,
                body.extra
                    .get("expected_authority_epoch")
                    .and_then(serde_json::Value::as_str),
            )
            .await
            .map_err(ApiError::Conflict)?;
            let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
            if let Some(header) = store
                .load_chat_header(&resolved_chat_id)
                .await
                .map_err(ApiError::from)?
            {
                let local =
                    crate::services::device_context::current_sync_device_id(&state.sync_device);
                let placement = crate::services::conversation_authority_target::load_chat_authority_placement_target(&runtime.workspace_dir(), &resolved_chat_id).await.map_err(ApiError::Conflict)?;
                if local.as_deref() == Some(placement.device_id.as_str()) {
                    let configuration = header
                        .extra
                        .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
                        .cloned()
                        .ok_or_else(|| {
                            ApiError::Conflict("Chat configuration snapshot is unavailable".into())
                        })?;
                    crate::services::product_hooks::ChatHookConfiguration::from_persisted(
                        configuration,
                    )
                    .map_err(ApiError::Conflict)?;
                }
            }
            if !chat_message.trim().is_empty() {
                resolve_pending_ask_user(state, agent_id, chat_id, &chat_message).await?;
            }
        }
    }

    let mut extra = body.extra;
    if let Some(user_id) = body.user_id {
        extra.insert("user_id".into(), serde_json::json!(user_id));
    }
    if let Some(channel) = body.channel {
        extra.insert("channel".into(), serde_json::json!(channel));
    }
    if let Some(model) = requested_model {
        extra.insert(
            "chat_model_slot".into(),
            serde_json::to_value(model).map_err(|error| ApiError::BadRequest(error.to_string()))?,
        );
    }

    Ok(ConsoleChatRequest {
        message: chat_message,
        chat_id,
        model: requested_model_id,
        is_reconnect: false,
        attachments,
        input: body.input,
        messages: body.messages,
        extra,
        channel_kind: Some(rua_channel::core::renderer::ChannelKind::Chat),
    })
}

async fn agent_console_chat_live(
    State(state): State<AppState>,
    Path((agent_id, path_chat_id)): Path<(String, String)>,
    Json(body): Json<dto::ChatLiveRequest>,
) -> Response {
    let chat_id = match resolve_chat_id(&path_chat_id) {
        Ok(chat_id) => chat_id,
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    if let Some(body_chat_id) = body.effective_chat_id() {
        if body_chat_id != chat_id {
            return ApiError::BadRequest(
                "Agent Chat path and request body identify different Chats".to_string(),
            )
            .into_response();
        }
    }
    let Some(viewer_id) = body.viewer_id.as_deref() else {
        return ApiError::BadRequest("viewer_id is required".to_string()).into_response();
    };

    let stream = state
        .console_facade
        .connect_chat_activity(&agent_id, &chat_id, viewer_id, body.activity_position())
        .await;

    conversation_live_sse_response(stream)
}

async fn agent_console_chat_activity(
    State(state): State<AppState>,
    Path((agent_id, _chat_id)): Path<(String, String)>,
) -> Response {
    let stream = chat_activity_stream(state, ChatActivityStreamScope::Agent(agent_id))
        .map(|r| r.map(|data| format!("{data}\n")));
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

pub(crate) fn scoped_chat_activity_stream(
    state: AppState,
    address: ChatAddress,
) -> impl futures::Stream<Item = Result<String, Infallible>> + Send + 'static {
    chat_activity_stream(state, ChatActivityStreamScope::Address(address))
}

#[derive(Clone)]
enum ChatActivityStreamScope {
    Agent(String),
    Address(ChatAddress),
}

impl ChatActivityStreamScope {
    fn matches(&self, address: &ChatAddress) -> bool {
        match self {
            Self::Agent(agent_id) => {
                address.agent_id().map(|id| id.as_str()) == Some(agent_id.as_str())
            }
            Self::Address(expected) => address == expected,
        }
    }
}

fn chat_activity_stream(
    state: AppState,
    scope: ChatActivityStreamScope,
) -> impl futures::Stream<Item = Result<String, Infallible>> + Send + 'static {
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<Result<String, Infallible>>(64);

    {
        let hub = Arc::clone(&state.active_stream_broadcast_hub);
        let out_tx = out_tx.clone();
        let scope = scope.clone();
        tokio::spawn(async move {
            let mut subscription = match &scope {
                ChatActivityStreamScope::Agent(agent_id) => hub.subscribe_agent(agent_id).await,
                ChatActivityStreamScope::Address(address) => {
                    hub.subscribe_chat(address, &BTreeMap::new()).await
                }
            };
            while let Some(rx) = subscription.rx.as_mut() {
                let frame = tokio::select! {
                    _ = out_tx.closed() => return,
                    frame = rx.recv() => frame,
                };
                let Some(frame) = frame else {
                    return;
                };
                if let Some(payload) = chat_activity_payload(frame, None) {
                    if out_tx.send(Ok(payload)).await.is_err() {
                        return;
                    }
                }
            }
        });
    }

    {
        let mut wakes = state.chat_activity_wake_hub.subscribe();
        let out_tx = out_tx.clone();
        let scope = scope.clone();
        tokio::spawn(async move {
            loop {
                let wake = tokio::select! {
                    _ = out_tx.closed() => return,
                    wake = wakes.recv() => wake,
                };
                match wake {
                    Ok(wake) if scope.matches(&wake.address) => {
                        let payload = chat_activity_wake_payload(&wake);
                        if out_tx.send(Ok(payload)).await.is_err() {
                            return;
                        }
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    futures::stream::unfold(out_rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}

fn chat_activity_payload(
    frame: ActiveStreamBroadcastFrame,
    authority_device_id: Option<&str>,
) -> Option<String> {
    let (device_delivery_id, address, phase) = match frame {
        ActiveStreamBroadcastFrame::Started {
            device_delivery_id,
            owner,
            ..
        } => (
            device_delivery_id,
            owner.as_chat_address()?.clone(),
            "started",
        ),
        ActiveStreamBroadcastFrame::Done {
            device_delivery_id,
            owner,
            ..
        } => (device_delivery_id, owner.as_chat_address()?.clone(), "done"),
        ActiveStreamBroadcastFrame::Error {
            device_delivery_id,
            owner,
            ..
        } => (
            device_delivery_id,
            owner
                .as_ref()
                .and_then(ActiveStreamOwner::as_chat_address)?
                .clone(),
            "error",
        ),
        _ => return None,
    };
    Some(
        chat_activity_payload_value(
            &address,
            device_delivery_id,
            phase,
            authority_device_id,
            None,
            None,
        )
        .to_string(),
    )
}

fn chat_activity_wake_payload(wake: &crate::services::chat_activity_wake::ChatLiveWake) -> String {
    chat_activity_payload_value(
        &wake.address,
        wake.stream_id.clone(),
        &wake.phase,
        Some(&wake.authority_device_id),
        Some(wake.version),
        Some(wake.authority_epoch),
    )
    .to_string()
}

fn chat_activity_payload_value(
    address: &ChatAddress,
    stream_id: String,
    phase: &str,
    authority_device_id: Option<&str>,
    version: Option<u16>,
    authority_epoch: Option<
        crate::services::conversation_authority_placement::ConversationAuthorityEpoch,
    >,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "type": "chat_live_wake",
        "address": address,
        "chat_id": address.chat_id(),
        "stream_id": stream_id,
        "phase": phase,
        "authority_device_id": authority_device_id,
    });
    let object = payload
        .as_object_mut()
        .expect("Chat Live wake payload is an object");
    if let Some(agent_id) = address.agent_id() {
        object.insert("agent_id".to_string(), serde_json::json!(agent_id));
    }
    if let Some(project_id) = address.project_id() {
        object.insert("project_id".to_string(), serde_json::json!(project_id));
    }
    if let Some(version) = version {
        object.insert("version".to_string(), serde_json::json!(version));
    }
    if let Some(authority_epoch) = authority_epoch {
        object.insert(
            "authority_epoch".to_string(),
            serde_json::json!(authority_epoch),
        );
    }
    payload
}

async fn agent_console_stop(
    State(state): State<AppState>,
    Path((agent_id, chat_id)): Path<(String, String)>,
    Json(body): Json<dto::StopChatRequest>,
) -> Response {
    let workspace = state.rua_root.join("workspaces").join(&agent_id);
    match super::chats::dispatch_agent_chat_control(
        &state,
        &agent_id,
        &workspace,
        &chat_id,
        body.into_action(),
    )
    .await
    {
        Ok(
            crate::services::device_command_bus::ConversationControlDeviceResponse::StopRequested {
                applied,
            },
        ) => precise_stop_response(applied),
        Ok(_) => {
            crate::errors::ApiError::Internal("unexpected stop response".into()).into_response()
        }
        Err(error) => error.into_response(),
    }
}

pub(crate) fn precise_stop_response(applied: bool) -> Response {
    Json(serde_json::json!({"stopped": applied,
        "status": if applied { "stop_requested" } else { "target_changed" },
        "detail": if applied { None } else { Some("The observed Turn is no longer active; refresh before stopping again.") }
    })).into_response()
}

async fn agent_console_push_messages(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(query): Query<PushMessagesQuery>,
) -> Response {
    do_push_messages(&state, &agent_id, query.session_id.as_deref()).await
}

async fn agent_console_push_messages_receive(Path(_agent_id): Path<String>) -> Response {
    let resp = dto::SuccessResponse::ok();
    (StatusCode::OK, Json(resp)).into_response()
}

async fn agent_console_recent_messages(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(query): Query<RecentMessagesQuery>,
) -> Response {
    do_recent_messages(&state, &agent_id, &query).await
}

pub(crate) fn conversation_live_sse_response(stream: SseOutputStream) -> Response {
    let stream = stream.map(|result| result.map(|payload| encode_sse_payload(&payload)));
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(stream))
        .expect("valid conversation Live SSE response")
}

fn encode_sse_payload(payload: &str) -> String {
    let payload = payload.trim_end_matches(['\r', '\n']);
    if payload.starts_with(':') {
        return format!("{payload}\n\n");
    }

    let mut encoded = String::new();
    for line in payload.lines() {
        encoded.push_str("data: ");
        encoded.push_str(line);
        encoded.push('\n');
    }
    encoded.push('\n');
    encoded
}

#[derive(Debug, Deserialize)]
struct RecentMessagesQuery {
    #[serde(default = "default_recent_limit")]
    limit: usize,
    chat_id: Option<String>,
}

fn default_recent_limit() -> usize {
    20
}

#[derive(Debug, Deserialize)]
pub struct PushMessagesQuery {
    pub session_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ConsolePendingApprovalInfo {
    request_id: String,
    tool_name: String,
    #[serde(with = "chrono::serde::ts_seconds")]
    created_at: chrono::DateTime<chrono::Utc>,
    expiry_seconds: u64,
}

async fn do_push_messages(state: &AppState, agent_id: &str, session_id: Option<&str>) -> Response {
    let all = state.push_message_store.list().await;
    let messages = all
        .into_iter()
        .filter(|m| match m.extra.get("agent_id").and_then(|v| v.as_str()) {
            Some(message_agent_id) => message_agent_id == agent_id,
            None => true,
        })
        .filter(|m| match session_id {
            Some(sid) => m.extra.get("session_id").and_then(|v| v.as_str()) == Some(sid),
            None => true,
        })
        .collect::<Vec<_>>();
    let pending_approvals = state
        .security
        .approval_authority()
        .list_pending_for_agent(agent_id)
        .await
        .into_iter()
        .filter(|approval| match session_id {
            Some(sid) => {
                approval
                    .owner
                    .as_chat_address()
                    .is_some_and(|address| address.chat_id().as_str() == sid)
                    || approval.owner.is_runtime_session(agent_id, sid)
            }
            None => true,
        })
        .map(|approval| ConsolePendingApprovalInfo {
            request_id: approval.id,
            tool_name: approval.tool_name,
            created_at: approval.created_at,
            expiry_seconds: 0,
        })
        .collect::<Vec<_>>();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "messages": messages,
            "pending_approvals": pending_approvals,
        })),
    )
        .into_response()
}

async fn resolve_workspace_dir(state: &AppState, agent_id: &str) -> Option<std::path::PathBuf> {
    match state.registry.get_or_create(agent_id).await {
        Ok(runtime) => Some(runtime.workspace_dir()),
        Err(_) => None,
    }
}

async fn do_recent_messages(
    state: &AppState,
    agent_id: &str,
    query: &RecentMessagesQuery,
) -> Response {
    let ws_dir = match resolve_workspace_dir(state, agent_id).await {
        Some(d) => d,
        None => return (StatusCode::OK, Json(serde_json::json!([]))).into_response(),
    };

    let chat_id = match &query.chat_id {
        Some(id) => id.clone(),
        None => match find_latest_chat_id(&ws_dir).await {
            Some(id) => id,
            None => return (StatusCode::OK, Json(serde_json::json!([]))).into_response(),
        },
    };

    let store = rua_session::WorkspaceSessionStore::new(&ws_dir);
    match store.load_chat_header(&chat_id).await {
        Ok(Some(header)) => {
            let turns = store
                .read_turn_range(&chat_id, 1, usize::MAX)
                .await
                .unwrap_or_default();
            let active_latest_sequence = turns
                .last()
                .map(|turn| turn.last_sequence)
                .unwrap_or(header.message_log.latest_sequence);
            let limit = query.limit;
            let start_sequence = active_latest_sequence
                .saturating_sub(limit.saturating_sub(1) as u64)
                .max(1);
            let read_limit = active_latest_sequence
                .saturating_sub(start_sequence)
                .saturating_add(1) as usize;
            let json_messages = if active_latest_sequence == 0 || limit == 0 {
                Vec::new()
            } else {
                store
                    .read_message_range(&chat_id, start_sequence, read_limit)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(message_log_entry_to_recent_message_json)
                    .collect()
            };
            return (StatusCode::OK, Json(json_messages)).into_response();
        }
        Ok(None) => {}
        Err(_) => return (StatusCode::OK, Json(serde_json::json!([]))).into_response(),
    }

    (StatusCode::OK, Json(serde_json::json!([]))).into_response()
}

fn message_log_entry_to_recent_message_json(
    message: rua_session::MessageLogEntry,
) -> serde_json::Value {
    let name = message
        .metadata
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&message.role);
    serde_json::json!({
        "id": message.message_id,
        "role": message.role,
        "content": message.content,
        "name": name,
        "timestamp": message.created_at,
        "metadata": message.metadata,
    })
}

async fn find_latest_chat_id(ws_dir: &std::path::Path) -> Option<String> {
    chat_index::list(ws_dir, chat_index::ChatIndexFilter::default(), 0, 1)
        .await
        .ok()
        .and_then(|page| page.entries.into_iter().next())
        .map(|entry| entry.id)
}

fn attachment_info_from_dto(attachment: &dto::AttachmentDto) -> AttachmentInfo {
    AttachmentInfo {
        url: attachment.url.clone(),
        name: attachment.name.clone(),
        mime_type: attachment.mime_type.clone(),
        stored_name: attachment.stored_name.clone(),
        size: attachment.size,
    }
}
