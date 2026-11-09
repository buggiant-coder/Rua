use std::collections::HashMap;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use rua_workspace::{ComponentState, WorkspaceLifecycleState};

use crate::state::AppState;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HealthReport {
    pub workspaces: HashMap<String, WorkspaceHealth>,

    pub overall: HealthOverall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceHealth {
    pub agent_id: String,

    pub lifecycle: WorkspaceLifecycleState,

    pub services: HashMap<String, ComponentState>,

    pub overall: ComponentState,

    pub active_sessions: usize,

    pub background_tasks: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthOverall {
    #[default]
    Healthy,

    Degraded,

    Unhealthy,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(health_handler))
}

pub async fn health_handler(State(state): State<AppState>) -> Response {
    let summaries = match state.registry.list().await {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(error = %err, "health: registry.list failed");
            let body = serde_json::json!({"detail": err.message});
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response();
        }
    };

    let mut report = HealthReport::default();
    let mut any_failed = false;
    let mut any_degraded = false;

    for summary in summaries {
        let runtime = match state.registry.get(&summary.agent_id).await {
            Some(rt) => rt,
            None => {
                tracing::debug!(
                    agent_id = %summary.agent_id,
                    "health: agent vanished between list and get, skipping"
                );
                continue;
            }
        };

        let workspace_health = runtime.health().await;
        let overall = compute_workspace_overall(&workspace_health.components);

        match overall {
            ComponentState::Failed { .. } => any_failed = true,
            ComponentState::Degraded { .. } => any_degraded = true,
            _ => {}
        }

        report.workspaces.insert(
            summary.agent_id.clone(),
            WorkspaceHealth {
                agent_id: summary.agent_id,
                lifecycle: workspace_health.lifecycle,
                services: workspace_health.components,
                overall,
                active_sessions: workspace_health.active_sessions,
                background_tasks: workspace_health.background_tasks,
            },
        );
    }

    report.overall = if any_failed {
        HealthOverall::Unhealthy
    } else if any_degraded {
        HealthOverall::Degraded
    } else {
        HealthOverall::Healthy
    };

    (StatusCode::OK, Json(report)).into_response()
}

fn compute_workspace_overall(services: &HashMap<String, ComponentState>) -> ComponentState {
    let mut failed: Vec<&str> = Vec::new();
    let mut degraded: Vec<&str> = Vec::new();
    for (name, state) in services {
        match state {
            ComponentState::Failed { .. } => failed.push(name.as_str()),
            ComponentState::Degraded { .. } => degraded.push(name.as_str()),
            _ => {}
        }
    }
    if !failed.is_empty() {
        failed.sort();
        return ComponentState::Failed {
            reason: format!("failed services: {}", failed.join(", ")),
        };
    }
    if !degraded.is_empty() {
        degraded.sort();
        return ComponentState::Degraded {
            reason: format!("degraded services: {}", degraded.join(", ")),
        };
    }
    ComponentState::Ready
}
