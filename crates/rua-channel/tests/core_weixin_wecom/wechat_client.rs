use std::collections::HashMap;
use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::extract::Query;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use rua_channel::core::channel::Channel;
use rua_channel::core::content::{
    AgentChannelRequest, ChannelTarget, IncomingContentPart, OutgoingContentPart, OutgoingMessage,
};
use rua_channel::core::ProcessingIndicator;
use rua_channel::wechat::ILinkClient;
use rua_channel::wechat::WechatChannel;
use rua_channel::wechat::WechatTypingIndicator;
use serde_json::Value;
use tempfile::tempdir;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const CHANNEL_VERSION: &str = "2.0.1";

async fn spawn_app(app: Router) -> (String, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("test server addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve test app");
    });

    (format!("http://{addr}"), handle)
}

fn indicator_request() -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "agent-1".into(),
        user_id: "user@im.wechat".into(),
        session_id: "wechat:user@im.wechat".into(),
        input: vec![IncomingContentPart::Text {
            text: "hello".into(),
        }],
        channel_id: "wechat".into(),
        conversation_id: "wechat:user@im.wechat".into(),
        message_id: None,
        metadata: serde_json::json!({}),
    }
}

#[test]
fn custom_base_url_is_trimmed() {
    let client = ILinkClient::new(
        "token123".into(),
        Some("https://custom.weixin.qq.com/".into()),
    );

    assert_eq!(client.base_url, "https://custom.weixin.qq.com");
}

#[test]
fn build_request_from_native_preserves_wechat_session_and_context_metadata() {
    let channel = WechatChannel::new("token123".into(), None);

    let request = channel
        .build_request_from_native(serde_json::json!({
            "from_user_id": "wx-user-1",
            "context_token": "ctx-123",
            "item_list": [
                {"type": 1, "text_item": {"text": "  first line  "}},
                {"type": 1, "text_item": {"text": ""}},
                {"type": 1, "text_item": {"text": "second line"}}
            ]
        }))
        .expect("native request");

    assert_eq!(request.user_id, "wx-user-1");
    assert_eq!(request.session_id, "wechat:wx-user-1");
    assert_eq!(request.channel_id, "wechat");
    assert_eq!(request.conversation_id, "wechat:wx-user-1");
    assert_eq!(request.message_id.as_deref(), Some("ctx-123"));
    assert_eq!(request.metadata["wechat_from_user_id"], "wx-user-1");
    assert_eq!(request.metadata["wechat_context_token"], "ctx-123");
    assert_eq!(
        request.input,
        vec![IncomingContentPart::Text {
            text: "first line\nsecond line".into(),
        }],
        "QwenPaw's WeChat request-building signal maps to Rust's native adapter: text items are trimmed, empty text is skipped, and provider context stays in metadata"
    );
}

#[test]
fn build_request_from_native_rejects_duplicate_context_token() {
    let channel = WechatChannel::new("token123".into(), None);
    let native = serde_json::json!({
        "from_user_id": "wx-user-1",
        "context_token": "ctx-duplicate",
        "item_list": [
            {"type": 1, "text_item": {"text": "hello"}}
        ]
    });

    channel.build_request_from_native(native.clone()).unwrap();

    let err = channel.build_request_from_native(native).unwrap_err();
    assert!(err.to_string().contains("duplicate wechat event"));
}

#[tokio::test]
async fn download_inbound_image_url_injects_local_path_for_typed_normalization() {
    let app = Router::new().route(
        "/media/image.jpg",
        get(|| async {
            axum::response::Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "image/jpeg")
                .body(axum::body::Body::from("jpeg-bytes"))
                .unwrap()
        }),
    );
    let (base_url, handle) = spawn_app(app).await;
    let temp = tempdir().unwrap();
    let channel = WechatChannel::new("token123".into(), Some(base_url.clone()))
        .with_media_dir(temp.path().to_string_lossy().to_string());
    let mut native = serde_json::json!({
        "from_user_id": "wx-user-1",
        "context_token": "ctx-123",
        "message_type": 1,
        "item_list": [
            {
                "type": 2,
                "image_item": {
                    "media": {
                        "url": format!("{base_url}/media/image.jpg")
                    }
                }
            }
        ]
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("wechat media download");

    let local_path = native["item_list"][0]["image_item"]["media"]["local_path"]
        .as_str()
        .expect("local path")
        .to_string();
    assert!(local_path.starts_with(temp.path().to_string_lossy().as_ref()));
    assert_eq!(
        native["item_list"][0]["image_item"]["media"]["mime"],
        "image/jpeg"
    );
    assert_eq!(std::fs::read_to_string(&local_path).unwrap(), "jpeg-bytes");

    let request = channel
        .build_request_from_native(native)
        .expect("typed media request");
    match &request.input[0] {
        IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert!(file_id.ends_with("/media/image.jpg"));
            assert_eq!(path.to_string_lossy(), local_path);
            assert_eq!(mime.as_deref(), Some("image/jpeg"));
        }
        other => panic!("expected image part, got {other:?}"),
    }

    handle.abort();
}

#[tokio::test]
async fn download_inbound_file_and_video_urls_inject_local_paths_for_typed_normalization() {
    let app = Router::new()
        .route(
            "/media/report.pdf",
            get(|| async {
                axum::response::Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/pdf")
                    .body(axum::body::Body::from("pdf-bytes"))
                    .unwrap()
            }),
        )
        .route(
            "/media/video.mp4",
            get(|| async {
                axum::response::Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "video/mp4")
                    .body(axum::body::Body::from("video-bytes"))
                    .unwrap()
            }),
        );
    let (base_url, handle) = spawn_app(app).await;
    let temp = tempdir().unwrap();
    let channel = WechatChannel::new("token123".into(), Some(base_url.clone()))
        .with_media_dir(temp.path().to_string_lossy().to_string());
    let mut native = serde_json::json!({
        "from_user_id": "wx-user-1",
        "context_token": "ctx-123",
        "message_type": 1,
        "item_list": [
            {
                "type": 4,
                "file_item": {
                    "file_name": "report.pdf",
                    "media": {
                        "url": format!("{base_url}/media/report.pdf")
                    }
                }
            },
            {
                "type": 5,
                "video_item": {
                    "media": {
                        "url": format!("{base_url}/media/video.mp4")
                    }
                }
            }
        ]
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("wechat media download");

    let file_path = native["item_list"][0]["file_item"]["media"]["local_path"]
        .as_str()
        .expect("file local path")
        .to_string();
    let video_path = native["item_list"][1]["video_item"]["media"]["local_path"]
        .as_str()
        .expect("video local path")
        .to_string();
    assert!(file_path.starts_with(temp.path().to_string_lossy().as_ref()));
    assert!(video_path.starts_with(temp.path().to_string_lossy().as_ref()));
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "pdf-bytes");
    assert_eq!(std::fs::read_to_string(&video_path).unwrap(), "video-bytes");

    let request = channel
        .build_request_from_native(native)
        .expect("typed media request");
    match &request.input[0] {
        IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        } => {
            assert!(file_id.ends_with("/media/report.pdf"));
            assert_eq!(path.to_string_lossy(), file_path);
            assert_eq!(mime.as_deref(), Some("application/pdf"));
            assert_eq!(name.as_deref(), Some("report.pdf"));
        }
        other => panic!("expected file part, got {other:?}"),
    }
    match &request.input[1] {
        IncomingContentPart::Video {
            file_id,
            path,
            mime,
            name,
        } => {
            assert!(file_id.ends_with("/media/video.mp4"));
            assert_eq!(path.to_string_lossy(), video_path);
            assert_eq!(mime.as_deref(), Some("video/mp4"));
            assert_eq!(name.as_deref(), Some("video.mp4"));
        }
        other => panic!("expected video part, got {other:?}"),
    }

    handle.abort();
}

#[tokio::test]
async fn download_inbound_image_decrypts_aes_ecb_media() {
    let encrypted = hex_bytes("249bb7a9409d089d3667fccbdc3d7dca");
    let app = Router::new().route(
        "/media/encrypted.jpg",
        get(move || {
            let encrypted = encrypted.clone();
            async move {
                axum::response::Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "image/jpeg")
                    .body(axum::body::Body::from(encrypted))
                    .unwrap()
            }
        }),
    );
    let (base_url, handle) = spawn_app(app).await;
    let temp = tempdir().unwrap();
    let channel = WechatChannel::new("token123".into(), Some(base_url.clone()))
        .with_media_dir(temp.path().to_string_lossy().to_string());
    let mut native = serde_json::json!({
        "from_user_id": "wx-user-1",
        "context_token": "ctx-123",
        "message_type": 1,
        "item_list": [
            {
                "type": 2,
                "image_item": {
                    "aeskey": "00112233445566778899aabbccddeeff",
                    "media": {
                        "url": format!("{base_url}/media/encrypted.jpg")
                    }
                }
            }
        ]
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("wechat encrypted media download");

    let local_path = native["item_list"][0]["image_item"]["media"]["local_path"]
        .as_str()
        .expect("local path");
    assert_eq!(std::fs::read_to_string(local_path).unwrap(), "jpeg-bytes");

    handle.abort();
}

#[tokio::test]
async fn download_inbound_media_failure_keeps_placeholder_payload() {
    let app = Router::new().route(
        "/media/image.jpg",
        get(|| async { StatusCode::BAD_GATEWAY }),
    );
    let (base_url, handle) = spawn_app(app).await;
    let temp = tempdir().unwrap();
    let channel = WechatChannel::new("token123".into(), Some(base_url.clone()))
        .with_media_dir(temp.path().to_string_lossy().to_string());
    let mut native = serde_json::json!({
        "from_user_id": "wx-user-1",
        "context_token": "ctx-123",
        "message_type": 1,
        "item_list": [
            {
                "type": 2,
                "image_item": {
                    "media": {
                        "url": format!("{base_url}/media/image.jpg")
                    }
                }
            }
        ]
    });

    channel
        .download_inbound_media(&mut native)
        .await
        .expect("download failure should preserve fallback parsing");

    let request = channel
        .build_request_from_native(native)
        .expect("fallback media request");
    match &request.input[0] {
        IncomingContentPart::Text { text } => assert_eq!(text, "[image: download failed]"),
        other => panic!("expected fallback text, got {other:?}"),
    }

    handle.abort();
}

fn hex_bytes(raw: &str) -> Vec<u8> {
    (0..raw.len())
        .step_by(2)
        .map(|idx| u8::from_str_radix(&raw[idx..idx + 2], 16).unwrap())
        .collect()
}

#[test]
fn local_media_paths_are_typed_parts_for_file_and_video() {
    let channel = WechatChannel::new("token123".into(), None);
    let request = channel
        .build_request_from_native(serde_json::json!({
            "from_user_id": "wx-user-1",
            "context_token": "ctx-123",
            "message_type": 1,
            "item_list": [
                {
                    "type": 4,
                    "file_item": {
                        "file_name": "report.pdf",
                        "media": {
                            "url": "https://example.com/report.pdf",
                            "local_path": "/tmp/report.pdf",
                            "mime": "application/pdf"
                        }
                    }
                },
                {
                    "type": 5,
                    "video_item": {
                        "media": {
                            "url": "https://example.com/video.mp4",
                            "local_path": "/tmp/video.mp4",
                            "mime": "video/mp4"
                        }
                    }
                }
            ]
        }))
        .expect("typed local media request");

    match &request.input[0] {
        IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "https://example.com/report.pdf");
            assert_eq!(path.to_string_lossy(), "/tmp/report.pdf");
            assert_eq!(mime.as_deref(), Some("application/pdf"));
            assert_eq!(name.as_deref(), Some("report.pdf"));
        }
        other => panic!("expected file part, got {other:?}"),
    }
    match &request.input[1] {
        IncomingContentPart::Video {
            file_id,
            path,
            mime,
            name,
        } => {
            assert_eq!(file_id, "https://example.com/video.mp4");
            assert_eq!(path.to_string_lossy(), "/tmp/video.mp4");
            assert_eq!(mime.as_deref(), Some("video/mp4"));
            assert_eq!(name.as_deref(), Some("video.mp4"));
        }
        other => panic!("expected video part, got {other:?}"),
    }
}

#[tokio::test]
async fn get_bot_qrcode_uses_ilink_endpoint_query_and_bearer_header() {
    let observed = Arc::new(Mutex::new(None::<(HeaderMap, HashMap<String, String>)>));
    let observed_for_route = observed.clone();
    let app = Router::new().route(
        "/ilink/bot/get_bot_qrcode",
        get(
            move |headers: HeaderMap, Query(query): Query<HashMap<String, String>>| {
                let observed = observed_for_route.clone();
                async move {
                    *observed.lock().await = Some((headers, query));
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "qrcode": "qr-code",
                            "qrcode_img_content": "base64",
                            "url": "https://example.test/qr"
                        })),
                    )
                }
            },
        ),
    );
    let (base_url, handle) = spawn_app(app).await;
    let client = ILinkClient::new("token123".into(), Some(base_url));

    let response = client.get_bot_qrcode().await.expect("qrcode response");
    handle.abort();

    assert_eq!(response.qrcode, "qr-code");
    assert_eq!(response.url.as_deref(), Some("https://example.test/qr"));
    let (headers, query) = observed
        .lock()
        .await
        .clone()
        .expect("request should be observed");
    assert_eq!(query.get("bot_type").map(String::as_str), Some("3"));
    assert_eq!(
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer token123")
    );
}

#[tokio::test]
async fn getupdates_posts_cursor_and_channel_version() {
    let observed = Arc::new(Mutex::new(None::<(HeaderMap, Value)>));
    let observed_for_route = observed.clone();
    let app = Router::new().route(
        "/ilink/bot/getupdates",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let observed = observed_for_route.clone();
            async move {
                *observed.lock().await = Some((headers, body));
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "ret": 0,
                        "msgs": [],
                        "get_updates_buf": "next-cursor"
                    })),
                )
            }
        }),
    );
    let (base_url, handle) = spawn_app(app).await;
    let client = ILinkClient::new("token123".into(), Some(base_url));

    let response = client.getupdates("cursor_abc").await.expect("updates");
    handle.abort();

    assert_eq!(response.ret, 0);
    assert_eq!(response.get_updates_buf.as_deref(), Some("next-cursor"));
    let (headers, body) = observed
        .lock()
        .await
        .clone()
        .expect("request should be observed");
    assert_eq!(body["get_updates_buf"], "cursor_abc");
    assert_eq!(body["base_info"]["channel_version"], CHANNEL_VERSION);
    assert_eq!(
        headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
}

#[tokio::test]
async fn sendmessage_posts_wechat_text_payload_shape() {
    let observed = Arc::new(Mutex::new(None::<Value>));
    let observed_for_route = observed.clone();
    let app = Router::new().route(
        "/ilink/bot/sendmessage",
        post(move |Json(body): Json<Value>| {
            let observed = observed_for_route.clone();
            async move {
                *observed.lock().await = Some(body);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({"ret": 0, "msg": "ok"})),
                )
            }
        }),
    );
    let (base_url, handle) = spawn_app(app).await;
    let client = ILinkClient::new("token123".into(), Some(base_url));

    let response = client
        .sendmessage("user@im.wechat", "hello", "ctx_123")
        .await
        .expect("sendmessage");
    handle.abort();

    assert_eq!(response["ret"], 0);
    let body = observed
        .lock()
        .await
        .clone()
        .expect("request should be observed");
    assert_eq!(body["msg"]["to_user_id"], "user@im.wechat");
    assert_eq!(body["msg"]["context_token"], "ctx_123");
    assert_eq!(body["msg"]["message_type"], 2);
    assert_eq!(body["msg"]["message_state"], 2);
    assert_eq!(body["msg"]["item_list"][0]["type"], 1);
    assert_eq!(body["msg"]["item_list"][0]["text_item"]["text"], "hello");
    assert!(body["msg"]["client_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    assert_eq!(body["base_info"]["channel_version"], CHANNEL_VERSION);
}

#[tokio::test]
async fn send_image_uploads_encrypted_media_then_sends_image_item() {
    let getupload_body = Arc::new(Mutex::new(None::<Value>));
    let upload_body = Arc::new(Mutex::new(None::<Vec<u8>>));
    let send_body = Arc::new(Mutex::new(None::<Value>));

    let upload_body_for_route = upload_body.clone();
    let cdn_app = Router::new().route(
        "/cdn/upload",
        post(move |body: Body| {
            let upload_body = upload_body_for_route.clone();
            async move {
                let bytes = to_bytes(body, 1024 * 1024)
                    .await
                    .expect("upload body")
                    .to_vec();
                *upload_body.lock().await = Some(bytes);
                Response::builder()
                    .status(StatusCode::OK)
                    .header("x-encrypted-param", "encrypted-param-123")
                    .body(Body::empty())
                    .unwrap()
            }
        }),
    );
    let (cdn_base_url, cdn_handle) = spawn_app(cdn_app).await;
    let upload_url = format!("{cdn_base_url}/cdn/upload");

    let getupload_for_route = getupload_body.clone();
    let send_for_route = send_body.clone();
    let ilink_app = Router::new()
        .route(
            "/ilink/bot/getuploadurl",
            post(move |Json(body): Json<Value>| {
                let getupload = getupload_for_route.clone();
                let upload_url = upload_url.clone();
                async move {
                    *getupload.lock().await = Some(body);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "upload_full_url": upload_url })),
                    )
                }
            }),
        )
        .route(
            "/ilink/bot/sendmessage",
            post(move |Json(body): Json<Value>| {
                let send = send_for_route.clone();
                async move {
                    *send.lock().await = Some(body);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({"ret": 0, "msg": "ok"})),
                    )
                }
            }),
        );
    let (base_url, ilink_handle) = spawn_app(ilink_app).await;
    let temp = tempdir().unwrap();
    let image_path = temp.path().join("chart.png");
    std::fs::write(&image_path, "png-bytes").unwrap();
    let client = ILinkClient::new("token123".into(), Some(base_url));

    let response = client
        .send_image("user@im.wechat", &image_path, "ctx_123")
        .await
        .expect("send image");
    assert_eq!(response["ret"], 0);

    let getupload = getupload_body
        .lock()
        .await
        .clone()
        .expect("getuploadurl request");
    assert_eq!(getupload["media_type"], 1);
    assert_eq!(getupload["to_user_id"], "user@im.wechat");
    assert_eq!(getupload["rawsize"], 9);
    assert_eq!(getupload["filesize"], 16);
    assert_eq!(
        getupload["rawfilemd5"].as_str().unwrap().len(),
        32,
        "WeChat upload must send raw MD5 like QwenPaw"
    );
    assert_eq!(getupload["aeskey"].as_str().unwrap().len(), 32);
    assert_eq!(getupload["base_info"]["channel_version"], CHANNEL_VERSION);

    let uploaded = upload_body.lock().await.clone().expect("cdn upload body");
    assert_eq!(uploaded.len(), 16);
    assert_ne!(uploaded, b"png-bytes");

    let send = send_body.lock().await.clone().expect("sendmessage body");
    assert_eq!(send["msg"]["to_user_id"], "user@im.wechat");
    assert_eq!(send["msg"]["context_token"], "ctx_123");
    assert_eq!(send["msg"]["item_list"][0]["type"], 2);
    assert_eq!(
        send["msg"]["item_list"][0]["image_item"]["media"]["encrypt_query_param"],
        "encrypted-param-123"
    );
    assert_eq!(
        send["msg"]["item_list"][0]["image_item"]["media"]["encrypt_type"],
        1
    );
    assert_eq!(send["msg"]["item_list"][0]["image_item"]["mid_size"], 16);
    assert!(
        send["msg"]["item_list"][0]["image_item"]["media"]["aes_key"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );

    ilink_handle.abort();
    cdn_handle.abort();
}

#[tokio::test]
async fn channel_send_posts_text_first_then_native_image_media() {
    let getupload_body = Arc::new(Mutex::new(None::<Value>));
    let upload_body = Arc::new(Mutex::new(None::<Vec<u8>>));
    let send_bodies = Arc::new(Mutex::new(Vec::<Value>::new()));

    let upload_body_for_route = upload_body.clone();
    let cdn_app = Router::new().route(
        "/cdn/upload",
        post(move |body: Body| {
            let upload_body = upload_body_for_route.clone();
            async move {
                let bytes = to_bytes(body, 1024 * 1024)
                    .await
                    .expect("upload body")
                    .to_vec();
                *upload_body.lock().await = Some(bytes);
                Response::builder()
                    .status(StatusCode::OK)
                    .header("x-encrypted-param", "encrypted-param-channel")
                    .body(Body::empty())
                    .unwrap()
            }
        }),
    );
    let (cdn_base_url, cdn_handle) = spawn_app(cdn_app).await;
    let upload_url = format!("{cdn_base_url}/cdn/upload");

    let getupload_for_route = getupload_body.clone();
    let sends_for_route = send_bodies.clone();
    let ilink_app = Router::new()
        .route(
            "/ilink/bot/getuploadurl",
            post(move |Json(body): Json<Value>| {
                let getupload = getupload_for_route.clone();
                let upload_url = upload_url.clone();
                async move {
                    *getupload.lock().await = Some(body);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "upload_full_url": upload_url })),
                    )
                }
            }),
        )
        .route(
            "/ilink/bot/sendmessage",
            post(move |Json(body): Json<Value>| {
                let sends = sends_for_route.clone();
                async move {
                    sends.lock().await.push(body);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({"ret": 0, "msg": "ok"})),
                    )
                }
            }),
        );
    let (base_url, ilink_handle) = spawn_app(ilink_app).await;
    let temp = tempdir().unwrap();
    let image_path = temp.path().join("chart.png");
    std::fs::write(&image_path, "png-bytes").unwrap();
    let channel = WechatChannel::new("token123".into(), Some(base_url));
    let target = ChannelTarget {
        agent_id: "agent".into(),
        conversation_id: "wechat:user@im.wechat".into(),
        sender_id: Some("user@im.wechat".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "wechat_from_user_id": "user@im.wechat",
            "wechat_context_token": "ctx_123"
        }),
    };
    let msg = OutgoingMessage {
        channel_id: "wechat".into(),
        conversation_id: "wechat:user@im.wechat".into(),
        receiver_id: Some("user@im.wechat".into()),
        content: vec![
            OutgoingContentPart::Text {
                text: "summary".into(),
            },
            OutgoingContentPart::Image {
                url: None,
                path: Some(image_path),
                alt: Some("chart.png".into()),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel
        .send(target, msg)
        .await
        .expect("wechat channel send");

    let sends = send_bodies.lock().await;
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[0]["msg"]["item_list"][0]["type"], 1);
    assert_eq!(
        sends[0]["msg"]["item_list"][0]["text_item"]["text"],
        "summary"
    );
    assert_eq!(sends[1]["msg"]["item_list"][0]["type"], 2);
    assert_eq!(
        sends[1]["msg"]["item_list"][0]["image_item"]["media"]["encrypt_query_param"],
        "encrypted-param-channel"
    );
    assert!(upload_body.lock().await.is_some());
    assert_eq!(
        getupload_body.lock().await.clone().unwrap()["media_type"],
        1
    );

    ilink_handle.abort();
    cdn_handle.abort();
}

#[tokio::test]
async fn channel_send_uploads_local_file_audio_and_video_media() {
    let getupload_bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let upload_bodies = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let send_bodies = Arc::new(Mutex::new(Vec::<Value>::new()));

    let upload_bodies_for_route = upload_bodies.clone();
    let cdn_app = Router::new().route(
        "/cdn/upload",
        post(move |body: Body| {
            let upload_bodies = upload_bodies_for_route.clone();
            async move {
                let bytes = to_bytes(body, 1024 * 1024)
                    .await
                    .expect("upload body")
                    .to_vec();
                upload_bodies.lock().await.push(bytes);
                Response::builder()
                    .status(StatusCode::OK)
                    .header("x-encrypted-param", "encrypted-param-channel")
                    .body(Body::empty())
                    .unwrap()
            }
        }),
    );
    let (cdn_base_url, cdn_handle) = spawn_app(cdn_app).await;
    let upload_url = format!("{cdn_base_url}/cdn/upload");

    let getupload_for_route = getupload_bodies.clone();
    let sends_for_route = send_bodies.clone();
    let ilink_app = Router::new()
        .route(
            "/ilink/bot/getuploadurl",
            post(move |Json(body): Json<Value>| {
                let getupload = getupload_for_route.clone();
                let upload_url = upload_url.clone();
                async move {
                    getupload.lock().await.push(body);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "upload_full_url": upload_url })),
                    )
                }
            }),
        )
        .route(
            "/ilink/bot/sendmessage",
            post(move |Json(body): Json<Value>| {
                let sends = sends_for_route.clone();
                async move {
                    sends.lock().await.push(body);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({"ret": 0, "msg": "ok"})),
                    )
                }
            }),
        );
    let (base_url, ilink_handle) = spawn_app(ilink_app).await;
    let temp = tempdir().unwrap();
    let file_path = temp.path().join("report.pdf");
    let audio_path = temp.path().join("voice.m4a");
    let video_path = temp.path().join("clip.mp4");
    std::fs::write(&file_path, "pdf-bytes").unwrap();
    std::fs::write(&audio_path, "audio-bytes").unwrap();
    std::fs::write(&video_path, "video-bytes").unwrap();
    let channel = WechatChannel::new("token123".into(), Some(base_url));
    let target = ChannelTarget {
        agent_id: "agent".into(),
        conversation_id: "wechat:user@im.wechat".into(),
        sender_id: Some("user@im.wechat".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "wechat_from_user_id": "user@im.wechat",
            "wechat_context_token": "ctx_123"
        }),
    };
    let msg = OutgoingMessage {
        channel_id: "wechat".into(),
        conversation_id: "wechat:user@im.wechat".into(),
        receiver_id: Some("user@im.wechat".into()),
        content: vec![
            OutgoingContentPart::File {
                url: None,
                path: Some(file_path),
                name: Some("report.pdf".into()),
            },
            OutgoingContentPart::Audio {
                url: None,
                path: Some(audio_path),
                name: Some("voice.m4a".into()),
            },
            OutgoingContentPart::Video {
                url: None,
                path: Some(video_path),
                name: Some("clip.mp4".into()),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel
        .send(target, msg)
        .await
        .expect("wechat channel send media");

    let getuploads = getupload_bodies.lock().await;
    let media_types: Vec<i64> = getuploads
        .iter()
        .map(|body| body["media_type"].as_i64().unwrap())
        .collect();
    assert_eq!(media_types, vec![3, 4, 2]);
    assert!(upload_bodies
        .lock()
        .await
        .iter()
        .all(|body| !body.is_empty()));

    let sends = send_bodies.lock().await;
    assert_eq!(sends.len(), 3);
    assert_eq!(sends[0]["msg"]["item_list"][0]["type"], 4);
    assert_eq!(
        sends[0]["msg"]["item_list"][0]["file_item"]["file_name"],
        "report.pdf"
    );
    assert_eq!(sends[1]["msg"]["item_list"][0]["type"], 4);
    assert_eq!(
        sends[1]["msg"]["item_list"][0]["file_item"]["file_name"],
        "voice.m4a"
    );
    assert_eq!(sends[2]["msg"]["item_list"][0]["type"], 5);
    assert_eq!(
        sends[2]["msg"]["item_list"][0]["video_item"]["media"]["encrypt_query_param"],
        "encrypted-param-channel"
    );

    ilink_handle.abort();
    cdn_handle.abort();
}

#[tokio::test]
async fn qrcode_status_deserializes_confirmed_login_fields() {
    let app = Router::new().route(
        "/ilink/bot/get_qrcode_status",
        get(|Query(query): Query<HashMap<String, String>>| async move {
            assert_eq!(query.get("qrcode").map(String::as_str), Some("qr-code"));
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "confirmed",
                    "bot_token": "new-token",
                    "baseurl": "https://api.weixin.test"
                })),
            )
        }),
    );
    let (base_url, handle) = spawn_app(app).await;
    let client = ILinkClient::new(String::new(), Some(base_url));

    let status = client
        .get_qrcode_status("qr-code")
        .await
        .expect("qrcode status");
    handle.abort();

    assert_eq!(status.status, "confirmed");
    assert_eq!(status.bot_token, "new-token");
    assert_eq!(status.baseurl, "https://api.weixin.test");
}

#[tokio::test]
async fn getconfig_and_sendtyping_use_ilink_typing_shapes() {
    let config_bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let typing_bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let config_for_route = config_bodies.clone();
    let typing_for_route = typing_bodies.clone();
    let app = Router::new()
        .route(
            "/ilink/bot/getconfig",
            post(move |Json(body): Json<Value>| {
                let config_bodies = config_for_route.clone();
                async move {
                    config_bodies.lock().await.push(body);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "ret": 0,
                            "errcode": 0,
                            "typing_ticket": "ticket-123"
                        })),
                    )
                }
            }),
        )
        .route(
            "/ilink/bot/sendtyping",
            post(move |Json(body): Json<Value>| {
                let typing_bodies = typing_for_route.clone();
                async move {
                    typing_bodies.lock().await.push(body);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({"ret": 0, "errcode": 0})),
                    )
                }
            }),
        );
    let (base_url, handle) = spawn_app(app).await;
    let client = ILinkClient::new("token123".into(), Some(base_url));

    let config = client
        .getconfig("user@im.wechat", "ctx-123")
        .await
        .expect("getconfig");
    assert_eq!(config["typing_ticket"], "ticket-123");
    client
        .sendtyping("user@im.wechat", "ticket-123", 1)
        .await
        .expect("sendtyping");

    let config_bodies = config_bodies.lock().await;
    assert_eq!(config_bodies.len(), 1);
    assert_eq!(config_bodies[0]["ilink_user_id"], "user@im.wechat");
    assert_eq!(config_bodies[0]["context_token"], "ctx-123");
    assert_eq!(
        config_bodies[0]["base_info"]["channel_version"],
        CHANNEL_VERSION
    );
    let typing_bodies = typing_bodies.lock().await;
    assert_eq!(typing_bodies.len(), 1);
    assert_eq!(typing_bodies[0]["ilink_user_id"], "user@im.wechat");
    assert_eq!(typing_bodies[0]["typing_ticket"], "ticket-123");
    assert_eq!(typing_bodies[0]["status"], 1);
    assert_eq!(
        typing_bodies[0]["base_info"]["channel_version"],
        CHANNEL_VERSION
    );

    handle.abort();
}

#[tokio::test]
async fn typing_indicator_fetches_ticket_starts_and_stops_typing() {
    let typing_bodies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let typing_for_route = typing_bodies.clone();
    let app = Router::new()
        .route(
            "/ilink/bot/getconfig",
            post(|Json(_body): Json<Value>| async move {
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "ret": 0,
                        "errcode": 0,
                        "typing_ticket": "ticket-abc"
                    })),
                )
            }),
        )
        .route(
            "/ilink/bot/sendtyping",
            post(move |Json(body): Json<Value>| {
                let typing_bodies = typing_for_route.clone();
                async move {
                    typing_bodies.lock().await.push(body);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({"ret": 0, "errcode": 0})),
                    )
                }
            }),
        );
    let (base_url, handle) = spawn_app(app).await;
    let client = ILinkClient::new("token123".into(), Some(base_url));
    let indicator = WechatTypingIndicator::new(client, "user@im.wechat".into(), "ctx-123".into());
    let req = indicator_request();

    indicator.start(&req).await.expect("start typing");
    indicator.stop(&req).await.expect("stop typing");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let typing_bodies = typing_bodies.lock().await;
    assert_eq!(typing_bodies.len(), 2);
    assert_eq!(typing_bodies[0]["ilink_user_id"], "user@im.wechat");
    assert_eq!(typing_bodies[0]["typing_ticket"], "ticket-abc");
    assert_eq!(typing_bodies[0]["status"], 1);
    assert_eq!(typing_bodies[1]["status"], 2);

    handle.abort();
}
