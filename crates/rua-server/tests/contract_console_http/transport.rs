use axum::{body::Body, routing::get, Json, Router};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};
use tokio_util::sync::CancellationToken;

struct Fixture {
    address: std::net::SocketAddr,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    _root: tempfile::TempDir,
    dropped: Arc<AtomicBool>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}
struct CancelMark(Arc<AtomicBool>);
impl Drop for CancelMark {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl Fixture {
    async fn new(mobile: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut state = rua_server::AppState::from_tempdir(
            root.path().to_path_buf(),
            rua_server::ServerProfile::Test,
        );
        state.auth = Arc::new(rua_server::auth::AuthService::new_disabled());
        if mobile {
            state.profile = rua_server::ServerProfile::MobileApk;
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let marker = dropped.clone();
        let api = Router::new()
            .route(
                "/api/agents/a/chats/slow/acp/permissions",
                get(move || {
                    let marker = marker.clone();
                    async move {
                        let _guard = CancelMark(marker);
                        std::future::pending::<Json<Value>>().await
                    }
                }),
            )
            .route(
                "/api/agents/a/chats/fast/acp/permissions",
                get(|| async { Json(json!({"source":"canonical"})) }),
            )
            .route(
                "/api/platform/conversation-activity",
                get(|| async {
                    Body::from_stream(
                        futures::stream::once(async {
                            Ok::<_, std::io::Error>("{\"type\":\"snapshot\"}\n")
                        })
                        .chain(futures::stream::pending()),
                    )
                }),
            );
        let app = rua_server::console_transport::router(state.clone(), api)
            .route("/api/projects", get(|| async { Json(json!([])) }))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let cancelled = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(cancelled.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            address,
            stop,
            task,
            _root: root,
            dropped,
        }
    }
    async fn socket(&self) -> WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>> {
        let mut request = format!("ws://{}/api/console/transport", self.address)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "Origin",
            format!("http://{}", self.address).parse().unwrap(),
        );
        connect_async(request).await.unwrap().0
    }
}
async fn send(socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, frame: Value) {
    socket
        .send(Message::Text(frame.to_string().into()))
        .await
        .unwrap();
}
async fn receive(socket: &mut WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .expect("bounded frame")
        .unwrap()
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

#[tokio::test]
async fn blocked_remote_channel_does_not_block_other_channels_or_local_reads() {
    let fixture = Fixture::new(false).await;
    let mut socket = fixture.socket().await;
    send(&mut socket, json!({"type":"auth", "version":1, "token":""})).await;
    assert_eq!(receive(&mut socket).await["type"], "ready");
    send(&mut socket, json!({"type":"open","id":"slow","method":"GET","path":"/agents/a/chats/slow/acp/permissions"})).await;
    for i in 0..4 {
        send(&mut socket, json!({"type":"open","id":format!("stream-{i}"),"method":"GET","path":"/platform/conversation-activity"})).await;
    }
    send(&mut socket, json!({"type":"open","id":"fast","method":"GET","path":"/agents/a/chats/fast/acp/permissions"})).await;
    let mut fast_body = String::new();
    loop {
        let frame = receive(&mut socket).await;
        if frame["id"] == "fast" {
            if frame["type"] == "data" {
                use base64::Engine;
                fast_body.push_str(
                    &String::from_utf8(
                        base64::engine::general_purpose::STANDARD
                            .decode(frame["data"].as_str().unwrap())
                            .unwrap(),
                    )
                    .unwrap(),
                );
            }
            if frame["type"] == "end" {
                break;
            }
        }
    }
    assert_eq!(
        serde_json::from_str::<Value>(&fast_body).unwrap(),
        json!({"source":"canonical"})
    );
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        reqwest::get(format!("http://{}/api/projects", fixture.address)),
    )
    .await
    .expect("local read independent of blocked peer")
    .unwrap();
    assert!(response.status().is_success());
    assert!(!fixture.dropped.load(Ordering::SeqCst));
    send(&mut socket, json!({"type":"cancel","id":"slow"})).await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while !fixture.dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancel drops only the remote request future");
    send(&mut socket, json!({"type":"open","id":"after-cancel","method":"GET","path":"/agents/a/chats/fast/acp/permissions"})).await;
    loop {
        if receive(&mut socket).await["id"] == "after-cancel" {
            break;
        }
    }
}

#[tokio::test]
async fn transport_rejects_foreign_origin_unauthenticated_mobile_and_mutation_replay() {
    let fixture = Fixture::new(false).await;
    let mut request = format!("ws://{}/api/console/transport", fixture.address)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Origin", "http://untrusted.example".parse().unwrap());
    let error = connect_async(request)
        .await
        .expect_err("foreign origin rejected");
    assert!(
        matches!(error, tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == 403)
    );
    let mut socket = fixture.socket().await;
    send(&mut socket, json!({"type":"auth", "version":1})).await;
    assert_eq!(receive(&mut socket).await["type"], "ready");
    send(&mut socket, json!({"type":"open","id":"mutation","method":"POST","path":"/agents/a/chats/c/console/stop"})).await;
    assert_eq!(receive(&mut socket).await["type"], "error");
    let mobile = Fixture::new(true).await;
    let mut socket = mobile.socket().await;
    send(&mut socket, json!({"type":"auth", "version":1})).await;
    assert_eq!(receive(&mut socket).await["type"], "fatal");
}

#[tokio::test]
async fn unsupported_version_fails_explicitly_before_channel_admission() {
    let fixture = Fixture::new(false).await;
    let mut socket = fixture.socket().await;
    send(&mut socket, json!({"type":"auth", "version":999})).await;
    let frame = receive(&mut socket).await;
    assert_eq!(frame["type"], "fatal");
    assert!(frame["detail"].as_str().unwrap().contains("version"));
}
