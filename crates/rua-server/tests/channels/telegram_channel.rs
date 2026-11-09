use std::sync::Arc;

use rua_channel::core::ChannelManager;
use rua_config::TelegramConfig;
use rua_workspace::services::ChannelsServiceAdapter;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_injection_registers_telegram_channel() {
    let manager = Arc::new(ChannelManager::new());
    let config = TelegramConfig {
        enabled: true,
        bot_token: "123:ABC".into(),
        chat_id: "456".into(),
        allowed_users: vec![],
        bot_username: String::new(),
        media_dir: String::new(),
        common: Default::default(),
    };

    let adapter = ChannelsServiceAdapter::new(Arc::clone(&manager)).with_telegram_config(config);

    adapter.register_configured_channels().await.unwrap();

    let channels = manager.channel_ids().await;
    assert!(
        channels.iter().any(|id| id == "telegram"),
        "Telegram channel should be registered; got: {channels:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_bot_token_skips_registration() {
    let manager = Arc::new(ChannelManager::new());
    let config = TelegramConfig {
        enabled: true,
        bot_token: String::new(),
        chat_id: "456".into(),
        allowed_users: vec![],
        bot_username: String::new(),
        media_dir: String::new(),
        common: Default::default(),
    };

    let adapter = ChannelsServiceAdapter::new(Arc::clone(&manager)).with_telegram_config(config);

    assert!(adapter.register_configured_channels().await.is_err());

    let channels = manager.channel_ids().await;
    assert!(
        !channels.iter().any(|id| id == "telegram"),
        "Telegram channel should NOT be registered with empty bot_token"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bot_token_without_chat_id_registers_for_inbound_replies() {
    let manager = Arc::new(ChannelManager::new());
    let config = TelegramConfig {
        enabled: true,
        bot_token: "123:ABC".into(),
        chat_id: String::new(),
        allowed_users: vec![],
        bot_username: String::new(),
        media_dir: String::new(),
        common: Default::default(),
    };

    let adapter = ChannelsServiceAdapter::new(Arc::clone(&manager)).with_telegram_config(config);

    adapter.register_configured_channels().await.unwrap();

    let channels = manager.channel_ids().await;
    assert!(
        channels.iter().any(|id| id == "telegram"),
        "Telegram inbound replies do not require a configured chat_id"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_config_skips_registration() {
    let manager = Arc::new(ChannelManager::new());

    let adapter = ChannelsServiceAdapter::new(Arc::clone(&manager));

    adapter.register_configured_channels().await.unwrap();

    let channels = manager.channel_ids().await;
    assert!(
        !channels.iter().any(|id| id == "telegram"),
        "Telegram channel should NOT be registered without config"
    );
}

#[test]
fn telegram_config_deserializes_from_json() {
    let json = serde_json::json!({
        "bot_token": "123:ABC",
        "chat_id": "456",
        "allowed_users": ["user1", "user2"]
    });

    let config: TelegramConfig = serde_json::from_value(json).unwrap();
    assert_eq!(config.bot_token, "123:ABC");
    assert_eq!(config.chat_id, "456");
    assert_eq!(config.allowed_users, vec!["user1", "user2"]);
}

#[test]
fn telegram_config_defaults() {
    let json = serde_json::json!({});
    let config: TelegramConfig = serde_json::from_value(json).unwrap();
    assert!(config.bot_token.is_empty());
    assert!(config.chat_id.is_empty());
    assert!(config.allowed_users.is_empty());
}

use axum::{routing::post, Router};
use rua_channel::core::channel::Channel;
use rua_channel::core::{ChannelTarget, OutgoingContentPart, OutgoingMessage};
use rua_channel::telegram::{TelegramChannel, TelegramConfig as ChannelTelegramConfig};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::TcpListener;

fn make_target(chat_id: &str) -> ChannelTarget {
    ChannelTarget {
        agent_id: String::new(),
        conversation_id: chat_id.into(),
        sender_id: None,
        reply_to: None,
        metadata: serde_json::json!({"chat_id": chat_id}),
    }
}

fn make_text_msg(text: &str) -> OutgoingMessage {
    OutgoingMessage {
        channel_id: "telegram".into(),
        conversation_id: String::new(),
        receiver_id: None,
        content: vec![OutgoingContentPart::Text { text: text.into() }],
        reply_to: None,
        metadata: serde_json::json!({}),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_posts_to_sendmessage_with_text_and_chat_id() {
    let received = Arc::new(tokio::sync::Mutex::new(None::<serde_json::Value>));
    let received_clone = Arc::clone(&received);

    let app = Router::new().route(
        "/bot123:ABC/sendMessage",
        post(move |body: axum::Json<serde_json::Value>| {
            let r = Arc::clone(&received_clone);
            async move {
                *r.lock().await = Some(body.0);
                axum::Json(serde_json::json!({"ok": true}))
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = ChannelTelegramConfig {
        bot_token: "123:ABC".into(),
        chat_id: "my-chat-789".into(),
        allowed_users: vec![],
        bot_username: String::new(),
        media_dir: String::new(),
    };
    let ch = TelegramChannel::new(config).with_base_url(format!("http://{addr}"));

    ch.send(make_target("my-chat-789"), make_text_msg("hello world"))
        .await
        .unwrap();

    let received_body = received.lock().await;
    let body = received_body.as_ref().unwrap();
    assert_eq!(body["chat_id"], "my-chat-789");
    assert_eq!(body["text"], "hello world");
    assert_eq!(body["parse_mode"], "HTML");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_with_mock_server_via_custom_base_url() {
    let received = Arc::new(tokio::sync::Mutex::new(None::<serde_json::Value>));
    let received_clone = Arc::clone(&received);

    let app = Router::new().route(
        "/bot123:ABC/sendMessage",
        post(move |body: axum::Json<serde_json::Value>| {
            let r = Arc::clone(&received_clone);
            async move {
                *r.lock().await = Some(body.0);
                axum::Json(serde_json::json!({"ok": true}))
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = ChannelTelegramConfig {
        bot_token: "123:ABC".into(),
        chat_id: "chat-42".into(),
        allowed_users: vec![],
        bot_username: String::new(),
        media_dir: String::new(),
    };
    let ch = TelegramChannel::new(config).with_base_url(format!("http://{addr}"));

    ch.send(make_target("chat-42"), make_text_msg("test message"))
        .await
        .unwrap();

    let received_body = received.lock().await;
    let body = received_body.as_ref().unwrap();
    assert_eq!(body["chat_id"], "chat-42");
    assert_eq!(body["text"], "test message");
    assert_eq!(body["parse_mode"], "HTML");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_4xx_returns_error() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&call_count);

    let app = Router::new().route(
        "/bot123:ABC/sendMessage",
        post(move || {
            let c = Arc::clone(&count_clone);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                (
                    axum::http::StatusCode::FORBIDDEN,
                    axum::Json(serde_json::json!({"ok": false, "description": "Forbidden"})),
                )
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let config = ChannelTelegramConfig {
        bot_token: "123:ABC".into(),
        chat_id: "chat-42".into(),
        allowed_users: vec![],
        bot_username: String::new(),
        media_dir: String::new(),
    };
    let ch = TelegramChannel::new(config).with_base_url(format!("http://{addr}"));

    let result = ch.send(make_target("chat-42"), make_text_msg("fail")).await;
    assert!(result.is_err(), "4xx should return error");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("403") || err_msg.contains("Forbidden"),
        "error should mention 403: {err_msg}"
    );
}
