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

/// Credential these transport tests present. The transport is an authenticated
/// surface for every profile, so the fixture uses a real static-token
/// `AuthService` instead of pretending authentication can be switched off.
const TRANSPORT_TOKEN: &str = "console-transport-contract-token";

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
        state.auth = Arc::new(rua_server::auth::AuthService::new_with_token(
            TRANSPORT_TOKEN.to_string(),
        ));
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
            )
            // An unbounded body. A finite one only forces server-side backlog if
            // it happens to be bigger than the kernel socket buffer, which makes
            // the assertion depend on host tuning instead of on the behaviour
            // under test. The channel is cancelled as soon as the server reports
            // loss, so this never streams forever in practice.
            .route(
                "/api/agents/a/chats/big/console/activity",
                get(|| async {
                    Body::from_stream(futures::stream::unfold((), |()| async {
                        Some((
                            Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b'x'; 16 * 1024])),
                            (),
                        ))
                    }))
                }),
            )
            // Exactly one maximum-size Console payload (256 KiB), delivered as a
            // single body chunk. A channel queue that cannot hold one whole
            // payload kills this stream even when the client reads normally.
            .route(
                "/api/agents/a/chats/payload/acp/permissions",
                get(|| async {
                    Body::from_stream(futures::stream::iter(vec![Ok::<_, std::io::Error>(
                        axum::body::Bytes::from(vec![b'y'; 256 * 1024]),
                    )]))
                }),
            );
        let app = rua_server::console_transport::router(state.clone(), api, None)
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
    send(&mut socket, json!({"type":"auth", "version":1, "token": TRANSPORT_TOKEN})).await;
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
async fn transport_rejects_foreign_origin_unauthenticated_and_mutation_replay() {
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

    // No credential at all: the transport rejects before admitting a channel.
    let mut socket = fixture.socket().await;
    send(&mut socket, json!({"type":"auth", "version":1})).await;
    assert_eq!(receive(&mut socket).await["type"], "fatal");

    // Mutation replay stays out of the closed allowlist even when authenticated.
    let mut socket = fixture.socket().await;
    send(&mut socket, json!({"type":"auth", "version":1, "token": TRANSPORT_TOKEN})).await;
    assert_eq!(receive(&mut socket).await["type"], "ready");
    send(&mut socket, json!({"type":"open","id":"mutation","method":"POST","path":"/agents/a/chats/c/console/stop"})).await;
    assert_eq!(receive(&mut socket).await["type"], "error");
}

#[tokio::test]
async fn mobile_transport_requires_a_credential() {
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

#[tokio::test]
async fn server_backlog_is_reported_as_server_side_loss_not_client_buffer_pressure() {
    let fixture = Fixture::new(false).await;
    let mut socket = fixture.socket().await;
    send(&mut socket, json!({"type":"auth", "version":1, "token": TRANSPORT_TOKEN})).await;
    assert_eq!(receive(&mut socket).await["type"], "ready");
    send(
        &mut socket,
        json!({"type":"open","id":"big","method":"GET","path":"/agents/a/chats/big/console/activity"}),
    )
    .await;
    // Backpressure has to be forced, not raced for. Draining the socket while
    // polling would let the client keep pace with the producer, so whether the
    // server ever reports loss would depend on the relative throughput of two
    // tasks on a loaded host. Not reading at all for a fixed settle period
    // instead guarantees the kernel socket buffer and then the per-channel
    // queue fill, so `enqueue` always takes the loss path.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let gap = loop {
        let mut found = None;
        while let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(100), socket.next())
                .await
        {
            let frame: Value = serde_json::from_str(&text).unwrap();
            if frame["id"] == "big" && frame["type"] == "gap" {
                found = Some(frame);
                break;
            }
        }
        if let Some(frame) = found {
            break frame;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "server must report backlog for a channel the client stopped draining"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(gap["code"], "server_channel_backlog");
    let detail = gap["detail"].as_str().unwrap();
    assert!(detail.contains("[server-backlog]"), "detail: {detail}");
    assert!(
        !detail.contains("[client-buffer]"),
        "server backlog must not be attributed to the client buffer: {detail}"
    );
}

#[test]
fn server_deadline_signal_is_distinguishable_from_the_client_timer() {
    // The page runs its own deadline with the same user-facing wording. The
    // machine-readable discriminator is what tells the two apart in the field.
    let frame = rua_server::console_transport::server_deadline_frame("op-1");
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["id"], "op-1");
    assert_eq!(frame["code"], "server_request_deadline");
    let detail = frame["detail"].as_str().unwrap();
    assert!(detail.contains("[server-deadline]"), "detail: {detail}");
    assert!(
        !detail.contains("[client-deadline]"),
        "server deadline must not be attributed to the page timer: {detail}"
    );
}

#[tokio::test]
async fn a_single_maximum_payload_is_delivered_completely_to_a_draining_client() {
    // Regression: a channel queue smaller than one payload (16 frames at 16 KiB)
    // cannot ever flush a maximum-size chunk. The old 8-slot queue let only 7
    // frames (112 KiB) through, so an ordinary burst of one full payload killed
    // the stream with a bogus "slow channel" backlog even though this client
    // reads continuously.
    let fixture = Fixture::new(false).await;
    let mut socket = fixture.socket().await;
    send(&mut socket, json!({"type":"auth", "version":1, "token": TRANSPORT_TOKEN})).await;
    assert_eq!(receive(&mut socket).await["type"], "ready");
    send(
        &mut socket,
        json!({"type":"open","id":"payload","method":"GET","path":"/agents/a/chats/payload/acp/permissions"}),
    )
    .await;

    let mut body = Vec::new();
    loop {
        let frame = receive(&mut socket).await;
        assert_eq!(frame["id"], "payload");
        match frame["type"].as_str().unwrap() {
            "head" => {}
            "data" => {
                use base64::Engine;
                body.extend_from_slice(
                    &base64::engine::general_purpose::STANDARD
                        .decode(frame["data"].as_str().unwrap())
                        .unwrap(),
                );
            }
            "end" => break,
            other => panic!("unexpected frame {other}: {frame}"),
        }
    }
    assert_eq!(
        body.len(),
        256 * 1024,
        "the whole payload must arrive instead of being truncated by a backlog"
    );
    assert!(body.iter().all(|byte| *byte == b'y'));
}

/// Every allowlisted subscription must be classified as long-lived, or the
/// transport's 30s bounded-read deadline tears it down on a timer.
///
/// `/events` is how the Runtime publishes user notifications, and it reached the
/// allowlist without reaching this classification — which would have made
/// notifications stop silently 30s after the console started.
#[test]
fn allowlisted_sse_endpoints_are_classified_as_persistent_streams() {
    for path in [
        "/platform/user-notifications/events",
        "/platform/conversation-activity",
        "/sync/devices/activity",
        "/terminal/sessions/abc/activity",
        "/agents/a/chats/c/console/activity",
    ] {
        assert!(
            rua_server::console_transport::is_persistent_stream(path),
            "{path} must not be killed by the bounded-read deadline"
        );
    }

    // Bounded reads still expire, otherwise a stuck snapshot pins a channel.
    for path in [
        "/agents/a/approvals/snapshot",
        "/agents/a/chats/c/acp/permissions",
    ] {
        assert!(
            !rua_server::console_transport::is_persistent_stream(path),
            "{path} is a bounded read and must keep its deadline"
        );
    }
}
