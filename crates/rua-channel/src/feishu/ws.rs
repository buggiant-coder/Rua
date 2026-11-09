//! WebSocket protocol adapted from lark-oapi 1.5.3 (MIT).
//! Copyright (c) 2023 Lark Technologies Pte. Ltd.
//! See THIRD_PARTY_NOTICES.md for the license and source attribution.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::http_client::default_http_client;

use super::ws_proto;

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(8);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const MAX_CONSECUTIVE_FAILURES: u32 = 3;
const FRAME_TYPE_DATA: i32 = 1;
const MESSAGE_TYPE_EVENT: &str = "event";
const HEADER_TYPE: &str = "type";
const HEADER_BIZ_RT: &str = "biz_rt";

#[derive(Debug, Deserialize)]
struct WsEndpointResponse {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    data: Option<WsEndpointData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WsEndpointData {
    #[serde(alias = "url", alias = "URL")]
    url: String,
    #[serde(default)]
    client_config: Option<WsClientConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WsClientConfig {
    #[serde(default)]
    ping_interval: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct WsFrame {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[allow(dead_code)]
    #[serde(default)]
    headers: Option<serde_json::Value>,
}

pub type OnMessageFn = Arc<dyn Fn(serde_json::Value) + Send + Sync>;
pub type OnCallbackFn = Arc<dyn Fn(serde_json::Value) -> Option<serde_json::Value> + Send + Sync>;

pub struct WsConfig {
    pub app_id: String,
    pub app_secret: String,
}

pub fn spawn_ws_task(
    config: WsConfig,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    spawn_ws_task_with_callback(config, on_message, None, stop_rx)
}

pub fn spawn_ws_task_with_callback(
    config: WsConfig,
    on_message: OnMessageFn,
    on_callback: Option<OnCallbackFn>,
    stop_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    spawn_ws_task_with_base_url(
        config,
        "https://open.feishu.cn".into(),
        on_message,
        on_callback,
        stop_rx,
    )
}

pub fn spawn_ws_task_with_base_url(
    config: WsConfig,
    base_url: String,
    on_message: OnMessageFn,
    on_callback: Option<OnCallbackFn>,
    stop_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(ws_run_loop(
        config,
        base_url,
        on_message,
        on_callback,
        stop_rx,
    ))
}

async fn open_ws_endpoint(
    client: &reqwest::Client,
    base_url: &str,
    config: &WsConfig,
) -> Result<WsEndpointData, String> {
    // QwenPaw uses lark-oapi: this endpoint authenticates application
    // credentials directly; it is not an Open API tenant-token endpoint.
    let resp = client
        .post(format!(
            "{}/callback/ws/endpoint",
            base_url.trim_end_matches('/')
        ))
        .header("locale", "zh")
        .json(&serde_json::json!({"AppID": config.app_id, "AppSecret": config.app_secret}))
        .send()
        .await
        .map_err(|e| format!("ws endpoint request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("ws endpoint returned {status}: {text}"));
    }

    let ep_resp: WsEndpointResponse = resp
        .json()
        .await
        .map_err(|e| format!("ws endpoint response parse failed: {e}"))?;

    if ep_resp.code != 0 {
        return Err(format!("ws endpoint error code: {}", ep_resp.code));
    }

    let data = ep_resp.data.ok_or("ws endpoint response missing data")?;
    if data.url.is_empty() {
        return Err("ws endpoint returned empty URL".into());
    }

    Ok(data)
}

async fn ws_run_loop(
    config: WsConfig,
    base_url: String,
    on_message: OnMessageFn,
    on_callback: Option<OnCallbackFn>,
    mut stop_rx: watch::Receiver<bool>,
) {
    let http = default_http_client();
    let mut backoff = INITIAL_BACKOFF;
    let mut consecutive_failures: u32 = 0;

    loop {
        if *stop_rx.borrow() {
            tracing::info!("feishu WS loop: stop signal before connect");
            return;
        }

        tracing::info!("feishu WS: opening WS endpoint");
        let endpoint_result = tokio::select! {
            _ = stop_rx.changed() => return,
            result = open_ws_endpoint(&http, &base_url, &config) => result,
        };
        let endpoint = match endpoint_result {
            Ok(url) => url,
            Err(e) => {
                consecutive_failures += 1;
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    tracing::error!(
                        "feishu WS: {consecutive_failures} consecutive endpoint failures: {e}"
                    );
                } else {
                    tracing::warn!("feishu WS endpoint error: {e}, retry in {backoff:?}");
                }
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = stop_rx.changed() => { return; }
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        tracing::info!("feishu WS: connecting");
        let service_id = url::Url::parse(&endpoint.url)
            .ok()
            .and_then(|url| {
                url.query_pairs()
                    .find(|(key, _)| key == "service_id")
                    .and_then(|(_, value)| value.parse::<i32>().ok())
            })
            .unwrap_or(0);
        let heartbeat = endpoint
            .client_config
            .as_ref()
            .and_then(|config| config.ping_interval)
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs)
            .unwrap_or(HEARTBEAT_INTERVAL);
        let connected = tokio::select! {
            _ = stop_rx.changed() => return,
            result = tokio_tungstenite::connect_async(&endpoint.url) => result,
        };
        let ws_stream = match connected {
            Ok((stream, _)) => {
                tracing::info!("feishu WS: connected");
                backoff = INITIAL_BACKOFF;
                consecutive_failures = 0;
                stream
            }
            Err(e) => {
                consecutive_failures += 1;
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    tracing::error!(
                        "feishu WS: {consecutive_failures} consecutive connect failures: {e}"
                    );
                } else {
                    tracing::warn!("feishu WS connect error: {e}, retry in {backoff:?}");
                }
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = stop_rx.changed() => { return; }
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        let disconnected = handle_ws_session_configured(
            ws_stream,
            &on_message,
            on_callback.clone(),
            &mut stop_rx,
            service_id,
            heartbeat,
        )
        .await;
        if !disconnected {
            tracing::info!("feishu WS: graceful shutdown");
            return;
        }

        tracing::warn!("feishu WS: disconnected, reconnecting in {backoff:?}");
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = stop_rx.changed() => { return; }
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

pub async fn handle_ws_session(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    on_message: &OnMessageFn,
    stop_rx: &mut watch::Receiver<bool>,
) -> bool {
    handle_ws_session_with_callback(ws_stream, on_message, None, stop_rx).await
}

pub async fn handle_ws_session_with_callback(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    on_message: &OnMessageFn,
    on_callback: Option<OnCallbackFn>,
    stop_rx: &mut watch::Receiver<bool>,
) -> bool {
    handle_ws_session_configured(
        ws_stream,
        on_message,
        on_callback,
        stop_rx,
        0,
        HEARTBEAT_INTERVAL,
    )
    .await
}

async fn handle_ws_session_configured(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    on_message: &OnMessageFn,
    on_callback: Option<OnCallbackFn>,
    stop_rx: &mut watch::Receiver<bool>,
    service_id: i32,
    heartbeat: Duration,
) -> bool {
    let (mut sink, mut stream) = ws_stream.split();
    let mut heartbeat_interval =
        tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    let mut fragments = std::collections::HashMap::<String, FragmentedEvent>::new();
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                let _ = sink.close().await;
                return false;
            }
            _ = heartbeat_interval.tick() => {
                let ping = ws_proto::Frame {
                    service: service_id,
                    method: 0,
                    headers: vec![ws_proto::Header { key: HEADER_TYPE.into(), value: "ping".into() }],
                    ..Default::default()
                };
                if sink.send(Message::Binary(ws_proto::encode_frame(&ping).into())).await.is_err() {
                    return true;
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_text_frame(&text, on_message);
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Some(response) = handle_binary_frame(&bytes, on_message, on_callback.as_ref(), &mut fragments) {
                            if sink.send(Message::Binary(response.into())).await.is_err() {
                                return true;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if sink.send(Message::Pong(data)).await.is_err() {
                            return true;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return true;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::error!("feishu WS read error: {e}");
                        return true;
                    }
                }
            }
        }
    }
}

struct FragmentedEvent {
    created_at: std::time::Instant,
    parts: Vec<Option<Vec<u8>>>,
    size: usize,
}

fn handle_binary_frame(
    bytes: &[u8],
    on_message: &OnMessageFn,
    on_callback: Option<&OnCallbackFn>,
    fragments: &mut std::collections::HashMap<String, FragmentedEvent>,
) -> Option<Vec<u8>> {
    let mut frame = match ws_proto::decode_frame(bytes) {
        Ok(frame) => frame,
        Err(err) => {
            tracing::warn!(error = %err, "feishu WS: protobuf frame decode failed");
            return None;
        }
    };
    if frame.method != FRAME_TYPE_DATA {
        return None;
    }
    if frame.header_value(HEADER_TYPE) != Some(MESSAGE_TYPE_EVENT) {
        return None;
    }
    let mut payload = frame.payload.clone().unwrap_or_default();
    let count = frame
        .header_value("sum")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1);
    if count > 1 {
        let id = frame.header_value("message_id")?.to_string();
        let index = frame.header_value("seq")?.parse::<usize>().ok()?;
        if count > 1024 || index >= count {
            return None;
        }
        fragments.retain(|_, event| event.created_at.elapsed() < Duration::from_secs(5));
        if fragments.len() >= 128 && !fragments.contains_key(&id) {
            return None;
        }
        let event = fragments
            .entry(id.clone())
            .or_insert_with(|| FragmentedEvent {
                created_at: std::time::Instant::now(),
                parts: vec![None; count],
                size: 0,
            });
        if event.parts.len() != count || event.size + payload.len() > 4 * 1024 * 1024 {
            fragments.remove(&id);
            return None;
        }
        if event.parts[index].is_none() {
            event.size += payload.len();
            event.parts[index] = Some(payload);
        }
        if event.parts.iter().any(Option::is_none) {
            return None;
        }
        payload = fragments
            .remove(&id)?
            .parts
            .into_iter()
            .flatten()
            .flatten()
            .collect();
    }
    let data = match serde_json::from_slice::<serde_json::Value>(&payload) {
        Ok(data) => data,
        Err(err) => {
            tracing::warn!(error = %err, "feishu WS: protobuf event payload parse failed");
            return Some(encode_callback_response_frame(frame, None, 500));
        }
    };
    on_message(data.clone());
    let started = std::time::Instant::now();
    let callback_response = on_callback.and_then(|handler| handler(data));
    frame.headers.push(ws_proto::Header {
        key: HEADER_BIZ_RT.into(),
        value: started.elapsed().as_millis().to_string(),
    });
    Some(encode_callback_response_frame(
        frame,
        callback_response,
        200,
    ))
}

fn encode_callback_response_frame(
    mut frame: ws_proto::Frame,
    callback_response: Option<serde_json::Value>,
    code: i32,
) -> Vec<u8> {
    // The outer frame is protobuf, but the SDK puts JSON in its payload.
    let mut response = serde_json::json!({"code": code});
    if let Some(value) = callback_response {
        if let Ok(bytes) = serde_json::to_vec(&value) {
            response["data"] =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes).into();
        }
    }
    frame.payload = Some(serde_json::to_vec(&response).expect("serializable WS response"));
    ws_proto::encode_frame(&frame)
}

fn handle_text_frame(text: &str, on_message: &OnMessageFn) {
    let frame: WsFrame = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("feishu WS: frame parse error: {e}");
            return;
        }
    };

    match frame.r#type.as_str() {
        "pong" => {
            tracing::debug!("feishu WS: pong received");
        }
        "event" => {
            if let Some(data) = frame.data {
                tracing::info!("feishu WS: event received");
                on_message(data);
            }
        }
        other => {
            tracing::debug!("feishu WS: unknown frame type: {other}");
        }
    }
}
