use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::ws::{Message, WebSocket},
    extract::{State, WebSocketUpgrade},
    routing::{get, post},
    Json, Router,
};
use rua_channel::core::channel::Channel;
use rua_channel::slack::{SlackChannel, SlackConfig};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};

use super::support::{recv, reply_process, wait_for_len};

#[derive(Clone)]
struct Api {
    addr: std::net::SocketAddr,
    sockets: mpsc::UnboundedSender<WebSocket>,
    replies: Arc<Mutex<Vec<Value>>>,
}

async fn open_socket(State(state): State<Api>) -> Json<Value> {
    Json(json!({"ok": true, "url": format!("ws://{}/socket", state.addr)}))
}

#[tokio::test]
async fn socket_mode_proxy_acks_deduplicates_threads_reconnects_and_restarts() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (sockets, mut connections) = mpsc::unbounded_channel();
    let replies = Arc::new(Mutex::new(Vec::new()));
    let api = Api {
        addr,
        sockets,
        replies: replies.clone(),
    };
    let app = Router::new()
        .route("/apps.connections.open", post(open_socket))
        .route(
            "/auth.test",
            post(|| async { Json(json!({"ok": true, "user_id": "B1"})) }),
        )
        .route(
            "/socket",
            get(
                |State(state): State<Api>, ws: WebSocketUpgrade| async move {
                    ws.on_upgrade(move |socket| async move {
                        state.sockets.send(socket).unwrap();
                    })
                },
            ),
        )
        .route(
            "/chat.postMessage",
            post(
                |State(state): State<Api>, Json(body): Json<Value>| async move {
                    state.replies.lock().await.push(body);
                    Json(json!({"ok": true, "ts": "reply.1"}))
                },
            ),
        )
        .with_state(api);
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (proxy_url, proxy_requests, proxy) = super::support::http_proxy(addr).await;
    let channel = SlackChannel::new(SlackConfig {
        bot_token: "bot-token".into(),
        app_token: "app-token".into(),
        ..Default::default()
    })
    .with_api_base(format!("http://{addr}"))
    .with_client(
        reqwest::Client::builder()
            .no_proxy()
            .pool_max_idle_per_host(0)
            .proxy(reqwest::Proxy::all(proxy_url).unwrap())
            .build()
            .unwrap(),
    )
    .with_acl_config(json!({"streaming_enabled": false}));
    let (process, mut received) = reply_process("ready");
    channel.start(process.clone()).await.unwrap();
    let mut socket = recv(&mut connections).await;
    socket
        .send(Message::Text(json!({"type":"hello"}).to_string().into()))
        .await
        .unwrap();
    let envelope = json!({
        "type": "events_api", "envelope_id": "env-1",
        "payload": { "event_id": "event-1", "event": {
            "type": "app_mention", "user": "U1", "channel": "C1", "channel_type": "channel",
            "text": "<@B1> hi", "ts": "message.1", "thread_ts": "root.1"
        }}
    });
    for _ in 0..2 {
        socket
            .send(Message::Text(envelope.to_string().into()))
            .await
            .unwrap();
        let ack = tokio::time::timeout(Duration::from_secs(5), socket.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(ack.to_text().unwrap()).unwrap(),
            json!({"envelope_id": "env-1"})
        );
    }
    let request = recv(&mut received).await;
    assert_eq!(request.user_id, "U1");
    wait_for_len(&replies, 1).await;
    assert_eq!(replies.lock().await[0]["thread_ts"], "root.1");
    assert_eq!(replies.lock().await[0]["text"], "ready");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), received.recv())
            .await
            .is_err()
    );

    socket
        .send(Message::Text(
            json!({"type": "disconnect"}).to_string().into(),
        ))
        .await
        .unwrap();
    let _reconnected = recv(&mut connections).await;
    channel.stop().await.unwrap();
    channel.start(process).await.unwrap();
    let mut socket = recv(&mut connections).await;
    socket
        .send(Message::Text(envelope.to_string().into()))
        .await
        .unwrap();
    let _ = recv(&mut received).await;
    wait_for_len(&replies, 2).await;
    channel.stop().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), connections.recv())
            .await
            .is_err()
    );
    let requests = proxy_requests.lock().await;
    for (method, path) in [
        ("POST", "apps.connections.open"),
        ("GET", "socket"),
        ("POST", "chat.postMessage"),
    ] {
        assert!(
            requests
                .iter()
                .any(|r| r.starts_with(&format!("{method} http://{addr}/{path} HTTP/1.1"))),
            "missing proxied {path}: {requests:?}"
        );
    }
    proxy.abort();
    server.abort();
}

#[tokio::test]
async fn socket_mode_rejects_missing_app_token_without_connecting() {
    let channel = SlackChannel::new(SlackConfig {
        bot_token: "test".into(),
        ..Default::default()
    });
    assert!(channel
        .start(reply_process("ok").0)
        .await
        .unwrap_err()
        .to_string()
        .contains("app_token"));
}
