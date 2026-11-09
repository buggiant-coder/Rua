use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::http_client::default_http_client;

const GATEWAY_OPEN_URL: &str = "https://api.dingtalk.com/v1.0/gateway/connections/open";
const CHATBOT_MESSAGE_TOPIC: &str = "/v1.0/im/bot/messages/get";
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
struct GatewayResponse {
    endpoint: String,
    ticket: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WsEnvelope {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    headers: serde_json::Value,
    #[serde(default)]
    data: Option<String>,
}

#[derive(Debug, Serialize)]
struct WsAck {
    code: u16,
    headers: AckHeaders,
    message: String,
    data: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AckHeaders {
    content_type: String,
    message_id: String,
}

impl WsAck {
    fn ok(message_id: &str) -> Self {
        Self {
            code: 200,
            headers: AckHeaders {
                content_type: "application/json".into(),
                message_id: message_id.into(),
            },
            message: "OK".into(),
            data: String::new(),
        }
    }
}

pub type OnMessageFn = Arc<dyn Fn(serde_json::Value) + Send + Sync>;

pub struct WsConfig {
    pub client_id: String,
    pub client_secret: String,
    pub enable_ws: bool,
}

pub fn spawn_ws_task(
    config: WsConfig,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
) -> Option<JoinHandle<()>> {
    spawn_ws_task_with_endpoint(config, on_message, stop_rx, GATEWAY_OPEN_URL.to_string())
}

pub(super) fn spawn_ws_task_with_endpoint(
    config: WsConfig,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
    gateway_url: String,
) -> Option<JoinHandle<()>> {
    if !config.enable_ws {
        tracing::info!("dingtalk WS disabled by config");
        return None;
    }
    let handle = tokio::spawn(ws_run_loop(config, on_message, stop_rx, gateway_url));
    Some(handle)
}

async fn open_gateway_connection(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    gateway_url: &str,
) -> Result<GatewayResponse, String> {
    let body = serde_json::json!({
        "clientId": client_id,
        "clientSecret": client_secret,
        "subscriptions": [
            {
                "type": "CALLBACK",
                "topic": CHATBOT_MESSAGE_TOPIC,
            }
        ],
        "ua": "rua-rust/1.0",
    });
    let resp = client
        .post(gateway_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("gateway HTTP request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("gateway returned {status}: {text}"));
    }

    resp.json::<GatewayResponse>()
        .await
        .map_err(|e| format!("gateway response parse failed: {e}"))
}

async fn ws_run_loop(
    config: WsConfig,
    on_message: OnMessageFn,
    mut stop_rx: watch::Receiver<bool>,
    gateway_url: String,
) {
    let http = default_http_client();
    let mut backoff = INITIAL_BACKOFF;

    loop {
        if *stop_rx.borrow() {
            tracing::info!("dingtalk WS loop: stop signal received before connect");
            return;
        }

        tracing::info!("dingtalk WS: opening gateway connection");
        let gw = match open_gateway_connection(
            &http,
            &config.client_id,
            &config.client_secret,
            &gateway_url,
        )
        .await
        {
            Ok(gw) => {
                backoff = INITIAL_BACKOFF;
                gw
            }
            Err(e) => {
                tracing::error!("dingtalk WS gateway error: {e}, retry in {backoff:?}");
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = stop_rx.changed() => { return; }
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        let ws_url = format!("{}?ticket={}", gw.endpoint, gw.ticket);
        tracing::info!("dingtalk WS: connecting to {}", gw.endpoint);

        let ws_stream = match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((stream, _)) => {
                tracing::info!("dingtalk WS: connected");
                backoff = INITIAL_BACKOFF;
                stream
            }
            Err(e) => {
                tracing::error!("dingtalk WS connect error: {e}, retry in {backoff:?}");
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = stop_rx.changed() => { return; }
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        let disconnected = handle_ws_session(ws_stream, &on_message, &mut stop_rx).await;
        if !disconnected {
            tracing::info!("dingtalk WS: graceful shutdown");
            return;
        }

        tracing::warn!("dingtalk WS: disconnected, reconnecting in {backoff:?}");
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = stop_rx.changed() => { return; }
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

pub async fn handle_ws_session(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    on_message: &OnMessageFn,
    stop_rx: &mut watch::Receiver<bool>,
) -> bool {
    let (mut sink, mut stream) = ws_stream.split();
    let mut heartbeat_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                let _ = sink.close().await;
                return false;
            }
            _ = heartbeat_interval.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return true;
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match handle_text_frame(&text, on_message, &mut sink).await {
                            Ok(FrameOutcome::Continue) => {}
                            Ok(FrameOutcome::Disconnect) => {
                                return true;
                            }
                            Err(e) => {
                                tracing::warn!("dingtalk WS frame error: {e}");
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if sink.send(Message::Pong(data)).await.is_err() {
                            return true;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return true;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::error!("dingtalk WS read error: {e}");
                        return true;
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameOutcome {
    Continue,
    Disconnect,
}

async fn handle_text_frame<S>(
    text: &str,
    on_message: &OnMessageFn,
    sink: &mut S,
) -> Result<FrameOutcome, String>
where
    S: futures_util::Sink<Message> + Unpin,
    S::Error: std::fmt::Display,
{
    let envelope: WsEnvelope =
        serde_json::from_str(text).map_err(|e| format!("parse envelope: {e}"))?;

    let message_id = envelope
        .headers
        .get("messageId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match envelope.r#type.as_str() {
        "SYSTEM" => {
            let topic = envelope
                .headers
                .get("topic")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if topic == "ping" {
                tracing::debug!("dingtalk WS: heartbeat ping from server");
                let ack = WsAck::ok(&message_id);
                let ack_json = serde_json::to_string(&ack).unwrap();
                sink.send(Message::Text(ack_json.into()))
                    .await
                    .map_err(|e| format!("send heartbeat ack: {e}"))?;
            } else if topic == "disconnect" {
                tracing::info!("dingtalk WS: server disconnect notification");
                return Ok(FrameOutcome::Disconnect);
            }
        }
        "CALLBACK" => {
            let ack = WsAck::ok(&message_id);
            let ack_json = serde_json::to_string(&ack).unwrap();
            sink.send(Message::Text(ack_json.into()))
                .await
                .map_err(|e| format!("send callback ack: {e}"))?;

            if let Some(data_str) = &envelope.data {
                match serde_json::from_str::<serde_json::Value>(data_str) {
                    Ok(payload) => {
                        tracing::info!("dingtalk WS: callback message received");
                        on_message(payload);
                    }
                    Err(e) => {
                        tracing::warn!("dingtalk WS: callback data parse failed: {e}");
                    }
                }
            }
        }
        other => {
            tracing::debug!("dingtalk WS: unknown message type: {other}");
        }
    }

    Ok(FrameOutcome::Continue)
}
