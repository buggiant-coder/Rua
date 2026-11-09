use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use base64::Engine;
use rua_channel::feishu::ws::{
    handle_ws_session, handle_ws_session_with_callback, OnCallbackFn, OnMessageFn,
};
use rua_channel::feishu::ws_proto::{decode_frame, encode_frame, Frame, Header};
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
        .expect("bind feishu ws test server");
    let addr = listener.local_addr().expect("ws test server addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve ws test app");
    });
    let url = format!("ws://127.0.0.1:{}/ws", addr.port());
    (url, server)
}

#[tokio::test]
async fn ws_session_dispatches_event_frames() {
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let (url, server) = start_ws_server(|mut socket: WebSocket| async move {
        let event = serde_json::json!({
            "type": "event",
            "data": {
                "header": {
                    "event_type": "im.message.receive_v1",
                    "event_id": "ev-001"
                },
                "event": {
                    "sender": {
                        "sender_id": { "open_id": "ou_test" },
                        "sender_type": "user"
                    },
                    "message": {
                        "message_id": "om_test",
                        "chat_id": "oc_test",
                        "chat_type": "p2p",
                        "message_type": "text",
                        "content": "{\"text\":\"hello\"}"
                    }
                }
            }
        });
        socket
            .send(AxumMessage::Text(
                serde_json::to_string(&event).expect("event json").into(),
            ))
            .await
            .expect("send event frame");
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
        .expect("connect to ws test server")
        .0;
    let disconnected = handle_ws_session(ws_stream, &on_message, &mut stop_rx.clone()).await;

    assert!(disconnected);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["header"]["event_type"], "im.message.receive_v1");

    server.abort();
}

#[tokio::test]
async fn ws_session_writes_protobuf_callback_response_for_card_actions() {
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();
    let callback_payload = serde_json::json!({
        "toast": {"type": "success", "content": "Approved tool Bash"},
        "card": {"type": "raw", "data": {"header": {"title": {"content": "✅ Approved"}}}}
    });
    let callback_payload_for_handler = callback_payload.clone();

    let event = serde_json::json!({
        "schema": "2.0",
        "header": {
            "event_type": "card.action.trigger",
            "event_id": "ev-card-1"
        },
        "event": {
            "action": {
                "value": {
                    "type": "tool_guard_approval",
                    "action": "approve",
                    "request_id": "req-1"
                }
            }
        }
    });
    let frame = Frame {
        seq_id: 7,
        log_id: 9,
        service: 1,
        method: 1,
        headers: vec![Header {
            key: "type".into(),
            value: "event".into(),
        }],
        payload_encoding: None,
        payload_type: None,
        payload: Some(serde_json::to_vec(&event).unwrap()),
        log_id_new: Some("trace-1".into()),
    };

    let (url, server) = start_ws_server(move |mut socket: WebSocket| {
        let bytes = encode_frame(&frame);
        let expected_callback_payload = callback_payload.clone();
        async move {
            socket
                .send(AxumMessage::Binary(bytes.into()))
                .await
                .expect("send protobuf event frame");
            let Some(Ok(AxumMessage::Binary(response_bytes))) = socket.recv().await else {
                panic!("expected protobuf callback response");
            };
            let response_frame = decode_frame(&response_bytes).expect("decode response frame");
            assert_eq!(response_frame.seq_id, 7);
            assert_eq!(response_frame.log_id, 9);
            assert_eq!(response_frame.method, 1);
            assert!(response_frame
                .headers
                .iter()
                .any(|header| header.key == "biz_rt"));
            let response: serde_json::Value =
                serde_json::from_slice(response_frame.payload.as_deref().unwrap())
                    .expect("response JSON");
            assert_eq!(response["code"], 200);
            let data = response["data"].as_str().expect("response data");
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(data)
                .expect("base64 data");
            let value: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
            assert_eq!(value, expected_callback_payload);
            let _ = socket.send(AxumMessage::Close(None)).await;
        }
    })
    .await;

    let on_message: OnMessageFn = Arc::new(move |payload| {
        let store = received_clone.clone();
        tokio::spawn(async move {
            store.lock().await.push(payload);
        });
    });
    let on_callback: OnCallbackFn = Arc::new(move |_| Some(callback_payload_for_handler.clone()));

    let (_stop_tx, stop_rx) = watch::channel(false);
    let ws_stream = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to ws test server")
        .0;
    let disconnected = handle_ws_session_with_callback(
        ws_stream,
        &on_message,
        Some(on_callback),
        &mut stop_rx.clone(),
    )
    .await;

    assert!(disconnected);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["header"]["event_type"], "card.action.trigger");

    server.abort();
}

#[tokio::test]
async fn ws_session_replies_to_ping_with_pong() {
    let (url, server) = start_ws_server(|mut socket: WebSocket| async move {
        socket
            .send(AxumMessage::Ping(vec![1, 2, 3].into()))
            .await
            .expect("send ping");
        if let Some(Ok(AxumMessage::Pong(data))) = socket.recv().await {
            assert_eq!(&*data, &[1, 2, 3]);
        } else {
            panic!("expected pong");
        }
        let _ = socket.send(AxumMessage::Close(None)).await;
    })
    .await;

    let on_message: OnMessageFn = Arc::new(|_| {});
    let (_stop_tx, stop_rx) = watch::channel(false);
    let ws_stream = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to ws test server")
        .0;
    let disconnected = handle_ws_session(ws_stream, &on_message, &mut stop_rx.clone()).await;

    assert!(disconnected);
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
        .expect("connect to ws test server")
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
                let event = serde_json::json!({
                    "type": "event",
                    "data": {
                        "header": { "event_type": "im.message.receive_v1" },
                        "event": {
                            "sender": { "sender_id": { "open_id": "ou_reconnect" } },
                            "message": {
                                "message_id": "om_reconnect",
                                "chat_type": "p2p",
                                "message_type": "text",
                                "content": "{\"text\":\"after reconnect\"}"
                            }
                        }
                    }
                });
                socket
                    .send(AxumMessage::Text(
                        serde_json::to_string(&event).expect("event json").into(),
                    ))
                    .await
                    .expect("send reconnect event");
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
                .expect("connect to ws test server")
                .0;
            let disconnected = handle_ws_session(ws, &on_msg, &mut stop_rx).await;
            if !disconnected {
                break;
            }
        }
    });

    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("reconnect did not complete within 5s")
        .expect("join reconnect task");

    assert!(connect_count.load(Ordering::SeqCst) >= 2);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["event"]["message"]["message_id"], "om_reconnect");

    server.abort();
}

#[tokio::test]
async fn custom_domain_uses_sdk_handshake_heartbeat_fragments_and_json_ack() {
    use axum::{extract::State, routing::post, Json, Router};
    use rua_channel::core::channel::{Channel, OutMessage, ToHandle};
    use rua_channel::core::content::OutgoingContentPart;
    use rua_channel::feishu::ws::{spawn_ws_task_with_base_url, WsConfig};
    use rua_channel::feishu::{FeishuChannel, FeishuConfig};
    use serde_json::{json, Value};

    #[derive(Clone)]
    struct Api {
        addr: std::net::SocketAddr,
        sockets: tokio::sync::mpsc::UnboundedSender<WebSocket>,
        sent: tokio::sync::mpsc::UnboundedSender<Value>,
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (sockets, mut connections) = tokio::sync::mpsc::unbounded_channel();
    let (sent, mut outgoing) = tokio::sync::mpsc::unbounded_channel();
    let app = Router::new()
        .route("/callback/ws/endpoint", post(|State(api): State<Api>, Json(body): Json<Value>| async move {
            assert_eq!(body, json!({"AppID":"app","AppSecret":"secret"}));
            Json(json!({"code":0,"data":{"URL":format!("ws://{}/ws?service_id=9", api.addr),"ClientConfig":{"PingInterval":1}}}))
        }))
        .route("/open-apis/auth/v3/tenant_access_token/internal", post(|Json(body): Json<Value>| async move {
            assert_eq!(body, json!({"app_id":"app","app_secret":"secret"}));
            Json(json!({"code":0,"tenant_access_token":"local-tenant-token","expire":7200}))
        }))
        .route("/open-apis/im/v1/messages", post(|State(api): State<Api>, headers: axum::http::HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer local-tenant-token");
            api.sent.send(body).unwrap();
            Json(json!({"code":0,"data":{"message_id":"m1"}}))
        }))
        .route("/ws", get(|State(api): State<Api>, ws: WebSocketUpgrade| async move {
            ws.on_upgrade(move |socket| async move { api.sockets.send(socket).unwrap(); })
        }))
        .with_state(Api { addr, sockets, sent });
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");
    let (stop_tx, stop_rx) = watch::channel(false);
    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let task = spawn_ws_task_with_base_url(
        WsConfig {
            app_id: "app".into(),
            app_secret: "secret".into(),
        },
        base.clone(),
        Arc::new(move |event| {
            events_tx.send(event).unwrap();
        }),
        None,
        stop_rx,
    );
    let mut socket = tokio::time::timeout(Duration::from_secs(5), connections.recv())
        .await
        .unwrap()
        .unwrap();
    let heartbeat = tokio::time::timeout(Duration::from_secs(3), socket.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let AxumMessage::Binary(bytes) = heartbeat else {
        panic!("expected protobuf heartbeat")
    };
    let frame = decode_frame(&bytes).unwrap();
    assert_eq!(frame.method, 0);
    assert_eq!(frame.service, 9);
    assert_eq!(frame.header_value("type"), Some("ping"));

    let payload = serde_json::to_vec(&json!({"event":{"text":"fragmented 中文"}})).unwrap();
    let mid = payload.len() / 2;
    for (index, part) in [&payload[..mid], &payload[mid..]].into_iter().enumerate() {
        let frame = Frame {
            method: 1,
            service: 9,
            headers: vec![
                Header {
                    key: "type".into(),
                    value: "event".into(),
                },
                Header {
                    key: "message_id".into(),
                    value: "fragment-1".into(),
                },
                Header {
                    key: "sum".into(),
                    value: "2".into(),
                },
                Header {
                    key: "seq".into(),
                    value: index.to_string(),
                },
            ],
            payload: Some(part.to_vec()),
            ..Default::default()
        };
        socket
            .send(AxumMessage::Binary(encode_frame(&frame).into()))
            .await
            .unwrap();
    }
    let event = tokio::time::timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event["event"]["text"], "fragmented 中文");
    let ack = tokio::time::timeout(Duration::from_secs(3), socket.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let AxumMessage::Binary(bytes) = ack else {
        panic!("expected protobuf ACK")
    };
    let frame = decode_frame(&bytes).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(frame.payload.as_deref().unwrap()).unwrap(),
        json!({"code":200})
    );

    let channel = FeishuChannel::new(FeishuConfig {
        app_id: Some("app".into()),
        app_secret: Some("secret".into()),
        ..Default::default()
    })
    .with_domain(&base)
    .unwrap();
    channel
        .send(
            ToHandle {
                agent_id: "default".into(),
                conversation_id: "chat1".into(),
                sender_id: None,
                reply_to: None,
                metadata: json!({"feishu_chat_type":"group","feishu_chat_id":"chat1"}),
            },
            OutMessage {
                channel_id: "feishu".into(),
                conversation_id: "chat1".into(),
                receiver_id: None,
                reply_to: None,
                content: vec![OutgoingContentPart::Text {
                    text: "hello".into(),
                }],
                metadata: json!({}),
            },
        )
        .await
        .unwrap();
    let message = outgoing.recv().await.unwrap();
    assert_eq!(message["receive_id"], "chat1");
    assert_eq!(
        serde_json::from_str::<Value>(message["content"].as_str().unwrap()).unwrap()["text"],
        "hello"
    );
    stop_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    server.abort();
}
