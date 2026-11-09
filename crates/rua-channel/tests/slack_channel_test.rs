use std::sync::Arc;

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use rua_channel::core::channel::Channel;
use rua_channel::core::content::{ChannelTarget, IncomingContentPart, OutgoingContentPart};
use rua_channel::core::streaming::{StreamHandler, StreamType};
use rua_channel::slack::{SlackChannel, SlackConfig, SlackStreamHandler};
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct SlackApiState {
    calls: Arc<Mutex<Vec<serde_json::Value>>>,
    uploads: Arc<Mutex<Vec<String>>>,
}

async fn start_slack_api() -> (String, SlackApiState, tokio::task::JoinHandle<()>) {
    let state = SlackApiState::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new()
        .route(
            "/chat.postMessage",
            post({
                let state = state.clone();
                move |Json(body): Json<serde_json::Value>| {
                    let state = state.clone();
                    async move {
                        state.calls.lock().await.push(serde_json::json!({
                            "method": "postMessage",
                            "body": body,
                        }));
                        Json(serde_json::json!({ "ok": true, "ts": "1710000000.000100" }))
                    }
                }
            }),
        )
        .route(
            "/chat.update",
            post({
                let state = state.clone();
                move |Json(body): Json<serde_json::Value>| {
                    let state = state.clone();
                    async move {
                        state.calls.lock().await.push(serde_json::json!({
                            "method": "update",
                            "body": body,
                        }));
                        Json(serde_json::json!({ "ok": true }))
                    }
                }
            }),
        )
        .route(
            "/files.getUploadURLExternal",
            post(move |axum::Form(form): axum::Form<std::collections::HashMap<String, String>>| async move {
                assert!(!form["filename"].is_empty());
                assert!(form["length"].parse::<usize>().unwrap() > 0);
                Json(serde_json::json!({"ok": true, "upload_url": format!("http://{addr}/upload"), "file_id": "F-local"}))
            }),
        )
        .route(
            "/upload",
            post({
                let state = state.clone();
                move |headers: axum::http::HeaderMap, body: Body| {
                    let state = state.clone();
                    async move {
                        assert!(!headers.contains_key(header::AUTHORIZATION), "pre-signed upload must not receive bot credential");
                        let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
                        state.uploads.lock().await.push(String::from_utf8_lossy(&bytes).to_string());
                        StatusCode::OK
                    }
                }
            }),
        )
        .route(
            "/files.completeUploadExternal",
            post({
                let state = state.clone();
                move |Json(body): Json<serde_json::Value>| {
                    let state = state.clone();
                    async move {
                        assert_eq!(body["channel_id"], "C123");
                        assert_eq!(body["files"][0]["id"], "F-local");
                        state.uploads.lock().await.last_mut().unwrap().push_str(&body.to_string());
                        Json(serde_json::json!({"ok": true}))
                    }
                }
            }),
        )
        .route(
            "/files-pri/T123/report.pdf",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(headers.get(header::AUTHORIZATION).unwrap(), "Bearer xoxb-token");
                "local-slack-pdf"
            }),
        )
        .route(
            "/files/{file_id}",
            post(|Path(file_id): Path<String>| async move {
                Json(serde_json::json!({ "ok": true, "file": file_id }))
            }),
        )
        .route(
            "/conversations.replies",
            get(|| async {
                Json(serde_json::json!({
                    "ok": true,
                    "messages": [
                        {"text": "root"},
                        {"text": "reply 1"},
                        {"text": "reply 2"}
                    ]
                }))
            }),
        );
    let app = app.route(
        "/private/report.png",
        get(|headers: axum::http::HeaderMap| async move {
            let auth = headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");
            assert_eq!(auth, "Bearer xoxb-token");
            (
                [(header::CONTENT_TYPE, "image/png")],
                b"slack-image-bytes".to_vec(),
            )
        }),
    );
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{}", addr.port()), state, handle)
}

fn slack_channel(api_base: &str) -> SlackChannel {
    SlackChannel::new(SlackConfig {
        bot_token: "xoxb-token".into(),
        bot_user_id: "U_BOT".into(),
        ..Default::default()
    })
    .with_api_base(api_base.to_string())
}

fn slack_channel_with_media_dir(api_base: &str, media_dir: &std::path::Path) -> SlackChannel {
    SlackChannel::new(SlackConfig {
        bot_token: "xoxb-token".into(),
        bot_user_id: "U_BOT".into(),
        media_dir: media_dir.to_string_lossy().to_string(),
        ..Default::default()
    })
    .with_api_base(api_base.to_string())
}

#[test]
fn build_request_from_app_mention_strips_bot_mention_and_uses_thread_session() {
    let channel = slack_channel("http://localhost");
    let native = serde_json::json!({
        "event_id": "Ev-1",
        "event": {
            "type": "app_mention",
            "user": "U123",
            "channel": "C123",
            "channel_type": "channel",
            "text": "<@U_BOT> summarize this",
            "ts": "1710000000.000100",
            "thread_ts": "1710000000.000000"
        }
    });

    let req = channel.build_request_from_native(native).unwrap();

    assert_eq!(req.channel_id, "slack");
    assert_eq!(req.user_id, "U123");
    assert_eq!(req.conversation_id, "C123");
    assert_eq!(req.session_id, "slack_thread:C123:1710000000.000000");
    assert_eq!(req.message_id.as_deref(), Some("Ev-1"));
    assert_eq!(req.metadata["bot_mentioned"], true);
    assert_eq!(req.metadata["is_group"], true);
    assert_eq!(
        req.input,
        vec![IncomingContentPart::Text {
            text: "summarize this".into()
        }]
    );
}

#[test]
fn build_request_from_dm_and_file_url() {
    let channel = slack_channel("http://localhost");
    let native = serde_json::json!({
        "event_id": "Ev-2",
        "event": {
            "type": "message",
            "user": "U123",
            "channel": "D123",
            "channel_type": "im",
            "text": "see file",
            "ts": "1710000000.000200",
            "files": [
                {
                    "name": "report.pdf",
                    "url_private_download": "https://files.slack.com/report.pdf"
                }
            ]
        }
    });

    let req = channel.build_request_from_native(native).unwrap();

    assert_eq!(req.session_id, "slack_dm:D123");
    assert_eq!(req.metadata["is_dm"], true);
    assert!(req.input.iter().any(|part| matches!(
        part,
        IncomingContentPart::Url { url, title }
            if url == "https://files.slack.com/report.pdf"
                && title.as_deref() == Some("report.pdf")
    )));
}

#[tokio::test]
async fn build_request_with_media_download_caches_slack_private_file_as_typed_part() {
    let (api_base, _state, server) = start_slack_api().await;
    let temp = tempfile::tempdir().unwrap();
    let channel = slack_channel_with_media_dir(&api_base, temp.path());
    let native = serde_json::json!({
        "event_id": "Ev-download-1",
        "event": {
            "type": "message",
            "user": "U123",
            "channel": "D123",
            "channel_type": "im",
            "text": "see image",
            "ts": "1710000000.000250",
            "files": [
                {
                    "name": "report.png",
                    "mimetype": "image/png",
                    "url_private_download": format!("{api_base}/private/report.png")
                }
            ]
        }
    });

    let req = channel
        .build_request_from_native_with_media_download(native)
        .await
        .unwrap();

    assert!(req.input.iter().any(|part| matches!(
        part,
        IncomingContentPart::Text { text } if text == "see image"
    )));
    assert!(
        !req.input
            .iter()
            .any(|part| matches!(part, IncomingContentPart::Url { .. })),
        "downloaded Slack file should replace URL fallback"
    );
    let image_path = req
        .input
        .iter()
        .find_map(|part| match part {
            IncomingContentPart::Image { path, mime, .. } => {
                assert_eq!(mime.as_deref(), Some("image/png"));
                Some(path.clone())
            }
            _ => None,
        })
        .expect("image part");
    assert_eq!(std::fs::read(image_path).unwrap(), b"slack-image-bytes");
    server.abort();
}

#[tokio::test]
async fn build_request_with_media_download_keeps_url_fallback_on_download_failure() {
    let (api_base, _state, server) = start_slack_api().await;
    let temp = tempfile::tempdir().unwrap();
    let channel = slack_channel_with_media_dir(&api_base, temp.path());
    let missing_url = format!("{api_base}/private/missing.pdf");
    let native = serde_json::json!({
        "event_id": "Ev-download-2",
        "event": {
            "type": "message",
            "user": "U123",
            "channel": "D123",
            "channel_type": "im",
            "text": "see file",
            "ts": "1710000000.000260",
            "files": [
                {
                    "name": "missing.pdf",
                    "mimetype": "application/pdf",
                    "url_private_download": missing_url
                }
            ]
        }
    });

    let req = channel
        .build_request_from_native_with_media_download(native)
        .await
        .unwrap();

    assert!(req.input.iter().any(|part| matches!(
        part,
        IncomingContentPart::Url { url, title }
            if url == &missing_url && title.as_deref() == Some("missing.pdf")
    )));
    server.abort();
}

#[test]
fn build_request_rejects_duplicate_event_id() {
    let channel = slack_channel("http://localhost");
    let native = serde_json::json!({
        "event_id": "Ev-duplicate",
        "event": {
            "type": "message",
            "user": "U123",
            "channel": "D123",
            "channel_type": "im",
            "text": "hello",
            "ts": "1710000000.000300"
        }
    });

    channel.build_request_from_native(native.clone()).unwrap();
    let err = channel.build_request_from_native(native).unwrap_err();
    assert!(err.to_string().contains("duplicate slack event"));
}

#[tokio::test]
async fn send_posts_text_and_uploads_local_file() {
    let (api_base, state, server) = start_slack_api().await;
    let channel = slack_channel(&api_base);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("report.txt");
    std::fs::write(&path, "report").unwrap();
    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "C123".into(),
        sender_id: Some("U123".into()),
        reply_to: Some("1710000000.000000".into()),
        metadata: serde_json::json!({}),
    };
    let message = rua_channel::core::content::OutgoingMessage {
        channel_id: "slack".into(),
        conversation_id: "C123".into(),
        receiver_id: Some("U123".into()),
        content: vec![
            OutgoingContentPart::Text {
                text: "hello slack".into(),
            },
            OutgoingContentPart::File {
                url: None,
                path: Some(path.clone()),
                name: Some("report.txt".into()),
            },
        ],
        reply_to: Some("1710000000.000000".into()),
        metadata: serde_json::json!({}),
    };

    channel.send(target, message).await.unwrap();

    let calls = state.calls.lock().await;
    assert_eq!(calls[0]["method"], "postMessage");
    assert_eq!(calls[0]["body"]["channel"], "C123");
    assert_eq!(calls[0]["body"]["text"], "hello slack");
    assert_eq!(calls[0]["body"]["thread_ts"], "1710000000.000000");
    drop(calls);
    let uploads = state.uploads.lock().await;
    assert_eq!(uploads.len(), 1);
    assert!(uploads[0].contains("report.txt"));
    server.abort();
}

#[tokio::test]
async fn send_uploads_slack_host_remote_media_without_text_fallback() {
    let (api_base, state, server) = start_slack_api().await;
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::http(&api_base).unwrap())
        .build()
        .unwrap();
    let channel = slack_channel(&api_base).with_client(client);
    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "C123".into(),
        sender_id: Some("U123".into()),
        reply_to: None,
        metadata: serde_json::json!({}),
    };
    let message = rua_channel::core::content::OutgoingMessage {
        channel_id: "slack".into(),
        conversation_id: "C123".into(),
        receiver_id: Some("U123".into()),
        content: vec![OutgoingContentPart::File {
            url: Some("http://files.slack.com/files-pri/T123/report.pdf".into()),
            path: None,
            name: Some("report.pdf".into()),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(target, message).await.unwrap();

    let calls = state.calls.lock().await;
    assert!(
        calls.is_empty(),
        "uploadable Slack URL should not be sent as text fallback"
    );
    drop(calls);
    let uploads = state.uploads.lock().await;
    assert_eq!(uploads.len(), 1);
    assert!(uploads[0].contains("local-slack-pdf"));
    assert!(uploads[0].contains("report.pdf"));
    server.abort();
}

#[tokio::test]
async fn send_remote_non_slack_media_uses_visible_text_fallback() {
    let (api_base, state, server) = start_slack_api().await;
    let channel = slack_channel(&api_base);
    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "C123".into(),
        sender_id: Some("U123".into()),
        reply_to: None,
        metadata: serde_json::json!({}),
    };
    let message = rua_channel::core::content::OutgoingMessage {
        channel_id: "slack".into(),
        conversation_id: "C123".into(),
        receiver_id: Some("U123".into()),
        content: vec![OutgoingContentPart::File {
            url: Some("https://example.com/report.pdf".into()),
            path: None,
            name: Some("report.pdf".into()),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(target, message).await.unwrap();

    let calls = state.calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0]["body"]["text"],
        "[File: report.pdf] https://example.com/report.pdf"
    );
    drop(calls);
    let uploads = state.uploads.lock().await;
    assert!(uploads.is_empty());
    server.abort();
}

#[tokio::test]
async fn stream_handler_creates_placeholder_and_edits_final() {
    let (api_base, state, server) = start_slack_api().await;
    let handler = SlackStreamHandler::new(
        reqwest::Client::new(),
        api_base,
        "xoxb-token".into(),
        "C123".into(),
        Some("1710000000.000000".into()),
    );

    handler.on_start(StreamType::Message).await.unwrap();
    handler
        .on_delta(StreamType::Message, "partial")
        .await
        .unwrap();
    handler
        .on_delta(StreamType::Message, "partial update")
        .await
        .unwrap();
    handler
        .on_end(StreamType::Message, "partial final")
        .await
        .unwrap();

    let calls = state.calls.lock().await;
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0]["method"], "postMessage");
    assert_eq!(calls[0]["body"]["text"], "partial");
    assert_eq!(calls[1]["method"], "update");
    assert_eq!(calls[1]["body"]["ts"], "1710000000.000100");
    assert_eq!(calls[1]["body"]["text"], "partial update");
    assert_eq!(calls[2]["method"], "update");
    assert_eq!(calls[2]["body"]["ts"], "1710000000.000100");
    assert_eq!(calls[2]["body"]["text"], "partial final");
    server.abort();
}

#[tokio::test]
async fn fetch_thread_context_reads_bounded_slack_replies() {
    let (api_base, _state, server) = start_slack_api().await;
    let channel = slack_channel(&api_base);

    let context = channel
        .fetch_thread_context("C123", "1710000000.000000", 2)
        .await
        .unwrap();

    assert_eq!(context, vec!["root".to_string(), "reply 1".to_string()]);
    server.abort();
}
