use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::get;
use rua_channel::core::channel::Channel;
use rua_channel::core::content::IncomingContentPart;
use rua_channel::onebot::{OneBotChannel, OneBotConfig};
use tempfile::tempdir;

async fn start_onebot_media_fixture() -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new()
        .route(
            "/media/photo.jpg",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "image/jpeg")
                    .body(Body::from("jpg-bytes"))
                    .unwrap()
            }),
        )
        .route(
            "/media/voice.amr",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "audio/amr")
                    .body(Body::from("amr-bytes"))
                    .unwrap()
            }),
        )
        .route(
            "/media/clip.mp4",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "video/mp4")
                    .body(Body::from("mp4-bytes"))
                    .unwrap()
            }),
        )
        .route(
            "/media/report.pdf",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/pdf")
                    .body(Body::from("pdf-bytes"))
                    .unwrap()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{}", addr.port()), server)
}

#[test]
fn parse_image_segment() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 100,
        "message_id": 1,
        "raw_message": "",
        "message": [
            {"type": "text", "data": {"text": "look at this"}},
            {"type": "image", "data": {"url": "https://example.com/img.png", "file": "abc.image"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.input.len(), 2);

    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "look at this"),
        other => panic!("expected Text, got {other:?}"),
    }
    match &req.input[1] {
        IncomingContentPart::Url { url, .. } => {
            assert_eq!(url, "https://example.com/img.png");
        }
        other => panic!("expected Url for image segment, got {other:?}"),
    }
}

#[test]
fn parse_multiple_media_segments() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 200,
        "message_id": 2,
        "raw_message": "",
        "message": [
            {"type": "text", "data": {"text": "files"}},
            {"type": "image", "data": {"url": "https://example.com/a.jpg"}},
            {"type": "record", "data": {"file": "voice.amr"}},
            {"type": "video", "data": {"url": "https://example.com/v.mp4"}},
            {"type": "file", "data": {"url": "https://example.com/doc.pdf", "name": "report.pdf"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.input.len(), 5);
    assert!(matches!(&req.input[0], IncomingContentPart::Text { .. }));
    assert!(matches!(&req.input[1], IncomingContentPart::Url { .. }));
    assert!(matches!(&req.input[2], IncomingContentPart::Url { .. }));
    assert!(matches!(&req.input[3], IncomingContentPart::Url { .. }));
    assert!(matches!(&req.input[4], IncomingContentPart::Url { .. }));

    if let IncomingContentPart::Url { url, title } = &req.input[4] {
        assert_eq!(url, "https://example.com/doc.pdf");
        assert_eq!(title.as_deref(), Some("report.pdf"));
    }
}

#[test]
fn parse_text_only_message_unchanged() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 300,
        "message_id": 3,
        "raw_message": "simple text",
        "message": [
            {"type": "text", "data": {"text": "simple text"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.input.len(), 1);
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "simple text"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn parse_image_prefers_url_over_file() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 400,
        "message_id": 4,
        "raw_message": "",
        "message": [
            {"type": "image", "data": {"url": "https://real.url/img.png", "file": "local_name.png"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.input.len(), 1);
    if let IncomingContentPart::Url { url, .. } = &req.input[0] {
        assert_eq!(url, "https://real.url/img.png");
    } else {
        panic!("expected Url part");
    }
}

#[test]
fn parse_image_falls_back_to_file_field() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 500,
        "message_id": 5,
        "raw_message": "",
        "message": [
            {"type": "image", "data": {"file": "abc123.image"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.input.len(), 1);
    if let IncomingContentPart::Url { url, .. } = &req.input[0] {
        assert_eq!(url, "abc123.image");
    } else {
        panic!("expected Url part");
    }
}

#[test]
fn parse_non_image_media_segments_preserve_url_or_file_fallback_and_titles() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 550,
        "message_id": 55,
        "raw_message": "",
        "message": [
            {"type": "record", "data": {"file": "voice.amr"}},
            {"type": "video", "data": {"url": "https://example.com/video.mp4", "file": "video-file-id"}},
            {"type": "file", "data": {"url": "https://example.com/report.pdf", "name": "report.pdf"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(
        req.input,
        vec![
            IncomingContentPart::Url {
                url: "voice.amr".into(),
                title: Some("voice.amr".into()),
            },
            IncomingContentPart::Url {
                url: "https://example.com/video.mp4".into(),
                title: Some("video-file-id".into()),
            },
            IncomingContentPart::Url {
                url: "https://example.com/report.pdf".into(),
                title: Some("report.pdf".into()),
            },
        ],
        "QwenPaw's OneBot media parsing signal maps to Rust's native adapter: url wins for media source, file/name remain stable labels, and file-only media still reaches the agent"
    );
}

#[test]
fn unknown_segment_types_ignored() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 600,
        "message_id": 6,
        "raw_message": "",
        "message": [
            {"type": "text", "data": {"text": "hi"}},
            {"type": "face", "data": {"id": "5"}},
            {"type": "at", "data": {"qq": "all"}},
            {"type": "reply", "data": {"id": "123"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.input.len(), 1);
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "hi"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn media_only_no_text() {
    let channel = OneBotChannel::new(OneBotConfig::default());

    let native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 700,
        "message_id": 7,
        "raw_message": "",
        "message": [
            {"type": "image", "data": {"url": "https://example.com/photo.jpg"}}
        ]
    });

    let req = channel.build_request_from_native(native).unwrap();
    assert_eq!(req.input.len(), 1);
    assert!(matches!(&req.input[0], IncomingContentPart::Url { .. }));
}

#[tokio::test]
async fn download_inbound_media_injects_local_paths_for_typed_parts() {
    let temp = tempdir().unwrap();
    let (base_url, server) = start_onebot_media_fixture().await;
    let channel = OneBotChannel::new(OneBotConfig {
        media_dir: temp.path().to_string_lossy().to_string(),
        ..Default::default()
    });
    let mut native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 700,
        "message_id": 7,
        "raw_message": "",
        "message": [
            {"type": "image", "data": {"url": format!("{base_url}/media/photo.jpg"), "file": "photo.jpg"}},
            {"type": "record", "data": {"url": format!("{base_url}/media/voice.amr"), "file": "voice.amr"}},
            {"type": "video", "data": {"url": format!("{base_url}/media/clip.mp4"), "file": "clip.mp4"}},
            {"type": "file", "data": {"url": format!("{base_url}/media/report.pdf"), "file": "report.pdf", "name": "report.pdf"}}
        ]
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("onebot media download");

    let req = channel
        .build_request_from_native(native)
        .expect("typed onebot media request");
    assert_eq!(req.input.len(), 4);
    match &req.input[0] {
        IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert_eq!(file_id, "photo.jpg");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("image/jpeg"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "jpg-bytes");
        }
        other => panic!("expected image part, got {other:?}"),
    }
    match &req.input[1] {
        IncomingContentPart::Audio {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "voice.amr");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("audio/amr"));
            assert_eq!(name.as_deref(), Some("voice.amr"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "amr-bytes");
        }
        other => panic!("expected audio part, got {other:?}"),
    }
    match &req.input[2] {
        IncomingContentPart::Video {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "clip.mp4");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("video/mp4"));
            assert_eq!(name.as_deref(), Some("clip.mp4"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "mp4-bytes");
        }
        other => panic!("expected video part, got {other:?}"),
    }
    match &req.input[3] {
        IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "report.pdf");
            assert!(path.starts_with(temp.path()));
            assert_eq!(mime.as_deref(), Some("application/pdf"));
            assert_eq!(name.as_deref(), Some("report.pdf"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), "pdf-bytes");
        }
        other => panic!("expected file part, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn download_inbound_media_failure_keeps_url_fallback() {
    let app = axum::Router::new().route(
        "/media/missing.jpg",
        get(|| async { StatusCode::BAD_GATEWAY }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let temp = tempdir().unwrap();
    let channel = OneBotChannel::new(OneBotConfig {
        media_dir: temp.path().to_string_lossy().to_string(),
        ..Default::default()
    });
    let mut native = serde_json::json!({
        "post_type": "message",
        "message_type": "private",
        "user_id": 700,
        "message_id": 7,
        "raw_message": "",
        "message": [
            {"type": "image", "data": {"url": format!("{base_url}/media/missing.jpg"), "file": "missing.jpg"}}
        ]
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("download failure should not abort fallback parsing");

    let req = channel
        .build_request_from_native(native)
        .expect("fallback onebot media request");
    match &req.input[0] {
        IncomingContentPart::Url { url, title } => {
            assert!(url.ends_with("/media/missing.jpg"));
            assert_eq!(title.as_deref(), Some("missing.jpg"));
        }
        other => panic!("expected URL fallback, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn media_download_limit_and_auth_scope_apply_to_native_downloads() {
    use std::sync::Arc;
    let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let captured = seen.clone();
    let app = axum::Router::new().route(
        "/{*path}",
        get(move |headers: axum::http::HeaderMap| {
            let captured = captured.clone();
            async move {
                captured.lock().await.push(
                    headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned),
                );
                let chunks = futures::stream::iter(vec![
                    Ok::<_, std::io::Error>(vec![0u8; 600_000]),
                    Ok(vec![0u8; 600_000]),
                ]);
                Response::builder()
                    .header("Content-Type", "image/png")
                    .body(Body::from_stream(chunks))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let temp = tempdir().unwrap();
    for (path, limit) in [("api/image", 1), ("outside/image", 2)] {
        let channel = OneBotChannel::new(OneBotConfig {
            http_url: format!("{base}/api"),
            access_token: Some("api-secret".into()),
            media_dir: temp.path().to_string_lossy().into_owned(),
            ..Default::default()
        })
        .with_acl_config(serde_json::json!({"media_download_max_mb":limit}));
        let mut native = serde_json::json!({"post_type":"message","message_type":"private","user_id":42,"message_id":limit,
            "message":[{"type":"image","data":{"url":format!("{base}/{path}")}}]});
        channel.download_inbound_media(&mut native).await.unwrap();
        let req = channel.build_request_from_native(native).unwrap();
        assert_eq!(
            req.input
                .iter()
                .any(|part| matches!(part, IncomingContentPart::Image { .. })),
            limit == 2
        );
    }
    assert_eq!(
        *seen.lock().await,
        vec![Some("Bearer api-secret".into()), None]
    );
    server.abort();
}
