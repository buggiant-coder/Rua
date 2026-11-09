use super::support::{recv, reply_process, wait_for_len};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        WebSocketUpgrade,
    },
    routing::{get, post},
    Json, Router,
};
use rua_channel::{
    core::channel::Channel,
    mattermost::{MattermostChannel, MattermostConfig},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, Mutex};

fn event(id: &str, root: &str, user: &str, text: &str) -> Message {
    Message::Text(
        json!({"event":"posted","data":{"channel_type":"O","post":json!({
        "id":id,"root_id":root,"channel_id":"channel-1","user_id":user,"message":text
    }).to_string()}})
        .to_string()
        .into(),
    )
}

#[tokio::test]
async fn follows_only_participated_threads_after_acl_admission_and_forgets_on_restart() {
    let (sockets, mut connections) = mpsc::unbounded_channel::<WebSocket>();
    let replies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let saved = replies.clone();
    let app = Router::new()
        .route(
            "/api/v4/users/me",
            get(|| async { Json(json!({"id":"bot","username":"rua"})) }),
        )
        .route(
            "/api/v4/websocket",
            get(move |ws: WebSocketUpgrade| {
                let sockets = sockets.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        sockets.send(socket).unwrap();
                    })
                }
            }),
        )
        .route(
            "/api/v4/posts",
            post(move |Json(body): Json<Value>| {
                let saved = saved.clone();
                async move {
                    saved.lock().await.push(body);
                    Json(json!({"id":"reply"}))
                }
            }),
        )
        .route(
            "/api/v4/posts/{id}/thread",
            get(|| async { Json(json!({"order":[],"posts":{}})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    for follow in [true, false] {
        replies.lock().await.clear();
        let channel=MattermostChannel::new(MattermostConfig {url:url.clone(),bot_token:"token".into(),..Default::default()})
            .with_acl_config(json!({"thread_follow_without_mention":follow,"group_policy":"denylist","deny_from":["blocked"],"deny_message":"","show_typing":false,"no_text_debounce":true}));
        let (process, mut requests) = reply_process("answer");
        channel.start(process.clone()).await.unwrap();
        let mut socket = recv(&mut connections).await;
        let auth = socket.recv().await.unwrap().unwrap();
        assert!(auth.to_text().unwrap().contains("authentication_challenge"));
        socket
            .send(event("root-1", "", "user", "@rua question"))
            .await
            .unwrap();
        assert_eq!(recv(&mut requests).await.metadata["root_id"], "root-1");
        wait_for_len(&replies, 1).await;
        socket
            .send(event("reply-1", "root-1", "user", "follow up"))
            .await
            .unwrap();
        if follow {
            assert_eq!(recv(&mut requests).await.metadata["root_id"], "root-1");
            wait_for_len(&replies, 2).await;
        } else {
            assert!(
                tokio::time::timeout(Duration::from_millis(150), requests.recv())
                    .await
                    .is_err()
            );
        }
        socket
            .send(event("blocked-root", "", "blocked", "@rua denied"))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        socket
            .send(event(
                "blocked-follow",
                "blocked-root",
                "user",
                "unmentioned",
            ))
            .await
            .unwrap();
        socket
            .send(event("unknown", "another-root", "user", "unmentioned"))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(150), requests.recv())
                .await
                .is_err()
        );
        channel.stop().await.unwrap();
        channel.start(process).await.unwrap();
        let mut socket = recv(&mut connections).await;
        let _ = socket.recv().await;
        socket
            .send(event("after-restart", "root-1", "user", "unmentioned"))
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(150), requests.recv())
                .await
                .is_err()
        );
        channel.stop().await.unwrap();
    }
    server.abort();
}
