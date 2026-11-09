use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use rua_channel::dingtalk::ws::{handle_ws_session, OnMessageFn};
use tokio::sync::{watch, Mutex};

async fn start_ws_server<F, Fut>(handler: F) -> (String, tokio::task::JoinHandle<()>)
where
    F: Fn(WebSocket) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let app = axum::Router::new().route(
        "/ws",
        get(move |ws: WebSocketUpgrade| {
            let h = handler.clone();
            async move { ws.on_upgrade(h).into_response() }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind dingtalk ws server");
    let addr = listener.local_addr().expect("dingtalk ws addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve dingtalk ws");
    });
    let url = format!("ws://127.0.0.1:{}/ws", addr.port());
    (url, server)
}

fn callback_frame(message_id: &str, content: &str) -> serde_json::Value {
    serde_json::json!({
        "specVersion": "1.0",
        "type": "CALLBACK",
        "headers": {
            "messageId": message_id,
            "topic": "/v1.0/im/bot/messages/get",
            "contentType": "application/json"
        },
        "data": format!(
            "{{\"msgType\":\"text\",\"text\":{{\"content\":\"{content}\"}},\"senderId\":\"u1\",\"conversationId\":\"c1\",\"conversationType\":\"1\",\"msgId\":\"m1\",\"senderNick\":\"Test\",\"robotCode\":\"bot1\",\"sessionWebhook\":\"http://example.com/hook\"}}"
        )
    })
}

#[tokio::test]
async fn ws_session_acks_callback_and_dispatches_payload() {
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let (url, server) = start_ws_server(|mut socket: WebSocket| async move {
        let callback = callback_frame("cb-test-001", "hello");
        socket
            .send(AxumMessage::Text(
                serde_json::to_string(&callback)
                    .expect("callback json")
                    .into(),
            ))
            .await
            .expect("send callback");

        if let Some(Ok(AxumMessage::Text(ack_text))) = socket.recv().await {
            let ack: serde_json::Value = serde_json::from_str(&ack_text).expect("ack json");
            assert_eq!(ack["code"], 200);
            assert_eq!(ack["headers"]["messageId"], "cb-test-001");
            assert_eq!(ack["message"], "OK");
        } else {
            panic!("expected callback ack");
        }

        let _ = socket.send(AxumMessage::Close(None)).await;
    })
    .await;

    let on_message: OnMessageFn = Arc::new(move |payload| {
        let store = received_clone.clone();
        tokio::spawn(async move {
            store.lock().await.push(payload);
        });
    });
    let (_stop_tx, stop_rx) = watch::channel(false);
    let ws_stream = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect dingtalk ws")
        .0;

    let disconnected = handle_ws_session(ws_stream, &on_message, &mut stop_rx.clone()).await;
    assert!(disconnected);

    tokio::time::sleep(Duration::from_millis(50)).await;
    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["msgType"], "text");
    assert_eq!(msgs[0]["text"]["content"], "hello");

    server.abort();
}

#[tokio::test]
async fn ws_session_acks_system_ping() {
    let (url, server) = start_ws_server(|mut socket: WebSocket| async move {
        let ping = serde_json::json!({
            "specVersion": "1.0",
            "type": "SYSTEM",
            "headers": {
                "topic": "ping",
                "messageId": "hb-test-001"
            }
        });
        socket
            .send(AxumMessage::Text(
                serde_json::to_string(&ping).expect("ping json").into(),
            ))
            .await
            .expect("send ping");

        if let Some(Ok(AxumMessage::Text(ack_text))) = socket.recv().await {
            let ack: serde_json::Value = serde_json::from_str(&ack_text).expect("ack json");
            assert_eq!(ack["code"], 200);
            assert_eq!(ack["headers"]["messageId"], "hb-test-001");
        } else {
            panic!("expected heartbeat ack");
        }

        let _ = socket.send(AxumMessage::Close(None)).await;
    })
    .await;

    let on_message: OnMessageFn = Arc::new(|_| {});
    let (_stop_tx, stop_rx) = watch::channel(false);
    let ws_stream = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect dingtalk ws")
        .0;

    let disconnected = handle_ws_session(ws_stream, &on_message, &mut stop_rx.clone()).await;
    assert!(disconnected);

    server.abort();
}

#[tokio::test]
async fn ws_session_sends_client_heartbeat_ping_for_liveness() {
    let observed_ping = Arc::new(AtomicUsize::new(0));
    let observed_ping_clone = observed_ping.clone();
    let (url, server) = start_ws_server(move |mut socket: WebSocket| {
        let observed_ping = observed_ping_clone.clone();
        async move {
            if let Some(Ok(AxumMessage::Ping(_))) =
                tokio::time::timeout(Duration::from_secs(2), socket.recv())
                    .await
                    .expect("client heartbeat ping should arrive")
            {
                observed_ping.fetch_add(1, Ordering::SeqCst);
            } else {
                panic!("expected client heartbeat ping");
            }

            let _ = socket.send(AxumMessage::Close(None)).await;
        }
    })
    .await;

    let on_message: OnMessageFn = Arc::new(|_| {});
    let (_stop_tx, stop_rx) = watch::channel(false);
    let ws_stream = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect dingtalk ws")
        .0;

    let disconnected = handle_ws_session(ws_stream, &on_message, &mut stop_rx.clone()).await;
    assert!(disconnected);
    assert_eq!(observed_ping.load(Ordering::SeqCst), 1);

    server.abort();
}

#[tokio::test]
async fn ws_session_exits_for_server_disconnect_notification() {
    let (url, server) = start_ws_server(|mut socket: WebSocket| async move {
        let disconnect = serde_json::json!({
            "specVersion": "1.0",
            "type": "SYSTEM",
            "headers": {
                "topic": "disconnect",
                "messageId": "disconnect-test-001"
            }
        });
        socket
            .send(AxumMessage::Text(
                serde_json::to_string(&disconnect)
                    .expect("disconnect json")
                    .into(),
            ))
            .await
            .expect("send disconnect");

        tokio::time::sleep(Duration::from_secs(30)).await;
    })
    .await;

    let on_message: OnMessageFn = Arc::new(|_| {});
    let (_stop_tx, stop_rx) = watch::channel(false);
    let ws_stream = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect dingtalk ws")
        .0;

    let disconnected = tokio::time::timeout(
        Duration::from_secs(2),
        handle_ws_session(ws_stream, &on_message, &mut stop_rx.clone()),
    )
    .await
    .expect("server disconnect notification should end the session");
    assert!(
        disconnected,
        "server disconnect must hand control back to the outer reconnect loop"
    );

    server.abort();
}

#[tokio::test]
async fn ws_session_can_be_reconnected_by_outer_loop() {
    let connect_count = Arc::new(AtomicUsize::new(0));
    let connect_count_clone = connect_count.clone();
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let (url, server) = start_ws_server(move |mut socket: WebSocket| {
        let count = connect_count_clone.clone();
        async move {
            let n = count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                let _ = socket.send(AxumMessage::Close(None)).await;
            } else {
                let callback = callback_frame("reconnect-msg", "after reconnect");
                socket
                    .send(AxumMessage::Text(
                        serde_json::to_string(&callback)
                            .expect("callback json")
                            .into(),
                    ))
                    .await
                    .expect("send reconnect callback");
                let _ = socket.recv().await;
                let _ = socket.send(AxumMessage::Close(None)).await;
            }
        }
    })
    .await;

    let on_message: OnMessageFn = Arc::new(move |payload| {
        let store = received_clone.clone();
        tokio::spawn(async move {
            store.lock().await.push(payload);
        });
    });
    let (_stop_tx, mut stop_rx) = watch::channel(false);
    let on_msg = on_message.clone();

    let task = tokio::spawn(async move {
        for _ in 0..2 {
            if *stop_rx.borrow() {
                break;
            }
            let ws = tokio_tungstenite::connect_async(&url)
                .await
                .expect("connect dingtalk ws")
                .0;
            let disconnected = handle_ws_session(ws, &on_msg, &mut stop_rx).await;
            if !disconnected {
                break;
            }
        }
    });

    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("reconnect timed out")
        .expect("join reconnect task");

    assert!(connect_count.load(Ordering::SeqCst) >= 2);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["text"]["content"], "after reconnect");

    server.abort();
}

#[tokio::test]
async fn ws_session_stop_signal_exits_without_reconnect() {
    let (url, server) = start_ws_server(|mut socket: WebSocket| async move {
        while let Some(Ok(_)) = socket.recv().await {}
    })
    .await;

    let on_message: OnMessageFn = Arc::new(|_| {});
    let (stop_tx, mut stop_rx) = watch::channel(false);
    let ws_stream = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect dingtalk ws")
        .0;
    let task =
        tokio::spawn(async move { handle_ws_session(ws_stream, &on_message, &mut stop_rx).await });

    tokio::time::sleep(Duration::from_millis(100)).await;
    stop_tx.send(true).expect("send stop");

    let disconnected = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("shutdown timed out")
        .expect("join ws session");

    assert!(!disconnected);
    server.abort();
}
