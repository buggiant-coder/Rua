use std::sync::Arc;

use axum::extract::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use rua_channel::dingtalk::emotion::{
    build_emotion_payload, send_emotion_with_urls, EmotionEndpointUrls,
};
use rua_channel::dingtalk::AccessTokenManager;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[derive(Debug)]
struct CapturedEmotion {
    endpoint: &'static str,
    token: String,
    body: serde_json::Value,
}

async fn start_mock_server(
    received: Arc<Mutex<Vec<CapturedEmotion>>>,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new()
        .route(
            "/v1.0/oauth2/accessToken",
            post(|| async {
                Json(serde_json::json!({
                    "accessToken": "token-abc",
                    "expireIn": 7200
                }))
            }),
        )
        .route(
            "/v1.0/robot/emotion/reply",
            post({
                let received = received.clone();
                move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                    let received = received.clone();
                    async move {
                        record_emotion("reply", headers, body, received).await;
                        StatusCode::OK
                    }
                }
            }),
        )
        .route(
            "/v1.0/robot/emotion/recall",
            post({
                let received = received.clone();
                move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                    let received = received.clone();
                    async move {
                        record_emotion("recall", headers, body, received).await;
                        StatusCode::OK
                    }
                }
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind dingtalk emotion server");
    let addr = listener.local_addr().expect("dingtalk emotion addr");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve dingtalk emotion server");
    });

    (format!("http://127.0.0.1:{}", addr.port()), server)
}

async fn record_emotion(
    endpoint: &'static str,
    headers: HeaderMap,
    body: serde_json::Value,
    received: Arc<Mutex<Vec<CapturedEmotion>>>,
) {
    let token = headers
        .get("x-acs-dingtalk-access-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    received.lock().await.push(CapturedEmotion {
        endpoint,
        token,
        body,
    });
}

#[test]
fn emotion_payload_uses_dingtalk_provider_shape() {
    let json = build_emotion_payload("bot-123", "msg-456", "conv-789", "Thinking");

    assert_eq!(json["robotCode"], "bot-123");
    assert_eq!(json["openMsgId"], "msg-456");
    assert_eq!(json["openConversationId"], "conv-789");
    assert_eq!(json["emotionType"], 2);
    assert_eq!(json["emotionName"], "Thinking");
    assert_eq!(json["textEmotion"]["emotionId"], "2659900");
    assert_eq!(json["textEmotion"]["emotionName"], "Thinking");
    assert_eq!(json["textEmotion"]["text"], "Thinking");
    assert_eq!(json["textEmotion"]["backgroundId"], "im_bg_1");
}

#[tokio::test]
async fn send_emotion_posts_reply_and_recall_with_access_token_header() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let (base_url, server) = start_mock_server(received.clone()).await;
    let token_url = format!("{base_url}/v1.0/oauth2/accessToken");
    let reply_url = format!("{base_url}/v1.0/robot/emotion/reply");
    let recall_url = format!("{base_url}/v1.0/robot/emotion/recall");
    let token_manager =
        AccessTokenManager::new("app-key".into(), "app-secret".into()).with_token_url(token_url);

    send_emotion_with_urls(
        &token_manager,
        "bot-rc",
        "msg-001",
        "conv-001",
        "Thinking",
        false,
        EmotionEndpointUrls {
            reply: &reply_url,
            recall: &recall_url,
        },
    )
    .await;
    send_emotion_with_urls(
        &token_manager,
        "bot-rc",
        "msg-001",
        "conv-001",
        "Thinking",
        true,
        EmotionEndpointUrls {
            reply: &reply_url,
            recall: &recall_url,
        },
    )
    .await;

    let messages = received.lock().await;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].endpoint, "reply");
    assert_eq!(messages[0].token, "token-abc");
    assert_eq!(messages[0].body["robotCode"], "bot-rc");
    assert_eq!(messages[0].body["openMsgId"], "msg-001");
    assert_eq!(messages[1].endpoint, "recall");
    assert_eq!(messages[1].token, "token-abc");
    assert_eq!(messages[1].body["openConversationId"], "conv-001");

    server.abort();
}
