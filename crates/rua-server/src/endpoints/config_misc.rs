use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::json;

use crate::endpoints::config::{
    read_heartbeat_cells, read_user_timezone, trigger_manual_heartbeat_run, workspace_working_dir,
    write_heartbeat_cells, write_user_timezone,
};
use crate::endpoints::dto;
use crate::errors::ApiError;
use crate::middleware::AgentExtractorContext;
use crate::state::AppState;
use crate::sync_publication::{
    publish_agent_config_file_committed, publish_sync_fact, SyncPublicationFact,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/config/heartbeat",
            get(get_heartbeat).put(update_heartbeat),
        )
        .route("/config/heartbeat/run", post(run_heartbeat_now))
        .route(
            "/config/user-timezone",
            get(get_user_timezone).put(update_user_timezone),
        )
}

async fn get_heartbeat(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    match read_heartbeat_cells(&state.agent_cells, &ctx.agent_id).await {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn update_heartbeat(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    Json(body): Json<dto::HeartbeatConfig>,
) -> Response {
    match write_heartbeat_cells(&state.agent_cells, &ctx.agent_id, body).await {
        Ok(config) => {
            let path = state
                .rua_root
                .join("workspaces")
                .join(&ctx.agent_id)
                .join("agent.json");
            publish_agent_config_file_committed(
                &state,
                &ctx.agent_id,
                &path,
                "heartbeat agent config",
            )
            .await;
            (StatusCode::OK, Json(config)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn run_heartbeat_now(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
) -> Response {
    match trigger_manual_heartbeat_run(&state, ctx.agent_id).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "started": true }))).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn get_user_timezone(State(state): State<AppState>) -> Response {
    let working_dir = match workspace_working_dir(&state).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    match read_user_timezone(&working_dir) {
        Ok(config) => (StatusCode::OK, Json(config)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn update_user_timezone(
    State(state): State<AppState>,
    Json(body): Json<dto::UserTimezoneConfig>,
) -> Response {
    if body.timezone.trim().is_empty() {
        return ApiError::BadRequest("timezone is required".into()).into_response();
    }
    let working_dir = match workspace_working_dir(&state).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    match write_user_timezone(&working_dir, body) {
        Ok(config) => {
            let _ = publish_sync_fact(
                &state,
                SyncPublicationFact::RootUserTimezoneCommitted {
                    timezone: serde_json::Value::String(config.timezone.clone()),
                },
            )
            .await;
            (StatusCode::OK, Json(config)).into_response()
        }
        Err(e) => e.into_response(),
    }
}
