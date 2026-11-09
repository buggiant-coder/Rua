use rua_channel::core::channel::Channel;
use rua_channel::core::content::OutgoingContentPart;
use rua_channel::core::ChannelTarget;
use rua_channel::qq::QqChannel;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct QqApiState {
    calls: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
}

async fn token_handler() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "access_token": "test-token",
        "expires_in": 3600
    }))
}

async fn qq_c2c_files_handler(
    axum::extract::Path(openid): axum::extract::Path<String>,
    axum::extract::State(state): axum::extract::State<QqApiState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    state
        .calls
        .lock()
        .await
        .push((format!("c2c-files:{openid}"), body));
    axum::Json(serde_json::json!({ "file_info": "file-info-123" }))
}

async fn qq_c2c_messages_handler(
    axum::extract::Path(openid): axum::extract::Path<String>,
    axum::extract::State(state): axum::extract::State<QqApiState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    state
        .calls
        .lock()
        .await
        .push((format!("c2c-messages:{openid}"), body));
    axum::Json(serde_json::json!({}))
}

async fn qq_group_files_handler(
    axum::extract::Path(openid): axum::extract::Path<String>,
    axum::extract::State(state): axum::extract::State<QqApiState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    state
        .calls
        .lock()
        .await
        .push((format!("group-files:{openid}"), body));
    axum::Json(serde_json::json!({ "file_info": "file-info-123" }))
}

async fn qq_group_messages_handler(
    axum::extract::Path(openid): axum::extract::Path<String>,
    axum::extract::State(state): axum::extract::State<QqApiState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    state
        .calls
        .lock()
        .await
        .push((format!("group-messages:{openid}"), body));
    axum::Json(serde_json::json!({}))
}

async fn qq_guild_messages_handler(
    axum::extract::Path(channel_id): axum::extract::Path<String>,
    axum::extract::State(state): axum::extract::State<QqApiState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::Json<serde_json::Value> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    state.calls.lock().await.push((
        format!("guild-messages:{channel_id}"),
        serde_json::json!({
            "content_type": content_type,
            "body": String::from_utf8_lossy(&body)
        }),
    ));
    axum::Json(serde_json::json!({}))
}

async fn qq_dm_messages_handler(
    axum::extract::Path(guild_id): axum::extract::Path<String>,
    axum::extract::State(state): axum::extract::State<QqApiState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::Json<serde_json::Value> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    state.calls.lock().await.push((
        format!("dm-messages:{guild_id}"),
        serde_json::json!({
            "content_type": content_type,
            "body": String::from_utf8_lossy(&body)
        }),
    ));
    axum::Json(serde_json::json!({}))
}

async fn start_qq_api_server(state: QqApiState) -> (tokio::task::JoinHandle<()>, String) {
    let app = axum::Router::new()
        .route("/token", axum::routing::post(token_handler))
        .route(
            "/v2/users/{openid}/files",
            axum::routing::post(qq_c2c_files_handler),
        )
        .route(
            "/v2/users/{openid}/messages",
            axum::routing::post(qq_c2c_messages_handler),
        )
        .route(
            "/v2/groups/{openid}/files",
            axum::routing::post(qq_group_files_handler),
        )
        .route(
            "/v2/groups/{openid}/messages",
            axum::routing::post(qq_group_messages_handler),
        )
        .route(
            "/channels/{channel_id}/messages",
            axum::routing::post(qq_guild_messages_handler),
        )
        .route(
            "/dms/{guild_id}/messages",
            axum::routing::post(qq_dm_messages_handler),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (server, base_url)
}

#[tokio::test]
async fn send_empty_content_skips_post() {
    let channel = QqChannel::new("app123".into(), "secret".into(), false);

    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "user1".into(),
        sender_id: Some("user1".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "c2c",
            "sender_id": "user1",
        }),
    };

    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "qq".into(),
        conversation_id: "user1".into(),
        receiver_id: Some("user1".into()),
        content: vec![OutgoingContentPart::Thinking {
            text: "internal only".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(to, msg).await.unwrap();
}

#[tokio::test]
async fn send_c2c_text_first_then_local_image_via_rich_media_upload() {
    let state = QqApiState::default();
    let (server, base_url) = start_qq_api_server(state.clone()).await;
    let channel = QqChannel::new_with_api_base_and_token_url(
        "app123".into(),
        "secret".into(),
        false,
        base_url.clone(),
        format!("{base_url}/token"),
    );
    let temp = tempdir().unwrap();
    let image_path = temp.path().join("chart.png");
    std::fs::write(&image_path, "png-bytes").unwrap();
    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "user1".into(),
        sender_id: Some("user1".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "c2c",
            "sender_id": "user-openid",
            "message_id": "msg-001",
        }),
    };
    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "qq".into(),
        conversation_id: "user1".into(),
        receiver_id: Some("user1".into()),
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

    channel.send(to, msg).await.unwrap();

    let calls = state.calls.lock().await;
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].0, "c2c-messages:user-openid");
    assert_eq!(calls[0].1["markdown"]["content"], "summary");
    assert_eq!(calls[0].1["msg_type"], 2);
    assert_eq!(calls[0].1["msg_id"], "msg-001");
    assert_eq!(calls[1].0, "c2c-files:user-openid");
    assert_eq!(calls[1].1["file_type"], 1);
    assert_eq!(calls[1].1["srv_send_msg"], false);
    assert_eq!(calls[1].1["file_name"], "chart.png");
    assert!(
        calls[1].1["file_data"].as_str().unwrap().len() > "png-bytes".len(),
        "local media should be base64 encoded for QQ rich-media upload"
    );
    assert_eq!(calls[2].0, "c2c-messages:user-openid");
    assert_eq!(calls[2].1["msg_type"], 7);
    assert_eq!(calls[2].1["media"]["file_info"], "file-info-123");
    assert_eq!(calls[2].1["content"], "chart.png");

    server.abort();
}

#[tokio::test]
async fn send_tool_guard_approval_uses_qq_markdown_keyboard() {
    let state = QqApiState::default();
    let (server, base_url) = start_qq_api_server(state.clone()).await;
    let channel = QqChannel::new_with_api_base_and_token_url(
        "app123".into(),
        "secret".into(),
        false,
        base_url.clone(),
        format!("{base_url}/token"),
    );
    let to = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "user-openid".into(),
        sender_id: Some("user-openid".into()),
        reply_to: Some("msg-001".into()),
        metadata: serde_json::json!({
            "message_type": "c2c",
            "sender_id": "user-openid",
            "message_id": "msg-001",
        }),
    };
    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "qq".into(),
        conversation_id: "user-openid".into(),
        receiver_id: Some("user-openid".into()),
        content: vec![OutgoingContentPart::Markdown {
            text: "Approve Bash?".into(),
        }],
        reply_to: Some("msg-001".into()),
        metadata: serde_json::json!({
            "message_type": "tool_guard_approval",
            "approval_request_id": "req-123456789",
            "tool_name": "Bash",
            "severity": "high",
        }),
    };

    channel.send(to, msg).await.unwrap();

    let calls = state.calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "c2c-messages:user-openid");
    assert_eq!(calls[0].1["msg_type"], 2);
    assert_eq!(calls[0].1["msg_id"], "msg-001");
    assert_eq!(calls[0].1["markdown"]["content"], "Approve Bash?");
    let buttons = &calls[0].1["keyboard"]["content"]["rows"][0]["buttons"];
    assert_eq!(buttons[0]["render_data"]["label"], "✅ Approve");
    assert_eq!(buttons[1]["render_data"]["label"], "🚫 Deny");
    let approve_data: serde_json::Value =
        serde_json::from_str(buttons[0]["action"]["data"].as_str().unwrap()).unwrap();
    assert_eq!(approve_data["p"], "tg_");
    assert_eq!(approve_data["a"], "approve");
    assert_eq!(approve_data["rid"], "req-123456789");
    assert_eq!(approve_data["tool"], "Bash");
    assert_eq!(approve_data["sev"], "high");
    assert_eq!(approve_data["sid"], "user-openid");
    assert_eq!(approve_data["sender"], "user-openid");
    assert_eq!(approve_data["mt"], "c2c");
    assert_eq!(approve_data["mid"], "msg-001");

    server.abort();
}

#[tokio::test]
async fn send_c2c_url_file_uses_qq_url_rich_media_upload() {
    let state = QqApiState::default();
    let (server, base_url) = start_qq_api_server(state.clone()).await;
    let channel = QqChannel::new_with_api_base_and_token_url(
        "app123".into(),
        "secret".into(),
        false,
        base_url.clone(),
        format!("{base_url}/token"),
    );
    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "user1".into(),
        sender_id: Some("user1".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "c2c",
            "sender_id": "user-openid",
        }),
    };
    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "qq".into(),
        conversation_id: "user1".into(),
        receiver_id: Some("user1".into()),
        content: vec![OutgoingContentPart::File {
            url: Some("https://cdn.example.com/report.pdf?token=1".into()),
            path: None,
            name: Some("report.pdf".into()),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(to, msg).await.unwrap();

    let calls = state.calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "c2c-files:user-openid");
    assert_eq!(calls[0].1["file_type"], 4);
    assert_eq!(
        calls[0].1["url"],
        "https://cdn.example.com/report.pdf?token=1"
    );
    assert!(calls[0].1.get("file_data").is_none());
    assert_eq!(calls[1].0, "c2c-messages:user-openid");
    assert_eq!(calls[1].1["msg_type"], 7);
    assert_eq!(calls[1].1["media"]["file_info"], "file-info-123");

    server.abort();
}

#[tokio::test]
async fn send_c2c_local_file_audio_and_video_use_rich_media_upload() {
    let state = QqApiState::default();
    let (server, base_url) = start_qq_api_server(state.clone()).await;
    let channel = QqChannel::new_with_api_base_and_token_url(
        "app123".into(),
        "secret".into(),
        false,
        base_url.clone(),
        format!("{base_url}/token"),
    );
    let temp = tempdir().unwrap();
    let file_path = temp.path().join("report.pdf");
    let voice_path = temp.path().join("voice.amr");
    let video_path = temp.path().join("clip.mp4");
    std::fs::write(&file_path, "pdf-bytes").unwrap();
    std::fs::write(&voice_path, "voice-bytes").unwrap();
    std::fs::write(&video_path, "video-bytes").unwrap();
    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "user1".into(),
        sender_id: Some("user1".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "c2c",
            "sender_id": "user-openid",
        }),
    };
    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "qq".into(),
        conversation_id: "user1".into(),
        receiver_id: Some("user1".into()),
        content: vec![
            OutgoingContentPart::File {
                url: None,
                path: Some(file_path),
                name: Some("report.pdf".into()),
            },
            OutgoingContentPart::Audio {
                url: None,
                path: Some(voice_path),
                name: Some("voice.amr".into()),
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

    channel.send(to, msg).await.unwrap();

    let calls = state.calls.lock().await;
    assert_eq!(calls.len(), 6);
    let uploads: Vec<_> = calls
        .iter()
        .filter(|(kind, _)| kind == "c2c-files:user-openid")
        .collect();
    assert_eq!(uploads.len(), 3);
    assert_eq!(uploads[0].1["file_type"], 4);
    assert_eq!(uploads[0].1["file_name"], "report.pdf");
    assert_eq!(uploads[1].1["file_type"], 3);
    assert_eq!(uploads[1].1["file_name"], "voice.amr");
    assert_eq!(uploads[2].1["file_type"], 2);
    assert_eq!(uploads[2].1["file_name"], "clip.mp4");
    assert!(uploads.iter().all(|(_, body)| body["file_data"]
        .as_str()
        .is_some_and(|value| !value.is_empty())));
    let sends = calls
        .iter()
        .filter(|(kind, _)| kind == "c2c-messages:user-openid")
        .count();
    assert_eq!(sends, 3);

    server.abort();
}

#[tokio::test]
async fn send_group_image_video_and_voice_use_group_rich_media_but_file_falls_back() {
    let state = QqApiState::default();
    let (server, base_url) = start_qq_api_server(state.clone()).await;
    let channel = QqChannel::new_with_api_base_and_token_url(
        "app123".into(),
        "secret".into(),
        false,
        base_url.clone(),
        format!("{base_url}/token"),
    );
    let temp = tempdir().unwrap();
    let image_path = temp.path().join("chart.png");
    let voice_path = temp.path().join("voice.amr");
    let video_path = temp.path().join("clip.mp4");
    let file_path = temp.path().join("report.pdf");
    std::fs::write(&image_path, "png-bytes").unwrap();
    std::fs::write(&voice_path, "voice-bytes").unwrap();
    std::fs::write(&video_path, "video-bytes").unwrap();
    std::fs::write(&file_path, "pdf-bytes").unwrap();
    let to = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "group:grp-openid".into(),
        sender_id: Some("member-openid".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "group",
            "sender_id": "member-openid",
            "group_openid": "grp-openid",
        }),
    };
    let msg = rua_channel::core::content::OutgoingMessage {
        channel_id: "qq".into(),
        conversation_id: "group:grp-openid".into(),
        receiver_id: None,
        content: vec![
            OutgoingContentPart::Image {
                url: None,
                path: Some(image_path),
                alt: Some("chart.png".into()),
            },
            OutgoingContentPart::Audio {
                url: None,
                path: Some(voice_path),
                name: Some("voice.amr".into()),
            },
            OutgoingContentPart::Video {
                url: None,
                path: Some(video_path),
                name: Some("clip.mp4".into()),
            },
            OutgoingContentPart::File {
                url: None,
                path: Some(file_path),
                name: Some("report.pdf".into()),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(to, msg).await.unwrap();

    let calls = state.calls.lock().await;
    let uploads: Vec<_> = calls
        .iter()
        .filter(|(kind, _)| kind == "group-files:grp-openid")
        .collect();
    assert_eq!(uploads.len(), 3);
    assert_eq!(uploads[0].1["file_type"], 1);
    assert_eq!(uploads[1].1["file_type"], 3);
    assert_eq!(uploads[2].1["file_type"], 2);
    assert!(uploads.iter().all(|(_, body)| body["file_type"] != 4));
    let group_sends = calls
        .iter()
        .filter(|(kind, _)| kind == "group-messages:grp-openid")
        .count();
    assert_eq!(group_sends, 3);

    server.abort();
}

#[tokio::test]
async fn send_guild_and_dm_images_use_native_image_or_file_image_payloads() {
    let state = QqApiState::default();
    let (server, base_url) = start_qq_api_server(state.clone()).await;
    let channel = QqChannel::new_with_api_base_and_token_url(
        "app123".into(),
        "secret".into(),
        false,
        base_url.clone(),
        format!("{base_url}/token"),
    );
    let temp = tempdir().unwrap();
    let local_image = temp.path().join("local.png");
    std::fs::write(&local_image, "png-bytes").unwrap();

    let guild_target = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "guild:channel-1".into(),
        sender_id: Some("user-openid".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "guild",
            "sender_id": "user-openid",
            "channel_id": "channel-1",
            "guild_id": "guild-1",
            "message_id": "msg-guild",
        }),
    };
    channel
        .send(
            guild_target,
            rua_channel::core::content::OutgoingMessage {
                channel_id: "qq".into(),
                conversation_id: "guild:channel-1".into(),
                receiver_id: None,
                content: vec![OutgoingContentPart::Image {
                    url: Some("https://cdn.example.com/image.png".into()),
                    path: None,
                    alt: Some("image.png".into()),
                }],
                reply_to: None,
                metadata: serde_json::json!({}),
            },
        )
        .await
        .unwrap();

    let dm_target = ChannelTarget {
        agent_id: String::new(),
        conversation_id: "dm:guild-1".into(),
        sender_id: Some("user-openid".into()),
        reply_to: None,
        metadata: serde_json::json!({
            "message_type": "dm",
            "sender_id": "user-openid",
            "guild_id": "guild-1",
            "message_id": "msg-dm",
        }),
    };
    channel
        .send(
            dm_target,
            rua_channel::core::content::OutgoingMessage {
                channel_id: "qq".into(),
                conversation_id: "dm:guild-1".into(),
                receiver_id: None,
                content: vec![OutgoingContentPart::Image {
                    url: None,
                    path: Some(local_image),
                    alt: Some("local.png".into()),
                }],
                reply_to: None,
                metadata: serde_json::json!({}),
            },
        )
        .await
        .unwrap();

    let calls = state.calls.lock().await;
    let guild_call = calls
        .iter()
        .find(|(kind, _)| kind == "guild-messages:channel-1")
        .expect("guild image send");
    assert!(guild_call.1["content_type"]
        .as_str()
        .unwrap()
        .contains("json"));
    let guild_body: serde_json::Value =
        serde_json::from_str(guild_call.1["body"].as_str().unwrap()).unwrap();
    assert_eq!(guild_body["image"], "https://cdn.example.com/image.png");
    assert_eq!(guild_body["msg_id"], "msg-guild");
    let dm_call = calls
        .iter()
        .find(|(kind, _)| kind == "dm-messages:guild-1")
        .expect("dm image send");
    assert!(dm_call.1["content_type"]
        .as_str()
        .unwrap()
        .contains("multipart/form-data"));
    assert!(dm_call.1["body"].as_str().unwrap().contains("png-bytes"));
    assert!(dm_call.1["body"].as_str().unwrap().contains("msg-dm"));

    server.abort();
}

struct GatewayReply(tokio::sync::mpsc::UnboundedSender<rua_channel::core::AgentChannelRequest>);

#[async_trait::async_trait]
impl rua_channel::core::channel::Process for GatewayReply {
    async fn handle(
        &self,
        request: rua_channel::core::AgentChannelRequest,
    ) -> futures::stream::BoxStream<'static, rua_core::RuntimeEvent> {
        self.0.send(request).unwrap();
        Box::pin(futures::stream::iter(vec![
            rua_core::RuntimeEvent::TextDelta {
                delta: "answer".into(),
                message_id: None,
            },
            rua_core::RuntimeEvent::End {
                finish_reason: None,
            },
        ]))
    }
}

#[tokio::test]
async fn gateway_group_media_ack_acl_and_markdown_follow_config() {
    use rua_channel::core::content::IncomingContentPart;
    let state = QqApiState::default();
    let (server, base) = start_qq_api_server(state.clone()).await;
    let temp = tempdir().unwrap();
    let image = temp.path().join("inbound.png");
    std::fs::write(&image, b"png-bytes").unwrap();
    for markdown in [true, false] {
        let channel = Arc::new(
            QqChannel::new_with_api_base_and_token_url(
                "app123".into(),
                "secret".into(),
                false,
                base.clone(),
                format!("{base}/token"),
            )
            .with_acl_config(serde_json::json!({
                "ack_message":"received","markdown_enabled":markdown,"no_text_debounce":true,
                "group_policy":"denylist","deny_from":["blocked"],"deny_message":""
            })),
        );
        let (received, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let process = Arc::new(GatewayReply(received));
        let event = serde_json::json!({
            "id":format!("m-{markdown}"),"author":{"member_openid":"member-1"},
            "group_openid":"group-1","content":"question",
            "attachments":[{"url":"https://cdn.invalid/image.png","local_path":image,"content_type":"image/png"}]
        });
        channel
            .handle_gateway_message(process.clone(), "GROUP_AT_MESSAGE_CREATE", event.clone())
            .await
            .unwrap();
        let request = requests.recv().await.unwrap();
        assert_eq!(request.user_id, "member-1");
        assert_eq!(request.metadata["group_openid"], "group-1");
        assert!(request
            .input
            .iter()
            .any(|p| matches!(p, IncomingContentPart::Image {path,..} if path==&image)));
        let calls = std::mem::take(&mut *state.calls.lock().await);
        assert_eq!(calls.len(), 2);
        assert!(calls
            .iter()
            .all(|(route, _)| route == "group-messages:group-1"));
        assert_eq!(calls[0].1["content"], "received");
        assert_eq!(calls[0].1["msg_type"], 0);
        let reply = &calls[1].1;
        if markdown {
            assert_eq!(reply["markdown"]["content"], "answer");
            assert_eq!(reply["msg_type"], 2);
        } else {
            assert_eq!(reply["content"], "answer");
            assert_eq!(reply["msg_type"], 0);
        }
        let mut denied = event;
        denied["id"] = serde_json::json!("denied");
        denied["author"]["member_openid"] = serde_json::json!("blocked");
        channel
            .handle_gateway_message(process, "GROUP_AT_MESSAGE_CREATE", denied)
            .await
            .unwrap();
        assert!(requests.try_recv().is_err());
        assert!(
            state.calls.lock().await.is_empty(),
            "denied sender must not get an ACK"
        );
    }
    server.abort();
}
