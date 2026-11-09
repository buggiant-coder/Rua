use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use rua_tunnel::{BinaryManager, CloudflareTunnelDriver, TunnelInfo, TunnelStatus};

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct StartRequest {
    pub local_port: Option<u16>,
}

#[derive(Debug, Serialize)]
pub struct TunnelInfoResponse {
    pub public_url: String,
    pub public_wss_url: String,
    pub local_port: u16,
    pub status: String,
}

impl From<TunnelInfo> for TunnelInfoResponse {
    fn from(info: TunnelInfo) -> Self {
        let status = match info.status {
            TunnelStatus::Running => "running",
            TunnelStatus::Stopped => "stopped",
        };
        Self {
            public_url: info.public_url,
            public_wss_url: info.public_wss_url,
            local_port: info.local_port,
            status: status.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub status: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_wss_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tunnel/start", post(tunnel_start))
        .route("/tunnel/status", get(tunnel_status))
        .route("/tunnel/stop", post(tunnel_stop))
}

fn load_tunnel_local_port(data_dir: &std::path::Path) -> Option<u16> {
    let bytes = std::fs::read(data_dir.join("config.json")).ok()?;
    let root: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let tunnel_val = root.get("tunnel")?;
    let cfg: rua_config::TunnelConfig = serde_json::from_value(tunnel_val.clone()).ok()?;
    cfg.local_port
}

const DEFAULT_TUNNEL_PORT: u16 = 8080;

async fn tunnel_start(State(state): State<AppState>, Json(req): Json<StartRequest>) -> Response {
    let mut guard = state.tunnel_driver.lock().await;

    if let Some(driver) = guard.as_ref() {
        if driver.is_running().await {
            let body = serde_json::json!({"detail": "tunnel already running"});
            return (StatusCode::CONFLICT, Json(body)).into_response();
        }
    }

    let local_port = req
        .local_port
        .or_else(|| load_tunnel_local_port(&state.rua_root))
        .unwrap_or(DEFAULT_TUNNEL_PORT);

    let binary_mgr = BinaryManager::new(state.rua_root.join("tunnel"));
    let driver = CloudflareTunnelDriver::new(binary_mgr);

    match driver.start(local_port).await {
        Ok(info) => {
            let resp = TunnelInfoResponse::from(info);
            *guard = Some(driver);
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => {
            let body = serde_json::json!({"detail": e.to_string()});
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
        }
    }
}

async fn tunnel_status(State(state): State<AppState>) -> Response {
    let guard = state.tunnel_driver.lock().await;
    let alert = state.tunnel_alert_state.read().await;

    let resp = match guard.as_ref() {
        Some(driver) => match driver.get_info().await {
            Some(info) => StatusResponse {
                status: "running".to_string(),
                state: alert.state.clone(),
                started_at: alert.started_at.map(|t| t.to_rfc3339()),
                last_failure_at: alert.last_failure_at.map(|t| t.to_rfc3339()),
                public_url: Some(info.public_url),
                public_wss_url: Some(info.public_wss_url),
                local_port: Some(info.local_port),
            },
            None => StatusResponse {
                status: "disconnected".to_string(),
                state: alert.state.clone(),
                started_at: alert.started_at.map(|t| t.to_rfc3339()),
                last_failure_at: alert.last_failure_at.map(|t| t.to_rfc3339()),
                public_url: None,
                public_wss_url: None,
                local_port: None,
            },
        },
        None => StatusResponse {
            status: "disconnected".to_string(),
            state: alert.state.clone(),
            started_at: alert.started_at.map(|t| t.to_rfc3339()),
            last_failure_at: alert.last_failure_at.map(|t| t.to_rfc3339()),
            public_url: None,
            public_wss_url: None,
            local_port: None,
        },
    };

    (StatusCode::OK, Json(resp)).into_response()
}

async fn tunnel_stop(State(state): State<AppState>) -> Response {
    let mut guard = state.tunnel_driver.lock().await;

    match guard.take() {
        Some(driver) => {
            driver.stop().await;
            let body = serde_json::json!({"status": "stopped"});
            (StatusCode::OK, Json(body)).into_response()
        }
        None => {
            let body = serde_json::json!({"status": "stopped"});
            (StatusCode::OK, Json(body)).into_response()
        }
    }
}
