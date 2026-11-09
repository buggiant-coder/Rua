use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use rua_workspace::WorkspaceRuntime;
use serde::{Deserialize, Serialize};

use crate::errors::ApiError;
use crate::extractors::AgentExtractorContext;
use crate::state::AppState;
use crate::sync_publication::{publish_sync_fact, SyncPublicationFact};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptStateDto {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub files: Vec<PromptFileDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptFileDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default, rename = "type")]
    pub file_type: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct AddFileBody {
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default, rename = "type")]
    pub file_type: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ReorderBody {
    pub file_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ChatQuery {
    chat_id: String,
}

#[derive(Debug, Deserialize)]
struct RemoveFileBody {
    file_id: String,
}

pub fn root_router() -> Router<AppState> {
    Router::new()
        .route(
            "/chats/prompt-state",
            get(get_prompt_state_root).put(put_prompt_state_root),
        )
        .route("/chats/prompt-files/add", post(add_file_root))
        .route("/chats/prompt-files/remove", post(remove_file_root))
        .route("/chats/prompt-files/reset", post(reset_files_root))
        .route("/chats/prompt-files/reorder", post(reorder_files_root))
}

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{agentId}/chats/{id}/prompt-state",
            get(get_prompt_state_agent).put(put_prompt_state_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/prompt-files",
            post(add_file_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/prompt-files/{fileId}",
            axum::routing::delete(remove_file_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/prompt-files/reset",
            post(reset_files_agent),
        )
        .route(
            "/agents/{agentId}/chats/{id}/prompt-files/reorder",
            put(reorder_files_agent),
        )
}

async fn get_prompt_state_root(
    AgentExtractorContext { runtime, .. }: AgentExtractorContext,
    Query(params): Query<ChatQuery>,
) -> Response {
    get_prompt_state_core(&runtime.workspace_dir(), &params.chat_id).await
}

async fn put_prompt_state_root(
    State(state): State<AppState>,
    AgentExtractorContext {
        runtime, agent_id, ..
    }: AgentExtractorContext,
    Query(params): Query<ChatQuery>,
    Json(body): Json<PromptStateDto>,
) -> Response {
    put_prompt_state_core(
        &state,
        &runtime.workspace_dir(),
        &agent_id,
        &params.chat_id,
        body,
    )
    .await
}

async fn add_file_root(
    State(state): State<AppState>,
    AgentExtractorContext {
        runtime, agent_id, ..
    }: AgentExtractorContext,
    Query(params): Query<ChatQuery>,
    Json(body): Json<AddFileBody>,
) -> Response {
    add_file_core(
        &state,
        &runtime.workspace_dir(),
        &agent_id,
        &params.chat_id,
        body,
    )
    .await
}

async fn remove_file_root(
    State(state): State<AppState>,
    AgentExtractorContext {
        runtime, agent_id, ..
    }: AgentExtractorContext,
    Query(params): Query<ChatQuery>,
    Json(body): Json<RemoveFileBody>,
) -> Response {
    remove_file_core(
        &state,
        &runtime.workspace_dir(),
        &agent_id,
        &params.chat_id,
        &body.file_id,
    )
    .await
}

async fn reset_files_root(
    State(state): State<AppState>,
    AgentExtractorContext {
        runtime, agent_id, ..
    }: AgentExtractorContext,
    Query(params): Query<ChatQuery>,
) -> Response {
    reset_files_core(&state, &runtime.workspace_dir(), &agent_id, &params.chat_id).await
}

async fn reorder_files_root(
    State(state): State<AppState>,
    AgentExtractorContext {
        runtime, agent_id, ..
    }: AgentExtractorContext,
    Query(params): Query<ChatQuery>,
    Json(body): Json<ReorderBody>,
) -> Response {
    reorder_files_core(
        &state,
        &runtime.workspace_dir(),
        &agent_id,
        &params.chat_id,
        body,
    )
    .await
}

async fn get_prompt_state_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    get_prompt_state_core(&runtime.workspace_dir(), &chat_id).await
}

async fn put_prompt_state_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(body): Json<PromptStateDto>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    put_prompt_state_core(&state, &runtime.workspace_dir(), &agent_id, &chat_id, body).await
}

async fn add_file_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(body): Json<AddFileBody>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    add_file_core(&state, &runtime.workspace_dir(), &agent_id, &chat_id, body).await
}

async fn remove_file_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id, file_id)): AxumPath<(String, String, String)>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    remove_file_core(
        &state,
        &runtime.workspace_dir(),
        &agent_id,
        &chat_id,
        &file_id,
    )
    .await
}

async fn reset_files_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    reset_files_core(&state, &runtime.workspace_dir(), &agent_id, &chat_id).await
}

async fn reorder_files_agent(
    State(state): State<AppState>,
    AxumPath((agent_id, chat_id)): AxumPath<(String, String)>,
    Json(body): Json<ReorderBody>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    reorder_files_core(&state, &runtime.workspace_dir(), &agent_id, &chat_id, body).await
}

async fn runtime_for_agent(
    state: &AppState,
    agent_id: &str,
) -> Result<Arc<WorkspaceRuntime>, ApiError> {
    state
        .registry
        .get_or_create(agent_id)
        .await
        .map_err(|e| ApiError::Unavailable(format!("workspace for '{agent_id}' unavailable: {e}")))
}

fn prompt_state_dir(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("prompt-state")
}

fn prompt_state_path(workspace_dir: &Path, chat_id: &str) -> PathBuf {
    prompt_state_dir(workspace_dir).join(format!("{chat_id}.json"))
}

async fn read_prompt_state(
    workspace_dir: &Path,
    chat_id: &str,
) -> Result<PromptStateDto, ApiError> {
    let path = prompt_state_path(workspace_dir, chat_id);
    if !path.exists() {
        return Ok(PromptStateDto::default());
    }
    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to read prompt state: {e}")))?;
    serde_json::from_str(&content)
        .map_err(|e| ApiError::Internal(format!("Failed to parse prompt state: {e}")))
}

async fn write_prompt_state(
    workspace_dir: &Path,
    chat_id: &str,
    state: &PromptStateDto,
) -> Result<(), ApiError> {
    let dir = prompt_state_dir(workspace_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to create prompt-state dir: {e}")))?;
    let path = prompt_state_path(workspace_dir, chat_id);
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| ApiError::Internal(format!("Failed to serialize prompt state: {e}")))?;
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to write prompt state: {e}")))
}

async fn get_prompt_state_core(ws_dir: &Path, chat_id: &str) -> Response {
    match read_prompt_state(ws_dir, chat_id).await {
        Ok(ps) => (StatusCode::OK, Json(ps)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn publish_prompt_state_sync_fact(
    state: &AppState,
    ws_dir: &Path,
    agent_id: &str,
    chat_id: &str,
) {
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::StableConversationState {
            scope_root: ws_dir.to_path_buf(),
            address: rua_core::ChatAddress::agent(agent_id, chat_id),
        },
    )
    .await;
}

async fn put_prompt_state_core(
    state: &AppState,
    ws_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    body: PromptStateDto,
) -> Response {
    if let Err(e) = write_prompt_state(ws_dir, chat_id, &body).await {
        return e.into_response();
    }
    publish_prompt_state_sync_fact(state, ws_dir, agent_id, chat_id).await;

    (StatusCode::OK, Json(body)).into_response()
}

async fn add_file_core(
    state: &AppState,
    ws_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    body: AddFileBody,
) -> Response {
    let mut ps = match read_prompt_state(ws_dir, chat_id).await {
        Ok(ps) => ps,
        Err(e) => return e.into_response(),
    };

    let file = PromptFileDto {
        id: uuid::Uuid::new_v4().to_string(),
        name: body.name,
        path: body.path,
        file_type: body.file_type,
        size: body.size,
    };

    ps.files.push(file.clone());

    if let Err(e) = write_prompt_state(ws_dir, chat_id, &ps).await {
        return e.into_response();
    }
    publish_prompt_state_sync_fact(state, ws_dir, agent_id, chat_id).await;

    (StatusCode::OK, Json(file)).into_response()
}

async fn remove_file_core(
    state: &AppState,
    ws_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    file_id: &str,
) -> Response {
    let mut ps = match read_prompt_state(ws_dir, chat_id).await {
        Ok(ps) => ps,
        Err(e) => return e.into_response(),
    };

    let before = ps.files.len();
    ps.files.retain(|f| f.id != file_id);

    if ps.files.len() == before {
        return ApiError::NotFound(format!("File '{file_id}' not found")).into_response();
    }

    if let Err(e) = write_prompt_state(ws_dir, chat_id, &ps).await {
        return e.into_response();
    }
    publish_prompt_state_sync_fact(state, ws_dir, agent_id, chat_id).await;

    (StatusCode::OK, Json(serde_json::json!({"deleted": true}))).into_response()
}

async fn reset_files_core(
    state: &AppState,
    ws_dir: &Path,
    agent_id: &str,
    chat_id: &str,
) -> Response {
    let mut ps = match read_prompt_state(ws_dir, chat_id).await {
        Ok(ps) => ps,
        Err(e) => return e.into_response(),
    };

    ps.files.clear();

    if let Err(e) = write_prompt_state(ws_dir, chat_id, &ps).await {
        return e.into_response();
    }
    publish_prompt_state_sync_fact(state, ws_dir, agent_id, chat_id).await;

    (StatusCode::OK, Json(ps)).into_response()
}

async fn reorder_files_core(
    state: &AppState,
    ws_dir: &Path,
    agent_id: &str,
    chat_id: &str,
    body: ReorderBody,
) -> Response {
    let mut ps = match read_prompt_state(ws_dir, chat_id).await {
        Ok(ps) => ps,
        Err(e) => return e.into_response(),
    };

    let mut reordered = Vec::with_capacity(body.file_ids.len());
    for fid in &body.file_ids {
        if let Some(f) = ps.files.iter().find(|f| &f.id == fid) {
            reordered.push(f.clone());
        }
    }

    for f in &ps.files {
        if !body.file_ids.contains(&f.id) {
            reordered.push(f.clone());
        }
    }
    ps.files = reordered;

    if let Err(e) = write_prompt_state(ws_dir, chat_id, &ps).await {
        return e.into_response();
    }
    publish_prompt_state_sync_fact(state, ws_dir, agent_id, chat_id).await;

    (StatusCode::OK, Json(ps)).into_response()
}
