use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::content::{split_text, IncomingContentPart, OutgoingContentPart};
use crate::core::event_dedup::{EventDedupDecision, EventDeduper};
use crate::core::pipeline;
use crate::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::ChannelId;
use crate::http_client::default_http_client;
use rua_core::Result;

use super::auth::TokenManager;
use super::codec::{
    build_auth_bind_msg, build_heartbeat_msg, build_ping_msg, build_push_ack, build_send_c2c_msg,
    build_send_group_msg, decode_auth_bind_rsp, decode_conn_msg, decode_kickout_msg,
    decode_ping_rsp, ConnHead, CMD_AUTH_BIND, CMD_KICKOUT, CMD_PING, CMD_TYPE_PUSH,
    CMD_TYPE_RESPONSE,
};
use super::media::{build_file_msg_body, build_image_msg_body, download_and_upload_media};
use super::types::{
    YuanbaoConfig, AUTH_ALREADY_CODE, AUTH_FAILED_CODES, DEFAULT_WS_URL, HEARTBEAT_FINISH,
    HEARTBEAT_INTERVAL_SECS, HEARTBEAT_RUNNING, HEARTBEAT_TIMEOUT_THRESHOLD,
    MAX_RECONNECT_ATTEMPTS, NO_RECONNECT_CLOSE_CODES, RECONNECT_DELAYS, SEND_TIMEOUT_SECS,
    TEXT_CHUNK_LIMIT, TYPING_KEEPALIVE_INTERVAL_SECS,
};

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type SharedWsSink = Arc<Mutex<WsSink>>;

const MAX_INBOUND_MEDIA_SIZE: usize = 50 * 1024 * 1024;
const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

pub struct YuanbaoChannel {
    channel_id: ChannelId,
    config: YuanbaoConfig,
    ws_url: String,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    stop_tx: watch::Sender<bool>,
    ws_handle: RwLock<Option<JoinHandle<()>>>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    event_deduper: Arc<StdMutex<EventDeduper>>,
    active_ws: Arc<RwLock<Option<SharedWsSink>>>,
    session_map: Arc<RwLock<HashMap<String, YuanbaoSessionRoute>>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct YuanbaoSessionRoute {
    chat_type: String,
    sender_id: String,
    group_code: String,
    msg_id: String,
}

impl YuanbaoSessionRoute {
    fn target_id(&self) -> String {
        if self.chat_type == "group" {
            self.group_code.clone()
        } else {
            self.sender_id.clone()
        }
    }
}

impl YuanbaoChannel {
    pub fn new(config: YuanbaoConfig) -> Self {
        let (stop_tx, _) = watch::channel(false);
        let session_map = load_session_map(&config.media_dir);
        Self {
            channel_id: "yuanbao".into(),
            ws_url: DEFAULT_WS_URL.into(),
            config,
            process: RwLock::new(None),
            started: RwLock::new(false),
            stop_tx,
            ws_handle: RwLock::new(None),
            render_options: MessageRenderOptions::default(),
            acl_metadata: serde_json::json!({}),
            event_deduper: Arc::new(StdMutex::new(EventDeduper::new(
                EVENT_DEDUP_TTL,
                EVENT_DEDUP_CAPACITY,
            ))),
            active_ws: Arc::new(RwLock::new(None)),
            session_map: Arc::new(RwLock::new(session_map)),
        }
    }

    pub fn with_ws_url(mut self, ws_url: String) -> Self {
        if !ws_url.trim().is_empty() {
            self.ws_url = ws_url;
        }
        self
    }

    pub fn with_render_options(
        mut self,
        options: crate::core::renderer::MessageRenderOptions,
    ) -> Self {
        self.render_options = options;
        self
    }

    pub fn with_acl_config(mut self, acl_metadata: serde_json::Value) -> Self {
        self.acl_metadata = acl_metadata;
        self
    }

    pub async fn download_inbound_media(&self, native: &mut serde_json::Value) -> Result<()> {
        download_yuanbao_inbound_media(&self.config.media_dir, native, None, None).await
    }
}

#[async_trait]
impl channel::Channel for YuanbaoChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        if !self.config.enabled {
            tracing::debug!("yuanbao: start() skipped (enabled=false)");
            return Ok(());
        }
        if self.config.app_id.is_empty() || self.config.app_secret.is_empty() {
            return Err(rua_core::RuaError::Validation {
                message: "yuanbao: app_id and app_secret are required".into(),
            });
        }

        *self.process.write().await = Some(process.clone());
        *self.started.write().await = true;

        self.stop_tx.send_replace(false);
        let ws_url = self.ws_url.clone();
        let config = self.config.clone();
        let process_arc = process;
        let mut stop_rx = self.stop_tx.subscribe();
        let render_options = self.render_options.clone();
        let acl_metadata = Arc::new(self.acl_metadata.clone());
        let event_deduper = self.event_deduper.clone();
        let active_ws = self.active_ws.clone();
        let session_map = self.session_map.clone();

        let handle = tokio::spawn(async move {
            let token_manager = Arc::new(TokenManager::new(
                config.app_id.clone(),
                config.app_secret.clone(),
                config.api_domain.clone(),
            ));

            let mut reconnect_attempts: u32 = 0;

            loop {
                if *stop_rx.borrow() {
                    break;
                }

                let connection_result = run_connection(
                    &token_manager,
                    &ws_url,
                    &process_arc,
                    &render_options,
                    &acl_metadata,
                    &config.api_domain,
                    &config.media_dir,
                    &event_deduper,
                    &active_ws,
                    &session_map,
                    &mut stop_rx,
                )
                .await;
                *active_ws.write().await = None;
                match connection_result {
                    Ok(close_code) => {
                        if *stop_rx.borrow() {
                            break;
                        }
                        if let Some(code) = close_code {
                            if NO_RECONNECT_CLOSE_CODES.contains(&code) {
                                tracing::error!(
                                    "yuanbao: non-retryable close code={code}, stopping"
                                );
                                break;
                            }
                        }
                        reconnect_attempts += 1;
                    }
                    Err(e) => {
                        if *stop_rx.borrow() {
                            break;
                        }
                        tracing::error!("yuanbao: connection error: {e}");
                        reconnect_attempts += 1;
                    }
                }

                if reconnect_attempts >= MAX_RECONNECT_ATTEMPTS {
                    tracing::error!("yuanbao: max reconnect attempts reached");
                    break;
                }

                let idx = (reconnect_attempts as usize).min(RECONNECT_DELAYS.len() - 1);
                let delay = Duration::from_secs(RECONNECT_DELAYS[idx]);
                tracing::info!(
                    "yuanbao: reconnecting in {}s (attempt {reconnect_attempts})",
                    delay.as_secs()
                );

                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = stop_rx.changed() => { break; }
                }
            }

            token_manager.close().await;
            tracing::info!("yuanbao: WS task exiting");
        });

        *self.ws_handle.write().await = Some(handle);
        tracing::info!("yuanbao: channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let _ = self.stop_tx.send(true);
        if let Some(h) = self.ws_handle.write().await.take() {
            h.abort();
            let _ = h.await;
        }
        *self.active_ws.write().await = None;
        if let Ok(mut deduper) = self.event_deduper.lock() {
            deduper.clear();
        }
        *self.started.write().await = false;
        tracing::info!("yuanbao: channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        let Some(ws_tx) = self.active_ws.read().await.clone() else {
            tracing::warn!("yuanbao: external send skipped; websocket is not connected");
            return Ok(());
        };
        let Some(route) = resolve_send_route(&self.session_map, &to).await else {
            tracing::warn!(
                conversation_id = %to.conversation_id,
                "yuanbao: external send skipped; no persisted session route"
            );
            return Ok(());
        };
        let token_manager = Arc::new(TokenManager::new(
            self.config.app_id.clone(),
            self.config.app_secret.clone(),
            self.config.api_domain.clone(),
        ));
        let token_data = token_manager.get_token().await?;
        send_outgoing_to_route(
            &ws_tx,
            &route.chat_type,
            &route.target_id(),
            &token_data.bot_id,
            &token_manager,
            &self.config.api_domain,
            &msg.content,
        )
        .await
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        if self.acl_metadata["accept_bot_messages"] != true && is_bot_message(&native) {
            return Err(rua_core::RuaError::Validation {
                message: "yuanbao bot message ignored".into(),
            });
        }
        let req = parse_inbound_to_request(&native, None)?;
        if check_duplicate_request(&self.event_deduper, "yuanbao", &req)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate yuanbao event".into(),
            });
        }
        persist_session_route_sync(&self.session_map, &self.config.media_dir, &req);
        Ok(req)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_connection(
    token_manager: &Arc<TokenManager>,
    ws_url: &str,
    process: &Arc<dyn Process>,
    render_options: &MessageRenderOptions,
    acl_metadata: &Arc<serde_json::Value>,
    api_domain: &str,
    media_dir: &str,
    event_deduper: &Arc<StdMutex<EventDeduper>>,
    active_ws: &Arc<RwLock<Option<SharedWsSink>>>,
    session_map: &Arc<RwLock<HashMap<String, YuanbaoSessionRoute>>>,
    stop_rx: &mut watch::Receiver<bool>,
) -> std::result::Result<Option<u16>, String> {
    let token_data = token_manager
        .get_token()
        .await
        .map_err(|e| format!("sign-token failed: {e}"))?;
    let bot_id = token_data.bot_id.clone();
    tracing::info!("yuanbao: got token for bot_id={bot_id}");

    let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| format!("WS connect failed: {e}"))?;

    let (ws_tx, mut ws_rx) = ws_stream.split();
    let ws_tx = Arc::new(Mutex::new(ws_tx));
    *active_ws.write().await = Some(ws_tx.clone());
    tracing::info!("yuanbao: WebSocket connected, sending auth...");

    let auth_binary = build_auth_bind_msg("ybBot", &bot_id, &token_data.source, &token_data.token);
    ws_tx
        .lock()
        .await
        .send(Message::Binary(auth_binary.into()))
        .await
        .map_err(|e| format!("auth send failed: {e}"))?;

    let auth_msg = tokio::time::timeout(Duration::from_secs(SEND_TIMEOUT_SECS), ws_rx.next())
        .await
        .map_err(|_| "auth response timeout".to_string())?
        .ok_or_else(|| "WS closed before auth response".to_string())?
        .map_err(|e| format!("auth receive error: {e}"))?;

    let auth_raw = match auth_msg {
        Message::Binary(b) => b,
        _ => return Err("expected binary auth response".into()),
    };

    let conn_msg =
        decode_conn_msg(&auth_raw).ok_or_else(|| "failed to decode auth response".to_string())?;
    if conn_msg.head.cmd != CMD_AUTH_BIND {
        return Err(format!("unexpected auth cmd: {}", conn_msg.head.cmd));
    }

    let rsp = decode_auth_bind_rsp(&conn_msg.data);
    if rsp.code != 0 && rsp.code != AUTH_ALREADY_CODE && conn_msg.head.status != 0 {
        return Err(format!(
            "auth failed: code={}, status={}, msg={}",
            rsp.code, conn_msg.head.status, rsp.message
        ));
    }

    tracing::info!("yuanbao: authenticated ✅ (connectId={})", rsp.connect_id);

    let mut heartbeat_interval =
        tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    heartbeat_interval.tick().await;

    let mut heartbeat_ack_received = true;
    let mut heartbeat_timeout_count: u32 = 0;

    loop {
        tokio::select! {
            _ = stop_rx.changed() => {
                let _ = ws_tx.lock().await.close().await;
                return Ok(None);
            }
            _ = heartbeat_interval.tick() => {
                if !heartbeat_ack_received {
                    heartbeat_timeout_count += 1;
                    if heartbeat_timeout_count >= HEARTBEAT_TIMEOUT_THRESHOLD {
                        tracing::error!("yuanbao: heartbeat threshold reached, reconnecting");
                        let _ = ws_tx.lock().await.close().await;
                        return Ok(None);
                    }
                } else {
                    heartbeat_timeout_count = 0;
                }
                heartbeat_ack_received = false;
                let ping = build_ping_msg();
                if ws_tx.lock().await.send(Message::Binary(ping.into())).await.is_err() {
                    return Ok(None);
                }
            }
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Binary(raw))) => {
                        if let Some(conn_msg) = decode_conn_msg(&raw) {
                            let result = handle_frame(
                                &conn_msg.head,
                                &conn_msg.data,
                                &ws_tx,
                                process,
                                &bot_id,
                                token_manager.clone(),
                                api_domain,
                                render_options,
                                acl_metadata,
                                media_dir,
                                event_deduper,
                                session_map,
                                &mut heartbeat_ack_received,
                                &mut heartbeat_interval,
                            ).await;
                            match result {
                                FrameResult::Continue => {}
                                FrameResult::Kicked | FrameResult::AuthFailed => {
                                    let _ = ws_tx.lock().await.close().await;
                                    return Ok(None);
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        let code = frame.as_ref().map(|f| f.code.into());
                        tracing::info!("yuanbao: WS closed by server code={code:?}");
                        return Ok(code);
                    }
                    Some(Err(e)) => {
                        tracing::error!("yuanbao: WS error: {e}");
                        return Ok(None);
                    }
                    None => {
                        return Ok(None);
                    }
                    _ => {}
                }
            }
        }
    }
}

enum FrameResult {
    Continue,
    Kicked,
    AuthFailed,
}

#[allow(clippy::too_many_arguments)]
async fn handle_frame(
    head: &ConnHead,
    data: &[u8],
    ws_tx: &Arc<Mutex<WsSink>>,
    process: &Arc<dyn Process>,
    bot_id: &str,
    token_manager: Arc<TokenManager>,
    api_domain: &str,
    render_options: &MessageRenderOptions,
    acl_metadata: &Arc<serde_json::Value>,
    media_dir: &str,
    event_deduper: &Arc<StdMutex<EventDeduper>>,
    session_map: &Arc<RwLock<HashMap<String, YuanbaoSessionRoute>>>,
    heartbeat_ack: &mut bool,
    heartbeat_interval: &mut tokio::time::Interval,
) -> FrameResult {
    match head.cmd_type {
        CMD_TYPE_RESPONSE => {
            if head.cmd == CMD_PING {
                *heartbeat_ack = true;
                let rsp = decode_ping_rsp(data);
                if rsp.heart_interval > 0 {
                    let new_interval = Duration::from_secs(rsp.heart_interval as u64);
                    *heartbeat_interval = tokio::time::interval(new_interval);
                    heartbeat_interval.tick().await;
                }
            } else if head.cmd == CMD_AUTH_BIND
                && head.status != 0
                && AUTH_FAILED_CODES.contains(&head.status)
            {
                tracing::warn!("yuanbao: auth failed in-band status={}", head.status);
                return FrameResult::AuthFailed;
            }
            FrameResult::Continue
        }
        CMD_TYPE_PUSH => {
            if head.need_ack {
                let ack = build_push_ack(head);
                let _ = ws_tx.lock().await.send(Message::Binary(ack.into())).await;
            }

            if head.cmd == CMD_KICKOUT {
                let kickout = decode_kickout_msg(data);
                tracing::warn!("yuanbao: kicked out: {}", kickout.reason);
                return FrameResult::Kicked;
            }

            if !data.is_empty() {
                if let Ok(json_str) = std::str::from_utf8(data) {
                    if let Ok(inbound) = serde_json::from_str::<serde_json::Value>(json_str) {
                        let ws_tx = ws_tx.clone();
                        let process = process.clone();
                        let bot_id = bot_id.to_string();
                        let token_manager = token_manager.clone();
                        let api_domain = api_domain.to_string();
                        let render_options = render_options.clone();
                        let acl = acl_metadata.clone();
                        let media_dir = media_dir.to_string();
                        let event_deduper = event_deduper.clone();
                        let session_map = session_map.clone();
                        tokio::spawn(async move {
                            let mut inbound = inbound;
                            if let Err(err) = download_yuanbao_inbound_media(
                                &media_dir,
                                &mut inbound,
                                Some(token_manager.clone()),
                                Some(&api_domain),
                            )
                            .await
                            {
                                tracing::warn!(error = %err, "yuanbao inbound media download failed; using placeholders");
                            }
                            handle_inbound_message(
                                &inbound,
                                &ws_tx,
                                &process,
                                &bot_id,
                                token_manager,
                                &api_domain,
                                &render_options,
                                &acl,
                                &event_deduper,
                                &session_map,
                                &media_dir,
                            )
                            .await;
                        });
                    }
                }
            }

            FrameResult::Continue
        }
        _ => FrameResult::Continue,
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_inbound_message(
    inbound: &serde_json::Value,
    ws_tx: &Arc<Mutex<WsSink>>,
    process: &Arc<dyn Process>,
    bot_id: &str,
    token_manager: Arc<TokenManager>,
    api_domain: &str,
    render_options: &MessageRenderOptions,
    acl_metadata: &Arc<serde_json::Value>,
    event_deduper: &Arc<StdMutex<EventDeduper>>,
    session_map: &Arc<RwLock<HashMap<String, YuanbaoSessionRoute>>>,
    media_dir: &str,
) {
    if acl_metadata["accept_bot_messages"] != true && is_bot_message(inbound) {
        return;
    }
    let mut request = match parse_inbound_to_request(inbound, Some(bot_id)) {
        Ok(r) => r,
        Err(_) => return,
    };
    match check_duplicate_request(event_deduper, "yuanbao", &request) {
        Ok(true) => {
            tracing::debug!(
                message_id = ?request.message_id,
                "yuanbao duplicate event skipped"
            );
            return;
        }
        Ok(false) => {}
        Err(err) => {
            tracing::warn!(error = %err, "yuanbao event dedup failed");
        }
    }

    if let (Some(acl_obj), Some(meta)) =
        (acl_metadata.as_object(), request.metadata.as_object_mut())
    {
        for (k, v) in acl_obj {
            meta.insert(k.clone(), v.clone());
        }
    }

    let from_account = inbound
        .get("from_account")
        .or_else(|| inbound.get("fromAccount"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if from_account == bot_id {
        return;
    }

    tracing::info!(
        "yuanbao: inbound from={} session={}",
        &request.user_id,
        &request.session_id
    );

    let chat_type = request
        .metadata
        .get("chat_type")
        .and_then(|v| v.as_str())
        .unwrap_or("c2c")
        .to_string();
    let target_id = if chat_type == "group" {
        request
            .metadata
            .get("group_code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        from_account.to_string()
    };
    let bot_id_owned = bot_id.to_string();
    let group_code = if chat_type == "group" {
        Some(target_id.clone())
    } else {
        None
    };
    persist_session_route(session_map, media_dir, &request).await;

    let typing_task = start_typing_keepalive(
        ws_tx.clone(),
        bot_id_owned.clone(),
        from_account.to_string(),
        group_code.clone(),
    );

    let send_fn: pipeline::SendFn = {
        let ws_tx = ws_tx.clone();
        let chat_type = chat_type.clone();
        let target_id = target_id.clone();
        let bot_id = bot_id_owned.clone();
        let token_manager = token_manager.clone();
        let api_domain = api_domain.to_string();
        Arc::new(move |_target, msg| {
            let ws_tx = ws_tx.clone();
            let chat_type = chat_type.clone();
            let target_id = target_id.clone();
            let bot_id = bot_id.clone();
            let token_manager = token_manager.clone();
            let api_domain = api_domain.clone();
            Box::pin(async move {
                send_outgoing_to_route(
                    &ws_tx,
                    &chat_type,
                    &target_id,
                    &bot_id,
                    &token_manager,
                    &api_domain,
                    &msg.content,
                )
                .await
            })
        })
    };

    let renderer = Arc::new(MessageRenderer::with_options(
        RenderStyle::PlainText,
        render_options.clone(),
    ));
    if let Err(e) = pipeline::run_inbound(process.clone(), request, None, renderer, send_fn).await {
        tracing::error!("yuanbao: run_inbound error: {e}");
    }
    stop_typing_keepalive(
        typing_task,
        ws_tx.clone(),
        bot_id_owned,
        from_account.to_string(),
        group_code,
    )
    .await;
}

async fn send_media_part(
    ws_tx: &Arc<Mutex<WsSink>>,
    chat_type: &str,
    target_id: &str,
    bot_id: &str,
    token_manager: &Arc<TokenManager>,
    api_domain: &str,
    part: &OutgoingContentPart,
) -> Result<bool> {
    let Some(media_path) = outgoing_media_path(part) else {
        return Ok(false);
    };
    let client = default_http_client();
    let auth_headers = token_manager.auth_headers().await?;
    let result = download_and_upload_media(&media_path, &client, api_domain, &auth_headers).await?;
    let body = if result.mime_type.starts_with("image/") {
        build_image_msg_body(&result)
    } else {
        build_file_msg_body(&result)
    };
    let (raw, _) = if chat_type == "group" {
        build_send_group_msg(target_id, bot_id, &body)
    } else {
        build_send_c2c_msg(target_id, bot_id, &body)
    };
    ws_tx
        .lock()
        .await
        .send(Message::Binary(raw.into()))
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("yuanbao media send failed: {err}"),
        })?;
    Ok(true)
}

async fn send_outgoing_to_route(
    ws_tx: &SharedWsSink,
    chat_type: &str,
    target_id: &str,
    bot_id: &str,
    token_manager: &Arc<TokenManager>,
    api_domain: &str,
    content: &[OutgoingContentPart],
) -> Result<()> {
    let text = render_text_and_unresolved_media(content);
    for chunk in split_text(&text, TEXT_CHUNK_LIMIT) {
        let body = vec![(
            "TIMTextElem".to_string(),
            serde_json::json!({"text": chunk}),
        )];
        send_raw_msg_body(ws_tx, chat_type, target_id, bot_id, &body).await?;
    }

    for part in content {
        if !is_media_part(part) {
            continue;
        }
        match send_media_part(
            ws_tx,
            chat_type,
            target_id,
            bot_id,
            token_manager,
            api_domain,
            part,
        )
        .await
        {
            Ok(true) | Ok(false) => {}
            Err(err) => {
                tracing::warn!(error = %err, "yuanbao media send failed; sending fallback text");
                let fallback = render_content(std::slice::from_ref(part));
                if fallback.is_empty() {
                    continue;
                }
                let body = vec![(
                    "TIMTextElem".to_string(),
                    serde_json::json!({"text": fallback}),
                )];
                send_raw_msg_body(ws_tx, chat_type, target_id, bot_id, &body).await?;
            }
        }
    }
    Ok(())
}

async fn send_raw_msg_body(
    ws_tx: &SharedWsSink,
    chat_type: &str,
    target_id: &str,
    bot_id: &str,
    body: &[(String, serde_json::Value)],
) -> Result<()> {
    let (raw, _) = if chat_type == "group" {
        build_send_group_msg(target_id, bot_id, body)
    } else {
        build_send_c2c_msg(target_id, bot_id, body)
    };
    ws_tx
        .lock()
        .await
        .send(Message::Binary(raw.into()))
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("yuanbao send failed: {err}"),
        })
}

fn outgoing_media_path(part: &OutgoingContentPart) -> Option<String> {
    match part {
        OutgoingContentPart::Image { url, path, .. }
        | OutgoingContentPart::File { url, path, .. }
        | OutgoingContentPart::Audio { url, path, .. }
        | OutgoingContentPart::Video { url, path, .. } => path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .or_else(|| url.clone()),
        _ => None,
    }
}

fn is_media_part(part: &OutgoingContentPart) -> bool {
    matches!(
        part,
        OutgoingContentPart::Image { .. }
            | OutgoingContentPart::File { .. }
            | OutgoingContentPart::Audio { .. }
            | OutgoingContentPart::Video { .. }
    )
}

fn render_text_and_unresolved_media(parts: &[OutgoingContentPart]) -> String {
    let mut segments = Vec::new();
    for part in parts {
        match part {
            OutgoingContentPart::Text { text }
            | OutgoingContentPart::Markdown { text }
            | OutgoingContentPart::Refusal { text } => segments.push(text.clone()),
            OutgoingContentPart::Image { url, path, alt } => {
                if url.is_none() && path.is_none() {
                    segments.push(alt.as_deref().unwrap_or("[Image]").to_string());
                }
            }
            OutgoingContentPart::File { url, path, name } => {
                if url.is_none() && path.is_none() {
                    segments.push(format!("[File: {}]", name.as_deref().unwrap_or("file")));
                }
            }
            OutgoingContentPart::Audio { url, path, name } => {
                if url.is_none() && path.is_none() {
                    segments.push(format!("[Audio: {}]", name.as_deref().unwrap_or("audio")));
                }
            }
            OutgoingContentPart::Video { url, path, name } => {
                if url.is_none() && path.is_none() {
                    segments.push(format!("[Video: {}]", name.as_deref().unwrap_or("video")));
                }
            }
            OutgoingContentPart::Tool { name, status, .. } => {
                segments.push(format!("[Tool: {name} - {status:?}]"));
            }
            OutgoingContentPart::Raw { value } => segments.push(value.to_string()),
            OutgoingContentPart::Thinking { .. } => {}
        }
    }
    segments.join("\n")
}

async fn resolve_send_route(
    session_map: &Arc<RwLock<HashMap<String, YuanbaoSessionRoute>>>,
    to: &ToHandle,
) -> Option<YuanbaoSessionRoute> {
    let candidates = send_route_keys(to);
    let map = session_map.read().await;
    candidates
        .iter()
        .find_map(|key| map.get(key).cloned())
        .filter(|route| !route.target_id().is_empty())
}

fn send_route_keys(to: &ToHandle) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(session_id) = to
        .metadata
        .get("session_id")
        .and_then(|value| value.as_str())
    {
        push_route_key(&mut keys, session_id);
    }
    push_route_key(&mut keys, &to.conversation_id);
    if let Some(sender_id) = to.sender_id.as_deref() {
        push_route_key(&mut keys, sender_id);
    }
    keys
}

fn push_route_key(keys: &mut Vec<String>, raw: &str) {
    let key = raw.trim();
    if key.is_empty() {
        return;
    }
    if !keys.iter().any(|existing| existing == key) {
        keys.push(key.to_string());
    }
    if let Some(stripped) = key.strip_prefix("yuanbao:") {
        if !stripped.is_empty() && !keys.iter().any(|existing| existing == stripped) {
            keys.push(stripped.to_string());
        }
    }
}

async fn persist_session_route(
    session_map: &Arc<RwLock<HashMap<String, YuanbaoSessionRoute>>>,
    media_dir: &str,
    request: &AgentRequest,
) {
    let Some((key, route)) = route_from_request(request) else {
        return;
    };
    {
        let mut map = session_map.write().await;
        map.insert(key, route);
        if let Err(err) = save_session_map(media_dir, &map) {
            tracing::debug!(error = %err, "yuanbao: save session map failed");
        }
    }
}

fn persist_session_route_sync(
    session_map: &Arc<RwLock<HashMap<String, YuanbaoSessionRoute>>>,
    media_dir: &str,
    request: &AgentRequest,
) {
    let Some((key, route)) = route_from_request(request) else {
        return;
    };
    let Ok(mut map) = session_map.try_write() else {
        return;
    };
    map.insert(key, route);
    if let Err(err) = save_session_map(media_dir, &map) {
        tracing::debug!(error = %err, "yuanbao: save session map failed");
    }
}

fn route_from_request(request: &AgentRequest) -> Option<(String, YuanbaoSessionRoute)> {
    let meta = request.metadata.as_object()?;
    let session_id = meta
        .get("session_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            request
                .session_id
                .strip_prefix("yuanbao:")
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| request.session_id.clone());
    if session_id.trim().is_empty() {
        return None;
    }
    let chat_type = meta
        .get("chat_type")
        .and_then(|value| value.as_str())
        .unwrap_or("c2c")
        .to_string();
    let sender_id = meta
        .get("from_account")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let group_code = meta
        .get("group_code")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    if (chat_type == "group" && group_code.is_empty())
        || (chat_type != "group" && sender_id.is_empty())
    {
        return None;
    }
    let route = YuanbaoSessionRoute {
        chat_type,
        sender_id,
        group_code,
        msg_id: request.message_id.clone().unwrap_or_default(),
    };
    Some((session_id, route))
}

fn load_session_map(media_dir: &str) -> HashMap<String, YuanbaoSessionRoute> {
    let path = session_map_path(media_dir);
    let Ok(bytes) = std::fs::read(&path) else {
        return HashMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_session_map(
    media_dir: &str,
    map: &HashMap<String, YuanbaoSessionRoute>,
) -> std::io::Result<()> {
    let path = session_map_path(media_dir);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "yuanbao_sessions.json".into()),
        uuid::Uuid::new_v4()
    ));
    let bytes = serde_json::to_vec_pretty(map)?;
    {
        let mut file = std::fs::File::create(&tmp_path)?;
        use std::io::Write;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp_path, &path)?;
    let _ = std::fs::File::open(parent).and_then(|file| file.sync_all());
    Ok(())
}

fn session_map_path(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    let base = if media_dir.is_empty() {
        std::env::temp_dir().join("rua-yuanbao-media")
    } else {
        PathBuf::from(media_dir)
    };
    base.parent()
        .map(|parent| parent.join("yuanbao_sessions.json"))
        .unwrap_or_else(|| PathBuf::from("yuanbao_sessions.json"))
}

fn parse_inbound_to_request(
    native: &serde_json::Value,
    bot_id: Option<&str>,
) -> Result<AgentRequest> {
    let callback_command = native
        .get("callback_command")
        .or_else(|| native.get("callbackCommand"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let from_account = native
        .get("from_account")
        .or_else(|| native.get("fromAccount"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let group_code = native
        .get("group_code")
        .or_else(|| native.get("groupCode"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let nickname = native
        .get("sender_nickname")
        .or_else(|| native.get("senderNickname"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let msg_id = native
        .get("msg_id")
        .or_else(|| native.get("msgId"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let is_group = callback_command.starts_with("Group.");
    let session_id = if is_group && !group_code.is_empty() {
        group_code.to_string()
    } else {
        short_id(from_account)
    };
    let conversation_id = format!("yuanbao:{session_id}");
    let sender_display = sender_display_name(nickname, from_account);

    let quoted_prefix = quoted_prefix_from_native(native);
    let mut text_parts = Vec::new();
    let mut media_parts = Vec::new();
    let mut mentioned = false;
    let mut mention_user_id = String::new();
    if let Some(body) = native.get("msg_body").or_else(|| native.get("msgBody")) {
        if let Some(arr) = body.as_array() {
            for elem in arr {
                let msg_type = elem
                    .get("msg_type")
                    .or_else(|| elem.get("msgType"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if msg_type == "TIMTextElem" {
                    let content = elem
                        .get("msg_content")
                        .or_else(|| elem.get("msgContent"))
                        .cloned()
                        .unwrap_or_default();
                    if let Some(text) = content.get("text").and_then(|v| v.as_str()) {
                        let t = text.trim();
                        if !t.is_empty() {
                            text_parts.push(t.to_string());
                        }
                    }
                } else if msg_type == "TIMCustomElem" {
                    if let Some(user_id) = custom_mention_user_id(elem) {
                        if mention_user_id.is_empty() {
                            mention_user_id = user_id.clone();
                        }
                        mentioned |= bot_id.is_none_or(|id| id == user_id);
                    }
                } else if msg_type == "TIMImageElem" {
                    let content = elem
                        .get("msg_content")
                        .or_else(|| elem.get("msgContent"))
                        .cloned()
                        .unwrap_or_default();
                    media_parts.push(yuanbao_media_part(&content, "image", "image.jpg"));
                } else if msg_type == "TIMFileElem" {
                    let content = elem
                        .get("msg_content")
                        .or_else(|| elem.get("msgContent"))
                        .cloned()
                        .unwrap_or_default();
                    let filename =
                        extract_nonempty(&content, &["file_name", "fileName"]).unwrap_or("file");
                    let kind = if is_audio_filename(filename) {
                        "audio"
                    } else {
                        "file"
                    };
                    media_parts.push(yuanbao_media_part(&content, kind, filename));
                }
            }
        }
    }

    let text = text_parts.join(" ");
    let input_text = match (quoted_prefix, text.trim().is_empty()) {
        (Some(prefix), true) => Some(prefix),
        (Some(prefix), false) => Some(format!("{prefix}\n{text}")),
        (None, false) => Some(text),
        (None, true) => None,
    };

    let mut input = Vec::new();
    if let Some(input_text) = input_text.filter(|text| !text.trim().is_empty()) {
        input.push(IncomingContentPart::Text { text: input_text });
    }
    input.extend(media_parts);
    input.retain(|part| match part {
        IncomingContentPart::Text { text } => !text.trim().is_empty(),
        _ => true,
    });
    if input.is_empty() {
        return Err(rua_core::RuaError::Validation {
            message: "yuanbao: no text content in message".into(),
        });
    }

    Ok(AgentRequest {
        agent_id: String::new(),
        user_id: sender_display.clone(),
        session_id: conversation_id.clone(),
        input,
        channel_id: "yuanbao".into(),
        conversation_id,
        message_id: if msg_id.is_empty() {
            None
        } else {
            Some(msg_id.to_string())
        },
        metadata: serde_json::json!({
            "chat_type": if is_group { "group" } else { "c2c" },
            "group_code": group_code,
            "from_account": from_account,
            "is_group": is_group,
            "mentioned": mentioned,
            "mention_user_id": mention_user_id,
            "session_id": session_id,
            "user_name": sender_display,
        }),
    })
}

fn check_duplicate_request(
    deduper: &Arc<StdMutex<EventDeduper>>,
    channel: &str,
    req: &AgentRequest,
) -> Result<bool> {
    let Some(message_id) = req
        .message_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(false);
    };
    let mut deduper = deduper.lock().map_err(|_| rua_core::RuaError::Runtime {
        message: format!("{channel} event deduper lock poisoned"),
    })?;
    Ok(
        deduper.check_and_mark(format!("{channel}:message:{message_id}"))
            == EventDedupDecision::Duplicate,
    )
}

fn yuanbao_media_part(
    content: &serde_json::Value,
    kind: &str,
    filename: &str,
) -> IncomingContentPart {
    if let Some(part) = yuanbao_local_media_part(content, kind, filename) {
        return part;
    }
    let has_url = yuanbao_media_url(content).is_some();
    IncomingContentPart::Text {
        text: if has_url {
            format!("[{kind}: download failed]")
        } else {
            format!("[{kind}: no url]")
        },
    }
}

fn yuanbao_local_media_part(
    content: &serde_json::Value,
    kind: &str,
    filename: &str,
) -> Option<IncomingContentPart> {
    let path =
        extract_nonempty(content, &["local_path", "localPath", "path"]).map(PathBuf::from)?;
    let file_id = yuanbao_media_url(content)
        .or_else(|| extract_nonempty(content, &["uuid", "file_uuid", "fileUuid"]))
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let mime = extract_nonempty(content, &["mime", "mime_type", "mimeType"]).map(ToOwned::to_owned);
    match kind {
        "image" => Some(IncomingContentPart::Image {
            file_id,
            path,
            mime,
        }),
        "audio" => Some(IncomingContentPart::Audio {
            file_id,
            path,
            mime,
            name: Some(filename.to_string()),
        }),
        _ => Some(IncomingContentPart::File {
            file_id,
            path,
            mime,
            name: Some(filename.to_string()),
        }),
    }
}

fn yuanbao_media_url(content: &serde_json::Value) -> Option<&str> {
    extract_nonempty(content, &["url"]).or_else(|| {
        content
            .get("image_info_array")
            .or_else(|| content.get("imageInfoArray"))
            .and_then(|value| value.as_array())
            .and_then(|items| {
                items.iter().find_map(|item| {
                    item.get("url")
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                })
            })
    })
}

fn extract_nonempty<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

async fn download_yuanbao_inbound_media(
    media_dir: &str,
    native: &mut serde_json::Value,
    token_manager: Option<Arc<TokenManager>>,
    api_domain: Option<&str>,
) -> Result<()> {
    let body = if native.get("msg_body").is_some() {
        native
            .get_mut("msg_body")
            .and_then(|value| value.as_array_mut())
    } else {
        native
            .get_mut("msgBody")
            .and_then(|value| value.as_array_mut())
    };
    let Some(body) = body else {
        return Ok(());
    };
    for elem in body {
        let msg_type = elem
            .get("msg_type")
            .or_else(|| elem.get("msgType"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        if !matches!(msg_type.as_str(), "TIMImageElem" | "TIMFileElem") {
            continue;
        }
        let filename = elem
            .get("msg_content")
            .or_else(|| elem.get("msgContent"))
            .and_then(|content| extract_nonempty(content, &["file_name", "fileName"]))
            .unwrap_or(if msg_type == "TIMImageElem" {
                "image.jpg"
            } else {
                "file"
            })
            .to_string();
        let content = if elem.get("msg_content").is_some() {
            elem.get_mut("msg_content")
        } else {
            elem.get_mut("msgContent")
        };
        if let Some(content) = content {
            download_yuanbao_media_object(
                media_dir,
                content,
                &filename,
                token_manager.as_ref(),
                api_domain,
            )
            .await?;
        }
    }
    Ok(())
}

async fn download_yuanbao_media_object(
    media_dir: &str,
    content: &mut serde_json::Value,
    filename: &str,
    token_manager: Option<&Arc<TokenManager>>,
    api_domain: Option<&str>,
) -> Result<()> {
    if extract_nonempty(content, &["local_path", "localPath", "path"]).is_some() {
        return Ok(());
    }
    let Some(url) = yuanbao_media_url(content).map(ToOwned::to_owned) else {
        return Ok(());
    };
    let resolved_url = resolve_yuanbao_download_url(&url, token_manager, api_domain).await?;
    let Some((bytes, mime)) = yuanbao_download_url(&resolved_url).await? else {
        return Ok(());
    };
    let dir = yuanbao_media_dir(media_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("create yuanbao media dir {}: {err}", dir.display()),
        })?;
    let safe_name = crate::core::media::sanitize_filename(filename);
    let final_path = dir.join(format!("{}_{}", uuid::Uuid::new_v4(), safe_name));
    atomic_write_file(&final_path, &bytes).await?;
    if let Some(obj) = content.as_object_mut() {
        obj.insert(
            "local_path".to_string(),
            serde_json::Value::String(final_path.to_string_lossy().to_string()),
        );
        if let Some(mime) = mime {
            obj.insert("mime".to_string(), serde_json::Value::String(mime));
        }
    }
    Ok(())
}

async fn resolve_yuanbao_download_url(
    media_url: &str,
    token_manager: Option<&Arc<TokenManager>>,
    api_domain: Option<&str>,
) -> Result<String> {
    let Some(resource_id) = yuanbao_resource_id(media_url) else {
        return Ok(media_url.to_string());
    };
    let Some(token_manager) = token_manager else {
        return Ok(media_url.to_string());
    };
    let Some(api_domain) = api_domain else {
        return Ok(media_url.to_string());
    };
    let auth_headers = token_manager.auth_headers().await?;
    let domain = api_domain
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let url = format!("https://{domain}/api/resource/v1/download");
    let client = default_http_client();
    let mut request = client
        .get(url)
        .query(&[("resourceId", resource_id.as_str())]);
    for (key, value) in auth_headers {
        request = request.header(key, value);
    }
    let resp = match request.timeout(Duration::from_secs(30)).send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(error = %err, "yuanbao download URL resolve failed");
            return Ok(media_url.to_string());
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "yuanbao download URL resolve non-success");
        return Ok(media_url.to_string());
    }
    let mut body: serde_json::Value =
        resp.json()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("yuanbao download URL resolve parse failed: {err}"),
            })?;
    if body.get("data").is_some_and(|value| value.is_object()) {
        body = body["data"].clone();
    }
    Ok(extract_nonempty(&body, &["url", "realUrl"])
        .unwrap_or(media_url)
        .to_string())
}

fn yuanbao_resource_id(media_url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(media_url).ok()?;
    parsed.query_pairs().find_map(|(key, value)| {
        if key == "resourceId" && !value.trim().is_empty() {
            Some(value.into_owned())
        } else {
            None
        }
    })
}

async fn yuanbao_download_url(url: &str) -> Result<Option<(Vec<u8>, Option<String>)>> {
    let resp = match default_http_client()
        .get(url)
        .timeout(Duration::from_secs(60))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%url, error = %err, "yuanbao media download failed");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(%url, status = %resp.status(), "yuanbao media download non-success");
        return Ok(None);
    }
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_INBOUND_MEDIA_SIZE {
            tracing::warn!(%url, len, "yuanbao media too large");
            return Ok(None);
        }
    }
    let mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let bytes = resp
        .bytes()
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("yuanbao media download bytes failed: {err}"),
        })?;
    if bytes.len() > MAX_INBOUND_MEDIA_SIZE {
        tracing::warn!(%url, len = bytes.len(), "yuanbao media too large");
        return Ok(None);
    }
    Ok(Some((bytes.to_vec(), mime)))
}

fn yuanbao_media_dir(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    if media_dir.is_empty() {
        std::env::temp_dir().join("rua-yuanbao-media")
    } else {
        PathBuf::from(media_dir)
    }
}

async fn atomic_write_file(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| rua_core::RuaError::Io {
        message: format!("missing parent dir for {}", path.display()),
    })?;
    let tmp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "yuanbao-media".into()),
        uuid::Uuid::new_v4()
    ));
    let mut file =
        tokio::fs::File::create(&tmp_path)
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("create temp media file {}: {err}", tmp_path.display()),
            })?;
    file.write_all(bytes)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("write temp media file {}: {err}", tmp_path.display()),
        })?;
    file.sync_all()
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("sync temp media file {}: {err}", tmp_path.display()),
        })?;
    drop(file);
    tokio::fs::rename(&tmp_path, path)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!(
                "rename temp media file {} to {}: {err}",
                tmp_path.display(),
                path.display()
            ),
        })?;
    let _ = std::fs::File::open(parent).and_then(|file| file.sync_all());
    Ok(())
}

fn custom_mention_user_id(elem: &serde_json::Value) -> Option<String> {
    let content = elem.get("msg_content").or_else(|| elem.get("msgContent"))?;
    let data = content.get("data").and_then(|v| v.as_str())?;
    let parsed = serde_json::from_str::<serde_json::Value>(data).ok()?;
    if parsed.get("elem_type").and_then(|v| v.as_i64())? != 1002 {
        return None;
    }
    parsed
        .get("user_id")
        .and_then(|v| v.as_str())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn render_content(parts: &[OutgoingContentPart]) -> String {
    let mut segments = Vec::new();
    for part in parts {
        match part {
            OutgoingContentPart::Text { text } | OutgoingContentPart::Markdown { text } => {
                segments.push(text.clone());
            }
            OutgoingContentPart::Refusal { text } => segments.push(text.clone()),
            OutgoingContentPart::Image { alt, .. } => {
                segments.push(alt.as_deref().unwrap_or("[Image]").to_string());
            }
            OutgoingContentPart::File { name, .. } => {
                segments.push(format!("[File: {}]", name.as_deref().unwrap_or("file")));
            }
            _ => {}
        }
    }
    segments.join("\n")
}

fn quoted_prefix_from_native(native: &serde_json::Value) -> Option<String> {
    let cloud_custom_data = native
        .get("cloud_custom_data")
        .or_else(|| native.get("cloudCustomData"))?;
    match cloud_custom_data {
        serde_json::Value::Object(map) => map.get("quote").and_then(build_quoted_prefix),
        serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(s)
            .ok()
            .and_then(|parsed| parsed.get("quote").cloned())
            .and_then(|quote| build_quoted_prefix(&quote)),
        _ => None,
    }
}

fn build_quoted_prefix(quote: &serde_json::Value) -> Option<String> {
    let obj = quote.as_object()?;
    let quote_type = obj.get("type").and_then(|v| v.as_i64()).unwrap_or(1);
    let desc = obj
        .get("desc")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let label = match quote_type {
        2 => "image",
        3 if is_audio_filename(desc) => "audio",
        3 => "file",
        _ => "message",
    };
    if desc.is_empty() {
        Some(format!("[quoted {label}]"))
    } else {
        Some(format!("[quoted {label}: {desc}]"))
    }
}

fn is_audio_filename(filename: &str) -> bool {
    let lower = filename.to_ascii_lowercase();
    matches!(
        lower.rsplit('.').next(),
        Some("mp3" | "wav" | "m4a" | "aac" | "ogg" | "flac" | "amr")
    )
}

fn start_typing_keepalive(
    ws_tx: Arc<Mutex<WsSink>>,
    from_account: String,
    to_account: String,
    group_code: Option<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(TYPING_KEEPALIVE_INTERVAL_SECS));
        loop {
            send_typing_heartbeat(
                &ws_tx,
                &from_account,
                &to_account,
                group_code.as_deref(),
                HEARTBEAT_RUNNING,
            )
            .await;
            interval.tick().await;
        }
    })
}

async fn stop_typing_keepalive(
    task: JoinHandle<()>,
    ws_tx: Arc<Mutex<WsSink>>,
    from_account: String,
    to_account: String,
    group_code: Option<String>,
) {
    task.abort();
    let _ = task.await;
    send_typing_heartbeat(
        &ws_tx,
        &from_account,
        &to_account,
        group_code.as_deref(),
        HEARTBEAT_FINISH,
    )
    .await;
}

async fn send_typing_heartbeat(
    ws_tx: &Arc<Mutex<WsSink>>,
    from_account: &str,
    to_account: &str,
    group_code: Option<&str>,
    heartbeat: u32,
) {
    if to_account.is_empty() {
        return;
    }
    let (raw, _) = build_heartbeat_msg(from_account, to_account, heartbeat, group_code);
    if ws_tx
        .lock()
        .await
        .send(Message::Binary(raw.into()))
        .await
        .is_err()
    {
        tracing::debug!("yuanbao: typing heartbeat send failed");
    }
}

fn short_id(raw_id: &str) -> String {
    let n = 8;
    if raw_id.len() >= n {
        raw_id[raw_id.len() - n..].to_string()
    } else {
        raw_id.to_string()
    }
}

fn sender_display_name(nickname: &str, raw_sender_id: &str) -> String {
    let nick = if nickname.trim().is_empty() {
        "unknown"
    } else {
        nickname.trim()
    };
    let suffix = if raw_sender_id.len() >= 4 {
        &raw_sender_id[raw_sender_id.len() - 4..]
    } else if raw_sender_id.is_empty() {
        "????"
    } else {
        raw_sender_id
    };
    format!("{nick}#{suffix}")
}

fn is_bot_message(native: &serde_json::Value) -> bool {
    native
        .get("from_account")
        .or_else(|| native.get("fromAccount"))
        .and_then(|v| v.as_str())
        .is_some_and(|id| id.starts_with("bot_"))
        || native["msg_body"].as_array().is_some_and(|parts| {
            parts.iter().any(|part| {
                part["msg_type"] == "TIMTextElem"
                    && part
                        .pointer("/msg_content/data")
                        .and_then(|v| v.as_str())
                        .and_then(|data| serde_json::from_str::<serde_json::Value>(data).ok())
                        .is_some_and(|data| data["elem_type"] == 1013)
            })
        })
}
