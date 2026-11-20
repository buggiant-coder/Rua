use crate::extractors::ChatPath;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::errors::ApiError;
use crate::services::conversation_authority_placement::ConversationAuthorityEpoch;
use crate::services::conversation_authority_target::{resolve_acp_route, RemotePlacementRoute};
use crate::services::device_command_bus::{
    AcpChatDeviceAction, AcpChatDeviceCommand, AcpChatDeviceResponse, AcpChatDeviceStatus,
    DeviceCommandDeliveryError,
};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chats/{chatId}/acp/runners", get(list_runners))
        .route("/chats/{chatId}/acp/sessions", get(list_sessions))
        .route("/chats/{chatId}/acp/enter", post(enter_acp))
        .route("/chats/{chatId}/acp/exit", post(exit_acp))
        .route("/chats/{chatId}/acp/status", get(status))
        .route("/chats/{chatId}/acp/permissions", get(permission_snapshot))
        .route(
            "/chats/{chatId}/acp/permissions/{requestId}/decide",
            post(decide_permission),
        )
}

#[derive(Debug, Deserialize)]
struct ListSessionsQuery {
    agent_id: Option<String>,
    runner: String,

    #[serde(default)]
    cwd: Option<String>,

    #[serde(default)]
    target_device_id: Option<String>,
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
struct EnterResponse {
    ok: bool,
    message: String,
    status: AcpChatDeviceStatus,
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
    agent_id: Option<String>,
    #[serde(default)]
    target_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AcpPermissionDecisionBody {
    option_id: String,
}

async fn permission_snapshot(
    State(state): State<AppState>,
    ChatPath((agent_id, chat_id)): ChatPath<(String, String)>,
) -> Response {
    let header = match rua_session::WorkspaceSessionStore::new(state.chat_workspace().as_path())
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
    match dispatch_acp(
        &state,
        &agent_id,
        &chat_id,
        None,
        AcpChatDeviceAction::PermissionSnapshot,
    )
    .await
    {
        Ok(AcpChatDeviceResponse::PermissionSnapshot { snapshot }) => {
            (StatusCode::OK, Json(snapshot)).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected ACP device response".into()).into_response(),
        Err(response) => response,
    }
}

async fn decide_permission(
    State(state): State<AppState>,
    ChatPath((agent_id, chat_id, request_id)): ChatPath<(String, String, String)>,
    Json(body): Json<AcpPermissionDecisionBody>,
) -> Response {
    if body.option_id.trim().is_empty() {
        return ApiError::BadRequest("option_id is required".to_string()).into_response();
    }
    match dispatch_acp(
        &state,
        &agent_id,
        &chat_id,
        None,
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
        Ok(_) => ApiError::Internal("unexpected ACP device response".into()).into_response(),
        Err(response) => response,
    }
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

async fn dispatch_acp(
    state: &AppState,
    agent_id: &str,
    chat_id: &str,
    requested_device: Option<&str>,
    action: AcpChatDeviceAction,
) -> Result<AcpChatDeviceResponse, Response> {
    let source_device_id = current_sync_device_id(state).ok_or_else(|| {
        ApiError::Unavailable("source sync device identity is unavailable".into()).into_response()
    })?;
    let workspace = state.chat_workspace();
    let (target_device_id, authority_epoch) =
        if chat_is_unmaterialized(workspace.as_path(), chat_id)
            .await
            .map_err(IntoResponse::into_response)?
        {
            let device = requested_device
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .unwrap_or(&source_device_id);
            (
                device.to_owned(),
                ConversationAuthorityEpoch::next_for_device(&source_device_id, None),
            )
        } else {
            match resolve_acp_route(
                workspace.as_path(),
                chat_id,
                requested_device,
                Some(&source_device_id),
            )
            .await
            .map_err(|error| ApiError::BadRequest(error).into_response())?
            {
                RemotePlacementRoute::Local {
                    authority_device_id,
                    authority_epoch,
                } => (authority_device_id, authority_epoch),
                RemotePlacementRoute::Remote {
                    target_device_id,
                    authority_epoch,
                    ..
                } => (target_device_id, authority_epoch),
                RemotePlacementRoute::MissingSourceDevice { .. } => {
                    return Err(ApiError::Unavailable(
                        "source sync device identity is unavailable".into(),
                    )
                    .into_response())
                }
            }
        };
    let local = source_device_id == target_device_id;
    let command = AcpChatDeviceCommand {
        command_id: format!("acp-chat-{}", uuid::Uuid::new_v4()),
        agent_id: agent_id.to_string(),
        address: rua_core::ChatAddress::new(chat_id),
        source_device_id,
        target_device_id,
        authority_epoch,
        action,
    };
    crate::services::conversation_control_dispatcher::dispatch_acp_control(
        &state.conversation_input_intake,
        command,
    )
    .await
    .map_err(|error| match error {
        DeviceCommandDeliveryError::Rejected { reason, detail }
            if local && reason == "acp_list_sessions_failed" =>
        {
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"detail": detail})),
            )
                .into_response()
        }
        other => acp_delivery_error(other).into_response(),
    })
}

fn current_sync_device_id(state: &AppState) -> Option<String> {
    state.sync_device.read().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|device| hex::encode(device.device_id_short()))
    })
}

fn acp_delivery_error(error: DeviceCommandDeliveryError) -> ApiError {
    if error.is_session_version_incompatible() {
        return ApiError::UpgradeRequired(error.to_string());
    }
    match error {
        DeviceCommandDeliveryError::Rejected { reason, detail } => {
            if reason.contains("not_found") || reason == "acp_permission_wrong_scope" {
                ApiError::NotFound(detail)
            } else if reason.contains("active_turn")
                || reason.contains("already_active")
                || reason.contains("stale")
                || reason.contains("binding")
                || reason.contains("redirect")
            {
                ApiError::Conflict(detail)
            } else if reason == "acp_permission_unknown_option"
                || reason.contains("disabled")
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
    axum::extract::Path(chat_id): axum::extract::Path<String>,
    Query(q): Query<ListSessionsQuery>,
) -> Response {
    let agent_id = match crate::services::chat_context::resolve_submission_context(
        &state,
        &chat_id,
        q.agent_id.as_deref(),
        None,
    )
    .await
    {
        Ok(context) => context.agent_id,
        Err(error) => return error.into_response(),
    };
    match dispatch_acp(
        &state,
        &agent_id,
        &chat_id,
        q.target_device_id.as_deref(),
        AcpChatDeviceAction::ListSessions {
            runner: q.runner,
            cwd: q.cwd,
        },
    )
    .await
    {
        Ok(AcpChatDeviceResponse::Sessions { sessions }) => Json(sessions).into_response(),
        Ok(_) => ApiError::Internal("unexpected ACP device response".into()).into_response(),
        Err(response) => response,
    }
}

async fn list_runners(
    State(state): State<AppState>,
    axum::extract::Path(chat_id): axum::extract::Path<String>,
    Query(q): Query<TargetDeviceQuery>,
) -> Response {
    let agent_id = match crate::services::chat_context::resolve_submission_context(
        &state,
        &chat_id,
        q.agent_id.as_deref(),
        None,
    )
    .await
    {
        Ok(context) => context.agent_id,
        Err(error) => return error.into_response(),
    };
    match dispatch_acp(
        &state,
        &agent_id,
        &chat_id,
        q.target_device_id.as_deref(),
        AcpChatDeviceAction::ListRunners,
    )
    .await
    {
        Ok(AcpChatDeviceResponse::Runners { runners }) => {
            (StatusCode::OK, Json(runners)).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected ACP device response".into()).into_response(),
        Err(response) => response,
    }
}

async fn enter_acp(
    State(state): State<AppState>,
    ChatPath((agent_id, chat_id)): ChatPath<(String, String)>,
    Json(body): Json<EnterAcpBody>,
) -> Response {
    let requested_cwd = body
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let action = AcpChatDeviceAction::Enter {
        runner: body.runner,
        cwd: requested_cwd,
        resume_session_id: body.resume_session_id,
    };
    match dispatch_acp(
        &state,
        &agent_id,
        &chat_id,
        body.target_device_id.as_deref(),
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
                status,
                history_preview,
            }),
        )
            .into_response(),
        Ok(_) => ApiError::Internal("unexpected ACP device response".into()).into_response(),
        Err(response) => response,
    }
}

async fn exit_acp(
    State(state): State<AppState>,
    ChatPath((agent_id, chat_id)): ChatPath<(String, String)>,
    Query(q): Query<TargetDeviceQuery>,
) -> Response {
    match dispatch_acp(
        &state,
        &agent_id,
        &chat_id,
        q.target_device_id.as_deref(),
        AcpChatDeviceAction::Exit,
    )
    .await
    {
        Ok(AcpChatDeviceResponse::Exited { ok, message }) => {
            (StatusCode::OK, Json(ExitResponse { ok, message })).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected ACP device response".into()).into_response(),
        Err(response) => response,
    }
}

async fn status(
    State(state): State<AppState>,
    ChatPath((agent_id, chat_id)): ChatPath<(String, String)>,
    Query(q): Query<TargetDeviceQuery>,
) -> Response {
    match dispatch_acp(
        &state,
        &agent_id,
        &chat_id,
        q.target_device_id.as_deref(),
        AcpChatDeviceAction::Status,
    )
    .await
    {
        Ok(AcpChatDeviceResponse::Status { status }) => {
            (StatusCode::OK, Json(status)).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected ACP device response".into()).into_response(),
        Err(response) => response,
    }
}
