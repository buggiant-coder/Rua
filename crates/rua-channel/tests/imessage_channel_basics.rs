#![cfg(target_os = "macos")]

use rua_channel::imessage::{IMessageChannel, IMessageConfig};

#[test]
fn channel_id_is_imessage() {
    use rua_channel::core::channel::Channel;
    let ch = IMessageChannel::new(IMessageConfig::default());
    assert_eq!(ch.id(), "imessage");
}

#[test]
fn config_defaults() {
    let cfg = IMessageConfig::default();
    assert!(cfg.enabled);
    assert_eq!(cfg.db_path, "~/Library/Messages/chat.db");
    assert!((cfg.poll_sec - 1.0).abs() < f64::EPSILON);
    assert!(cfg.bot_prefix.is_empty());
    assert!(cfg.allowed_senders.is_empty());
    assert_eq!(cfg.max_decoded_size, 10 * 1024 * 1024);
}

#[test]
fn config_deserialize_minimal() {
    let json = r#"{"enabled": false, "poll_sec": 2.5}"#;
    let cfg: IMessageConfig = serde_json::from_str(json).unwrap();
    assert!(!cfg.enabled);
    assert!((cfg.poll_sec - 2.5).abs() < f64::EPSILON);
    assert_eq!(cfg.db_path, "~/Library/Messages/chat.db");
}

#[test]
fn config_deserialize_full() {
    let json = r#"{
        "enabled": true,
        "db_path": "/tmp/test.db",
        "poll_sec": 0.5,
        "bot_prefix": "[bot]",
        "allowed_senders": ["+1234567890", "user@icloud.com"],
        "media_dir": "/tmp/media",
        "max_decoded_size": 1048576
    }"#;
    let cfg: IMessageConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.db_path, "/tmp/test.db");
    assert!((cfg.poll_sec - 0.5).abs() < f64::EPSILON);
    assert_eq!(cfg.bot_prefix, "[bot]");
    assert_eq!(cfg.allowed_senders.len(), 2);
    assert_eq!(cfg.media_dir, "/tmp/media");
    assert_eq!(cfg.max_decoded_size, 1_048_576);
}

#[cfg(target_os = "macos")]
#[test]
fn build_request_from_native_valid() {
    use rua_channel::core::channel::Channel;

    let ch = IMessageChannel::new(IMessageConfig::default());
    let native = serde_json::json!({
        "sender_id": "+1234567890",
        "text": "hello",
        "rowid": 42
    });
    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.channel_id, "imessage");
    assert_eq!(req.user_id, "+1234567890");
    assert_eq!(req.message_id, Some("42".to_string()));
}

#[cfg(target_os = "macos")]
#[test]
fn build_request_blocked_sender() {
    use rua_channel::core::channel::Channel;

    let cfg = IMessageConfig {
        allowed_senders: vec!["allowed@icloud.com".into()],
        ..Default::default()
    };
    let ch = IMessageChannel::new(cfg);
    let native = serde_json::json!({
        "sender_id": "blocked@icloud.com",
        "text": "hello"
    });
    let result = ch.build_request_from_native(native);
    assert!(result.is_err());
}

#[test]
fn bot_prefix_strips_prefix() {
    let cfg = IMessageConfig {
        bot_prefix: "[bot]".into(),
        ..Default::default()
    };
    assert_eq!(
        cfg.filter_by_prefix("[bot] hello world"),
        Some("hello world".into())
    );
}

#[test]
fn bot_prefix_skips_without_prefix() {
    let cfg = IMessageConfig {
        bot_prefix: "[bot]".into(),
        ..Default::default()
    };
    assert_eq!(cfg.filter_by_prefix("hello world"), None);
}

#[test]
fn bot_prefix_passthrough_when_empty() {
    let cfg = IMessageConfig::default();
    assert_eq!(
        cfg.filter_by_prefix("hello world"),
        Some("hello world".into())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn outgoing_media_helpers_match_qwenpaw_text_first_local_file_behavior() {
    use rua_channel::core::content::OutgoingContentPart;
    use std::path::PathBuf;

    let image_path = PathBuf::from("/tmp/chart.png");
    let file_path = PathBuf::from("/tmp/report.pdf");
    let audio_path = PathBuf::from("/tmp/voice.m4a");
    let video_path = PathBuf::from("/tmp/clip.mov");
    let parts = vec![
        OutgoingContentPart::Text {
            text: "summary".into(),
        },
        OutgoingContentPart::Image {
            url: None,
            path: Some(image_path.clone()),
            alt: Some("chart".into()),
        },
        OutgoingContentPart::File {
            url: None,
            path: Some(file_path.clone()),
            name: Some("report.pdf".into()),
        },
        OutgoingContentPart::Audio {
            url: None,
            path: Some(audio_path.clone()),
            name: Some("voice.m4a".into()),
        },
        OutgoingContentPart::Video {
            url: None,
            path: Some(video_path.clone()),
            name: Some("clip.mov".into()),
        },
    ];

    assert_eq!(
        IMessageChannel::render_text_and_unresolved_media(&parts),
        "summary",
        "local media should be sent as native imsg --file messages, not duplicated as text placeholders"
    );
    let paths: Vec<_> = parts
        .iter()
        .filter_map(IMessageChannel::local_media_path)
        .map(|path| path.to_path_buf())
        .collect();
    assert_eq!(paths, vec![image_path, file_path, audio_path, video_path]);
}

#[cfg(target_os = "macos")]
#[test]
fn outgoing_unresolved_media_keeps_visible_fallback_text() {
    use rua_channel::core::content::OutgoingContentPart;

    let parts = vec![
        OutgoingContentPart::Image {
            url: Some("https://example.invalid/chart.png".into()),
            path: None,
            alt: Some("chart".into()),
        },
        OutgoingContentPart::Audio {
            url: None,
            path: None,
            name: Some("voice.m4a".into()),
        },
    ];

    assert_eq!(
        IMessageChannel::render_text_and_unresolved_media(&parts),
        "[Audio: voice.m4a]"
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn outgoing_remote_url_media_downloads_to_local_file_before_native_send() {
    use axum::body::Body;
    use axum::http::{header, StatusCode};
    use axum::response::Response;
    use axum::routing::get;
    use rua_channel::core::content::OutgoingContentPart;

    let app = axum::Router::new().route(
        "/clip.mp4",
        get(|| async {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "video/mp4")
                .body(Body::from("video-bytes"))
                .unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let channel = IMessageChannel::new(IMessageConfig {
        media_dir: temp.path().to_string_lossy().to_string(),
        ..Default::default()
    });
    let part = OutgoingContentPart::Video {
        url: Some(format!("http://127.0.0.1:{}/clip.mp4", addr.port())),
        path: None,
        name: Some("clip.mp4".into()),
    };

    let resolved = channel
        .resolve_media_path(&part)
        .await
        .expect("remote media resolved")
        .expect("resolved local path");

    assert!(resolved.starts_with(temp.path()));
    assert_eq!(std::fs::read_to_string(&resolved).unwrap(), "video-bytes");

    server.abort();
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn outgoing_data_url_media_decodes_to_local_file_before_native_send() {
    use base64::Engine;
    use rua_channel::core::content::OutgoingContentPart;

    let temp = tempfile::tempdir().unwrap();
    let channel = IMessageChannel::new(IMessageConfig {
        media_dir: temp.path().to_string_lossy().to_string(),
        max_decoded_size: 1024,
        ..Default::default()
    });
    let data = base64::engine::general_purpose::STANDARD.encode(b"png-bytes");
    let part = OutgoingContentPart::Image {
        url: Some(format!("data:image/png;base64,{data}")),
        path: None,
        alt: Some("chart".into()),
    };

    let resolved = channel
        .resolve_media_path(&part)
        .await
        .expect("data URL resolved")
        .expect("resolved local path");

    assert!(resolved.starts_with(temp.path()));
    assert_eq!(
        resolved.extension().and_then(|value| value.to_str()),
        Some("png")
    );
    assert_eq!(std::fs::read(&resolved).unwrap(), b"png-bytes");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn stub_start_returns_not_implemented() {
    use rua_channel::core::channel::Channel;

    let rt = tokio::runtime::Runtime::new().unwrap();
    let ch = IMessageChannel::new(IMessageConfig::default());

    let result = rt.block_on(async {
        use async_trait::async_trait;
        use futures::stream::BoxStream;
        use rua_channel::core::channel::{AgentRequest, Process};
        use rua_core::RuntimeEvent;
        use std::sync::Arc;

        struct DummyProcess;
        #[async_trait]
        impl Process for DummyProcess {
            async fn handle(&self, _req: AgentRequest) -> BoxStream<'static, RuntimeEvent> {
                Box::pin(futures::stream::empty())
            }
        }

        ch.start(Arc::new(DummyProcess)).await
    });
    assert!(result.is_err());
}
