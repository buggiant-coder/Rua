use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use rua_acp::client::AcpClientAdapter;

use crate::errors::ApiError;
use crate::services::acp_chat_registry::AcpChatRuntimeHandle;
use crate::services::chat_metadata_store::AcpBindingMetadata;
use crate::services::conversation_authority_placement::ConversationAuthorityEpoch;
use crate::services::conversation_authority_target::{resolve_acp_route, RemotePlacementRoute};
use crate::services::device_command_bus::{
    acp_runner_infos_from_service, AcpChatDeviceAction, AcpChatDeviceCommand,
    AcpChatDeviceResponse, AcpChatDeviceSessionEntry, AcpChatDeviceStatus,
    DeviceCommandDeliveryError,
};
use crate::state::AppState;

fn expand_tilde(path: &str) -> String {
    let home = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok());
    let Some(home) = home else {
        return path.to_string();
    };
    if path == "~" {
        return home;
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        return format!("{home}/{rest}");
    }
    path.to_string()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/chats/{chatId}/acp/runners",
            get(list_runners),
        )
        .route(
            "/agents/{agentId}/chats/{chatId}/acp/sessions",
            get(list_sessions),
        )
        .route(
            "/agents/{agentId}/chats/{chatId}/acp/enter",
            post(enter_acp),
        )
        .route("/agents/{agentId}/chats/{chatId}/acp/exit", post(exit_acp))
        .route("/agents/{agentId}/chats/{chatId}/acp/status", get(status))
        .route(
            "/agents/{agentId}/chats/{chatId}/acp/permissions",
            get(permission_snapshot),
        )
        .route(
            "/agents/{agentId}/chats/{chatId}/acp/permissions/{requestId}/decide",
            post(decide_permission),
        )
}

#[derive(Debug, Deserialize)]
struct ListSessionsQuery {
    runner: String,

    #[serde(default)]
    cwd: Option<String>,

    #[serde(default)]
    target_device_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct SessionEntry {
    session_id: String,
    title: Option<String>,
    cwd: String,
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EnterAcpBody {
    runner: String,

    #[serde(default)]
    cwd: Option<String>,

    #[serde(default)]
    resume_session_id: Option<String>,

    #[serde(default)]
    target_device_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct AcpStatusView {
    active: bool,
    runner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    cwd: Option<String>,

    chat_id: String,

    workspace_dir: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    target_device_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct EnterResponse {
    ok: bool,
    message: String,
    status: AcpStatusView,
    #[serde(skip_serializing_if = "Option::is_none")]
    history_preview: Option<String>,
}

#[derive(Debug, Serialize)]
struct ExitResponse {
    ok: bool,
    message: String,
}

#[derive(Debug, Deserialize)]
struct TargetDeviceQuery {
    #[serde(default)]
    target_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AcpPermissionDecisionBody {
    option_id: String,
}

async fn resolve_handle(
    state: &AppState,
    agent_id: &str,
) -> Result<Arc<AcpChatRuntimeHandle>, ApiError> {
    let _ = state.registry.get_or_create(agent_id).await.map_err(|e| {
        ApiError::Unavailable(format!("workspace for '{agent_id}' unavailable: {e}"))
    })?;

    let handle = state.acp_chat_registry.get(agent_id).ok_or_else(|| {
        ApiError::Internal(format!(
            "ACP chat runtime for agent '{agent_id}' not registered (command runtime factory may not have run yet)"
        ))
    })?;
    handle.refresh_config();
    Ok(handle)
}

async fn permission_snapshot(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
) -> Response {
    let workspace_dir = match workspace_dir_for_agent(&state, &agent_id).await {
        Ok(workspace_dir) => workspace_dir,
        Err(error) => return error.into_response(),
    };
    let header = match rua_session::WorkspaceSessionStore::new(&workspace_dir)
        .load_chat_header(&chat_id)
        .await
    {
        Ok(header) => header,
        Err(error) => return ApiError::from(error).into_response(),
    };
    if !header.is_some_and(|header| {
        crate::services::conversation_authority_placement::chat_acp_binding_active(
            &serde_json::Value::Object(header.extra),
        )
    }) {
        return Json(rua_acp::permissions::AcpPermissionSnapshot::default()).into_response();
    }
    match acp_placement_route(&state, &agent_id, &chat_id, None).await {
        Ok(Some(RemotePlacementRoute::Remote {
            target_device_id,
            authority_epoch,
            ..
        })) => {
            return match dispatch_remote_acp(
                &state,
                &agent_id,
                &chat_id,
                &target_device_id,
                authority_epoch,
                AcpChatDeviceAction::PermissionSnapshot,
            )
            .await
            {
                Ok(AcpChatDeviceResponse::PermissionSnapshot { snapshot }) => {
                    (StatusCode::OK, Json(snapshot)).into_response()
                }
                Ok(_) => {
                    ApiError::Internal("unexpected ACP device response".into()).into_response()
                }
                Err(error) => error.into_response(),
            };
        }
        Ok(Some(RemotePlacementRoute::MissingSourceDevice { .. })) => {
            return ApiError::Unavailable("source sync device identity is unavailable".to_string())
                .into_response();
        }
        Ok(Some(RemotePlacementRoute::Local { .. })) | Ok(None) => {}
        Err(error) => return error.into_response(),
    }
    (
        StatusCode::OK,
        Json(
            state
                .acp_permission_relay
                .snapshot(&agent_id, &chat_id)
                .await,
        ),
    )
        .into_response()
}

async fn decide_permission(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id, request_id)): AxumPath<(String, String, String)>,
    Json(body): Json<AcpPermissionDecisionBody>,
) -> Response {
    if body.option_id.trim().is_empty() {
        return ApiError::BadRequest("option_id is required".to_string()).into_response();
    }
    match acp_placement_route(&state, &agent_id, &chat_id, None).await {
        Ok(Some(RemotePlacementRoute::Remote {
            target_device_id,
            authority_epoch,
            ..
        })) => {
            return match dispatch_remote_acp(
                &state,
                &agent_id,
                &chat_id,
                &target_device_id,
                authority_epoch,
                AcpChatDeviceAction::DecidePermission {
                    request_id,
                    option_id: body.option_id,
                },
            )
            .await
            {
                Ok(AcpChatDeviceResponse::PermissionDecided { request }) => {
                    (StatusCode::OK, Json(request)).into_response()
                }
                Ok(_) => {
                    ApiError::Internal("unexpected ACP device response".into()).into_response()
                }
                Err(error) => error.into_response(),
            };
        }
        Ok(Some(RemotePlacementRoute::MissingSourceDevice { .. })) => {
            return ApiError::Unavailable("source sync device identity is unavailable".to_string())
                .into_response();
        }
        Ok(Some(RemotePlacementRoute::Local { .. })) | Ok(None) => {}
        Err(error) => return error.into_response(),
    }
    if let Some(request) = state.acp_permission_relay.request(&request_id).await {
        if request.agent_id == agent_id
            && request.chat_id == chat_id
            && request
                .options
                .iter()
                .any(|option| option.option_id == body.option_id)
        {
            let _ = state
                .conversation_authority
                .transition_active_turn_phase(
                    &rua_core::ChatAddress::agent(&agent_id, &chat_id),
                    None,
                    crate::services::conversation_authority::ConversationActiveTurnPhase::RunningTool,
                )
                .await;
        }
    }
    match state
        .acp_permission_relay
        .decide(&agent_id, &chat_id, &request_id, &body.option_id)
        .await
    {
        Ok(request) => (StatusCode::OK, Json(request)).into_response(),
        Err(rua_acp::permissions::AcpPermissionRelayError::NotFound(id)) => {
            ApiError::NotFound(id).into_response()
        }
        Err(rua_acp::permissions::AcpPermissionRelayError::WrongScope) => {
            ApiError::NotFound(request_id).into_response()
        }
        Err(rua_acp::permissions::AcpPermissionRelayError::UnknownOption(option_id)) => {
            ApiError::BadRequest(format!("unknown ACP permission option: {option_id}"))
                .into_response()
        }
    }
}

async fn build_status(handle: &AcpChatRuntimeHandle, chat_id: &str) -> AcpStatusView {
    let workspace_dir = expand_tilde(&handle.workspace_dir.to_string_lossy());
    match handle.manager.get(chat_id).await {
        Some(session) => AcpStatusView {
            active: true,
            runner: Some(session.runner),
            session_id: session.runner_session_id,
            cwd: Some(expand_tilde(&session.cwd.to_string_lossy())),
            chat_id: chat_id.to_string(),
            workspace_dir,
            target_device_id: None,
        },
        None => AcpStatusView {
            active: false,
            runner: None,
            session_id: None,
            cwd: None,
            chat_id: chat_id.to_string(),
            workspace_dir,
            target_device_id: None,
        },
    }
}

impl From<AcpChatDeviceStatus> for AcpStatusView {
    fn from(status: AcpChatDeviceStatus) -> Self {
        Self {
            active: status.active,
            runner: status.runner,
            session_id: status.session_id,
            cwd: status.cwd,
            chat_id: status.chat_id,
            workspace_dir: status.workspace_dir,
            target_device_id: Some(status.target_device_id),
        }
    }
}

impl From<AcpChatDeviceSessionEntry> for SessionEntry {
    fn from(entry: AcpChatDeviceSessionEntry) -> Self {
        Self {
            session_id: entry.session_id,
            title: entry.title,
            cwd: entry.cwd,
            updated_at: entry.updated_at,
        }
    }
}

async fn acp_placement_route(
    state: &AppState,
    agent_id: &str,
    chat_id: &str,
    explicit_target_device_id: Option<&str>,
) -> Result<Option<RemotePlacementRoute>, ApiError> {
    let workspace_dir = workspace_dir_for_agent(state, agent_id).await?;
    if chat_is_unmaterialized(&workspace_dir, chat_id).await? {
        let local_device_id = current_sync_device_id(state).ok_or_else(|| {
            ApiError::Internal("local Device identity is unavailable".to_string())
        })?;
        if let Some(explicit_target) = explicit_target_device_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if explicit_target != local_device_id {
                return Ok(Some(RemotePlacementRoute::Remote {
                    target_device_id: explicit_target.to_string(),
                    source: crate::services::conversation_authority_target::PlacementTargetSource::DraftTarget,
                    source_device_id: local_device_id.clone(),
                    authority_epoch: ConversationAuthorityEpoch::next_for_device(
                        &local_device_id,
                        None,
                    ),
                }));
            }
        }
        return Ok(None);
    }
    resolve_acp_route(
        &workspace_dir,
        chat_id,
        explicit_target_device_id,
        current_sync_device_id(state).as_deref(),
    )
    .await
    .map(Some)
    .map_err(|e| ApiError::BadRequest(e.to_string()))
}

async fn chat_is_unmaterialized(
    workspace_dir: &std::path::Path,
    chat_id: &str,
) -> Result<bool, ApiError> {
    let indexed = crate::services::chat_index::get(workspace_dir, chat_id)
        .await
        .map_err(ApiError::Internal)?;
    let header = rua_session::WorkspaceSessionStore::new(workspace_dir)
        .load_chat_header(chat_id)
        .await
        .map_err(ApiError::from)?;
    Ok(indexed.is_none() && header.is_none())
}

async fn require_materialized_acp_chat(
    state: &AppState,
    agent_id: &str,
    chat_id: &str,
) -> Result<(), ApiError> {
    let workspace_dir = workspace_dir_for_agent(state, agent_id).await?;
    if chat_is_unmaterialized(&workspace_dir, chat_id).await? {
        return Err(ApiError::NotFound(format!("Chat not found: {chat_id}")));
    }
    Ok(())
}

async fn dispatch_remote_acp(
    state: &AppState,
    agent_id: &str,
    chat_id: &str,
    target_device_id: &str,
    authority_epoch: ConversationAuthorityEpoch,
    action: AcpChatDeviceAction,
) -> Result<AcpChatDeviceResponse, ApiError> {
    let source_device_id = current_sync_device_id(state).ok_or_else(|| {
        ApiError::Unavailable("source sync device identity is unavailable".to_string())
    })?;
    let command = AcpChatDeviceCommand {
        command_id: format!("acp-chat-{}", uuid::Uuid::new_v4()),
        agent_id: agent_id.to_string(),
        address: rua_core::ChatAddress::agent(agent_id, chat_id),
        source_device_id,
        target_device_id: target_device_id.to_string(),
        authority_epoch,
        action,
    };
    crate::services::conversation_control_dispatcher::dispatch_acp_control(
        &state.conversation_input_intake,
        command,
    )
    .await
    .map_err(acp_delivery_error)
}

fn current_sync_device_id(state: &AppState) -> Option<String> {
    state.sync_device.read().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|device| hex::encode(device.device_id_short()))
    })
}

async fn workspace_dir_for_agent(
    state: &AppState,
    agent_id: &str,
) -> Result<std::path::PathBuf, ApiError> {
    let runtime = state.registry.get_or_create(agent_id).await.map_err(|e| {
        ApiError::Unavailable(format!("workspace for '{agent_id}' unavailable: {e}"))
    })?;
    Ok(runtime.workspace_dir())
}

async fn enter_acp_via_authority(
    state: &AppState,
    agent_id: &str,
    chat_id: &str,
    binding: AcpBindingMetadata,
) -> Result<(), ApiError> {
    let workspace_dir = workspace_dir_for_agent(state, agent_id).await?;
    state
        .conversation_authority
        .enter_acp(
            Some(state),
            &workspace_dir,
            &rua_core::ChatAddress::agent(agent_id, chat_id),
            agent_id,
            binding,
        )
        .await
        .map(|_| ())
        .map_err(ApiError::from)
}

async fn exit_acp_via_authority(
    state: &AppState,
    agent_id: &str,
    chat_id: &str,
) -> Result<(), ApiError> {
    let workspace_dir = workspace_dir_for_agent(state, agent_id).await?;
    state
        .conversation_authority
        .exit_acp(
            Some(state),
            &workspace_dir,
            &rua_core::ChatAddress::agent(agent_id, chat_id),
            agent_id,
        )
        .await
        .map(|_| ())
        .map_err(ApiError::from)
}

fn acp_delivery_error(error: DeviceCommandDeliveryError) -> ApiError {
    if error.is_session_version_incompatible() {
        return ApiError::UpgradeRequired(error.to_string());
    }
    match error {
        DeviceCommandDeliveryError::Rejected { reason, detail } => {
            if reason.contains("not_found") {
                ApiError::NotFound(detail)
            } else if reason.contains("active_turn")
                || reason.contains("already_active")
                || reason.contains("stale")
                || reason.contains("binding")
                || reason.contains("redirect")
            {
                ApiError::Conflict(detail)
            } else if reason.contains("disabled")
                || reason.contains("invalid")
                || reason.contains("wrong")
            {
                ApiError::BadRequest(detail)
            } else {
                ApiError::Unavailable(format!("{reason}: {detail}"))
            }
        }
        DeviceCommandDeliveryError::Timeout => {
            ApiError::Unavailable("remote ACP control timed out".to_string())
        }
        other => ApiError::Unavailable(format!("remote ACP control failed: {other}")),
    }
}

async fn list_sessions(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Query(q): Query<ListSessionsQuery>,
) -> Response {
    match acp_placement_route(&state, &agent_id, &chat_id, q.target_device_id.as_deref()).await {
        Ok(Some(RemotePlacementRoute::Remote {
            target_device_id,
            authority_epoch,
            ..
        })) => {
            return match dispatch_remote_acp(
                &state,
                &agent_id,
                &chat_id,
                &target_device_id,
                authority_epoch,
                AcpChatDeviceAction::ListSessions {
                    runner: q.runner,
                    cwd: q.cwd,
                },
            )
            .await
            {
                Ok(AcpChatDeviceResponse::Sessions { sessions }) => {
                    let entries: Vec<SessionEntry> =
                        sessions.into_iter().map(SessionEntry::from).collect();
                    (StatusCode::OK, Json(entries)).into_response()
                }
                Ok(_) => {
                    ApiError::Internal("unexpected ACP device response".into()).into_response()
                }
                Err(e) => e.into_response(),
            };
        }
        Ok(Some(RemotePlacementRoute::MissingSourceDevice { .. })) => {
            return ApiError::Unavailable("source sync device identity is unavailable".to_string())
                .into_response();
        }
        Ok(Some(RemotePlacementRoute::Local { .. })) | Ok(None) => {}
        Err(e) => return e.into_response(),
    }

    let handle = match resolve_handle(&state, &agent_id).await {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    let config = match handle.service.agent_config(&q.runner) {
        Some(c) if c.enabled => c,
        Some(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "detail": format!("ACP runner `{}` is disabled", q.runner)
                })),
            )
                .into_response();
        }
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "detail": format!("ACP runner `{}` not configured", q.runner)
                })),
            )
                .into_response();
        }
    };

    let cwd_filter: Option<String> = match q.cwd.as_deref().map(str::trim).filter(|s| !s.is_empty())
    {
        Some("all") => None,
        Some(path) => Some(expand_tilde(path)),
        None => Some(expand_tilde(&handle.workspace_dir.to_string_lossy())),
    };

    match AcpClientAdapter::list_sessions_for_runtime(&config, cwd_filter, &state.cache_dir).await {
        Ok(sessions) => {
            let entries: Vec<SessionEntry> = sessions
                .into_iter()
                .map(|s| SessionEntry {
                    session_id: s.session_id.to_string(),
                    title: s.title,
                    cwd: s.cwd.to_string_lossy().to_string(),
                    updated_at: s.updated_at,
                })
                .collect();
            (StatusCode::OK, Json(entries)).into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "detail": format!("Failed to list ACP sessions from `{}`: {}", q.runner, e)
            })),
        )
            .into_response(),
    }
}

async fn list_runners(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Query(q): Query<TargetDeviceQuery>,
) -> Response {
    match acp_placement_route(&state, &agent_id, &chat_id, q.target_device_id.as_deref()).await {
        Ok(Some(RemotePlacementRoute::Remote {
            target_device_id,
            authority_epoch,
            ..
        })) => {
            return match dispatch_remote_acp(
                &state,
                &agent_id,
                &chat_id,
                &target_device_id,
                authority_epoch,
                AcpChatDeviceAction::ListRunners,
            )
            .await
            {
                Ok(AcpChatDeviceResponse::Runners { runners }) => {
                    (StatusCode::OK, Json(runners)).into_response()
                }
                Ok(_) => {
                    ApiError::Internal("unexpected ACP device response".into()).into_response()
                }
                Err(e) => e.into_response(),
            };
        }
        Ok(Some(RemotePlacementRoute::MissingSourceDevice { .. })) => {
            return ApiError::Unavailable("source sync device identity is unavailable".to_string())
                .into_response();
        }
        Ok(Some(RemotePlacementRoute::Local { .. })) | Ok(None) => {}
        Err(e) => return e.into_response(),
    }

    let handle = match resolve_handle(&state, &agent_id).await {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    (
        StatusCode::OK,
        Json(acp_runner_infos_from_service(&handle.service)),
    )
        .into_response()
}

async fn enter_acp(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(body): Json<EnterAcpBody>,
) -> Response {
    if let Err(error) = require_materialized_acp_chat(&state, &agent_id, &chat_id).await {
        return error.into_response();
    }
    let requested_cwd = body
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    match acp_placement_route(
        &state,
        &agent_id,
        &chat_id,
        body.target_device_id.as_deref(),
    )
    .await
    {
        Ok(Some(RemotePlacementRoute::Remote {
            target_device_id,
            authority_epoch,
            ..
        })) => {
            let action = AcpChatDeviceAction::Enter {
                runner: body.runner,
                cwd: requested_cwd.clone(),
                resume_session_id: body.resume_session_id,
            };
            return match dispatch_remote_acp(
                &state,
                &agent_id,
                &chat_id,
                &target_device_id,
                authority_epoch,
                action,
            )
            .await
            {
                Ok(AcpChatDeviceResponse::Entered {
                    ok,
                    message,
                    status,
                    history_preview,
                }) => (
                    StatusCode::OK,
                    Json(EnterResponse {
                        ok,
                        message,
                        status: AcpStatusView::from(status),
                        history_preview,
                    }),
                )
                    .into_response(),
                Ok(_) => {
                    ApiError::Internal("unexpected ACP device response".into()).into_response()
                }
                Err(e) => e.into_response(),
            };
        }
        Ok(Some(RemotePlacementRoute::MissingSourceDevice { .. })) => {
            return ApiError::Unavailable("source sync device identity is unavailable".to_string())
                .into_response();
        }
        Ok(Some(RemotePlacementRoute::Local { .. })) | Ok(None) => {}
        Err(e) => return e.into_response(),
    }

    let handle = match resolve_handle(&state, &agent_id).await {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    if handle.manager.get(&chat_id).await.is_some() {
        return ApiError::Conflict("chat is already in ACP passthrough mode".to_string())
            .into_response();
    }

    let expanded_cwd: Option<String> = body
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(expand_tilde);
    let cwd_arg = expanded_cwd.as_deref();
    let (message, history_preview) = match body.resume_session_id.as_deref() {
        Some(sid) if !sid.trim().is_empty() => {
            let resumed = handle
                .controller
                .resume_with_history_preview(&chat_id, &body.runner, sid.trim(), cwd_arg)
                .await;
            (resumed.message, resumed.history_preview)
        }
        _ => {
            let message = handle
                .controller
                .enter(&chat_id, &body.runner, cwd_arg)
                .await;
            (message, None)
        }
    };

    let mut status = build_status(&handle, &chat_id).await;
    status.target_device_id = current_sync_device_id(&state);
    if status.active {
        if let Err(error) = enter_acp_via_authority(
            &state,
            &agent_id,
            &chat_id,
            AcpBindingMetadata {
                target_device_id: status.target_device_id.clone(),
                runner: status.runner.clone().unwrap_or(body.runner),
                cwd: status.cwd.clone(),
                runner_session_id: status.session_id.clone(),
            },
        )
        .await
        {
            let _ = handle.controller.exit(&chat_id).await;
            return error.into_response();
        }
    }
    let resp = EnterResponse {
        ok: status.active,
        message,
        status,
        history_preview,
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn exit_acp(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Query(q): Query<TargetDeviceQuery>,
) -> Response {
    match acp_placement_route(&state, &agent_id, &chat_id, q.target_device_id.as_deref()).await {
        Ok(Some(RemotePlacementRoute::Remote {
            target_device_id,
            authority_epoch,
            ..
        })) => {
            return match dispatch_remote_acp(
                &state,
                &agent_id,
                &chat_id,
                &target_device_id,
                authority_epoch,
                AcpChatDeviceAction::Exit,
            )
            .await
            {
                Ok(AcpChatDeviceResponse::Exited { ok, message }) => {
                    (StatusCode::OK, Json(ExitResponse { ok, message })).into_response()
                }
                Ok(_) => {
                    ApiError::Internal("unexpected ACP device response".into()).into_response()
                }
                Err(e) => e.into_response(),
            };
        }
        Ok(Some(RemotePlacementRoute::MissingSourceDevice { .. })) => {
            return ApiError::Unavailable("source sync device identity is unavailable".to_string())
                .into_response();
        }
        Ok(Some(RemotePlacementRoute::Local { .. })) | Ok(None) => {}
        Err(e) => return e.into_response(),
    }

    let handle = match resolve_handle(&state, &agent_id).await {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    let message = handle.controller.exit(&chat_id).await;
    if let Err(error) = exit_acp_via_authority(&state, &agent_id, &chat_id).await {
        return error.into_response();
    }
    let resp = ExitResponse { ok: true, message };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn status(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Query(q): Query<TargetDeviceQuery>,
) -> Response {
    match acp_placement_route(&state, &agent_id, &chat_id, q.target_device_id.as_deref()).await {
        Ok(Some(RemotePlacementRoute::Remote {
            target_device_id,
            authority_epoch,
            ..
        })) => {
            return match dispatch_remote_acp(
                &state,
                &agent_id,
                &chat_id,
                &target_device_id,
                authority_epoch,
                AcpChatDeviceAction::Status,
            )
            .await
            {
                Ok(AcpChatDeviceResponse::Status { status }) => {
                    (StatusCode::OK, Json(AcpStatusView::from(status))).into_response()
                }
                Ok(_) => {
                    ApiError::Internal("unexpected ACP device response".into()).into_response()
                }
                Err(e) => e.into_response(),
            };
        }
        Ok(Some(RemotePlacementRoute::MissingSourceDevice { .. })) => {
            return ApiError::Unavailable("source sync device identity is unavailable".to_string())
                .into_response();
        }
        Ok(Some(RemotePlacementRoute::Local { .. })) | Ok(None) => {}
        Err(e) => return e.into_response(),
    }

    let handle = match resolve_handle(&state, &agent_id).await {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    let view = build_status(&handle, &chat_id).await;
    (StatusCode::OK, Json(view)).into_response()
}
