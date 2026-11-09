use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;

use mcp_manager::types::{McpCallContext, McpServerId, ToolFilter};
use rua_mcp::secret_mask::mask_client_sensitive_fields;
use rua_mcp::McpClientStore;

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::middleware::AgentExtractorContext;
use crate::state::AppState;
use crate::sync_publication::publish_agent_config_file_committed;

pub fn root_router() -> Router<AppState> {
    Router::new()
        .route("/mcp", get(list_clients).post(create_client))
        .route(
            "/mcp/clients",
            get(list_clients_compat).post(register_client_compat),
        )
        .route("/mcp/clients/{name}/tools", get(get_client_tools_direct))
        .route("/mcp/clients/{name}/call", post(call_client_tool_direct))
        .route(
            "/mcp/clients/{name}",
            get(get_client_compat).delete(delete_client_compat),
        )
        .route("/mcp/tools/{client_key}", get(get_client_tools))
        .route("/mcp/toggle/{client_key}", patch(toggle_client))
        .route(
            "/mcp/{client_key}",
            get(get_client).put(update_client).delete(delete_client),
        )
        .route("/mcp/{client_key}/tools", get(get_client_tools))
        .route(
            "/mcp/{client_key}/tool-toggle/{*tool_name}",
            patch(toggle_client_tool),
        )
        .route("/mcp/{client_key}/call", post(call_client_tool))
        .route("/mcp/{client_key}/toggle", patch(toggle_client))
}

pub fn agent_scoped_router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agentId}/mcp/apply", post(apply_saved_mcp))
        .route(
            "/agents/{agentId}/mcp",
            get(list_clients_agent).post(create_client_agent),
        )
        .route(
            "/agents/{agentId}/mcp/{client_key}",
            get(get_client_agent)
                .put(update_client_agent)
                .delete(delete_client_agent),
        )
        .route(
            "/agents/{agentId}/mcp/tools/{client_key}",
            get(get_client_tools_agent),
        )
        .route(
            "/agents/{agentId}/mcp/toggle/{client_key}",
            patch(toggle_client_agent),
        )
        .route(
            "/agents/{agentId}/mcp/{client_key}/tools",
            get(get_client_tools_agent),
        )
        .route(
            "/agents/{agentId}/mcp/{client_key}/tool-toggle/{*tool_name}",
            patch(toggle_client_tool_agent),
        )
        .route(
            "/agents/{agentId}/mcp/{client_key}/call",
            post(call_client_tool_agent),
        )
        .route(
            "/agents/{agentId}/mcp/{client_key}/toggle",
            patch(toggle_client_agent),
        )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpToolCallRequest {
    tool: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpToolToggleRequest {
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClientRegisterRequest {
    name: String,
    #[serde(rename = "type", default)]
    transport_type: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: Option<serde_json::Value>,
    #[serde(default)]
    headers: Option<serde_json::Value>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

async fn build_store(state: &AppState) -> Result<McpClientStore, ApiError> {
    Ok(McpClientStore::with_agent_mcp_cells(
        state.rua_root.clone(),
        std::sync::Arc::clone(&state.agent_mcp_cells),
    )
    .with_device_key(*state.device_key.as_bytes()))
}

pub(crate) async fn publish_agent_config_after_mcp_write(
    state: &AppState,
    store: &McpClientStore,
    agent_id: &str,
) {
    let path = rua_mcp::agent_io::agent_json_path(store.data_dir(), Some(agent_id));
    publish_agent_config_file_committed(state, agent_id, &path, "mcp agent config").await;
}

async fn list_tools_via_manager(
    state: &AppState,
    agent_id: &str,
    client_key: &str,
) -> Result<Vec<dto::McpToolInfo>, Response> {
    let manager = state.mcp_manager_for(agent_id).await.ok_or_else(|| {
        ApiError::Unavailable(format!(
            "MCP manager not initialized for agent '{agent_id}'"
        ))
        .into_response()
    })?;
    let filter = ToolFilter {
        server_id: Some(McpServerId::new(client_key)),
        name_prefix: None,
        include_disabled: true,
    };
    let tools = manager
        .list_tools(filter)
        .await
        .map_err(|e| ApiError::Unavailable(e.to_string()).into_response())?;

    let store = build_store(state).await.map_err(|e| e.into_response())?;
    let disabled_tools: BTreeSet<String> = match store
        .get_client_for_execution(Some(agent_id), client_key)
        .await
    {
        Ok(Some(client)) => client.server.disabled_tools.into_iter().collect(),
        Ok(None) => BTreeSet::new(),
        Err(e) => return Err(ApiError::from(e).into_response()),
    };
    Ok(tools
        .into_iter()
        .map(|t| dto::McpToolInfo {
            id: t.name.clone(),
            enabled: !disabled_tools.contains(&t.name),
            name: t.name,
            description: t.description,
            schema: Some(t.input_schema),
            extra: serde_json::Map::new(),
        })
        .collect())
}

async fn client_enabled_or_response(
    state: &AppState,
    agent_id: &str,
    client_key: &str,
) -> Result<bool, Response> {
    let store = match build_store(state).await {
        Ok(s) => s,
        Err(e) => return Err(e.into_response()),
    };
    match store.get_client(Some(agent_id), client_key).await {
        Ok(Some(client)) => Ok(client.server.enabled),
        Ok(None) => {
            Err(ApiError::NotFound(format!("MCP client '{client_key}' not found")).into_response())
        }
        Err(e) => Err(ApiError::from(e).into_response()),
    }
}

async fn call_tool_via_manager(
    state: &AppState,
    agent_id: &str,
    client_key: &str,
    tool_name: &str,
    tool_args: serde_json::Value,
) -> Result<serde_json::Value, Response> {
    let manager = state.mcp_manager_for(agent_id).await.ok_or_else(|| {
        ApiError::Unavailable(format!(
            "MCP manager not initialized for agent '{agent_id}'"
        ))
        .into_response()
    })?;
    let server_id = McpServerId::new(client_key);
    let result = manager
        .call_tool(&server_id, tool_name, tool_args, McpCallContext::default())
        .await
        .map_err(|e| ApiError::Unavailable(e.to_string()).into_response())?;
    serde_json::to_value(&result).map_err(|e| {
        ApiError::Internal(format!("failed to serialize MCP call result: {e}")).into_response()
    })
}

async fn list_clients(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.list_clients(Some(&ctx.agent_id)).await {
        Ok(mut clients) => {
            for c in &mut clients {
                mask_client_sensitive_fields(c);
            }
            (StatusCode::OK, Json(clients)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_client(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(client_key): Path<String>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.get_client(Some(&ctx.agent_id), &client_key).await {
        Ok(Some(mut client)) => {
            mask_client_sensitive_fields(&mut client);
            (StatusCode::OK, Json(client)).into_response()
        }
        Ok(None) => {
            ApiError::NotFound(format!("MCP client '{client_key}' not found")).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_client_tools(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(client_key): Path<String>,
) -> Response {
    match client_enabled_or_response(&state, &ctx.agent_id, &client_key).await {
        Ok(false) => return (StatusCode::OK, Json(Vec::<dto::McpToolInfo>::new())).into_response(),
        Ok(true) => {}
        Err(resp) => return resp,
    }

    if let Some(cached) = state.mcp_tools_cache.get(&ctx.agent_id, &client_key).await {
        return (StatusCode::OK, Json(cached)).into_response();
    }

    match list_tools_via_manager(&state, &ctx.agent_id, &client_key).await {
        Ok(tools) => {
            state
                .mcp_tools_cache
                .put(&ctx.agent_id, &client_key, tools.clone())
                .await;
            (StatusCode::OK, Json(tools)).into_response()
        }
        Err(resp) => resp,
    }
}

async fn call_client_tool(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(client_key): Path<String>,
    Json(body): Json<McpToolCallRequest>,
) -> Response {
    match client_enabled_or_response(&state, &ctx.agent_id, &client_key).await {
        Ok(false) => {
            return ApiError::BadRequest(format!("MCP client '{client_key}' is disabled"))
                .into_response();
        }
        Ok(true) => {}
        Err(resp) => return resp,
    }

    match call_tool_via_manager(
        &state,
        &ctx.agent_id,
        &client_key,
        &body.tool,
        body.arguments,
    )
    .await
    {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(resp) => resp,
    }
}

async fn toggle_client_tool(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path((client_key, tool_name)): Path<(String, String)>,
    Json(body): Json<McpToolToggleRequest>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store
        .set_tool_enabled(Some(&ctx.agent_id), &client_key, &tool_name, body.enabled)
        .await
    {
        Ok(mut client) => {
            mask_client_sensitive_fields(&mut client);
            state
                .mcp_tools_cache
                .invalidate(&ctx.agent_id, &client_key)
                .await;
            publish_agent_config_after_mcp_write(&state, &store, &ctx.agent_id).await;
            (StatusCode::OK, Json(client)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn create_client(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let create = match parse_mcp_create_request(body) {
        Ok(c) => c,
        Err(msg) => return ApiError::BadRequest(msg).into_response(),
    };

    if create.key.is_empty() {
        return ApiError::BadRequest("key is required".into()).into_response();
    }

    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.create_client(Some(&ctx.agent_id), create).await {
        Ok(mut client) => {
            mask_client_sensitive_fields(&mut client);
            let application = crate::mcp_reload::apply_current(&state, &ctx.agent_id).await;
            publish_agent_config_after_mcp_write(&state, &store, &ctx.agent_id).await;
            mcp_write_response(StatusCode::CREATED, client, application)
        }
        Err(e) => {
            if let rua_core::RuaError::Validation { ref message } = e {
                if message.contains("already exists") {
                    return ApiError::Conflict(message.clone()).into_response();
                }
            }
            ApiError::from(e).into_response()
        }
    }
}

async fn update_client(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(client_key): Path<String>,
    Json(body): Json<dto::McpClientUpdate>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store
        .update_client(Some(&ctx.agent_id), &client_key, body)
        .await
    {
        Ok(mut client) => {
            mask_client_sensitive_fields(&mut client);
            let application = crate::mcp_reload::apply_current(&state, &ctx.agent_id).await;
            publish_agent_config_after_mcp_write(&state, &store, &ctx.agent_id).await;
            mcp_write_response(StatusCode::OK, client, application)
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn toggle_client(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(client_key): Path<String>,
    _body: axum::body::Bytes,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let new_enabled = match store.get_client(Some(&ctx.agent_id), &client_key).await {
        Ok(Some(c)) => !c.server.enabled,
        Ok(None) => {
            return ApiError::NotFound(format!("MCP client '{client_key}' not found"))
                .into_response();
        }
        Err(e) => return ApiError::from(e).into_response(),
    };
    match store
        .toggle_client(Some(&ctx.agent_id), &client_key, new_enabled)
        .await
    {
        Ok(mut client) => {
            mask_client_sensitive_fields(&mut client);
            let application = crate::mcp_reload::apply_current(&state, &ctx.agent_id).await;
            publish_agent_config_after_mcp_write(&state, &store, &ctx.agent_id).await;
            mcp_write_response(StatusCode::OK, client, application)
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn delete_client(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(client_key): Path<String>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.delete_client(Some(&ctx.agent_id), &client_key).await {
        Ok(()) => {
            let application = crate::mcp_reload::apply_current(&state, &ctx.agent_id).await;
            publish_agent_config_after_mcp_write(&state, &store, &ctx.agent_id).await;
            (
                StatusCode::OK,
                Json(
                    json!({"message": format!("MCP client '{}' deleted successfully", client_key), "application": application}),
                ),
            )
                .into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn register_client_compat(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<ClientRegisterRequest>,
) -> Response {
    let create = dto::McpClientCreate {
        key: body.name.clone(),
        name: body.name,
        enabled: true,
        transport: body.transport_type,
        command: body.command,
        args: body.args,
        env: body.env,
        url: body.url,
        headers: body.headers,
        timeout: None,
        auto_start: None,
        oauth: None,
        disabled_tools: Vec::new(),
        extra: body.extra,
    };

    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.create_client(Some(&ctx.agent_id), create).await {
        Ok(mut client) => {
            mask_client_sensitive_fields(&mut client);
            crate::mcp_reload::reload_from_disk(&state, &ctx.agent_id).await;
            publish_agent_config_after_mcp_write(&state, &store, &ctx.agent_id).await;
            (StatusCode::CREATED, Json(client)).into_response()
        }
        Err(e) => {
            if let rua_core::RuaError::Validation { ref message } = e {
                if message.contains("already exists") {
                    return ApiError::Conflict(message.clone()).into_response();
                }
            }
            ApiError::from(e).into_response()
        }
    }
}

async fn list_clients_compat(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.list_clients(Some(&ctx.agent_id)).await {
        Ok(mut clients) => {
            for c in &mut clients {
                mask_client_sensitive_fields(c);
            }
            (StatusCode::OK, Json(clients)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_client_compat(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(name): Path<String>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.get_client(Some(&ctx.agent_id), &name).await {
        Ok(Some(mut client)) => {
            mask_client_sensitive_fields(&mut client);
            (StatusCode::OK, Json(client)).into_response()
        }
        Ok(None) => ApiError::NotFound(format!("MCP client '{name}' not found")).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn delete_client_compat(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(name): Path<String>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.delete_client(Some(&ctx.agent_id), &name).await {
        Ok(()) => {
            crate::mcp_reload::reload_from_disk(&state, &ctx.agent_id).await;
            publish_agent_config_after_mcp_write(&state, &store, &ctx.agent_id).await;
            (
                StatusCode::OK,
                Json(json!({"message": format!("MCP client '{}' deleted successfully", name)})),
            )
                .into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_client_tools_direct(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(name): Path<String>,
) -> Response {
    match client_enabled_or_response(&state, &ctx.agent_id, &name).await {
        Ok(false) => return (StatusCode::OK, Json(Vec::<dto::McpToolInfo>::new())).into_response(),
        Ok(true) => {}
        Err(resp) => return resp,
    }

    if let Some(cached) = state.mcp_tools_cache.get(&ctx.agent_id, &name).await {
        return (StatusCode::OK, Json(cached)).into_response();
    }
    match list_tools_via_manager(&state, &ctx.agent_id, &name).await {
        Ok(tools) => {
            state
                .mcp_tools_cache
                .put(&ctx.agent_id, &name, tools.clone())
                .await;
            (StatusCode::OK, Json(tools)).into_response()
        }
        Err(resp) => resp,
    }
}

async fn call_client_tool_direct(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Path(name): Path<String>,
    Json(body): Json<McpToolCallRequest>,
) -> Response {
    match client_enabled_or_response(&state, &ctx.agent_id, &name).await {
        Ok(false) => {
            return ApiError::BadRequest(format!("MCP client '{name}' is disabled"))
                .into_response();
        }
        Ok(true) => {}
        Err(resp) => return resp,
    }

    match call_tool_via_manager(&state, &ctx.agent_id, &name, &body.tool, body.arguments).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(resp) => resp,
    }
}

async fn list_clients_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.list_clients(Some(&agent_id)).await {
        Ok(mut clients) => {
            for c in &mut clients {
                mask_client_sensitive_fields(c);
            }
            (StatusCode::OK, Json(clients)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_client_agent(
    State(state): State<AppState>,
    Path((agent_id, client_key)): Path<(String, String)>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.get_client(Some(&agent_id), &client_key).await {
        Ok(Some(mut client)) => {
            mask_client_sensitive_fields(&mut client);
            (StatusCode::OK, Json(client)).into_response()
        }
        Ok(None) => {
            ApiError::NotFound(format!("MCP client '{client_key}' not found")).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_client_tools_agent(
    State(state): State<AppState>,
    Path((agent_id, client_key)): Path<(String, String)>,
) -> Response {
    match client_enabled_or_response(&state, &agent_id, &client_key).await {
        Ok(false) => return (StatusCode::OK, Json(Vec::<dto::McpToolInfo>::new())).into_response(),
        Ok(true) => {}
        Err(resp) => return resp,
    }

    if let Some(cached) = state.mcp_tools_cache.get(&agent_id, &client_key).await {
        return (StatusCode::OK, Json(cached)).into_response();
    }

    match list_tools_via_manager(&state, &agent_id, &client_key).await {
        Ok(tools) => {
            state
                .mcp_tools_cache
                .put(&agent_id, &client_key, tools.clone())
                .await;
            (StatusCode::OK, Json(tools)).into_response()
        }
        Err(resp) => resp,
    }
}

async fn call_client_tool_agent(
    State(state): State<AppState>,
    Path((agent_id, client_key)): Path<(String, String)>,
    Json(body): Json<McpToolCallRequest>,
) -> Response {
    match client_enabled_or_response(&state, &agent_id, &client_key).await {
        Ok(false) => {
            return ApiError::BadRequest(format!("MCP client '{client_key}' is disabled"))
                .into_response();
        }
        Ok(true) => {}
        Err(resp) => return resp,
    }

    match call_tool_via_manager(&state, &agent_id, &client_key, &body.tool, body.arguments).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(resp) => resp,
    }
}

async fn toggle_client_tool_agent(
    State(state): State<AppState>,
    Path((agent_id, client_key, tool_name)): Path<(String, String, String)>,
    Json(body): Json<McpToolToggleRequest>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store
        .set_tool_enabled(Some(&agent_id), &client_key, &tool_name, body.enabled)
        .await
    {
        Ok(mut client) => {
            mask_client_sensitive_fields(&mut client);
            state
                .mcp_tools_cache
                .invalidate(&agent_id, &client_key)
                .await;
            publish_agent_config_after_mcp_write(&state, &store, &agent_id).await;
            (StatusCode::OK, Json(client)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn create_client_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let create = match parse_mcp_create_request(body) {
        Ok(c) => c,
        Err(msg) => return ApiError::BadRequest(msg).into_response(),
    };

    if create.key.is_empty() {
        return ApiError::BadRequest("key is required".into()).into_response();
    }

    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.create_client(Some(&agent_id), create).await {
        Ok(mut client) => {
            mask_client_sensitive_fields(&mut client);
            let application = crate::mcp_reload::apply_current(&state, &agent_id).await;
            publish_agent_config_after_mcp_write(&state, &store, &agent_id).await;
            mcp_write_response(StatusCode::CREATED, client, application)
        }
        Err(e) => {
            if let rua_core::RuaError::Validation { ref message } = e {
                if message.contains("already exists") {
                    return ApiError::Conflict(message.clone()).into_response();
                }
            }
            ApiError::from(e).into_response()
        }
    }
}

async fn update_client_agent(
    State(state): State<AppState>,
    Path((agent_id, client_key)): Path<(String, String)>,
    Json(body): Json<dto::McpClientUpdate>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store
        .update_client(Some(&agent_id), &client_key, body)
        .await
    {
        Ok(mut client) => {
            mask_client_sensitive_fields(&mut client);
            let application = crate::mcp_reload::apply_current(&state, &agent_id).await;
            publish_agent_config_after_mcp_write(&state, &store, &agent_id).await;
            mcp_write_response(StatusCode::OK, client, application)
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn toggle_client_agent(
    State(state): State<AppState>,
    Path((agent_id, client_key)): Path<(String, String)>,
    _body: axum::body::Bytes,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    let new_enabled = match store.get_client(Some(&agent_id), &client_key).await {
        Ok(Some(c)) => !c.server.enabled,
        Ok(None) => {
            return ApiError::NotFound(format!("MCP client '{client_key}' not found"))
                .into_response();
        }
        Err(e) => return ApiError::from(e).into_response(),
    };
    match store
        .toggle_client(Some(&agent_id), &client_key, new_enabled)
        .await
    {
        Ok(mut client) => {
            mask_client_sensitive_fields(&mut client);
            let application = crate::mcp_reload::apply_current(&state, &agent_id).await;
            publish_agent_config_after_mcp_write(&state, &store, &agent_id).await;
            mcp_write_response(StatusCode::OK, client, application)
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn delete_client_agent(
    State(state): State<AppState>,
    Path((agent_id, client_key)): Path<(String, String)>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.delete_client(Some(&agent_id), &client_key).await {
        Ok(()) => {
            let application = crate::mcp_reload::apply_current(&state, &agent_id).await;
            publish_agent_config_after_mcp_write(&state, &store, &agent_id).await;
            (
                StatusCode::OK,
                Json(
                    json!({"message": format!("MCP client '{}' deleted successfully", client_key), "application": application}),
                ),
            )
                .into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn apply_saved_mcp(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    let application = crate::mcp_reload::apply_current(&state, &agent_id).await;
    (StatusCode::OK, Json(application)).into_response()
}

fn mcp_write_response(
    status: StatusCode,
    value: impl Serialize,
    application: crate::mcp_reload::McpApplication,
) -> Response {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::Object(mut value)) => {
            value.insert("application".to_string(), json!(application));
            (status, Json(value)).into_response()
        }
        _ => ApiError::Internal("Failed to encode saved MCP configuration".to_string())
            .into_response(),
    }
}

fn parse_mcp_create_request(body: serde_json::Value) -> Result<dto::McpClientCreate, String> {
    if body.get("client_key").is_some() && body.get("client").is_some() {
        let nested: dto::McpClientCreateNested =
            serde_json::from_value(body).map_err(|e| format!("Invalid nested request: {e}"))?;
        nested
            .into_flat()
            .map_err(|e| format!("Invalid client object: {e}"))
    } else {
        serde_json::from_value(body).map_err(|e| format!("Invalid request: {e}"))
    }
}
