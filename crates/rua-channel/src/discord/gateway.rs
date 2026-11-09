use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

const GUILD_MESSAGES: u64 = 1 << 9;
const DIRECT_MESSAGES: u64 = 1 << 12;
const MESSAGE_CONTENT: u64 = 1 << 15;

pub const DEFAULT_INTENTS: u64 = GUILD_MESSAGES | DIRECT_MESSAGES | MESSAGE_CONTENT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    Dispatch = 0,
    Heartbeat = 1,
    Identify = 2,
    Resume = 6,
    Reconnect = 7,
    InvalidSession = 9,
    Hello = 10,
    HeartbeatAck = 11,
}

impl OpCode {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Dispatch),
            1 => Some(Self::Heartbeat),
            2 => Some(Self::Identify),
            6 => Some(Self::Resume),
            7 => Some(Self::Reconnect),
            9 => Some(Self::InvalidSession),
            10 => Some(Self::Hello),
            11 => Some(Self::HeartbeatAck),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GatewayPayload {
    pub op: u8,
    #[serde(default)]
    pub d: Option<serde_json::Value>,
    #[serde(default)]
    pub s: Option<u64>,
    #[serde(default)]
    pub t: Option<String>,
}

#[derive(Debug, Serialize)]
struct IdentifyPayload {
    token: String,
    intents: u64,
    properties: IdentifyProperties,
}

#[derive(Debug, Serialize)]
struct IdentifyProperties {
    os: String,
    browser: String,
    device: String,
}

#[derive(Debug, Serialize)]
struct ResumePayload {
    token: String,
    session_id: String,
    seq: u64,
}

#[derive(Debug, Deserialize)]
pub struct HelloData {
    pub heartbeat_interval: u64,
}

#[derive(Debug, Deserialize)]
pub struct ReadyData {
    pub session_id: String,
    #[serde(default)]
    pub resume_gateway_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MessageCreateData {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub guild_id: Option<String>,
    pub author: MessageAuthor,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<MessageAttachment>,
    #[serde(default)]
    pub mentions: Vec<MessageAuthor>,
    #[serde(default)]
    pub mention_everyone: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MessageAuthor {
    pub id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub bot: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MessageAttachment {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub local_path: Option<String>,
}

pub type OnMessageFn = Arc<dyn Fn(MessageCreateData) + Send + Sync>;

pub struct GatewayConfig {
    pub bot_token: String,
    pub intents: u64,
}

pub fn spawn_gateway_task(
    config: GatewayConfig,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    spawn_gateway_task_with_client(
        config,
        on_message,
        stop_rx,
        crate::http_client::default_http_client(),
        GATEWAY_URL.to_string(),
    )
}

pub(super) fn spawn_gateway_task_with_client(
    config: GatewayConfig,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
    client: reqwest::Client,
    gateway_url: String,
) -> JoinHandle<()> {
    tokio::spawn(gateway_run_loop(
        config,
        on_message,
        stop_rx,
        client,
        gateway_url,
    ))
}

struct SessionState {
    session_id: String,
    resume_url: Option<String>,
    sequence: Option<u64>,
}

async fn gateway_run_loop(
    config: GatewayConfig,
    on_message: OnMessageFn,
    mut stop_rx: watch::Receiver<bool>,
    client: reqwest::Client,
    gateway_url: String,
) {
    let mut backoff = INITIAL_BACKOFF;
    let mut session: Option<SessionState> = None;

    loop {
        if *stop_rx.borrow() {
            tracing::info!("discord gateway: stop signal received before connect");
            return;
        }

        let url = session
            .as_ref()
            .and_then(|s| s.resume_url.as_deref())
            .unwrap_or(&gateway_url);
        let connect_url = if url.contains('?') {
            url.to_string()
        } else {
            format!("{url}?v=10&encoding=json")
        };

        tracing::info!("discord gateway: connecting to {connect_url}");

        let connection = tokio::select! {
            _ = stop_rx.changed() => return,
            connection = crate::http_client::connect_websocket(&client, &connect_url) => connection,
        };
        let ws_stream = match connection {
            Ok(stream) => {
                tracing::info!("discord gateway: connected");
                backoff = INITIAL_BACKOFF;
                stream
            }
            Err(e) => {
                tracing::error!("discord gateway connect error: {e}, retry in {backoff:?}");
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = stop_rx.changed() => { return; }
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        let result = handle_gateway_session(
            ws_stream,
            &config,
            &on_message,
            &mut stop_rx,
            session.take(),
        )
        .await;

        match result {
            SessionResult::Shutdown => {
                tracing::info!("discord gateway: graceful shutdown");
                return;
            }
            SessionResult::Reconnect(state) => {
                session = state;
                tracing::warn!("discord gateway: disconnected, reconnecting in {backoff:?}");
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = stop_rx.changed() => { return; }
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
            SessionResult::ReconnectImmediate(state) => {
                session = state;
                tracing::info!("discord gateway: server requested reconnect");
                backoff = INITIAL_BACKOFF;
            }
        }
    }
}

enum SessionResult {
    Shutdown,
    Reconnect(Option<SessionState>),
    ReconnectImmediate(Option<SessionState>),
}

async fn handle_gateway_session(
    ws_stream: tokio_tungstenite::WebSocketStream<reqwest::Upgraded>,
    config: &GatewayConfig,
    on_message: &OnMessageFn,
    stop_rx: &mut watch::Receiver<bool>,
    prev_session: Option<SessionState>,
) -> SessionResult {
    let (mut sink, mut stream) = ws_stream.split();

    let first = tokio::select! {
        _ = stop_rx.changed() => return SessionResult::Shutdown,
        first = read_next_payload(&mut stream) => first,
    };
    let hello = match first {
        Some(payload) if payload.op == OpCode::Hello as u8 => payload,
        _ => {
            tracing::error!("discord gateway: expected Hello, got something else");
            return SessionResult::Reconnect(None);
        }
    };

    let heartbeat_interval = hello
        .d
        .as_ref()
        .and_then(|d| serde_json::from_value::<HelloData>(d.clone()).ok())
        .map(|h| Duration::from_millis(h.heartbeat_interval))
        .unwrap_or(Duration::from_secs(41));

    tracing::debug!("discord gateway: heartbeat interval {heartbeat_interval:?}");

    let is_resume = prev_session.is_some();
    if let Some(ref state) = prev_session {
        let resume = serde_json::json!({
            "op": OpCode::Resume as u8,
            "d": ResumePayload {
                token: config.bot_token.clone(),
                session_id: state.session_id.clone(),
                seq: state.sequence.unwrap_or(0),
            }
        });
        if sink
            .send(Message::Text(
                serde_json::to_string(&resume).unwrap().into(),
            ))
            .await
            .is_err()
        {
            return SessionResult::Reconnect(None);
        }
        tracing::info!("discord gateway: sent Resume");
    } else {
        let identify = serde_json::json!({
            "op": OpCode::Identify as u8,
            "d": IdentifyPayload {
                token: config.bot_token.clone(),
                intents: config.intents,
                properties: IdentifyProperties {
                    os: "linux".into(),
                    browser: "rua-rust".into(),
                    device: "rua-rust".into(),
                },
            }
        });
        if sink
            .send(Message::Text(
                serde_json::to_string(&identify).unwrap().into(),
            ))
            .await
            .is_err()
        {
            return SessionResult::Reconnect(None);
        }
        tracing::info!("discord gateway: sent Identify");
    }

    let mut heartbeat_timer = tokio::time::interval(heartbeat_interval);
    heartbeat_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    heartbeat_timer.tick().await;

    let mut sequence: Option<u64> = prev_session.as_ref().and_then(|s| s.sequence);
    let mut session_id: Option<String> = prev_session.map(|s| s.session_id);
    let mut resume_url: Option<String> = None;
    let mut got_ack = true;

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                let _ = sink.close().await;
                return SessionResult::Shutdown;
            }
            _ = heartbeat_timer.tick() => {
                if !got_ack {
                    tracing::warn!("discord gateway: missed heartbeat ACK, reconnecting");
                    let state = session_id.map(|sid| SessionState {
                        session_id: sid,
                        resume_url: resume_url.clone(),
                        sequence,
                    });
                    return SessionResult::Reconnect(state);
                }
                let hb = serde_json::json!({ "op": OpCode::Heartbeat as u8, "d": sequence });
                if sink.send(Message::Text(serde_json::to_string(&hb).unwrap().into())).await.is_err() {
                    let state = session_id.map(|sid| SessionState {
                        session_id: sid,
                        resume_url: resume_url.clone(),
                        sequence,
                    });
                    return SessionResult::Reconnect(state);
                }
                got_ack = false;
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let payload: GatewayPayload = match serde_json::from_str(&text) {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::warn!("discord gateway: parse error: {e}");
                                continue;
                            }
                        };

                        if let Some(s) = payload.s {
                            sequence = Some(s);
                        }

                        match OpCode::from_u8(payload.op) {
                            Some(OpCode::Dispatch) => {
                                let event_name = payload.t.as_deref().unwrap_or("");
                                match event_name {
                                    "READY" => {
                                        if let Some(d) = &payload.d {
                                            if let Ok(ready) = serde_json::from_value::<ReadyData>(d.clone()) {
                                                session_id = Some(ready.session_id);
                                                resume_url = ready.resume_gateway_url;
                                                tracing::info!("discord gateway: READY");
                                            }
                                        }
                                    }
                                    "RESUMED" => {
                                        tracing::info!("discord gateway: RESUMED");
                                    }
                                    "MESSAGE_CREATE" => {
                                        if let Some(d) = payload.d {
                                            match serde_json::from_value::<MessageCreateData>(d) {
                                                Ok(msg_data) => {
                                                    on_message(msg_data);
                                                }
                                                Err(e) => {
                                                    tracing::warn!("discord gateway: MESSAGE_CREATE parse error: {e}");
                                                }
                                            }
                                        }
                                    }
                                    _ => {
                                        tracing::debug!("discord gateway: unhandled event {event_name}");
                                    }
                                }
                            }
                            Some(OpCode::HeartbeatAck) => {
                                got_ack = true;
                            }
                            Some(OpCode::Heartbeat) => {
                                let hb = serde_json::json!({ "op": OpCode::Heartbeat as u8, "d": sequence });
                                if sink.send(Message::Text(serde_json::to_string(&hb).unwrap().into())).await.is_err() {
                                    let state = session_id.map(|sid| SessionState {
                                        session_id: sid,
                                        resume_url: resume_url.clone(),
                                        sequence,
                                    });
                                    return SessionResult::Reconnect(state);
                                }
                            }
                            Some(OpCode::Reconnect) => {
                                tracing::info!("discord gateway: server requested reconnect");
                                let state = session_id.map(|sid| SessionState {
                                    session_id: sid,
                                    resume_url: resume_url.clone(),
                                    sequence,
                                });
                                return SessionResult::ReconnectImmediate(state);
                            }
                            Some(OpCode::InvalidSession) => {
                                let resumable = payload.d
                                    .as_ref()
                                    .and_then(|d| d.as_bool())
                                    .unwrap_or(false);
                                if resumable && is_resume {
                                    tracing::info!("discord gateway: invalid session (resumable)");
                                    let state = session_id.map(|sid| SessionState {
                                        session_id: sid,
                                        resume_url: resume_url.clone(),
                                        sequence,
                                    });
                                    return SessionResult::Reconnect(state);
                                }
                                tracing::warn!("discord gateway: invalid session (not resumable), re-identifying");
                                return SessionResult::Reconnect(None);
                            }
                            _ => {
                                tracing::debug!("discord gateway: unknown opcode {}", payload.op);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        let state = session_id.map(|sid| SessionState {
                            session_id: sid,
                            resume_url: resume_url.clone(),
                            sequence,
                        });
                        return SessionResult::Reconnect(state);
                    }
                    Some(Err(e)) => {
                        tracing::error!("discord gateway: read error: {e}");
                        let state = session_id.map(|sid| SessionState {
                            session_id: sid,
                            resume_url: resume_url.clone(),
                            sequence,
                        });
                        return SessionResult::Reconnect(state);
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn read_next_payload(
    stream: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
) -> Option<GatewayPayload> {
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                return serde_json::from_str::<GatewayPayload>(&text).ok();
            }
            Ok(Message::Close(_)) | Err(_) => return None,
            _ => continue,
        }
    }
    None
}
