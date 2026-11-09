use std::sync::Arc;

use axum::extract::Json;
use rua_channel::core::channel::Channel;
use rua_channel::core::content::{ChannelTarget, OutgoingContentPart, OutgoingMessage};
use rua_channel::feishu::api;
use rua_channel::feishu::{FeishuChannel, FeishuConfig};
use tokio::sync::Mutex;

#[test]
fn resolve_receive_id_group_returns_chat_id() {
    let metadata = serde_json::json!({
        "feishu_chat_type": "group",
        "feishu_chat_id": "oc_group_123",
        "feishu_sender_id": "ou_user_456",
    });
    let (rid_type, rid) = api::resolve_receive_id(&metadata).unwrap();
    assert_eq!(rid_type, "chat_id");
    assert_eq!(rid, "oc_group_123");
}

#[test]
fn resolve_receive_id_p2p_returns_open_id() {
    let metadata = serde_json::json!({
        "feishu_chat_type": "p2p",
        "feishu_chat_id": "oc_chat_789",
        "feishu_sender_id": "ou_user_456",
    });
    let (rid_type, rid) = api::resolve_receive_id(&metadata).unwrap();
    assert_eq!(rid_type, "open_id");
    assert_eq!(rid, "ou_user_456");
}

#[test]
fn resolve_receive_id_empty_metadata_returns_none() {
    let metadata = serde_json::json!({});
    assert!(api::resolve_receive_id(&metadata).is_none());
}

#[test]
fn resolve_receive_id_group_empty_chat_id_falls_to_open_id() {
    let metadata = serde_json::json!({
        "feishu_chat_type": "group",
        "feishu_chat_id": "",
        "feishu_sender_id": "ou_user_456",
    });
    let (rid_type, rid) = api::resolve_receive_id(&metadata).unwrap();
    assert_eq!(rid_type, "open_id");
    assert_eq!(rid, "ou_user_456");
}

#[test]
fn resolve_receive_id_missing_sender_returns_none() {
    let metadata = serde_json::json!({
        "feishu_chat_type": "p2p",
        "feishu_chat_id": "oc_chat_789",
    });
    assert!(api::resolve_receive_id(&metadata).is_none());
}

#[test]
fn im_v1_body_content_is_json_string() {
    let text = "hello feishu";
    let content_json = serde_json::json!({"text": text}).to_string();
    let body = serde_json::json!({
        "receive_id": "oc_group_123",
        "msg_type": "text",
        "content": content_json,
    });

    assert!(body["content"].is_string());
    let content_str = body["content"].as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(content_str).unwrap();
    assert_eq!(parsed["text"], "hello feishu");

    assert_eq!(body["receive_id"], "oc_group_123");
    assert_eq!(body["msg_type"], "text");
}

#[test]
fn token_request_body_has_app_id_and_secret() {
    let body = serde_json::json!({
        "app_id": "cli_test_app",
        "app_secret": "secret_123",
    });
    assert_eq!(body["app_id"], "cli_test_app");
    assert_eq!(body["app_secret"], "secret_123");

    assert_eq!(body.as_object().unwrap().len(), 2);
}

#[tokio::test]
async fn send_without_app_credentials_uses_webhook() {
    let webhook_received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let webhook_received_clone = webhook_received.clone();

    let app = axum::Router::new().route(
        "/hook/webhook",
        axum::routing::post(move |Json(body): Json<serde_json::Value>| {
            let store = webhook_received_clone.clone();
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
        webhook_url: format!("http://127.0.0.1:{}/hook/webhook", addr.port()),
        secret: None,
        ..Default::default()
    };
    let ch = FeishuChannel::new(config);

    let target = ChannelTarget {
        agent_id: "agent-1".into(),
        conversation_id: "conv-1".into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({
            "feishu_chat_type": "group",
            "feishu_chat_id": "oc_group_123",
            "feishu_sender_id": "ou_user_456",
        }),
    };
    let msg = OutgoingMessage {
        channel_id: "feishu".into(),
        conversation_id: "conv-1".into(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text {
            text: "webhook fallback".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    ch.send(target, msg).await.unwrap();

    let wh_msgs = webhook_received.lock().await;
    assert_eq!(wh_msgs.len(), 1);
    assert_eq!(wh_msgs[0]["content"]["text"], "webhook fallback");
    assert_eq!(wh_msgs[0]["msg_type"], "text");

    server.abort();
}

#[tokio::test]
async fn send_im_v1_against_mock_server() {
    let received = Arc::new(Mutex::new(Vec::<(String, serde_json::Value)>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/open-apis/im/v1/messages",
        axum::routing::post(
            move |axum::extract::Query(params): axum::extract::Query<
                std::collections::HashMap<String, String>,
            >,
                  headers: axum::http::HeaderMap,
                  Json(body): Json<serde_json::Value>| {
                let store = received_clone.clone();
                async move {
                    let auth = headers
                        .get("Authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let rid_type = params.get("receive_id_type").cloned().unwrap_or_default();
                    let mut entry = body;
                    entry["_receive_id_type"] = serde_json::json!(rid_type);
                    entry["_auth"] = serde_json::json!(auth);
                    store.lock().await.push((auth, entry));
                    axum::Json(serde_json::json!({"code": 0, "msg": "success"}))
                }
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", addr.port());

    let content_json = serde_json::json!({"text": "hello im test"}).to_string();
    let body = serde_json::json!({
        "receive_id": "oc_group_123",
        "msg_type": "text",
        "content": content_json,
    });
    let url = format!("{base}/open-apis/im/v1/messages?receive_id_type=chat_id");

    let resp = client
        .post(&url)
        .header("Authorization", "Bearer t-mock-token")
        .header("Content-Type", "application/json; charset=utf-8")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let msgs = received.lock().await;
    assert_eq!(msgs.len(), 1);
    let (auth, entry) = &msgs[0];
    assert_eq!(auth, "Bearer t-mock-token");
    assert_eq!(entry["receive_id"], "oc_group_123");
    assert_eq!(entry["msg_type"], "text");
    assert_eq!(entry["_receive_id_type"], "chat_id");

    let content_str = entry["content"].as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(content_str).unwrap();
    assert_eq!(parsed["text"], "hello im test");

    server.abort();
}
