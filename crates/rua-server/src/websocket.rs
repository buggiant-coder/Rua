use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::response::Response;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsIncoming {
    Chat {
        text: String,
        #[serde(default = "default_agent_id")]
        agent_id: String,
        #[serde(default)]
        conversation_id: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Map<String, serde_json::Value>,
    },

    Message {
        content: String,
        #[serde(default = "default_agent_id")]
        agent_id: String,
        #[serde(default)]
        conversation_id: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Map<String, serde_json::Value>,
    },

    Ping {
        #[serde(default)]
        timestamp: Option<i64>,
    },
}

fn default_agent_id() -> String {
    "default".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsOutgoing {
    Connected { session_id: String },

    Text { text: String },

    Response { response: serde_json::Value },

    Pong { timestamp: i64 },

    Error { error: String },
}

pub async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state, "default".to_string()))
}

pub async fn ws_handler_agent_scoped(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state, agent_id))
}

async fn handle_ws(socket: WebSocket, state: AppState, default_agent_id: String) {
    let session_id = uuid::Uuid::new_v4().to_string();
    let (mut sender, mut receiver) = socket.split();

    tracing::info!(
        session_id = %session_id,
        agent_id = %default_agent_id,
        "websocket channel session started"
    );

    let connected = WsOutgoing::Connected {
        session_id: session_id.clone(),
    };
    if let Err(e) = send_ws_message(&mut sender, &connected).await {
        tracing::error!(error = %e, "failed to send Connected message");
        return;
    }

    while let Some(msg_result) = receiver.next().await {
        let msg = match msg_result {
            Ok(msg) => msg,
            Err(e) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %e,
                    "websocket receive error"
                );
                break;
            }
        };

        match msg {
            Message::Text(text) => {
                let incoming: WsIncoming = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %e,
                            "invalid websocket frame, skipping"
                        );
                        let err = WsOutgoing::Error {
                            error: format!("invalid message format: {e}"),
                        };
                        let _ = send_ws_message(&mut sender, &err).await;
                        continue;
                    }
                };

                match incoming {
                    WsIncoming::Chat {
                        text,
                        agent_id: _,
                        conversation_id,
                        extra: _,
                    } => {
                        let conv_id = conversation_id.unwrap_or_else(|| session_id.clone());
                        match enqueue_ws_message(&state, &session_id, &conv_id, &text).await {
                            Ok(()) => {
                                let out = WsOutgoing::Response {
                                    response: serde_json::json!({"status": "accepted"}),
                                };
                                if let Err(e) = send_ws_message(&mut sender, &out).await {
                                    tracing::error!(
                                        session_id = %session_id,
                                        error = %e,
                                        "failed to send response"
                                    );
                                    break;
                                }
                            }
                            Err(err_msg) => {
                                let err = WsOutgoing::Error { error: err_msg };
                                let _ = send_ws_message(&mut sender, &err).await;
                            }
                        }
                    }
                    WsIncoming::Message {
                        content,
                        agent_id: _,
                        conversation_id,
                        extra: _,
                    } => {
                        let conv_id = conversation_id.unwrap_or_else(|| session_id.clone());
                        match enqueue_ws_message(&state, &session_id, &conv_id, &content).await {
                            Ok(()) => {
                                let out = WsOutgoing::Response {
                                    response: serde_json::json!({"status": "accepted"}),
                                };
                                if let Err(e) = send_ws_message(&mut sender, &out).await {
                                    tracing::error!(
                                        session_id = %session_id,
                                        error = %e,
                                        "failed to send response"
                                    );
                                    break;
                                }
                            }
                            Err(err_msg) => {
                                let err = WsOutgoing::Error { error: err_msg };
                                let _ = send_ws_message(&mut sender, &err).await;
                            }
                        }
                    }
                    WsIncoming::Ping { timestamp } => {
                        let ts = timestamp.unwrap_or_else(|| Utc::now().timestamp_millis());
                        let pong = WsOutgoing::Pong { timestamp: ts };
                        if let Err(e) = send_ws_message(&mut sender, &pong).await {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %e,
                                "failed to send pong"
                            );
                            break;
                        }
                    }
                }
            }
            Message::Close(_) => {
                tracing::info!(session_id = %session_id, "websocket close frame received");
                break;
            }
            _ => {}
        }
    }

    tracing::info!(session_id = %session_id, "websocket channel session ended");
}

async fn send_ws_message<S>(sender: &mut S, msg: &WsOutgoing) -> Result<(), axum::Error>
where
    S: SinkExt<Message, Error = axum::Error> + Unpin,
{
    let json = serde_json::to_string(msg).unwrap_or_default();
    sender.send(Message::Text(json.into())).await
}

async fn enqueue_ws_message(
    state: &AppState,
    session_id: &str,
    conversation_id: &str,
    text: &str,
) -> Result<(), String> {
    let message_id = uuid::Uuid::new_v4().to_string();

    let push_msg = crate::endpoints::dto::PushMessage {
        id: message_id,
        content: text.to_string(),
        timestamp: Some(Utc::now().to_rfc3339()),
        extra: {
            let mut m = serde_json::Map::new();
            m.insert(
                "channel_id".into(),
                serde_json::Value::String("websocket".into()),
            );
            m.insert(
                "sender_id".into(),
                serde_json::Value::String(session_id.to_string()),
            );
            m.insert(
                "conversation_id".into(),
                serde_json::Value::String(conversation_id.to_string()),
            );
            m
        },
    };
    state.push_message_store.push(push_msg).await;

    let payload = serde_json::json!({
        "channel_id": "websocket",
        "session_id": session_id,
        "conversation_id": conversation_id,
        "text": text,
    });
    let active_agent = AppState::resolve_active_agent_id(&state.rua_root);
    let cm = match state.channel_manager_for(&active_agent).await {
        Some(cm) => cm,
        None => {
            tracing::error!("channel_manager not available for websocket enqueue");
            return Err("channel_manager not available".to_string());
        }
    };
    cm.handle_incoming("websocket".to_string(), session_id.to_string(), payload)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "websocket channel handle_incoming failed");
            format!("handle_incoming failed: {e}")
        })
}
