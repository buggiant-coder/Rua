use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::errors::ApiError;
use crate::services::chat_context::ChatContext;
use crate::services::chat_task::{ChatTaskAction, ChatTaskInvocation, ChatTaskResponse};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct TaskListQuery {
    state: Option<String>,
    limit: Option<usize>,
}
#[derive(Debug, Deserialize)]
struct SendTaskInputRequest {
    submission_id: String,
    input: rua_task::TaskInput,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chats/{chatId}/tasks", get(list_tasks))
        .route("/chats/{chatId}/tasks/activity", get(task_activity))
        .route("/chats/{chatId}/tasks/{task_id}", get(get_task))
        .route("/chats/{chatId}/tasks/{task_id}/cancel", post(cancel_task))
        .route(
            "/chats/{chatId}/tasks/{task_id}/input",
            post(send_task_input),
        )
        .route("/chats/{chatId}/tasks/{task_id}/watch", post(watch_task))
        .route(
            "/chats/{chatId}/tasks/{task_id}/unwatch",
            post(unwatch_task),
        )
        .route("/chats/{chatId}/tasks/{task_id}/retry", post(retry_task))
        .route(
            "/chats/{chatId}/tasks/{task_id}/result",
            get(get_task_result),
        )
}

async fn task_activity(
    State(state): State<AppState>,
    _chat: ChatContext,
    Path(chat_id): Path<String>,
) -> Response {
    let owner = rua_task::TaskOwner::new(chat_id);
    let receiver = state.task_runtime.subscribe_activity();
    let filter = rua_task::TaskListFilter {
        state: None,
        limit: Some(200),
        offset: 0,
    };
    let snapshot = match state.chat_task.list_records(&owner, filter.clone()).await {
        Ok(records) => records,
        Err(error) => return ApiError::Unavailable(error).into_response(),
    };
    let initial = Event::default()
        .event("snapshot")
        .json_data(json!({"type":"snapshot","tasks":snapshot}))
        .unwrap();
    let mut timer = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(2),
        Duration::from_secs(2),
    );
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let stream = futures::stream::unfold(
        (
            Some(initial),
            receiver,
            owner,
            state.chat_task,
            filter,
            timer,
        ),
        |(initial, mut receiver, owner, service, filter, mut timer)| async move {
            if let Some(initial) = initial {
                return Some((
                    Ok::<_, Infallible>(initial),
                    (None, receiver, owner, service, filter, timer),
                ));
            }
            loop {
                let frame = tokio::select! {
                    event = receiver.recv() => match event {
                        Ok(event) if event.owner == owner => Event::default().event("task").json_data(&event).unwrap(),
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => Event::default().event("resync_required").json_data(json!({"type":"resync_required","skipped":skipped})).unwrap(),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    },
                    _ = timer.tick() => match service.list_records(&owner,filter.clone()).await {
                        Ok(tasks) => Event::default().event("snapshot").json_data(json!({"type":"snapshot","tasks":tasks})).unwrap(),
                        Err(error) => Event::default().event("resync_required").json_data(json!({"type":"resync_required","detail":error})).unwrap(),
                    }
                };
                return Some((Ok(frame), (None, receiver, owner, service, filter, timer)));
            }
        },
    );
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response()
}

async fn list_tasks(
    State(state): State<AppState>,
    _chat: ChatContext,
    Path(chat_id): Path<String>,
    Query(query): Query<TaskListQuery>,
) -> Response {
    let state_filter = match query
        .state
        .map(|value| serde_json::from_value::<rua_task::TaskState>(json!(value)))
        .transpose()
    {
        Ok(state) => state,
        Err(error) => {
            return ApiError::BadRequest(format!("Invalid Task state: {error}")).into_response()
        }
    };
    match state
        .chat_task
        .list_records(
            &rua_task::TaskOwner::new(chat_id),
            rua_task::TaskListFilter {
                state: state_filter,
                limit: query.limit,
                offset: 0,
            },
        )
        .await
    {
        Ok(tasks) => Json(tasks).into_response(),
        Err(error) => ApiError::Unavailable(error).into_response(),
    }
}

async fn get_task(
    State(state): State<AppState>,
    _chat: ChatContext,
    Path((chat_id, task_id)): Path<(String, String)>,
) -> Response {
    route(
        &state,
        &chat_id,
        &task_id,
        ChatTaskAction::Get {
            task_id: task_id.clone(),
        },
    )
    .await
}
async fn get_task_result(
    State(state): State<AppState>,
    _chat: ChatContext,
    Path((chat_id, task_id)): Path<(String, String)>,
) -> Response {
    route(
        &state,
        &chat_id,
        &task_id,
        ChatTaskAction::Result {
            task_id: task_id.clone(),
        },
    )
    .await
}
async fn cancel_task(
    State(state): State<AppState>,
    _chat: ChatContext,
    Path((chat_id, task_id)): Path<(String, String)>,
) -> Response {
    route(
        &state,
        &chat_id,
        &task_id,
        ChatTaskAction::Cancel {
            task_id: task_id.clone(),
        },
    )
    .await
}
async fn watch_task(
    State(state): State<AppState>,
    _chat: ChatContext,
    Path((chat_id, task_id)): Path<(String, String)>,
) -> Response {
    route(
        &state,
        &chat_id,
        &task_id,
        ChatTaskAction::Watch {
            task_id: task_id.clone(),
        },
    )
    .await
}
async fn unwatch_task(
    State(state): State<AppState>,
    _chat: ChatContext,
    Path((chat_id, task_id)): Path<(String, String)>,
) -> Response {
    route(
        &state,
        &chat_id,
        &task_id,
        ChatTaskAction::Unwatch {
            task_id: task_id.clone(),
        },
    )
    .await
}
async fn send_task_input(
    State(state): State<AppState>,
    _chat: ChatContext,
    Path((chat_id, task_id)): Path<(String, String)>,
    Json(request): Json<SendTaskInputRequest>,
) -> Response {
    let submission = match rua_task::TaskInputSubmission::new(request.submission_id, request.input)
    {
        Ok(submission) => submission,
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    route(
        &state,
        &chat_id,
        &task_id,
        ChatTaskAction::Input {
            task_id: task_id.clone(),
            submission,
        },
    )
    .await
}
async fn route(state: &AppState, chat_id: &str, task_id: &str, action: ChatTaskAction) -> Response {
    response(
        state
            .chat_task
            .route_action(&rua_task::TaskOwner::new(chat_id), task_id, action)
            .await,
    )
}
fn response(reply: Result<ChatTaskResponse, String>) -> Response {
    match reply {
        Ok(ChatTaskResponse::Record { record, .. }) => Json(record).into_response(),
        Ok(ChatTaskResponse::Value { value }) => Json(value).into_response(),
        Ok(ChatTaskResponse::Rejected { reason, .. }) if reason == "task_not_found" => {
            ApiError::NotFound("Task not found".into()).into_response()
        }
        Ok(ChatTaskResponse::Rejected { reason, detail })
            if matches!(
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
        Ok(ChatTaskResponse::Rejected { detail, .. }) | Err(detail) => {
            ApiError::Unavailable(detail).into_response()
        }
        _ => ApiError::Internal("Unexpected Task response".into()).into_response(),
    }
}

async fn retry_task(
    State(state): State<AppState>,
    _chat: ChatContext,
    Path((chat_id, task_id)): Path<(String, String)>,
) -> Response {
    let owner = rua_task::TaskOwner::new(chat_id);
    let _admission = match state
        .conversation_input_intake
        .project_work_admission(&owner.chat_address)
        .await
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
    let scope = rua_core::ChatStore::resolve(&state.rua_root, owner.chat_address.clone());
    let header = match rua_session::WorkspaceSessionStore::new(scope.root())
        .load_chat_header(owner.chat_id())
        .await
    {
        Ok(Some(header)) => header,
        Ok(None) => return ApiError::NotFound("Chat not found".into()).into_response(),
        Err(error) => return ApiError::from(error).into_response(),
    };
    let authorization = match rua_task::TaskAttemptAuthorization::new(header.agent_id) {
        Ok(mut authorization) => {
            authorization.configuration_epoch = header
                .extra
                .get("hook_configuration")
                .and_then(|value| value.get("epoch"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            authorization
        }
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    let placement =
        match crate::services::conversation_authority_target::load_chat_authority_placement_target(
            scope.root(),
            owner.chat_id(),
        )
        .await
        {
            Ok(placement) => placement,
            Err(error) => return ApiError::Unavailable(error).into_response(),
        };
    let invocation = ChatTaskInvocation {
        authorization,
        authority_device_id: placement.device_id,
        authority_epoch: placement.authority_epoch,
        extra: Default::default(),
    };
    let preparation = match state
        .chat_task
        .route_action(
            &owner,
            &task_id,
            ChatTaskAction::PrepareRetry {
                task_id: task_id.clone(),
                invocation,
            },
        )
        .await
    {
        Ok(ChatTaskResponse::Prepared { preparation }) => preparation,
        other => return response(other),
    };
    let expected = preparation.expected_attempt_no;
    let expected_authorization = preparation.invocation.authorization.clone();
    let first = state
        .chat_task
        .route_action(&owner, &task_id, ChatTaskAction::Retry { preparation })
        .await;
    if first.is_ok() {
        return response(first);
    }
    // A lost acceptance receipt is not permission to launch another Attempt.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(ChatTaskResponse::Record { record, reference }) = state
            .chat_task
            .route_action(
                &owner,
                &task_id,
                ChatTaskAction::Get {
                    task_id: task_id.clone(),
                },
            )
            .await
        {
            if expected.is_some_and(|attempt| record.attempt_no == attempt.saturating_add(1))
                && record.attempts.iter().any(|attempt| {
                    attempt.attempt_no == record.attempt_no
                        && attempt.authorization_origin == expected_authorization.origin
                        && attempt.authorization_agent_id == expected_authorization.agent_id
                        && attempt.configuration_epoch == expected_authorization.configuration_epoch
                })
            {
                return response(Ok(ChatTaskResponse::Record { record, reference }));
            }
        }
    }
    ApiError::Unavailable(format!("Retry acceptance for Task {task_id} is unconfirmed; query the same Task ID before retrying again")).into_response()
}
