use rua_channel::xiaoyi::channel::{build_artifact_msg, build_status_update_msg};
use rua_channel::xiaoyi::XiaoyiChannel;

fn channel() -> XiaoyiChannel {
    let config = rua_channel::xiaoyi::XiaoyiConfig {
        access_key: "ak".into(),
        secret_key: "sk".into(),
        agent_id: "agent-1".into(),
        ws_url: "wss://example.com/ws".into(),
        backup_ws_url: String::new(),
        media_dir: String::new(),
    };
    XiaoyiChannel::new(config)
}

#[test]
fn channel_id_is_xiaoyi() {
    use rua_channel::core::channel::Channel;
    let ch = channel();
    assert_eq!(ch.id(), "xiaoyi");
}

#[test]
fn build_request_from_message_stream() {
    use rua_channel::core::channel::Channel;
    let ch = channel();

    let native = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "msg-001",
        "method": "message/stream",
        "params": {
            "sessionId": "session-abc",
            "id": "task-xyz",
            "message": {
                "role": "user",
                "parts": [
                    {"kind": "text", "text": "Hello xiaoyi"}
                ]
            }
        }
    });

    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.channel_id, "xiaoyi");
    assert_eq!(req.agent_id, "agent-1");
    assert_eq!(req.conversation_id, "xiaoyi:session-abc");
    assert_eq!(req.user_id, "session-abc");
    assert_eq!(req.metadata["session_id"], "session-abc");
    assert_eq!(req.metadata["task_id"], "task-xyz");

    let text = req
        .input
        .iter()
        .find_map(|p| match p {
            rua_channel::core::content::IncomingContentPart::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(text, "Hello xiaoyi");
}

#[test]
fn build_request_from_native_rejects_duplicate_request_id() {
    use rua_channel::core::channel::Channel;
    let ch = channel();

    let native = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "msg-duplicate",
        "method": "message/stream",
        "params": {
            "sessionId": "session-abc",
            "id": "task-xyz",
            "message": {
                "role": "user",
                "parts": [
                    {"kind": "text", "text": "Hello xiaoyi"}
                ]
            }
        }
    });

    ch.build_request_from_native(native.clone()).unwrap();

    let err = ch.build_request_from_native(native).unwrap_err();
    assert!(err.to_string().contains("duplicate xiaoyi event"));
}

#[test]
fn build_request_accepts_top_level_session_and_request_id() {
    use rua_channel::core::channel::Channel;
    let ch = channel();

    let native = serde_json::json!({
        "agentId": "agent-1",
        "id": "request-top-level",
        "sessionId": "session-top-level",
        "method": "message/stream",
        "params": {
            "message": {
                "parts": [
                    {"kind": "text", "text": "top-level ids"}
                ]
            }
        }
    });

    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.agent_id, "agent-1");
    assert_eq!(req.conversation_id, "xiaoyi:session-top-level");
    assert_eq!(req.user_id, "session-top-level");
    assert_eq!(req.message_id.as_deref(), Some("request-top-level"));
    assert_eq!(req.metadata["session_id"], "session-top-level");
    assert_eq!(req.metadata["task_id"], "request-top-level");
}

#[test]
fn build_request_rejects_empty_text() {
    use rua_channel::core::channel::Channel;
    let ch = channel();

    let native = serde_json::json!({
        "method": "message/stream",
        "params": {
            "sessionId": "s1",
            "message": {
                "parts": [
                    {"kind": "text", "text": ""}
                ]
            }
        }
    });

    assert!(ch.build_request_from_native(native).is_err());
}

#[test]
fn build_request_rejects_mismatched_agent_id() {
    use rua_channel::core::channel::Channel;
    let ch = channel();

    let native = serde_json::json!({
        "agentId": "other-agent",
        "method": "message/stream",
        "params": {
            "sessionId": "s1",
            "id": "t1",
            "message": {
                "parts": [
                    {"kind": "text", "text": "hello"}
                ]
            }
        }
    });

    assert!(ch.build_request_from_native(native).is_err());
}

#[test]
fn build_request_handles_multiple_text_parts() {
    use rua_channel::core::channel::Channel;
    let ch = channel();

    let native = serde_json::json!({
        "method": "message/stream",
        "params": {
            "sessionId": "s2",
            "id": "t2",
            "message": {
                "parts": [
                    {"kind": "text", "text": "hello"},
                    {"kind": "text", "text": "world"}
                ]
            }
        }
    });

    let req = ch.build_request_from_native(native).unwrap();
    let text = req
        .input
        .iter()
        .find_map(|p| match p {
            rua_channel::core::content::IncomingContentPart::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(text, "hello world");
}

#[test]
fn build_request_accepts_local_file_parts() {
    use rua_channel::core::channel::Channel;
    let ch = channel();

    let native = serde_json::json!({
        "method": "message/stream",
        "params": {
            "sessionId": "session-media",
            "id": "task-media",
            "message": {
                "parts": [
                    {
                        "kind": "file",
                        "file": {
                            "uri": "https://example.com/image.png",
                            "name": "image.png",
                            "mimeType": "image/png",
                            "local_path": "/tmp/image.png"
                        }
                    },
                    {
                        "kind": "file",
                        "file": {
                            "uri": "https://example.com/report.pdf",
                            "name": "report.pdf",
                            "mimeType": "application/pdf",
                            "local_path": "/tmp/report.pdf"
                        }
                    }
                ]
            }
        }
    });

    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.input.len(), 2);
    match &req.input[0] {
        rua_channel::core::content::IncomingContentPart::Image {
            file_id,
            path,
            mime,
        } => {
            assert_eq!(file_id, "https://example.com/image.png");
            assert_eq!(path.to_string_lossy(), "/tmp/image.png");
            assert_eq!(mime.as_deref(), Some("image/png"));
        }
        other => panic!("expected image part, got {other:?}"),
    }
    match &req.input[1] {
        rua_channel::core::content::IncomingContentPart::File {
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
}

#[test]
fn build_request_rejects_missing_session_id() {
    use rua_channel::core::channel::Channel;
    let ch = channel();

    let native = serde_json::json!({
        "method": "message/stream",
        "params": {
            "id": "task-1",
            "message": {
                "parts": [
                    {"kind": "text", "text": "hello"}
                ]
            }
        }
    });

    assert!(ch.build_request_from_native(native).is_err());
}

#[test]
fn build_request_surfaces_file_download_failure_placeholder() {
    use rua_channel::core::channel::Channel;
    let ch = channel();

    let native = serde_json::json!({
        "method": "message/stream",
        "params": {
            "sessionId": "session-1",
            "id": "task-1",
            "message": {
                "parts": [
                    {"kind": "file", "file": {"uri": "https://example.com/photo.png", "name": "photo.png"}}
                ]
            }
        }
    });

    let req = ch.build_request_from_native(native).unwrap();
    let text = req
        .input
        .iter()
        .find_map(|part| match part {
            rua_channel::core::content::IncomingContentPart::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(text, "[photo.png: download failed]");
}

#[test]
fn build_request_uses_session_id_as_task_fallback() {
    use rua_channel::core::channel::Channel;
    let ch = channel();

    let native = serde_json::json!({
        "method": "message/stream",
        "params": {
            "sessionId": "session-fallback",
            "message": {
                "parts": [
                    {"kind": "text", "text": "hello"}
                ]
            }
        }
    });

    let req = ch.build_request_from_native(native).unwrap();
    assert_eq!(req.metadata["session_id"], "session-fallback");
    assert_eq!(req.metadata["task_id"], "session-fallback");
}

#[test]
fn chunk_text_no_split_under_limit() {
    let chunks = XiaoyiChannel::chunk_text("short text", 100);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], "short text");
}

#[test]
fn chunk_text_splits_at_limit() {
    let text = "a".repeat(10);
    let chunks = XiaoyiChannel::chunk_text(&text, 3);
    assert_eq!(chunks.len(), 4);
    for chunk in &chunks[..3] {
        assert_eq!(chunk.len(), 3);
    }
    assert_eq!(chunks[3].len(), 1);
}

#[test]
fn chunk_text_prefers_newline_boundaries() {
    let text = "alpha\nbravo\ncharlie";
    let chunks = XiaoyiChannel::chunk_text(text, 11);
    assert_eq!(
        chunks,
        vec!["alpha\nbravo".to_string(), "charlie".to_string()]
    );
}

#[test]
fn chunk_text_hard_splits_only_lines_over_limit() {
    let text = "head\nabcdefghij\ntail";
    let chunks = XiaoyiChannel::chunk_text(text, 5);
    assert_eq!(
        chunks,
        vec![
            "head".to_string(),
            "abcde".to_string(),
            "fghij".to_string(),
            "tail".to_string(),
        ]
    );
}

#[test]
fn chunk_text_respects_char_boundaries() {
    let text = "你好世界测试";
    let chunks = XiaoyiChannel::chunk_text(text, 7);
    assert!(chunks.len() >= 2);
    for chunk in &chunks {
        assert!(chunk.is_char_boundary(0));
    }
}

#[test]
fn chunk_text_handles_limit_smaller_than_one_multibyte_char() {
    let chunks = XiaoyiChannel::chunk_text("你好", 1);
    assert_eq!(chunks, vec!["你".to_string(), "好".to_string()]);
}

#[test]
fn chunk_text_zero_limit_returns_original_text() {
    let chunks = XiaoyiChannel::chunk_text("hello", 0);
    assert_eq!(chunks, vec!["hello".to_string()]);
}

#[test]
fn outgoing_media_parts_build_qwenpaw_compatible_file_artifacts() {
    use rua_channel::core::content::OutgoingContentPart;

    let image = OutgoingContentPart::Image {
        url: Some("https://cdn.example.com/chart.png".into()),
        path: None,
        alt: Some("chart.png".into()),
    };
    let video = OutgoingContentPart::Video {
        url: None,
        path: Some("/tmp/clip.mp4".into()),
        name: Some("clip.mp4".into()),
    };
    let file = OutgoingContentPart::File {
        url: Some("file:///tmp/report.pdf".into()),
        path: None,
        name: Some("report.pdf".into()),
    };

    assert_eq!(
        XiaoyiChannel::media_artifact_part(&image).unwrap(),
        serde_json::json!({
            "kind": "file",
            "file": {
                "name": "chart.png",
                "mimeType": "image/png",
                "uri": "https://cdn.example.com/chart.png"
            }
        })
    );
    assert_eq!(
        XiaoyiChannel::media_artifact_part(&video).unwrap(),
        serde_json::json!({
            "kind": "file",
            "file": {
                "name": "clip.mp4",
                "mimeType": "video/mp4",
                "uri": "/tmp/clip.mp4"
            }
        })
    );
    assert_eq!(
        XiaoyiChannel::media_artifact_part(&file).unwrap(),
        serde_json::json!({
            "kind": "file",
            "file": {
                "name": "report.pdf",
                "mimeType": "application/octet-stream",
                "uri": "file:///tmp/report.pdf"
            }
        })
    );
}

#[test]
fn outgoing_text_render_does_not_duplicate_resolved_media() {
    use rua_channel::core::content::OutgoingContentPart;

    let parts = vec![
        OutgoingContentPart::Text {
            text: "summary".into(),
        },
        OutgoingContentPart::Image {
            url: Some("https://cdn.example.com/chart.png".into()),
            path: None,
            alt: Some("chart.png".into()),
        },
        OutgoingContentPart::Audio {
            url: None,
            path: None,
            name: Some("voice.wav".into()),
        },
    ];

    assert_eq!(
        XiaoyiChannel::render_text_and_unresolved_media(&parts),
        "summary\n[Audio: voice.wav]",
        "XiaoYi media with URI is sent as A2A file artifact; unsupported audio remains visible fallback"
    );
}

#[test]
fn outbound_text_artifact_uses_qwenpaw_stream_chunk_shape() {
    let msg = build_artifact_msg(
        "agent-1",
        "session-1",
        "task-1",
        "message-1",
        vec![serde_json::json!({"kind": "text", "text": "hello"})],
        true,
        false,
    );
    assert_eq!(msg["msgType"], "agent_response");
    assert_eq!(msg["agentId"], "agent-1");
    assert_eq!(msg["sessionId"], "session-1");
    assert_eq!(msg["taskId"], "task-1");

    let detail: serde_json::Value =
        serde_json::from_str(msg["msgDetail"].as_str().expect("msgDetail JSON")).unwrap();
    assert_eq!(detail["jsonrpc"], "2.0");
    assert_eq!(detail["id"], "message-1");
    assert_eq!(detail["result"]["taskId"], "task-1");
    assert_eq!(detail["result"]["kind"], "artifact-update");
    assert_eq!(detail["result"]["append"], true);
    assert_eq!(detail["result"]["lastChunk"], true);
    assert_eq!(detail["result"]["final"], false);
    assert_eq!(detail["result"]["artifact"]["parts"][0]["kind"], "text");
    assert_eq!(detail["result"]["artifact"]["parts"][0]["text"], "hello");
}

#[test]
fn status_update_completed_precedes_final_artifact_for_stream_close() {
    let msg = build_status_update_msg("agent-1", "session-1", "task-1", "message-1");
    assert_eq!(msg["msgType"], "agent_response");
    assert_eq!(msg["agentId"], "agent-1");
    assert_eq!(msg["sessionId"], "session-1");
    assert_eq!(msg["taskId"], "task-1");

    let detail: serde_json::Value =
        serde_json::from_str(msg["msgDetail"].as_str().expect("msgDetail JSON")).unwrap();
    assert_eq!(detail["jsonrpc"], "2.0");
    assert_eq!(detail["id"], "message-1");
    assert_eq!(detail["result"]["taskId"], "task-1");
    assert_eq!(detail["result"]["kind"], "status-update");
    assert_eq!(detail["result"]["final"], false);
    assert_eq!(detail["result"]["status"]["state"], "completed");
    assert_eq!(detail["result"]["status"]["message"]["role"], "agent");
    assert_eq!(
        detail["result"]["status"]["message"]["parts"][0],
        serde_json::json!({"kind": "text", "text": ""})
    );
}

#[tokio::test]
async fn dual_ws_routes_response_back_to_session_server() {
    use async_trait::async_trait;
    use futures::stream::{self, BoxStream};
    use futures_util::{SinkExt, StreamExt};
    use rua_channel::core::channel::{Channel, Process};
    use rua_core::RuntimeEvent;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::{Mutex, Notify};
    use tokio_tungstenite::tungstenite::Message;

    #[derive(Clone)]
    struct EchoProcess;

    #[async_trait]
    impl Process for EchoProcess {
        async fn handle(
            &self,
            _req: rua_channel::core::channel::AgentRequest,
        ) -> BoxStream<'static, RuntimeEvent> {
            Box::pin(stream::iter(vec![
                RuntimeEvent::TextDelta {
                    delta: "reply from agent".into(),
                    message_id: Some("agent-msg-1".into()),
                },
                RuntimeEvent::End {
                    finish_reason: Some("stop".into()),
                },
            ]))
        }
    }

    #[derive(Clone, Default)]
    struct WsCapture {
        messages: Arc<Mutex<Vec<serde_json::Value>>>,
        notify: Arc<Notify>,
    }

    impl WsCapture {
        async fn push_text(&self, text: &str) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
                self.messages.lock().await.push(value);
                self.notify.notify_waiters();
            }
        }

        async fn has_agent_response(&self) -> bool {
            self.messages
                .lock()
                .await
                .iter()
                .any(|value| value["msgType"] == "agent_response")
        }
    }

    async fn spawn_server(
        request: Option<serde_json::Value>,
    ) -> (String, WsCapture, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind ws");
        let addr = listener.local_addr().expect("local addr");
        let capture = WsCapture::default();
        let capture_task = capture.clone();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept ws");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept websocket");
            if let Some(request) = request {
                ws.send(Message::Text(request.to_string().into()))
                    .await
                    .expect("send request");
            }
            while let Some(message) = ws.next().await {
                match message {
                    Ok(Message::Text(text)) => capture_task.push_text(&text).await,
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
        });
        (format!("ws://{}", addr), capture, handle)
    }

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "request-backup",
        "method": "message/stream",
        "params": {
            "sessionId": "session-backup",
            "id": "task-backup",
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "hello from backup"}]
            }
        }
    });
    let (primary_url, primary_capture, primary_task) = spawn_server(None).await;
    let (backup_url, backup_capture, backup_task) = spawn_server(Some(request)).await;

    let channel = XiaoyiChannel::new(rua_channel::xiaoyi::XiaoyiConfig {
        access_key: "ak".into(),
        secret_key: "sk".into(),
        agent_id: "agent-1".into(),
        ws_url: primary_url,
        backup_ws_url: backup_url,
        media_dir: String::new(),
    });
    channel
        .start(Arc::new(EchoProcess))
        .await
        .expect("start channel");

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if backup_capture.has_agent_response().await {
                break;
            }
            backup_capture.notify.notified().await;
        }
    })
    .await
    .expect("backup should receive agent response");

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !primary_capture.has_agent_response().await,
        "response for backup-originated session must not be routed to primary"
    );

    channel.stop().await.expect("stop channel");
    primary_task.abort();
    backup_task.abort();
}
