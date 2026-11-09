use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::types::{XiaoyiConfig, HEARTBEAT_INTERVAL_SECS, INITIAL_BACKOFF_SECS, MAX_BACKOFF_SECS};

type HmacSha256 = Hmac<Sha256>;

pub fn generate_signature(sk: &str, timestamp: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(sk.as_bytes()).expect("HMAC key length is valid");
    mac.update(timestamp.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

pub fn generate_auth_headers(ak: &str, sk: &str, agent_id: &str) -> Vec<(String, String)> {
    let timestamp = format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let signature = generate_signature(sk, &timestamp);
    vec![
        ("x-access-key".into(), ak.into()),
        ("x-sign".into(), signature),
        ("x-ts".into(), timestamp),
        ("x-agent-id".into(), agent_id.into()),
    ]
}

pub(crate) enum WsOutbound {
    Json {
        server_name: Option<String>,
        json: String,
    },
}

pub type OnMessageFn = Arc<dyn Fn(serde_json::Value, String) + Send + Sync>;

pub struct WsHandle {
    pub(crate) outbound_tx: mpsc::Sender<WsOutbound>,
    _task: JoinHandle<()>,
}

impl WsHandle {
    pub async fn send_json(&self, json: String) -> Result<(), String> {
        self.send_json_to_server(None, json).await
    }

    pub async fn send_json_to_server(
        &self,
        server_name: Option<&str>,
        json: String,
    ) -> Result<(), String> {
        self.outbound_tx
            .send(WsOutbound::Json {
                server_name: server_name.map(ToOwned::to_owned),
                json,
            })
            .await
            .map_err(|e| format!("ws outbound channel closed: {e}"))
    }
}

pub fn spawn_ws_client(
    config: XiaoyiConfig,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
) -> WsHandle {
    let (outbound_tx, outbound_rx) = mpsc::channel(64);
    let task = tokio::spawn(ws_coordinator(config, on_message, stop_rx, outbound_rx));
    WsHandle {
        outbound_tx,
        _task: task,
    }
}

struct EndpointHandle {
    server_name: &'static str,
    outbound_tx: mpsc::Sender<String>,
    connected_rx: watch::Receiver<bool>,
}

async fn ws_coordinator(
    config: XiaoyiConfig,
    on_message: OnMessageFn,
    mut stop_rx: watch::Receiver<bool>,
    mut outbound_rx: mpsc::Receiver<WsOutbound>,
) {
    let primary = spawn_endpoint(
        "primary",
        config.ws_url.clone(),
        config.clone(),
        on_message.clone(),
        stop_rx.clone(),
    );
    let backup = if config.backup_ws_url.trim().is_empty() {
        None
    } else {
        Some(spawn_endpoint(
            "backup",
            config.backup_ws_url.clone(),
            config,
            on_message,
            stop_rx.clone(),
        ))
    };

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                return;
            }
            outbound = outbound_rx.recv() => {
                let Some(WsOutbound::Json { server_name, json }) = outbound else {
                    return;
                };
                send_via_endpoint(&primary, backup.as_ref(), server_name.as_deref(), json).await;
            }
        }
    }
}

fn spawn_endpoint(
    server_name: &'static str,
    ws_url: String,
    config: XiaoyiConfig,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
) -> EndpointHandle {
    let (outbound_tx, outbound_rx) = mpsc::channel(64);
    let (connected_tx, connected_rx) = watch::channel(false);
    tokio::spawn(ws_run_loop(
        server_name,
        ws_url,
        config,
        on_message,
        stop_rx,
        outbound_rx,
        connected_tx,
    ));
    EndpointHandle {
        server_name,
        outbound_tx,
        connected_rx,
    }
}

async fn send_via_endpoint(
    primary: &EndpointHandle,
    backup: Option<&EndpointHandle>,
    server_name: Option<&str>,
    json: String,
) {
    let (first, second) = match server_name {
        Some("backup") => (backup, Some(primary)),
        _ => (Some(primary), backup),
    };

    if let Some(endpoint) = first {
        if *endpoint.connected_rx.borrow() && endpoint.outbound_tx.send(json.clone()).await.is_ok()
        {
            return;
        }
    }
    if let Some(endpoint) = second {
        if *endpoint.connected_rx.borrow() && endpoint.outbound_tx.send(json.clone()).await.is_ok()
        {
            return;
        }
    }
    if let Some(endpoint) = first {
        let _ = endpoint.outbound_tx.send(json).await;
        tracing::warn!(
            server = endpoint.server_name,
            "xiaoyi WS: no connected endpoint available, queued on preferred endpoint"
        );
    }
}

async fn ws_run_loop(
    server_name: &'static str,
    ws_url: String,
    config: XiaoyiConfig,
    on_message: OnMessageFn,
    mut stop_rx: watch::Receiver<bool>,
    mut outbound_rx: mpsc::Receiver<String>,
    connected_tx: watch::Sender<bool>,
) {
    let mut backoff = Duration::from_secs(INITIAL_BACKOFF_SECS);
    let max_backoff = Duration::from_secs(MAX_BACKOFF_SECS);

    loop {
        if *stop_rx.borrow() {
            tracing::info!("xiaoyi WS loop: stop signal received before connect");
            return;
        }

        tracing::info!("xiaoyi WS [{server_name}]: connecting to {ws_url}");

        let mut request = match ws_url.as_str().into_client_request() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("xiaoyi WS [{server_name}]: invalid URL: {e}");
                return;
            }
        };

        {
            let headers =
                generate_auth_headers(&config.access_key, &config.secret_key, &config.agent_id);
            let req_headers = request.headers_mut();
            for (key, value) in headers {
                use tokio_tungstenite::tungstenite::http::header::{HeaderName, HeaderValue};
                if let (Ok(name), Ok(val)) = (
                    HeaderName::from_bytes(key.as_bytes()),
                    HeaderValue::from_str(&value),
                ) {
                    req_headers.insert(name, val);
                }
            }
        }

        let ws_stream = match tokio_tungstenite::connect_async(request).await {
            Ok((stream, _)) => {
                tracing::info!("xiaoyi WS [{server_name}]: connected");
                backoff = Duration::from_secs(INITIAL_BACKOFF_SECS);
                stream
            }
            Err(e) => {
                tracing::error!(
                    "xiaoyi WS [{server_name}] connect error: {e}, retry in {backoff:?}"
                );
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = stop_rx.changed() => { return; }
                }
                backoff = (backoff * 2).min(max_backoff);
                continue;
            }
        };

        let disconnected = handle_ws_session(
            ws_stream,
            server_name,
            &config,
            &on_message,
            &mut stop_rx,
            &mut outbound_rx,
            &connected_tx,
        )
        .await;
        let _ = connected_tx.send(false);

        if !disconnected {
            tracing::info!("xiaoyi WS [{server_name}]: graceful shutdown");
            return;
        }

        tracing::warn!("xiaoyi WS [{server_name}]: disconnected, reconnecting in {backoff:?}");
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = stop_rx.changed() => { return; }
        }
        backoff = (backoff * 2).min(max_backoff);
    }
}

async fn handle_ws_session(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    server_name: &'static str,
    config: &XiaoyiConfig,
    on_message: &OnMessageFn,
    stop_rx: &mut watch::Receiver<bool>,
    outbound_rx: &mut mpsc::Receiver<String>,
    connected_tx: &watch::Sender<bool>,
) -> bool {
    let (mut sink, mut stream) = ws_stream.split();

    let init_msg = serde_json::json!({
        "msgType": "clawd_bot_init",
        "agentId": config.agent_id,
        "msgDetail": serde_json::json!({
            "agentId": config.agent_id,
            "hostname": std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("COMPUTERNAME"))
                .unwrap_or_default(),
        }).to_string(),
    });
    if let Err(e) = sink
        .send(Message::Text(
            serde_json::to_string(&init_msg).unwrap().into(),
        ))
        .await
    {
        tracing::error!("xiaoyi WS [{server_name}]: failed to send init: {e}");
        return true;
    }
    tracing::info!("xiaoyi WS [{server_name}]: init message sent");
    let _ = connected_tx.send(true);

    let mut heartbeat_interval =
        tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let agent_id = config.agent_id.clone();

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                let _ = sink.close().await;
                return false;
            }
            _ = heartbeat_interval.tick() => {
                let hb = serde_json::json!({
                    "msgType": "heartbeat",
                    "agentId": agent_id,
                });
                if sink.send(Message::Text(serde_json::to_string(&hb).unwrap().into())).await.is_err() {
                    return true;
                }
            }
            outbound = outbound_rx.recv() => {
                match outbound {
                    Some(json) => {
                        if sink.send(Message::Text(json.into())).await.is_err() {
                            return true;
                        }
                    }
                    None => {

                        let _ = sink.close().await;
                        return false;
                    }
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<serde_json::Value>(&text) {
                            Ok(value) => on_message(value, server_name.to_string()),
                            Err(e) => {
                                tracing::warn!("xiaoyi WS [{server_name}]: invalid JSON frame: {e}");
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
                        tracing::error!("xiaoyi WS [{server_name}] read error: {e}");
                        return true;
                    }
                }
            }
        }
    }
}
