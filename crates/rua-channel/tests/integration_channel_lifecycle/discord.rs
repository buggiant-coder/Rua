use super::support::{http_proxy, recv, reply_process, wait_for_len};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        WebSocketUpgrade,
    },
    routing::{get, post},
    Json, Router,
};
use rua_channel::{
    core::channel::Channel,
    discord::{DiscordChannel, DiscordConfig},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

#[tokio::test]
async fn gateway_and_replies_use_proxy_and_restart_with_identify() {
    let (sockets, mut connections) = mpsc::unbounded_channel::<WebSocket>();
    let replies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = replies.clone();
    let app = Router::new()
        .route(
            "/gateway",
            get(move |ws: WebSocketUpgrade| {
                let sockets = sockets.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        sockets.send(socket).unwrap();
                    })
                }
            }),
        )
        .route(
            "/channels/{id}/messages",
            post(move |Json(body): Json<Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().await.push(body);
                    Json(json!({"id":"reply"}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (proxy_url, proxy_requests, proxy) = http_proxy(addr).await;
    let channel=DiscordChannel::new(DiscordConfig {bot_token:"token".into(),bot_user_id:Some("bot".into()),..Default::default()})
        .with_api_base(format!("http://{addr}")).with_gateway_url(format!("ws://{addr}/gateway"))
        .with_client(reqwest::Client::builder().no_proxy().pool_max_idle_per_host(0).proxy(reqwest::Proxy::all(proxy_url).unwrap().basic_auth("proxy-user","proxy-pass")).build().unwrap())
        .with_acl_config(json!({"streaming_enabled":false,"show_typing":false,"no_text_debounce":true,"accept_bot_messages":true}));
    let (process, mut requests) = reply_process("proxied answer");
    for n in 0..2 {
        channel.start(process.clone()).await.unwrap();
        let mut socket = recv(&mut connections).await;
        socket
            .send(Message::Text(
                json!({"op":10,"d":{"heartbeat_interval":60000}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        let identify: Value =
            serde_json::from_str(socket.recv().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(identify["op"], 2);
        assert_eq!(identify["d"]["token"], "token");
        socket.send(Message::Text(json!({"op":0,"s":1,"t":"READY","d":{"session_id":"session","resume_gateway_url":format!("ws://{addr}/gateway")}}).to_string().into())).await.unwrap();
        for user in ["bot", "another-bot"] {
            socket.send(Message::Text(json!({"op":0,"s":2,"t":"MESSAGE_CREATE","d":{
                "id":format!("{n}-{user}"),"channel_id":"channel","author":{"id":user,"username":user,"bot":true},"content":"question"
            }}).to_string().into())).await.unwrap();
        }
        assert_eq!(recv(&mut requests).await.user_id, "another-bot");
        wait_for_len(&replies, n + 1).await;
        assert_eq!(replies.lock().await[n]["content"], "proxied answer");
        assert!(requests.try_recv().is_err());
        channel.stop().await.unwrap();
    }
    let requests = proxy_requests.lock().await;
    assert!(requests
        .iter()
        .any(|r| r.starts_with(&format!("GET http://{addr}/gateway?"))));
    assert!(requests
        .iter()
        .any(|r| r.starts_with(&format!("POST http://{addr}/channels/channel/messages"))));
    assert!(requests.iter().all(|r| r.to_ascii_lowercase().contains(
        "proxy-authorization: basic cHJveHktdXNlcjpwcm94eS1wYXNz"
            .to_ascii_lowercase()
            .as_str()
    )));
    proxy.abort();
    server.abort();
}
