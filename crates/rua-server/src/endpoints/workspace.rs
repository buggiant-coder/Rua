use std::path::PathBuf;

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};

use rua_agent::profile_store::AgentProfileStore;
use rua_workspace::{WorkspaceImportExport, WorkspaceImportSummary, WorkspaceInfoRecord};

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::extractors::AgentExtractorContext;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/workspace/download", get(download))
        .route("/workspace/upload", post(upload))
        .route("/agents/{agentId}/workspace", get(agent_workspace_info))
        .route("/agents/{agentId}/workspace/download", get(agent_download))
        .route("/agents/{agentId}/workspace/upload", post(agent_upload))
}

fn workspace_dir_for(state: &AppState, agent_id: &str) -> PathBuf {
    state.rua_root.join("workspaces").join(agent_id)
}

fn driver_for(state: &AppState, agent_id: Option<&str>) -> WorkspaceImportExport {
    let id = AgentProfileStore::resolve_agent_id(agent_id);
    WorkspaceImportExport::new(workspace_dir_for(state, id), id.to_string())
}

fn import_summary_to_dto(s: WorkspaceImportSummary) -> dto::WorkspaceImportResult {
    dto::WorkspaceImportResult {
        success: s.success,
        agents_imported: s.agents_imported,
        message: s.message,
        extra: serde_json::Map::new(),
    }
}

fn workspace_info_to_dto(info: WorkspaceInfoRecord) -> dto::WorkspaceInfo {
    dto::WorkspaceInfo {
        workspace_dir: info.workspace_dir,
        total_size: Some(info.total_size),
        file_count: Some(info.file_count),
        extra: serde_json::Map::new(),
    }
}

fn download_response(data: Vec<u8>, agent_id: &str) -> Response {
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let filename = format!("workspace_{agent_id}_{timestamp}.zip");
    let headers = [
        (
            axum::http::header::CONTENT_TYPE,
            "application/octet-stream".to_string(),
        ),
        (
            axum::http::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        ),
    ];
    (StatusCode::OK, headers, data).into_response()
}

async fn download(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    match driver_for(&state, Some(&ctx.agent_id)).export() {
        Ok(data) => download_response(data, &ctx.agent_id),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn upload(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    mut multipart: Multipart,
) -> Response {
    let bytes = match extract_file_field(&mut multipart).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    match driver_for(&state, Some(&ctx.agent_id)).import(&bytes) {
        Ok(result) => (StatusCode::OK, Json(import_summary_to_dto(result))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_workspace_info(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    match driver_for(&state, Some(&agent_id)).workspace_info() {
        Some(info) => (StatusCode::OK, Json(workspace_info_to_dto(info))).into_response(),
        None => {
            ApiError::NotFound(format!("Agent '{agent_id}' workspace not found")).into_response()
        }
    }
}

async fn agent_download(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    match driver_for(&state, Some(&agent_id)).export() {
        Ok(data) => download_response(data, &agent_id),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn agent_upload(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    mut multipart: Multipart,
) -> Response {
    let bytes = match extract_file_field(&mut multipart).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    match driver_for(&state, Some(&agent_id)).import(&bytes) {
        Ok(result) => (StatusCode::OK, Json(import_summary_to_dto(result))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn extract_file_field(multipart: &mut Multipart) -> Result<bytes::Bytes, Response> {
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            return field.bytes().await.map_err(|e| {
                ApiError::BadRequest(format!("Failed to read file field: {e}")).into_response()
            });
        }
    }
    Err(ApiError::BadRequest("Missing 'file' field in multipart form".to_string()).into_response())
}
