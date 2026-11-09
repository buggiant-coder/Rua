use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Extension;
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::endpoints::dto;
use crate::errors::{ApiError, ValidationError};
use crate::middleware::AgentExtractorContext;
use crate::state::AppState;

const PUSH_MESSAGES_FILE: &str = "push_messages.json";
const PUSH_MESSAGES_CAPACITY: usize = 500;
const DELIVERY_CLASS_EPHEMERAL: &str = "ephemeral";

#[derive(Debug, Clone)]
pub struct PushMessageStore {
    messages: Arc<Mutex<Vec<dto::PushMessage>>>,
    data_dir: Option<PathBuf>,
}

impl PushMessageStore {
    pub fn new(data_dir: Option<PathBuf>) -> Self {
        let mut messages = if let Some(ref dir) = data_dir {
            let path = dir.join(PUSH_MESSAGES_FILE);
            match std::fs::read_to_string(&path) {
                Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
                Err(_) => vec![],
            }
        } else {
            vec![]
        };
        prune_push_messages(&mut messages);
        Self {
            messages: Arc::new(Mutex::new(messages)),
            data_dir,
        }
    }

    pub async fn push(&self, msg: dto::PushMessage) {
        if is_ephemeral_push_message(&msg) {
            return;
        }
        let mut msgs = self.messages.lock().await;
        if !msg.id.is_empty() && msgs.iter().any(|existing| existing.id == msg.id) {
            return;
        }
        msgs.push(msg);
        prune_push_messages(&mut msgs);
        if let Some(ref dir) = self.data_dir {
            let path = dir.join(PUSH_MESSAGES_FILE);
            let value = serde_json::to_value(&*msgs).unwrap_or_else(|_| serde_json::json!([]));
            let _ = rua_config::atomic_write_json(&path, &value);
        }
    }

    pub async fn list(&self) -> Vec<dto::PushMessage> {
        self.messages.lock().await.clone()
    }
}

fn is_ephemeral_push_message(msg: &dto::PushMessage) -> bool {
    msg.extra
        .get("delivery_class")
        .or_else(|| msg.extra.get("persistence"))
        .and_then(|value| value.as_str())
        .is_some_and(|value| value == DELIVERY_CLASS_EPHEMERAL)
}

fn prune_push_messages(messages: &mut Vec<dto::PushMessage>) {
    if messages.len() > PUSH_MESSAGES_CAPACITY {
        let remove_count = messages.len() - PUSH_MESSAGES_CAPACITY;
        messages.drain(0..remove_count);
    }
}

impl Default for PushMessageStore {
    fn default() -> Self {
        Self::new(None)
    }
}

fn build_push_message(
    agent_id: Option<&str>,
    req: &dto::MessageSendRequest,
) -> Result<(String, dto::PushMessage), ()> {
    let msg_id = uuid::Uuid::new_v4().to_string();
    let mut extra = serde_json::Map::new();
    if let Some(aid) = agent_id {
        extra.insert(
            "agent_id".into(),
            serde_json::Value::String(aid.to_string()),
        );
    }
    if let Some(ch) = &req.channel {
        extra.insert("channel".into(), serde_json::Value::String(ch.clone()));
    }
    for key in ["target_user", "target_session", "session_id"] {
        if let Some(value) = req.extra.get(key) {
            extra.insert(key.into(), value.clone());
        }
    }
    if !extra.contains_key("session_id") {
        if let Some(target_session) = req.extra.get("target_session") {
            extra.insert("session_id".into(), target_session.clone());
        }
    }
    let content = if req.message.is_empty() {
        req.extra
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    } else {
        req.message.clone()
    };
    if content.trim().is_empty() {
        return Err(());
    }
    let push_msg = dto::PushMessage {
        id: msg_id.clone(),
        content,
        timestamp: Some(chrono::Utc::now().to_rfc3339()),
        extra,
    };
    Ok((msg_id, push_msg))
}

fn message_required_response() -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({
            "detail": "message or text is required"
        })),
    )
        .into_response()
}

fn json_validation_response(err: JsonRejection) -> Response {
    ApiError::Validation(vec![ValidationError {
        loc: vec!["body".into()],
        msg: err.body_text(),
        error_type: "value_error.json".into(),
    }])
    .into_response()
}

fn ack_response(msg_id: String) -> Response {
    (
        StatusCode::OK,
        Json(dto::MessageSendResponse {
            success: true,
            message_id: Some(msg_id),
            extra: serde_json::Map::new(),
        }),
    )
        .into_response()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/messages/send", post(send_message))
        .route("/messages/push", post(push_message))
        .route("/agents/{agentId}/messages/send", post(send_message_agent))
        .route("/agents/{agentId}/messages/push", post(push_message_agent))
}

async fn send_message(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    body: Result<Json<dto::MessageSendRequest>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(err) => return json_validation_response(err),
    };
    let (msg_id, push_msg) = match build_push_message(Some(&ctx.agent_id), &body) {
        Ok(value) => value,
        Err(()) => return message_required_response(),
    };
    state.push_message_store.push(push_msg).await;
    ack_response(msg_id)
}

async fn push_message(
    State(state): State<AppState>,
    Extension(ctx): Extension<AgentExtractorContext>,
    body: Result<Json<dto::MessageSendRequest>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(err) => return json_validation_response(err),
    };
    let (msg_id, push_msg) = match build_push_message(Some(&ctx.agent_id), &body) {
        Ok(value) => value,
        Err(()) => return message_required_response(),
    };
    state.push_message_store.push(push_msg).await;
    ack_response(msg_id)
}

async fn send_message_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    body: Result<Json<dto::MessageSendRequest>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(err) => return json_validation_response(err),
    };
    let (msg_id, push_msg) = match build_push_message(Some(&agent_id), &body) {
        Ok(value) => value,
        Err(()) => return message_required_response(),
    };
    state.push_message_store.push(push_msg).await;
    ack_response(msg_id)
}

async fn push_message_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    body: Result<Json<dto::MessageSendRequest>, JsonRejection>,
) -> Response {
    let Json(body) = match body {
        Ok(body) => body,
        Err(err) => return json_validation_response(err),
    };
    let (msg_id, push_msg) = match build_push_message(Some(&agent_id), &body) {
        Ok(value) => value,
        Err(()) => return message_required_response(),
    };
    state.push_message_store.push(push_msg).await;
    ack_response(msg_id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebhookPathParams {
    channel_type: String,
    #[serde(default)]
    #[allow(dead_code)]
    path: Option<String>,
}

pub fn channel_webhook_router() -> Router<AppState> {
    Router::new()
        .route(
            "/channel/{channelType}/webhook",
            post(local_channel_webhook),
        )
        .route(
            "/channel/{channelType}/webhook/{path}",
            post(local_channel_webhook).get(local_channel_webhook_get),
        )
}

async fn local_channel_webhook(
    State(state): State<AppState>,
    Path(params): Path<WebhookPathParams>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let text = extract_text_from_webhook(&body);
    let msg_id = body
        .get("msg_id")
        .or_else(|| body.get("msgId"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let push_msg = dto::PushMessage {
        id: msg_id,
        content: text,
        timestamp: Some(chrono::Utc::now().to_rfc3339()),
        extra: {
            let mut m = serde_json::Map::new();
            m.insert(
                "channel_id".into(),
                serde_json::Value::String(params.channel_type.clone()),
            );
            m
        },
    };
    state.push_message_store.push(push_msg).await;

    tracing::info!(
        channel = %params.channel_type,
        "local channel webhook: message stored"
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "received",
            "channel": params.channel_type,
        })),
    )
        .into_response()
}

async fn local_channel_webhook_get(Path(params): Path<WebhookPathParams>) -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(serde_json::json!({
            "detail": "Webhook endpoint only accepts POST requests",
            "channel": params.channel_type,
        })),
    )
        .into_response()
}

fn extract_text_from_webhook(body: &serde_json::Value) -> String {
    if let Some(text) = body.get("msg").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    if let Some(text) = body.get("text").and_then(|v| {
        v.as_str().map(|s| s.to_string()).or_else(|| {
            v.get("content")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string())
        })
    }) {
        return text;
    }
    if let Some(text) = body.get("content").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    if let Some(text) = body.get("message").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    body.to_string()
}
