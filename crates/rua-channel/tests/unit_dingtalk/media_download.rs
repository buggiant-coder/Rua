use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rua_channel::core::channel::Channel;
use rua_channel::core::content::IncomingContentPart;
use rua_channel::dingtalk::DingTalkChannel;
use tempfile::tempdir;

fn sample_media_payload(msg_type: &str, download_code: &str) -> serde_json::Value {
    serde_json::json!({
        "msgType": msg_type,
        "msgId": "msg-001",
        "conversationId": "conv-123",
        "conversationType": "1",
        "senderId": "user-456",
        "senderNick": "Alice",
        "robotCode": "robot-001",
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xyz",
        "text": { "content": "//n" },
        "content": {
            "downloadCode": download_code
        }
    })
}

async fn start_dingtalk_media_fixture() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind dingtalk media fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let app = Router::new()
        .route(
            "/v1.0/robot/messageFiles/download",
            post(
                |State(base_url): State<String>, Json(body): Json<serde_json::Value>| async move {
                    let download_code = body["downloadCode"].as_str().expect("download code");
                    assert_eq!(body["robotCode"], "robot-001");
                    Json(serde_json::json!({
                        "downloadUrl": format!("{base_url}/media/{download_code}")
                    }))
                },
            ),
        )
        .route(
            "/media/{download_code}",
            get(
                |axum::extract::Path(download_code): axum::extract::Path<String>| async move {
                    let (mime, body) = match download_code.as_str() {
                        "img_dl_code" => ("image/png", "png-bytes"),
                        "audio_dl_code" => ("audio/amr", "audio-bytes"),
                        "video_dl_code" => ("video/mp4", "video-bytes"),
                        "file_dl_code" => ("application/pdf", "pdf-bytes"),
                        other => panic!("unexpected download code: {other}"),
                    };
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", mime)
                        .body(Body::from(body))
                        .unwrap()
                },
            ),
        )
        .with_state(base_url.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });
    (base_url, server)
}

#[tokio::test]
async fn download_inbound_picture_injects_local_path_for_typed_normalization() {
    let temp = tempdir().unwrap();
    let (base_url, server) = start_dingtalk_media_fixture().await;
    let channel = DingTalkChannel::new()
        .with_robot_code("robot-001".into())
        .with_media_dir(temp.path().to_string_lossy().to_string())
        .with_open_api_base_url(base_url);
    let mut native = sample_media_payload("picture", "img_dl_code");

    channel
        .download_inbound_media_with_token(&mut native, "access-token")
        .await
        .expect("dingtalk media download");

    let local_path = native["content"]["local_path"]
        .as_str()
        .expect("local path")
        .to_string();
    assert!(local_path.starts_with(temp.path().to_string_lossy().as_ref()));
    assert!(local_path.contains("img_dl_code"));
    assert_eq!(native["content"]["mime"], "image/png");
    assert_eq!(std::fs::read_to_string(&local_path).unwrap(), "png-bytes");

    let req = channel
        .build_request_from_native(native)
        .expect("typed dingtalk media request");
    match &req.input[0] {
        IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert_eq!(file_id, "img_dl_code");
            assert_eq!(path.to_string_lossy(), local_path);
            assert_eq!(mime.as_deref(), Some("image/png"));
        }
        other => panic!("expected image part, got {other:?}"),
    }

    server.abort();
}

#[tokio::test]
async fn download_inbound_richtext_audio_video_and_file_injects_local_paths() {
    let temp = tempdir().unwrap();
    let (base_url, server) = start_dingtalk_media_fixture().await;
    let channel = DingTalkChannel::new()
        .with_robot_code("robot-001".into())
        .with_media_dir(temp.path().to_string_lossy().to_string())
        .with_open_api_base_url(base_url);
    let mut native = serde_json::json!({
        "msgType": "richText",
        "msgId": "msg-001",
        "conversationId": "conv-123",
        "conversationType": "1",
        "senderId": "user-456",
        "senderNick": "Alice",
        "robotCode": "robot-001",
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xyz",
        "content": {
            "richText": [
                {
                    "type": "voice",
                    "downloadCode": "audio_dl_code",
                    "fileName": "audio.amr"
                },
                {
                    "type": "video",
                    "downloadCode": "video_dl_code",
                    "fileName": "video.mp4"
                },
                {
                    "type": "file",
                    "downloadCode": "file_dl_code",
                    "fileName": "report.pdf"
                }
            ]
        }
    });

    channel
        .download_inbound_media_with_token(&mut native, "access-token")
        .await
        .expect("dingtalk richText media download");

    for (idx, mime, bytes) in [
        (0, "audio/amr", "audio-bytes"),
        (1, "video/mp4", "video-bytes"),
        (2, "application/pdf", "pdf-bytes"),
    ] {
        let local_path = native["content"]["richText"][idx]["local_path"]
            .as_str()
            .expect("local path");
        assert!(local_path.starts_with(temp.path().to_string_lossy().as_ref()));
        assert_eq!(native["content"]["richText"][idx]["mime"], mime);
        assert_eq!(std::fs::read_to_string(local_path).unwrap(), bytes);
    }

    let req = channel
        .build_request_from_native(native)
        .expect("typed dingtalk richText media request");
    assert!(matches!(
        &req.input[0],
        IncomingContentPart::Audio { mime, name, .. }
            if mime.as_deref() == Some("audio/amr") && name.as_deref() == Some("audio.amr")
    ));
    assert!(matches!(
        &req.input[1],
        IncomingContentPart::Video { mime, name, .. }
            if mime.as_deref() == Some("video/mp4") && name.as_deref() == Some("video.mp4")
    ));
    assert!(matches!(
        &req.input[2],
        IncomingContentPart::File { mime, name, .. }
            if mime.as_deref() == Some("application/pdf") && name.as_deref() == Some("report.pdf")
    ));

    server.abort();
}

#[tokio::test]
async fn download_inbound_media_failure_keeps_placeholder_payload() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind failure fixture");
    let addr = listener.local_addr().expect("fixture addr");
    let app = Router::new().route(
        "/v1.0/robot/messageFiles/download",
        post(|| async { StatusCode::BAD_GATEWAY.into_response() }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve fixture");
    });
    let temp = tempdir().unwrap();
    let channel = DingTalkChannel::new()
        .with_robot_code("robot-001".into())
        .with_media_dir(temp.path().to_string_lossy().to_string())
        .with_open_api_base_url(format!("http://127.0.0.1:{}", addr.port()));
    let mut native = sample_media_payload("picture", "img_dl_code");

    channel
        .download_inbound_media_with_token(&mut native, "access-token")
        .await
        .expect("download failure should not abort fallback parsing");

    let req = channel
        .build_request_from_native(native)
        .expect("fallback dingtalk media request");
    match &req.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "[image: img_dl_code]"),
        other => panic!("expected fallback text, got {other:?}"),
    }

    server.abort();
}

#[test]
fn parser_preserves_local_audio_video_and_file_as_typed_parts() {
    let native = serde_json::json!({
        "msgType": "richText",
        "msgId": "msg-001",
        "conversationId": "conv-123",
        "conversationType": "1",
        "senderId": "user-456",
        "senderNick": "Alice",
        "robotCode": "robot-001",
        "sessionWebhook": "https://oapi.dingtalk.com/robot/sendBySession?session=xyz",
        "content": {
            "richText": [
                {
                    "type": "voice",
                    "downloadCode": "audio_dl_code",
                    "local_path": "/tmp/audio.amr",
                    "mime": "audio/amr",
                    "fileName": "audio.amr"
                },
                {
                    "type": "video",
                    "downloadCode": "video_dl_code",
                    "local_path": "/tmp/video.mp4",
                    "mime": "video/mp4",
                    "fileName": "video.mp4"
                },
                {
                    "type": "file",
                    "downloadCode": "file_dl_code",
                    "local_path": "/tmp/report.pdf",
                    "mime": "application/pdf",
                    "fileName": "report.pdf"
                }
            ]
        }
    });

    let req = DingTalkChannel::new()
        .build_request_from_native(native)
        .expect("local media dingtalk request");

    match &req.input[0] {
        IncomingContentPart::Audio {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "audio_dl_code");
            assert_eq!(path.to_string_lossy(), "/tmp/audio.amr");
            assert_eq!(mime.as_deref(), Some("audio/amr"));
            assert_eq!(name.as_deref(), Some("audio.amr"));
        }
        other => panic!("expected audio part, got {other:?}"),
    }
    match &req.input[1] {
        IncomingContentPart::Video {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "video_dl_code");
            assert_eq!(path.to_string_lossy(), "/tmp/video.mp4");
            assert_eq!(mime.as_deref(), Some("video/mp4"));
            assert_eq!(name.as_deref(), Some("video.mp4"));
        }
        other => panic!("expected video part, got {other:?}"),
    }
    match &req.input[2] {
        IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "file_dl_code");
            assert_eq!(path.to_string_lossy(), "/tmp/report.pdf");
            assert_eq!(mime.as_deref(), Some("application/pdf"));
            assert_eq!(name.as_deref(), Some("report.pdf"));
        }
        other => panic!("expected file part, got {other:?}"),
    }
}
