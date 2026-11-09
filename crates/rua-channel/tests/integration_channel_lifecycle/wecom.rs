use super::support::{recv, reply_process};
use futures::{SinkExt, StreamExt};
use rua_channel::{core::channel::Channel, wecom::WecomChannel};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn welcome_uses_event_request_id_and_channel_restarts() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (tx, mut connections) = mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tx.send(tokio_tungstenite::accept_async(stream).await.unwrap())
                .unwrap();
        }
    });
    let channel = WecomChannel::new("bot-1".into(), "secret".into())
        .with_ws_url(url)
        .with_acl_config(json!({"welcome_text":"Welcome!","max_reconnect_attempts":0}));
    let (process, mut requests) = reply_process("answer");
    for _ in 0..2 {
        channel.start(Arc::clone(&process)).await.unwrap();
        let mut socket = recv(&mut connections).await;
        let auth: Value =
            serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(auth["cmd"], "aibot_subscribe");
        socket
            .send(Message::Text(
                json!({"headers":auth["headers"],"errcode":0})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        socket.send(Message::Text(json!({"cmd":"aibot_event_callback","headers":{"req_id":"event-1"},"body":{"event":{"eventtype":"enter_chat"}}}).to_string().into())).await.unwrap();
        let welcome = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let frame: Value =
                    serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                if frame["cmd"] == "aibot_respond_welcome_msg" {
                    break frame;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(welcome["headers"]["req_id"], "event-1");
        assert_eq!(welcome["body"]["text"]["content"], "Welcome!");
        assert!(requests.try_recv().is_err());
        channel.stop().await.unwrap();
    }
    server.abort();
}

#[tokio::test]
async fn zero_reconnect_limit_stops_after_failed_authentication() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let channel = WecomChannel::new("bot".into(), "secret".into())
        .with_ws_url(format!("ws://{}", listener.local_addr().unwrap()))
        .with_acl_config(json!({"max_reconnect_attempts":0}));
    channel.start(reply_process("unused").0).await.unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    let auth: Value =
        serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    socket
        .send(Message::Text(
            json!({"headers":auth["headers"],"errcode":40001})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(1300), listener.accept())
            .await
            .is_err()
    );
    channel.stop().await.unwrap();
}
