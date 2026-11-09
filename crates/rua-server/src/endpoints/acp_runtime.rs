use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{stream, StreamExt};
use rua_acp::remote::{AcpRuntimeCancelResponse, AcpRuntimeStreamItem, AcpRuntimeTurnRequest};
use rua_contract::agent::AgentRuntime;
use rua_core::RuaError;
use tokio::sync::watch;

use crate::errors::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AcpRuntimeSessionKey {
    agent_id: String,
    session_id: String,
}

impl AcpRuntimeSessionKey {
    fn new(agent_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
        }
    }
}

type ActiveTurnKey = AcpRuntimeSessionKey;

static ACTIVE_TURNS: OnceLock<Mutex<HashMap<ActiveTurnKey, watch::Sender<bool>>>> = OnceLock::new();

fn active_turns() -> &'static Mutex<HashMap<ActiveTurnKey, watch::Sender<bool>>> {
    ACTIVE_TURNS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct ActiveTurnGuard {
    key: ActiveTurnKey,
    cancel_tx: watch::Sender<bool>,
    mcp_config_handle: Option<String>,
}

impl Drop for ActiveTurnGuard {
    fn drop(&mut self) {
        let _ = self.cancel_tx.send(true);
        active_turns()
            .lock()
            .expect("ACP runtime active-turn registry poisoned")
            .remove(&self.key);
        if let Some(handle) = &self.mcp_config_handle {
            let _ = rua_acp::server::take_request_mcp_config(handle);
        }
    }
}

struct RuntimeStreamState {
    events: rua_contract::agent::RuntimeCommandStream,
    _guard: ActiveTurnGuard,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent_id}/acp/runtime/health", get(runtime_health))
        .route("/agents/{agent_id}/acp/runtime/turn", post(run_turn))
        .route(
            "/agents/{agent_id}/acp/runtime/{session_id}/cancel",
            post(cancel_turn),
        )
}

async fn runtime_health(State(state): State<AppState>, Path(agent_id): Path<String>) -> Response {
    let runtime = match state.registry.get_or_create(&agent_id).await {
        Ok(runtime) => runtime,
        Err(_) => return ApiError::NotFound(agent_id).into_response(),
    };
    if runtime.react_runtime().await.is_none() {
        return ApiError::Unavailable("react runtime not available".into()).into_response();
    }
    (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
}

async fn run_turn(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(mut body): Json<AcpRuntimeTurnRequest>,
) -> Response {
    if body.request.agent_id.as_str() != agent_id {
        return ApiError::BadRequest("request agent_id does not match route".into())
            .into_response();
    }

    let workspace = match state.registry.get_or_create(&agent_id).await {
        Ok(runtime) => runtime,
        Err(_) => return ApiError::NotFound(agent_id).into_response(),
    };
    let runtime: Arc<dyn AgentRuntime> = match workspace.react_runtime().await {
        Some(runtime) => runtime,
        None => {
            return ApiError::Unavailable("react runtime not available".into()).into_response();
        }
    };

    let session_id = body.request.session_id.to_string();
    let key = AcpRuntimeSessionKey::new(&agent_id, session_id);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    {
        let mut active = active_turns()
            .lock()
            .expect("ACP runtime active-turn registry poisoned");
        if active.contains_key(&key) {
            return ApiError::Conflict("ACP session already has an active prompt".into())
                .into_response();
        }
        active.insert(key.clone(), cancel_tx.clone());
    }

    body.request.cancel = Some(cancel_rx);
    body.request.workspace_dir = Some(body.workspace_dir);
    body.request
        .metadata
        .remove(rua_acp::server::ACP_MCP_CONFIG_HANDLE_META_KEY);
    let mcp_config_handle = body
        .mcp_config
        .map(rua_acp::server::register_request_mcp_config);
    if let Some(handle) = &mcp_config_handle {
        body.request.metadata.insert(
            rua_acp::server::ACP_MCP_CONFIG_HANDLE_META_KEY.to_string(),
            serde_json::Value::String(handle.clone()),
        );
    }

    let guard = ActiveTurnGuard {
        key,
        cancel_tx,
        mcp_config_handle,
    };
    let events = match runtime.stream(body.request).await {
        Ok(events) => events,
        Err(error) => {
            drop(guard);
            return ApiError::Internal(error.to_string()).into_response();
        }
    };

    let output = stream::unfold(
        RuntimeStreamState {
            events,
            _guard: guard,
        },
        |mut state| async move {
            let item = state.events.next().await?;
            let wire = match item {
                Ok(event) => AcpRuntimeStreamItem::Event {
                    event: Box::new(event),
                },
                Err(error) => AcpRuntimeStreamItem::Error { error },
            };
            let mut bytes = match serde_json::to_vec(&wire) {
                Ok(bytes) => bytes,
                Err(error) => serde_json::to_vec(&AcpRuntimeStreamItem::Error {
                    error: RuaError::Runtime {
                        message: format!("serialize ACP runtime event: {error}"),
                    },
                })
                .expect("RuaError must serialize"),
            };
            bytes.push(b'\n');
            Some((Ok::<Bytes, Infallible>(Bytes::from(bytes)), state))
        },
    );

    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(output))
        .expect("ACP runtime stream response")
}

async fn cancel_turn(
    State(state): State<AppState>,
    Path((agent_id, session_id)): Path<(String, String)>,
) -> Response {
    let key = AcpRuntimeSessionKey::new(&agent_id, &session_id);
    let cancel_tx = active_turns()
        .lock()
        .expect("ACP runtime active-turn registry poisoned")
        .get(&key)
        .cloned();
    let cancelled = cancel_tx.is_some_and(|tx| tx.send(true).is_ok());

    if let Ok(workspace) = state.registry.get_or_create(&agent_id).await {
        if let Some(security) = workspace.security_module().await {
            security
                .approval_authority()
                .clear_by_runtime_session(&agent_id, &session_id)
                .await;
        }
    }

    (StatusCode::OK, Json(AcpRuntimeCancelResponse { cancelled })).into_response()
}
