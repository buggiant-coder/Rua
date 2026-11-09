use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};

use rua_workspace::{ShutdownPolicy, ShutdownReport, WorkspaceError, WorkspaceErrorKind};

use crate::state::AppState;

pub fn register_workspace_admin_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/workspace/{agent_id}/reload",
            post(workspace_reload_handler),
        )
        .route(
            "/workspace/{agent_id}/shutdown",
            post(workspace_shutdown_handler),
        )
}

pub async fn workspace_reload_handler(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    match state.registry.reload_agent(&agent_id).await {
        Ok(()) => {
            tracing::info!(agent_id = %agent_id, "workspace_admin: reload accepted");
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "agent_id": agent_id,
                    "status": "reloaded",
                })),
            )
                .into_response()
        }
        Err(err) => map_workspace_error(&agent_id, &err),
    }
}

pub async fn workspace_shutdown_handler(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Response {
    // One call owns the whole shutdown: drain, work termination through the
    // authority, component stops and the deadline. Doing any of it here as well
    // would run the policy twice and report the operator's own request back.
    match state
        .registry
        .remove_with_policy(&agent_id, ShutdownPolicy::default())
        .await
    {
        Ok(report) => shutdown_response(&agent_id, &report),
        Err(err) => map_workspace_error(&agent_id, &err),
    }
}

/// Turns an observed shutdown into a response that cannot overstate it.
///
/// `200` is only returned for a clean report. Anything that leaked, timed out or
/// left work unresolved is a gateway timeout or a server error, with the same
/// report attached so the caller can see *what* is still alive.
fn shutdown_response(agent_id: &str, report: &ShutdownReport) -> Response {
    let clean = report.is_clean();
    let within_timeout = report.finished_within_timeout;
    let body = serde_json::json!({
        "agent_id": agent_id,
        "status": if clean { "stopped" } else { "incomplete" },
        "report": report,
    });

    if clean {
        tracing::info!(agent_id, "workspace_admin: shutdown completed");
        return (StatusCode::OK, Json(body)).into_response();
    }

    let (status, detail) = if !within_timeout || !report.timed_out_components.is_empty() {
        (
            StatusCode::GATEWAY_TIMEOUT,
            format!("shutdown for agent '{agent_id}' did not finish inside its deadline"),
        )
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("shutdown for agent '{agent_id}' left resources or work behind"),
        )
    };
    tracing::warn!(
        agent_id,
        failed = ?report.failed_components,
        timed_out = ?report.timed_out_components,
        turns_unresolved = report.turns_unresolved,
        tasks_unresolved = report.tasks_unresolved,
        queued_inputs_unresolved = report.queued_inputs_unresolved,
        authority_confirmed = report.authority_confirmed,
        "workspace_admin: shutdown did not complete cleanly"
    );
    let mut body = body;
    if let Some(object) = body.as_object_mut() {
        object.insert("detail".to_string(), serde_json::json!(detail));
        object.insert(
            "kind".to_string(),
            serde_json::json!(WorkspaceErrorKind::Timeout.to_string()),
        );
    }
    (status, Json(body)).into_response()
}

fn map_workspace_error(agent_id: &str, err: &WorkspaceError) -> Response {
    let status = match err.kind {
        WorkspaceErrorKind::AgentNotFound => StatusCode::NOT_FOUND,
        WorkspaceErrorKind::Timeout => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let body = serde_json::json!({
        "agent_id": agent_id,
        "detail": err.message,
        "kind": err.kind.to_string(),
    });
    (status, Json(body)).into_response()
}
