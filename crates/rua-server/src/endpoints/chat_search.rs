use std::path::Path;
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rua_workspace::WorkspaceRuntime;
use serde::Deserialize;

use crate::endpoints::dto::{ChatSearchResponse, ChatSearchResult};
use crate::errors::ApiError;
use crate::extractors::AgentExtractorContext;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct SearchParams {
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

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new().route("/agents/{agentId}/chats/search", get(search_agent))
}

async fn search_root(
    AgentExtractorContext { agent_id, runtime }: AgentExtractorContext,
    Query(params): Query<SearchParams>,
) -> Response {
    search_core(&agent_id, &runtime.workspace_dir(), params).await
}

async fn search_agent(
    State(state): State<AppState>,
    AxumPath(agent_id): AxumPath<String>,
    Query(params): Query<SearchParams>,
) -> Response {
    let runtime = match runtime_for_agent(&state, &agent_id).await {
        Ok(rt) => rt,
        Err(e) => return e.into_response(),
    };
    search_core(&agent_id, &runtime.workspace_dir(), params).await
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

async fn search_core(agent_id: &str, ws_dir: &Path, params: SearchParams) -> Response {
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

    let address = rua_core::ChatAddress::agent(agent_id, "00000000-0000-4000-8000-000000000000");
    let saved = match rua_chat::ChatService::for_resolved_scope(ws_dir, address.clone())
        .search(
            &address,
            rua_chat::ChatSearchQuery {
                query,
                offset,
                limit,
            },
        )
        .await
    {
        Ok(result) => result,
        Err(e) => {
            return ApiError::Internal(format!(
                "Failed to search saved conversations for agent {agent_id}: {e}"
            ))
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
