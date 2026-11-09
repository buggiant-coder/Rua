use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use mcp_manager::types::{McpServerConfig, McpServerId};
use rua_mcp::secret_mask;
use rua_mcp::store_dto::normalize_legacy_json;
use rua_mcp::SharedMcpStore;

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::state::AppState;
use crate::sync_publication::{
    publish_shared_mcp_catalog_committed, publish_shared_mcp_client_deleted,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/shared-mcp", get(list_shared).post(create_shared))
        .route("/shared-mcp/broadcast", post(broadcast))
        .route("/shared-mcp/sync", post(sync_from_agent))
        .route(
            "/shared-mcp/{client_key}",
            get(get_shared).put(update_shared).delete(delete_shared),
        )
}

async fn build_store(state: &AppState) -> Result<SharedMcpStore, ApiError> {
    Ok(SharedMcpStore::new(state.rua_root.clone()).with_device_key(*state.device_key.as_bytes()))
}

fn create_to_typed(create: &dto::SharedMcpClientCreate) -> Result<McpServerConfig, ApiError> {
    let raw = SharedMcpStore::create_to_value(create).map_err(ApiError::from)?;
    let normalized = normalize_legacy_json(&raw);
    serde_json::from_value::<McpServerConfig>(normalized).map_err(|e| {
        ApiError::BadRequest(format!(
            "failed to deserialize shared MCP config '{}': {e}",
            create.key
        ))
    })
}

fn apply_typed_update(
    current: McpServerConfig,
    mut update: dto::SharedMcpClientUpdate,
    client_key: &str,
) -> Result<McpServerConfig, ApiError> {
    let mut raw = serde_json::to_value(&current).map_err(|e| {
        ApiError::Internal(format!(
            "failed to serialize current shared MCP config '{client_key}': {e}"
        ))
    })?;
    secret_mask::restore_masked_values(&mut update, &raw);
    SharedMcpStore::apply_update(&mut raw, &update);
    let normalized = normalize_legacy_json(&raw);
    serde_json::from_value::<McpServerConfig>(normalized).map_err(|e| {
        ApiError::BadRequest(format!(
            "failed to deserialize shared MCP config after update '{client_key}': {e}"
        ))
    })
}

fn masked_dto(key: String, server: McpServerConfig) -> dto::SharedMcpClientConfig {
    let mut dto = dto::SharedMcpClientConfig { key, server };
    secret_mask::mask_client_secrets(&mut dto);
    dto
}

async fn publish_shared_mcp_after_write(state: &AppState) {
    let path = state.rua_root.join("shared_mcp.json");
    let value = match std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
    {
        Some(value) => value,
        None => serde_json::json!({ "clients": {} }),
    };
    publish_shared_mcp_catalog_committed(state, value).await;
}

async fn list_shared(State(state): State<AppState>) -> Response {
    match state.shared_mcp_cells.list().await {
        Ok(map) => {
            let dtos: Vec<dto::SharedMcpClientConfig> = map
                .into_iter()
                .map(|(id, server)| masked_dto(id.to_string(), server))
                .collect();
            (StatusCode::OK, Json(dtos)).into_response()
        }
        Err(e) => {
            ApiError::Internal(format!("list shared MCP configs failed: {e}")).into_response()
        }
    }
}

async fn get_shared(State(state): State<AppState>, Path(client_key): Path<String>) -> Response {
    let id = McpServerId::new(&client_key);
    match state.shared_mcp_cells.cell(&id).read().await {
        Ok(Some(server)) => {
            let dto = masked_dto(client_key, server);
            (StatusCode::OK, Json(dto)).into_response()
        }
        Ok(None) => ApiError::NotFound(format!("Shared MCP client '{client_key}' not found"))
            .into_response(),
        Err(e) => ApiError::Internal(format!("read shared MCP config '{client_key}' failed: {e}"))
            .into_response(),
    }
}

async fn create_shared(
    State(state): State<AppState>,
    Json(body): Json<dto::SharedMcpClientCreate>,
) -> Response {
    if body.key.is_empty() {
        return ApiError::BadRequest("key is required".into()).into_response();
    }

    let id = McpServerId::new(&body.key);
    let cell = state.shared_mcp_cells.cell(&id);

    match cell.read().await {
        Ok(Some(_)) => {
            return ApiError::Conflict(format!("Shared MCP client '{}' already exists", body.key))
                .into_response();
        }
        Ok(None) => {}
        Err(e) => {
            return ApiError::Internal(format!(
                "read shared MCP config '{}' before create failed: {e}",
                body.key
            ))
            .into_response();
        }
    }

    let new_config = match create_to_typed(&body) {
        Ok(cfg) => cfg,
        Err(e) => return e.into_response(),
    };

    if let Err(e) = cell.write(new_config.clone()).await {
        return ApiError::Internal(format!(
            "write shared MCP config '{}' failed: {e}",
            body.key
        ))
        .into_response();
    }

    publish_shared_mcp_after_write(&state).await;

    let dto = masked_dto(body.key, new_config);
    (StatusCode::CREATED, Json(dto)).into_response()
}

async fn update_shared(
    State(state): State<AppState>,
    Path(client_key): Path<String>,
    Json(body): Json<dto::SharedMcpClientUpdate>,
) -> Response {
    let id = McpServerId::new(&client_key);
    let cell = state.shared_mcp_cells.cell(&id);

    let current = match cell.read().await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return ApiError::NotFound(format!("Shared MCP client '{client_key}' not found"))
                .into_response();
        }
        Err(e) => {
            return ApiError::Internal(format!(
                "read shared MCP config '{client_key}' before update failed: {e}"
            ))
            .into_response();
        }
    };

    let new_config = match apply_typed_update(current, body, &client_key) {
        Ok(cfg) => cfg,
        Err(e) => return e.into_response(),
    };

    if let Err(e) = cell.write(new_config.clone()).await {
        return ApiError::Internal(format!(
            "write shared MCP config '{client_key}' failed: {e}"
        ))
        .into_response();
    }

    publish_shared_mcp_after_write(&state).await;

    let dto = masked_dto(client_key, new_config);
    (StatusCode::OK, Json(dto)).into_response()
}

async fn delete_shared(State(state): State<AppState>, Path(client_key): Path<String>) -> Response {
    let id = McpServerId::new(&client_key);
    let cell = state.shared_mcp_cells.cell(&id);

    match cell.read().await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return ApiError::NotFound(format!("Shared MCP client '{client_key}' not found"))
                .into_response();
        }
        Err(e) => {
            return ApiError::Internal(format!(
                "read shared MCP config '{client_key}' before delete failed: {e}"
            ))
            .into_response();
        }
    }

    if let Err(e) = cell.delete().await {
        return ApiError::Internal(format!(
            "delete shared MCP config '{client_key}' failed: {e}"
        ))
        .into_response();
    }

    publish_shared_mcp_client_deleted(&state, &client_key).await;

    let resp = dto::SuccessResponse::ok();
    (StatusCode::OK, Json(resp)).into_response()
}

async fn broadcast(
    State(state): State<AppState>,
    Json(body): Json<dto::SharedMcpBroadcastRequest>,
) -> Response {
    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.broadcast(body).await {
        Ok(mut result) => {
            for target in &mut result.results {
                if target.success {
                    let application =
                        crate::mcp_reload::apply_current(&state, &target.agent_id).await;
                    target
                        .extra
                        .insert("application".to_string(), serde_json::json!(application));
                }
            }
            publish_shared_mcp_after_write(&state).await;
            (StatusCode::OK, Json(result)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn sync_from_agent(
    State(state): State<AppState>,
    Json(body): Json<dto::SharedMcpSyncRequest>,
) -> Response {
    if body.agent_id.is_empty() {
        return ApiError::BadRequest("agent_id is required".into()).into_response();
    }

    let store = match build_store(&state).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    match store.sync_from_agent(body).await {
        Ok(result) => {
            publish_shared_mcp_after_write(&state).await;
            (StatusCode::OK, Json(result)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}
