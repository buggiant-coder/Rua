use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use rua_workspace::WorkspaceRuntime;

use crate::extractors::AgentExtractorContext;
use crate::state::AppState;

const AGENT_ID_HEADER: &str = "x-agent-id";

const CHANNEL_HEADER: &str = "x-channel";

const DEFAULT_AGENT_ID: &str = "default";

pub async fn agent_context_middleware(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let explicit = extract_explicit_id(&req);
    let session_id = extract_session_id(&req);
    let channel = extract_channel(&req);

    let agent_id = resolve_agent_id(explicit.as_deref());

    let runtime: Arc<WorkspaceRuntime> = match state.registry.get_or_create(&agent_id).await {
        Ok(rt) => rt,
        Err(err) => {
            tracing::error!(
                agent_id = %agent_id,
                path = %req.uri().path(),
                error = %err,
                "agent_context_middleware: registry.get_or_create failed"
            );
            let body = serde_json::json!({
                "detail": format!("Agent '{}' is unavailable", agent_id),
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response();
        }
    };

    tracing::trace!(
        agent_id = %agent_id,
        path = %req.uri().path(),
        explicit = ?explicit,
        session_id = ?session_id,
        channel = ?channel,
        "agent_context_middleware: resolved agent"
    );

    req.extensions_mut()
        .insert(AgentExtractorContext { agent_id, runtime });

    next.run(req).await
}

fn resolve_agent_id(explicit: Option<&str>) -> String {
    if let Some(id) = explicit {
        let trimmed = id.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    DEFAULT_AGENT_ID.to_string()
}

pub(crate) fn extract_explicit_id(req: &Request<Body>) -> Option<String> {
    if let Some(value) = req
        .headers()
        .get(AGENT_ID_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    query_param(req, "agent_id")
}

pub(crate) fn extract_session_id(req: &Request<Body>) -> Option<String> {
    let path = req.uri().path();
    let mut segments = path.split('/').filter(|seg| !seg.is_empty()).peekable();

    while let Some(seg) = segments.next() {
        if matches!(seg, "sessions" | "chats") {
            if let Some(next) = segments.peek() {
                let candidate = (*next).to_string();
                if !candidate.is_empty() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

pub(crate) fn extract_channel(req: &Request<Body>) -> Option<String> {
    if let Some(value) = req
        .headers()
        .get(CHANNEL_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let path = req.uri().path();
    let mut segments = path.split('/').filter(|seg| !seg.is_empty()).peekable();
    while let Some(seg) = segments.next() {
        if seg == "channels" {
            if let Some(next) = segments.peek() {
                let candidate = (*next).to_string();
                if !candidate.is_empty() {
                    return Some(candidate);
                }
            }
        }
    }

    query_param(req, "channel")
}

fn query_param(req: &Request<Body>, key: &str) -> Option<String> {
    let query = req.uri().query()?;
    for pair in query.split('&') {
        let mut split = pair.splitn(2, '=');
        let k = split.next()?;
        if k != key {
            continue;
        }
        let v = split.next().unwrap_or("");
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}
