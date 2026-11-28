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
/// Largest single streamed payload chunk handed to this transport (conversation
/// replay and checkpoint payloads are capped at this size upstream).
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
/// Per-channel queue depth. It must hold one whole payload after splitting, plus
/// one reserved slot so loss can always be signalled explicitly. A smaller queue
/// cannot ever flush a maximum-size payload: the frames of one chunk would fill
/// it and the channel would be killed on every burst, regardless of how fast the
/// client reads.
const CHANNEL_QUEUE_FRAMES: usize = MAX_PAYLOAD_BYTES / FRAME_BYTES + 2;
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

// A maximum-size payload must fit with one slot to spare for the loss signal.
// Breaking this invariant makes large bursts unflushable and kills the channel.
const _: () = assert!(
    MAX_PAYLOAD_BYTES.div_ceil(FRAME_BYTES) < CHANNEL_QUEUE_FRAMES,
    "channel queue must hold one whole payload plus a reserved loss slot"
);

#[derive(Clone)]
struct ConsoleTransport {
    pub state: AppState,
    pub api: Router,
    pub shell: Option<crate::desktop_host::DesktopHostConfig>,
}

/// Mount the multiplexed adapter over the canonical, authenticated API router.
pub fn router(
    state: AppState,
    api: Router,
    shell: Option<crate::desktop_host::DesktopHostConfig>,
) -> Router<AppState> {
    Router::new()
        .route(TRANSPORT_PATH, axum::routing::get(upgrade))
        .with_state(ConsoleTransport { state, api, shell })
}

/// WebSocket upgrade path for the Console Transport.
///
/// This path authenticates in its bounded first frame rather than at the HTTP
/// layer, because browsers cannot attach an `Authorization` header to a
/// WebSocket handshake. `auth::requires_shell_credential` refers to this
/// constant so the Desktop Shell layer and the transport cannot drift apart.
pub const TRANSPORT_PATH: &str = "/api/console/transport";

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
    *request.uri_mut() = TRANSPORT_PATH.parse().expect("static URI");
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
                | ["platform", "user-notifications", "events"]
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

fn enqueue(tx: &mpsc::Sender<Value>, frame: Value, id: &str, path: &str) -> bool {
    // Reserve one slot for explicit loss. A slow channel cannot block peers.
    let remaining = tx.capacity();
    if remaining <= 1 {
        // Server-side loss: this channel's WebSocket writer could not drain the
        // queue. Distinct from the client dropping frames into its own read
        // buffer, which reports [client-buffer] on the browser side.
        tracing::warn!(
            channel = id,
            path,
            remaining,
            queue_frames = CHANNEL_QUEUE_FRAMES,
            "console transport channel backlog: dropping frames for a slow channel"
        );
        let _ = tx.try_send(json!({
            "type":"gap",
            "id":id,
            "code":"server_channel_backlog",
            "detail":"Console channel overflow [server-backlog]; the server could not flush this channel (reconcile from cursor or snapshot)"
        }));
        return false;
    }
    tx.try_send(frame).is_ok()
}

/// Whether an allowlisted path is a long-lived subscription rather than a
/// bounded read.
///
/// Bounded reads get a 30s server deadline so a stuck operation cannot pin a
/// channel forever. Subscriptions have no natural end, so applying that same
/// deadline would kill them on a timer. This must stay in step with the client
/// rule in `console/src/api/consoleTransport.ts`: an SSE endpoint classified as
/// bounded here is torn down after 30s, and one classified as a stream there
/// masks a genuinely stuck read.
pub fn is_persistent_stream(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    path.ends_with("/activity")
        || path.ends_with("/conversation-activity")
        || path.ends_with("/live")
        || path.ends_with("/events")
}

/// The server-initiated deadline signal. Exposed so the transport contract test
/// can assert its discriminator without waiting out a real 30s deadline.
pub fn server_deadline_frame(id: &str) -> Value {
    json!({
        "type":"error",
        "id":id,
        "code":"server_request_deadline",
        "detail":"Console request deadline exceeded [server-deadline]; the server gave up on this operation (the page has its own independent timer)"
    })
}

async fn dispatch(api: Router, request: Request<Body>, id: String, tx: mpsc::Sender<Value>) {
    let path = request.uri().path().to_owned();
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
        &path,
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
                &path,
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
    if auth::authentication_error_with_shell(&transport.state, &auth_request, transport.shell.as_ref()).is_some() {
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
                if auth::authentication_error_with_shell(&transport.state, &auth_request, transport.shell.as_ref()).is_some() {
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
                        if auth::authentication_error_with_shell(&transport.state, &auth_request, transport.shell.as_ref()).is_some() { break; }
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
                        let is_stream = is_persistent_stream(&path);
                        let (tx, rx) = mpsc::channel(CHANNEL_QUEUE_FRAMES);
                        frames.push(futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|frame| (frame, rx)) }).boxed());
                        let token = cancellation.child_token();
                        channels.insert(id.clone(), token.clone());
                        let api = transport.api.clone();
                        let log_path = path.clone();
                        tasks.spawn(async move {
                            tokio::select! {
                                _ = token.cancelled() => {},
                                _ = dispatch(api, request, id.clone(), tx.clone()) => {},
                                _ = tokio::time::sleep(Duration::from_secs(30)), if !is_stream => {
                                    tracing::warn!(
                                        channel = %id,
                                        path = %log_path,
                                        "console transport request deadline exceeded on the server"
                                    );
                                    let _ = tx.try_send(server_deadline_frame(&id));
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
