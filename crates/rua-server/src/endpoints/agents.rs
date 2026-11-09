use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde::Deserialize;

use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::state::AppState;
use crate::sync_publication::{
    publish_agent_config_committed, publish_agent_config_deleted, publish_agent_security_committed,
    publish_agent_security_deleted, publish_sync_fact, SyncPublicationFact,
};

#[derive(Debug, Deserialize)]
struct AgentOrderRequest {
    #[serde(alias = "agent_ids")]
    order: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AgentToggleRequest {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct AgentRenameRequest {
    old_id: String,
    new_id: String,
}

fn config_json_path(state: &AppState) -> PathBuf {
    state.rua_root.join("config.json")
}

async fn publish_agents_config(state: &AppState) {
    let path = config_json_path(state);
    let config: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(_) => return,
        },
        Err(_) => return,
    };
    let Some(agents) = config.get("agents").cloned() else {
        return;
    };
    let _ = publish_sync_fact(
        state,
        SyncPublicationFact::RootAgentRegistryCommitted { agents },
    )
    .await;
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents", get(list_agents).post(create_agent))
        .route("/agents/order", put(save_agent_order))
        .route("/agents/rename", post(rename_agent))
        .route(
            "/agents/{id}",
            get(get_agent).put(update_agent).delete(delete_agent),
        )
        .route("/agents/{agentId}/status", get(get_agent_status))
        .route("/agents/{agentId}/toggle", patch(toggle_agent))
}

async fn list_agents(State(state): State<AppState>) -> Response {
    match state.agent_service.list_agents().await {
        Ok(agents) => {
            let resp = dto::AgentListResponse { agents };
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_agent(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.agent_service.get_agent(&id).await {
        Ok(Some(agent)) => (StatusCode::OK, Json(agent)).into_response(),
        Ok(None) => ApiError::NotFound(format!("Agent '{id}' not found")).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn get_agent_status(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    match state.agent_service.get_agent_status(&agent_id).await {
        Ok(Some(status)) => (StatusCode::OK, Json(status)).into_response(),
        Ok(None) => ApiError::NotFound(format!("Agent '{agent_id}' not found")).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn create_agent(
    State(state): State<AppState>,
    Json(body): Json<dto::AgentCreate>,
) -> Response {
    if body.name.is_empty() {
        return ApiError::BadRequest("Agent name is required".into()).into_response();
    }

    match state.agent_service.create_agent(body).await {
        Ok(agent) => {
            let builtin = rua_security::SecurityConfigStore::builtin_template();
            if let Err(e) = state.security.save_and_invalidate(&agent.id, &builtin) {
                tracing::warn!(
                    "failed to write security.json for agent '{}': {e}",
                    agent.id
                );
            }
            let security_val = serde_json::to_value(&builtin).unwrap_or_default();
            publish_agent_security_committed(&state, &agent.id, security_val).await;
            let agent_id = agent.id.clone();
            let state_for_reload = state.clone();
            tokio::spawn(async move {
                crate::mcp_reload::reload_from_disk(&state_for_reload, &agent_id).await;
            });
            let agent_val = serde_json::to_value(&agent).unwrap_or_default();
            let published_agent_id = agent.id.clone();
            publish_agent_config_committed(&state, &published_agent_id, agent_val).await;
            publish_agents_config(&state).await;
            (StatusCode::CREATED, Json(agent)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn update_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(mut body): Json<dto::AgentUpdate>,
) -> Response {
    let security_update = body.security.take();
    let has_running_update = body.running.is_some();
    let has_channels_update = body.channels.is_some();
    let has_maintenance_update = body.extra.contains_key("maintenance_interval_hours");
    let previous_mcp = if body.mcp.is_some() {
        match state.agent_service.get_agent(&id).await {
            Ok(Some(agent)) => agent.mcp,
            Ok(None) => {
                return ApiError::NotFound(format!("Agent '{id}' not found")).into_response()
            }
            Err(error) => return ApiError::from(error).into_response(),
        }
    } else {
        None
    };
    let has_mcp_update = body.mcp.is_some();
    match state.agent_service.update_agent(&id, body).await {
        Ok(agent) => {
            if let Some(security_value) = security_update {
                if let Ok(cfg) = serde_json::from_value::<rua_config::types::security::SecurityConfig>(
                    security_value,
                ) {
                    if let Err(e) = state.security.save_and_invalidate(&id, &cfg) {
                        tracing::warn!("failed to save security config for agent '{id}': {e}");
                    }
                    let security_val = serde_json::to_value(&cfg).unwrap_or_default();
                    publish_agent_security_committed(&state, &id, security_val).await;
                }
            }
            let application = if has_mcp_update && previous_mcp != agent.mcp {
                Some(crate::mcp_reload::apply_current(&state, &id).await)
            } else {
                None
            };
            let mut agent_val = serde_json::to_value(&agent).unwrap_or_default();
            let published_agent_id = id.clone();
            publish_agent_config_committed(&state, &published_agent_id, agent_val.clone()).await;
            if has_channels_update
                && crate::channels_reload::apply_current(&state, &id)
                    .await
                    .is_err()
            {
                return ApiError::Internal(
                    "Agent configuration saved, but Channel resource application failed"
                        .to_string(),
                )
                .into_response();
            }
            if has_running_update {
                if let Some(runtime) = state.registry.get(&id).await {
                    if runtime.apply_embedding_configuration().await.is_err() {
                        return ApiError::Internal(
                            "Agent configuration saved, but Embedding resource application failed"
                                .to_string(),
                        )
                        .into_response();
                    }
                }
            }
            if has_maintenance_update {
                if let Some(runtime) = state.registry.get(&id).await {
                    if runtime.apply_maintenance_configuration().await.is_err() {
                        return ApiError::Internal(
                            "Agent configuration saved, but maintenance application failed"
                                .to_string(),
                        )
                        .into_response();
                    }
                }
            }
            if let Some(application) = application {
                agent_val["application"] = serde_json::json!(application);
            }
            (StatusCode::OK, Json(agent_val)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn delete_agent(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if id == "default" {
        return ApiError::BadRequest("Cannot delete the default agent".into()).into_response();
    }

    if let Err(error) = state
        .conversation_input_intake
        .cleanup_deleted_project_agent(&id)
        .await
    {
        return ApiError::Conflict(format!(
            "Failed to stop Project work before deleting Agent '{id}': {error}"
        ))
        .into_response();
    }
    if state.registry.get(&id).await.is_some() {
        // The shutdown outcome decides whether the Agent record may be deleted:
        // removing it after a leak would leave processes, sessions and work with
        // no owner to stop them. Only a clean report lets deletion continue.
        match state
            .registry
            .remove_with_policy(&id, rua_workspace::ShutdownPolicy::default())
            .await
        {
            Ok(report) if report.is_clean() => {}
            Ok(report) => {
                tracing::warn!(
                    agent_id = %id,
                    failed = ?report.failed_components,
                    timed_out = ?report.timed_out_components,
                    turns_unresolved = report.turns_unresolved,
                    tasks_unresolved = report.tasks_unresolved,
                    queued_inputs_unresolved = report.queued_inputs_unresolved,
                    authority_confirmed = report.authority_confirmed,
                    "refusing to delete an Agent whose runtime did not stop cleanly"
                );
                return ApiError::Conflict(format!(
                    "Agent runtime did not stop cleanly: failed=[{}] timed_out=[{}] unresolved_turns={} unresolved_tasks={} unresolved_inputs={}",
                    report.failed_components.join(", "),
                    report.timed_out_components.join(", "),
                    report.turns_unresolved,
                    report.tasks_unresolved,
                    report.queued_inputs_unresolved,
                ))
                .into_response();
            }
            Err(error) => {
                return ApiError::Internal(format!(
                    "Failed to remove Agent runtime before deletion: {error}"
                ))
                .into_response();
            }
        }
    }

    let store = state.security.config_store();
    if let Err(e) = store.delete(&id) {
        tracing::warn!("failed to delete security.json for agent '{id}': {e}");
    }
    state.security.invalidate(&id);

    match state.agent_service.delete_agent(&id).await {
        Ok(()) => {
            publish_agent_config_deleted(&state, &id).await;
            publish_agent_security_deleted(&state, &id).await;
            publish_agents_config(&state).await;
            let resp = serde_json::json!({ "success": true, "agent_id": id });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}

async fn save_agent_order(
    State(state): State<AppState>,
    Json(body): Json<AgentOrderRequest>,
) -> Response {
    let path = config_json_path(&state);

    let mut config: rua_config::RootConfig = match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(e) => {
                return ApiError::Internal(format!("Failed to parse config.json: {e}"))
                    .into_response();
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => rua_config::RootConfig::default(),
        Err(e) => {
            return ApiError::Internal(format!("Failed to read config.json: {e}")).into_response();
        }
    };

    let expected_ids: std::collections::BTreeSet<_> =
        config.agents.profiles.keys().cloned().collect();
    let mut seen_ids = std::collections::BTreeSet::new();
    let has_duplicate = body.order.iter().any(|id| !seen_ids.insert(id.clone()));
    if has_duplicate || seen_ids != expected_ids {
        return ApiError::BadRequest(
            "agent_ids must contain each configured agent id exactly once".into(),
        )
        .into_response();
    }

    config.agents.agent_order = body.order.clone();

    let value = match serde_json::to_value(&config) {
        Ok(v) => v,
        Err(e) => {
            return ApiError::Internal(format!("Failed to serialize config: {e}")).into_response();
        }
    };

    if let Err(e) = rua_config::atomic_write_json(&path, &value) {
        return ApiError::Internal(format!("Failed to write config.json: {e}")).into_response();
    }

    publish_agents_config(&state).await;
    let resp = serde_json::json!({ "success": true, "agent_ids": body.order });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn toggle_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    body: Option<Json<AgentToggleRequest>>,
) -> Response {
    if agent_id == "default" {
        return ApiError::BadRequest("Cannot disable the default agent".into()).into_response();
    }

    match state.agent_service.get_agent(&agent_id).await {
        Ok(None) => {
            return ApiError::NotFound(format!("Agent '{agent_id}' not found")).into_response();
        }
        Err(e) => return ApiError::from(e).into_response(),
        Ok(Some(_)) => {}
    }

    let path = config_json_path(&state);
    let mut config: rua_config::RootConfig = match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(c) => c,
            Err(e) => {
                return ApiError::Internal(format!("Failed to parse config.json: {e}"))
                    .into_response();
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => rua_config::RootConfig::default(),
        Err(e) => {
            return ApiError::Internal(format!("Failed to read config.json: {e}")).into_response();
        }
    };

    let new_enabled = if let Some(Json(b)) = body {
        b.enabled
    } else {
        let current = config
            .agents
            .profiles
            .get(&agent_id)
            .and_then(|v| v.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        !current
    };

    if let Some(profile_ref) = config.agents.profiles.get_mut(&agent_id) {
        if let Some(obj) = profile_ref.as_object_mut() {
            obj.insert("enabled".into(), serde_json::Value::Bool(new_enabled));
        }
    }

    let value = match serde_json::to_value(&config) {
        Ok(v) => v,
        Err(e) => {
            return ApiError::Internal(format!("Failed to serialize config: {e}")).into_response();
        }
    };

    if let Err(e) = rua_config::atomic_write_json(&path, &value) {
        return ApiError::Internal(format!("Failed to write config.json: {e}")).into_response();
    }

    publish_agents_config(&state).await;
    let resp = serde_json::json!({
        "success": true,
        "agent_id": agent_id,
        "enabled": new_enabled,
    });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn rename_agent(
    State(state): State<AppState>,
    Json(body): Json<AgentRenameRequest>,
) -> Response {
    if body.new_id.is_empty() {
        return ApiError::BadRequest("new_id is required".into()).into_response();
    }

    let update = dto::AgentUpdate {
        name: Some(body.new_id.clone()),
        description: None,
        enabled: None,
        language: None,
        system_prompt_files: None,
        running: None,
        default_model: None,
        channels: None,
        mcp: None,
        heartbeat: None,
        llm_routing: None,
        tools: None,
        plan: None,
        security: None,
        skill_names: None,
        extra: serde_json::Map::new(),
    };

    match state.agent_service.update_agent(&body.old_id, update).await {
        Ok(agent) => {
            let agent_val = serde_json::to_value(&agent).unwrap_or_default();
            publish_agent_config_committed(&state, &body.old_id, agent_val).await;
            publish_agents_config(&state).await;
            let resp = serde_json::json!({
                "id": agent.id,
                "workspace_dir": agent.workspace_dir,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => ApiError::from(e).into_response(),
    }
}
