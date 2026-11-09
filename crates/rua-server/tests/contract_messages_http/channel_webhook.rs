use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generic_channel_webhook_stores_text_content_as_local_push_message() {
    let test = build_app();

    let webhook = request(
        &test.app,
        Method::POST,
        "/channel/dingtalk/webhook",
        Some(json!({
            "msgId": "dt-msg-001",
            "text": {
                "content": "hello from webhook"
            }
        })),
        None,
    )
    .await;
    assert_eq!(webhook.status(), StatusCode::OK);
    assert_eq!(
        read_json(webhook).await,
        json!({
            "status": "received",
            "channel": "dingtalk"
        })
    );

    let listed = request(
        &test.app,
        Method::GET,
        "/agents/default/console/push-messages",
        None,
        None,
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let body = read_json(listed).await;
    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1, "expected one webhook message: {body:?}");
    assert_eq!(messages[0]["id"], "dt-msg-001");
    assert_eq!(messages[0]["text"], "hello from webhook");
    assert_eq!(messages[0]["channel_id"], "dingtalk");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generic_channel_webhook_accepts_common_text_aliases_and_falls_back_to_json() {
    let test = build_app();

    let cases = [
        (
            "telegram",
            json!({"msg_id": "msg-field", "msg": "from msg"}),
            "msg-field",
            "from msg",
        ),
        (
            "discord",
            json!({"msg_id": "message-field", "message": "from message"}),
            "message-field",
            "from message",
        ),
        (
            "custom",
            json!({"msg_id": "fallback-field", "payload": {"nested": true}}),
            "fallback-field",
            "{\"msg_id\":\"fallback-field\",\"payload\":{\"nested\":true}}",
        ),
    ];

    for (channel, payload, expected_id, expected_text) in cases {
        let webhook = request(
            &test.app,
            Method::POST,
            &format!("/channel/{channel}/webhook"),
            Some(payload),
            None,
        )
        .await;
        assert_eq!(webhook.status(), StatusCode::OK);
        assert_eq!(read_json(webhook).await["channel"], channel);

        let listed = request(
            &test.app,
            Method::GET,
            "/agents/default/console/push-messages",
            None,
            None,
        )
        .await;
        assert_eq!(listed.status(), StatusCode::OK);
        let body = read_json(listed).await;
        let found = body["messages"]
            .as_array()
            .expect("messages array")
            .iter()
            .find(|message| message["id"] == expected_id)
            .unwrap_or_else(|| panic!("missing webhook message {expected_id}: {body:?}"));
        assert_eq!(found["text"], expected_text);
        assert_eq!(found["channel_id"], channel);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generic_channel_webhook_get_returns_fastapi_detail_error() {
    let test = build_app();

    let resp = request(
        &test.app,
        Method::GET,
        "/channel/telegram/webhook/path-token",
        None,
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        read_json(resp).await,
        json!({
            "detail": "Webhook endpoint only accepts POST requests",
            "channel": "telegram"
        })
    );
}
