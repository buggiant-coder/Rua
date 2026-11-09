use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

pub type OnMessageFn = Arc<dyn Fn(serde_json::Value) + Send + Sync>;

pub struct WsConfig {
    pub base_url: String,
    pub bot_token: String,
}

pub fn spawn_ws_task(
    config: WsConfig,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
) -> Option<JoinHandle<()>> {
    if config.base_url.is_empty() || config.bot_token.is_empty() {
        tracing::warn!("mattermost WS: url or bot_token empty, skipping");
        return None;
    }

    let handle = tokio::spawn(ws_loop(config, on_message, stop_rx));
    Some(handle)
}

async fn ws_loop(config: WsConfig, on_message: OnMessageFn, mut stop_rx: watch::Receiver<bool>) {
    let ws_url = config
        .base_url
        .replace("http://", "ws://")
        .replace("https://", "wss://")
        + "/api/v4/websocket";

    let mut backoff = INITIAL_BACKOFF;

    loop {
        if *stop_rx.borrow() {
            tracing::debug!("mattermost WS: stop signal received");
            return;
        }

        tracing::info!("mattermost WS: connecting to {ws_url}");
        match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((ws_stream, _)) => {
                backoff = INITIAL_BACKOFF;
                let (mut write, mut read) = ws_stream.split();

                let auth = serde_json::json!({
                    "seq": 1,
                    "action": "authentication_challenge",
                    "data": { "token": config.bot_token }
                });
                if let Err(e) = write.send(Message::Text(auth.to_string().into())).await {
                    tracing::error!("mattermost WS: auth send failed: {e}");
                    continue;
                }
                tracing::info!("mattermost WS: connected and authenticated");

                loop {
                    tokio::select! {
                        msg = read.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                                        if data.get("event").and_then(|v| v.as_str()) == Some("posted") {
                                            on_message(data);
                                        }
                                    }
                                }
                                Some(Ok(Message::Close(_))) | None => {
                                    tracing::info!("mattermost WS: connection closed");
                                    break;
                                }
                                Some(Err(e)) => {
                                    tracing::warn!("mattermost WS: read error: {e}");
                                    break;
                                }
                                _ => {}
                            }
                        }
                        _ = stop_rx.changed() => {
                            if *stop_rx.borrow() {
                                tracing::debug!("mattermost WS: stop signal in read loop");
                                let _ = write.send(Message::Close(None)).await;
                                return;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("mattermost WS: connect failed: {e}");
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    return;
                }
            }
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}
