use rua_channel::core::channel::Channel;
use rua_channel::core::content::{IncomingContentPart, OutgoingContentPart};
use rua_channel::mqtt::types::{MqttCredentials, MqttQos};
use rua_channel::mqtt::{MqttChannel, MqttConfig};

#[test]
fn config_defaults() {
    let cfg = MqttConfig::default();
    assert_eq!(cfg.broker_url, "localhost");
    assert_eq!(cfg.port, 1883);
    assert_eq!(cfg.subscribe_topic, "rua/inbound");
    assert_eq!(cfg.publish_topic, "rua/outbound");
    assert_eq!(cfg.qos, MqttQos::AtLeastOnce);
    assert!(cfg.credentials.is_none());
    assert!(cfg.client_id.is_none());
    assert_eq!(cfg.keep_alive_secs, 30);
    assert_eq!(cfg.channel_capacity, 10);
}

#[test]
fn config_serde_roundtrip() {
    let cfg = MqttConfig {
        broker_url: "broker.example.com".into(),
        port: 8883,
        client_id: Some("test-client".into()),
        subscribe_topic: "in/topic".into(),
        publish_topic: "out/topic".into(),
        qos: MqttQos::ExactlyOnce,
        credentials: Some(MqttCredentials {
            username: "user".into(),
            password: "pass".into(),
        }),
        keep_alive_secs: 60,
        channel_capacity: 20,
        tls_enabled: false,
        tls_ca_cert: None,
        tls_client_cert: None,
        tls_client_key: None,
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: MqttConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.broker_url, "broker.example.com");
    assert_eq!(parsed.port, 8883);
    assert_eq!(parsed.client_id.as_deref(), Some("test-client"));
    assert_eq!(parsed.qos, MqttQos::ExactlyOnce);
    assert!(parsed.credentials.is_some());
}

#[test]
fn config_from_json_with_defaults() {
    let json = r#"{"broker_url": "mqtt.local"}"#;
    let cfg: MqttConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.broker_url, "mqtt.local");
    assert_eq!(cfg.port, 1883);
    assert_eq!(cfg.subscribe_topic, "rua/inbound");
}

#[test]
fn qos_conversion() {
    assert_eq!(
        rumqttc::QoS::from(MqttQos::AtMostOnce),
        rumqttc::QoS::AtMostOnce
    );
    assert_eq!(
        rumqttc::QoS::from(MqttQos::AtLeastOnce),
        rumqttc::QoS::AtLeastOnce
    );
    assert_eq!(
        rumqttc::QoS::from(MqttQos::ExactlyOnce),
        rumqttc::QoS::ExactlyOnce
    );
}

#[test]
fn channel_id() {
    let ch = MqttChannel::new(MqttConfig::default());
    assert_eq!(ch.id(), "mqtt");
}

#[test]
fn build_request_from_native_parses_payload() {
    let ch = MqttChannel::new(MqttConfig::default());
    let native = serde_json::json!({
        "payload": "hello from mqtt",
        "topic": "test/topic",
        "agent_id": "agent-1"
    });
    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.channel_id, "mqtt");
    assert_eq!(req.conversation_id, "test/topic");
    assert_eq!(req.agent_id, "agent-1");
    assert_eq!(req.input.len(), 1);
}

#[test]
fn build_request_from_native_rejects_empty_payload() {
    let ch = MqttChannel::new(MqttConfig::default());
    for native in [
        serde_json::json!({}),
        serde_json::json!({"payload": "   ", "topic": "test/topic"}),
        serde_json::json!({"payload": "{\"text\":\"   \"}", "topic": "test/topic"}),
    ] {
        let err = ch
            .build_request_from_native(native)
            .expect_err("empty mqtt payload should not dispatch agent work");
        assert!(
            err.to_string().contains("mqtt payload"),
            "unexpected error: {err}"
        );
    }
}

#[test]
fn build_request_from_native_json_payload_uses_device_identity_metadata() {
    let ch = MqttChannel::new(MqttConfig::default());
    let native = serde_json::json!({
        "payload": serde_json::json!({
            "text": "Hello MQTT",
            "redirect_client_id": "device-001"
        }).to_string(),
        "topic": "rua/in/device-ignored",
        "metadata": {
            "source": "unit-test"
        }
    });

    let req = ch.build_request_from_native(native).unwrap();

    assert_eq!(req.channel_id, "mqtt");
    assert_eq!(req.user_id, "device-001");
    assert_eq!(req.session_id, "rua/in/device-ignored");
    assert_eq!(req.conversation_id, "rua/in/device-ignored");
    assert_eq!(req.metadata["client_id"], "device-001");
    assert_eq!(req.metadata["source"], "unit-test");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "Hello MQTT"),
        other => panic!("mqtt json payload normalized to non-text input: {other:?}"),
    }
}

#[test]
fn build_request_from_native_rejects_duplicate_explicit_message_id() {
    let ch = MqttChannel::new(MqttConfig::default());
    let native = serde_json::json!({
        "payload": serde_json::json!({
            "id": "mqtt-msg-1",
            "text": "Hello MQTT"
        }).to_string(),
        "topic": "rua/in/device-1"
    });

    ch.build_request_from_native(native.clone()).unwrap();

    let err = ch.build_request_from_native(native).unwrap_err();
    assert!(err.to_string().contains("duplicate mqtt event"));
}

#[test]
fn build_request_from_native_allows_repeated_payload_without_explicit_id() {
    let ch = MqttChannel::new(MqttConfig::default());
    let native = serde_json::json!({
        "payload": "same text",
        "topic": "rua/in/device-1"
    });

    ch.build_request_from_native(native.clone()).unwrap();
    ch.build_request_from_native(native).unwrap();
}

#[test]
fn render_content_matches_qwenpaw_mqtt_media_fallbacks() {
    let rendered = MqttChannel::render_content(&[
        OutgoingContentPart::Text {
            text: "summary".into(),
        },
        OutgoingContentPart::Image {
            url: Some("https://cdn.example.com/chart.png".into()),
            path: None,
            alt: Some("chart".into()),
        },
        OutgoingContentPart::Video {
            url: Some("https://cdn.example.com/clip.mp4".into()),
            path: None,
            name: Some("clip.mp4".into()),
        },
        OutgoingContentPart::Audio {
            url: Some("https://cdn.example.com/voice.mp3".into()),
            path: None,
            name: Some("voice.mp3".into()),
        },
        OutgoingContentPart::File {
            url: Some("https://cdn.example.com/report.pdf".into()),
            path: None,
            name: Some("report.pdf".into()),
        },
    ]);

    assert_eq!(
        rendered,
        "summary\n[Image] https://cdn.example.com/chart.png\n[Video] https://cdn.example.com/clip.mp4\n[Audio]\n[File] https://cdn.example.com/report.pdf"
    );
}

#[tokio::test]
async fn send_on_unstarted_channel_errors() {
    let ch = MqttChannel::new(MqttConfig::default());
    let target = rua_channel::core::ChannelTarget {
        agent_id: "a".into(),
        conversation_id: "c".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::Value::Null,
    };
    let msg = rua_channel::core::OutgoingMessage {
        channel_id: "mqtt".into(),
        conversation_id: "c".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "test".into(),
        }],
        reply_to: None,
        metadata: serde_json::Value::Null,
    };
    let result = ch.send(target, msg).await;
    assert!(result.is_err());
}

mod mock_broker {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    pub struct MockMqttBroker {
        pub addr: SocketAddr,
        shutdown: Arc<Notify>,
    }

    impl MockMqttBroker {
        pub async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let shutdown = Arc::new(Notify::new());
            let shutdown_rx = shutdown.clone();

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        accept = listener.accept() => {
                            if let Ok((stream, _)) = accept {
                                let shutdown_inner = shutdown_rx.clone();
                                tokio::spawn(handle_connection(stream, shutdown_inner));
                            }
                        }
                        _ = shutdown_rx.notified() => break,
                    }
                }
            });

            MockMqttBroker { addr, shutdown }
        }

        pub fn shutdown(&self) {
            self.shutdown.notify_one();
        }
    }

    async fn handle_connection(mut stream: tokio::net::TcpStream, shutdown: Arc<Notify>) {
        let mut buf = [0u8; 4096];
        loop {
            tokio::select! {
                n = stream.read(&mut buf) => {
                    let n = match n {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    let packet_type = buf[0] >> 4;
                    match packet_type {

                        1 => {
                            let connack = [0x20, 0x02, 0x00, 0x00];
                            let _ = stream.write_all(&connack).await;
                        }

                        8 => {

                            let remaining_start = 2;
                            let pkt_id_hi = buf[remaining_start];
                            let pkt_id_lo = buf[remaining_start + 1];
                            let suback = [0x90, 0x03, pkt_id_hi, pkt_id_lo, 0x01];
                            let _ = stream.write_all(&suback).await;
                        }

                        3 => {
                            let _ = stream.write_all(&buf[..n]).await;
                        }

                        12 => {
                            let pingresp = [0xd0, 0x00];
                            let _ = stream.write_all(&pingresp).await;
                        }

                        14 => return,
                        _ => {}
                    }
                }
                _ = shutdown.notified() => return,
            }
        }
    }
}

#[tokio::test]
async fn publish_via_mock_broker() {
    let broker = mock_broker::MockMqttBroker::start().await;

    let config = MqttConfig {
        broker_url: "127.0.0.1".into(),
        port: broker.addr.port(),
        subscribe_topic: "test/sub".into(),
        publish_topic: "test/pub".into(),
        ..Default::default()
    };

    let ch = MqttChannel::new(config);

    struct NoopProcess;
    #[async_trait::async_trait]
    impl rua_channel::core::channel::Process for NoopProcess {
        async fn handle(
            &self,
            _req: rua_channel::core::channel::AgentRequest,
        ) -> futures::stream::BoxStream<'static, rua_core::RuntimeEvent> {
            Box::pin(futures::stream::empty())
        }
    }

    ch.start(Arc::new(NoopProcess)).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    ch.publish("test/pub", b"hello mqtt").await.unwrap();

    ch.stop().await.unwrap();
    broker.shutdown();
}

use std::sync::Arc;

#[test]
fn tls_fields_default_to_disabled() {
    let cfg = MqttConfig::default();
    assert!(!cfg.tls_enabled);
    assert!(cfg.tls_ca_cert.is_none());
    assert!(cfg.tls_client_cert.is_none());
    assert!(cfg.tls_client_key.is_none());
}

#[test]
fn tls_config_serde_roundtrip() {
    let cfg = MqttConfig {
        tls_enabled: true,
        tls_ca_cert: Some("/path/to/ca.pem".into()),
        tls_client_cert: Some("/path/to/client.pem".into()),
        tls_client_key: Some("/path/to/client.key".into()),
        ..Default::default()
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let parsed: MqttConfig = serde_json::from_str(&json).unwrap();
    assert!(parsed.tls_enabled);
    assert_eq!(parsed.tls_ca_cert.as_deref(), Some("/path/to/ca.pem"));
    assert_eq!(
        parsed.tls_client_cert.as_deref(),
        Some("/path/to/client.pem")
    );
    assert_eq!(
        parsed.tls_client_key.as_deref(),
        Some("/path/to/client.key")
    );
}

#[test]
fn tls_fields_absent_in_json_default_to_disabled() {
    let json = r#"{"broker_url": "mqtt.local"}"#;
    let cfg: MqttConfig = serde_json::from_str(json).unwrap();
    assert!(!cfg.tls_enabled);
    assert!(cfg.tls_ca_cert.is_none());
}

#[test]
fn build_mqtt_options_tls_enabled_sets_tls_transport() {
    let cfg = MqttConfig {
        tls_enabled: true,
        port: 8883,
        ..Default::default()
    };
    let ch = MqttChannel::new(cfg);
    let opts = ch.build_mqtt_options().unwrap();
    assert!(matches!(opts.transport(), rumqttc::Transport::Tls(_)));
}

#[test]
fn build_mqtt_options_tls_disabled_uses_tcp() {
    let cfg = MqttConfig::default();
    let ch = MqttChannel::new(cfg);
    let opts = ch.build_mqtt_options().unwrap();
    assert!(matches!(opts.transport(), rumqttc::Transport::Tcp));
}

#[test]
fn build_mqtt_options_tls_auto_adjusts_port_to_8883() {
    let cfg = MqttConfig {
        tls_enabled: true,
        port: 1883,
        ..Default::default()
    };
    let ch = MqttChannel::new(cfg);
    let opts = ch.build_mqtt_options().unwrap();
    assert!(matches!(opts.transport(), rumqttc::Transport::Tls(_)));
}

#[test]
fn build_mqtt_options_tls_preserves_custom_port() {
    let cfg = MqttConfig {
        tls_enabled: true,
        port: 9883,
        ..Default::default()
    };
    let ch = MqttChannel::new(cfg);
    let opts = ch.build_mqtt_options().unwrap();
    assert!(matches!(opts.transport(), rumqttc::Transport::Tls(_)));
}
