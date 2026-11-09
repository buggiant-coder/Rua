use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::content::{IncomingContentPart, OutgoingContentPart};
use crate::core::event_dedup::{EventDedupDecision, EventDeduper};
use crate::core::pipeline;
use crate::core::renderer::{ChannelKind, MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::streaming::ProcessingIndicator;
use crate::core::{BufferDebouncer, ChannelId};
use crate::wechat::client::ILinkClient;
use rua_core::Result;

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(800);
const MAX_INBOUND_MEDIA_SIZE: usize = 50 * 1024 * 1024;
const TYPING_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

pub struct WechatTypingIndicator {
    client: ILinkClient,
    to_user_id: String,
    context_token: String,
    ticket: Mutex<Option<String>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl WechatTypingIndicator {
    pub fn new(client: ILinkClient, to_user_id: String, context_token: String) -> Self {
        Self {
            client,
            to_user_id,
            context_token,
            ticket: Mutex::new(None),
            task: Mutex::new(None),
        }
    }

    async fn fetch_ticket(&self) -> Result<Option<String>> {
        let resp = self
            .client
            .getconfig(&self.to_user_id, &self.context_token)
            .await?;
        let ret = resp
            .get("ret")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        let errcode = resp
            .get("errcode")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if ret != 0 || errcode != 0 {
            tracing::debug!(%ret, %errcode, "wechat getconfig did not return typing ticket");
            return Ok(None);
        }
        Ok(resp
            .get("typing_ticket")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string))
    }
}

#[async_trait]
impl ProcessingIndicator for WechatTypingIndicator {
    async fn start(&self, _req: &AgentRequest) -> Result<()> {
        self.stop(_req).await?;
        let Some(ticket) = self.fetch_ticket().await? else {
            return Ok(());
        };
        self.client.sendtyping(&self.to_user_id, &ticket, 1).await?;
        *self.ticket.lock().await = Some(ticket.clone());

        let client = self.client.clone();
        let to_user_id = self.to_user_id.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(TYPING_KEEPALIVE_INTERVAL).await;
                if let Err(err) = client.sendtyping(&to_user_id, &ticket, 1).await {
                    tracing::debug!(error = %err, "wechat typing keepalive failed");
                }
            }
        });
        *self.task.lock().await = Some(handle);
        Ok(())
    }

    async fn stop(&self, _req: &AgentRequest) -> Result<()> {
        if let Some(handle) = self.task.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }
        if let Some(ticket) = self.ticket.lock().await.take() {
            let _ = self.client.sendtyping(&self.to_user_id, &ticket, 2).await;
        }
        Ok(())
    }
}

pub struct WechatChannel {
    channel_id: ChannelId,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    bot_token: String,
    base_url: Option<String>,
    stop_tx: watch::Sender<bool>,
    poll_handle: RwLock<Option<JoinHandle<()>>>,
    cancel: RwLock<tokio_util::sync::CancellationToken>,
    outgoing_merge: tokio::sync::Mutex<Option<Arc<super::merge::ReplyMerger>>>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    debouncer: BufferDebouncer<String, AgentRequest>,
    media_dir: String,
    event_deduper: Arc<StdMutex<EventDeduper>>,
}

impl WechatChannel {
    pub fn new(bot_token: String, base_url: Option<String>) -> Self {
        let (stop_tx, _) = watch::channel(false);
        let render_options = MessageRenderOptions::for_channel_kind(ChannelKind::Weixin);
        Self {
            channel_id: "wechat".into(),
            process: RwLock::new(None),
            started: RwLock::new(false),
            bot_token,
            base_url,
            stop_tx,
            poll_handle: RwLock::new(None),
            cancel: RwLock::new(tokio_util::sync::CancellationToken::new()),
            outgoing_merge: tokio::sync::Mutex::new(None),
            render_options,
            acl_metadata: serde_json::json!({}),
            debouncer: BufferDebouncer::new(DEBOUNCE_WINDOW),
            media_dir: String::new(),
            event_deduper: Arc::new(StdMutex::new(EventDeduper::new(
                EVENT_DEDUP_TTL,
                EVENT_DEDUP_CAPACITY,
            ))),
        }
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

    pub fn with_media_dir(mut self, media_dir: String) -> Self {
        self.media_dir = media_dir;
        self
    }

    pub async fn download_inbound_media(&self, native: &mut serde_json::Value) -> Result<()> {
        let client = ILinkClient::new(self.bot_token.clone(), self.base_url.clone());
        download_wechat_inbound_media(&client, &self.media_dir, native).await
    }

    fn render_content(parts: &[OutgoingContentPart]) -> String {
        let mut segments = Vec::new();
        for part in parts {
            match part {
                OutgoingContentPart::Text { text } => segments.push(text.clone()),
                OutgoingContentPart::Markdown { text } => segments.push(text.clone()),
                OutgoingContentPart::Refusal { text } => segments.push(text.clone()),
                OutgoingContentPart::Tool { name, status, .. } => {
                    segments.push(format!("[Tool: {name} - {status:?}]"));
                }
                OutgoingContentPart::Image { alt, .. } => {
                    segments.push(alt.as_deref().unwrap_or("[Image]").to_string());
                }
                OutgoingContentPart::File { name, .. } => {
                    let label = name.as_deref().unwrap_or("file");
                    segments.push(format!("[File: {label}]"));
                }
                OutgoingContentPart::Audio { name, .. } => {
                    if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
                        segments.push(format!("[Audio: {label}]"));
                    } else {
                        segments.push("[Audio]".to_string());
                    }
                }
                OutgoingContentPart::Video { name, .. } => {
                    if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
                        segments.push(format!("[Video: {label}]"));
                    } else {
                        segments.push("[Video]".to_string());
                    }
                }
                OutgoingContentPart::Raw { value } => {
                    segments.push(value.to_string());
                }
                OutgoingContentPart::Thinking { .. } => {}
            }
        }
        segments.join("\n")
    }

    fn render_text_and_unresolved_media(parts: &[OutgoingContentPart]) -> String {
        let mut segments = Vec::new();
        for part in parts {
            match part {
                OutgoingContentPart::Text { text }
                | OutgoingContentPart::Markdown { text }
                | OutgoingContentPart::Refusal { text } => segments.push(text.clone()),
                OutgoingContentPart::Tool { name, status, .. } => {
                    segments.push(format!("[Tool: {name} - {status:?}]"));
                }
                OutgoingContentPart::Image { path, alt, .. } => {
                    if path.is_none() {
                        segments.push(alt.as_deref().unwrap_or("[Image]").to_string());
                    }
                }
                OutgoingContentPart::File { path, name, .. } => {
                    if path.is_none() {
                        let label = name.as_deref().unwrap_or("file");
                        segments.push(format!("[File: {label}]"));
                    }
                }
                OutgoingContentPart::Audio { path, name, .. } => {
                    if path.is_none() {
                        if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
                            segments.push(format!("[Audio: {label}]"));
                        } else {
                            segments.push("[Audio]".to_string());
                        }
                    }
                }
                OutgoingContentPart::Video { path, name, .. } => {
                    if path.is_none() {
                        if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
                            segments.push(format!("[Video: {label}]"));
                        } else {
                            segments.push("[Video]".to_string());
                        }
                    }
                }
                OutgoingContentPart::Raw { value } => segments.push(value.to_string()),
                OutgoingContentPart::Thinking { .. } => {}
            }
        }
        segments.join("\n")
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

    async fn send_media_part(
        client: &ILinkClient,
        to_user_id: &str,
        context_token: &str,
        part: &OutgoingContentPart,
    ) -> Result<bool> {
        match part {
            OutgoingContentPart::Image {
                path: Some(path), ..
            } => {
                client.send_image(to_user_id, path, context_token).await?;
                Ok(true)
            }
            OutgoingContentPart::File {
                path: Some(path),
                name,
                ..
            } => {
                let filename = media_filename(path, name.as_deref());
                client
                    .send_file(to_user_id, path, &filename, context_token)
                    .await?;
                Ok(true)
            }
            OutgoingContentPart::Audio {
                path: Some(path),
                name,
                ..
            } => {
                let filename = media_filename(path, name.as_deref());
                client
                    .send_audio(to_user_id, path, &filename, context_token)
                    .await?;
                Ok(true)
            }
            OutgoingContentPart::Video {
                path: Some(path), ..
            } => {
                client.send_video(to_user_id, path, context_token).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn send_content(
        client: &ILinkClient,
        to_user_id: &str,
        context_token: &str,
        parts: &[OutgoingContentPart],
    ) -> Result<()> {
        let text = Self::render_text_and_unresolved_media(parts);
        if !text.is_empty() {
            let resp = client.sendmessage(to_user_id, &text, context_token).await?;
            let ret = resp.get("ret").and_then(|v| v.as_i64()).unwrap_or(0);
            if ret != 0 {
                tracing::warn!("wechat send ret={ret}: {resp}");
            }
        }
        for part in parts {
            if !Self::is_media_part(part) {
                continue;
            }
            match Self::send_media_part(client, to_user_id, context_token, part).await {
                Ok(true) => {}
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "wechat media send failed; sending fallback text");
                    let fallback = Self::render_content(std::slice::from_ref(part));
                    if !fallback.is_empty() {
                        let resp = client
                            .sendmessage(to_user_id, &fallback, context_token)
                            .await?;
                        let ret = resp.get("ret").and_then(|v| v.as_i64()).unwrap_or(0);
                        if ret != 0 {
                            tracing::warn!("wechat fallback send ret={ret}: {resp}");
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn media_filename(path: &Path, name: Option<&str>) -> String {
    name.filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            path.file_name()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "file".to_string())
}

#[async_trait]
impl channel::Channel for WechatChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("wechat channel already started");
            return Ok(());
        }
        *self.process.write().await = Some(process.clone());
        *started = true;

        let mut client = ILinkClient::new(self.bot_token.clone(), self.base_url.clone());

        if client.bot_token.is_empty() {
            tracing::info!("wechat: no bot_token, starting QR code login…");
            let qr = client.get_bot_qrcode().await?;
            let scan_url = qr.url.unwrap_or_else(|| qr.qrcode_img_content.clone());
            tracing::info!("wechat: scan QR code to log in: {scan_url}");
            let (token, base_url) = client.wait_for_login(&qr.qrcode).await?;
            client.bot_token = token;
            client.base_url = base_url.trim_end_matches('/').to_string();
            tracing::info!("wechat: QR code login succeeded");
        }

        let process_for_poll = process;
        let debouncer = self.debouncer.clone();
        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::Markdown,
            self.render_options.clone(),
        ));
        let acl_metadata = Arc::new(self.acl_metadata.clone());

        let send_fn: pipeline::SendFn = Arc::new({
            let client = client.clone();
            move |to, msg| {
                let client = client.clone();
                Box::pin(async move {
                    let context_token = to
                        .metadata
                        .get("wechat_context_token")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let to_user_id = to
                        .metadata
                        .get("wechat_from_user_id")
                        .and_then(|v| v.as_str())
                        .or(to.sender_id.as_deref())
                        .unwrap_or_default();
                    WechatChannel::send_content(&client, to_user_id, context_token, &msg.content)
                        .await
                })
            }
        });

        self.stop_tx.send_replace(false);
        let mut stop_rx = self.stop_tx.subscribe();
        let cancel = tokio_util::sync::CancellationToken::new();
        *self.cancel.write().await = cancel.clone();
        let poll_client = client.clone();
        let media_dir = self.media_dir.clone();
        let event_deduper = self.event_deduper.clone();
        let handle = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            let mut cursor = String::new();
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = stop_rx.changed() => break,
                    _ = tasks.join_next(), if !tasks.is_empty() => {},
                    result = poll_client.getupdates(&cursor) => {
                        match result {
                            Ok(data) => {
                                if let Some(new_cursor) = data.get_updates_buf {
                                    cursor = new_cursor;
                                }
                                    if let Some(msgs) = data.msgs {
                                        for mut msg in msgs {
                                            let msg_type = msg.get("message_type")
                                                .and_then(|v| v.as_i64())
                                                .unwrap_or(0);
                                            if msg_type != 1 {
                                                continue;
                                            }
                                            if let Err(err) = download_wechat_inbound_media(&poll_client, &media_dir, &mut msg).await {
                                                tracing::warn!(error = %err, "wechat inbound media download failed; using placeholders");
                                            }
                                            let mut req = match build_request_from_native_value(msg) {
                                                Some(req) => req,
                                                None => continue,
                                            };
                                            match wechat_check_duplicate_request(&event_deduper, &req) {
                                                Ok(true) => {
                                                    tracing::debug!(
                                                        message_id = ?req.message_id,
                                                        "wechat duplicate event skipped"
                                                    );
                                                    continue;
                                                }
                                                Ok(false) => {}
                                                Err(err) => {
                                                    tracing::warn!(error = %err, "wechat event dedup failed");
                                                }
                                            }
                                            let from_user_id = req.user_id.clone();

                                        if let (Some(acl_obj), Some(meta)) =
                                            (acl_metadata.as_object(), req.metadata.as_object_mut())
                                        {
                                            for (k, v) in acl_obj {
                                                meta.insert(k.clone(), v.clone());
                                            }
                                        }

                                        let p = process_for_poll.clone();
                                        let d = debouncer.clone();
                                        let r = renderer.clone();
                                        let sf = send_fn.clone();
                                        let indicator_client = poll_client.clone();
                                        let context_token = req
                                            .metadata
                                            .get("wechat_context_token")
                                            .and_then(|value| value.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let indicator = Arc::new(WechatTypingIndicator::new(
                                            indicator_client,
                                            from_user_id.clone(),
                                            context_token,
                                        ));
                                        let sender_key = from_user_id.clone();
                                        tasks.spawn(async move {
                                            let merger = (req.metadata["message_merge_enabled"] == true).then(|| {
                                                super::merge::ReplyMerger::new(sf.clone(), req.metadata["message_merge_delay_ms"].as_u64().unwrap_or(0))
                                            });
                                            let sf = merger.as_ref().map(|merger|merger.sender()).unwrap_or(sf);
                                            if let Err(e) =
                                                pipeline::run_inbound_streaming_with_indicator(
                                                    p,
                                                    req,
                                                    Some((d, sender_key)),
                                                    r,
                                                    sf,
                                                    None,
                                                    Some(indicator),
                                                )
                                                .await
                                            {
                                                tracing::error!("wechat pipeline error: {e}");
                                            }
                                            if let Some(merger) = merger {
                                                if let Err(error) = merger.finish().await {
                                                    tracing::warn!(%error, "WeChat reply flush failed");
                                                }
                                            }
                                        });
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("wechat getupdates error: {e}");
                                tokio::select! {
                                    _ = cancel.cancelled() => break,
                                    _ = tokio::time::sleep(Duration::from_secs(5)) => {},
                                }
                            }
                        }
                    }
                }
            }
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        });

        *self.poll_handle.write().await = Some(handle);
        tracing::info!("wechat channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut started = self.started.write().await;
        self.cancel.read().await.cancel();
        self.stop_tx.send_replace(true);
        let flush = if let Some(merger) = self.outgoing_merge.lock().await.take() {
            merger.finish().await
        } else {
            Ok(())
        };
        if let Some(mut handle) = self.poll_handle.write().await.take() {
            if tokio::time::timeout(Duration::from_secs(5), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
                let _ = handle.await;
            }
        }
        if let Ok(mut deduper) = self.event_deduper.lock() {
            deduper.clear();
        }
        *self.process.write().await = None;
        *started = false;
        tracing::info!("wechat channel stopped");
        flush
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        let context_token = to
            .metadata
            .get("wechat_context_token")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let to_user_id = to
            .metadata
            .get("wechat_from_user_id")
            .and_then(|v| v.as_str())
            .or(to.sender_id.as_deref())
            .ok_or_else(|| rua_core::RuaError::Validation {
                message: "missing wechat_from_user_id in target metadata".into(),
            })?;
        let client = ILinkClient::new(self.bot_token.clone(), self.base_url.clone());
        if self.acl_metadata["message_merge_enabled"] == true
            && self.acl_metadata["message_merge_delay_ms"]
                .as_u64()
                .unwrap_or(0)
                > 0
        {
            let mut outgoing = self.outgoing_merge.lock().await;
            let merger = outgoing.get_or_insert_with(|| {
                let send: pipeline::SendFn = Arc::new(move |to, msg| {
                    let client = client.clone();
                    Box::pin(async move {
                        let receiver = to.metadata["wechat_from_user_id"]
                            .as_str()
                            .or(to.sender_id.as_deref())
                            .unwrap_or_default();
                        let token = to.metadata["wechat_context_token"]
                            .as_str()
                            .unwrap_or_default();
                        Self::send_content(&client, receiver, token, &msg.content).await
                    })
                });
                super::merge::ReplyMerger::new(
                    send,
                    self.acl_metadata["message_merge_delay_ms"]
                        .as_u64()
                        .unwrap_or(0),
                )
            });
            merger.sender()(to, msg).await
        } else {
            Self::send_content(&client, to_user_id, context_token, &msg.content).await
        }
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        let req = build_request_from_native_value(native).ok_or_else(|| {
            rua_core::RuaError::Validation {
                message: "wechat native payload has no parseable content".into(),
            }
        })?;
        if wechat_check_duplicate_request(&self.event_deduper, &req)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate wechat event".into(),
            });
        }
        Ok(req)
    }
}

fn wechat_check_duplicate_request(
    deduper: &Arc<StdMutex<EventDeduper>>,
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
        message: "wechat event deduper lock poisoned".into(),
    })?;
    Ok(
        deduper.check_and_mark(format!("wechat:message:{message_id}"))
            == EventDedupDecision::Duplicate,
    )
}

fn build_request_from_native_value(native: serde_json::Value) -> Option<AgentRequest> {
    let from_user_id = native
        .get("from_user_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let context_token = native
        .get("context_token")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let group_id = native
        .get("group_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let mut text_parts = Vec::new();
    let mut media_parts = Vec::new();
    if let Some(items) = native.get("item_list").and_then(|v| v.as_array()) {
        for item in items {
            match item.get("type").and_then(|v| v.as_i64()).unwrap_or(0) {
                1 => {
                    if let Some(text) = item
                        .get("text_item")
                        .and_then(|v| v.get("text"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                    {
                        text_parts.push(text.to_string());
                    }
                }
                2 => media_parts.push(wechat_media_part(
                    item.get("image_item").and_then(|value| value.get("media")),
                    "image",
                    "image.jpg",
                )),
                3 => {
                    let voice = item.get("voice_item").unwrap_or(&serde_json::Value::Null);
                    let text = voice
                        .get("text_item")
                        .and_then(|value| value.get("text"))
                        .and_then(|value| value.as_str())
                        .or_else(|| voice.get("text").and_then(|value| value.as_str()))
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .unwrap_or("[voice: no transcription]");
                    text_parts.push(text.to_string());
                }
                4 => {
                    let file_item = item.get("file_item").unwrap_or(&serde_json::Value::Null);
                    let filename = extract_nonempty(file_item, &["file_name", "filename", "name"])
                        .unwrap_or("file.bin");
                    media_parts.push(wechat_media_part(file_item.get("media"), "file", filename));
                }
                5 => media_parts.push(wechat_media_part(
                    item.get("video_item").and_then(|value| value.get("media")),
                    "video",
                    "video.mp4",
                )),
                other => text_parts.push(format!("[unsupported type: {other}]")),
            }
        }
    }

    let mut input = Vec::new();
    let text = text_parts.join("\n");
    if !text.trim().is_empty() {
        input.push(IncomingContentPart::Text { text });
    }
    input.extend(media_parts);
    input.retain(|part| match part {
        IncomingContentPart::Text { text } => !text.trim().is_empty(),
        _ => true,
    });
    if input.is_empty() {
        return None;
    }

    let is_group = !group_id.is_empty();
    let session_id = if is_group {
        format!("wechat:group:{group_id}")
    } else {
        format!("wechat:{from_user_id}")
    };
    Some(AgentRequest {
        agent_id: String::new(),
        user_id: if is_group {
            String::new()
        } else {
            from_user_id.clone()
        },
        session_id: session_id.clone(),
        input,
        channel_id: "wechat".into(),
        conversation_id: session_id,
        message_id: if context_token.is_empty() {
            None
        } else {
            Some(context_token.clone())
        },
        metadata: serde_json::json!({
            "wechat_from_user_id": from_user_id,
            "wechat_context_token": context_token,
            "wechat_group_id": group_id,
            "is_group": is_group,
        }),
    })
}

fn wechat_media_part(
    media: Option<&serde_json::Value>,
    kind: &str,
    filename: &str,
) -> IncomingContentPart {
    if let Some(part) = wechat_local_media_part(media, kind, filename) {
        return part;
    }
    let text = if media
        .and_then(|media| extract_nonempty(media, &["url", "encrypt_query_param"]))
        .is_some()
    {
        format!("[{kind}: download failed]")
    } else {
        format!("[{kind}: no url]")
    };
    IncomingContentPart::Text { text }
}

fn wechat_local_media_part(
    media: Option<&serde_json::Value>,
    kind: &str,
    filename: &str,
) -> Option<IncomingContentPart> {
    let media = media?;
    let path = extract_nonempty(media, &["local_path", "path"]).map(PathBuf::from)?;
    let file_id = extract_nonempty(media, &["url", "media_id", "encrypt_query_param"])
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let mime =
        extract_nonempty(media, &["mime", "mime_type", "content_type"]).map(ToOwned::to_owned);
    match kind {
        "image" => Some(IncomingContentPart::Image {
            file_id,
            path,
            mime,
        }),
        "video" => Some(IncomingContentPart::Video {
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

fn extract_nonempty<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

async fn download_wechat_inbound_media(
    client: &ILinkClient,
    media_dir: &str,
    native: &mut serde_json::Value,
) -> Result<()> {
    let Some(items) = native
        .get_mut("item_list")
        .and_then(|value| value.as_array_mut())
    else {
        return Ok(());
    };
    for item in items {
        match item
            .get("type")
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
        {
            2 => {
                let aes_key = item
                    .get("image_item")
                    .and_then(|value| {
                        extract_nonempty(value, &["aeskey"]).or_else(|| {
                            value
                                .get("media")
                                .and_then(|media| extract_nonempty(media, &["aes_key", "aesKey"]))
                        })
                    })
                    .unwrap_or("")
                    .to_string();
                if let Some(media) = item
                    .get_mut("image_item")
                    .and_then(|value| value.get_mut("media"))
                {
                    download_wechat_media_object(client, media_dir, media, "image.jpg", &aes_key)
                        .await?;
                }
            }
            4 => {
                let filename = item
                    .get("file_item")
                    .and_then(|value| extract_nonempty(value, &["file_name", "filename", "name"]))
                    .unwrap_or("file.bin")
                    .to_string();
                if let Some(media) = item
                    .get_mut("file_item")
                    .and_then(|value| value.get_mut("media"))
                {
                    let aes_key = extract_nonempty(media, &["aes_key", "aesKey"])
                        .unwrap_or("")
                        .to_string();
                    download_wechat_media_object(client, media_dir, media, &filename, &aes_key)
                        .await?;
                }
            }
            5 => {
                if let Some(media) = item
                    .get_mut("video_item")
                    .and_then(|value| value.get_mut("media"))
                {
                    let aes_key = extract_nonempty(media, &["aes_key", "aesKey"])
                        .unwrap_or("")
                        .to_string();
                    download_wechat_media_object(client, media_dir, media, "video.mp4", &aes_key)
                        .await?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

async fn download_wechat_media_object(
    client: &ILinkClient,
    media_dir: &str,
    media: &mut serde_json::Value,
    filename_hint: &str,
    aes_key: &str,
) -> Result<()> {
    if extract_nonempty(media, &["local_path", "path"]).is_some() {
        return Ok(());
    }
    let url = extract_nonempty(media, &["url"]).unwrap_or("").to_string();
    let encrypt_query_param = extract_nonempty(media, &["encrypt_query_param"])
        .unwrap_or("")
        .to_string();
    if url.is_empty() && encrypt_query_param.is_empty() {
        return Ok(());
    }
    let (bytes, mime) = match client
        .download_media(&url, aes_key, &encrypt_query_param)
        .await
    {
        Ok(download) => download,
        Err(err) => {
            tracing::warn!(error = %err, "wechat media download failed");
            return Ok(());
        }
    };
    if bytes.len() > MAX_INBOUND_MEDIA_SIZE {
        tracing::warn!(len = bytes.len(), "wechat media too large");
        return Ok(());
    }
    let dir = wechat_media_dir(media_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("create wechat media dir {}: {err}", dir.display()),
        })?;
    let filename = crate::core::media::sanitize_filename(filename_hint);
    let final_path = dir.join(format!("{}_{}", uuid::Uuid::new_v4(), filename));
    atomic_write_file(&final_path, &bytes).await?;
    if let Some(obj) = media.as_object_mut() {
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

fn wechat_media_dir(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    if media_dir.is_empty() {
        std::env::temp_dir().join("rua-wechat-media")
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
            .unwrap_or_else(|| "wechat-media".into()),
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
