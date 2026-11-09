use std::convert::Infallible;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::Stream;
use serde::Deserialize;

use crate::errors::ApiError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct TaskListQuery {
    state: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RetryProjectTaskRequest {
    authorization_agent_id: String,
}

#[derive(Debug, Deserialize)]
struct SendTaskInputRequest {
    submission_id: String,
    input: rua_task::TaskInput,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent_id}/chats/{chat_id}/tasks", get(list_tasks))
        .route(
            "/agents/{agent_id}/chats/{chat_id}/tasks/activity",
            get(task_activity),
        )
        .route(
            "/agents/{agent_id}/chats/{chat_id}/tasks/{task_id}",
            get(get_task),
        )
        .route(
            "/agents/{agent_id}/chats/{chat_id}/tasks/{task_id}/cancel",
            post(cancel_task),
        )
        .route(
            "/agents/{agent_id}/chats/{chat_id}/tasks/{task_id}/input",
            post(send_task_input),
        )
        .route(
            "/agents/{agent_id}/chats/{chat_id}/tasks/{task_id}/watch",
            post(watch_task),
        )
        .route(
            "/agents/{agent_id}/chats/{chat_id}/tasks/{task_id}/unwatch",
            post(unwatch_task),
        )
        .route(
            "/agents/{agent_id}/chats/{chat_id}/tasks/{task_id}/retry",
            post(retry_task),
        )
        .route(
            "/agents/{agent_id}/chats/{chat_id}/tasks/{task_id}/result",
            get(get_task_result),
        )
        .route(
            "/projects/{project_id}/chats/{chat_id}/tasks",
            get(list_project_tasks),
        )
        .route(
            "/projects/{project_id}/chats/{chat_id}/tasks/activity",
            get(project_task_activity),
        )
        .route(
            "/projects/{project_id}/chats/{chat_id}/tasks/{task_id}",
            get(get_project_task),
        )
        .route(
            "/projects/{project_id}/chats/{chat_id}/tasks/{task_id}/cancel",
            post(cancel_project_task),
        )
        .route(
            "/projects/{project_id}/chats/{chat_id}/tasks/{task_id}/input",
            post(send_project_task_input),
        )
        .route(
            "/projects/{project_id}/chats/{chat_id}/tasks/{task_id}/watch",
            post(watch_project_task),
        )
        .route(
            "/projects/{project_id}/chats/{chat_id}/tasks/{task_id}/unwatch",
            post(unwatch_project_task),
        )
        .route(
            "/projects/{project_id}/chats/{chat_id}/tasks/{task_id}/retry",
            post(retry_project_task),
        )
        .route(
            "/projects/{project_id}/chats/{chat_id}/tasks/{task_id}/result",
            get(get_project_task_result),
        )
}

async fn task_activity(
    State(state): State<AppState>,
    Path((agent_id, chat_id)): Path<(String, String)>,
) -> Response {
    let owner = rua_task::TaskOwner::new(agent_id, chat_id);
    task_activity_for_owner(state, owner).await
}

async fn project_task_activity(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
) -> Response {
    let action = crate::services::device_command_bus::ProjectDeviceAction::ListTasks {
        project_id: rua_core::ProjectId::new(&project_id),
        chat_id: chat_id.clone(),
        filter: rua_task::TaskListFilter {
            state: None,
            limit: Some(200),
            offset: 0,
        },
    };
    match remote_project_task_response(&state, &project_id, action).await {
        Ok(Some(crate::services::device_command_bus::ProjectDeviceResponse::TaskList {
            tasks,
        })) => {
            let initial = Event::default()
                .event("snapshot")
                .json_data(serde_json::json!({
                    "type": "snapshot",
                    "tasks": tasks,
                }))
                .expect("remote Task snapshot is serializable");
            return Sse::new(futures::stream::once(async move {
                Ok::<_, Infallible>(initial)
            }))
            .into_response();
        }
        Ok(Some(_)) => {
            return ApiError::Internal(
                "resource Device returned an unexpected Task response".to_string(),
            )
            .into_response();
        }
        Err(response) => return response,
        Ok(None) => {}
    }
    let owner = project_task_owner(project_id, chat_id);
    task_activity_for_owner(state, owner).await
}

async fn task_activity_for_owner(state: AppState, owner: rua_task::TaskOwner) -> Response {
    let receiver = state.task_runtime.subscribe_activity();
    let snapshot = match state.task_runtime.activity_snapshot(&owner).await {
        Ok(records) => records,
        Err(error) => return task_error_response(error),
    };
    let initial = Event::default()
        .event("snapshot")
        .json_data(serde_json::json!({
            "type": "snapshot",
            "tasks": snapshot,
        }))
        .expect("Task snapshot is serializable");
    let stream = task_activity_stream(owner, receiver, initial);
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response()
}

fn task_activity_stream(
    owner: rua_task::TaskOwner,
    receiver: tokio::sync::broadcast::Receiver<rua_task::TaskEvent>,
    initial: Event,
) -> impl Stream<Item = Result<Event, Infallible>> {
    futures::stream::unfold(
        (Some(initial), receiver, owner),
        |(initial, mut receiver, owner)| async move {
            if let Some(initial) = initial {
                return Some((Ok(initial), (None, receiver, owner)));
            }
            loop {
                match receiver.recv().await {
                    Ok(event) if event.owner == owner => {
                        let frame = Event::default()
                            .event("task")
                            .json_data(&event)
                            .expect("Task event is serializable");
                        return Some((Ok(frame), (None, receiver, owner)));
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        let frame = Event::default()
                            .event("resync_required")
                            .json_data(serde_json::json!({
                                "type": "resync_required",
                                "skipped": skipped,
                            }))
                            .expect("resync event is serializable");
                        return Some((Ok(frame), (None, receiver, owner)));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        },
    )
}

async fn list_tasks(
    State(state): State<AppState>,
    Path((agent_id, chat_id)): Path<(String, String)>,
    Query(query): Query<TaskListQuery>,
) -> Response {
    list_tasks_for_owner(state, rua_task::TaskOwner::new(agent_id, chat_id), query).await
}

async fn list_project_tasks(
    State(state): State<AppState>,
    Path((project_id, chat_id)): Path<(String, String)>,
    Query(query): Query<TaskListQuery>,
) -> Response {
    let task_state = match parse_task_state(query.state) {
        Ok(state) => state,
        Err(error) => return error.into_response(),
    };
    let filter = rua_task::TaskListFilter {
        state: task_state,
        limit: query.limit,
        offset: 0,
    };
    let action = crate::services::device_command_bus::ProjectDeviceAction::ListTasks {
        project_id: rua_core::ProjectId::new(&project_id),
        chat_id: chat_id.clone(),
        filter: filter.clone(),
    };
    match remote_project_task_response(&state, &project_id, action).await {
        Ok(Some(crate::services::device_command_bus::ProjectDeviceResponse::TaskList {
            tasks,
        })) => return Json(tasks).into_response(),
        Ok(Some(_)) => {
            return ApiError::Internal(
                "resource Device returned an unexpected Task response".to_string(),
            )
            .into_response();
        }
        Err(response) => return response,
        Ok(None) => {}
    }
    list_tasks_with_filter_for_owner(state, project_task_owner(project_id, chat_id), filter).await
}

async fn list_tasks_for_owner(
    state: AppState,
    owner: rua_task::TaskOwner,
    query: TaskListQuery,
) -> Response {
    let task_state = match parse_task_state(query.state) {
        Ok(state) => state,
        Err(error) => return error.into_response(),
    };
    let filter = rua_task::TaskListFilter {
        state: task_state,
        limit: query.limit,
        offset: 0,
    };
    list_tasks_with_filter_for_owner(state, owner, filter).await
}

fn parse_task_state(state: Option<String>) -> Result<Option<rua_task::TaskState>, ApiError> {
    match state {
        Some(value) => {
            serde_json::from_value::<rua_task::TaskState>(serde_json::Value::String(value.clone()))
                .map(Some)
                .map_err(|_| ApiError::BadRequest(format!("unknown Task state '{value}'")))
        }
        None => Ok(None),
    }
}

async fn list_tasks_with_filter_for_owner(
    state: AppState,
    owner: rua_task::TaskOwner,
    filter: rua_task::TaskListFilter,
) -> Response {
    match state.task_runtime.list(&owner, filter).await {
        Ok(records) => (StatusCode::OK, Json(records)).into_response(),
        Err(error) => task_error_response(error),
    }
}

async fn get_task(
    State(state): State<AppState>,
    Path((agent_id, chat_id, task_id)): Path<(String, String, String)>,
) -> Response {
    get_task_for_owner(state, rua_task::TaskOwner::new(agent_id, chat_id), task_id).await
}

async fn get_project_task(
    State(state): State<AppState>,
    Path((project_id, chat_id, task_id)): Path<(String, String, String)>,
) -> Response {
    let action = crate::services::device_command_bus::ProjectDeviceAction::GetTask {
        project_id: rua_core::ProjectId::new(&project_id),
        chat_id: chat_id.clone(),
        task_id: task_id.clone(),
    };
    match remote_project_task_response(&state, &project_id, action).await {
        Ok(Some(crate::services::device_command_bus::ProjectDeviceResponse::Task { task })) => {
            return Json(task).into_response();
        }
        Ok(Some(_)) => {
            return ApiError::Internal(
                "resource Device returned an unexpected Task response".to_string(),
            )
            .into_response();
        }
        Err(response) => return response,
        Ok(None) => {}
    }
    get_task_for_owner(state, project_task_owner(project_id, chat_id), task_id).await
}

async fn get_task_for_owner(
    state: AppState,
    owner: rua_task::TaskOwner,
    task_id: String,
) -> Response {
    match state.task_runtime.get(&owner, &task_id).await {
        Ok(record) => (StatusCode::OK, Json(record)).into_response(),
        Err(error) => task_error_response(error),
    }
}

async fn get_task_result(
    State(state): State<AppState>,
    Path((agent_id, chat_id, task_id)): Path<(String, String, String)>,
) -> Response {
    get_task_result_for_owner(state, rua_task::TaskOwner::new(agent_id, chat_id), task_id).await
}

async fn get_project_task_result(
    State(state): State<AppState>,
    Path((project_id, chat_id, task_id)): Path<(String, String, String)>,
) -> Response {
    let action = crate::services::device_command_bus::ProjectDeviceAction::GetTaskResult {
        project_id: rua_core::ProjectId::new(&project_id),
        chat_id: chat_id.clone(),
        task_id: task_id.clone(),
    };
    match remote_project_task_response(&state, &project_id, action).await {
        Ok(Some(crate::services::device_command_bus::ProjectDeviceResponse::TaskResult {
            result,
        })) => return Json(result).into_response(),
        Ok(Some(_)) => {
            return ApiError::Internal(
                "resource Device returned an unexpected Task response".to_string(),
            )
            .into_response();
        }
        Err(response) => return response,
        Ok(None) => {}
    }
    get_task_result_for_owner(state, project_task_owner(project_id, chat_id), task_id).await
}

async fn get_task_result_for_owner(
    state: AppState,
    owner: rua_task::TaskOwner,
    task_id: String,
) -> Response {
    match state.task_runtime.read_full_result(&owner, &task_id).await {
        Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(value) => (StatusCode::OK, Json(value)).into_response(),
            Err(error) => {
                ApiError::Internal(format!("stored Task result is not valid JSON: {error}"))
                    .into_response()
            }
        },
        Err(error) => task_error_response(error),
    }
}

async fn cancel_task(
    State(state): State<AppState>,
    Path((agent_id, chat_id, task_id)): Path<(String, String, String)>,
) -> Response {
    cancel_task_for_owner(state, rua_task::TaskOwner::new(agent_id, chat_id), task_id).await
}

async fn send_task_input(
    State(state): State<AppState>,
    Path((agent_id, chat_id, task_id)): Path<(String, String, String)>,
    Json(request): Json<SendTaskInputRequest>,
) -> Response {
    let submission = match rua_task::TaskInputSubmission::new(request.submission_id, request.input)
    {
        Ok(submission) => submission,
        Err(message) => return ApiError::BadRequest(message).into_response(),
    };
    send_task_input_for_owner(
        state,
        rua_task::TaskOwner::new(agent_id, chat_id),
        task_id,
        submission,
    )
    .await
}

async fn send_project_task_input(
    State(state): State<AppState>,
    Path((project_id, chat_id, task_id)): Path<(String, String, String)>,
    Json(request): Json<SendTaskInputRequest>,
) -> Response {
    let submission = match rua_task::TaskInputSubmission::new(request.submission_id, request.input)
    {
        Ok(submission) => submission,
        Err(message) => return ApiError::BadRequest(message).into_response(),
    };
    let action = crate::services::device_command_bus::ProjectDeviceAction::SendTaskInput {
        project_id: rua_core::ProjectId::new(&project_id),
        chat_id: chat_id.clone(),
        task_id: task_id.clone(),
        submission: submission.clone(),
    };
    match remote_project_task_response(&state, &project_id, action).await {
        Ok(Some(crate::services::device_command_bus::ProjectDeviceResponse::TaskInput {
            acceptance,
        })) => return Json(acceptance).into_response(),
        Ok(Some(_)) => {
            return ApiError::Internal(
                "resource Device returned an unexpected Task response".to_string(),
            )
            .into_response();
        }
        Err(response) => return response,
        Ok(None) => {}
    }
    send_task_input_for_owner(
        state,
        project_task_owner(project_id, chat_id),
        task_id,
        submission,
    )
    .await
}

async fn send_task_input_for_owner(
    state: AppState,
    owner: rua_task::TaskOwner,
    task_id: String,
    submission: rua_task::TaskInputSubmission,
) -> Response {
    match state
        .task_runtime
        .send_input(&owner, &task_id, submission)
        .await
    {
        Ok(acceptance) => (StatusCode::OK, Json(acceptance)).into_response(),
        Err(error) => task_error_response(error),
    }
}

async fn watch_task(
    State(state): State<AppState>,
    Path((agent_id, chat_id, task_id)): Path<(String, String, String)>,
) -> Response {
    watch_task_for_owner(
        state,
        rua_task::TaskOwner::new(agent_id, chat_id),
        task_id,
        true,
    )
    .await
}

async fn unwatch_task(
    State(state): State<AppState>,
    Path((agent_id, chat_id, task_id)): Path<(String, String, String)>,
) -> Response {
    watch_task_for_owner(
        state,
        rua_task::TaskOwner::new(agent_id, chat_id),
        task_id,
        false,
    )
    .await
}

async fn watch_project_task(
    State(state): State<AppState>,
    Path((project_id, chat_id, task_id)): Path<(String, String, String)>,
) -> Response {
    watch_project_task_for_owner(state, project_id, chat_id, task_id, true).await
}

async fn unwatch_project_task(
    State(state): State<AppState>,
    Path((project_id, chat_id, task_id)): Path<(String, String, String)>,
) -> Response {
    watch_project_task_for_owner(state, project_id, chat_id, task_id, false).await
}

async fn watch_project_task_for_owner(
    state: AppState,
    project_id: String,
    chat_id: String,
    task_id: String,
    watch: bool,
) -> Response {
    let action = if watch {
        crate::services::device_command_bus::ProjectDeviceAction::WatchTask {
            project_id: rua_core::ProjectId::new(&project_id),
            chat_id: chat_id.clone(),
            task_id: task_id.clone(),
        }
    } else {
        crate::services::device_command_bus::ProjectDeviceAction::UnwatchTask {
            project_id: rua_core::ProjectId::new(&project_id),
            chat_id: chat_id.clone(),
            task_id: task_id.clone(),
        }
    };
    match remote_project_task_response(&state, &project_id, action).await {
        Ok(Some(crate::services::device_command_bus::ProjectDeviceResponse::TaskWatch {
            status,
        })) => return Json(status).into_response(),
        Ok(Some(_)) => {
            return ApiError::Internal(
                "resource Device returned an unexpected Task response".to_string(),
            )
            .into_response();
        }
        Err(response) => return response,
        Ok(None) => {}
    }
    watch_task_for_owner(
        state,
        project_task_owner(project_id, chat_id),
        task_id,
        watch,
    )
    .await
}

async fn watch_task_for_owner(
    state: AppState,
    owner: rua_task::TaskOwner,
    task_id: String,
    watch: bool,
) -> Response {
    let result = if watch {
        state.task_runtime.watch(&owner, &task_id).await
    } else {
        state.task_runtime.unwatch(&owner, &task_id).await
    };
    match result {
        Ok(status) => (StatusCode::OK, Json(status)).into_response(),
        Err(error) => task_error_response(error),
    }
}

async fn cancel_project_task(
    State(state): State<AppState>,
    Path((project_id, chat_id, task_id)): Path<(String, String, String)>,
) -> Response {
    let action = crate::services::device_command_bus::ProjectDeviceAction::CancelTask {
        project_id: rua_core::ProjectId::new(&project_id),
        chat_id: chat_id.clone(),
        task_id: task_id.clone(),
    };
    match remote_project_task_response(&state, &project_id, action).await {
        Ok(Some(crate::services::device_command_bus::ProjectDeviceResponse::Task { task })) => {
            return Json(task).into_response();
        }
        Ok(Some(_)) => {
            return ApiError::Internal(
                "resource Device returned an unexpected Task response".to_string(),
            )
            .into_response();
        }
        Err(response) => return response,
        Ok(None) => {}
    }
    cancel_task_for_owner(state, project_task_owner(project_id, chat_id), task_id).await
}

async fn cancel_task_for_owner(
    state: AppState,
    owner: rua_task::TaskOwner,
    task_id: String,
) -> Response {
    match state.task_runtime.cancel(&owner, &task_id).await {
        Ok(record) => (StatusCode::OK, Json(record)).into_response(),
        Err(error) => task_error_response(error),
    }
}

async fn retry_task(
    State(state): State<AppState>,
    Path((agent_id, chat_id, task_id)): Path<(String, String, String)>,
) -> Response {
    let authorization = match rua_task::TaskAttemptAuthorization::new(agent_id.clone()) {
        Ok(authorization) => authorization,
        Err(message) => return ApiError::BadRequest(message).into_response(),
    };
    retry_task_for_owner(
        state,
        rua_task::TaskOwner::new(agent_id, chat_id),
        task_id,
        authorization,
    )
    .await
}

async fn retry_project_task(
    State(state): State<AppState>,
    Path((project_id, chat_id, task_id)): Path<(String, String, String)>,
    Json(request): Json<RetryProjectTaskRequest>,
) -> Response {
    let authorization =
        match rua_task::TaskAttemptAuthorization::new(request.authorization_agent_id.clone()) {
            Ok(authorization) => authorization,
            Err(message) => return ApiError::BadRequest(message).into_response(),
        };
    let action = crate::services::device_command_bus::ProjectDeviceAction::RetryTask {
        project_id: rua_core::ProjectId::new(&project_id),
        chat_id: chat_id.clone(),
        task_id: task_id.clone(),
        authorization_agent_id: request.authorization_agent_id,
    };
    match remote_project_task_response(&state, &project_id, action).await {
        Ok(Some(crate::services::device_command_bus::ProjectDeviceResponse::Task { task })) => {
            return Json(task).into_response();
        }
        Ok(Some(_)) => {
            return ApiError::Internal(
                "resource Device returned an unexpected Task response".to_string(),
            )
            .into_response();
        }
        Err(response) => return response,
        Ok(None) => {}
    }
    retry_task_for_owner(
        state,
        project_task_owner(project_id, chat_id),
        task_id,
        authorization,
    )
    .await
}

async fn retry_task_for_owner(
    state: AppState,
    owner: rua_task::TaskOwner,
    task_id: String,
    mut authorization: rua_task::TaskAttemptAuthorization,
) -> Response {
    let _resource_admission = match state
        .conversation_input_intake
        .project_work_admission(&owner.chat_address)
    {
        Ok(guard) => guard,
        Err(error) => return ApiError::Conflict(error).into_response(),
    };
    if let Err(error) = state
        .collaboration
        .ensure_configuration_ready(&owner.chat_address)
        .await
    {
        return ApiError::Conflict(error).into_response();
    }
    let scope = rua_core::ChatScopeStore::resolve(&state.rua_root, owner.chat_address.clone());
    match rua_session::WorkspaceSessionStore::new(scope.root())
        .load_chat_header(owner.chat_id())
        .await
    {
        Ok(Some(header)) => {
            authorization.configuration_epoch = header
                .extra
                .get("hook_configuration")
                .and_then(|configuration| configuration.get("epoch"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }
        Ok(None) => {}
        Err(error) => return ApiError::from(error).into_response(),
    }
    match state
        .task_runtime
        .retry(&owner, &task_id, authorization)
        .await
    {
        Ok(record) => (StatusCode::OK, Json(record)).into_response(),
        Err(error) => task_error_response(error),
    }
}

fn project_task_owner(project_id: String, chat_id: String) -> rua_task::TaskOwner {
    rua_task::TaskOwner::from_address(rua_core::ChatAddress::project(project_id, chat_id))
}

async fn remote_project_task_response(
    state: &AppState,
    project_id: &str,
    action: crate::services::device_command_bus::ProjectDeviceAction,
) -> Result<Option<crate::services::device_command_bus::ProjectDeviceResponse>, Response> {
    let project_id = rua_core::ProjectId::new(project_id);
    let project = match state.project_catalog.get(&project_id) {
        Ok(Some(project)) => project,
        Ok(None) => {
            return Err(
                ApiError::NotFound(format!("Project not found: {project_id}")).into_response(),
            );
        }
        Err(error) => return Err(ApiError::Internal(error.to_string()).into_response()),
    };
    let source_device_id =
        match crate::services::device_context::current_sync_device_id(&state.sync_device) {
            Some(device_id) => device_id,
            None => {
                return Err(ApiError::Unavailable(
                    "local Device identity is unavailable".to_string(),
                )
                .into_response());
            }
        };
    if project.resource_device_id == source_device_id {
        return Ok(None);
    }
    let session_manager = match crate::services::device_session_access::current_session_manager(
        &state.device_network,
    )
    .await
    {
        Ok(manager) => manager,
        Err(error) => return Err(ApiError::Unavailable(error).into_response()),
    };
    let command = crate::services::device_command_bus::ProjectDeviceCommand {
        command_id: format!("project-task-{}", uuid::Uuid::new_v4()),
        source_device_id,
        target_device_id: project.resource_device_id,
        action,
    };
    crate::services::device_command_bus::deliver_project_device_command_via_session(
        &session_manager,
        &command,
        std::time::Duration::from_secs(10),
    )
    .await
    .map(Some)
    .map_err(remote_task_error_response)
}

fn remote_task_error_response(
    error: crate::services::device_command_bus::DeviceCommandDeliveryError,
) -> Response {
    match error {
        crate::services::device_command_bus::DeviceCommandDeliveryError::Rejected {
            reason,
            detail,
        } if reason == "task_not_found" => ApiError::NotFound(detail).into_response(),
        crate::services::device_command_bus::DeviceCommandDeliveryError::Rejected {
            reason,
            detail,
        } if matches!(
            reason.as_str(),
            "task_invalid"
                | "task_invalid_transition"
                | "task_limit_exceeded"
                | "task_authority_rejected"
                | "task_input_unavailable"
                | "task_input_conflict"
        ) =>
        {
            ApiError::Conflict(detail).into_response()
        }
        other => ApiError::Unavailable(other.to_string()).into_response(),
    }
}

fn task_error_response(error: rua_task::TaskError) -> Response {
    match error {
        rua_task::TaskError::NotFound(_) => {
            ApiError::NotFound("Task not found".to_string()).into_response()
        }
        rua_task::TaskError::Invalid(message) => ApiError::Conflict(message).into_response(),
        rua_task::TaskError::InvalidTransition(message) => {
            ApiError::Conflict(message).into_response()
        }
        rua_task::TaskError::LimitExceeded(message) => ApiError::Conflict(message).into_response(),
        rua_task::TaskError::AuthorityRejected(message) => {
            ApiError::Conflict(message).into_response()
        }
        rua_task::TaskError::InputUnavailable(message)
        | rua_task::TaskError::InputConflict(message) => {
            ApiError::Conflict(message).into_response()
        }
        rua_task::TaskError::AuthorityUnavailable(message)
        | rua_task::TaskError::ResourceUnavailable(message) => {
            ApiError::Unavailable(message).into_response()
        }
        rua_task::TaskError::WaitTimedOut(task_id) => {
            ApiError::Unavailable(format!("timed out waiting for Task {task_id}")).into_response()
        }
        other => ApiError::Internal(other.to_string()).into_response(),
    }
}
