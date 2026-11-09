use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use mcp_manager::status::McpConnectionState;
use serde::Serialize;

use rua_mcp::McpClientStore;

use crate::errors::ApiError;
use crate::extractors::AgentExtractorContext;
use crate::state::AppState;

#[derive(Serialize)]
struct McpStatusEntry {
    server_name: String,
    status: &'static str,
    last_error: Option<String>,
    auth: McpAuthStatus,
}

#[derive(Serialize)]
struct McpAuthStatus {
    kind: &'static str,
    required: bool,
    authorized: bool,
    expired: bool,
    expires_at: f64,
    scope: String,
    client_id: String,
    reason: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/mcp/status", get(mcp_status))
}

async fn mcp_status(
    State(state): State<AppState>,
    ctx: Option<Extension<AgentExtractorContext>>,
) -> Response {
    let agent_id = ctx
        .map(|Extension(ctx)| ctx.agent_id)
        .unwrap_or_else(|| "default".to_string());
    let manager = match state.mcp_manager_for(&agent_id).await {
        Some(m) => m,
        None => return (StatusCode::OK, Json(Vec::<McpStatusEntry>::new())).into_response(),
    };

    match manager.list_servers().await {
        Ok(servers) => {
            let store = build_store(&state).await.ok();
            let mut entries = Vec::with_capacity(servers.len());
            for s in servers {
                let auth = auth_status_for(&store, &agent_id, &s.key, &s.last_error).await;
                entries.push(McpStatusEntry {
                    server_name: s.key,
                    status: state_to_str(s.state),
                    last_error: s.last_error,
                    auth,
                });
            }
            (StatusCode::OK, Json(entries)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn build_store(state: &AppState) -> Result<McpClientStore, ApiError> {
    let runtime = state.registry.get_or_create("default").await.map_err(|e| {
        ApiError::Internal(format!(
            "failed to obtain workspace runtime for MCP status store: {e}"
        ))
    })?;
    Ok(McpClientStore::with_agent_mcp_cells(
        runtime.rua_root(),
        std::sync::Arc::clone(&state.agent_mcp_cells),
    ))
    .map(|store| store.with_device_key(*state.device_key.as_bytes()))
}

async fn auth_status_for(
    store: &Option<McpClientStore>,
    agent_id: &str,
    client_key: &str,
    last_error: &Option<String>,
) -> McpAuthStatus {
    let auth_error = last_error.as_deref().is_some_and(is_auth_error);
    let Some(store) = store else {
        return McpAuthStatus {
            kind: if auth_error { "oauth2" } else { "none" },
            required: auth_error,
            authorized: false,
            expired: false,
            expires_at: 0.0,
            scope: String::new(),
            client_id: String::new(),
            reason: auth_error.then(|| "auth_failed".to_string()),
        };
    };

    let client = store
        .get_client_for_execution(Some(agent_id), client_key)
        .await
        .ok()
        .flatten();
    let Some(client) = client else {
        return McpAuthStatus {
            kind: if auth_error { "oauth2" } else { "none" },
            required: auth_error,
            authorized: false,
            expired: false,
            expires_at: 0.0,
            scope: String::new(),
            client_id: String::new(),
            reason: auth_error.then(|| "auth_failed".to_string()),
        };
    };

    let remote = matches!(
        client.server.transport,
        mcp_manager::types::McpTransportConfig::StreamableHttp { .. }
            | mcp_manager::types::McpTransportConfig::Sse { .. }
    );
    let Some(oauth) = client.server.oauth else {
        return McpAuthStatus {
            kind: if auth_error { "oauth2" } else { "none" },
            required: remote && auth_error,
            authorized: false,
            expired: false,
            expires_at: 0.0,
            scope: String::new(),
            client_id: String::new(),
            reason: auth_error.then(|| "auth_failed".to_string()),
        };
    };

    let authorized = oauth.has_valid_access_token();
    let expired = !oauth.access_token.is_empty() && !authorized;
    let reason = if expired {
        Some("expired".to_string())
    } else if auth_error {
        Some("auth_failed".to_string())
    } else {
        None
    };
    McpAuthStatus {
        kind: "oauth2",
        required: remote,
        authorized,
        expired,
        expires_at: oauth.expires_at,
        scope: oauth.scope,
        client_id: oauth.client_id,
        reason,
    }
}

fn is_auth_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("auth failed")
        || lower.contains("unauthorized")
        || lower.contains("http 401")
        || lower.contains("http 403")
}

fn state_to_str(state: McpConnectionState) -> &'static str {
    match state {
        McpConnectionState::Disabled => "disabled",
        McpConnectionState::NotConnected => "not_connected",
        McpConnectionState::Connecting => "connecting",
        McpConnectionState::Ready => "ready",
        McpConnectionState::Degraded => "degraded",
        McpConnectionState::Failed => "failed",
        McpConnectionState::Stopping => "stopping",
    }
}
