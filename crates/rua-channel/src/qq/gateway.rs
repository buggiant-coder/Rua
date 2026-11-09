use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::http_client::default_http_client;
use crate::qq::api::{get_gateway_url, QqTokenManager};
use crate::qq::types::*;

pub type OnMessageFn = Arc<dyn Fn(String, serde_json::Value) + Send + Sync>;

pub(super) fn spawn_ws_task_with_limit(
    token_manager: Arc<QqTokenManager>,
    sandbox: bool,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
    max_attempts: i64,
) -> JoinHandle<()> {
    tokio::spawn(ws_run_loop(
        token_manager,
        sandbox,
        on_message,
        stop_rx,
        max_attempts,
    ))
}

struct WsState {
    session_id: Option<String>,
    last_seq: Option<u64>,
    reconnect_attempts: usize,
    should_refresh_token: bool,
    max_attempts: i64,
}

impl WsState {
    fn new(max_attempts: i64) -> Self {
        Self {
            session_id: None,
            last_seq: None,
            reconnect_attempts: 0,
            should_refresh_token: false,
            max_attempts,
        }
    }
}

async fn ws_run_loop(
    token_manager: Arc<QqTokenManager>,
    sandbox: bool,
    on_message: OnMessageFn,
    mut stop_rx: watch::Receiver<bool>,
    max_attempts: i64,
) {
    let http = default_http_client();
    let mut state = WsState::new(max_attempts);

    loop {
        if *stop_rx.borrow() {
            tracing::info!("qq WS loop: stop signal received");
            return;
        }

        if state.should_refresh_token {
            token_manager.clear_cache().await;
            state.should_refresh_token = false;
        }

        let token = match token_manager.get_access_token().await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("qq get token failed: {e}");
                if !wait_reconnect(&mut state, &mut stop_rx).await {
                    return;
                }
                continue;
            }
        };

        let ws_url = match get_gateway_url(&http, &token, sandbox).await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("qq get gateway URL failed: {e}");
                if !wait_reconnect(&mut state, &mut stop_rx).await {
                    return;
                }
                continue;
            }
        };

        tracing::info!("qq WS: connecting to {ws_url}");

        let ws_stream = match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((stream, _)) => {
                tracing::info!("qq WS: connected");
                state.reconnect_attempts = 0;
                stream
            }
            Err(e) => {
                tracing::error!("qq WS connect error: {e}");
                if !wait_reconnect(&mut state, &mut stop_rx).await {
                    return;
                }
                continue;
            }
        };

        let should_reconnect =
            run_ws_session(ws_stream, &token, &on_message, &mut state, &mut stop_rx).await;

        if !should_reconnect || *stop_rx.borrow() {
            return;
        }

        if !wait_reconnect(&mut state, &mut stop_rx).await {
            return;
        }
    }
}

async fn run_ws_session(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    token: &str,
    on_message: &OnMessageFn,
    state: &mut WsState,
    stop_rx: &mut watch::Receiver<bool>,
) -> bool {
    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (hb_tx, mut hb_rx) = tokio::sync::mpsc::channel::<String>(4);
    let mut heartbeat_handle: Option<JoinHandle<()>> = None;

    let should_reconnect = loop {
        tokio::select! {
            msg = ws_read.next() => {
                let text = match msg {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(_))) => {
                        tracing::info!("qq WS: server closed connection");
                        break true;
                    }
                    Some(Err(e)) => {
                        tracing::error!("qq WS read error: {e}");
                        break true;
                    }
                    None => {
                        tracing::info!("qq WS: stream ended");
                        break true;
                    }
                    _ => continue,
                };

                let payload: GatewayPayload = match serde_json::from_str(&text) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("qq WS: invalid payload: {e}");
                        continue;
                    }
                };

                if let Some(s) = payload.s {
                    state.last_seq = Some(s);
                }

                match handle_op(payload, token, state, &hb_tx, &mut heartbeat_handle, on_message) {
                    OpAction::Continue => {}
                    OpAction::Send(msg_str) => {
                        if let Err(e) = ws_write.send(Message::Text(msg_str.into())).await {
                            tracing::error!("qq WS send error: {e}");
                            break true;
                        }
                    }
                    OpAction::Reconnect => break true,
                }
            }

            Some(hb_msg) = hb_rx.recv() => {
                if let Err(e) = ws_write.send(Message::Text(hb_msg.into())).await {
                    tracing::error!("qq WS heartbeat send error: {e}");
                    break true;
                }
                tracing::debug!("qq heartbeat sent");
            }

            _ = stop_rx.changed() => {
                tracing::info!("qq WS: stop signal received");
                break false;
            }
        }
    };

    if let Some(h) = heartbeat_handle.take() {
        h.abort();
    }

    should_reconnect
}

enum OpAction {
    Continue,
    Send(String),
    Reconnect,
}

fn handle_op(
    payload: GatewayPayload,
    token: &str,
    state: &mut WsState,
    hb_tx: &tokio::sync::mpsc::Sender<String>,
    heartbeat_handle: &mut Option<JoinHandle<()>>,
    on_message: &OnMessageFn,
) -> OpAction {
    match payload.op {
        OP_HELLO => {
            let interval = payload
                .d
                .as_ref()
                .and_then(|d| serde_json::from_value::<HelloData>(d.clone()).ok())
                .map(|h| h.heartbeat_interval)
                .unwrap_or(45000);

            let outgoing = if state.session_id.is_some() && state.last_seq.is_some() {
                OutgoingPayload {
                    op: OP_RESUME,
                    d: serde_json::json!({
                        "token": format!("QQBot {token}"),
                        "session_id": state.session_id.as_deref().unwrap_or(""),
                        "seq": state.last_seq.unwrap_or(0),
                    }),
                }
            } else {
                let intents = INTENT_PUBLIC_GUILD_MESSAGES
                    | INTENT_GUILD_MEMBERS
                    | INTENT_INTERACTION
                    | INTENT_DIRECT_MESSAGE
                    | INTENT_GROUP_AND_C2C;

                OutgoingPayload {
                    op: OP_IDENTIFY,
                    d: serde_json::json!({
                        "token": format!("QQBot {token}"),
                        "intents": intents,
                        "shard": [0, 1],
                    }),
                }
            };

            if let Some(h) = heartbeat_handle.take() {
                h.abort();
            }
            let tx = hb_tx.clone();
            *heartbeat_handle = Some(tokio::spawn(async move {
                let dur = Duration::from_millis(interval);
                let mut ticker = tokio::time::interval(dur);
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    let p = serde_json::json!({"op": OP_HEARTBEAT, "d": null});
                    if tx.send(serde_json::to_string(&p).unwrap()).await.is_err() {
                        return;
                    }
                }
            }));

            OpAction::Send(serde_json::to_string(&outgoing).unwrap())
        }

        OP_DISPATCH => {
            let event_type = payload.t.as_deref().unwrap_or("");
            let data = payload.d.unwrap_or(serde_json::Value::Null);

            match event_type {
                "READY" => {
                    state.session_id = data
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    state.reconnect_attempts = 0;
                    tracing::info!("qq READY session_id={:?}", state.session_id);
                }
                "RESUMED" => {
                    state.reconnect_attempts = 0;
                    tracing::info!("qq session resumed");
                }
                t if get_event_spec(t).is_some() => {
                    on_message(t.to_string(), data);
                }
                other => {
                    tracing::debug!("qq dispatch event ignored: {other}");
                }
            }
            OpAction::Continue
        }

        OP_HEARTBEAT_ACK => {
            tracing::debug!("qq heartbeat ack");
            OpAction::Continue
        }

        OP_RECONNECT => {
            tracing::info!("qq server requested reconnect");
            OpAction::Reconnect
        }

        OP_INVALID_SESSION => {
            let can_resume = payload
                .d
                .as_ref()
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            tracing::error!("qq invalid session can_resume={can_resume}");
            if !can_resume {
                state.session_id = None;
                state.last_seq = None;
                state.should_refresh_token = true;
            }
            OpAction::Reconnect
        }

        other => {
            tracing::debug!("qq WS: unknown op={other}");
            OpAction::Continue
        }
    }
}

async fn wait_reconnect(state: &mut WsState, stop_rx: &mut watch::Receiver<bool>) -> bool {
    if state.max_attempts >= 0 && state.reconnect_attempts as i64 >= state.max_attempts {
        tracing::error!("qq max reconnect attempts reached");
        return false;
    }

    let delay = reconnect_delay(state);
    state.reconnect_attempts += 1;

    tracing::info!(
        "qq reconnecting in {}s (attempt {})",
        delay.as_secs(),
        state.reconnect_attempts
    );

    tokio::select! {
        _ = tokio::time::sleep(delay) => true,
        _ = stop_rx.changed() => false,
    }
}

fn reconnect_delay(state: &WsState) -> Duration {
    let idx = state.reconnect_attempts.min(RECONNECT_DELAYS.len() - 1);
    Duration::from_secs(RECONNECT_DELAYS[idx])
}
