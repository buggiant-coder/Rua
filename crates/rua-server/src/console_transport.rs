//! One authenticated, bounded transport for Console subscriptions and remote reads.
//! Logical operations enter the canonical API router in-process; no HTTP loopback,
//! duplicate business handlers, or browser connection-pool slots are involved.
use std::{collections::HashMap, time::Duration};

use axum::{
    body::Body,
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{header, Request, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::{
    stream::{BoxStream, SelectAll},
    SinkExt, StreamExt,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower::ServiceExt;

use crate::{auth, state::AppState};

const VERSION: u64 = 1;
const MAX_CHANNELS: usize = 64;
const FRAME_BYTES: usize = 16 * 1024;
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct ConsoleTransport {
    pub state: AppState,
    pub api: Router,
}

/// Mount the multiplexed adapter over the canonical, authenticated API router.
pub fn router(state: AppState, api: Router) -> Router<AppState> {
    Router::new()
        .route("/api/console/transport", axum::routing::get(upgrade))
        .with_state(ConsoleTransport { state, api })
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    Auth {
        version: u64,
        #[serde(default)]
        token: String,
        #[serde(flatten)]
        _extra: serde_json::Map<String, Value>,
    },
    Open {
        id: String,
        method: String,
        path: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        agent_id: String,
        #[serde(flatten)]
        _extra: serde_json::Map<String, Value>,
    },
    Cancel {
        id: String,
        #[serde(flatten)]
        _extra: serde_json::Map<String, Value>,
    },
}

async fn upgrade(
    State(transport): State<ConsoleTransport>,
    ws: WebSocketUpgrade,
    request: Request<Body>,
) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    if !matches!((origin, host), (Some(origin), Some(host)) if crate::desktop_host::origin_matches_host(origin, host))
    {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"detail":"Forbidden origin"})),
        )
            .into_response();
    }
    let (parts, _) = request.into_parts();
    ws.max_message_size(256 * 1024)
        .max_frame_size(256 * 1024)
        .on_upgrade(move |socket| run(socket, transport, parts))
}

async fn send(socket: &mut WebSocket, frame: Value) -> bool {
    matches!(
        tokio::time::timeout(
            SEND_TIMEOUT,
            socket.send(Message::Text(frame.to_string().into()))
        )
        .await,
        Ok(Ok(()))
    )
}

fn request_context(parts: &axum::http::request::Parts, token: &str) -> Request<Body> {
    let mut request = Request::new(Body::empty());
    *request.uri_mut() = "/api/console/transport".parse().expect("static URI");
    *request.headers_mut() = parts.headers.clone();
    *request.extensions_mut() = parts.extensions.clone();
    request.headers_mut().remove(header::AUTHORIZATION);
    if !token.is_empty() {
        if let Ok(value) = format!("Bearer {token}").parse() {
            request.headers_mut().insert(header::AUTHORIZATION, value);
        }
    }
    request
}

/// Closed allowlist: reads and passive subscriptions only. Reconnecting this
/// transport can never replay a Turn submission, permission decision or Stop.
fn allowed(method: &str, path: &str) -> bool {
    if path.len() > 4096 || !path.starts_with('/') || path.starts_with("//") || path.contains('#') {
        return false;
    }
    let path = path.split('?').next().unwrap_or(path);
    let segments: Vec<_> = path.trim_start_matches('/').split('/').collect();
    if segments
        .iter()
        .any(|s| s.is_empty() || *s == "." || *s == "..")
    {
        return false;
    }
    matches!(
        (method, segments.as_slice()),
        (
            "GET",
            ["platform", "conversation-activity"]
                | ["sync", "devices", "activity"]
                | ["terminal", "sessions", _, "activity"],
        ) | ("GET", ["agents", _, "approvals", "snapshot"])
            | (
                "GET",
                ["agents" | "projects", _, "chats", _, "console", "activity"]
                    | [
                        "agents" | "projects",
                        _,
                        "chats",
                        _,
                        "approvals",
                        "snapshot"
                    ]
                    | ["agents" | "projects", _, "chats", _, "acp", "permissions"],
            )
            | (
                "POST",
                ["agents" | "projects", _, "chats", _, "console", "live"]
            )
    )
}

fn enqueue(tx: &mpsc::Sender<Value>, frame: Value, id: &str) -> bool {
    // Reserve one slot for explicit loss. A slow channel cannot block peers.
    if tx.capacity() <= 1 {
        let _ = tx.try_send(json!({"type":"gap", "id":id, "detail":"Console channel overflow; reconcile from cursor or snapshot"}));
        return false;
    }
    tx.try_send(frame).is_ok()
}

async fn dispatch(api: Router, request: Request<Body>, id: String, tx: mpsc::Sender<Value>) {
    let response = api.oneshot(request).await.expect("Router is infallible");
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_owned();
    if !enqueue(
        &tx,
        json!({"type":"head", "id":id, "status":status, "content_type":content_type}),
        &id,
    ) {
        return;
    }
    let mut body = response.into_body().into_data_stream();
    while let Some(chunk) = body.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => {
                let _ = tx.try_send(
                    json!({"type":"error", "id":id, "detail":"Console response stream failed"}),
                );
                return;
            }
        };
        for bytes in chunk.chunks(FRAME_BYTES) {
            if !enqueue(
                &tx,
                json!({"type":"data", "id":id, "data":STANDARD.encode(bytes)}),
                &id,
            ) {
                return;
            }
            // Cooperate with the fair per-channel multiplexer even for ready bodies.
            tokio::task::yield_now().await;
        }
    }
    let _ = tx.send(json!({"type":"end", "id":id})).await;
}

async fn run(
    mut socket: WebSocket,
    transport: ConsoleTransport,
    parts: axum::http::request::Parts,
) {
    let first = tokio::time::timeout(Duration::from_secs(5), socket.recv()).await;
    let auth_frame = match first {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<ClientFrame>(&text).ok(),
        _ => None,
    };
    let Some(ClientFrame::Auth { version, token, .. }) = auth_frame else {
        send(
            &mut socket,
            json!({"type":"fatal", "detail":"Initial authentication required"}),
        )
        .await;
        return;
    };
    if version != VERSION {
        send(
            &mut socket,
            json!({"type":"fatal", "detail":"Unsupported Console Transport version"}),
        )
        .await;
        return;
    }
    let auth_request = request_context(&parts, &token);
    if auth::authentication_error(&transport.state, &auth_request).is_some() {
        send(
            &mut socket,
            json!({"type":"fatal", "detail":"Authentication required or expired"}),
        )
        .await;
        return;
    }
    if !send(&mut socket, json!({"type":"ready", "version":VERSION})).await {
        return;
    }
    let cancellation = CancellationToken::new();
    let tasks = TaskTracker::new();
    let mut channels: HashMap<String, CancellationToken> = HashMap::new();
    let mut frames: SelectAll<BoxStream<'static, Value>> = SelectAll::new();
    let mut auth_check = tokio::time::interval(Duration::from_secs(2));
    let mut shutdown = transport.state.shutdown_tx.subscribe();
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = auth_check.tick() => {
                if auth::authentication_error(&transport.state, &auth_request).is_some() {
                    send(&mut socket, json!({"type":"fatal", "detail":"Authentication revoked or expired"})).await;
                    break;
                }
            }
            Some(frame) = frames.next(), if !frames.is_empty() => {
                if let Some(id) = frame["id"].as_str() {
                    if !channels.contains_key(id) { continue; }
                    if matches!(frame["type"].as_str(), Some("end" | "gap" | "error")) {
                        if let Some(token) = channels.remove(id) { token.cancel(); }
                    }
                }
                if !send(&mut socket, frame).await { break; }
            }
            incoming = socket.recv() => {
                let text = match incoming {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Ping(bytes))) => { if socket.send(Message::Pong(bytes)).await.is_err() { break; } continue; },
                    Some(Ok(Message::Pong(_))) => continue,
                    _ => break,
                };
                let Ok(frame) = serde_json::from_str::<ClientFrame>(&text) else { break; };
                match frame {
                    ClientFrame::Auth { .. } => break,
                    ClientFrame::Cancel { id, .. } => { if let Some(token) = channels.remove(&id) { token.cancel(); } },
                    ClientFrame::Open { id, method, path, body, agent_id, .. } => {
                        if auth::authentication_error(&transport.state, &auth_request).is_some() { break; }
                        if id.is_empty() || id.len() > 64 || channels.contains_key(&id) { break; }
                        if channels.len() >= MAX_CHANNELS || body.len() > 128 * 1024 || !allowed(&method, &path) {
                            if !send(&mut socket, json!({"type":"error", "id":id, "detail":"Console operation not admitted"})).await { break; }
                            continue;
                        }
                        let mut request = request_context(&parts, &token);
                        let Ok(uri) = format!("/api{path}").parse() else { break; };
                        *request.uri_mut() = uri;
                        *request.method_mut() = method.parse().expect("allowlisted method");
                        // Do not pass upgrade/hop-by-hop headers into application adapters.
                        request.headers_mut().remove(header::CONNECTION);
                        request.headers_mut().remove(header::UPGRADE);
                        request.headers_mut().insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
                        if !agent_id.is_empty() {
                            let Ok(value) = agent_id.parse() else { break; };
                            request.headers_mut().insert("x-agent-id", value);
                        }
                        *request.body_mut() = Body::from(body);
                        let is_stream = path.split('?').next().is_some_and(|p| p.ends_with("/activity") || p.ends_with("/conversation-activity") || p.ends_with("/live"));
                        let (tx, rx) = mpsc::channel(8);
                        frames.push(futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|frame| (frame, rx)) }).boxed());
                        let token = cancellation.child_token();
                        channels.insert(id.clone(), token.clone());
                        let api = transport.api.clone();
                        tasks.spawn(async move {
                            tokio::select! {
                                _ = token.cancelled() => {},
                                _ = dispatch(api, request, id.clone(), tx.clone()) => {},
                                _ = tokio::time::sleep(Duration::from_secs(30)), if !is_stream => {
                                    let _ = tx.try_send(json!({"type":"error", "id":id, "detail":"Console request deadline exceeded"}));
                                }
                            }
                        });
                    }
                }
            }
        }
    }
    cancellation.cancel();
    tasks.close();
    tasks.wait().await;
    let _ = tokio::time::timeout(SEND_TIMEOUT, socket.close()).await;
}
