use axum::{
    extract::OriginalUri,
    routing::{any, post},
    Json, Router,
};
use rua_channel::{
    core::{channel::Channel, ChannelTarget, OutgoingContentPart, OutgoingMessage},
    dingtalk::DingTalkChannel,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn cron_cards_and_mentions_use_custom_endpoint_and_configured_layout() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let captured = sent.clone();
    let app = Router::new()
        .route(
            "/v1.0/oauth2/accessToken",
            post(|| async { Json(json!({"accessToken":"token","expireIn":7200})) }),
        )
        .route(
            "/{*path}",
            any(move |uri: OriginalUri, Json(body): Json<Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().await.push((uri.0.path().to_string(), body));
                    Json(json!({}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    for mention in [true, false] {
        let channel = DingTalkChannel::new()
            .with_ws_config("app".into(), "secret".into(), false)
            .with_media_dir(temp.path().to_string_lossy().into_owned())
            .with_robot_code("robot".into())
            .with_card_config(true, "template".into(), "content".into())
            .with_endpoint(base.clone())
            .with_channel_metadata(json!({
                "cron_message_type":"card","card_auto_layout":true,"at_sender_on_reply":mention
            }));
        let to = ChannelTarget {
            agent_id: String::new(),
            conversation_id: "cid-group".into(),
            sender_id: Some("staff".into()),
            reply_to: None,
            metadata: json!({"is_group":true,"sender_staff_id":"staff"}),
        };
        let msg = OutgoingMessage {
            channel_id: "dingtalk".into(),
            conversation_id: "cid-group".into(),
            receiver_id: None,
            content: vec![OutgoingContentPart::Text {
                text: "scheduled reply".into(),
            }],
            reply_to: None,
            metadata: json!({}),
        };
        let mut remembered = to.clone();
        let native = json!({"conversationId":"cid-group","conversationType":"2","senderId":"sender","senderStaffId":"staff",
            "msgId":format!("native-{mention}"),"msgtype":"text","text":{"content":"question"}});
        channel.build_request_from_native(native).unwrap();
        remembered.sender_id = Some("sender".into());
        remembered.metadata = Value::Null;
        channel.send(remembered, msg.clone()).await.unwrap();
        let calls = std::mem::take(&mut *sent.lock().await);
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].0, "/v1.0/card/instances");
        assert_eq!(
            calls[0].1["cardData"]["cardParamMap"]["config"],
            r#"{"autoLayout":true}"#
        );
        assert_eq!(calls[0].1.get("cardAtUserIds").is_some(), mention);
        assert_eq!(calls[1].0, "/v1.0/card/instances/deliver");
        assert_eq!(calls[1].1["openSpaceId"], "dtv1.card//IM_GROUP.cid-group");
        assert_eq!(calls[2].0, "/v1.0/card/streaming");
        assert_eq!(calls[2].1["content"], "scheduled reply");
        let mut markdown_to = to;
        markdown_to.metadata["cron_message_type"] = json!("markdown");
        markdown_to.metadata["session_webhook"] = json!(format!("{base}/webhook"));
        channel.send(markdown_to, msg).await.unwrap();
        let calls = std::mem::take(&mut *sent.lock().await);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "/webhook");
        assert_eq!(calls[0].1.get("at").is_some(), mention);
        if mention {
            assert_eq!(calls[0].1["at"]["atUserIds"][0], "staff");
        }
    }
    let restored = DingTalkChannel::new()
        .with_ws_config("app".into(), "secret".into(), false)
        .with_media_dir(temp.path().to_string_lossy().into_owned())
        .with_robot_code("robot".into())
        .with_endpoint(base.clone());
    restored
        .send(
            ChannelTarget {
                agent_id: String::new(),
                conversation_id: "dingtalk:group:cid-group:sender".into(),
                sender_id: Some("sender".into()),
                reply_to: None,
                metadata: Value::Null,
            },
            OutgoingMessage {
                channel_id: "dingtalk".into(),
                conversation_id: "cid-group".into(),
                receiver_id: None,
                content: vec![OutgoingContentPart::Text {
                    text: "scheduled markdown".into(),
                }],
                reply_to: None,
                metadata: json!({}),
            },
        )
        .await
        .unwrap();
    let calls = sent.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "/v1.0/robot/groupMessages/send");
    assert_eq!(calls[0].1["openConversationId"], "cid-group");
    assert_eq!(
        serde_json::from_str::<Value>(calls[0].1["msgParam"].as_str().unwrap()).unwrap()["text"],
        "scheduled markdown"
    );
    server.abort();
}

#[tokio::test]
async fn proactive_media_uses_robot_api_without_webhook_and_after_webhook_expiry() {
    use axum::{body::Bytes, extract::Query, http::HeaderMap};
    use std::collections::HashMap;
    let sent = Arc::new(Mutex::new(Vec::new()));
    let uploads = Arc::new(Mutex::new(Vec::new()));
    let captured = sent.clone();
    let uploaded = uploads.clone();
    let app = Router::new()
        .route(
            "/v1.0/oauth2/accessToken",
            post(|| async { Json(json!({"accessToken":"media-token","expireIn":7200})) }),
        )
        .route(
            "/media/upload",
            post(
                move |Query(query): Query<HashMap<String, String>>, body: Bytes| {
                    let uploaded = uploaded.clone();
                    async move {
                        assert_eq!(query["access_token"], "media-token");
                        uploaded.lock().await.push((
                            query["type"].clone(),
                            String::from_utf8_lossy(&body).into_owned(),
                        ));
                        Json(json!({"errcode":0,"media_id":"@uploaded"}))
                    }
                },
            ),
        )
        .route(
            "/expired",
            post(|| async {
                (
                    axum::http::StatusCode::OK,
                    Json(json!({"errcode":310000,"errmsg":"expired"})),
                )
            }),
        )
        .route(
            "/{*path}",
            any(
                move |uri: OriginalUri, headers: HeaderMap, Json(body): Json<Value>| {
                    let captured = captured.clone();
                    async move {
                        assert_eq!(headers["x-acs-dingtalk-access-token"], "media-token");
                        captured.lock().await.push((uri.0.path().to_owned(), body));
                        Json(json!({}))
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("report.pdf");
    {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(temp.path()).unwrap();
        file.write_all(b"report-bytes").unwrap();
        file.as_file().sync_all().unwrap();
        file.persist(&local).unwrap();
        std::fs::File::open(temp.path())
            .unwrap()
            .sync_all()
            .unwrap();
    }
    let channel = DingTalkChannel::new()
        .with_ws_config("app".into(), "secret".into(), false)
        .with_robot_code("robot".into())
        .with_endpoint(base.clone());
    for group in [true, false] {
        for expired in [false, true] {
            let mut metadata = json!({"is_group":group,"sender_staff_id":"staff-42"});
            if expired {
                metadata["session_webhook"] = json!(format!("{base}/expired"));
            }
            channel
                .send(
                    ChannelTarget {
                        agent_id: String::new(),
                        conversation_id: "cid-group".into(),
                        sender_id: Some("sender".into()),
                        reply_to: None,
                        metadata,
                    },
                    OutgoingMessage {
                        channel_id: "dingtalk".into(),
                        conversation_id: "cid-group".into(),
                        receiver_id: None,
                        reply_to: None,
                        metadata: json!({}),
                        content: vec![
                            OutgoingContentPart::Image {
                                url: Some("https://example.test/chart.png".into()),
                                path: None,
                                alt: None,
                            },
                            OutgoingContentPart::File {
                                path: Some(local.clone()),
                                url: None,
                                name: Some("report.pdf".into()),
                            },
                            OutgoingContentPart::Audio {
                                path: Some(local.clone()),
                                url: None,
                                name: Some("voice.amr".into()),
                            },
                            OutgoingContentPart::Video {
                                path: Some(local.clone()),
                                url: None,
                                name: Some("clip.mp4".into()),
                            },
                        ],
                    },
                )
                .await
                .unwrap();
            let calls = std::mem::take(&mut *sent.lock().await);
            assert_eq!(calls.len(), 4);
            for (path, body) in &calls {
                assert_eq!(body["robotCode"], "robot");
                if group {
                    assert_eq!(path, "/v1.0/robot/groupMessages/send");
                    assert_eq!(body["openConversationId"], "cid-group");
                } else {
                    assert_eq!(path, "/v1.0/robot/oToMessages/batchSend");
                    assert_eq!(body["userIds"], json!(["staff-42"]));
                }
            }
            assert_eq!(calls[0].1["msgKey"], "sampleImageMsg");
            let param: Value =
                serde_json::from_str(calls[0].1["msgParam"].as_str().unwrap()).unwrap();
            assert_eq!(param["photoURL"], "https://example.test/chart.png");
            for ((_, body), (name, extension)) in calls[1..].iter().zip([
                ("report.pdf", "pdf"),
                ("voice.amr", "amr"),
                ("clip.mp4", "mp4"),
            ]) {
                assert_eq!(body["msgKey"], "sampleFile");
                let param: Value =
                    serde_json::from_str(body["msgParam"].as_str().unwrap()).unwrap();
                assert_eq!(
                    param,
                    json!({"mediaId":"@uploaded","fileName":name,"fileType":extension})
                );
            }
            let uploads = std::mem::take(&mut *uploads.lock().await);
            assert_eq!(uploads.len(), if expired { 6 } else { 3 });
            assert!(uploads
                .iter()
                .all(|(kind, body)| kind == "file" && body.contains("report-bytes")));
        }
    }
    server.abort();
}
