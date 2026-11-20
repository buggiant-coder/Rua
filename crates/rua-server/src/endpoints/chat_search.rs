use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::endpoints::dto::{ChatSearchResponse, ChatSearchResult};
use crate::errors::ApiError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct SearchParams {
    agent_id: Option<String>,
    project_id: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    offset: Option<u64>,
}

pub fn root_router() -> Router<AppState> {
    Router::new().route("/chats/search", get(search_root))
}

async fn search_root(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Response {
    let query = params.q.as_deref().unwrap_or("").trim().to_string();
    let offset = params.offset.unwrap_or(0) as usize;
    let limit = params.limit.unwrap_or(50).clamp(1, 200) as usize;

    if query.is_empty() {
        let resp = ChatSearchResponse {
            results: vec![],
            offset,
            limit,
            total: 0,
            next_offset: None,
            extra: serde_json::Map::new(),
        };
        return (StatusCode::OK, Json(resp)).into_response();
    }

    let saved = match rua_chat::ChatService::new(&state.rua_root)
        .search(rua_chat::ChatSearchQuery {
            agent_id: params.agent_id,
            project_id: params.project_id,
            query,
            offset,
            limit,
        })
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
        .filter(|hit| hit.message.role == "user" || hit.message.role == "assistant")
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
