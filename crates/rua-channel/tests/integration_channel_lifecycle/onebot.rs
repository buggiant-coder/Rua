use super::support::{recv, reply_process};
use futures::{SinkExt, StreamExt};
use rua_channel::core::channel::Channel;
use rua_channel::core::content::{ChannelTarget, OutgoingContentPart, OutgoingMessage};
use rua_channel::onebot::{OneBotChannel, OneBotConfig};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

async fn connect(
    port: u16,
    token: Option<&str>,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = format!("ws://127.0.0.1:{port}/")
        .into_client_request()
        .unwrap();
    if let Some(token) = token {
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse().unwrap());
    }
    tokio_tungstenite::connect_async(request).await.unwrap().0
}

#[tokio::test]
async fn reverse_socket_replies_to_origin_connection_and_restarts_without_http() {
    let reserved = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = reserved.local_addr().unwrap().port();
    drop(reserved);
    let channel = Arc::new(OneBotChannel::new(OneBotConfig {
        ws_port: port,
        access_token: Some("test-secret".into()),
        ..Default::default()
    }));
    let (process, mut requests) = reply_process("onebot reply");
    channel.start(process.clone()).await.unwrap();
    assert!(
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/"))
            .await
            .is_err()
    );
    let mut first = connect(port, Some("test-secret")).await;
    let mut second = connect(port, Some("test-secret")).await;
    let event = json!({
        "post_type":"message","message_type":"group","user_id":10,"group_id":20,
        "self_id":30,"message_id":101,"message":[{"type":"text","data":{"text":"hello"}}]
    });
    second
        .send(Message::Text(event.to_string().into()))
        .await
        .unwrap();
    let request = recv(&mut requests).await;
    let action = tokio::time::timeout(Duration::from_secs(5), second.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let action: Value = serde_json::from_str(action.to_text().unwrap()).unwrap();
    assert_eq!(action["action"], "send_msg");
    assert_eq!(action["params"]["group_id"], 20);
    assert_eq!(
        action["params"]["message"][0]["data"]["text"],
        "onebot reply"
    );
    second
        .send(Message::Text(
            json!({"echo":action["echo"],"retcode":0,"status":"ok","data":{"message_id":111}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    second
        .send(Message::Text(event.to_string().into()))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), requests.recv())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), first.next())
            .await
            .is_err()
    );

    let channel_send = channel.clone();
    let sending = tokio::spawn(async move {
        channel_send
            .send(
                ChannelTarget {
                    agent_id: String::new(),
                    conversation_id: "group_20".into(),
                    sender_id: Some("10".into()),
                    reply_to: None,
                    metadata: request.metadata,
                },
                OutgoingMessage {
                    channel_id: "onebot".into(),
                    conversation_id: "group_20".into(),
                    receiver_id: None,
                    content: vec![OutgoingContentPart::Text {
                        text: "manual".into(),
                    }],
                    reply_to: None,
                    metadata: json!({}),
                },
            )
            .await
    });
    let action: Value =
        serde_json::from_str(second.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    second
        .send(Message::Text(
            json!({"echo":action["echo"],"retcode":100,"status":"failed","data":{}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert!(sending
        .await
        .unwrap()
        .unwrap_err()
        .to_string()
        .contains("failed"));
    channel.stop().await.unwrap();
    channel.start(process).await.unwrap();
    let mut fresh = connect(port, Some("test-secret")).await;
    fresh
        .send(Message::Text(event.to_string().into()))
        .await
        .unwrap();
    let _ = recv(&mut requests).await;
    channel.stop().await.unwrap();
}

#[tokio::test]
async fn bind_failures_are_reported_and_empty_local_token_allows_connecting() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let channel = OneBotChannel::new(OneBotConfig {
        ws_port: port,
        access_token: Some(String::new()),
        ..Default::default()
    });
    let (process, _) = reply_process("ok");
    assert!(channel
        .start(process.clone())
        .await
        .unwrap_err()
        .to_string()
        .contains("bind"));
    drop(listener);
    channel.start(process).await.unwrap();
    let _socket = connect(port, None).await;
    channel.stop().await.unwrap();
}
