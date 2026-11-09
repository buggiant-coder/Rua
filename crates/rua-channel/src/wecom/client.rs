use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use md5::{Digest, Md5};
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use super::types::{cmd, generate_req_id, WecomBotConfig, WsFrame};

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const MAX_MISSED_PONG: u32 = 2;
pub const UPLOAD_CHUNK_SIZE: usize = 512 * 1024;

pub type OnMessageFn = Arc<dyn Fn(WsFrame) + Send + Sync>;

#[derive(Debug)]
pub struct OutboundCommand {
    frame: WsFrame,
    ack_tx: Option<oneshot::Sender<WsFrame>>,
}

#[derive(Clone, Debug)]
pub struct WecomWsSender {
    tx: mpsc::UnboundedSender<OutboundCommand>,
}

impl WecomWsSender {
    pub async fn send_frame(&self, frame: WsFrame) -> Result<(), String> {
        self.tx
            .send(OutboundCommand {
                frame,
                ack_tx: None,
            })
            .map_err(|e| format!("wecom outbound queue closed: {e}"))
    }

    pub async fn send_frame_wait_ack(
        &self,
        frame: WsFrame,
        timeout: Duration,
    ) -> Result<WsFrame, String> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(OutboundCommand {
                frame,
                ack_tx: Some(ack_tx),
            })
            .map_err(|e| format!("wecom outbound queue closed: {e}"))?;
        tokio::time::timeout(timeout, ack_rx)
            .await
            .map_err(|_| "wecom outbound ack timed out".to_string())?
            .map_err(|_| "wecom outbound ack sender dropped".to_string())
    }

    pub async fn send_message(&self, chatid: &str, body: serde_json::Value) -> Result<(), String> {
        let frame = WsFrame {
            cmd: Some(cmd::SEND_MSG.into()),
            headers: super::types::WsHeaders {
                req_id: generate_req_id(cmd::SEND_MSG),
            },
            body: Some(serde_json::json!({
                "chatid": chatid,
                "msg": body,
            })),
            ..Default::default()
        };
        self.send_frame(frame).await
    }

    pub async fn reply(&self, source: &WsFrame, body: serde_json::Value) -> Result<(), String> {
        let frame = WsFrame {
            cmd: Some(cmd::RESPONSE.into()),
            headers: super::types::WsHeaders {
                req_id: source.headers.req_id.clone(),
            },
            body: Some(body),
            ..Default::default()
        };
        self.send_frame(frame).await
    }

    pub async fn update_template_card(
        &self,
        source: &WsFrame,
        template_card: serde_json::Value,
    ) -> Result<(), String> {
        self.reply(
            source,
            serde_json::json!({
                "msgtype": "template_card",
                "template_card": template_card,
            }),
        )
        .await
    }

    pub async fn upload_media(
        &self,
        path: &Path,
        media_type: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let data = tokio::fs::read(path)
            .await
            .map_err(|e| format!("read wecom upload file {}: {e}", path.display()))?;
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("file");
        let init_ack = self
            .send_frame_wait_ack(
                build_upload_init_frame(media_type, filename, &data),
                timeout,
            )
            .await?;
        ensure_ack_ok(&init_ack)?;
        let upload_id = init_ack
            .body
            .as_ref()
            .and_then(|body| body.get("upload_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "wecom upload init ack missing upload_id".to_string())?
            .to_string();

        for frame in build_upload_chunk_frames(&upload_id, &data) {
            let ack = self.send_frame_wait_ack(frame, timeout).await?;
            ensure_ack_ok(&ack)?;
        }

        let finish_ack = self
            .send_frame_wait_ack(build_upload_finish_frame(&upload_id), timeout)
            .await?;
        ensure_ack_ok(&finish_ack)?;
        finish_ack
            .body
            .as_ref()
            .and_then(|body| body.get("media_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .ok_or_else(|| "wecom upload finish ack missing media_id".to_string())
    }
}

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

pub fn spawn_ws_task(
    config: WecomBotConfig,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
) -> (WecomWsSender, JoinHandle<()>) {
    let (sender, outbound_rx) = create_ws_sender();
    let handle = spawn_ws_task_with_sender(config, on_message, stop_rx, outbound_rx);
    (sender, handle)
}

pub fn spawn_ws_task_with_sender(
    config: WecomBotConfig,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
    outbound_rx: mpsc::UnboundedReceiver<OutboundCommand>,
) -> JoinHandle<()> {
    spawn_ws_task_with_options(config, on_message, stop_rx, outbound_rx, -1, String::new())
}

pub(super) fn spawn_ws_task_with_options(
    config: WecomBotConfig,
    on_message: OnMessageFn,
    stop_rx: watch::Receiver<bool>,
    outbound_rx: mpsc::UnboundedReceiver<OutboundCommand>,
    max_reconnect_attempts: i64,
    welcome_text: String,
) -> JoinHandle<()> {
    tokio::spawn(ws_run_loop(
        config,
        on_message,
        stop_rx,
        outbound_rx,
        max_reconnect_attempts,
        welcome_text,
    ))
}

async fn ws_run_loop(
    config: WecomBotConfig,
    on_message: OnMessageFn,
    mut stop_rx: watch::Receiver<bool>,
    mut outbound_rx: mpsc::UnboundedReceiver<OutboundCommand>,
    max_reconnect_attempts: i64,
    welcome_text: String,
) {
    let mut backoff = INITIAL_BACKOFF;
    let mut reconnect_attempts = 0_i64;

    loop {
        if *stop_rx.borrow() {
            tracing::info!("wecom WS: stop signal before connect");
            return;
        }

        if max_reconnect_attempts >= 0 && reconnect_attempts > max_reconnect_attempts {
            tracing::error!("wecom WS: reconnect limit reached");
            return;
        }
        reconnect_attempts = reconnect_attempts.saturating_add(1);
        tracing::info!("wecom WS: connecting to {}", config.ws_url);
        let connection = tokio::select! {
            _ = stop_rx.changed() => return,
            connection = tokio_tungstenite::connect_async(&config.ws_url) => connection,
        };
        let ws_stream = match connection {
            Ok((stream, _)) => {
                tracing::info!("wecom WS: connected");
                backoff = INITIAL_BACKOFF;
                stream
            }
            Err(e) => {
                tracing::error!("wecom WS connect error: {e}, retry in {backoff:?}");
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = stop_rx.changed() => { return; }
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        let disconnected = handle_ws_session(
            ws_stream,
            &config,
            &on_message,
            &mut stop_rx,
            &mut outbound_rx,
            &welcome_text,
            &mut reconnect_attempts,
        )
        .await;
        if !disconnected {
            tracing::info!("wecom WS: graceful shutdown");
            return;
        }

        tracing::warn!("wecom WS: disconnected, reconnecting in {backoff:?}");
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = stop_rx.changed() => { return; }
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn handle_ws_session(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    config: &WecomBotConfig,
    on_message: &OnMessageFn,
    stop_rx: &mut watch::Receiver<bool>,
    outbound_rx: &mut mpsc::UnboundedReceiver<OutboundCommand>,
    welcome_text: &str,
    reconnect_attempts: &mut i64,
) -> bool {
    let (sink, mut stream) = ws_stream.split();
    let sink = Arc::new(Mutex::new(sink));

    let auth_frame = WsFrame {
        cmd: Some(cmd::SUBSCRIBE.into()),
        headers: super::types::WsHeaders {
            req_id: generate_req_id(cmd::SUBSCRIBE),
        },
        body: Some(serde_json::json!({
            "bot_id": config.bot_id,
            "secret": config.secret,
        })),
        ..Default::default()
    };
    if let Err(e) = send_frame(&sink, &auth_frame).await {
        tracing::error!("wecom WS: failed to send auth: {e}");
        return true;
    }
    tracing::info!("wecom WS: auth frame sent");

    let mut heartbeat_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut missed_pong: u32 = 0;
    let mut authenticated = false;
    let mut pending_acks: HashMap<String, oneshot::Sender<WsFrame>> = HashMap::new();

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                let _ = sink.lock().await.close().await;
                return false;
            }
            _ = heartbeat_interval.tick() => {
                if !authenticated {
                    continue;
                }
                if missed_pong >= MAX_MISSED_PONG {
                    tracing::warn!("wecom WS: no pong for {missed_pong} pings, reconnecting");
                    let _ = sink.lock().await.close().await;
                    return true;
                }
                missed_pong += 1;
                let hb = WsFrame {
                    cmd: Some(cmd::HEARTBEAT.into()),
                    headers: super::types::WsHeaders {
                        req_id: generate_req_id(cmd::HEARTBEAT),
                    },
                    ..Default::default()
                };
                if let Err(e) = send_frame(&sink, &hb).await {
                    tracing::error!("wecom WS: heartbeat send failed: {e}");
                    return true;
                }
            }
            outbound = outbound_rx.recv() => {
                let Some(outbound) = outbound else {
                    let _ = sink.lock().await.close().await;
                    return false;
                };
                let req_id = outbound.frame.headers.req_id.clone();
                if let Some(ack_tx) = outbound.ack_tx {
                    pending_acks.insert(req_id.clone(), ack_tx);
                }
                if let Err(e) = send_frame(&sink, &outbound.frame).await {
                    if let Some(ack_tx) = pending_acks.remove(&req_id) {
                        let _ = ack_tx.send(WsFrame {
                            headers: super::types::WsHeaders { req_id },
                            errcode: Some(-1),
                            errmsg: Some(e.clone()),
                            ..Default::default()
                        });
                    }
                    tracing::error!("wecom WS: outbound send failed: {e}");
                    return true;
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let frame: WsFrame = match serde_json::from_str(&text) {
                            Ok(f) => f,
                            Err(e) => {
                                tracing::warn!("wecom WS: parse frame failed: {e}");
                                continue;
                            }
                        };
                        match frame.cmd.as_deref() {
                            Some(cmd::EVENT_CALLBACK)
                                if frame.body.as_ref().and_then(|body| body.pointer("/event/eventtype")).and_then(|v| v.as_str()) == Some("enter_chat") => {
                                if !welcome_text.is_empty() {
                                    let welcome = WsFrame {
                                        cmd: Some(cmd::RESPONSE_WELCOME.into()),
                                        headers: frame.headers.clone(),
                                        body: Some(serde_json::json!({"msgtype":"text","text":{"content":welcome_text}})),
                                        ..Default::default()
                                    };
                                    if let Err(error) = send_frame(&sink, &welcome).await {
                                        tracing::warn!(%error, "wecom welcome failed");
                                    }
                                }
                            }
                            Some(cmd::CALLBACK) | Some(cmd::EVENT_CALLBACK) => {
                                on_message(frame);
                            }
                            None => {

                                let req_id = &frame.headers.req_id;
                                if let Some(ack_tx) = pending_acks.remove(req_id) {
                                    let _ = ack_tx.send(frame);
                                    continue;
                                }
                                if req_id.starts_with(cmd::SUBSCRIBE) {
                                    if frame.errcode == Some(0) {
                                        tracing::info!("wecom WS: authenticated");
                                        authenticated = true;
                                        *reconnect_attempts = 0;
                                    } else {
                                        tracing::error!(
                                            "wecom WS: auth failed: errcode={:?} errmsg={:?}",
                                            frame.errcode, frame.errmsg
                                        );
                                        return true;
                                    }
                                } else if req_id.starts_with(cmd::HEARTBEAT)
                                    && frame.errcode == Some(0)
                                {
                                    missed_pong = 0;
                                    tracing::debug!("wecom WS: heartbeat ack");
                                }

                            }
                            Some(other) => {
                                tracing::debug!("wecom WS: unknown cmd: {other}");
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if sink.lock().await.send(Message::Pong(data)).await.is_err() {
                            return true;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        return true;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        tracing::error!("wecom WS read error: {e}");
                        return true;
                    }
                }
            }
        }
    }
}

pub fn create_ws_sender() -> (WecomWsSender, mpsc::UnboundedReceiver<OutboundCommand>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (WecomWsSender { tx }, rx)
}

fn ensure_ack_ok(frame: &WsFrame) -> Result<(), String> {
    match frame.errcode.unwrap_or(0) {
        0 => Ok(()),
        code => Err(format!(
            "wecom outbound ack failed: errcode={code} errmsg={}",
            frame.errmsg.as_deref().unwrap_or_default()
        )),
    }
}

async fn send_frame(sink: &Arc<Mutex<WsSink>>, frame: &WsFrame) -> Result<(), String> {
    let json = serde_json::to_string(frame).map_err(|e| format!("serialize: {e}"))?;
    sink.lock()
        .await
        .send(Message::Text(json.into()))
        .await
        .map_err(|e| format!("send: {e}"))
}

pub fn build_reply_stream_body(stream_id: &str, content: &str, finish: bool) -> serde_json::Value {
    serde_json::json!({
        "msgtype": "stream",
        "stream": {
            "id": stream_id,
            "content": content,
            "finish": finish,
        }
    })
}

pub fn build_upload_init_frame(media_type: &str, filename: &str, data: &[u8]) -> WsFrame {
    let total_chunks = data.len().div_ceil(UPLOAD_CHUNK_SIZE);
    let mut md5 = Md5::new();
    md5.update(data);
    WsFrame {
        cmd: Some(cmd::UPLOAD_INIT.into()),
        headers: super::types::WsHeaders {
            req_id: generate_req_id(cmd::UPLOAD_INIT),
        },
        body: Some(serde_json::json!({
            "type": media_type,
            "filename": filename,
            "total_size": data.len(),
            "total_chunks": total_chunks,
            "md5": format!("{:x}", md5.finalize()),
        })),
        ..Default::default()
    }
}

pub fn build_upload_chunk_frames(upload_id: &str, data: &[u8]) -> Vec<WsFrame> {
    data.chunks(UPLOAD_CHUNK_SIZE)
        .enumerate()
        .map(|(idx, chunk)| WsFrame {
            cmd: Some(cmd::UPLOAD_CHUNK.into()),
            headers: super::types::WsHeaders {
                req_id: generate_req_id(cmd::UPLOAD_CHUNK),
            },
            body: Some(serde_json::json!({
                "upload_id": upload_id,
                "chunk_index": idx,
                "base64_data": base64::engine::general_purpose::STANDARD.encode(chunk),
            })),
            ..Default::default()
        })
        .collect()
}

pub fn build_upload_finish_frame(upload_id: &str) -> WsFrame {
    WsFrame {
        cmd: Some(cmd::UPLOAD_FINISH.into()),
        headers: super::types::WsHeaders {
            req_id: generate_req_id(cmd::UPLOAD_FINISH),
        },
        body: Some(serde_json::json!({ "upload_id": upload_id })),
        ..Default::default()
    }
}

pub fn build_wecom_media_message(media_type: &str, media_id: &str) -> serde_json::Value {
    let msgtype = match media_type {
        "image" => "image",
        "voice" => "voice",
        "video" => "video",
        _ => "file",
    };
    serde_json::json!({
        "msgtype": msgtype,
        msgtype: { "media_id": media_id },
    })
}
