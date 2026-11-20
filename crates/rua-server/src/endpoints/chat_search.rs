use super::chats::ListChatsParams;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::endpoints::dto::{ChatSearchResponse, ChatSearchResult};
use crate::errors::ApiError;
use crate::state::AppState;

pub fn root_router() -> Router<AppState> {
    Router::new().route("/chats/search", get(search_root))
}

async fn search_root(
    State(state): State<AppState>,
    Query(params): Query<ListChatsParams>,
) -> Response {
    search_chats(&state, params, None).await
}

pub(super) async fn search_chats(
    state: &AppState,
    mut params: ListChatsParams,
    route_project: Option<&str>,
) -> Response {
    let query = params.q.take().unwrap_or_default();
    let chats = match params.into_query(route_project) {
        Ok(query) => query,
        Err(error) => return error.into_response(),
    };
    let saved = match rua_chat::ChatService::new(&state.rua_root)
        .search(rua_chat::ChatSearchQuery { query, chats })
        .await
    {
        Ok(result) => result,
        Err(e) => {
            return ApiError::Internal(format!("Failed to search saved conversations: {e}"))
                .into_response();
        }
    };
    let results: Vec<ChatSearchResult> = saved
        .results
        .into_iter()
        .map(|hit| ChatSearchResult {
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

    let resp = ChatSearchResponse {
        results,
        offset: saved.offset,
        limit: saved.limit,
        total: saved.total,
        next_offset: saved.next_offset,
        extra: serde_json::Map::new(),
    };
    (StatusCode::OK, Json(resp)).into_response()
}
