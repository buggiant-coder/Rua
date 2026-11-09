use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::errors::ApiError;
use crate::services::user_inbox::UserInboxError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct ListQuery {
    cursor: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
    read: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ReadStateRequest {
    read: bool,
}

#[derive(Debug, Serialize)]
struct UnreadCountResponse {
    count: usize,
}

#[derive(Debug, Serialize)]
struct MarkAllReadResponse {
    updated_count: usize,
}

fn default_limit() -> usize {
    20
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/inbox/messages", get(list))
        .route(
            "/inbox/messages/{id}",
            get(get_message).patch(set_read).delete(delete_message),
        )
        .route("/inbox/unread-count", get(unread_count))
        .route("/inbox/read-all", axum::routing::post(mark_all_read))
}

async fn list(State(state): State<AppState>, Query(query): Query<ListQuery>) -> Response {
    match state
        .user_inbox
        .list(query.cursor.as_deref(), query.limit, query.read)
        .await
    {
        Ok(page) => Json(page).into_response(),
        Err(error) => inbox_error(error),
    }
}

async fn get_message(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.user_inbox.get(&id).await {
        Some(message) => Json(message).into_response(),
        None => ApiError::NotFound("Inbox message not found".to_string()).into_response(),
    }
}

async fn set_read(
    State(state): State<AppState>,
    Path(id): Path<String>,
    payload: Result<Json<ReadStateRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(error) => return ApiError::BadRequest(error.body_text()).into_response(),
    };
    match state.user_inbox.set_read(&id, request.read).await {
        Ok(message) => Json(message).into_response(),
        Err(error) => inbox_error(error),
    }
}

async fn delete_message(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.user_inbox.delete(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => ApiError::NotFound("Inbox message not found".to_string()).into_response(),
        Err(error) => inbox_error(error),
    }
}

async fn unread_count(State(state): State<AppState>) -> Json<UnreadCountResponse> {
    Json(UnreadCountResponse {
        count: state.user_inbox.unread_count().await,
    })
}

async fn mark_all_read(State(state): State<AppState>) -> Response {
    match state.user_inbox.mark_all_read().await {
        Ok(updated_count) => Json(MarkAllReadResponse { updated_count }).into_response(),
        Err(error) => inbox_error(error),
    }
}

fn inbox_error(error: UserInboxError) -> Response {
    match error {
        UserInboxError::Validation(message) => ApiError::BadRequest(message).into_response(),
        UserInboxError::NotFound => {
            ApiError::NotFound("Inbox message not found".to_string()).into_response()
        }
        UserInboxError::Persistence(error) => ApiError::Internal(error.to_string()).into_response(),
    }
}
