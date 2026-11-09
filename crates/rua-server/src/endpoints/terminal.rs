use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::errors::ApiError;
use crate::services::terminal::{
    terminal_activity_frame_json, TerminalActivityFrame, TerminalExecutionDevice,
    TerminalPlacementError, TerminalPlacementService, TerminalStreamCursor,
    TERMINAL_SCROLLBACK_MAX_LINES, TERMINAL_SCROLLBACK_MIN_LINES,
};
use crate::state::AppState;
use crate::streaming::{sse_json_event, sse_response};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/terminal/profiles", get(list_profiles))
        .route("/terminal/sessions", get(list_sessions).post(open_session))
        .route(
            "/terminal/sessions/{session_id}/attach",
            post(attach_session),
        )
        .route(
            "/terminal/sessions/{session_id}/scrollback",
            post(update_scrollback),
        )
        .route(
            "/terminal/sessions/{session_id}/detach",
            post(detach_session),
        )
        .route(
            "/terminal/sessions/{session_id}/activity",
            get(activity_stream),
        )
        .route(
            "/terminal/sessions/{session_id}/take-input-lease",
            post(take_input_lease),
        )
        .route(
            "/terminal/sessions/{session_id}/adapt-pty-geometry",
            post(adapt_pty_geometry),
        )
        .route(
            "/terminal/sessions/{session_id}/palette",
            post(update_palette),
        )
        .route(
            "/terminal/sessions/{session_id}/rename",
            post(rename_session),
        )
        .route("/terminal/sessions/{session_id}/input", post(send_input))
        .route("/terminal/sessions/{session_id}", delete(kill_session))
}

#[derive(Debug, Deserialize)]
struct ListSessionsQuery {
    execution_device: String,
}

#[derive(Debug, Deserialize)]
struct SessionExecutionQuery {
    #[serde(default)]
    execution_device: Option<String>,
    #[serde(default)]
    browser_viewer_id: Option<String>,
    #[serde(default)]
    next_activity_sequence: u64,
    #[serde(default)]
    next_output_offset: u64,
    #[serde(default = "default_true")]
    retain_authority: bool,
}

#[derive(Debug, Deserialize)]
struct OpenSessionRequest {
    execution_device: ExecutionDeviceBody,
    browser_viewer_id: String,
    scrollback_lines: usize,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    terminal_profile_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ViewerRequest {
    browser_viewer_id: String,
    #[serde(default)]
    scrollback_lines: Option<usize>,
    #[serde(default)]
    cursor: Option<TerminalStreamCursor>,
    #[serde(default)]
    execution_device: Option<ExecutionDeviceBody>,
}

#[derive(Debug, Deserialize)]
struct ScrollbackRequest {
    scrollback_lines: usize,
    #[serde(default)]
    execution_device: Option<ExecutionDeviceBody>,
}

#[derive(Debug, Deserialize)]
struct InputRequest {
    browser_viewer_id: String,
    data: String,
    #[serde(default)]
    execution_device: Option<ExecutionDeviceBody>,
}

#[derive(Debug, Deserialize)]
struct AdaptPtyGeometryRequest {
    browser_viewer_id: String,
    cols: u16,
    rows: u16,
    #[serde(default)]
    execution_device: Option<ExecutionDeviceBody>,
}

#[derive(Debug, Deserialize)]
struct UpdatePaletteRequest {
    browser_viewer_id: String,
    palette: crate::services::terminal::TerminalPalette,
    #[serde(default)]
    execution_device: Option<ExecutionDeviceBody>,
}

#[derive(Debug, Deserialize)]
struct RenameSessionRequest {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    execution_device: Option<ExecutionDeviceBody>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ExecutionDeviceBody {
    Local,
    Remote { device_id: String },
}

async fn list_sessions(
    State(state): State<AppState>,
    Query(query): Query<ListSessionsQuery>,
) -> Response {
    let execution_device = match parse_execution_device_query(&query.execution_device) {
        Ok(device) => device,
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    match terminal_service(&state)
        .list_sessions(execution_device, current_sync_device_id(&state).as_deref())
        .await
    {
        Ok(sessions) => (
            StatusCode::OK,
            Json(serde_json::json!({ "sessions": sessions })),
        )
            .into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn list_profiles(
    State(state): State<AppState>,
    Query(query): Query<ListSessionsQuery>,
) -> Response {
    let execution_device = match parse_execution_device_query(&query.execution_device) {
        Ok(device) => device,
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    match terminal_service(&state)
        .list_profiles(execution_device, current_sync_device_id(&state).as_deref())
        .await
    {
        Ok(profiles) => (StatusCode::OK, Json(profiles)).into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn open_session(
    State(state): State<AppState>,
    Json(body): Json<OpenSessionRequest>,
) -> Response {
    if let Err(error) = validate_scrollback_lines(body.scrollback_lines) {
        return ApiError::BadRequest(error).into_response();
    }
    let execution_device = match body.execution_device {
        ExecutionDeviceBody::Local => TerminalExecutionDevice::Local,
        ExecutionDeviceBody::Remote { device_id } => TerminalExecutionDevice::Remote { device_id },
    };
    match terminal_service(&state)
        .open_session(
            execution_device,
            current_sync_device_id(&state).as_deref(),
            body.browser_viewer_id,
            body.cwd,
            body.terminal_profile_id,
            body.scrollback_lines,
        )
        .await
    {
        Ok(session) => (
            StatusCode::OK,
            Json(serde_json::json!({ "session": session })),
        )
            .into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn kill_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionExecutionQuery>,
) -> Response {
    let execution_device = match query.execution_device.as_deref() {
        Some(value) => match parse_execution_device_query(value) {
            Ok(device) => device,
            Err(error) => return ApiError::BadRequest(error).into_response(),
        },
        None => TerminalExecutionDevice::Local,
    };
    let Some(browser_viewer_id) = query.browser_viewer_id else {
        return ApiError::BadRequest("browser_viewer_id is required".to_string()).into_response();
    };
    match terminal_service(&state)
        .kill_session(
            execution_device,
            current_sync_device_id(&state).as_deref(),
            &session_id,
            browser_viewer_id,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn attach_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<ViewerRequest>,
) -> Response {
    let execution_device = execution_device_from_body(body.execution_device);
    let Some(scrollback_lines) = body.scrollback_lines else {
        return ApiError::BadRequest("scrollback_lines is required".to_string()).into_response();
    };
    if let Err(error) = validate_scrollback_lines(scrollback_lines) {
        return ApiError::BadRequest(error).into_response();
    }
    match terminal_service(&state)
        .attach_session(
            execution_device,
            current_sync_device_id(&state).as_deref(),
            &session_id,
            body.browser_viewer_id,
            scrollback_lines,
            body.cursor,
        )
        .await
    {
        Ok(attachment) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "session": attachment.session,
                "snapshot": attachment.snapshot,
                "tail": replay_json(attachment.tail),
                "live_cursor": attachment.live_cursor,
            })),
        )
            .into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn update_scrollback(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<ScrollbackRequest>,
) -> Response {
    if let Err(error) = validate_scrollback_lines(body.scrollback_lines) {
        return ApiError::BadRequest(error).into_response();
    }
    let execution_device = execution_device_from_body(body.execution_device);
    match terminal_service(&state)
        .set_scrollback_lines(
            execution_device,
            current_sync_device_id(&state).as_deref(),
            &session_id,
            body.scrollback_lines,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn detach_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<ViewerRequest>,
) -> Response {
    let execution_device = execution_device_from_body(body.execution_device);
    match terminal_service(&state)
        .detach_session(
            execution_device,
            current_sync_device_id(&state).as_deref(),
            &session_id,
            body.browser_viewer_id,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn take_input_lease(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<ViewerRequest>,
) -> Response {
    let execution_device = execution_device_from_body(body.execution_device);
    match terminal_service(&state)
        .take_input_lease(
            execution_device,
            current_sync_device_id(&state).as_deref(),
            &session_id,
            body.browser_viewer_id,
        )
        .await
    {
        Ok(session) => (
            StatusCode::OK,
            Json(serde_json::json!({ "session": session })),
        )
            .into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn send_input(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<InputRequest>,
) -> Response {
    let execution_device = execution_device_from_body(body.execution_device);
    match terminal_service(&state)
        .send_input(
            execution_device,
            current_sync_device_id(&state).as_deref(),
            &session_id,
            body.browser_viewer_id,
            body.data.into_bytes(),
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn adapt_pty_geometry(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<AdaptPtyGeometryRequest>,
) -> Response {
    let execution_device = execution_device_from_body(body.execution_device);
    match terminal_service(&state)
        .adapt_pty_geometry(
            execution_device,
            current_sync_device_id(&state).as_deref(),
            &session_id,
            body.browser_viewer_id,
            crate::services::terminal::TerminalPtyGeometry {
                cols: body.cols,
                rows: body.rows,
            },
        )
        .await
    {
        Ok(session) => (
            StatusCode::OK,
            Json(serde_json::json!({ "session": session })),
        )
            .into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn update_palette(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<UpdatePaletteRequest>,
) -> Response {
    let execution_device = execution_device_from_body(body.execution_device);
    match terminal_service(&state)
        .update_palette(
            execution_device,
            current_sync_device_id(&state).as_deref(),
            &session_id,
            body.browser_viewer_id,
            body.palette,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn rename_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<RenameSessionRequest>,
) -> Response {
    let execution_device = execution_device_from_body(body.execution_device);
    match terminal_service(&state)
        .rename_session(
            execution_device,
            current_sync_device_id(&state).as_deref(),
            &session_id,
            body.display_name,
        )
        .await
    {
        Ok(session) => (
            StatusCode::OK,
            Json(serde_json::json!({ "session": session })),
        )
            .into_response(),
        Err(error) => placement_error_response(error),
    }
}

async fn activity_stream(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<SessionExecutionQuery>,
) -> Response {
    let execution_device = match query.execution_device.as_deref() {
        Some(value) => match parse_execution_device_query(value) {
            Ok(device) => device,
            Err(error) => return ApiError::BadRequest(error).into_response(),
        },
        None => TerminalExecutionDevice::Local,
    };
    let Some(browser_viewer_id) = query.browser_viewer_id.as_deref() else {
        return ApiError::BadRequest("browser_viewer_id is required".to_string()).into_response();
    };
    let rx = match terminal_service(&state)
        .subscribe_activity(
            execution_device,
            current_sync_device_id(&state).as_deref(),
            &session_id,
            browser_viewer_id,
            query.retain_authority,
            TerminalStreamCursor {
                next_activity_sequence: query.next_activity_sequence,
                next_output_offset: query.next_output_offset,
            },
        )
        .await
    {
        Ok(rx) => rx,
        Err(error) => return placement_error_response(error),
    };
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|frame| {
            let payload = terminal_activity_frame_json(&frame);
            (sse_json_event("terminal_activity", &payload), rx)
        })
    });
    sse_response(stream).into_response()
}

fn default_true() -> bool {
    true
}

fn replay_json(frames: Vec<TerminalActivityFrame>) -> Vec<serde_json::Value> {
    frames
        .into_iter()
        .map(|frame| terminal_activity_frame_json(&frame))
        .collect()
}

fn parse_execution_device_query(value: &str) -> Result<TerminalExecutionDevice, String> {
    match value {
        "local" => Ok(TerminalExecutionDevice::Local),
        other if !other.trim().is_empty() => Ok(TerminalExecutionDevice::Remote {
            device_id: other.to_string(),
        }),
        _ => Err("execution_device is required".to_string()),
    }
}

fn validate_scrollback_lines(value: usize) -> Result<(), String> {
    if (TERMINAL_SCROLLBACK_MIN_LINES..=TERMINAL_SCROLLBACK_MAX_LINES).contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "scrollback_lines must be between {TERMINAL_SCROLLBACK_MIN_LINES} and {TERMINAL_SCROLLBACK_MAX_LINES}"
        ))
    }
}

fn execution_device_from_body(value: Option<ExecutionDeviceBody>) -> TerminalExecutionDevice {
    match value {
        Some(ExecutionDeviceBody::Local) | None => TerminalExecutionDevice::Local,
        Some(ExecutionDeviceBody::Remote { device_id }) => {
            TerminalExecutionDevice::Remote { device_id }
        }
    }
}

fn current_sync_device_id(state: &AppState) -> Option<String> {
    state.sync_device.read().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|device| hex::encode(device.device_id_short()))
    })
}

fn terminal_service(state: &AppState) -> TerminalPlacementService {
    TerminalPlacementService::new(
        state.terminal_authority.clone(),
        state.terminal_remote_client.clone(),
        state.terminal_remote_activity_client.clone(),
    )
}

fn placement_error_response(error: TerminalPlacementError) -> Response {
    match error {
        TerminalPlacementError::MissingSourceDevice { .. }
        | TerminalPlacementError::Unavailable(_) => ApiError::Unavailable(error.to_string()),
        TerminalPlacementError::NotFound(_) => ApiError::NotFound(error.to_string()),
        TerminalPlacementError::CursorUnavailable(_) => ApiError::Conflict(error.to_string()),
        TerminalPlacementError::InputLeaseRequired => {
            ApiError::Forbidden("terminal input lease is required".to_string())
        }
        TerminalPlacementError::Internal(_) => ApiError::Internal(error.to_string()),
    }
    .into_response()
}
