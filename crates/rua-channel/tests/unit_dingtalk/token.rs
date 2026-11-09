use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use rua_channel::dingtalk::AccessTokenManager;
use tokio::net::TcpListener;

fn mock_token_server(call_count: Arc<AtomicU32>, expire_in: u64) -> Router {
    Router::new().route(
        "/v1.0/oauth2/accessToken",
        post(move |_: Json<serde_json::Value>| {
            let count = call_count.clone();
            async move {
                let n = count.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({
                    "accessToken": format!("token-{n}"),
                    "expireIn": expire_in
                }))
            }
        }),
    )
}

async fn start_mock_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind token server");
    let addr = listener.local_addr().expect("token server addr");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve token server");
    });
    format!("http://127.0.0.1:{}/v1.0/oauth2/accessToken", addr.port())
}

#[tokio::test]
async fn token_expiry_triggers_refresh() {
    let call_count = Arc::new(AtomicU32::new(0));
    let url = start_mock_server(mock_token_server(call_count.clone(), 2)).await;
    let manager = AccessTokenManager::new("key".into(), "secret".into()).with_token_url(url);

    let t1 = manager.current_token().await.expect("initial token");
    assert_eq!(t1, "token-0");
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_secs(3)).await;

    let t2 = manager.current_token().await.expect("refreshed token");
    assert_eq!(t2, "token-1");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn refresh_failure_retries() {
    let call_count = Arc::new(AtomicU32::new(0));
    let cc = call_count.clone();
    let app = Router::new().route(
        "/v1.0/oauth2/accessToken",
        post(move |_: Json<serde_json::Value>| {
            let count = cc.clone();
            async move {
                let n = count.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
                } else {
                    Json(serde_json::json!({
                        "accessToken": "token-after-retry",
                        "expireIn": 7200
                    }))
                    .into_response()
                }
            }
        }),
    );
    let url = start_mock_server(app).await;
    let manager = AccessTokenManager::new("key".into(), "secret".into()).with_token_url(url);

    manager.start().await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let token = manager.current_token().await.expect("token after retry");
    assert_eq!(token, "token-after-retry");
    assert!(call_count.load(Ordering::SeqCst) >= 2);

    manager.stop().await;
}
