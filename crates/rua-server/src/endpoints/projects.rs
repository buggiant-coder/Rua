use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use rua_core::{ChatAddress, ProjectId};
use rua_project::{ProjectDescriptor, ProjectLifecycle};
use serde::{Deserialize, Serialize};

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::services::device_command_bus::{
    ConversationControlDeviceAction, ConversationControlDeviceResponse, DeviceCommandDeliveryError,
    ProjectDeviceResponse, ProjectResourceOperation,
};
use crate::services::project_control::{
    resolve_project_control_target, ProjectControlError, ProjectControlTarget,
};
use crate::services::projects::{
    ProjectCreateResolution, ProjectPlacementError, ProjectPlacementService,
};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects", get(list_projects).post(create_project))
        .route(
            "/project-sidebar",
            get(get_project_sidebar).patch(update_project_sidebar),
        )
        .route("/projects/{projectId}/resources", patch(edit_resources))
        .route(
            "/projects/{projectId}/resources/primary",
            patch(set_primary_directory),
        )
        .route(
            "/projects/{projectId}/workspace/files",
            get(get_project_files),
        )
        .route("/projects/directory", post(browse_directory))
        .route("/projects/directory/create", post(create_directory))
        .route(
            "/projects/{projectId}/workspace/git",
            get(get_project_git_status),
        )
        .route(
            "/projects/{projectId}/workspace/git/history",
            get(get_project_git_history),
        )
        .route(
            "/projects/{projectId}/lifecycle-preview",
            get(get_project_lifecycle_preview),
        )
        .route("/projects/{projectId}/archive", post(archive_project))
        .route("/projects/{projectId}/reopen", post(reopen_project))
        .route(
            "/projects/{projectId}/permanent-delete",
            post(permanently_delete_project),
        )
        .route(
            "/projects/{projectId}",
            get(get_project).patch(rename_project),
        )
        .route("/projects/{projectId}/chats", get(list_project_chats))
        .route(
            "/projects/{projectId}/chats/archive-idle",
            post(archive_idle_project_chats),
        )
        .route(
            "/projects/{projectId}/chats/delete-archived",
            post(delete_archived_project_chats),
        )
        .route(
            "/projects/{projectId}/chats/search",
            get(search_project_chats),
        )
        .route("/chats/{chatId}", patch(update_project_chat))
        .route(
            "/chats/{chatId}/resource-policy",
            get(get_project_resource_policy),
        )
        .route(
            "/chats/{chatId}/agent-selection",
            patch(switch_project_chat_agent),
        )
}

async fn get_project_sidebar(State(state): State<AppState>) -> Response {
    match rua_project::ProjectSidebarStore::new(state.rua_root.clone()).snapshot() {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(error) => ApiError::Internal(error.to_string()).into_response(),
    }
}
#[derive(Deserialize)]
struct ProjectSidebarUpdateRequest {
    expected_revision: String,
    #[serde(flatten)]
    action: rua_project::ProjectSidebarAction,
}
async fn update_project_sidebar(
    State(state): State<AppState>,
    Json(body): Json<ProjectSidebarUpdateRequest>,
) -> Response {
    let projects = match state.project_catalog.list() {
        Ok(projects) => projects
            .into_iter()
            .map(|project| project.project_id.to_string())
            .collect::<Vec<_>>(),
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    match rua_project::ProjectSidebarStore::new(state.rua_root.clone()).update(
        &body.expected_revision,
        body.action,
        &projects,
    ) {
        Ok((snapshot, records)) => {
            if !records.is_empty() {
                crate::sync_publication::publish_sync_fact(
                    &state,
                    crate::sync_publication::SyncPublicationFact::ProjectSidebarRecordsCommitted {
                        records,
                    },
                )
                .await;
            }
            Json(snapshot).into_response()
        }
        Err(error) => ApiError::Conflict(error.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct ProjectListQuery {
    #[serde(default)]
    lifecycle: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProjectListResponse {
    projects: Vec<ProjectListEntry>,
}

#[derive(Debug, Serialize)]
struct ProjectListEntry {
    #[serde(flatten)]
    project: ProjectDescriptor,
    last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
struct BrowseDirectoryRequest {
    device_id: String,
    path: String,
}

#[derive(Debug, Deserialize)]
struct CreateDirectoryRequest {
    device_id: String,
    parent: String,
    name: String,
}

#[derive(Debug, Serialize)]
struct BrowseDirectoryResponse {
    device_id: String,
    path: std::path::PathBuf,
    roots: Vec<rua_project::ProjectDirectoryEntry>,
    parent_path: Option<std::path::PathBuf>,
    entries: Vec<rua_project::ProjectDirectoryEntry>,
    proposed_name: String,
}

#[derive(Debug, Deserialize)]
struct CreateProjectRequest {
    resource_device_id: String,
    #[serde(default)]
    resource_root: Option<String>,
    #[serde(default)]
    resource_roots: Option<Vec<String>>,
    #[serde(default)]
    primary_index: usize,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RenameProjectRequest {
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct ProjectLifecycleConfirmRequest {
    confirmed: bool,
}

#[derive(Debug, Deserialize)]
struct ProjectChatAgentSwitchRequest {
    expected_agent_id: String,
    agent_id: String,
}

#[derive(Debug, Deserialize)]
struct ProjectChatListQuery {
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    tag: Option<rua_chat::ChatTag>,
    #[serde(default)]
    sort_by: rua_chat::ChatSortField,
    #[serde(default)]
    sort_order: rua_chat::SortOrder,
    #[serde(default = "super::chats::default_conversation_view")]
    view: rua_session::chat_index::ChatConversationView,
    #[serde(default)]
    source_chat_id: Option<String>,
    #[serde(default)]
    executor: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    archived: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ProjectChatSearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_project_chat_search_limit")]
    limit: usize,
}

const fn default_project_chat_search_limit() -> usize {
    50
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProjectChatDeleteQuery {
    #[serde(default)]
    force: bool,
}

async fn list_projects(
    State(state): State<AppState>,
    Query(query): Query<ProjectListQuery>,
) -> Response {
    let result = match query.lifecycle.as_deref().unwrap_or("active") {
        "active" => state.project_catalog.list_active(),
        "archived" => state.project_catalog.list_archived(),
        "all" => state.project_catalog.list(),
        other => {
            return ApiError::BadRequest(format!("unsupported Project lifecycle filter: {other}"))
                .into_response();
        }
    };
    let projects = match result {
        Ok(projects) => projects,
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    let workspace = state.chat_workspace();
    let has_index = workspace.as_path().join("chat-index.sqlite").is_file();
    let mut entries = Vec::with_capacity(projects.len());
    for project in projects {
        let filter = rua_session::chat_index::ChatIndexFilter {
            project_id: Some(project.project_id.to_string()),
            ..Default::default()
        };
        let last_activity_at = if has_index {
            match crate::services::chat_index::list(workspace.as_path(), filter, 0, 1).await {
                Ok(page) => page.entries.first().map(|entry| entry.updated_at),
                Err(error) => return ApiError::Internal(error).into_response(),
            }
        } else {
            None
        };
        entries.push(ProjectListEntry {
            project,
            last_activity_at,
        });
    }
    Json(ProjectListResponse { projects: entries }).into_response()
}

async fn get_project(State(state): State<AppState>, Path(project_id): Path<String>) -> Response {
    match state.project_catalog.get(&ProjectId::new(&project_id)) {
        Ok(Some(project)) => Json(project).into_response(),
        Ok(None) => ApiError::NotFound(format!("Project not found: {project_id}")).into_response(),
        Err(error) => ApiError::Internal(error.to_string()).into_response(),
    }
}

async fn get_project_lifecycle_preview(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Response {
    let project_id = ProjectId::new(project_id);
    let project = match state.project_catalog.get(&project_id) {
        Ok(Some(project)) => project,
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service.lifecycle_preview(&project, &state).await {
        Ok(preview) => Json(preview).into_response(),
        Err(error) => placement_error(error),
    }
}

async fn archive_project(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<ProjectLifecycleConfirmRequest>,
) -> Response {
    if !body.confirmed {
        return ApiError::BadRequest("Archive requires explicit confirmation".to_string())
            .into_response();
    }
    let project_id = ProjectId::new(project_id);
    let project = match state.project_catalog.get(&project_id) {
        Ok(Some(project)) => project,
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service.archive(&project, &state).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => placement_error(error),
    }
}

async fn reopen_project(State(state): State<AppState>, Path(project_id): Path<String>) -> Response {
    let project_id = ProjectId::new(project_id);
    let project = match state.project_catalog.get(&project_id) {
        Ok(Some(project)) => project,
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service.reopen(&project, &state).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => placement_error(error),
    }
}

async fn permanently_delete_project(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<ProjectLifecycleConfirmRequest>,
) -> Response {
    if !body.confirmed {
        return ApiError::BadRequest("Permanent Delete requires explicit confirmation".to_string())
            .into_response();
    }
    let project_id = ProjectId::new(project_id);
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service.permanent_delete(&project_id, &state).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => placement_error(error),
    }
}

async fn browse_directory(
    State(state): State<AppState>,
    Json(body): Json<BrowseDirectoryRequest>,
) -> Response {
    if body.device_id.trim().is_empty() || body.path.trim().is_empty() {
        return ApiError::BadRequest("device_id and path must not be empty".to_string())
            .into_response();
    }
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service
        .browse_directory(body.device_id.trim(), body.path.trim())
        .await
    {
        Ok(listing) => directory_response(body.device_id, listing),
        Err(error) => placement_error(error),
    }
}

async fn create_directory(
    State(state): State<AppState>,
    Json(body): Json<CreateDirectoryRequest>,
) -> Response {
    if body.device_id.trim().is_empty()
        || body.parent.trim().is_empty()
        || body.name.trim().is_empty()
    {
        return ApiError::BadRequest("device_id, parent and name must not be empty".to_string())
            .into_response();
    }
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service
        .create_directory(body.device_id.trim(), body.parent.trim(), body.name.trim())
        .await
    {
        Ok(listing) => directory_response(body.device_id, listing),
        Err(error) => placement_error(error),
    }
}

fn directory_response(
    device_id: String,
    listing: rua_project::ProjectDirectoryListing,
) -> Response {
    let proposed_name = rua_project::propose_project_display_name(&listing.path);
    let parent_path = listing.path.parent().map(std::path::Path::to_path_buf);
    Json(BrowseDirectoryResponse {
        device_id,
        path: listing.path,
        parent_path,
        entries: listing.entries,
        roots: listing.roots,
        proposed_name,
    })
    .into_response()
}

#[derive(Debug, Default, Deserialize)]
struct ProjectResourceQuery {
    directory_id: Option<String>,
    #[serde(default)]
    path: String,
}
#[derive(Debug, Deserialize)]
struct ArchiveIdleChatsRequest {
    chat_ids: Option<Vec<String>>,
}
async fn archive_idle_project_chats(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<ArchiveIdleChatsRequest>,
) -> Response {
    let project = match state.project_catalog.get(&ProjectId::new(project_id)) {
        Ok(Some(project)) => project,
        Ok(None) => return ApiError::NotFound("Project not found".into()).into_response(),
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service
        .archive_idle_chats(&project, body.chat_ids, &state)
        .await
    {
        Ok(result) => Json(result).into_response(),
        Err(error) => placement_error(error),
    }
}

async fn get_project_files(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<ProjectResourceQuery>,
) -> Response {
    let project = match state.project_catalog.get(&ProjectId::new(project_id)) {
        Ok(Some(project)) => project,
        Ok(None) => return ApiError::NotFound("Project not found".into()).into_response(),
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    let Some(directory_id) = query.directory_id else {
        return ApiError::BadRequest("directory_id is required".into()).into_response();
    };
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service
        .inspect_resource(
            &project,
            &directory_id,
            ProjectResourceOperation::Files { path: query.path },
        )
        .await
    {
        Ok(ProjectDeviceResponse::Directory {
            device_id,
            path,
            entries,
            roots,
        }) => Json(BrowseDirectoryResponse {
            device_id,
            path,
            entries,
            roots,
            parent_path: None,
            proposed_name: project.display_name,
        })
        .into_response(),
        Ok(_) => placement_error(ProjectPlacementError::UnexpectedResponse),
        Err(error) => placement_error(error),
    }
}

async fn get_project_git_status(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<ProjectResourceQuery>,
) -> Response {
    let project_id = ProjectId::new(project_id);
    let project = match state.project_catalog.get(&project_id) {
        Ok(Some(project)) => project,
        Ok(None) => {
            return ApiError::NotFound("Project not found".to_string()).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    if let Some(directory_id) = query.directory_id {
        return match service
            .inspect_resource(&project, &directory_id, ProjectResourceOperation::GitStatus)
            .await
        {
            Ok(ProjectDeviceResponse::GitStatus { status }) => Json(status).into_response(),
            Ok(_) => placement_error(ProjectPlacementError::UnexpectedResponse),
            Err(error) => placement_error(error),
        };
    }
    match service.git_status(&project).await {
        Ok(status) => Json(status).into_response(),
        Err(error) => placement_error(error),
    }
}

#[derive(Debug, Deserialize)]
struct ProjectGitHistoryQuery {
    directory_id: Option<String>,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_git_history_limit")]
    limit: usize,
}

const fn default_git_history_limit() -> usize {
    30
}

async fn get_project_git_history(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<ProjectGitHistoryQuery>,
) -> Response {
    let project_id = ProjectId::new(project_id);
    let project = match state.project_catalog.get(&project_id) {
        Ok(Some(project)) => project,
        Ok(None) => return ApiError::NotFound("Project not found".to_string()).into_response(),
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    if let Some(directory_id) = query.directory_id {
        return match service
            .inspect_resource(
                &project,
                &directory_id,
                ProjectResourceOperation::GitHistory {
                    offset: query.offset,
                    limit: query.limit,
                },
            )
            .await
        {
            Ok(ProjectDeviceResponse::GitHistory { history }) => Json(history).into_response(),
            Ok(_) => placement_error(ProjectPlacementError::UnexpectedResponse),
            Err(error) => placement_error(error),
        };
    }
    match service
        .git_history(&project, query.offset, query.limit.clamp(1, 100))
        .await
    {
        Ok(history) => Json(history).into_response(),
        Err(error) => placement_error(error),
    }
}

async fn create_project(
    State(state): State<AppState>,
    Json(body): Json<CreateProjectRequest>,
) -> Response {
    if body.resource_root.is_none() && body.resource_roots.is_none() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"detail": "resource_roots or resource_root is required"})),
        )
            .into_response();
    }
    if body.resource_device_id.trim().is_empty()
        || (body.resource_root.is_some() && body.resource_roots.is_some())
    {
        return ApiError::BadRequest(
            "Provide a resource Device and either resource_roots or a legacy resource_root".into(),
        )
        .into_response();
    }
    let resource_roots = match body.resource_root {
        Some(root) => vec![root],
        None => body.resource_roots.unwrap_or_default(),
    };
    if resource_roots.is_empty()
        || resource_roots.iter().any(|root| root.trim().is_empty())
        || body.primary_index >= resource_roots.len()
    {
        return ApiError::BadRequest(
            "Select nonempty resource directories and a valid primary index".into(),
        )
        .into_response();
    }
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service
        .create(
            body.resource_device_id.trim(),
            &resource_roots,
            body.primary_index,
            body.display_name,
            &state,
        )
        .await
    {
        Ok(ProjectCreateResolution::Created(outcome)) => {
            (StatusCode::CREATED, Json(outcome)).into_response()
        }
        Ok(ProjectCreateResolution::Duplicate(owner)) => duplicate_response(owner),
        Err(error) => placement_error(error),
    }
}

#[derive(Deserialize)]
struct EditResourcesRequest {
    expected_revision: u64,
    resource_roots: Vec<std::path::PathBuf>,
    primary_index: usize,
    #[serde(default)]
    directory_ids: Vec<Option<String>>,
}

async fn edit_resources(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<EditResourcesRequest>,
) -> Response {
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service
        .edit_resources(
            &ProjectId::new(project_id),
            body.expected_revision,
            &body.resource_roots,
            body.primary_index,
            &body.directory_ids,
            &state,
        )
        .await
    {
        Ok(outcome) => Json(outcome).into_response(),
        Err(error) => placement_error(error),
    }
}

#[derive(Deserialize)]
struct SetPrimaryDirectoryRequest {
    expected_revision: u64,
    directory_id: String,
}

async fn set_primary_directory(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<SetPrimaryDirectoryRequest>,
) -> Response {
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service
        .set_primary_directory(
            &ProjectId::new(project_id),
            body.expected_revision,
            &body.directory_id,
            &state,
        )
        .await
    {
        Ok(project) => Json(project).into_response(),
        Err(error) => placement_error(error),
    }
}

async fn rename_project(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(body): Json<RenameProjectRequest>,
) -> Response {
    let project_id = ProjectId::new(project_id);
    match state
        .project_catalog
        .rename(&project_id, &body.display_name)
    {
        Ok(project) => {
            let publication = crate::sync_publication::publish_project_descriptor_committed(
                &state,
                project.clone(),
            )
            .await;
            if publication.status == crate::sync_publication::PublicationStatus::AppendFailed {
                tracing::warn!(
                    project_id = %project.project_id,
                    errors = ?publication.errors,
                    "Project renamed but descriptor publication failed"
                );
            }
            Json(project).into_response()
        }
        Err(rua_project::ProjectError::NotFound(_)) => {
            ApiError::NotFound(format!("Project not found: {project_id}")).into_response()
        }
        Err(rua_project::ProjectError::InvalidDisplayName) => {
            ApiError::BadRequest("display_name must not be empty".to_string()).into_response()
        }
        Err(error) => ApiError::Internal(error.to_string()).into_response(),
    }
}

async fn list_project_chats(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<ProjectChatListQuery>,
) -> Response {
    let project_id = ProjectId::new(project_id);
    match state.project_catalog.get(&project_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    }
    let scope_root = rua_core::RuaRoot::new(&state.rua_root)
        .chat_workspace()
        .as_path()
        .to_path_buf();
    let page =
        match rua_chat::ChatService::from_workspace(rua_core::WorkspaceRoot::new(&scope_root))
            .list(rua_chat::ChatListQuery {
                project_id: Some(project_id.to_string()),
                agent_id: None,
                user_id: None,
                channel: None,
                search: query.search,
                tag: query.tag,
                conversation_view: query.view,
                source_chat_id: query.source_chat_id,
                executor: query.executor,
                status: query.status,
                sort_by: query.sort_by,
                sort_order: query.sort_order,
                offset: query.offset.unwrap_or(0),
                limit: query.limit.unwrap_or(50).clamp(1, 200),
                archived: Some(query.archived.unwrap_or(false)),
            })
            .await
        {
            Ok(page) => page,
            Err(error) => return ApiError::Internal(error.to_string()).into_response(),
        };
    let mut chats = page
        .entries
        .into_iter()
        .map(crate::endpoints::chats::chat_from_index_entry)
        .collect::<Vec<_>>();
    if let Err(error) = crate::endpoints::chats::populate_collaboration_source_availability(
        &state.rua_root,
        &mut chats,
    )
    .await
    {
        return ApiError::Internal(error).into_response();
    }
    for chat in &mut chats {
        chat.status = state
            .conversation_authority
            .active_turn(&ChatAddress::new(&chat.id))
            .await
            .as_ref()
            .map(|turn| turn.status_hint().to_string())
            .unwrap_or_else(|| "idle".to_string());
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Total-Count",
        HeaderValue::from_str(&page.total.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0")),
    );
    (StatusCode::OK, headers, Json(chats)).into_response()
}

async fn search_project_chats(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<ProjectChatSearchQuery>,
) -> Response {
    let project_id = ProjectId::new(project_id);
    match state.project_catalog.get(&project_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    }
    let scope_root = rua_core::RuaRoot::new(&state.rua_root)
        .chat_workspace()
        .as_path()
        .to_path_buf();
    let saved =
        match rua_chat::ChatService::from_workspace(rua_core::WorkspaceRoot::new(&scope_root))
            .search(rua_chat::ChatSearchQuery {
                project_id: Some(project_id.to_string()),
                agent_id: None,
                query: query.q,
                offset: query.offset,
                limit: query.limit.clamp(1, 100),
            })
            .await
        {
            Ok(result) => result,
            Err(error) => return ApiError::Internal(error.to_string()).into_response(),
        };
    let results = saved
        .results
        .into_iter()
        .filter(|hit| hit.message.role == "user" || hit.message.role == "assistant")
        .map(|hit| dto::ChatSearchResult {
            chat_id: hit.chat_id,
            chat_name: hit.title,
            card_index: hit.message.sequence.saturating_sub(1) as usize,
            role: hit.message.role,
            snippet: hit.snippet,
            message_id: hit.message.message_id,
            turn_id: hit.message.turn_id,
            sequence: hit.message.sequence,
            span_start_sequence: hit.span.start_sequence,
            span_end_sequence: hit.span.end_sequence,
            preview: hit.preview.map(|preview| preview.text),
            extra: serde_json::Map::new(),
        })
        .collect();
    Json(dto::ChatSearchResponse {
        results,
        offset: saved.offset,
        limit: saved.limit,
        total: saved.total,
        next_offset: saved.next_offset,
        extra: serde_json::Map::new(),
    })
    .into_response()
}

async fn update_project_chat(
    State(state): State<AppState>,
    Path(chat_id): Path<String>,
    Json(mut update): Json<crate::services::project_chat::ProjectChatUpdate>,
) -> Response {
    let project_id =
        match crate::services::chat_context::load_chat_context(&state.rua_root, &chat_id).await {
            Ok(Some(context)) => match context.project_id {
                Some(id) => id,
                None => return ApiError::BadRequest("Chat has no Project".into()).into_response(),
            },
            Ok(None) => {
                return ApiError::NotFound(format!("Chat not found: {chat_id}")).into_response()
            }
            Err(error) => return error.into_response(),
        };
    let project_id = ProjectId::new(project_id);
    let project = match state.project_catalog.get(&project_id) {
        Ok(Some(project)) => project,
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    if project.lifecycle != ProjectLifecycle::Active {
        return ApiError::Conflict("Archived Project cannot update Chats".to_string())
            .into_response();
    }
    let chat_id = match crate::services::console_chat::resolve_chat_id(&chat_id) {
        Ok(chat_id) => chat_id,
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    if let Some(model) = update.model.take() {
        if update.name.is_some()
            || update.pinned.is_some()
            || update.pin_order.is_some()
            || update.archived.is_some()
        {
            return ApiError::BadRequest(
                "Project Chat model updates cannot be combined with list metadata updates"
                    .to_string(),
            )
            .into_response();
        }
        return apply_project_chat_model_update(&state, &project_id.to_string(), &chat_id, model)
            .await;
    }
    let Some(source_device_id) =
        crate::services::device_context::current_sync_device_id(&state.sync_device)
    else {
        return ApiError::Conflict("source Device identity is unavailable".to_string())
            .into_response();
    };

    let result = if source_device_id == project.resource_device_id {
        state
            .conversation_input_intake
            .update_project_chat(&project_id, &chat_id, update, &source_device_id)
            .await
    } else {
        let session_manager = match crate::services::device_session_access::current_session_manager(
            &state.device_network,
        )
        .await
        {
            Ok(manager) => manager,
            Err(error) => return ApiError::Unavailable(error).into_response(),
        };
        let command = crate::services::device_command_bus::ProjectDeviceCommand {
            command_id: format!("project-chat-update-{}", uuid::Uuid::new_v4()),
            source_device_id,
            target_device_id: project.resource_device_id,
            action: crate::services::device_command_bus::ProjectDeviceAction::UpdateChat {
                project_id,
                chat_id,
                update,
            },
        };
        match crate::services::device_command_bus::deliver_project_device_command_via_session(
            &session_manager,
            &command,
            std::time::Duration::from_secs(10),
        )
        .await
        {
            Ok(crate::services::device_command_bus::ProjectDeviceResponse::ChatUpdated {
                chat,
            }) => serde_json::from_value(chat)
                .map_err(|error| format!("resource Device returned invalid Project Chat: {error}")),
            Ok(_) => Err("resource Device returned an unexpected Project response".to_string()),
            Err(error) if error.failure_kind() == "device_offline" => {
                return ApiError::Unavailable(error.to_string()).into_response();
            }
            Err(error) => Err(error.to_string()),
        }
    };

    match result {
        Ok(chat) => Json(chat).into_response(),
        Err(error) if error.contains("not found") => ApiError::NotFound(error).into_response(),
        Err(error) => ApiError::Conflict(error).into_response(),
    }
}

#[derive(Debug, Serialize)]
struct DeleteArchivedProjectChatsResponse {
    deleted: Vec<String>,
    skipped: Vec<DeleteArchivedProjectChatsSkip>,
}

#[derive(Debug, Serialize)]
struct DeleteArchivedProjectChatsSkip {
    chat_id: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct ProjectChatArchiveDeleteQuery {
    #[serde(default)]
    force: bool,
    #[serde(flatten)]
    filter: ProjectChatListQuery,
}

async fn delete_archived_project_chats(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<ProjectChatArchiveDeleteQuery>,
) -> Response {
    let project_id = ProjectId::new(project_id);
    let project = match state.project_catalog.get(&project_id) {
        Ok(Some(project)) => project,
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    if project.lifecycle != ProjectLifecycle::Active {
        return ApiError::Conflict("Archived Project cannot delete Chats".to_string())
            .into_response();
    }
    let Some(source_device_id) =
        crate::services::device_context::current_sync_device_id(&state.sync_device)
    else {
        return ApiError::Conflict("source Device identity is unavailable".to_string())
            .into_response();
    };
    let scope_root = rua_core::RuaRoot::new(&state.rua_root)
        .chat_workspace()
        .as_path()
        .to_path_buf();
    let page =
        match rua_chat::ChatService::from_workspace(rua_core::WorkspaceRoot::new(&scope_root))
            .list(rua_chat::ChatListQuery {
                project_id: Some(project_id.to_string()),
                agent_id: None,
                user_id: None,
                channel: None,
                search: query.filter.search,
                tag: query.filter.tag,
                conversation_view: query.filter.view,
                source_chat_id: query.filter.source_chat_id,
                executor: query.filter.executor,
                status: query.filter.status,
                sort_by: rua_chat::ChatSortField::UpdatedAt,
                sort_order: rua_chat::SortOrder::Desc,
                offset: 0,
                limit: i64::MAX as u64,
                archived: Some(true),
            })
            .await
        {
            Ok(page) => page,
            Err(error) => return ApiError::Internal(error.to_string()).into_response(),
        };
    let mut deleted = Vec::new();
    let mut skipped = Vec::new();
    for entry in page.entries {
        match route_project_chat_delete(
            &state,
            &project_id,
            &entry.id,
            query.force,
            &source_device_id,
        )
        .await
        {
            Ok(result) => {
                deleted.push(result.chat_id);
            }
            Err(crate::services::project_chat::ProjectChatDeleteError::Busy(detail)) => {
                skipped.push(DeleteArchivedProjectChatsSkip {
                    chat_id: entry.id,
                    reason: detail,
                });
            }
            Err(crate::services::project_chat::ProjectChatDeleteError::NotFound(detail)) => {
                skipped.push(DeleteArchivedProjectChatsSkip {
                    chat_id: entry.id,
                    reason: detail,
                });
            }
            Err(crate::services::project_chat::ProjectChatDeleteError::Pinned(detail)) => {
                skipped.push(DeleteArchivedProjectChatsSkip {
                    chat_id: entry.id,
                    reason: detail,
                });
            }
            Err(error) => {
                skipped.push(DeleteArchivedProjectChatsSkip {
                    chat_id: entry.id,
                    reason: error.to_string(),
                });
            }
        }
    }
    Json(DeleteArchivedProjectChatsResponse { deleted, skipped }).into_response()
}

pub(crate) async fn delete_project_chat(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Query(query): Query<ProjectChatDeleteQuery>,
) -> Response {
    let project_id = ProjectId::new(project_id);
    let project = match state.project_catalog.get(&project_id) {
        Ok(Some(project)) => project,
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    if project.lifecycle != ProjectLifecycle::Active {
        return ApiError::Conflict("Archived Project cannot delete Chats".to_string())
            .into_response();
    }
    let chat_id = match crate::services::console_chat::resolve_chat_id(&chat_id) {
        Ok(chat_id) => chat_id,
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    let Some(source_device_id) =
        crate::services::device_context::current_sync_device_id(&state.sync_device)
    else {
        return ApiError::Conflict("source Device identity is unavailable".to_string())
            .into_response();
    };

    let result = route_project_chat_delete(
        &state,
        &project_id,
        &chat_id,
        query.force,
        &source_device_id,
    )
    .await;

    match result {
        Ok(result) => Json(serde_json::json!({
            "deleted": true,
            "chat_id": result.chat_id,
            "was_busy": result.was_busy,
        }))
        .into_response(),
        Err(crate::services::project_chat::ProjectChatDeleteError::Busy(detail)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "detail": detail,
                "code": "project_chat_busy",
                "requires_confirmation": true,
            })),
        )
            .into_response(),
        Err(crate::services::project_chat::ProjectChatDeleteError::NotFound(detail)) => {
            ApiError::NotFound(detail).into_response()
        }
        Err(crate::services::project_chat::ProjectChatDeleteError::Pinned(detail)) => {
            ApiError::Conflict(detail).into_response()
        }
        Err(crate::services::project_chat::ProjectChatDeleteError::Unavailable(detail)) => {
            ApiError::Unavailable(detail).into_response()
        }
        Err(error) => ApiError::Conflict(error.to_string()).into_response(),
    }
}

pub(crate) async fn delete_project_chat_via_authority(
    state: &AppState,
    project_id: ProjectId,
    chat_id: String,
) -> Result<
    crate::services::project_chat::ProjectChatDeleteResult,
    crate::services::project_chat::ProjectChatDeleteError,
> {
    let source_device_id = crate::services::device_context::current_sync_device_id(
        &state.sync_device,
    )
    .ok_or_else(|| {
        crate::services::project_chat::ProjectChatDeleteError::Failed(
            "source Device identity is unavailable".to_string(),
        )
    })?;
    route_project_chat_delete(state, &project_id, &chat_id, false, &source_device_id).await
}

async fn route_project_chat_delete(
    state: &AppState,
    project_id: &ProjectId,
    chat_id: &str,
    force: bool,
    source_device_id: &str,
) -> Result<
    crate::services::project_chat::ProjectChatDeleteResult,
    crate::services::project_chat::ProjectChatDeleteError,
> {
    use crate::services::device_command_bus::{
        deliver_project_device_command_via_session, ProjectDeviceAction, ProjectDeviceCommand,
        ProjectDeviceResponse,
    };
    use crate::services::project_chat::ProjectChatDeleteError;

    let project = state
        .project_catalog
        .get(project_id)
        .map_err(|error| ProjectChatDeleteError::Failed(error.to_string()))?
        .ok_or_else(|| {
            ProjectChatDeleteError::NotFound(format!("Project not found: {project_id}"))
        })?;
    if project.resource_device_id == source_device_id {
        return state
            .conversation_input_intake
            .delete_project_chat(project_id, chat_id, force, source_device_id)
            .await;
    }
    let session_manager =
        crate::services::device_session_access::current_session_manager(&state.device_network)
            .await
            .map_err(ProjectChatDeleteError::Unavailable)?;
    let command = ProjectDeviceCommand {
        command_id: format!("project-chat-delete-{}", uuid::Uuid::new_v4()),
        source_device_id: source_device_id.to_string(),
        target_device_id: project.resource_device_id,
        action: ProjectDeviceAction::DeleteChat {
            project_id: project_id.clone(),
            chat_id: chat_id.to_string(),
            force,
        },
    };
    match deliver_project_device_command_via_session(
        &session_manager,
        &command,
        std::time::Duration::from_secs(30),
    )
    .await
    {
        Ok(ProjectDeviceResponse::ChatDeleted { result }) => result,
        Ok(_) => Err(ProjectChatDeleteError::Failed(
            "resource Device returned an unexpected Project response".to_string(),
        )),
        Err(error) if error.failure_kind() == "device_offline" => {
            Err(ProjectChatDeleteError::Unavailable(error.to_string()))
        }
        Err(error) => Err(ProjectChatDeleteError::Failed(error.to_string())),
    }
}

async fn project_control_context(
    state: &AppState,
    project_id: &str,
    chat_id: &str,
) -> Result<ProjectControlTarget, ApiError> {
    resolve_project_control_target(state, project_id, chat_id)
        .await
        .map_err(project_target_error)
}

async fn dispatch_project_control(
    state: &AppState,
    context: &ProjectControlTarget,
    action: ConversationControlDeviceAction,
) -> Result<ConversationControlDeviceResponse, ApiError> {
    context
        .dispatch_control(state, action)
        .await
        .map_err(project_target_error)
}

fn project_target_error(error: ProjectControlError) -> ApiError {
    match error {
        ProjectControlError::ProjectNotFound { .. }
        | ProjectControlError::ProjectChatNotFound { .. } => ApiError::NotFound(error.to_string()),
        ProjectControlError::ArchivedProject | ProjectControlError::InvalidAuthorityState(_) => {
            ApiError::Conflict(error.to_string())
        }
        ProjectControlError::InvalidChatId(detail) => ApiError::BadRequest(detail),
        ProjectControlError::Store(detail) => ApiError::Internal(detail),
        ProjectControlError::UpgradeRequired(detail) => ApiError::UpgradeRequired(detail),
        ProjectControlError::Rejected { reason, detail } => {
            project_control_error(DeviceCommandDeliveryError::Rejected { reason, detail })
        }
        other => ApiError::Unavailable(other.to_string()),
    }
}

fn project_control_error(error: DeviceCommandDeliveryError) -> ApiError {
    if error.is_session_version_incompatible() {
        return ApiError::UpgradeRequired(error.to_string());
    }
    match error {
        DeviceCommandDeliveryError::Rejected { reason, detail } => {
            if reason.contains("not_found") {
                ApiError::NotFound(detail)
            } else if reason.contains("invalid")
                || reason.contains("wrong")
                || reason.contains("unavailable_model")
                || reason.contains("model_update")
            {
                ApiError::BadRequest(detail)
            } else {
                ApiError::Conflict(detail)
            }
        }
        DeviceCommandDeliveryError::AuthorityRedirect(_) => {
            ApiError::Conflict("Project Chat authority is fixed to its resource Device".to_string())
        }
        other => ApiError::Unavailable(other.to_string()),
    }
}

async fn get_project_resource_policy(
    State(state): State<AppState>,
    Path(chat_id): Path<String>,
) -> Response {
    let project_id =
        match crate::services::chat_context::load_chat_context(&state.rua_root, &chat_id).await {
            Ok(Some(context)) => match context.project_id {
                Some(id) => id,
                None => return ApiError::BadRequest("Chat has no Project".into()).into_response(),
            },
            Ok(None) => {
                return ApiError::NotFound(format!("Chat not found: {chat_id}")).into_response()
            }
            Err(error) => return error.into_response(),
        };
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::ViewResourcePolicy,
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::ResourcePolicy { workspace }) => {
            (StatusCode::OK, Json(workspace)).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected Project Chat Resource Policy response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn apply_project_chat_model_update(
    state: &AppState,
    project_id: &str,
    chat_id: &str,
    model: serde_json::Value,
) -> Response {
    let context = match project_control_context(state, project_id, chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    let chat = match dispatch_project_control(
        state,
        &context,
        ConversationControlDeviceAction::UpdateModel { model },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::ModelUpdated {
            chat: Some(chat), ..
        }) => chat,
        Ok(ConversationControlDeviceResponse::ModelUpdated { chat: None, .. }) => {
            return ApiError::Internal(
                "resource Device did not return the updated Project Chat".to_string(),
            )
            .into_response();
        }
        Ok(_) => {
            return ApiError::Internal(
                "unexpected Project Chat model control response".to_string(),
            )
            .into_response();
        }
        Err(error) => return error.into_response(),
    };
    let chat = match serde_json::from_value::<crate::endpoints::chats::ChatSpecDto>(chat) {
        Ok(chat) => chat,
        Err(error) => {
            return ApiError::Internal(format!(
                "resource Device returned invalid Project Chat: {error}"
            ))
            .into_response();
        }
    };
    Json(chat).into_response()
}

async fn switch_project_chat_agent(
    State(state): State<AppState>,
    Path(chat_id): Path<String>,
    Json(body): Json<ProjectChatAgentSwitchRequest>,
) -> Response {
    let project_id =
        match crate::services::chat_context::load_chat_context(&state.rua_root, &chat_id).await {
            Ok(Some(context)) => match context.project_id {
                Some(id) => id,
                None => return ApiError::BadRequest("Chat has no Project".into()).into_response(),
            },
            Ok(None) => {
                return ApiError::NotFound(format!("Chat not found: {chat_id}")).into_response()
            }
            Err(error) => return error.into_response(),
        };
    if body.expected_agent_id.trim().is_empty() || body.agent_id.trim().is_empty() {
        return ApiError::BadRequest(
            "expected_agent_id and agent_id must not be empty".to_string(),
        )
        .into_response();
    }
    let project_id = ProjectId::new(project_id);
    let project = match state.project_catalog.get(&project_id) {
        Ok(Some(project)) => project,
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    if project.lifecycle != ProjectLifecycle::Active {
        return ApiError::Conflict("Archived Project cannot switch Agent".to_string())
            .into_response();
    }
    let chat_id = match crate::services::console_chat::resolve_chat_id(&chat_id) {
        Ok(chat_id) => chat_id,
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    let Some(source_device_id) =
        crate::services::device_context::current_sync_device_id(&state.sync_device)
    else {
        return ApiError::Conflict("source Device identity is unavailable".to_string())
            .into_response();
    };

    let result = if source_device_id == project.resource_device_id {
        state
            .conversation_input_intake
            .switch_project_chat_agent(
                &project_id,
                &chat_id,
                body.expected_agent_id.trim(),
                body.agent_id.trim(),
                &source_device_id,
            )
            .await
    } else {
        let session_manager = match crate::services::device_session_access::current_session_manager(
            &state.device_network,
        )
        .await
        {
            Ok(manager) => manager,
            Err(error) => return ApiError::Unavailable(error).into_response(),
        };
        let command = crate::services::device_command_bus::ProjectDeviceCommand {
            command_id: format!("project-agent-switch-{}", uuid::Uuid::new_v4()),
            source_device_id,
            target_device_id: project.resource_device_id,
            action: crate::services::device_command_bus::ProjectDeviceAction::SwitchChatAgent {
                project_id,
                chat_id,
                expected_agent_id: body.expected_agent_id.trim().to_string(),
                agent_id: body.agent_id.trim().to_string(),
            },
        };
        match crate::services::device_command_bus::deliver_project_device_command_via_session(
            &session_manager,
            &command,
            std::time::Duration::from_secs(10),
        )
        .await
        {
            Ok(
                crate::services::device_command_bus::ProjectDeviceResponse::ChatAgentSelection {
                    state,
                },
            ) => Ok(state),
            Ok(_) => Err("resource Device returned an unexpected Project response".to_string()),
            Err(error) if error.failure_kind() == "device_offline" => {
                return ApiError::Unavailable(error.to_string()).into_response();
            }
            Err(error) => Err(error.to_string()),
        }
    };

    match result {
        Ok(state) => Json(state).into_response(),
        Err(error) => ApiError::Conflict(error).into_response(),
    }
}

fn placement_error(error: ProjectPlacementError) -> Response {
    match error {
        ProjectPlacementError::MissingLocalDevice => ApiError::Conflict(error.to_string()),
        ProjectPlacementError::Unavailable(_) => ApiError::Unavailable(error.to_string()),
        ProjectPlacementError::Rejected(_) | ProjectPlacementError::Invalid(_) => {
            ApiError::BadRequest(error.to_string())
        }
        ProjectPlacementError::Lifecycle(_) => ApiError::Conflict(error.to_string()),
        ProjectPlacementError::Store(_) | ProjectPlacementError::UnexpectedResponse => {
            ApiError::Internal(error.to_string())
        }
    }
    .into_response()
}

fn duplicate_response(owner: ProjectDescriptor) -> Response {
    let actions = match owner.lifecycle {
        ProjectLifecycle::Active => vec!["view"],
        ProjectLifecycle::Archived => vec!["view", "reopen"],
    };
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "detail": "This directory is already bound to a Project.",
            "code": "project_binding_duplicate",
            "project": owner,
            "actions": actions,
        })),
    )
        .into_response()
}
