use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Json, Path};
use axum::http::HeaderMap;
use rua_channel::core::channel::Channel;
use rua_channel::core::content::{ChannelTarget, OutgoingContentPart, OutgoingMessage};
use rua_channel::core::streaming::{PlatformStreamPolicy, StreamHandler, StreamType};
use rua_channel::feishu::channel::FeishuStreamHandler;
use rua_channel::feishu::{FeishuChannel, FeishuConfig, FeishuTokenManager};
use tokio::sync::Mutex;

fn make_target() -> ChannelTarget {
    ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

fn make_text_msg(text: &str) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "feishu".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text { text: text.into() }],
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

fn approval_msg() -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "feishu".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "Approve Bash?".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "tool_guard_approval",
            "approval_request_id": "req-123",
            "tool_name": "Bash",
            "severity": "high",
        }),
    }
}

#[test]
fn channel_exposes_stable_feishu_id() {
    let ch = FeishuChannel::new(FeishuConfig::default());

    assert_eq!(ch.id(), "feishu");
}

#[test]
fn build_text_body_without_secret_omits_signature_fields() {
    let body = FeishuChannel::build_text_body("hello", None);

    assert_eq!(body["msg_type"], "text");
    assert_eq!(body["content"]["text"], "hello");
    assert!(body.get("sign").is_none());
    assert!(body.get("timestamp").is_none());
}

#[test]
fn build_text_body_with_secret_includes_signature_fields() {
    let body = FeishuChannel::build_text_body("hello", Some("my-secret"));

    assert_eq!(body["msg_type"], "text");
    assert_eq!(body["content"]["text"], "hello");
    assert!(body.get("sign").is_some());
    assert!(body.get("timestamp").is_some());
}

#[tokio::test]
async fn integration_send_text_post_body() {
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/hook/test",
        axum::routing::post(move |Json(body): Json<serde_json::Value>| {
            let store = received_clone.clone();
            async move {
                store.lock().await.push(body);
                axum::http::StatusCode::OK
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = FeishuConfig {
        webhook_url: format!("http://127.0.0.1:{}/hook/test", addr.port()),
        secret: None,
        ..Default::default()
    };
    let ch = FeishuChannel::new(config);

    ch.send(make_target(), make_text_msg("hello feishu"))
        .await
        .unwrap();

    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["msg_type"], "text");
    assert_eq!(msgs[0]["content"]["text"], "hello feishu");
    assert!(msgs[0].get("sign").is_none());
    assert!(msgs[0].get("timestamp").is_none());

    server.abort();
}

#[tokio::test]
async fn integration_send_renders_text_and_markdown_parts() {
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/hook/render",
        axum::routing::post(move |Json(body): Json<serde_json::Value>| {
            let store = received_clone.clone();
            async move {
                store.lock().await.push(body);
                axum::http::StatusCode::OK
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = FeishuConfig {
        webhook_url: format!("http://127.0.0.1:{}/hook/render", addr.port()),
        secret: None,
        ..Default::default()
    };
    let ch = FeishuChannel::new(config);
    let msg = OutgoingMessage {
        channel_id: "feishu".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![
            OutgoingContentPart::Text {
                text: "Hello".into(),
            },
            OutgoingContentPart::Markdown {
                text: "**world**".into(),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    ch.send(make_target(), msg).await.unwrap();

    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["content"]["text"], "Hello\n**world**");

    server.abort();
}

#[tokio::test]
async fn integration_send_uploads_image_and_file_via_im_v1() {
    let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let token_calls = calls.clone();
    let image_calls = calls.clone();
    let file_calls = calls.clone();
    let msg_calls = calls.clone();
    let app = axum::Router::new()
        .route(
            "/open-apis/auth/v3/tenant_access_token/internal",
            axum::routing::post(move |body: Bytes| {
                let calls = token_calls.clone();
                async move {
                    calls
                        .lock()
                        .await
                        .push(("token".into(), String::from_utf8_lossy(&body).to_string()));
                    Json(serde_json::json!({
                        "code": 0,
                        "tenant_access_token": "tenant-token",
                        "expire": 7200
                    }))
                }
            }),
        )
        .route(
            "/open-apis/im/v1/images",
            axum::routing::post(move |headers: HeaderMap, body: Bytes| {
                let calls = image_calls.clone();
                async move {
                    calls.lock().await.push((
                        "image_upload".into(),
                        format!(
                            "{}\n{}",
                            headers
                                .get("content-type")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default(),
                            String::from_utf8_lossy(&body)
                        ),
                    ));
                    Json(serde_json::json!({
                        "code": 0,
                        "data": {"image_key": "img-key-1"}
                    }))
                }
            }),
        )
        .route(
            "/open-apis/im/v1/files",
            axum::routing::post(move |headers: HeaderMap, body: Bytes| {
                let calls = file_calls.clone();
                async move {
                    calls.lock().await.push((
                        "file_upload".into(),
                        format!(
                            "{}\n{}",
                            headers
                                .get("content-type")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default(),
                            String::from_utf8_lossy(&body)
                        ),
                    ));
                    Json(serde_json::json!({
                        "code": 0,
                        "data": {"file_key": "file-key-1"}
                    }))
                }
            }),
        )
        .route(
            "/open-apis/im/v1/messages",
            axum::routing::post(move |Json(body): Json<serde_json::Value>| {
                let calls = msg_calls.clone();
                async move {
                    calls
                        .lock()
                        .await
                        .push(("message".into(), body.to_string()));
                    Json(serde_json::json!({"code": 0, "msg": "ok"}))
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let image_path = temp.path().join("diagram.png");
    let file_path = temp.path().join("report.pdf");
    let audio_path = temp.path().join("voice.mp3");
    let video_path = temp.path().join("clip.mp4");
    std::fs::write(&image_path, b"png-bytes").unwrap();
    std::fs::write(&file_path, b"pdf-bytes").unwrap();
    std::fs::write(&audio_path, b"audio-bytes").unwrap();
    std::fs::write(&video_path, b"video-bytes").unwrap();
    let client = reqwest::Client::new();
    let tm = FeishuTokenManager::new_with_token_url(
        "app".into(),
        "secret".into(),
        client.clone(),
        format!("{base_url}/open-apis/auth/v3/tenant_access_token/internal"),
    );
    let ch = FeishuChannel::new(FeishuConfig::default())
        .with_client(client)
        .with_token_manager(tm)
        .with_open_api_base_url(base_url);
    let target = ChannelTarget {
        metadata: serde_json::json!({
            "feishu_chat_type": "p2p",
            "feishu_sender_id": "ou-user-1"
        }),
        ..make_target()
    };
    let msg = OutgoingMessage {
        channel_id: "feishu".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![
            OutgoingContentPart::Text {
                text: "summary".into(),
            },
            OutgoingContentPart::Image {
                url: None,
                path: Some(image_path),
                alt: Some("diagram".into()),
            },
            OutgoingContentPart::File {
                url: None,
                path: Some(file_path),
                name: Some("report.pdf".into()),
            },
            OutgoingContentPart::Audio {
                url: None,
                path: Some(audio_path),
                name: Some("voice.mp3".into()),
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

    ch.send(target, msg).await.unwrap();

    let calls = calls.lock().await;
    assert!(calls.iter().any(|(kind, _)| kind == "token"));
    let image_upload = calls
        .iter()
        .find(|(kind, _)| kind == "image_upload")
        .expect("image upload");
    assert!(image_upload.1.contains("multipart/form-data"));
    assert!(image_upload.1.contains("png-bytes"));
    let file_uploads: Vec<_> = calls
        .iter()
        .filter(|(kind, _)| kind == "file_upload")
        .collect();
    assert_eq!(file_uploads.len(), 3);
    assert!(file_uploads[0].1.contains("multipart/form-data"));
    assert!(file_uploads[0].1.contains("pdf-bytes"));
    assert!(file_uploads[1].1.contains("audio-bytes"));
    assert!(file_uploads[2].1.contains("video-bytes"));
    let messages: Vec<serde_json::Value> = calls
        .iter()
        .filter(|(kind, _)| kind == "message")
        .map(|(_, body)| serde_json::from_str(body).unwrap())
        .collect();
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0]["msg_type"], "text");
    assert_eq!(messages[0]["content"], r#"{"text":"summary"}"#);
    assert_eq!(messages[1]["msg_type"], "image");
    assert_eq!(messages[1]["content"], r#"{"image_key":"img-key-1"}"#);
    for message in messages.iter().skip(2) {
        assert_eq!(message["msg_type"], "file");
        assert_eq!(message["content"], r#"{"file_key":"file-key-1"}"#);
    }

    server.abort();
}

#[tokio::test]
async fn integration_send_tool_guard_approval_uses_interactive_card() {
    let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let token_calls = calls.clone();
    let msg_calls = calls.clone();
    let app = axum::Router::new()
        .route(
            "/open-apis/auth/v3/tenant_access_token/internal",
            axum::routing::post(move |body: Bytes| {
                let calls = token_calls.clone();
                async move {
                    calls
                        .lock()
                        .await
                        .push(("token".into(), String::from_utf8_lossy(&body).to_string()));
                    Json(serde_json::json!({
                        "code": 0,
                        "tenant_access_token": "tenant-token",
                        "expire": 7200
                    }))
                }
            }),
        )
        .route(
            "/open-apis/im/v1/messages",
            axum::routing::post(move |Json(body): Json<serde_json::Value>| {
                let calls = msg_calls.clone();
                async move {
                    calls
                        .lock()
                        .await
                        .push(("message".into(), body.to_string()));
                    Json(serde_json::json!({"code": 0, "msg": "ok"}))
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    let tm = FeishuTokenManager::new_with_token_url(
        "app".into(),
        "secret".into(),
        client.clone(),
        format!("{base_url}/open-apis/auth/v3/tenant_access_token/internal"),
    );
    let ch = FeishuChannel::new(FeishuConfig::default())
        .with_client(client)
        .with_token_manager(tm)
        .with_open_api_base_url(base_url);
    let target = ChannelTarget {
        metadata: serde_json::json!({
            "feishu_chat_type": "p2p",
            "feishu_sender_id": "ou-user-1"
        }),
        ..make_target()
    };

    ch.send(target, approval_msg()).await.unwrap();

    let calls = calls.lock().await;
    let message: serde_json::Value = serde_json::from_str(
        &calls
            .iter()
            .find(|(kind, _)| kind == "message")
            .expect("message")
            .1,
    )
    .unwrap();
    assert_eq!(message["receive_id"], "ou-user-1");
    assert_eq!(message["msg_type"], "interactive");
    let content: serde_json::Value =
        serde_json::from_str(message["content"].as_str().expect("content json")).unwrap();
    assert_eq!(content["header"]["template"], "red");
    assert_eq!(content["elements"][0]["content"], "Approve Bash?");
    let actions = &content["elements"][2]["actions"];
    assert_eq!(actions[0]["value"]["type"], "tool_guard_approval");
    assert_eq!(actions[0]["value"]["action"], "approve");
    assert_eq!(actions[0]["value"]["request_id"], "req-123");
    assert_eq!(actions[1]["value"]["action"], "deny");

    server.abort();
}

#[tokio::test]
async fn stream_handler_creates_updates_and_finalizes_cardkit_card() {
    let calls = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let token_calls = calls.clone();
    let card_create_calls = calls.clone();
    let message_calls = calls.clone();
    let content_calls = calls.clone();
    let settings_calls = calls.clone();
    let app = axum::Router::new()
        .route(
            "/open-apis/auth/v3/tenant_access_token/internal",
            axum::routing::post(move |body: Bytes| {
                let calls = token_calls.clone();
                async move {
                    calls
                        .lock()
                        .await
                        .push(("token".into(), String::from_utf8_lossy(&body).to_string()));
                    Json(serde_json::json!({
                        "code": 0,
                        "tenant_access_token": "tenant-token",
                        "expire": 7200
                    }))
                }
            }),
        )
        .route(
            "/open-apis/cardkit/v1/cards",
            axum::routing::post(move |Json(body): Json<serde_json::Value>| {
                let calls = card_create_calls.clone();
                async move {
                    calls
                        .lock()
                        .await
                        .push(("card_create".into(), body.to_string()));
                    Json(serde_json::json!({
                        "code": 0,
                        "data": { "card_id": "card-123" }
                    }))
                }
            }),
        )
        .route(
            "/open-apis/im/v1/messages",
            axum::routing::post(move |Json(body): Json<serde_json::Value>| {
                let calls = message_calls.clone();
                async move {
                    calls
                        .lock()
                        .await
                        .push(("message".into(), body.to_string()));
                    Json(serde_json::json!({"code": 0, "msg": "ok"}))
                }
            }),
        )
        .route(
            "/open-apis/cardkit/v1/cards/{card_id}/elements/{element_id}/content",
            axum::routing::put(
                move |Path((card_id, element_id)): Path<(String, String)>,
                      Json(body): Json<serde_json::Value>| {
                    let calls = content_calls.clone();
                    async move {
                        calls.lock().await.push((
                            "content".into(),
                            serde_json::json!({
                                "card_id": card_id,
                                "element_id": element_id,
                                "body": body
                            })
                            .to_string(),
                        ));
                        Json(serde_json::json!({"code": 0, "msg": "ok"}))
                    }
                },
            ),
        )
        .route(
            "/open-apis/cardkit/v1/cards/{card_id}/settings",
            axum::routing::patch(
                move |Path(card_id): Path<String>, Json(body): Json<serde_json::Value>| {
                    let calls = settings_calls.clone();
                    async move {
                        calls.lock().await.push((
                            "settings".into(),
                            serde_json::json!({
                                "card_id": card_id,
                                "body": body
                            })
                            .to_string(),
                        ));
                        Json(serde_json::json!({"code": 0, "msg": "ok"}))
                    }
                },
            ),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    let tm = FeishuTokenManager::new_with_token_url(
        "app".into(),
        "secret".into(),
        client.clone(),
        format!("{base_url}/open-apis/auth/v3/tenant_access_token/internal"),
    );
    let handler = FeishuStreamHandler::new(
        client,
        base_url,
        tm,
        "open_id".into(),
        "ou-user-1".into(),
        PlatformStreamPolicy::card_update(std::time::Duration::ZERO, 3000),
    );

    handler
        .on_delta(StreamType::Message, "hello")
        .await
        .expect("create card");
    handler
        .on_delta(StreamType::Message, "hello there")
        .await
        .expect("update card");
    handler
        .on_end(StreamType::Message, "hello world")
        .await
        .expect("finalize card");

    let calls = calls.lock().await;
    let card_create: serde_json::Value = serde_json::from_str(
        &calls
            .iter()
            .find(|(kind, _)| kind == "card_create")
            .expect("card create")
            .1,
    )
    .unwrap();
    let card_data: serde_json::Value =
        serde_json::from_str(card_create["data"].as_str().expect("card data")).unwrap();
    assert_eq!(card_create["type"], "card_json");
    assert_eq!(card_data["config"]["streaming_mode"], true);
    assert_eq!(
        card_data["body"]["elements"][0]["element_id"],
        "rua_stream_text"
    );
    assert_eq!(card_data["body"]["elements"][0]["content"], "hello");

    let message: serde_json::Value = serde_json::from_str(
        &calls
            .iter()
            .find(|(kind, _)| kind == "message")
            .expect("interactive message")
            .1,
    )
    .unwrap();
    assert_eq!(message["receive_id"], "ou-user-1");
    assert_eq!(message["msg_type"], "interactive");
    let content: serde_json::Value =
        serde_json::from_str(message["content"].as_str().expect("message content")).unwrap();
    assert_eq!(content["type"], "card");
    assert_eq!(content["data"]["card_id"], "card-123");

    let content_updates = calls
        .iter()
        .filter(|(kind, _)| kind == "content")
        .map(|(_, body)| serde_json::from_str::<serde_json::Value>(body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(content_updates.len(), 2);
    assert_eq!(content_updates[0]["card_id"], "card-123");
    assert_eq!(content_updates[0]["element_id"], "rua_stream_text");
    assert_eq!(content_updates[0]["body"]["content"], "hello there");
    assert_eq!(content_updates[0]["body"]["sequence"], 1);
    assert_eq!(content_updates[1]["card_id"], "card-123");
    assert_eq!(content_updates[1]["element_id"], "rua_stream_text");
    assert_eq!(content_updates[1]["body"]["content"], "hello world");
    assert_eq!(content_updates[1]["body"]["sequence"], 2);

    let settings: serde_json::Value = serde_json::from_str(
        &calls
            .iter()
            .find(|(kind, _)| kind == "settings")
            .expect("settings update")
            .1,
    )
    .unwrap();
    assert_eq!(settings["card_id"], "card-123");
    assert_eq!(settings["body"]["sequence"], 3);
    let settings_body: serde_json::Value =
        serde_json::from_str(settings["body"]["settings"].as_str().unwrap()).unwrap();
    assert_eq!(
        settings_body["config"]["streaming_mode"],
        serde_json::Value::Bool(false)
    );
    assert_eq!(settings_body["config"]["summary"]["content"], "hello world");

    server.abort();
}

#[tokio::test]
async fn integration_send_with_secret_includes_sign() {
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/hook/signed",
        axum::routing::post(move |Json(body): Json<serde_json::Value>| {
            let store = received_clone.clone();
            async move {
                store.lock().await.push(body);
                axum::http::StatusCode::OK
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = FeishuConfig {
        webhook_url: format!("http://127.0.0.1:{}/hook/signed", addr.port()),
        secret: Some("test-secret-123".into()),
        ..Default::default()
    };
    let ch = FeishuChannel::new(config);

    ch.send(make_target(), make_text_msg("signed msg"))
        .await
        .unwrap();

    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["msg_type"], "text");
    assert_eq!(msgs[0]["content"]["text"], "signed msg");
    assert!(msgs[0]["sign"].is_string(), "sign field must be present");
    assert!(
        msgs[0]["timestamp"].is_string(),
        "timestamp field must be present"
    );

    server.abort();
}

#[tokio::test]
async fn integration_4xx_returns_error() {
    let app = axum::Router::new().route(
        "/hook/fail",
        axum::routing::post(|| async { axum::http::StatusCode::BAD_REQUEST }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = FeishuConfig {
        webhook_url: format!("http://127.0.0.1:{}/hook/fail", addr.port()),
        secret: None,
        ..Default::default()
    };
    let ch = FeishuChannel::new(config);

    let result = ch.send(make_target(), make_text_msg("will fail")).await;
    assert!(result.is_err());

    server.abort();
}

#[tokio::test]
async fn integration_empty_webhook_url_skips_send() {
    let config = FeishuConfig {
        webhook_url: String::new(),
        secret: None,
        ..Default::default()
    };
    let ch = FeishuChannel::new(config);

    let result = ch.send(make_target(), make_text_msg("should skip")).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn integration_empty_content_skips_post() {
    let config = FeishuConfig {
        webhook_url: "http://127.0.0.1:1/nonexistent".into(),
        secret: None,
        ..Default::default()
    };
    let ch = FeishuChannel::new(config);

    let msg = OutgoingMessage {
        channel_id: "feishu".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Thinking {
            text: "internal only".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    let result = ch.send(make_target(), msg).await;
    assert!(result.is_ok());
}
