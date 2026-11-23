use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use rua_core::ProjectId;
use rua_project::{ProjectDescriptor, ProjectLifecycle};
use serde::{Deserialize, Serialize};

use crate::errors::ApiError;
use crate::services::device_command_bus::{
    ConversationControlDeviceAction, ConversationControlDeviceResponse, DeviceCommandDeliveryError,
    ProjectDeviceResponse, ProjectResourceOperation,
};
use crate::services::project_control::{
    resolve_project_control_target, ProjectControlError, ProjectControlTarget,
};
use crate::services::projects::{
    BrowseDirectoryResponse, ProjectPlacementError, ProjectPlacementService,
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
            "/projects/{projectId}/execution-default",
            axum::routing::put(set_execution_default),
        )
        .route(
            "/devices/{deviceId}/execution-directory",
            get(browse_execution_directory),
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

#[derive(Debug, Deserialize)]
struct CreateProjectRequest {
    execution_default: Option<rua_core::ExecutionDirectorySelection>,
    directories: Vec<rua_project::ProjectResourceDirectoryInput>,
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
    let activity = match rua_session::chat_index::project_last_activity(
        state.chat_workspace().as_path(),
    )
    .await
    {
        Ok(activity) => activity,
        Err(error) => return ApiError::Internal(error).into_response(),
    };
    let entries = projects
        .into_iter()
        .map(|project| ProjectListEntry {
            last_activity_at: activity.get(project.project_id.as_str()).copied(),
            project,
        })
        .collect();
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
        .browse_directory(body.device_id.trim(), &body.path)
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
        .create_directory(body.device_id.trim(), &body.parent, &body.name)
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
    Json(BrowseDirectoryResponse::from_listing(device_id, listing)).into_response()
}

#[derive(Debug, Deserialize)]
struct ProjectResourceQuery {
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    resource_root: std::path::PathBuf,
    path: Option<std::path::PathBuf>,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct LocatedGitResult<T> {
    device_id: String,
    resource_root: std::path::PathBuf,
    #[serde(flatten)]
    result: T,
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
    let Some(path) = query.path.clone() else {
        return ApiError::BadRequest("An explicit resource path is required".into())
            .into_response();
    };
    let operation = ProjectResourceOperation::Files {
        path,
        offset: query.offset,
        limit: query.limit.unwrap_or(100),
    };
    inspect_resource_response(&state, project_id, query, operation).await
}

async fn get_project_git_status(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<ProjectResourceQuery>,
) -> Response {
    inspect_resource_response(
        &state,
        project_id,
        query,
        ProjectResourceOperation::GitStatus,
    )
    .await
}

async fn get_project_git_history(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(query): Query<ProjectResourceQuery>,
) -> Response {
    let operation = ProjectResourceOperation::GitHistory {
        offset: query.offset,
        limit: query.limit.unwrap_or(30),
    };
    inspect_resource_response(&state, project_id, query, operation).await
}

async fn inspect_resource_response(
    state: &AppState,
    project_id: String,
    query: ProjectResourceQuery,
    operation: ProjectResourceOperation,
) -> Response {
    if query.device_id.trim().is_empty() || query.resource_root.as_os_str().is_empty() {
        return ApiError::BadRequest("device_id and resource_root are required".into())
            .into_response();
    }
    let project = match state.project_catalog.get(&ProjectId::new(project_id)) {
        Ok(Some(project)) => project,
        Ok(None) => return ApiError::NotFound("Project not found".into()).into_response(),
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    let service = match ProjectPlacementService::from_state(state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    let response = service
        .inspect_resource(
            &project,
            &query.device_id,
            &query.resource_root,
            operation.clone(),
        )
        .await;
    match (operation, response) {
        (
            ProjectResourceOperation::Files { .. },
            Ok(ProjectDeviceResponse::Directory {
                device_id,
                path,
                entries,
                roots,
                omitted_entries,
                next_offset,
            }),
        ) if device_id == query.device_id => Json(BrowseDirectoryResponse {
            device_id,
            path,
            entries,
            roots,
            omitted_entries,
            next_offset,
            parent_path: None,
            proposed_name: project.display_name,
        })
        .into_response(),
        (ProjectResourceOperation::GitStatus, Ok(ProjectDeviceResponse::GitStatus { status })) => {
            Json(LocatedGitResult {
                device_id: query.device_id,
                resource_root: query.resource_root,
                result: status,
            })
            .into_response()
        }
        (
            ProjectResourceOperation::GitHistory { .. },
            Ok(ProjectDeviceResponse::GitHistory { history }),
        ) => Json(LocatedGitResult {
            device_id: query.device_id,
            resource_root: query.resource_root,
            result: history,
        })
        .into_response(),
        (_, Err(error)) => placement_error(error),
        _ => placement_error(ProjectPlacementError::UnexpectedResponse),
    }
}

#[derive(Deserialize)]
struct ExecutionDirectoryBrowseQuery {
    path: Option<String>,
}

async fn browse_execution_directory(
    State(state): State<AppState>,
    Path(device): Path<String>,
    Query(query): Query<ExecutionDirectoryBrowseQuery>,
) -> Response {
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service
        .browse_execution_directory(&device, query.path.as_deref().unwrap_or("~"))
        .await
    {
        Ok(listing) => Json(listing).into_response(),
        Err(error) => placement_error(error),
    }
}

async fn set_execution_default(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(selection) = body.get("execution_default") else {
        return ApiError::BadRequest("execution_default is required".into()).into_response();
    };
    let selection = match serde_json::from_value(selection.clone()) {
        Ok(selection) => selection,
        Err(e) => return ApiError::BadRequest(e.to_string()).into_response(),
    };
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service
        .set_execution_default(&ProjectId::new(id), selection, &state)
        .await
    {
        Ok(project) => Json(project).into_response(),
        Err(error) => placement_error(error),
    }
}

async fn create_project(
    State(state): State<AppState>,
    Json(body): Json<CreateProjectRequest>,
) -> Response {
    let service = match ProjectPlacementService::from_state(&state) {
        Ok(service) => service,
        Err(error) => return placement_error(error),
    };
    match service
        .create(
            body.directories,
            body.display_name,
            body.execution_default,
            &state,
        )
        .await
    {
        Ok(outcome) => (StatusCode::CREATED, Json(outcome)).into_response(),
        Err(error) => placement_error(error),
    }
}

#[derive(Deserialize)]
struct EditResourcesRequest {
    expected_revision: u64,
    directories: Vec<rua_project::ProjectResourceDirectoryInput>,
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
            body.directories,
            &state,
        )
        .await
    {
        Ok(outcome) => Json(outcome).into_response(),
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

fn require_discovery_project(state: &AppState, project_id: &str) -> Result<(), ApiError> {
    match state.project_catalog.get(&ProjectId::new(project_id)) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(ApiError::NotFound(format!(
            "Project not found: {project_id}"
        ))),
        Err(error) => Err(ApiError::Internal(error.to_string())),
    }
}

async fn list_project_chats(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(params): Query<super::chats::ListChatsParams>,
) -> Response {
    if let Err(error) = require_discovery_project(&state, &project_id) {
        return error.into_response();
    }
    super::chats::list_chats_core(&state, params, Some(&project_id)).await
}

async fn search_project_chats(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Query(params): Query<super::chats::ListChatsParams>,
) -> Response {
    if let Err(error) = require_discovery_project(&state, &project_id) {
        return error.into_response();
    }
    super::chat_search::search_chats(&state, params, Some(&project_id)).await
}

#[derive(serde::Deserialize)]
struct ProjectChatPatch {
    #[serde(flatten)]
    update: crate::services::project_chat::ProjectChatUpdate,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

async fn update_project_chat(
    State(state): State<AppState>,
    Path(chat_id): Path<String>,
    Json(patch): Json<ProjectChatPatch>,
) -> Response {
    let requested_chat_id = match crate::services::console_chat::resolve_chat_id(&chat_id) {
        Ok(chat_id) => chat_id,
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    let membership = match crate::services::chat_context::saved_project_membership(
        &state.rua_root,
        &requested_chat_id,
    )
    .await
    {
        Ok(Some(membership)) => membership,
        Ok(None) => {
            return ApiError::NotFound(format!("Chat not found: {requested_chat_id}"))
                .into_response();
        }
        Err(error) => return error.into_response(),
    };
    let requested =
        match crate::services::chat_context::requested_project(patch.extra.get("project_id")) {
            Ok(requested) => requested,
            Err(error) => return error.into_response(),
        };
    if let Err(error) =
        crate::services::chat_context::check_project_membership(membership.as_deref(), requested)
    {
        return error.into_response();
    }
    let Some(project_id) = membership else {
        return ApiError::BadRequest("Chat has no Project".into()).into_response();
    };
    let mut update = patch.update;
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

    let placement =
        match crate::services::conversation_authority_target::load_chat_authority_placement_target(
            state.chat_workspace().as_path(),
            &chat_id,
        )
        .await
        {
            Ok(placement) => placement,
            Err(error) => return ApiError::Conflict(error).into_response(),
        };
    let result = if source_device_id == placement.device_id {
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
            target_device_id: placement.device_id,
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
            }) => serde_json::from_value(chat).map_err(|error| {
                format!("Chat authority Device returned invalid Project Chat: {error}")
            }),
            Ok(_) => {
                Err("Chat authority Device returned an unexpected Project response".to_string())
            }
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
    filter: super::chats::ListChatsParams,
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
    let filters = match query.filter.into_query(Some(project_id.as_str())) {
        Ok(filters) => filters,
        Err(error) => return error.into_response(),
    };
    let page = match rua_chat::ChatService::new(&state.rua_root)
        .list(rua_chat::ChatListQuery {
            sort_by: rua_chat::ChatSortField::UpdatedAt,
            sort_order: rua_chat::SortOrder::Desc,
            archived: Some(true),
            offset: 0,
            limit: i64::MAX as u64,
            ..filters
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

    let header = crate::services::chat_context::read_chat_header(&state.rua_root, chat_id)
        .await
        .map_err(|e| ProjectChatDeleteError::Failed(e.to_string()))?
        .filter(|h| h.project_id.as_deref() == Some(project_id.as_str()))
        .ok_or_else(|| {
            ProjectChatDeleteError::NotFound(format!("Project Chat not found: {chat_id}"))
        })?;
    let placement =
        crate::services::conversation_authority_placement::parse_conversation_authority_placement(
            &serde_json::Value::Object(header.extra),
        )
        .map_err(|e| ProjectChatDeleteError::Failed(e.to_string()))?;
    if placement.device_id == source_device_id {
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
        target_device_id: placement.device_id,
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
            "Chat authority Device returned an unexpected Project response".to_string(),
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
        DeviceCommandDeliveryError::AuthorityRedirect(_) => ApiError::Conflict(
            "Project Chat authority is fixed to its saved execution Device".to_string(),
        ),
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
                "Chat authority Device did not return the updated Project Chat".to_string(),
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
                "Chat authority Device returned invalid Project Chat: {error}"
            ))
            .into_response();
        }
    };
    Json(chat).into_response()
}

fn placement_error(error: ProjectPlacementError) -> Response {
    match error {
        ProjectPlacementError::NotFound(_) => ApiError::NotFound(error.to_string()),
        ProjectPlacementError::MissingLocalDevice => ApiError::Conflict(error.to_string()),
        ProjectPlacementError::Unavailable(_) | ProjectPlacementError::OutcomeUnconfirmed(_) => {
            ApiError::Unavailable(error.to_string())
        }
        ProjectPlacementError::Rejected(_) | ProjectPlacementError::Invalid(_) => {
            ApiError::BadRequest(error.to_string())
        }
        ProjectPlacementError::Conflict(_) => ApiError::Conflict(error.to_string()),
        ProjectPlacementError::Store(_) | ProjectPlacementError::UnexpectedResponse => {
            ApiError::Internal(error.to_string())
        }
    }
    .into_response()
}
