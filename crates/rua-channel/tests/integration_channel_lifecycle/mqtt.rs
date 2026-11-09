use super::support::{recv, reply_process};
use rua_channel::core::channel::Channel;
use rua_channel::mqtt::types::MqttQos;
use rua_channel::mqtt::{MqttChannel, MqttConfig};
use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn read_packet(socket: &mut (impl tokio::io::AsyncRead + Unpin)) -> (u8, Vec<u8>) {
    let kind = socket.read_u8().await.unwrap();
    let mut length = 0usize;
    let mut multiplier = 1;
    loop {
        let byte = socket.read_u8().await.unwrap();
        length += (byte & 127) as usize * multiplier;
        if byte & 128 == 0 {
            break;
        }
        multiplier *= 128;
    }
    let mut data = vec![0; length];
    socket.read_exact(&mut data).await.unwrap();
    (kind, data)
}

async fn publish(socket: &mut (impl tokio::io::AsyncWrite + Unpin), topic: &str, text: &str) {
    let mut data = Vec::new();
    data.extend_from_slice(&(topic.len() as u16).to_be_bytes());
    data.extend_from_slice(topic.as_bytes());
    data.extend_from_slice(text.as_bytes());
    let mut header = vec![0x30];
    let mut length = data.len();
    loop {
        let byte = (length % 128) as u8;
        length /= 128;
        header.push(byte | if length > 0 { 128 } else { 0 });
        if length == 0 {
            break;
        }
    }
    socket.write_all(&header).await.unwrap();
    socket.write_all(&data).await.unwrap();
}

#[tokio::test]
async fn inbound_publish_is_processed_and_replied_on_device_topic_after_restart() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (reply_tx, mut replies) = tokio::sync::mpsc::unbounded_channel();
    let broker = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (kind, _) = read_packet(&mut socket).await;
            assert_eq!(kind >> 4, 1);
            socket.write_all(&[0x20, 2, 0, 0]).await.unwrap();
            let (kind, subscription) = read_packet(&mut socket).await;
            assert_eq!(kind >> 4, 8);
            socket
                .write_all(&[0x90, 3, subscription[0], subscription[1], 0])
                .await
                .unwrap();
            let payload = json!({"text":"hello","id":"same-event","redirect_client_id":"device-a"})
                .to_string();
            publish(&mut socket, "clients/device-a/in", &payload).await;
            publish(&mut socket, "clients/device-a/in", &payload).await;
            let (kind, reply) = read_packet(&mut socket).await;
            assert_eq!(kind >> 4, 3);
            let topic_len = u16::from_be_bytes([reply[0], reply[1]]) as usize;
            let topic = String::from_utf8(reply[2..2 + topic_len].to_vec()).unwrap();
            let text = String::from_utf8(reply[2 + topic_len..].to_vec()).unwrap();
            reply_tx.send((topic, text)).unwrap();
            // Observe an actual transport disconnect before the next start.
            let mut rest = [0u8; 16];
            let _ = socket.read(&mut rest).await;
        }
    });
    let channel = MqttChannel::new(MqttConfig {
        broker_url: "127.0.0.1".into(),
        port,
        subscribe_topic: "clients/+/in".into(),
        publish_topic: "clients/{client_id}/out".into(),
        qos: MqttQos::AtMostOnce,
        channel_capacity: 1, // Reproduces deadlock if replies run inside EventLoop::poll.
        ..Default::default()
    });
    let (process, mut requests) = reply_process("mqtt answer");
    for _ in 0..2 {
        channel.start(process.clone()).await.unwrap();
        let request = recv(&mut requests).await;
        assert_eq!(request.user_id, "device-a");
        assert_eq!(
            recv(&mut replies).await,
            ("clients/device-a/out".into(), "mqtt answer".into())
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), requests.recv())
                .await
                .is_err()
        );
        channel.stop().await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(5), broker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[allow(clippy::result_large_err)] // tungstenite requires an HTTP response as the handshake error.
async fn websocket_transport_uses_mqtt_subprotocol_and_delivers_agent_reply() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{
        handshake::server::{Request, Response},
        Message,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let broker = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let socket = tokio_tungstenite::accept_hdr_async(
            stream,
            |request: &Request, mut response: Response| {
                assert_eq!(request.uri().path(), "/mqtt");
                assert_eq!(request.headers()["sec-websocket-protocol"], "mqtt");
                response
                    .headers_mut()
                    .insert("Sec-WebSocket-Protocol", "mqtt".parse().unwrap());
                Ok(response)
            },
        )
        .await
        .unwrap();
        let (mut sink, mut stream) = socket.split();
        let (mut broker, bridge) = tokio::io::duplex(65536);
        let (mut read, mut write) = tokio::io::split(bridge);
        let bridge_task = tokio::spawn(async move {
            let mut bytes = [0u8; 8192];
            loop {
                tokio::select! {
                    frame=stream.next()=>match frame {
                        Some(Ok(Message::Binary(bytes)))=> {write.write_all(&bytes).await.unwrap();},
                        Some(Ok(Message::Ping(bytes)))=>{sink.send(Message::Pong(bytes)).await.unwrap();},
                        _=>break,
                    },
                    n=read.read(&mut bytes)=>{
                        let n=n.unwrap();if n==0 {break;}
                        sink.send(Message::Binary(bytes[..n].to_vec().into())).await.unwrap();
                    }
                }
            }
        });
        let (kind, _) = read_packet(&mut broker).await;
        assert_eq!(kind >> 4, 1);
        broker.write_all(&[0x20, 2, 0, 0]).await.unwrap();
        let (kind, subscription) = read_packet(&mut broker).await;
        assert_eq!(kind >> 4, 8);
        broker
            .write_all(&[0x90, 3, subscription[0], subscription[1], 0])
            .await
            .unwrap();
        publish(
            &mut broker,
            "clients/device-ws/in",
            &json!({"text":"hello","redirect_client_id":"device-ws"}).to_string(),
        )
        .await;
        let (kind, reply) = read_packet(&mut broker).await;
        assert_eq!(kind >> 4, 3);
        let topic_len = u16::from_be_bytes([reply[0], reply[1]]) as usize;
        assert_eq!(&reply[2..2 + topic_len], b"clients/device-ws/out");
        assert_eq!(&reply[2 + topic_len..], b"ws answer");
        bridge_task.abort();
    });
    let channel = MqttChannel::new(MqttConfig {
        broker_url: "127.0.0.1".into(),
        port,
        subscribe_topic: "clients/+/in".into(),
        publish_topic: "clients/{client_id}/out".into(),
        qos: MqttQos::AtMostOnce,
        ..Default::default()
    })
    .with_transport("websockets".into());
    let (process, mut requests) = reply_process("ws answer");
    channel.start(process).await.unwrap();
    assert_eq!(recv(&mut requests).await.user_id, "device-ws");
    tokio::time::timeout(Duration::from_secs(5), broker)
        .await
        .unwrap()
        .unwrap();
    channel.stop().await.unwrap();
}
