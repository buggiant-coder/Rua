//! Rust transport for QwenPaw's Slack Socket Mode lifecycle (Apache-2.0).
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::channel::SlackChannel;
use crate::core::channel::Process;
use rua_core::{Result, RuaError};

pub(super) async fn api_call(
    client: &reqwest::Client,
    base: &str,
    method: &str,
    token: &str,
) -> Result<Value> {
    let response = client
        .post(format!("{base}/{method}"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| RuaError::Runtime {
            message: format!("Slack {method}: {error}"),
        })?;
    let status = response.status();
    let body: Value = response.json().await.map_err(|error| RuaError::Runtime {
        message: format!("Slack {method}: {error}"),
    })?;
    if !status.is_success() || body["ok"] != true {
        return Err(RuaError::Runtime {
            message: format!(
                "Slack {method} failed: {}",
                body["error"].as_str().unwrap_or("invalid response")
            ),
        });
    }
    Ok(body)
}

pub(super) async fn connection_url(
    client: &reqwest::Client,
    base: &str,
    token: &str,
) -> Result<String> {
    let body = api_call(client, base, "apps.connections.open", token).await?;
    body["url"]
        .as_str()
        .filter(|url| !url.is_empty())
        .map(str::to_string)
        .ok_or_else(|| RuaError::Runtime {
            message: "Slack connection response has no URL".into(),
        })
}

pub(super) async fn run(
    channel: Arc<SlackChannel>,
    process: Arc<dyn Process>,
    first_url: String,
    cancel: CancellationToken,
) {
    let mut url = Some(first_url);
    let mut failures = 0u32;
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        let attempt = async {
            let url = match url.take() {
                Some(url) => url,
                None => {
                    connection_url(
                        &channel.client,
                        &channel.api_base,
                        &channel.config.app_token,
                    )
                    .await?
                }
            };
            let mut socket = crate::http_client::connect_websocket(&channel.client, &url).await?;
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Ok::<_, RuaError>(()),
                    completed = tasks.join_next(), if !tasks.is_empty() => {
                        if let Some(Err(error)) = completed {
                            tracing::warn!(%error, "Slack message task failed");
                        }
                    }
                    frame = socket.next() => {
                        use tokio_tungstenite::tungstenite::Message;
                        match frame {
                            Some(Ok(Message::Text(text))) => {
                                let envelope: Value = match serde_json::from_str(&text) {
                                    Ok(value) => value,
                                    Err(_) => continue,
                                };
                                if envelope["type"] == "hello" {
                                    failures = 0;
                                    continue;
                                }
                                if envelope["type"] == "disconnect" {
                                    return Ok(());
                                }
                                if let Some(id) = envelope["envelope_id"].as_str() {
                                    socket.send(Message::Text(json!({"envelope_id": id}).to_string().into()))
                                        .await.map_err(|error| RuaError::Runtime { message: format!("Slack ack: {error}") })?;
                                }
                                if envelope["type"] != "events_api" {
                                    continue;
                                }
                                let payload = envelope["payload"].clone();
                                let channel = channel.clone();
                                let process = process.clone();
                                tasks.spawn(async move {
                                    if let Err(error) = channel.handle_socket_payload(process, payload).await {
                                        tracing::debug!(%error, "Slack event skipped or failed");
                                    }
                                });
                            }
                            Some(Ok(Message::Ping(data))) => {
                                let _ = socket.send(Message::Pong(data)).await;
                            }
                            Some(Ok(Message::Close(_))) | None => return Ok(()),
                            Some(Err(error)) => return Err(RuaError::Runtime { message: format!("Slack socket: {error}") }),
                            _ => {}
                        }
                    }
                }
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = attempt => {
                if let Err(error) = result {
                    tracing::warn!(%error, "Slack Socket Mode reconnecting");
                }
            }
        }
        failures = failures.saturating_add(1);
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(1u64 << failures.min(5))) => {}
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}
