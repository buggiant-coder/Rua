use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};

use rua_workspace::{FileContentRecord, FileRecord};

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::middleware::AgentExtractorContext;
use crate::services::agent_prompt_files::agent_prompt_files as build_manager;
use crate::state::AppState;

pub fn root_router() -> Router<AppState> {
    Router::new()
        .route("/agent/files", get(list_files))
        .route(
            "/agent/files/{fileName}",
            get(get_file).put(write_file).delete(delete_file),
        )
        .route("/agent/files/{fileName}/rename", post(rename_file))
        .route("/agent/subdirs-info", get(list_subdirs_info))
        .route(
            "/agent/subdirs/{subdir}",
            get(list_subdir_files).post(create_subdir_file),
        )
        .route(
            "/agent/subdirs/{subdir}/{md_name}",
            get(get_subdir_file)
                .put(write_subdir_file)
                .delete(delete_subdir_file),
        )
        .route(
            "/agent/subdirs/{subdir}/{md_name}/rename",
            post(rename_subdir_file),
        )
        .route(
            "/agent/system-prompt-files",
            get(list_system_prompt_files).put(update_system_prompt_files),
        )
}

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agentId}/files", get(list_files_agent))
        .route(
            "/agents/{agentId}/files/{fileName}",
            get(get_file_agent)
                .put(write_file_agent)
                .delete(delete_file_agent),
        )
        .route(
            "/agents/{agentId}/files/{fileName}/rename",
            post(rename_file_agent),
        )
        .route(
            "/agents/{agentId}/subdirs-info",
            get(list_subdirs_info_agent),
        )
        .route(
            "/agents/{agentId}/subdirs/{subdir}",
            get(list_subdir_files_agent).post(create_subdir_file_agent),
        )
        .route(
            "/agents/{agentId}/subdirs/{subdir}/{md_name}",
            get(get_subdir_file_agent)
                .put(write_subdir_file_agent)
                .delete(delete_subdir_file_agent),
        )
        .route(
            "/agents/{agentId}/subdirs/{subdir}/{md_name}/rename",
            post(rename_subdir_file_agent),
        )
        .route(
            "/agents/{agentId}/system-prompt-files",
            get(list_system_prompt_files_agent).put(update_system_prompt_files_agent),
        )
}

fn record_to_file_entry(r: FileRecord) -> dto::FileEntry {
    dto::FileEntry {
        name: r.name,
        path: r.path,
        size: r.size,
        modified: r.modified,
        created_time: r.created_time,
        builtin: r.builtin,
        extra: serde_json::Map::new(),
    }
}

fn record_to_file_content(r: FileContentRecord) -> dto::FileContent {
    dto::FileContent {
        name: r.name,
        path: r.path,
        content: r.content,
        size: r.size,
        extra: serde_json::Map::new(),
    }
}

async fn list_files(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    list_files_impl(&state, &ctx.agent_id).await
}

async fn get_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(file_name): Path<String>,
) -> Response {
    get_file_impl(&state, &ctx.agent_id, &file_name).await
}

async fn write_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(file_name): Path<String>,
    Json(body): Json<dto::FileWriteRequest>,
) -> Response {
    write_file_impl(&state, &ctx.agent_id, &file_name, body).await
}

async fn delete_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(file_name): Path<String>,
) -> Response {
    delete_file_impl(&state, &ctx.agent_id, &file_name).await
}

async fn rename_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(file_name): Path<String>,
    Json(body): Json<dto::FileRenameRequest>,
) -> Response {
    rename_file_impl(&state, &ctx.agent_id, &file_name, &body.new_name).await
}

async fn list_subdirs_info(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    list_subdirs_info_impl(&state, &ctx.agent_id).await
}

async fn list_subdir_files(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(subdir): Path<String>,
) -> Response {
    list_subdir_files_impl(&state, &ctx.agent_id, &subdir).await
}

async fn create_subdir_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(subdir): Path<String>,
    Json(body): Json<dto::SubdirFileCreateRequest>,
) -> Response {
    create_subdir_file_impl(&state, &ctx.agent_id, &subdir, body).await
}

async fn get_subdir_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path((subdir, md_name)): Path<(String, String)>,
) -> Response {
    get_subdir_file_impl(&state, &ctx.agent_id, &subdir, &md_name).await
}

async fn write_subdir_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path((subdir, md_name)): Path<(String, String)>,
    Json(body): Json<dto::FileWriteRequest>,
) -> Response {
    write_subdir_file_impl(&state, &ctx.agent_id, &subdir, &md_name, body.content).await
}

async fn rename_subdir_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path((subdir, md_name)): Path<(String, String)>,
    Json(body): Json<dto::FileRenameRequest>,
) -> Response {
    rename_subdir_file_impl(&state, &ctx.agent_id, &subdir, &md_name, &body.new_name).await
}

async fn delete_subdir_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path((subdir, md_name)): Path<(String, String)>,
) -> Response {
    delete_subdir_file_impl(&state, &ctx.agent_id, &subdir, &md_name).await
}

async fn list_system_prompt_files(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    list_system_prompt_files_impl(&state, &ctx.agent_id).await
}

async fn update_system_prompt_files(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(files): Json<Vec<String>>,
) -> Response {
    update_system_prompt_files_impl(&state, &ctx.agent_id, files).await
}

async fn list_files_agent(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    list_files_impl(&state, &agent_id).await
}

async fn get_file_agent(
    State(state): State<AppState>,
    Path((agent_id, file_name)): Path<(String, String)>,
) -> Response {
    get_file_impl(&state, &agent_id, &file_name).await
}

async fn write_file_agent(
    State(state): State<AppState>,
    Path((agent_id, file_name)): Path<(String, String)>,
    Json(body): Json<dto::FileWriteRequest>,
) -> Response {
    write_file_impl(&state, &agent_id, &file_name, body).await
}

async fn delete_file_agent(
    State(state): State<AppState>,
    Path((agent_id, file_name)): Path<(String, String)>,
) -> Response {
    delete_file_impl(&state, &agent_id, &file_name).await
}

async fn rename_file_agent(
    State(state): State<AppState>,
    Path((agent_id, file_name)): Path<(String, String)>,
    Json(body): Json<dto::FileRenameRequest>,
) -> Response {
    rename_file_impl(&state, &agent_id, &file_name, &body.new_name).await
}

async fn list_subdirs_info_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    list_subdirs_info_impl(&state, &agent_id).await
}

async fn list_subdir_files_agent(
    State(state): State<AppState>,
    Path((agent_id, subdir)): Path<(String, String)>,
) -> Response {
    list_subdir_files_impl(&state, &agent_id, &subdir).await
}

async fn create_subdir_file_agent(
    State(state): State<AppState>,
    Path((agent_id, subdir)): Path<(String, String)>,
    Json(body): Json<dto::SubdirFileCreateRequest>,
) -> Response {
    create_subdir_file_impl(&state, &agent_id, &subdir, body).await
}

async fn get_subdir_file_agent(
    State(state): State<AppState>,
    Path((agent_id, subdir, md_name)): Path<(String, String, String)>,
) -> Response {
    get_subdir_file_impl(&state, &agent_id, &subdir, &md_name).await
}

async fn write_subdir_file_agent(
    State(state): State<AppState>,
    Path((agent_id, subdir, md_name)): Path<(String, String, String)>,
    Json(body): Json<dto::FileWriteRequest>,
) -> Response {
    write_subdir_file_impl(&state, &agent_id, &subdir, &md_name, body.content).await
}

async fn rename_subdir_file_agent(
    State(state): State<AppState>,
    Path((agent_id, subdir, md_name)): Path<(String, String, String)>,
    Json(body): Json<dto::FileRenameRequest>,
) -> Response {
    rename_subdir_file_impl(&state, &agent_id, &subdir, &md_name, &body.new_name).await
}

async fn delete_subdir_file_agent(
    State(state): State<AppState>,
    Path((agent_id, subdir, md_name)): Path<(String, String, String)>,
) -> Response {
    delete_subdir_file_impl(&state, &agent_id, &subdir, &md_name).await
}

async fn list_system_prompt_files_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    list_system_prompt_files_impl(&state, &agent_id).await
}

async fn update_system_prompt_files_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(files): Json<Vec<String>>,
) -> Response {
    update_system_prompt_files_impl(&state, &agent_id, files).await
}

async fn list_files_impl(state: &AppState, agent_id: &str) -> Response {
    match build_manager(state, agent_id).list_files().await {
        Ok(files) => {
            let dtos: Vec<dto::FileEntry> = files.into_iter().map(record_to_file_entry).collect();
            (StatusCode::OK, Json(dtos)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_file_impl(state: &AppState, agent_id: &str, file_name: &str) -> Response {
    match build_manager(state, agent_id).get_file(file_name).await {
        Ok(Some(file)) => (StatusCode::OK, Json(record_to_file_content(file))).into_response(),
        Ok(None) => ApiError::NotFound(format!("File '{file_name}' not found")).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn write_file_impl(
    state: &AppState,
    agent_id: &str,
    file_name: &str,
    body: dto::FileWriteRequest,
) -> Response {
    let _ = body.path;
    match build_manager(state, agent_id)
        .write_file(file_name, body.content)
        .await
    {
        Ok(file) => (StatusCode::OK, Json(record_to_file_content(file))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn delete_file_impl(state: &AppState, agent_id: &str, file_name: &str) -> Response {
    match build_manager(state, agent_id).delete_file(file_name).await {
        Ok(()) => (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn rename_file_impl(
    state: &AppState,
    agent_id: &str,
    file_name: &str,
    new_name: &str,
) -> Response {
    match build_manager(state, agent_id)
        .rename_file(file_name, new_name)
        .await
    {
        Ok(file) => (StatusCode::OK, Json(record_to_file_content(file))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn list_subdirs_info_impl(state: &AppState, agent_id: &str) -> Response {
    match build_manager(state, agent_id).list_subdirs_info().await {
        Ok(info) => (StatusCode::OK, Json(info)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn list_subdir_files_impl(state: &AppState, agent_id: &str, subdir: &str) -> Response {
    match build_manager(state, agent_id)
        .list_subdir_files(subdir)
        .await
    {
        Ok(files) => {
            let dtos: Vec<dto::FileEntry> = files.into_iter().map(record_to_file_entry).collect();
            (StatusCode::OK, Json(dtos)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn create_subdir_file_impl(
    state: &AppState,
    agent_id: &str,
    subdir: &str,
    body: dto::SubdirFileCreateRequest,
) -> Response {
    match build_manager(state, agent_id)
        .create_subdir_file(subdir, body.filename, body.content)
        .await
    {
        Ok(file) => (StatusCode::CREATED, Json(record_to_file_content(file))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_subdir_file_impl(
    state: &AppState,
    agent_id: &str,
    subdir: &str,
    md_name: &str,
) -> Response {
    match build_manager(state, agent_id)
        .get_subdir_file(subdir, md_name)
        .await
    {
        Ok(Some(file)) => (StatusCode::OK, Json(record_to_file_content(file))).into_response(),
        Ok(None) => {
            ApiError::NotFound(format!("File '{md_name}' not found in '{subdir}'")).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn write_subdir_file_impl(
    state: &AppState,
    agent_id: &str,
    subdir: &str,
    md_name: &str,
    content: String,
) -> Response {
    match build_manager(state, agent_id)
        .write_subdir_file(subdir, md_name, content)
        .await
    {
        Ok(file) => (StatusCode::OK, Json(record_to_file_content(file))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn rename_subdir_file_impl(
    state: &AppState,
    agent_id: &str,
    subdir: &str,
    md_name: &str,
    new_name: &str,
) -> Response {
    match build_manager(state, agent_id)
        .rename_subdir_file(subdir, md_name, new_name)
        .await
    {
        Ok(file) => (StatusCode::OK, Json(record_to_file_content(file))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn delete_subdir_file_impl(
    state: &AppState,
    agent_id: &str,
    subdir: &str,
    md_name: &str,
) -> Response {
    match build_manager(state, agent_id)
        .delete_subdir_file(subdir, md_name)
        .await
    {
        Ok(()) => (StatusCode::OK, Json(dto::SuccessResponse::ok())).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn list_system_prompt_files_impl(state: &AppState, agent_id: &str) -> Response {
    match build_manager(state, agent_id)
        .list_system_prompt_files()
        .await
    {
        Ok(files) => (StatusCode::OK, Json(files)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn update_system_prompt_files_impl(
    state: &AppState,
    agent_id: &str,
    files: Vec<String>,
) -> Response {
    match build_manager(state, agent_id)
        .update_system_prompt_files(files)
        .await
    {
        Ok(files) => {
            let path = state
                .rua_root
                .join("workspaces")
                .join(agent_id)
                .join("agent.json");
            crate::sync_publication::publish_agent_config_file_committed(
                state,
                agent_id,
                &path,
                "system prompt files agent config",
            )
            .await;
            (StatusCode::OK, Json(files)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}
