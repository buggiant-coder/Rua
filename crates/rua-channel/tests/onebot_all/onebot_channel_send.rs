use std::sync::Arc;

use axum::routing::post;
use axum::Json;
use tokio::sync::Mutex;

use rua_channel::core::channel::Channel;
use rua_channel::core::content::OutgoingContentPart;
use rua_channel::core::ChannelTarget;
use rua_channel::onebot::{OneBotChannel, OneBotConfig};

async fn start_send_msg_recorder() -> (
    String,
    Arc<Mutex<Vec<serde_json::Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/send_msg",
        post(move |Json(body): Json<serde_json::Value>| {
            let store = received_clone.clone();
            async move {
                store.lock().await.push(body);
                axum::Json(serde_json::json!({"status": "ok", "retcode": 0}))
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (
        format!("http://127.0.0.1:{}", addr.port()),
        received,
        server,
    )
}

async fn start_onebot_api_recorder() -> (
    String,
    Arc<Mutex<Vec<(String, serde_json::Value)>>>,
    tokio::task::JoinHandle<()>,
) {
    let received = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
    let send_store = received.clone();
    let private_file_store = received.clone();
    let group_file_store = received.clone();

    let app = axum::Router::new()
        .route(
            "/send_msg",
            post(move |Json(body): Json<serde_json::Value>| {
                let store = send_store.clone();
                async move {
                    store.lock().await.push(("send_msg".into(), body));
                    axum::Json(serde_json::json!({"status": "ok", "retcode": 0}))
                }
            }),
        )
        .route(
            "/upload_private_file",
            post(move |Json(body): Json<serde_json::Value>| {
                let store = private_file_store.clone();
                async move {
                    store
                        .lock()
                        .await
                        .push(("upload_private_file".into(), body));
                    axum::Json(serde_json::json!({"status": "ok", "retcode": 0}))
                }
            }),
        )
        .route(
            "/upload_group_file",
            post(move |Json(body): Json<serde_json::Value>| {
                let store = group_file_store.clone();
                async move {
                    store.lock().await.push(("upload_group_file".into(), body));
                    axum::Json(serde_json::json!({"status": "ok", "retcode": 0}))
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (
        format!("http://127.0.0.1:{}", addr.port()),
        received,
        server,
    )
}

fn make_channel(http_url: String) -> OneBotChannel {
    OneBotChannel::new(OneBotConfig {
        http_url,
        ws_host: "127.0.0.1".into(),
        ws_port: 0,
        access_token: None,
        ..Default::default()
    })
}

#[tokio::test]
async fn send_calls_onebot_send_msg() {
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/send_msg",
        post(move |Json(body): Json<serde_json::Value>| {
            let store = received_clone.clone();
            async move {
                store.lock().await.push(body);
                axum::Json(serde_json::json!({"status": "ok", "retcode": 0}))
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = OneBotConfig {
        http_url: format!("http://127.0.0.1:{}", addr.port()),
        ws_host: "127.0.0.1".into(),
        ws_port: 0,
        access_token: None,
        ..Default::default()
    };
    let channel = OneBotChannel::new(config);

    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "private_12345".into(),
        sender_id: Some("12345".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "private",
            "user_id": 12345,
        }),
    };

    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "onebot".into(),
        conversation_id: "private_12345".into(),
        receiver_id: Some("12345".into()),
        content: vec![OutgoingContentPart::Text {
            text: "hello onebot".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "private",
            "user_id": 12345,
        }),
    };

    channel.send(to, msg).await.unwrap();

    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["message"][0]["data"]["text"], "hello onebot");
    assert_eq!(msgs[0]["user_id"], 12345);

    server.abort();
}

#[tokio::test]
async fn send_uses_target_metadata_when_message_metadata_empty() {
    let (http_url, received, server) = start_send_msg_recorder().await;
    let channel = make_channel(http_url);

    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "group_67890".into(),
        sender_id: Some("12345".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "group",
            "group_id": "67890",
        }),
    };

    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "onebot".into(),
        conversation_id: "group_67890".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "target metadata route".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(to, msg).await.unwrap();

    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["message_type"], "group");
    assert_eq!(msgs[0]["group_id"], 67890);
    assert_eq!(
        msgs[0]["message"][0]["data"]["text"],
        "target metadata route"
    );

    server.abort();
}

#[tokio::test]
async fn send_infers_group_target_from_conversation_id() {
    let (http_url, received, server) = start_send_msg_recorder().await;
    let channel = make_channel(http_url);

    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "group_67890".into(),
        sender_id: Some("12345".into()),
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "onebot".into(),
        conversation_id: "group_67890".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text { text: "hi".into() }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(to, msg).await.unwrap();

    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["message_type"], "group");
    assert_eq!(msgs[0]["group_id"], 67890);

    server.abort();
}

#[tokio::test]
async fn send_image_url_as_onebot_image_segment() {
    let (http_url, received, server) = start_send_msg_recorder().await;
    let channel = make_channel(http_url);

    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "private_12345".into(),
        sender_id: Some("12345".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "private",
            "user_id": "12345",
        }),
    };

    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "onebot".into(),
        conversation_id: "private_12345".into(),
        receiver_id: Some("12345".into()),
        content: vec![
            OutgoingContentPart::Text { text: "see".into() },
            OutgoingContentPart::Image {
                url: Some("https://img.example.com/pic.png".into()),
                path: None,
                alt: Some("pic".into()),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(to, msg).await.unwrap();

    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["message_type"], "private");
    assert_eq!(msgs[0]["user_id"], 12345);
    assert_eq!(msgs[0]["message"][0]["type"], "text");
    assert_eq!(msgs[0]["message"][0]["data"]["text"], "see");
    assert_eq!(msgs[0]["message"][1]["type"], "image");
    assert_eq!(
        msgs[0]["message"][1]["data"]["file"],
        "https://img.example.com/pic.png"
    );

    server.abort();
}

#[tokio::test]
async fn send_audio_and_video_as_onebot_media_segments() {
    let (http_url, received, server) = start_send_msg_recorder().await;
    let channel = make_channel(http_url);

    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "private_12345".into(),
        sender_id: Some("12345".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "private",
            "user_id": "12345",
        }),
    };

    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "onebot".into(),
        conversation_id: "private_12345".into(),
        receiver_id: Some("12345".into()),
        content: vec![
            OutgoingContentPart::Audio {
                url: Some("https://audio.example.com/a.mp3".into()),
                path: None,
                name: Some("a.mp3".into()),
            },
            OutgoingContentPart::Video {
                url: Some("https://video.example.com/v.mp4".into()),
                path: None,
                name: Some("v.mp4".into()),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(to, msg).await.unwrap();

    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["message"][0]["type"], "record");
    assert_eq!(
        msgs[0]["message"][0]["data"]["file"],
        "https://audio.example.com/a.mp3"
    );
    assert_eq!(msgs[0]["message"][1]["type"], "video");
    assert_eq!(
        msgs[0]["message"][1]["data"]["file"],
        "https://video.example.com/v.mp4"
    );

    server.abort();
}

#[tokio::test]
async fn send_file_uses_onebot_upload_private_file_api() {
    let (http_url, received, server) = start_onebot_api_recorder().await;
    let channel = make_channel(http_url);

    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "private_12345".into(),
        sender_id: Some("12345".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "private",
            "user_id": "12345",
        }),
    };

    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "onebot".into(),
        conversation_id: "private_12345".into(),
        receiver_id: Some("12345".into()),
        content: vec![OutgoingContentPart::File {
            url: Some("file:///tmp/report.pdf".into()),
            path: None,
            name: Some("report.pdf".into()),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(to, msg).await.unwrap();

    let calls = received.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "upload_private_file");
    assert_eq!(calls[0].1["user_id"], 12345);
    assert_eq!(calls[0].1["file"], "file:///tmp/report.pdf");
    assert_eq!(calls[0].1["name"], "report.pdf");

    server.abort();
}

#[tokio::test]
async fn send_file_uses_onebot_upload_group_file_api() {
    let (http_url, received, server) = start_onebot_api_recorder().await;
    let channel = make_channel(http_url);

    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "group_67890".into(),
        sender_id: Some("12345".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "group",
            "group_id": 67890,
        }),
    };

    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "onebot".into(),
        conversation_id: "group_67890".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::File {
            url: Some("file:///tmp/group-report.pdf".into()),
            path: None,
            name: Some("group-report.pdf".into()),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(to, msg).await.unwrap();

    let calls = received.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "upload_group_file");
    assert_eq!(calls[0].1["group_id"], 67890);
    assert_eq!(calls[0].1["file"], "file:///tmp/group-report.pdf");
    assert_eq!(calls[0].1["name"], "group-report.pdf");

    server.abort();
}

#[tokio::test]
async fn send_group_message() {
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/send_msg",
        post(move |Json(body): Json<serde_json::Value>| {
            let store = received_clone.clone();
            async move {
                store.lock().await.push(body);
                axum::Json(serde_json::json!({"status": "ok", "retcode": 0}))
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = OneBotConfig {
        http_url: format!("http://127.0.0.1:{}", addr.port()),
        ws_host: "127.0.0.1".into(),
        ws_port: 0,
        access_token: None,
        ..Default::default()
    };
    let channel = OneBotChannel::new(config);

    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "group_67890".into(),
        sender_id: Some("12345".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "group",
            "group_id": 67890,
        }),
    };

    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "onebot".into(),
        conversation_id: "group_67890".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "hello group".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "group",
            "group_id": 67890,
        }),
    };

    channel.send(to, msg).await.unwrap();

    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["message_type"], "group");
    assert_eq!(msgs[0]["group_id"], 67890);

    server.abort();
}

#[tokio::test]
async fn configured_media_encoding_applies_to_images_and_files_with_size_limit() {
    use base64::Engine;
    let (url, calls, server) = start_onebot_api_recorder().await;
    let temp = tempfile::tempdir().unwrap();
    let small = temp.path().join("photo with spaces.png");
    let large = temp.path().join("large.png");
    std::fs::write(&small, b"image-bytes").unwrap();
    std::fs::write(&large, vec![0u8; 1_000_001]).unwrap();
    for (enabled, path) in [(true, &small), (false, &small), (true, &large)] {
        let channel = make_channel(url.clone()).with_acl_config(serde_json::json!({
            "media_base64":enabled, "media_base64_max_mb":1
        }));
        let target = ChannelTarget {
            agent_id: String::new(),
            conversation_id: "private_42".into(),
            sender_id: Some("42".into()),
            reply_to: None,
            metadata: serde_json::json!({"message_type":"private","user_id":42}),
        };
        let message = rua_channel::core::OutgoingMessage {
            channel_id: "onebot".into(),
            conversation_id: "private_42".into(),
            receiver_id: Some("42".into()),
            content: vec![
                OutgoingContentPart::Image {
                    path: None,
                    url: Some(url::Url::from_file_path(path).unwrap().to_string()),
                    alt: None,
                },
                OutgoingContentPart::File {
                    path: Some(path.clone()),
                    url: None,
                    name: Some("photo.png".into()),
                },
            ],
            reply_to: None,
            metadata: serde_json::json!({}),
        };
        channel.send(target, message).await.unwrap();
        let batch = std::mem::take(&mut *calls.lock().await);
        assert_eq!(batch.len(), 2);
        let file = batch
            .iter()
            .find(|(method, _)| method == "upload_private_file")
            .unwrap()
            .1["file"]
            .as_str()
            .unwrap();
        let image = batch
            .iter()
            .find(|(method, _)| method == "send_msg")
            .unwrap()
            .1["message"][0]["data"]["file"]
            .as_str()
            .unwrap();
        if enabled && path == &small {
            let encoded = format!(
                "base64://{}",
                base64::engine::general_purpose::STANDARD.encode(b"image-bytes")
            );
            assert_eq!(file, encoded);
            assert_eq!(image, encoded);
        } else {
            assert!(!file.starts_with("base64://"));
            assert!(!image.starts_with("base64://"));
        }
    }
    server.abort();
}
