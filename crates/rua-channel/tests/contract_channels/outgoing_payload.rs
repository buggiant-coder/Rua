use std::sync::Arc;

use rua_channel::core::channel::Channel;
use rua_channel::core::content::{ChannelTarget, OutgoingContentPart, OutgoingMessage};
use tokio::sync::Mutex;

async fn start_json_post_sink(
    path: &'static str,
) -> (
    String,
    Arc<Mutex<Vec<serde_json::Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = Arc::clone(&received);

    let app = axum::Router::new().route(
        path,
        axum::routing::post(
            move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                let store = Arc::clone(&received_clone);
                async move {
                    store.lock().await.push(body);
                    axum::http::StatusCode::OK
                }
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind json sink");
    let addr = listener.local_addr().expect("json sink local addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("json sink serve");
    });
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    (base_url, received, server)
}

fn text_reply(channel_id: &str, conversation_id: &str, text: &str) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: channel_id.into(),
        conversation_id: conversation_id.into(),
        receiver_id: Some("user-1".into()),
        content: vec![OutgoingContentPart::Text { text: text.into() }],
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

fn mixed_media_reply(channel_id: &str, conversation_id: &str) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: channel_id.into(),
        conversation_id: conversation_id.into(),
        receiver_id: Some("user-1".into()),
        content: vec![
            OutgoingContentPart::Text {
                text: "summary".into(),
            },
            OutgoingContentPart::Image {
                url: Some("https://example.invalid/diagram.png".into()),
                path: None,
                alt: Some("architecture diagram".into()),
            },
            OutgoingContentPart::File {
                url: None,
                path: None,
                name: Some("report.pdf".into()),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

#[cfg(feature = "dingtalk")]
#[test]
fn dingtalk_webhook_text_and_markdown_body_shapes_are_stable() {
    let text = rua_channel::dingtalk::sender::DingTalkSender::build_text_body("hello");
    assert_eq!(
        text,
        serde_json::json!({
            "msgtype": "text",
            "text": {
                "content": "hello"
            }
        })
    );

    let markdown =
        rua_channel::dingtalk::sender::DingTalkSender::build_markdown_body("Reply", "**bold**");
    assert_eq!(
        markdown,
        serde_json::json!({
            "msgtype": "markdown",
            "markdown": {
                "title": "Reply",
                "text": "**bold**"
            }
        })
    );

    let image = rua_channel::dingtalk::sender::DingTalkSender::build_markdown_image_body(
        "Image", "preview", "@img",
    );
    assert_eq!(image["msgtype"], "markdown");
    assert_eq!(image["markdown"]["text"], "![preview](@img)");

    let file = rua_channel::dingtalk::sender::DingTalkSender::build_file_body("@file");
    assert_eq!(file["msgtype"], "file");
    assert_eq!(file["file"]["media_id"], "@file");

    let video = rua_channel::dingtalk::sender::DingTalkSender::build_video_body(
        "@video", "mp4", 10, "@cover",
    );
    assert_eq!(video["msgtype"], "video");
    assert_eq!(video["video"]["videoMediaId"], "@video");
    assert_eq!(video["video"]["videoType"], "mp4");
    assert_eq!(video["video"]["duration"], 10);
    assert_eq!(video["video"]["picMediaId"], "@cover");
}

#[cfg(feature = "dingtalk")]
#[test]
fn dingtalk_ai_card_group_payload_uses_group_space_and_at_user_fields() {
    let create = rua_channel::dingtalk::ai_card::build_create_card_body(
        "tpl-id", "track-1", "content", "staff-1", true,
    );
    assert_eq!(create["cardTemplateId"], "tpl-id");
    assert_eq!(create["outTrackId"], "track-1");
    assert_eq!(create["callbackType"], "STREAM");
    assert_eq!(create["cardData"]["cardParamMap"]["content"], "");
    assert_eq!(create["imGroupOpenSpaceModel"]["supportForward"], true);
    assert_eq!(create["imRobotOpenSpaceModel"]["supportForward"], true);
    assert_eq!(create["cardAtUserIds"], serde_json::json!(["staff-1"]));
    assert_eq!(create["userIdType"], 1);

    let deliver = rua_channel::dingtalk::ai_card::build_deliver_card_body(
        "track-1",
        "robot-code",
        "conversation-1",
        "staff-1",
        true,
    );
    assert_eq!(deliver["outTrackId"], "track-1");
    assert_eq!(deliver["openSpaceId"], "dtv1.card//IM_GROUP.conversation-1");
    assert_eq!(
        deliver["imGroupOpenDeliverModel"]["robotCode"],
        "robot-code"
    );
    assert!(deliver.get("imRobotOpenDeliverModel").is_none());
}

#[cfg(feature = "dingtalk")]
#[test]
fn dingtalk_ai_card_p2p_payload_uses_robot_space_without_at_user() {
    let create = rua_channel::dingtalk::ai_card::build_create_card_body(
        "tpl-id", "track-1", "content", "staff-1", false,
    );
    assert!(create.get("cardAtUserIds").is_none());
    assert!(create.get("userIdType").is_none());

    let deliver = rua_channel::dingtalk::ai_card::build_deliver_card_body(
        "track-1",
        "robot-code",
        "conversation-1",
        "staff-1",
        false,
    );
    assert_eq!(deliver["openSpaceId"], "dtv1.card//IM_ROBOT.staff-1");
    assert_eq!(deliver["imRobotOpenDeliverModel"]["spaceType"], "IM_ROBOT");
    assert!(deliver.get("imGroupOpenDeliverModel").is_none());
}

#[cfg(feature = "dingtalk")]
#[test]
fn dingtalk_ai_card_streaming_update_has_provider_camel_case_fields() {
    let update = rua_channel::dingtalk::ai_card::build_streaming_update_body(
        "track-1",
        "content",
        "partial answer",
        true,
    );

    assert_eq!(update["outTrackId"], "track-1");
    assert_eq!(update["key"], "content");
    assert_eq!(update["content"], "partial answer");
    assert_eq!(update["isFull"], true);
    assert_eq!(update["isFinalize"], true);
    assert_eq!(update["isError"], false);
    assert!(update["guid"]
        .as_str()
        .map(|value| !value.is_empty())
        .unwrap_or(false));
}

#[cfg(feature = "dingtalk")]
#[test]
fn dingtalk_renderer_turns_mixed_media_parts_into_stable_markdown() {
    use rua_channel::core::renderer::RenderStyle;
    use rua_channel::dingtalk::renderer::MessageRenderer;

    let renderer = MessageRenderer::new(RenderStyle::Markdown);
    let payloads = renderer.render(&[
        OutgoingContentPart::Text {
            text: "summary".into(),
        },
        OutgoingContentPart::Image {
            url: Some("https://cdn.example.com/diagram.png".into()),
            path: None,
            alt: Some("architecture diagram".into()),
        },
        OutgoingContentPart::File {
            url: Some("https://cdn.example.com/report.pdf".into()),
            path: None,
            name: Some("report.pdf".into()),
        },
    ]);

    assert_eq!(payloads.len(), 1);
    let body = payloads[0].to_json();
    assert_eq!(body["msgtype"], "markdown");
    assert_eq!(
        body["markdown"]["text"],
        "summary\n![architecture diagram](https://cdn.example.com/diagram.png)\n[File: report.pdf]"
    );
}

#[cfg(feature = "dingtalk")]
#[tokio::test]
async fn dingtalk_send_posts_markdown_to_session_webhook() {
    let (base_url, received, server) = start_json_post_sink("/robot/sendBySession").await;
    let webhook_url = format!("{base_url}/robot/sendBySession");
    let channel = rua_channel::dingtalk::DingTalkChannel::new();

    channel
        .send(
            ChannelTarget {
                agent_id: "agent-1".into(),
                conversation_id: "conv-1".into(),
                sender_id: Some("user-1".into()),
                reply_to: None,
                metadata: serde_json::json!({
                    "session_webhook": webhook_url,
                }),
            },
            text_reply("dingtalk", "conv-1", "hello user"),
        )
        .await
        .expect("dingtalk send");

    let bodies = received.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["msgtype"], "markdown");
    assert_eq!(bodies[0]["markdown"]["title"], "Reply");
    assert_eq!(bodies[0]["markdown"]["text"], "hello user");

    server.abort();
}

#[cfg(feature = "dingtalk")]
#[tokio::test]
async fn dingtalk_send_uses_native_public_image_and_text_fallback_for_unresolved_file() {
    let (base_url, received, server) = start_json_post_sink("/robot/sendBySession").await;
    let webhook_url = format!("{base_url}/robot/sendBySession");
    let channel = rua_channel::dingtalk::DingTalkChannel::new();

    channel
        .send(
            ChannelTarget {
                agent_id: "agent-1".into(),
                conversation_id: "conv-1".into(),
                sender_id: Some("user-1".into()),
                reply_to: None,
                metadata: serde_json::json!({
                    "session_webhook": webhook_url,
                }),
            },
            mixed_media_reply("dingtalk", "conv-1"),
        )
        .await
        .expect("dingtalk media send");

    let bodies = received.lock().await;
    assert_eq!(
        bodies.len(),
        2,
        "unexpected DingTalk media payloads: {bodies:#?}"
    );
    assert_eq!(bodies[0]["msgtype"], "markdown");
    assert_eq!(bodies[0]["markdown"]["text"], "summary\n[File: report.pdf]");
    assert_eq!(bodies[1]["msgtype"], "image");
    assert_eq!(
        bodies[1]["image"]["picURL"],
        "https://example.invalid/diagram.png"
    );

    server.abort();
}

#[cfg(feature = "dingtalk")]
#[tokio::test]
async fn dingtalk_unresolved_media_is_rendered_once_and_sourced_fallback_is_not_text_deduplicated()
{
    let cases = [
        (
            "summary",
            vec![
                OutgoingContentPart::Image {
                    url: None,
                    path: None,
                    alt: Some("diagram".into()),
                },
                OutgoingContentPart::File {
                    url: None,
                    path: None,
                    name: Some("report.pdf".into()),
                },
                OutgoingContentPart::Audio {
                    url: None,
                    path: None,
                    name: Some("recording".into()),
                },
                OutgoingContentPart::Video {
                    url: None,
                    path: None,
                    name: Some("clip".into()),
                },
            ],
            vec!["summary\ndiagram\n[File: report.pdf]\n[Audio: recording]\n[Video: clip]"],
        ),
        (
            "[File: report.pdf]",
            vec![OutgoingContentPart::File {
                url: Some("https://example.invalid/report.pdf".into()),
                path: None,
                name: Some("report.pdf".into()),
            }],
            vec!["[File: report.pdf]", "[File: report.pdf]"],
        ),
    ];
    for (text, media, expected) in cases {
        let (base_url, received, server) = start_json_post_sink("/robot/sendBySession").await;
        let channel = rua_channel::dingtalk::DingTalkChannel::new();
        let mut message = text_reply("dingtalk", "conv-1", text);
        message.content.extend(media);
        channel.send(ChannelTarget {
            agent_id: "agent-1".into(),
            conversation_id: "conv-1".into(),
            sender_id: None,
            reply_to: None,
            metadata: serde_json::json!({"session_webhook": format!("{base_url}/robot/sendBySession")}),
        }, message).await.expect("send media");
        let bodies = received.lock().await;
        assert_eq!(bodies.len(), expected.len(), "{bodies:#?}");
        for (body, text) in bodies.iter().zip(expected) {
            assert_eq!(body["msgtype"], "markdown");
            assert_eq!(body["markdown"]["text"], text);
        }
        server.abort();
    }
}

#[cfg(feature = "feishu")]
#[test]
fn feishu_webhook_text_body_shape_is_stable_with_and_without_secret() {
    let without_secret = rua_channel::feishu::FeishuChannel::build_text_body("hello", None);
    assert_eq!(
        without_secret,
        serde_json::json!({
            "msg_type": "text",
            "content": {
                "text": "hello"
            }
        })
    );

    let with_secret =
        rua_channel::feishu::FeishuChannel::build_text_body("hello", Some("my-secret"));
    assert_eq!(with_secret["msg_type"], "text");
    assert_eq!(with_secret["content"]["text"], "hello");
    assert!(with_secret["timestamp"]
        .as_str()
        .and_then(|value| value.parse::<i64>().ok())
        .is_some());
    assert!(with_secret["sign"]
        .as_str()
        .map(|value| !value.is_empty())
        .unwrap_or(false));
}

#[cfg(feature = "feishu")]
#[tokio::test]
async fn feishu_send_posts_text_to_webhook() {
    let (base_url, received, server) = start_json_post_sink("/open-apis/bot/v2/hook/test").await;
    let channel = rua_channel::feishu::FeishuChannel::new(rua_channel::feishu::FeishuConfig {
        webhook_url: format!("{base_url}/open-apis/bot/v2/hook/test"),
        secret: None,
        ..Default::default()
    });

    channel
        .send(
            ChannelTarget {
                agent_id: "agent-1".into(),
                conversation_id: "oc_chat_1".into(),
                sender_id: Some("ou_user_1".into()),
                reply_to: None,
                metadata: serde_json::json!({}),
            },
            text_reply("feishu", "oc_chat_1", "hello feishu"),
        )
        .await
        .expect("feishu send");

    let bodies = received.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["msg_type"], "text");
    assert_eq!(bodies[0]["content"]["text"], "hello feishu");

    server.abort();
}

#[cfg(feature = "feishu")]
#[tokio::test]
async fn feishu_send_renders_media_parts_as_text_placeholders() {
    let (base_url, received, server) = start_json_post_sink("/open-apis/bot/v2/hook/test").await;
    let channel = rua_channel::feishu::FeishuChannel::new(rua_channel::feishu::FeishuConfig {
        webhook_url: format!("{base_url}/open-apis/bot/v2/hook/test"),
        secret: None,
        ..Default::default()
    });

    channel
        .send(
            ChannelTarget {
                agent_id: "agent-1".into(),
                conversation_id: "oc_chat_1".into(),
                sender_id: Some("ou_user_1".into()),
                reply_to: None,
                metadata: serde_json::json!({}),
            },
            mixed_media_reply("feishu", "oc_chat_1"),
        )
        .await
        .expect("feishu media placeholder send");

    let bodies = received.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["msg_type"], "text");
    assert_eq!(
        bodies[0]["content"]["text"],
        "summary\narchitecture diagram\n[File: report.pdf]"
    );

    server.abort();
}

#[cfg(feature = "wecom")]
#[test]
fn wecom_reply_stream_payload_shape_is_stable() {
    let body =
        rua_channel::wecom::client::build_reply_stream_body("stream-1", "partial answer", false);

    assert_eq!(
        body,
        serde_json::json!({
            "msgtype": "stream",
            "stream": {
                "id": "stream-1",
                "content": "partial answer",
                "finish": false
            }
        })
    );
}

#[cfg(feature = "discord")]
#[tokio::test]
async fn discord_webhook_send_posts_content_payload() {
    let (base_url, received, server) = start_json_post_sink("/discord-webhook").await;
    let channel = rua_channel::discord::DiscordChannel::new(rua_channel::discord::DiscordConfig {
        webhook_url: format!("{base_url}/discord-webhook"),
        bot_token: String::new(),
        ..Default::default()
    });

    channel
        .send(
            ChannelTarget {
                agent_id: "agent-1".into(),
                conversation_id: "channel-1".into(),
                sender_id: Some("user-1".into()),
                reply_to: None,
                metadata: serde_json::json!({}),
            },
            text_reply("discord", "channel-1", "hello discord"),
        )
        .await
        .expect("discord webhook send");

    let bodies = received.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0], serde_json::json!({"content": "hello discord"}));

    server.abort();
}

#[cfg(feature = "discord")]
#[tokio::test]
async fn discord_webhook_send_attempts_url_media_and_falls_back_visible_text() {
    let (base_url, received, server) = start_json_post_sink("/discord-webhook").await;
    let channel = rua_channel::discord::DiscordChannel::new(rua_channel::discord::DiscordConfig {
        webhook_url: format!("{base_url}/discord-webhook"),
        bot_token: String::new(),
        ..Default::default()
    });

    channel
        .send(
            ChannelTarget {
                agent_id: "agent-1".into(),
                conversation_id: "channel-1".into(),
                sender_id: Some("user-1".into()),
                reply_to: None,
                metadata: serde_json::json!({}),
            },
            mixed_media_reply("discord", "channel-1"),
        )
        .await
        .expect("discord media placeholder send");

    let bodies = received.lock().await;
    assert_eq!(bodies.len(), 2);
    assert_eq!(
        bodies[0],
        serde_json::json!({"content": "summary\n[File: report.pdf]"})
    );
    assert_eq!(
        bodies[1],
        serde_json::json!({"content": "architecture diagram"})
    );

    server.abort();
}

#[cfg(feature = "mattermost")]
#[tokio::test]
async fn mattermost_send_posts_channel_message_and_root_id() {
    let (base_url, received, server) = start_json_post_sink("/api/v4/posts").await;
    let channel = rua_channel::mattermost::MattermostChannel::new(
        rua_channel::mattermost::MattermostConfig {
            url: base_url,
            bot_token: "test-token".into(),
            ..Default::default()
        },
    );

    channel
        .send(
            ChannelTarget {
                agent_id: "agent-1".into(),
                conversation_id: "ch-123".into(),
                sender_id: Some("user-1".into()),
                reply_to: None,
                metadata: serde_json::json!({
                    "mm_channel_id": "ch-123",
                    "root_id": "root-abc",
                }),
            },
            text_reply("mattermost", "ch-123", "hello mattermost"),
        )
        .await
        .expect("mattermost send");

    let bodies = received.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["channel_id"], "ch-123");
    assert_eq!(bodies[0]["message"], "hello mattermost");
    assert_eq!(bodies[0]["root_id"], "root-abc");

    server.abort();
}

#[cfg(feature = "mattermost")]
#[tokio::test]
async fn mattermost_send_renders_media_parts_as_message_placeholders() {
    let (base_url, received, server) = start_json_post_sink("/api/v4/posts").await;
    let channel = rua_channel::mattermost::MattermostChannel::new(
        rua_channel::mattermost::MattermostConfig {
            url: base_url,
            bot_token: "test-token".into(),
            ..Default::default()
        },
    );

    channel
        .send(
            ChannelTarget {
                agent_id: "agent-1".into(),
                conversation_id: "ch-123".into(),
                sender_id: Some("user-1".into()),
                reply_to: None,
                metadata: serde_json::json!({
                    "mm_channel_id": "ch-123",
                }),
            },
            mixed_media_reply("mattermost", "ch-123"),
        )
        .await
        .expect("mattermost media send");

    let bodies = received.lock().await;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["channel_id"], "ch-123");
    assert_eq!(bodies[0]["message"], "summary\n[File: report.pdf]");

    server.abort();
}

#[cfg(feature = "yuanbao")]
#[test]
fn yuanbao_c2c_and_group_send_payload_headers_are_stable() {
    let body = vec![(
        "TIMTextElem".to_string(),
        serde_json::json!({"text": "hello"}),
    )];

    let (c2c_raw, c2c_msg_id) =
        rua_channel::yuanbao::codec::build_send_c2c_msg("target-user", "bot-account", &body);
    let c2c = rua_channel::yuanbao::codec::decode_conn_msg(&c2c_raw).expect("decode c2c");
    assert_eq!(c2c.head.cmd, "send_c2c_message");
    assert_eq!(c2c.head.module, "yuanbao_openclaw_proxy");
    assert_eq!(c2c.head.msg_id, c2c_msg_id);
    assert_eq!(c2c_msg_id.len(), 32);
    assert!(!c2c.data.is_empty());

    let (group_raw, group_msg_id) =
        rua_channel::yuanbao::codec::build_send_group_msg("group-code", "bot-account", &body);
    let group = rua_channel::yuanbao::codec::decode_conn_msg(&group_raw).expect("decode group");
    assert_eq!(group.head.cmd, "send_group_message");
    assert_eq!(group.head.module, "yuanbao_openclaw_proxy");
    assert_eq!(group.head.msg_id, group_msg_id);
    assert_eq!(group_msg_id.len(), 32);
    assert!(!group.data.is_empty());
}
