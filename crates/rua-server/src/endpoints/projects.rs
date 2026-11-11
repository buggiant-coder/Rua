use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use futures::StreamExt;
use rua_channel::core::console::{AttachmentInfo, ConsoleChatRequest};
use rua_core::{ChatAddress, ChatScopeStore, ProjectId};
use rua_project::{ProjectDescriptor, ProjectLifecycle};
use serde::{Deserialize, Serialize};

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::services::conversation_authority_placement::ConversationAuthorityEpoch;
use crate::services::device_command_bus::{
    AcpChatDeviceAction, AcpChatDeviceResponse, ConversationApprovalDecision,
    ConversationControlDeviceAction, ConversationControlDeviceResponse, DeviceCommandDeliveryError,
    ProjectDeviceResponse, ProjectResourceOperation,
};
use crate::services::project_control::{
    resolve_optional_project_control_target, resolve_project_control_target, ProjectControlError,
    ProjectControlTarget,
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
        .route(
            "/projects/{projectId}/chats/{chatId}/console/chat",
            post(project_chat_cli),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}",
            get(get_project_chat)
                .put(update_project_chat_control)
                .patch(update_project_chat)
                .delete(delete_project_chat),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/metadata",
            get(get_project_chat_metadata),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/configuration",
            get(preview_project_chat_configuration).put(apply_project_chat_configuration),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/running-config",
            get(get_project_chat_running_config).put(put_project_chat_running_config),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/rollback",
            post(rollback_project_chat),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/projection-exclusions",
            get(get_project_projection_exclusions).put(put_project_projection_exclusions),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/branch",
            post(branch_project_chat),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/console/submit",
            post(submit_project_chat),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/llm-debug-capture",
            get(project_llm_debug_capture_status)
                .post(project_llm_debug_capture_start)
                .delete(project_llm_debug_capture_stop),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/console/live",
            post(project_chat_live),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/console/activity",
            get(project_chat_activity),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/console/stop",
            post(stop_project_chat),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/resource-policy",
            get(get_project_resource_policy),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/acp-history-preview",
            post(persist_project_acp_history_preview),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/queue/{inputId}",
            patch(edit_project_queued_input).delete(delete_project_queued_input),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/queue/{inputId}/send-now",
            post(send_now_project_queued_input),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/approvals/snapshot",
            get(get_project_approval_snapshot),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/approvals/{approvalId}/decide",
            post(decide_project_approval),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/acp/runners",
            get(list_project_acp_runners),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/acp/sessions",
            get(list_project_acp_sessions),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/acp/enter",
            post(enter_project_acp),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/acp/exit",
            post(exit_project_acp),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/acp/status",
            get(project_acp_status),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/acp/permissions",
            get(project_acp_permission_snapshot),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/acp/permissions/{requestId}/decide",
            post(decide_project_acp_permission),
        )
        .route(
            "/projects/{projectId}/chats/{chatId}/agent-selection",
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
struct ProjectChatSubmitRequest {
    agent_id: String,
    #[serde(flatten)]
    chat: serde_json::Map<String, serde_json::Value>,
}

struct PreparedProjectChatSubmit {
    project_id: ProjectId,
    chat_id: String,
    requested_agent_id: String,
    request: ConsoleChatRequest,
}

#[derive(Serialize)]
struct ProjectLlmDebugCaptureStatus {
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<std::path::PathBuf>,
}

impl ProjectLlmDebugCaptureStatus {
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

fn project_capture_address(project_id: String, chat_id: String) -> ChatAddress {
    ChatAddress::project(ProjectId::new(project_id), chat_id)
}

async fn project_llm_debug_capture_status(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Json<ProjectLlmDebugCaptureStatus> {
    let address = project_capture_address(project_id, chat_id);
    let scope_root = ChatScopeStore::resolve(&state.rua_root, address.clone());
    Json(
        state
            .llm_debug_captures
            .get(scope_root.root(), &address)
            .ok()
            .flatten()
            .map(ProjectLlmDebugCaptureStatus::active)
            .unwrap_or_else(ProjectLlmDebugCaptureStatus::inactive),
    )
}

async fn project_llm_debug_capture_start(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Json<ProjectLlmDebugCaptureStatus> {
    let address = project_capture_address(project_id, chat_id);
    let scope_root = ChatScopeStore::resolve(&state.rua_root, address.clone());
    let status = match state.llm_debug_captures.start(scope_root.root(), address) {
        Ok(info) => ProjectLlmDebugCaptureStatus::active(info),
        Err(error) => {
            tracing::warn!(%error, "failed to start local LLM debug capture");
            ProjectLlmDebugCaptureStatus::inactive()
        }
    };
    Json(status)
}

async fn project_llm_debug_capture_stop(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Json<ProjectLlmDebugCaptureStatus> {
    let address = project_capture_address(project_id, chat_id);
    let scope_root = ChatScopeStore::resolve(&state.rua_root, address.clone());
    if let Err(error) = state.llm_debug_captures.stop(scope_root.root(), &address) {
        tracing::warn!(%error, "failed to stop local LLM debug capture");
    }
    Json(ProjectLlmDebugCaptureStatus::inactive())
}

#[derive(Debug, Deserialize)]
struct ProjectChatLiveRequest {
    viewer_id: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    activity_generation: Option<String>,
    #[serde(default)]
    next_activity_sequence: Option<u64>,
}

impl ProjectChatLiveRequest {
    fn activity_position(&self) -> Option<rua_core::ConversationActivityPosition> {
        Some(rua_core::ConversationActivityPosition::new(
            self.activity_generation.as_deref()?.trim(),
            self.next_activity_sequence?,
        ))
        .filter(|position| !position.activity_generation.is_empty())
    }
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
struct ProjectChatDeleteQuery {
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize)]
struct ProjectChatControlUpdate {
    #[serde(default)]
    model: Option<serde_json::Value>,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    selected_variant_id: Option<String>,
}

async fn preview_project_chat_configuration(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    project_chat_configuration(
        &state,
        &project_id,
        &chat_id,
        ConversationControlDeviceAction::ViewConfiguration,
    )
    .await
}

async fn apply_project_chat_configuration(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<crate::endpoints::chats::ChatConfigurationApply>,
) -> Response {
    project_chat_configuration(
        &state,
        &project_id,
        &chat_id,
        ConversationControlDeviceAction::UpdateConfiguration {
            expected_epoch: body.expected_epoch,
            candidate_epoch: body.candidate_epoch,
        },
    )
    .await
}

async fn project_chat_configuration(
    state: &AppState,
    project_id: &str,
    chat_id: &str,
    action: ConversationControlDeviceAction,
) -> Response {
    let context = match project_control_context(state, project_id, chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(state, &context, action).await {
        Ok(ConversationControlDeviceResponse::Configuration { configuration }) => {
            Json(configuration).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected Project configuration response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn get_project_chat_running_config(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::ViewRunningConfig,
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::RunningConfig { running }) => {
            Json(running).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected Project Chat running-config response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn put_project_chat_running_config(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(running): Json<serde_json::Value>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::UpdateRunningConfig { running },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::RunningConfigUpdated { running, .. }) => {
            Json(running).into_response()
        }
        Ok(_) => {
            ApiError::Internal("unexpected Project Chat running-config update response".to_string())
                .into_response()
        }
        Err(error) => error.into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct ProjectChatBranchRequest {
    before_turn_id: String,
}

#[derive(Debug, Deserialize)]
struct ProjectQueueEditRequest {
    message: String,
}

#[derive(Debug, Deserialize)]
struct ProjectQueueSendNowRequest {
    command_id: String,
    authority_epoch: ConversationAuthorityEpoch,
}

#[derive(Debug, Deserialize)]
struct ProjectAcpSessionsQuery {
    runner: String,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProjectAcpEnterRequest {
    runner: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    resume_session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProjectAcpPermissionDecision {
    option_id: String,
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
    let mut entries = Vec::with_capacity(projects.len());
    for project in projects {
        let scope = ChatScopeStore::resolve(
            &state.rua_root,
            ChatAddress::project(
                project.project_id.clone(),
                "00000000-0000-4000-8000-000000000000",
            ),
        );
        let last_activity_at = match rua_session::chat_index::latest_activity(scope.root()).await {
            Ok(activity) => activity,
            Err(error) => return ApiError::Internal(error).into_response(),
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
    let scope_root = ChatScopeStore::resolve(
        &state.rua_root,
        ChatAddress::project(project_id.clone(), "00000000-0000-4000-8000-000000000000"),
    )
    .root()
    .to_path_buf();
    let address = ChatAddress::project(project_id.clone(), "00000000-0000-4000-8000-000000000000");
    let page = match rua_chat::ChatService::for_resolved_scope(&scope_root, address.clone())
        .list(
            &address,
            rua_chat::ChatListQuery {
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
            },
        )
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
            .active_turn(&ChatAddress::project(project_id.clone(), &chat.id))
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
    let scope_root = ChatScopeStore::resolve(
        &state.rua_root,
        ChatAddress::project(project_id.clone(), "00000000-0000-4000-8000-000000000000"),
    )
    .root()
    .to_path_buf();
    let address = ChatAddress::project(project_id, "00000000-0000-4000-8000-000000000000");
    let saved = match rua_chat::ChatService::for_resolved_scope(&scope_root, address.clone())
        .search(
            &address,
            rua_chat::ChatSearchQuery {
                query: query.q,
                offset: query.offset,
                limit: query.limit.clamp(1, 100),
            },
        )
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
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(mut update): Json<crate::services::project_chat::ProjectChatUpdate>,
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
    let scope_root = ChatScopeStore::resolve(
        &state.rua_root,
        ChatAddress::project(project_id.clone(), "00000000-0000-4000-8000-000000000000"),
    )
    .root()
    .to_path_buf();
    let address = ChatAddress::project(project_id.clone(), "00000000-0000-4000-8000-000000000000");
    let page = match rua_chat::ChatService::for_resolved_scope(&scope_root, address.clone())
        .list(
            &address,
            rua_chat::ChatListQuery {
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
            },
        )
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

async fn delete_project_chat(
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

async fn project_control_context_if_materialized(
    state: &AppState,
    project_id: &str,
    chat_id: &str,
) -> Result<Option<ProjectControlTarget>, ApiError> {
    resolve_optional_project_control_target(state, project_id, chat_id)
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

async fn dispatch_project_acp(
    state: &AppState,
    context: &ProjectControlTarget,
    action: AcpChatDeviceAction,
) -> Result<AcpChatDeviceResponse, ApiError> {
    context
        .dispatch_acp(state, action)
        .await
        .map_err(|error| match error {
            ProjectControlError::Rejected { reason, detail } => {
                project_acp_error(DeviceCommandDeliveryError::Rejected { reason, detail })
            }
            other => project_target_error(other),
        })
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

fn project_acp_error(error: DeviceCommandDeliveryError) -> ApiError {
    if error.is_session_version_incompatible() {
        return ApiError::UpgradeRequired(error.to_string());
    }
    match error {
        DeviceCommandDeliveryError::Rejected { reason, detail } => {
            if reason.contains("not_found") {
                ApiError::NotFound(detail)
            } else if reason.contains("disabled")
                || reason.contains("invalid")
                || reason.contains("wrong")
                || reason.contains("unknown_option")
            {
                ApiError::BadRequest(detail)
            } else if reason.contains("unavailable") && !reason.contains("project_resource") {
                ApiError::Unavailable(detail)
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

async fn update_project_chat_control(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<ProjectChatControlUpdate>,
) -> Response {
    match (body.model, body.turn_id, body.selected_variant_id) {
        (Some(model), None, None) => {
            apply_project_chat_model_update(&state, &project_id, &chat_id, model).await
        }
        (None, Some(turn_id), Some(selected_variant_id)) => {
            let context = match project_control_context(&state, &project_id, &chat_id).await {
                Ok(context) => context,
                Err(error) => return error.into_response(),
            };
            match dispatch_project_control(
                &state,
                &context,
                ConversationControlDeviceAction::SelectVariant {
                    turn_id: turn_id.clone(),
                    selected_variant_id: selected_variant_id.clone(),
                },
            )
            .await
            {
                Ok(ConversationControlDeviceResponse::VariantSelected { .. }) => {
                    Json(serde_json::json!({
                        "turn_id": turn_id,
                        "selected_variant_id": selected_variant_id,
                    }))
                    .into_response()
                }
                Ok(_) => ApiError::Internal(
                    "unexpected Project Chat variant selection response".to_string(),
                )
                .into_response(),
                Err(error) => error.into_response(),
            }
        }
        _ => ApiError::BadRequest(
            "Project Chat control requires either model or turn_id with selected_variant_id"
                .to_string(),
        )
        .into_response(),
    }
}

async fn rollback_project_chat(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<ProjectChatBranchRequest>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::RollbackBeforeTurn {
            before_turn_id: body.before_turn_id,
        },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::RolledBack {
            chat,
            cancelled_tasks,
        }) => Json(serde_json::json!({
            "chat": chat,
            "cancelled_tasks": cancelled_tasks,
        }))
        .into_response(),
        Ok(_) => ApiError::Internal("unexpected Project Chat rollback response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct ProjectProjectionExclusionsDto {
    #[serde(default)]
    message_ids: Vec<String>,
}

async fn get_project_projection_exclusions(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::ViewProjectionExclusions,
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::ProjectionExclusions { message_ids }) => {
            Json(ProjectProjectionExclusionsDto { message_ids }).into_response()
        }
        Ok(_) => {
            ApiError::Internal("unexpected Project Chat projection exclusions response".to_string())
                .into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn put_project_projection_exclusions(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<ProjectProjectionExclusionsDto>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::UpdateProjectionExclusions {
            message_ids: body.message_ids,
        },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::ProjectionExclusionsUpdated { message_ids }) => {
            Json(ProjectProjectionExclusionsDto { message_ids }).into_response()
        }
        Ok(_) => ApiError::Internal(
            "unexpected Project Chat projection exclusions update response".to_string(),
        )
        .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn branch_project_chat(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<ProjectChatBranchRequest>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::BranchBeforeTurn {
            before_turn_id: body.before_turn_id,
        },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::Branched { chat }) => {
            Json(serde_json::json!({ "chat": chat })).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected Project Chat branch response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn get_project_resource_policy(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
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

async fn persist_project_acp_history_preview(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<crate::endpoints::chats::AcpHistoryPreviewBody>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    crate::endpoints::chats::persist_acp_history_preview_core(
        context.address(),
        context.agent_id(),
        context.scope_root(),
        context.address().chat_id().as_str(),
        body,
        &state,
    )
    .await
}

async fn edit_project_queued_input(
    State(state): State<AppState>,
    Path((project_id, chat_id, input_id)): Path<(String, String, String)>,
    Json(body): Json<ProjectQueueEditRequest>,
) -> Response {
    if body.message.trim().is_empty() {
        return ApiError::BadRequest("message cannot be empty".to_string()).into_response();
    }
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::EditQueuedInput {
            input_id,
            message: body.message,
        },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::Edited { entry, control }) => {
            Json(serde_json::json!({ "entry": entry, "control": control })).into_response()
        }
        Ok(_) => {
            ApiError::Internal("unexpected Project queue edit response".to_string()).into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn delete_project_queued_input(
    State(state): State<AppState>,
    Path((project_id, chat_id, input_id)): Path<(String, String, String)>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::DeleteQueuedInput { input_id },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::Deleted {
            deleted_input_id,
            control,
        }) => Json(serde_json::json!({
            "deleted_input_id": deleted_input_id,
            "control": control,
        }))
        .into_response(),
        Ok(_) => ApiError::Internal("unexpected Project queue delete response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn send_now_project_queued_input(
    State(state): State<AppState>,
    Path((project_id, chat_id, input_id)): Path<(String, String, String)>,
    Json(body): Json<ProjectQueueSendNowRequest>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    if body.authority_epoch != context.authority_epoch() {
        return ApiError::Conflict("stale conversation authority Epoch".to_string())
            .into_response();
    }
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::SendNow {
            input_id,
            control_command_id: body.command_id,
        },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::SendNow {
            input_id,
            applied,
            control,
        }) => Json(serde_json::json!({
            "input_id": input_id,
            "applied": applied,
            "control": control,
        }))
        .into_response(),
        Ok(_) => {
            ApiError::Internal("unexpected Project Send Now response".to_string()).into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn stop_project_chat(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<super::dto::StopChatRequest>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(&state, &context, body.into_action()).await {
        Ok(ConversationControlDeviceResponse::StopRequested { applied }) => {
            super::console::precise_stop_response(applied)
        }
        Ok(_) => ApiError::Internal("unexpected stop response".into()).into_response(),
        Err(error) => error.into_response(),
    }
}

async fn get_project_approval_snapshot(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    let context = match project_control_context_if_materialized(&state, &project_id, &chat_id).await
    {
        Ok(Some(context)) => context,
        Ok(None) => {
            return Json(rua_security::ApprovalSnapshot {
                pending: Vec::new(),
                resolved: Vec::new(),
            })
            .into_response();
        }
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::ApprovalSnapshot,
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::ApprovalSnapshot { snapshot }) => {
            Json(snapshot).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected Project approval snapshot response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn decide_project_approval(
    State(state): State<AppState>,
    Path((project_id, chat_id, approval_id)): Path<(String, String, String)>,
    Json(decision): Json<ConversationApprovalDecision>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_control(
        &state,
        &context,
        ConversationControlDeviceAction::DecideApproval {
            approval_id,
            decision,
        },
    )
    .await
    {
        Ok(ConversationControlDeviceResponse::ApprovalDecided { request }) => {
            Json(request).into_response()
        }
        Ok(_) => ApiError::Internal("unexpected Project approval decision response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn list_project_acp_runners(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_acp(&state, &context, AcpChatDeviceAction::ListRunners).await {
        Ok(AcpChatDeviceResponse::Runners { runners }) => Json(runners).into_response(),
        Ok(_) => ApiError::Internal("unexpected Project ACP runners response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn list_project_acp_sessions(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Query(query): Query<ProjectAcpSessionsQuery>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_acp(
        &state,
        &context,
        AcpChatDeviceAction::ListSessions {
            runner: query.runner,
            cwd: query.cwd,
        },
    )
    .await
    {
        Ok(AcpChatDeviceResponse::Sessions { sessions }) => Json(sessions).into_response(),
        Ok(_) => ApiError::Internal("unexpected Project ACP sessions response".to_string())
            .into_response(),
        Err(error) => error.into_response(),
    }
}

async fn enter_project_acp(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<ProjectAcpEnterRequest>,
) -> Response {
    if body.runner.trim().is_empty() {
        return ApiError::BadRequest("runner is required".to_string()).into_response();
    }
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_acp(
        &state,
        &context,
        AcpChatDeviceAction::Enter {
            runner: body.runner,
            cwd: body.cwd,
            resume_session_id: body.resume_session_id,
        },
    )
    .await
    {
        Ok(AcpChatDeviceResponse::Entered {
            ok,
            message,
            status,
            history_preview,
        }) => Json(serde_json::json!({
            "ok": ok,
            "message": message,
            "status": status,
            "history_preview": history_preview,
        }))
        .into_response(),
        Ok(_) => {
            ApiError::Internal("unexpected Project ACP enter response".to_string()).into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn exit_project_acp(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_acp(&state, &context, AcpChatDeviceAction::Exit).await {
        Ok(AcpChatDeviceResponse::Exited { ok, message }) => {
            Json(serde_json::json!({ "ok": ok, "message": message })).into_response()
        }
        Ok(_) => {
            ApiError::Internal("unexpected Project ACP exit response".to_string()).into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn project_acp_status(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_acp(&state, &context, AcpChatDeviceAction::Status).await {
        Ok(AcpChatDeviceResponse::Status { status }) => Json(status).into_response(),
        Ok(_) => {
            ApiError::Internal("unexpected Project ACP status response".to_string()).into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn project_acp_permission_snapshot(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    let context = match project_control_context_if_materialized(&state, &project_id, &chat_id).await
    {
        Ok(Some(context)) => context,
        Ok(None) => {
            return Json(rua_acp::permissions::AcpPermissionSnapshot::default()).into_response();
        }
        Err(error) => return error.into_response(),
    };
    match dispatch_project_acp(&state, &context, AcpChatDeviceAction::PermissionSnapshot).await {
        Ok(AcpChatDeviceResponse::PermissionSnapshot { snapshot }) => {
            Json(snapshot).into_response()
        }
        Ok(_) => {
            ApiError::Internal("unexpected Project ACP permission snapshot response".to_string())
                .into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn decide_project_acp_permission(
    State(state): State<AppState>,
    Path((project_id, chat_id, request_id)): Path<(String, String, String)>,
    Json(body): Json<ProjectAcpPermissionDecision>,
) -> Response {
    if body.option_id.trim().is_empty() {
        return ApiError::BadRequest("option_id is required".to_string()).into_response();
    }
    let context = match project_control_context(&state, &project_id, &chat_id).await {
        Ok(context) => context,
        Err(error) => return error.into_response(),
    };
    match dispatch_project_acp(
        &state,
        &context,
        AcpChatDeviceAction::DecidePermission {
            request_id,
            option_id: body.option_id,
        },
    )
    .await
    {
        Ok(AcpChatDeviceResponse::PermissionDecided { request }) => Json(request).into_response(),
        Ok(_) => {
            ApiError::Internal("unexpected Project ACP permission decision response".to_string())
                .into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn get_project_chat_metadata(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    let project_id = ProjectId::new(project_id);
    match state.project_catalog.get(&project_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    }
    let scope =
        ChatScopeStore::resolve(&state.rua_root, ChatAddress::project(project_id, &chat_id));
    crate::endpoints::chats::get_chat_metadata_core(scope.root(), &chat_id).await
}

async fn get_project_chat(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Query(params): Query<crate::endpoints::chats::ChatHistoryParams>,
) -> Response {
    let project_id = ProjectId::new(project_id);
    match state.project_catalog.get(&project_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    }
    if let Err(error) = crate::services::console_chat::resolve_chat_id(&chat_id) {
        return ApiError::BadRequest(error).into_response();
    }
    if let Some(owner) = find_project_chat_owner(&state, &chat_id).await {
        if owner != project_id {
            return ApiError::NotFound(format!(
                "Project Chat {chat_id} belongs to Project {owner}"
            ))
            .into_response();
        }
    }
    let address = ChatAddress::project(project_id, &chat_id);
    let scope_root = ChatScopeStore::resolve(&state.rua_root, address.clone())
        .root()
        .to_path_buf();
    crate::endpoints::chats::get_chat_core(&state, &address, &scope_root, &chat_id, params).await
}

async fn project_chat_cli(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<ProjectChatSubmitRequest>,
) -> Response {
    let prepared = match prepare_project_chat_submit(&state, project_id, chat_id, body).await {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };
    let address = ChatAddress::project(prepared.project_id, &prepared.chat_id);
    crate::endpoints::chat_cli::submit_single_turn(
        &state,
        address,
        &prepared.requested_agent_id,
        prepared.request,
    )
    .await
}

async fn submit_project_chat(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<ProjectChatSubmitRequest>,
) -> Response {
    let prepared = match prepare_project_chat_submit(&state, project_id, chat_id, body).await {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };
    let outcome = state
        .conversation_input_intake
        .submit_project_delivery(
            &prepared.project_id,
            &prepared.requested_agent_id,
            prepared.request,
        )
        .await;
    project_submit_response(outcome)
}

async fn prepare_project_chat_submit(
    state: &AppState,
    project_id: String,
    chat_id: String,
    body: ProjectChatSubmitRequest,
) -> Result<PreparedProjectChatSubmit, Response> {
    let ProjectChatSubmitRequest { agent_id, chat } = body;
    let mut chat = match serde_json::from_value::<dto::ChatRequest>(serde_json::Value::Object(chat))
    {
        Ok(chat) => chat,
        Err(error) => {
            return Err(ApiError::BadRequest(format!(
                "invalid Project Chat submit request: {error}"
            ))
            .into_response());
        }
    };
    let model_id = chat.prepare_model_for_console();

    let project_id = ProjectId::new(project_id);
    let project = match state.project_catalog.get(&project_id) {
        Ok(Some(project)) => project,
        Ok(None) => {
            return Err(
                ApiError::NotFound(format!("Project not found: {project_id}")).into_response(),
            );
        }
        Err(error) => return Err(ApiError::Internal(error.to_string()).into_response()),
    };
    if project.lifecycle != ProjectLifecycle::Active {
        return Err(
            ApiError::Conflict("Archived Project cannot accept new work".to_string())
                .into_response(),
        );
    }
    let chat_id = match crate::services::console_chat::resolve_chat_id(&chat_id) {
        Ok(chat_id) => chat_id,
        Err(error) => return Err(ApiError::BadRequest(error).into_response()),
    };
    if let Some(body_chat_id) = chat.effective_chat_id() {
        if body_chat_id != chat_id {
            return Err(ApiError::BadRequest(
                "Project Chat path and request body identify different Chats".to_string(),
            )
            .into_response());
        }
    }
    if agent_id.trim().is_empty() {
        return Err(ApiError::BadRequest("agent_id is required".to_string()).into_response());
    }
    if !chat.has_user_content() {
        return Err(ApiError::BadRequest("message is required".to_string()).into_response());
    }
    if let Some(owner) = find_project_chat_owner(state, &chat_id).await {
        if owner != project_id {
            return Err(ApiError::Conflict(format!(
                "Project Chat {chat_id} belongs to Project {owner}"
            ))
            .into_response());
        }
    }

    chat.chat_id = Some(chat_id.clone());
    let request = ConsoleChatRequest {
        message: chat.effective_message(),
        chat_id: chat.effective_chat_id(),
        model: model_id,
        is_reconnect: false,
        attachments: chat
            .attachments
            .iter()
            .map(|attachment| AttachmentInfo {
                url: attachment.url.clone(),
                name: attachment.name.clone(),
                mime_type: attachment.mime_type.clone(),
                stored_name: attachment.stored_name.clone(),
                size: attachment.size,
            })
            .collect(),
        input: chat.input,
        messages: chat.messages,
        extra: chat.extra,
        channel_kind: Some(rua_channel::core::renderer::ChannelKind::Chat),
    };
    Ok(PreparedProjectChatSubmit {
        project_id,
        chat_id,
        requested_agent_id: agent_id.trim().to_string(),
        request,
    })
}

async fn project_chat_live(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<ProjectChatLiveRequest>,
) -> Response {
    if body.viewer_id.trim().is_empty() {
        return ApiError::BadRequest("viewer_id is required".to_string()).into_response();
    }
    let project_id = ProjectId::new(project_id);
    match state.project_catalog.get(&project_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    }
    let chat_id = match crate::services::console_chat::resolve_chat_id(&chat_id) {
        Ok(chat_id) => chat_id,
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    let address = ChatAddress::project(project_id, &chat_id);
    let scope_root = ChatScopeStore::resolve(&state.rua_root, address.clone())
        .root()
        .to_path_buf();
    let execution_agent_id = match rua_session::WorkspaceSessionStore::new(&scope_root)
        .load_chat_header(&chat_id)
        .await
    {
        Ok(Some(header)) => match crate::services::project_chat::selection_from_header(&header) {
            Ok(selection) => selection.agent_id,
            Err(error) => return ApiError::Internal(error).into_response(),
        },
        Ok(None) => match body.agent_id.as_deref().map(str::trim) {
            Some(agent_id) if !agent_id.is_empty() => agent_id.to_string(),
            _ => {
                return ApiError::BadRequest(
                    "agent_id is required while Project Chat history is unavailable".to_string(),
                )
                .into_response();
            }
        },
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    let stream = state
        .console_facade
        .connect_scoped_chat_activity(
            address,
            &execution_agent_id,
            body.viewer_id.trim(),
            body.activity_position(),
        )
        .await;
    crate::endpoints::console::conversation_live_sse_response(stream)
}

async fn project_chat_activity(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    let project_id = ProjectId::new(project_id);
    match state.project_catalog.get(&project_id) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ApiError::NotFound(format!("Project not found: {project_id}")).into_response();
        }
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    }
    let chat_id = match crate::services::console_chat::resolve_chat_id(&chat_id) {
        Ok(chat_id) => chat_id,
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    let stream = crate::endpoints::console::scoped_chat_activity_stream(
        state,
        ChatAddress::project(project_id, chat_id),
    )
    .map(|result| result.map(|data| format!("{data}\n")));
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn switch_project_chat_agent(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Json(body): Json<ProjectChatAgentSwitchRequest>,
) -> Response {
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

fn project_submit_response(
    outcome: crate::services::conversation_input_intake::ConversationInputDeliveryOutcome,
) -> Response {
    use crate::services::conversation_input_intake::ConversationInputDeliveryOutcome;
    match outcome {
        ConversationInputDeliveryOutcome::Delivered {
            chat_id,
            transport,
            accepted,
            queued_to_active_authority,
            input_id,
            submission_id,
            detail,
            device_delivery_id,
            target_device_id,
            device_delivery_sequence,
            accepted_at,
            control,
        } => Json(serde_json::json!({
            "accepted": accepted,
            "mode": if queued_to_active_authority { "queued" } else { "started" },
            "materialized": true,
            "chat_id": chat_id,
            "input_id": input_id,
            "submission_id": submission_id,
            "detail": detail,
            "device_delivery_id": device_delivery_id,
            "target_device_id": target_device_id,
            "device_delivery_sequence": device_delivery_sequence,
            "accepted_at": accepted_at,
            "control": control,
            "transport": format!("{transport:?}"),
        }))
        .into_response(),
        ConversationInputDeliveryOutcome::Unconfirmed {
            chat_id,
            detail,
            submission_id,
            device_delivery_id,
            target_device_id,
            device_delivery_sequence,
        } => Json(serde_json::json!({
            "accepted": false,
            "mode": "unconfirmed",
            "chat_id": chat_id,
            "input_id": null,
            "submission_id": submission_id,
            "detail": detail,
            "device_delivery_id": device_delivery_id,
            "target_device_id": target_device_id,
            "device_delivery_sequence": device_delivery_sequence,
            "accepted_at": null,
            "failure_kind": "delivery_unknown",
            "control": null,
        }))
        .into_response(),
        ConversationInputDeliveryOutcome::Rejected {
            detail,
            target_device_id,
            failure_kind,
        } => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "accepted": false,
                "mode": "rejected",
                "materialized": false,
                "detail": detail,
                "target_device_id": target_device_id,
                "failure_kind": failure_kind,
            })),
        )
            .into_response(),
    }
}

async fn find_project_chat_owner(state: &AppState, chat_id: &str) -> Option<ProjectId> {
    let projects = state.project_catalog.list().ok()?;
    for project in projects {
        let address = ChatAddress::project(project.project_id.clone(), chat_id);
        let scope_root = ChatScopeStore::resolve(&state.rua_root, address)
            .root()
            .to_path_buf();
        if !scope_root.join("chat-index.sqlite").is_file() {
            continue;
        }
        if crate::services::chat_index::get(&scope_root, chat_id)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            return Some(project.project_id);
        }
    }
    None
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
