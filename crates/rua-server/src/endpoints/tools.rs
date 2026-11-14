use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use rua_core::RuaError;
use rua_tool::ToolConfigError;

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::state::AppState;
use crate::sync_publication::publish_agent_config_file_committed;

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agentId}/tools", get(list_tools_agent))
        .route(
            "/agents/{agentId}/tools/selection-policy",
            get(get_selection_policy_agent).put(set_selection_policy_agent),
        )
        .route(
            "/agents/{agentId}/tools/selection-policy/preview",
            post(preview_selection_policy_agent),
        )
        .route(
            "/agents/{agentId}/tools/{tool_name}/toggle",
            patch(toggle_tool_agent),
        )
        .route(
            "/agents/{agentId}/tools/{tool_name}/async-execution",
            patch(set_async_execution_agent),
        )
        .route(
            "/agents/{agentId}/tools/{tool_name}/config",
            get(get_tool_config_agent).put(set_tool_config_agent),
        )
}

#[derive(serde::Deserialize)]
struct SelectionPolicyPreviewRequest {
    selection_policy: rua_config::types::tools::ToolSelectionPolicy,
}

#[derive(serde::Deserialize)]
struct SelectionPolicyApplyRequest {
    expected: Option<serde_json::Value>,
    candidate: rua_config::types::tools::ToolSelectionPolicy,
}

async fn get_selection_policy_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    if let Err(error) = require_registered_agent(&state, &agent_id).await {
        return error.into_response();
    }
    match state
        .tool_config_store
        .selection_policy(Some(&agent_id))
        .await
    {
        Ok(policy) => Json(serde_json::json!({"selection_policy": policy})).into_response(),
        Err(error) => map_policy_error(error).into_response(),
    }
}

async fn preview_selection_policy_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> Response {
    if let Err(error) = require_registered_agent(&state, &agent_id).await {
        return error.into_response();
    }
    let body: SelectionPolicyPreviewRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
    };
    match state
        .tool_config_store
        .preview_selection_policy(Some(&agent_id), body.selection_policy)
        .await
    {
        Ok(preview) => Json(preview).into_response(),
        Err(error) => map_policy_error(error).into_response(),
    }
}

async fn set_selection_policy_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> Response {
    if let Err(error) = require_registered_agent(&state, &agent_id).await {
        return error.into_response();
    }
    let body: SelectionPolicyApplyRequest = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
    };
    match state
        .tool_config_store
        .set_selection_policy(Some(&agent_id), body.expected, body.candidate)
        .await
    {
        Ok(policy) => {
            publish_agent_config_after_tools_write(&state, &agent_id).await;
            Json(serde_json::json!({"selection_policy": policy})).into_response()
        }
        Err(error) => map_policy_error(error).into_response(),
    }
}

async fn require_registered_agent(state: &AppState, agent_id: &str) -> Result<(), ApiError> {
    state
        .agent_service
        .get_agent_status(agent_id)
        .await
        .map_err(ApiError::Core)?
        .ok_or_else(|| ApiError::NotFound(format!("Agent '{agent_id}' not found")))?;
    Ok(())
}

pub(crate) fn map_policy_error(error: ToolConfigError) -> ApiError {
    match error {
        ToolConfigError::Conflict(message) => ApiError::Conflict(message),
        ToolConfigError::Config(message) => ApiError::BadRequest(message),
        other => map_store_error(other),
    }
}

pub(crate) fn map_store_error(err: ToolConfigError) -> ApiError {
    match err {
        ToolConfigError::NotFound(name) => ApiError::Core(RuaError::NotFound {
            resource: "tool".into(),
            id: name,
        }),
        ToolConfigError::Conflict(message) => ApiError::Conflict(message),
        other => ApiError::Internal(other.to_string()),
    }
}

fn parse_async_execution_body(body: Bytes) -> Result<dto::ToolAsyncExecutionRequest, ApiError> {
    if body.is_empty() {
        return Err(ApiError::BadRequest(
            "missing JSON body with async_execution".into(),
        ));
    }
    serde_json::from_slice(&body).map_err(|e| ApiError::BadRequest(e.to_string()))
}

async fn publish_agent_config_after_tools_write(state: &AppState, agent_id: &str) {
    let path = state
        .rua_root
        .join("workspaces")
        .join(agent_id)
        .join("agent.json");
    publish_agent_config_file_committed(state, agent_id, &path, "tools agent config").await;
}

async fn list_tools_agent(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    let store = &state.tool_config_store;
    match store.list_tools(Some(&agent_id)).await {
        Ok(tools) => {
            let wire: Vec<dto::ToolInfo> = tools.into_iter().map(Into::into).collect();
            (StatusCode::OK, Json(wire)).into_response()
        }
        Err(e) => map_policy_error(e).into_response(),
    }
}

async fn toggle_tool_agent(
    State(state): State<AppState>,
    Path((agent_id, tool_name)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let store = &state.tool_config_store;
    let toggle_req: Option<dto::ToolToggleRequest> = if body.is_empty() {
        None
    } else {
        match serde_json::from_slice(&body) {
            Ok(r) => Some(r),
            Err(e) => return ApiError::BadRequest(e.to_string()).into_response(),
        }
    };
    let new_enabled = if let Some(req) = toggle_req {
        req.enabled
    } else {
        let tools = match store.list_tools(Some(&agent_id)).await {
            Ok(t) => t,
            Err(e) => return map_policy_error(e).into_response(),
        };
        let current = tools
            .iter()
            .find(|t| t.name == tool_name)
            .map(|t| t.enabled)
            .unwrap_or(false);
        !current
    };
    match store
        .set_enabled(Some(&agent_id), &tool_name, new_enabled)
        .await
    {
        Ok(tool) => {
            publish_agent_config_after_tools_write(&state, &agent_id).await;
            (StatusCode::OK, Json(dto::ToolInfo::from(tool))).into_response()
        }
        Err(e) => map_policy_error(e).into_response(),
    }
}

async fn set_async_execution_agent(
    State(state): State<AppState>,
    Path((agent_id, tool_name)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let body = match parse_async_execution_body(body) {
        Ok(body) => body,
        Err(e) => return e.into_response(),
    };
    let store = &state.tool_config_store;
    match store
        .set_async_execution(Some(&agent_id), &tool_name, body.async_execution)
        .await
    {
        Ok(tool) => {
            publish_agent_config_after_tools_write(&state, &agent_id).await;
            (StatusCode::OK, Json(dto::ToolInfo::from(tool))).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn get_tool_config_agent(
    State(state): State<AppState>,
    Path((agent_id, tool_name)): Path<(String, String)>,
) -> Response {
    let store = &state.tool_config_store;
    match store.get_config(Some(&agent_id), &tool_name).await {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => map_store_error(e).into_response(),
    }
}

async fn set_tool_config_agent(
    State(state): State<AppState>,
    Path((agent_id, tool_name)): Path<(String, String)>,
    Json(body): Json<dto::ToolConfigUpdateRequest>,
) -> Response {
    let store = &state.tool_config_store;
    match store
        .set_config(Some(&agent_id), &tool_name, body.config)
        .await
    {
        Ok(config) => {
            publish_agent_config_after_tools_write(&state, &agent_id).await;
            (StatusCode::OK, Json(config)).into_response()
        }
        Err(e) => map_store_error(e).into_response(),
    }
}
