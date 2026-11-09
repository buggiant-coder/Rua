use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures_util::{future::BoxFuture, SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse as WsErrorResponse, Request as WsRequest, Response as WsResponse,
};
use tokio_tungstenite::tungstenite::{http, Message};
use tokio_util::sync::CancellationToken;

use rua_core::{Result, RuaError};
use serde_json::{json, Value};

pub type OnMessageFn = Arc<dyn Fn(Value) + Send + Sync>;
pub type AsyncOnMessageFn = Arc<dyn Fn(Value) -> BoxFuture<'static, ()> + Send + Sync>;
type Pending = Arc<StdMutex<HashMap<String, (String, oneshot::Sender<Value>)>>>;

#[derive(Clone, Default)]
pub struct WsRpc {
    connections: Arc<Mutex<HashMap<String, mpsc::Sender<Value>>>>,
    pending: Pending,
}

struct PendingCall {
    echo: String,
    pending: Pending,
}

impl Drop for PendingCall {
    fn drop(&mut self) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.echo);
    }
}

impl WsRpc {
    pub async fn call(&self, action: &str, params: Value) -> Result<Value> {
        self.call_on(None, action, params).await
    }

    pub async fn call_on(
        &self,
        connection: Option<&str>,
        action: &str,
        params: Value,
    ) -> Result<Value> {
        let (connection_id, sender) = {
            let connections = self.connections.lock().await;
            let entry = if let Some(id) = connection {
                connections.get_key_value(id)
            } else if connections.len() == 1 {
                connections.iter().next()
            } else {
                None
            };
            entry
                .map(|(id, sender)| (id.clone(), sender.clone()))
                .ok_or_else(|| RuaError::Runtime {
                    message: "OneBot reverse WebSocket connection unavailable or ambiguous".into(),
                })?
        };
        let echo = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(echo.clone(), (connection_id, tx));
        let _pending = PendingCall {
            echo: echo.clone(),
            pending: self.pending.clone(),
        };
        let response = tokio::time::timeout(Duration::from_secs(10), async {
            sender
                .send(json!({"action": action, "params": params, "echo": echo}))
                .await
                .map_err(|_| RuaError::Runtime {
                    message: "OneBot connection closed".into(),
                })?;
            rx.await.map_err(|_| RuaError::Runtime {
                message: "OneBot response channel closed".into(),
            })
        })
        .await
        .map_err(|_| RuaError::Runtime {
            message: format!("OneBot {action} timed out"),
        })??;
        if response["retcode"].as_i64() != Some(0) || response["status"] == "failed" {
            return Err(RuaError::Runtime {
                message: format!("OneBot {action} failed: {response}"),
            });
        }
        Ok(response["data"].clone())
    }

    fn response(&self, connection_id: &str, value: Value) {
        if let Some(echo) = value["echo"].as_str() {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            if pending.get(echo).is_some_and(|(id, _)| id == connection_id) {
                if let Some((_, sender)) = pending.remove(echo) {
                    let _ = sender.send(value);
                }
            }
        }
    }

    pub async fn clear(&self) {
        self.connections.lock().await.clear();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

pub struct WsServerConfig {
    pub host: String,
    pub port: u16,
    pub access_token: Option<String>,
}

/// Compatibility entry point for callback users. The channel uses start_ws_server
/// so bind failures are returned to its lifecycle owner.
pub fn spawn_ws_server(
    config: WsServerConfig,
    on_message: OnMessageFn,
    mut stop_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let cancel = CancellationToken::new();
        let callback: AsyncOnMessageFn = Arc::new(move |value| {
            on_message(value);
            Box::pin(async {})
        });
        match start_ws_server(config, callback, cancel.clone(), WsRpc::default()).await {
            Ok(mut task) => {
                tokio::select! {
                    _ = stop_rx.changed() => cancel.cancel(),
                    _ = &mut task => return,
                }
                let _ = task.await;
            }
            Err(error) => tracing::error!(%error, "OneBot listener failed"),
        }
    })
}

pub async fn start_ws_server(
    config: WsServerConfig,
    on_message: AsyncOnMessageFn,
    cancel: CancellationToken,
    rpc: WsRpc,
) -> Result<JoinHandle<()>> {
    let host = config.host.trim_matches(['[', ']']);
    let listener = TcpListener::bind((host, config.port))
        .await
        .map_err(|error| RuaError::Runtime {
            message: format!("OneBot bind {}:{}: {error}", config.host, config.port),
        })?;
    Ok(tokio::spawn(async move {
        let mut clients = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                _ = clients.join_next(), if !clients.is_empty() => {}
                accept = listener.accept() => {
                    match accept {
                        Ok((stream, _)) => {
                            let callback = on_message.clone();
                            let token = config.access_token.clone().filter(|token| !token.trim().is_empty());
                            let rpc = rpc.clone();
                            let cancel = cancel.clone();
                            clients.spawn(async move {
                                handle_client(stream, callback, token.as_deref(), rpc, cancel).await;
                            });
                        }
                        Err(error) => tracing::warn!(%error, "OneBot accept failed"),
                    }
                }
            }
        }
        // Cancellation reaches handshakes, socket reads, and message processing.
        while clients.join_next().await.is_some() {}
        rpc.clear().await;
    }))
}

#[allow(clippy::result_large_err)]
async fn handle_client(
    stream: TcpStream,
    on_message: AsyncOnMessageFn,
    access_token: Option<&str>,
    rpc: WsRpc,
    cancel: CancellationToken,
) {
    let handshake = tokio_tungstenite::accept_hdr_async(
        stream,
        |req: &WsRequest, res: WsResponse| -> std::result::Result<WsResponse, WsErrorResponse> {
            if let Some(expected) = access_token {
                let provided = req
                    .headers()
                    .get("Authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|value| value.trim().split_once(' '));
                if !provided.is_some_and(|(scheme, token)| {
                    (scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("token"))
                        && token.trim() == expected.trim()
                }) {
                    let mut reject = http::Response::new(Some("Unauthorized".into()));
                    *reject.status_mut() = http::StatusCode::UNAUTHORIZED;
                    return Err(reject);
                }
            }
            Ok(res)
        },
    );
    let mut socket = tokio::select! {
        _ = cancel.cancelled() => return,
        result = tokio::time::timeout(Duration::from_secs(10), handshake) => {
            match result {
                Ok(Ok(socket)) => socket,
                _ => return,
            }
        }
    };
    let connection_id = uuid::Uuid::new_v4().to_string();
    let (outbound, mut outgoing) = mpsc::channel::<Value>(64);
    rpc.connections
        .lock()
        .await
        .insert(connection_id.clone(), outbound);
    let mut tasks = JoinSet::new();
    let run = async {
        loop {
            tokio::select! {
                _ = tasks.join_next(), if !tasks.is_empty() => {}
                outgoing = outgoing.recv() => {
                    let Some(value) = outgoing else { break; };
                    if socket.send(Message::Text(value.to_string().into())).await.is_err() { break; }
                }
                msg = socket.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(mut value) = serde_json::from_str::<Value>(&text) {
                                if value.get("echo").is_some() {
                                    rpc.response(&connection_id, value);
                                } else if value.is_object() {
                                    value["_onebot_connection_id"] = connection_id.clone().into();
                                    tasks.spawn(on_message(value));
                                }
                            }
                        }
                        Some(Ok(Message::Ping(data))) => match socket.send(Message::Pong(data)).await {
                            Ok(()) => {}
                            Err(_) => break,
                        }
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                        _ => {}
                    }
                }
            }
        }
    };
    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = run => {}
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    rpc.connections.lock().await.remove(&connection_id);
    rpc.pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|_, (id, _)| id != &connection_id);
}
