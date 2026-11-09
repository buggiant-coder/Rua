use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::content::{IncomingContentPart, OutgoingContentPart};
use crate::core::event_dedup::{EventDedupDecision, EventDeduper};
use crate::core::pipeline;
use crate::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::ChannelId;
use crate::http_client::default_http_client;
use rua_core::Result;

use crate::onebot::types::OneBotConfig;
use crate::onebot::ws;

const MAX_RETRIES: u32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INBOUND_MEDIA_SIZE: u64 = 50 * 1024 * 1024;
const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

pub struct OneBotChannel {
    channel_id: ChannelId,
    config: OneBotConfig,
    client: reqwest::Client,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    ws_cancel: RwLock<Option<CancellationToken>>,
    ws_handle: RwLock<Option<JoinHandle<()>>>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    media_dir: String,
    rpc: ws::WsRpc,
    event_deduper: Arc<StdMutex<EventDeduper>>,
}

impl OneBotChannel {
    pub fn new(config: OneBotConfig) -> Self {
        let media_dir = config.media_dir.clone();
        Self {
            channel_id: "onebot".into(),
            config,
            client: default_http_client(),
            process: RwLock::new(None),
            started: RwLock::new(false),
            ws_cancel: RwLock::new(None),
            ws_handle: RwLock::new(None),
            render_options: MessageRenderOptions::default(),
            acl_metadata: serde_json::json!({}),
            media_dir,
            rpc: ws::WsRpc::default(),
            event_deduper: Arc::new(StdMutex::new(EventDeduper::new(
                EVENT_DEDUP_TTL,
                EVENT_DEDUP_CAPACITY,
            ))),
        }
    }

    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
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
        download_onebot_inbound_media_with(
            &self.client,
            &self.config.http_url,
            self.config.access_token.as_deref(),
            &self.media_dir,
            native,
            media_download_limit(&self.acl_metadata),
        )
        .await
    }

    async fn post_send_msg(&self, body: serde_json::Value, connection: Option<&str>) -> Result<()> {
        if self.config.http_url.is_empty() {
            return self
                .rpc
                .call_on(connection, "send_msg", body)
                .await
                .map(|_| ());
        }

        let url = format!("{}/send_msg", self.config.http_url.trim_end_matches('/'));
        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << (attempt - 1));
                tracing::debug!(
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying onebot send_msg"
                );
                tokio::time::sleep(delay).await;
            }

            let mut req = self.client.post(&url).json(&body).timeout(REQUEST_TIMEOUT);
            if let Some(token) = &self.config.access_token {
                req = req.header("Authorization", format!("Bearer {token}"));
            }

            match req.send().await {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(attempt, "onebot send_msg succeeded");
                    return Ok(());
                }
                Ok(resp) => {
                    let status = resp.status();
                    let resp_body = resp.text().await.unwrap_or_default();
                    tracing::warn!(attempt, %status, body = %resp_body, "onebot send_msg non-success");
                    last_error = Some(format!("HTTP {status}: {resp_body}"));
                }
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "onebot send_msg failed");
                    last_error = Some(e.to_string());
                }
            }
        }

        Err(rua_core::RuaError::Runtime {
            message: format!(
                "onebot send_msg failed after {MAX_RETRIES} retries: {}",
                last_error.unwrap_or_default()
            ),
        })
    }

    async fn post_api(
        &self,
        path: &str,
        body: serde_json::Value,
        connection: Option<&str>,
    ) -> Result<()> {
        if self.config.http_url.is_empty() {
            return self.rpc.call_on(connection, path, body).await.map(|_| ());
        }

        let url = format!(
            "{}/{}",
            self.config.http_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let mut req = self.client.post(&url).json(&body).timeout(REQUEST_TIMEOUT);
        if let Some(token) = &self.config.access_token {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let resp = req
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("onebot {path} failed: {err}"),
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(rua_core::RuaError::Runtime {
                message: format!("onebot {path} HTTP {status}: {text}"),
            });
        }
        Ok(())
    }
}

fn onebot_check_duplicate_request(
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
        message: "onebot event deduper lock poisoned".into(),
    })?;
    Ok(
        deduper.check_and_mark(format!("onebot:message:{message_id}"))
            == EventDedupDecision::Duplicate,
    )
}

#[async_trait]
impl channel::Channel for OneBotChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("onebot channel already started");
            return Ok(());
        }
        let host = self.config.ws_host.trim_matches(['[', ']']);
        let loopback = host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        if !loopback
            && self
                .config
                .access_token
                .as_deref()
                .is_none_or(|token| token.trim().is_empty())
        {
            return Err(rua_core::RuaError::Validation {
                message: "OneBot non-loopback ws_host requires access_token".into(),
            });
        }
        let cancel = CancellationToken::new();

        let ws_config = ws::WsServerConfig {
            host: self.config.ws_host.clone(),
            port: self.config.ws_port,
            access_token: self.config.access_token.clone(),
        };
        let process_for_ws = process;
        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::Markdown,
            self.render_options.clone(),
        ));
        let acl_metadata = Arc::new(self.acl_metadata.clone());
        let send_fn: pipeline::SendFn = Arc::new({
            let mut sender = Self::new(self.config.clone())
                .with_client(self.client.clone())
                .with_acl_config(self.acl_metadata.clone());
            sender.rpc = self.rpc.clone();
            let sender = Arc::new(sender);
            move |to, msg| {
                let sender = sender.clone();
                Box::pin(async move { channel::Channel::send(sender.as_ref(), to, msg).await })
            }
        });
        let media_client = self.client.clone();
        let media_http_url = self.config.http_url.clone();
        let media_access_token = self.config.access_token.clone();
        let media_dir = self.media_dir.clone();
        let event_deduper = self.event_deduper.clone();
        let on_message: ws::AsyncOnMessageFn = Arc::new(move |native| {
            let p = process_for_ws.clone();
            let r = renderer.clone();
            let sf = send_fn.clone();
            let dl_client = media_client.clone();
            let dl_http_url = media_http_url.clone();
            let dl_token = media_access_token.clone();
            let dl_media_dir = media_dir.clone();
            let acl = acl_metadata.clone();
            let event_deduper = event_deduper.clone();
            Box::pin(async move {
                let mut native = native;
                if let Err(err) = download_onebot_inbound_media_with(
                    &dl_client,
                    &dl_http_url,
                    dl_token.as_deref(),
                    &dl_media_dir,
                    &mut native,
                    media_download_limit(&acl),
                )
                .await
                {
                    tracing::warn!(error = %err, "onebot inbound media download failed; using URL fallbacks");
                }
                match parse_onebot_event(native) {
                    Ok(Some((mut req, media))) => {
                        match onebot_check_duplicate_request(&event_deduper, &req) {
                            Ok(true) => {
                                tracing::debug!(
                                    message_id = ?req.message_id,
                                    "onebot duplicate event skipped"
                                );
                                return;
                            }
                            Ok(false) => {}
                            Err(err) => {
                                tracing::warn!(error = %err, "onebot event dedup failed");
                            }
                        }
                        for seg in &media {
                            if let Some(part) = download_onebot_media(
                                &dl_client,
                                &dl_http_url,
                                dl_token.as_deref(),
                                &dl_media_dir,
                                seg,
                                media_download_limit(&acl),
                            )
                            .await
                            {
                                req.input.push(part);
                            } else {
                                req.input.push(IncomingContentPart::Url {
                                    url: seg.url.clone(),
                                    title: seg.name.clone(),
                                });
                            }
                        }
                        if let (Some(acl_obj), Some(meta)) =
                            (acl.as_object(), req.metadata.as_object_mut())
                        {
                            for (k, v) in acl_obj {
                                meta.insert(k.clone(), v.clone());
                            }
                        }
                        if let Err(e) = pipeline::run_inbound(p, req, None, r, sf).await {
                            tracing::error!("onebot pipeline error: {e}");
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::error!("onebot WS: parse failed: {e}");
                    }
                }
            })
        });
        let handle =
            ws::start_ws_server(ws_config, on_message, cancel.clone(), self.rpc.clone()).await?;
        *self.ws_handle.write().await = Some(handle);
        *self.ws_cancel.write().await = Some(cancel);
        *started = true;
        tracing::info!("onebot channel started with reverse WS server");

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut started = self.started.write().await;
        if !*started {
            return Ok(());
        }

        if let Some(cancel) = self.ws_cancel.write().await.take() {
            cancel.cancel();
        }
        if let Some(mut handle) = self.ws_handle.write().await.take() {
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
        self.rpc.clear().await;
        *self.process.write().await = None;
        *started = false;
        tracing::info!("onebot channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, mut msg: OutMessage) -> Result<()> {
        for part in &mut msg.content {
            match part {
                OutgoingContentPart::Image { url, path, .. }
                | OutgoingContentPart::Audio { url, path, .. }
                | OutgoingContentPart::Video { url, path, .. }
                | OutgoingContentPart::File { url, path, .. } => {
                    if let Some(reference) = onebot_media_file(url.as_deref(), path.as_ref()) {
                        *url = Some(normalize_media_reference(reference, &self.acl_metadata).await);
                    }
                }
                _ => {}
            }
        }
        let connection = to.metadata["_onebot_connection_id"].as_str();
        for part in &msg.content {
            if let OutgoingContentPart::File { url, path, name } = part {
                if let Some(file) = onebot_media_file(url.as_deref(), path.as_ref()) {
                    let name = name.as_deref().unwrap_or("file");
                    let message_type = resolve_onebot_message_type(&to, &msg);
                    let body = if message_type == "group" {
                        serde_json::json!({
                            "group_id": resolve_onebot_group_id(&to, &msg).unwrap_or(0),
                            "file": file,
                            "name": name,
                        })
                    } else {
                        serde_json::json!({
                            "user_id": resolve_onebot_user_id(&to, &msg).unwrap_or(0),
                            "file": file,
                            "name": name,
                        })
                    };
                    let path = if message_type == "group" {
                        "upload_group_file"
                    } else {
                        "upload_private_file"
                    };
                    self.post_api(path, body, connection).await?;
                }
            }
        }
        let Some(body) = build_onebot_send_body(&to, &msg) else {
            tracing::debug!("empty content, skipping onebot send");
            return Ok(());
        };

        self.post_send_msg(body, connection).await
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        let (mut req, media) =
            parse_onebot_event(native)?.ok_or_else(|| rua_core::RuaError::Validation {
                message: "onebot event produced no request (self message or unsupported type)"
                    .into(),
            })?;
        if onebot_check_duplicate_request(&self.event_deduper, &req)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate onebot event".into(),
            });
        }
        for seg in media {
            req.input.push(IncomingContentPart::Url {
                url: seg.url,
                title: seg.name,
            });
        }
        Ok(req)
    }
}

fn build_onebot_send_body(to: &ToHandle, msg: &OutMessage) -> Option<serde_json::Value> {
    let message = build_onebot_message_segments(&msg.content);
    if message.is_empty() {
        return None;
    }

    let message_type = resolve_onebot_message_type(to, msg);
    let mut body = serde_json::json!({
        "message_type": message_type,
        "message": message,
    });

    if message_type == "group" {
        body["group_id"] = serde_json::json!(resolve_onebot_group_id(to, msg).unwrap_or(0));
    } else {
        body["user_id"] = serde_json::json!(resolve_onebot_user_id(to, msg).unwrap_or(0));
    }

    Some(body)
}

fn build_onebot_message_segments(parts: &[OutgoingContentPart]) -> Vec<serde_json::Value> {
    let mut segments = Vec::new();
    for part in parts {
        match part {
            OutgoingContentPart::Text { text }
            | OutgoingContentPart::Markdown { text }
            | OutgoingContentPart::Refusal { text } => {
                push_text_segment(&mut segments, text);
            }
            OutgoingContentPart::Tool { name, status, .. } => {
                push_text_segment(&mut segments, &format!("[Tool: {name} - {status:?}]"));
            }
            OutgoingContentPart::Image { url, path, alt } => {
                if let Some(file) = url
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .or_else(|| {
                        path.as_ref()
                            .map(|p| p.to_string_lossy().into_owned())
                            .filter(|s| !s.is_empty())
                    })
                {
                    segments.push(serde_json::json!({
                        "type": "image",
                        "data": { "file": file }
                    }));
                } else if let Some(alt) = alt.as_deref() {
                    push_text_segment(&mut segments, alt);
                }
            }
            OutgoingContentPart::File { url, path, name } => {
                if onebot_media_file(url.as_deref(), path.as_ref()).is_none() {
                    let label = name.as_deref().unwrap_or("file");
                    push_text_segment(&mut segments, &format!("[File: {label}]"));
                }
            }
            OutgoingContentPart::Audio { url, path, name } => {
                if let Some(file) = onebot_media_file(url.as_deref(), path.as_ref()) {
                    segments.push(serde_json::json!({
                        "type": "record",
                        "data": { "file": file }
                    }));
                } else if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
                    push_text_segment(&mut segments, &format!("[Audio: {label}]"));
                } else {
                    push_text_segment(&mut segments, "[Audio]");
                }
            }
            OutgoingContentPart::Video { url, path, name } => {
                if let Some(file) = onebot_media_file(url.as_deref(), path.as_ref()) {
                    segments.push(serde_json::json!({
                        "type": "video",
                        "data": { "file": file }
                    }));
                } else if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
                    push_text_segment(&mut segments, &format!("[Video: {label}]"));
                } else {
                    push_text_segment(&mut segments, "[Video]");
                }
            }
            OutgoingContentPart::Raw { value } => {
                push_text_segment(&mut segments, &value.to_string());
            }
            OutgoingContentPart::Thinking { .. } => {}
        }
    }
    segments
}

fn onebot_media_file(url: Option<&str>, path: Option<&std::path::PathBuf>) -> Option<String> {
    url.filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            path.map(|path| path.to_string_lossy().into_owned())
                .filter(|value| !value.trim().is_empty())
        })
}

fn push_text_segment(segments: &mut Vec<serde_json::Value>, text: &str) {
    if !text.is_empty() {
        segments.push(serde_json::json!({
            "type": "text",
            "data": { "text": text }
        }));
    }
}

fn resolve_onebot_message_type(to: &ToHandle, msg: &OutMessage) -> &'static str {
    let explicit = msg
        .metadata
        .get("message_type")
        .or_else(|| to.metadata.get("message_type"))
        .and_then(|v| v.as_str());
    match explicit {
        Some("group") => "group",
        Some(_) => "private",
        None if to.conversation_id.starts_with("group_") => "group",
        None => "private",
    }
}

fn resolve_onebot_group_id(to: &ToHandle, msg: &OutMessage) -> Option<i64> {
    first_i64(&msg.metadata, &["group_id"])
        .or_else(|| first_i64(&to.metadata, &["group_id"]))
        .or_else(|| prefixed_i64(&to.conversation_id, "group_"))
}

fn resolve_onebot_user_id(to: &ToHandle, msg: &OutMessage) -> Option<i64> {
    first_i64(&msg.metadata, &["user_id", "sender_id"])
        .or_else(|| first_i64(&to.metadata, &["user_id", "sender_id"]))
        .or_else(|| to.sender_id.as_deref().and_then(parse_i64))
        .or_else(|| msg.receiver_id.as_deref().and_then(parse_i64))
        .or_else(|| prefixed_i64(&to.conversation_id, "private_"))
}

fn first_i64(metadata: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .filter_map(|key| metadata.get(*key))
        .find_map(value_as_i64)
}

fn value_as_i64(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(parse_i64))
}

fn prefixed_i64(value: &str, prefix: &str) -> Option<i64> {
    value.strip_prefix(prefix).and_then(parse_i64)
}

fn parse_i64(value: &str) -> Option<i64> {
    value.parse::<i64>().ok()
}

fn parse_onebot_event(
    native: serde_json::Value,
) -> Result<Option<(AgentRequest, Vec<MediaSegment>)>> {
    let post_type = native
        .get("post_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if post_type != "message" {
        tracing::debug!(post_type, "onebot: skipping non-message event");
        return Ok(None);
    }

    let message_type = native
        .get("message_type")
        .and_then(|v| v.as_str())
        .unwrap_or("private")
        .to_string();

    let user_id = native.get("user_id").and_then(|v| v.as_i64()).unwrap_or(0);

    let group_id = native.get("group_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let user_name = native
        .get("sender")
        .and_then(|v| v.as_object())
        .and_then(|sender| {
            sender
                .get("card")
                .or_else(|| sender.get("nickname"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .trim()
        .to_string();

    let message_id = native
        .get("message_id")
        .and_then(|v| v.as_i64())
        .map(|id| id.to_string());

    let self_id = native.get("self_id").and_then(|v| v.as_i64());
    let (input, media, bot_mentioned) = extract_content_parts(&native, self_id);
    if input.is_empty() && media.is_empty() {
        return Ok(None);
    }

    let conversation_id = if message_type == "group" {
        format!("group_{group_id}")
    } else {
        format!("private_{user_id}")
    };

    Ok(Some((
        AgentRequest {
            agent_id: String::new(),
            user_id: user_id.to_string(),
            session_id: conversation_id.clone(),
            input,
            channel_id: "onebot".into(),
            conversation_id,
            message_id,
            metadata: serde_json::json!({
                "_onebot_connection_id": native["_onebot_connection_id"],
                "message_type": message_type,
                "user_id": user_id,
                "sender_id": user_id.to_string(),
                "group_id": group_id,
                "is_group": group_id != 0,
                "mentioned": bot_mentioned,
                "bot_mentioned": bot_mentioned,
                "user_name": if user_name.is_empty() { user_id.to_string() } else { user_name },
            }),
        },
        media,
    )))
}

struct MediaSegment {
    seg_type: String,
    url: String,
    name: Option<String>,
}

fn extract_content_parts(
    native: &serde_json::Value,
    self_id: Option<i64>,
) -> (Vec<IncomingContentPart>, Vec<MediaSegment>, bool) {
    let message = match native.get("message") {
        Some(v) => v,
        None => {
            return (Vec::new(), Vec::new(), false);
        }
    };

    if let Some(s) = message.as_str() {
        if s.trim().is_empty() {
            return (Vec::new(), Vec::new(), false);
        }
        return (
            vec![IncomingContentPart::Text {
                text: s.to_string(),
            }],
            Vec::new(),
            false,
        );
    }

    let arr = match message.as_array() {
        Some(a) => a,
        None => return (Vec::new(), Vec::new(), false),
    };

    let mut text_parts = Vec::new();
    let mut direct_media = Vec::new();
    let mut media = Vec::new();
    let mut bot_mentioned = false;

    for seg in arr {
        let seg_type = seg.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let data = match seg.get("data") {
            Some(d) => d,
            None => continue,
        };

        match seg_type {
            "text" => {
                if let Some(t) = data.get("text").and_then(|v| v.as_str()) {
                    if !t.trim().is_empty() {
                        text_parts.push(t.to_string());
                    }
                }
            }
            "image" | "record" | "video" | "file" => {
                if let Some(part) = onebot_local_media_part(seg_type, data) {
                    direct_media.push(part);
                    continue;
                }
                let url = data
                    .get("url")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("file").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                if !url.is_empty() {
                    let name = data
                        .get("name")
                        .or_else(|| data.get("file"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    media.push(MediaSegment {
                        seg_type: seg_type.to_string(),
                        url,
                        name,
                    });
                }
            }
            "at" => {
                if let Some(id) = self_id {
                    let at_matches = data
                        .get("qq")
                        .and_then(value_as_i64)
                        .is_some_and(|qq| qq == id);
                    bot_mentioned |= at_matches;
                }
            }
            _ => {}
        }
    }

    let mut content = if text_parts.is_empty() {
        Vec::new()
    } else {
        vec![IncomingContentPart::Text {
            text: text_parts.join(""),
        }]
    };
    content.extend(direct_media);

    (content, media, bot_mentioned)
}

async fn download_onebot_media(
    client: &reqwest::Client,
    http_url: &str,
    access_token: Option<&str>,
    media_dir: &str,
    seg: &MediaSegment,
    max_bytes: u64,
) -> Option<IncomingContentPart> {
    let download_url = if seg.url.starts_with("http://") || seg.url.starts_with("https://") {
        seg.url.clone()
    } else if !http_url.is_empty() {
        let api = match seg.seg_type.as_str() {
            "image" => "get_image",
            "record" => "get_record",
            "video" => "get_image",
            "file" => "get_image",
            _ => return None,
        };
        format!(
            "{}/{}?file={}",
            http_url.trim_end_matches('/'),
            api,
            seg.url
        )
    } else {
        return None;
    };

    let dir = onebot_media_dir(media_dir);
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        tracing::warn!("onebot: failed to create temp media dir");
        return None;
    }

    let mut req = client.get(&download_url).timeout(REQUEST_TIMEOUT);
    // OneBot credentials belong to its API, not third-party media URLs.
    if let Some(token) = access_token.filter(|_| {
        !http_url.is_empty()
            && download_url.starts_with(&format!("{}/", http_url.trim_end_matches('/')))
    }) {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    let resp = match req.send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            tracing::warn!(
                status = %r.status(),
                url = %download_url,
                "onebot: media download failed"
            );
            return None;
        }
        Err(e) => {
            tracing::warn!(error = %e, url = %download_url, "onebot: media download error");
            return None;
        }
    };

    let mime = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if let Some(len) = resp.content_length() {
        if len > max_bytes {
            tracing::warn!(len, "onebot: media too large");
            return None;
        }
    }

    let mut resp = resp;
    let mut bytes = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() as u64 + chunk.len() as u64 > max_bytes {
                    tracing::warn!("onebot: media too large");
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(%error, "onebot: media body read error");
                return None;
            }
        }
    }

    let ext = mime.as_deref().and_then(mime_to_ext).unwrap_or("bin");
    let fallback_name = format!("media.{ext}");
    let name = seg.name.as_deref().unwrap_or(&fallback_name);
    let file_id = seg
        .name
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().as_simple().to_string());
    let file_path = dir.join(crate::core::media::sanitize_filename(name));

    if let Err(e) = atomic_write_file(&file_path, &bytes).await {
        tracing::warn!(error = %e, "onebot: failed to write media file");
        return None;
    }

    match seg.seg_type.as_str() {
        "image" => Some(IncomingContentPart::Image {
            file_id,
            path: file_path,
            mime,
        }),
        _ => Some(IncomingContentPart::File {
            file_id,
            path: file_path,
            mime,
            name: Some(name.to_string()),
        }),
    }
}

async fn download_onebot_inbound_media_with(
    client: &reqwest::Client,
    http_url: &str,
    access_token: Option<&str>,
    media_dir: &str,
    native: &mut serde_json::Value,
    max_bytes: u64,
) -> Result<()> {
    let Some(message) = native
        .get_mut("message")
        .and_then(|value| value.as_array_mut())
    else {
        return Ok(());
    };
    for seg in message {
        let seg_type = seg
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        if !matches!(seg_type.as_str(), "image" | "record" | "video" | "file") {
            continue;
        }
        let Some(data) = seg.get_mut("data") else {
            continue;
        };
        if data
            .get("local_path")
            .and_then(|value| value.as_str())
            .is_some()
        {
            continue;
        }
        let url = data
            .get("url")
            .and_then(|value| value.as_str())
            .or_else(|| data.get("file").and_then(|value| value.as_str()))
            .unwrap_or("")
            .to_string();
        if url.is_empty() {
            continue;
        }
        let name = data
            .get("name")
            .or_else(|| data.get("file"))
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned);
        let media = MediaSegment {
            seg_type: seg_type.clone(),
            url,
            name: name.clone(),
        };
        if let Some(part) =
            download_onebot_media(client, http_url, access_token, media_dir, &media, max_bytes)
                .await
        {
            let (path, mime, file_id, name) = match part {
                IncomingContentPart::Image {
                    file_id,
                    path,
                    mime,
                } => (path, mime, file_id, None),
                IncomingContentPart::Audio {
                    file_id,
                    path,
                    mime,
                    name,
                }
                | IncomingContentPart::Video {
                    file_id,
                    path,
                    mime,
                    name,
                }
                | IncomingContentPart::File {
                    file_id,
                    path,
                    mime,
                    name,
                } => (path, mime, file_id, name),
                _ => continue,
            };
            if let Some(obj) = data.as_object_mut() {
                obj.insert(
                    "local_path".to_string(),
                    serde_json::Value::String(path.to_string_lossy().to_string()),
                );
                obj.insert("file_id".to_string(), serde_json::Value::String(file_id));
                if let Some(mime) = mime {
                    obj.insert("mime".to_string(), serde_json::Value::String(mime));
                }
                if let Some(name) = name {
                    obj.insert("name".to_string(), serde_json::Value::String(name));
                }
            }
        }
    }
    Ok(())
}

fn onebot_local_media_part(
    seg_type: &str,
    data: &serde_json::Value,
) -> Option<IncomingContentPart> {
    let path = data
        .get("local_path")
        .or_else(|| data.get("path"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)?;
    let file_id = data
        .get("file_id")
        .or_else(|| data.get("file"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let mime = data
        .get("mime")
        .or_else(|| data.get("mime_type"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let name = data
        .get("name")
        .or_else(|| data.get("file"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    match seg_type {
        "image" => Some(IncomingContentPart::Image {
            file_id,
            path,
            mime,
        }),
        "record" => Some(IncomingContentPart::Audio {
            file_id,
            path,
            mime,
            name,
        }),
        "video" => Some(IncomingContentPart::Video {
            file_id,
            path,
            mime,
            name,
        }),
        "file" => Some(IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        }),
        _ => None,
    }
}

fn onebot_media_dir(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    if media_dir.is_empty() {
        std::env::temp_dir().join("rua-onebot-media")
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
            .unwrap_or_else(|| "onebot-media".into()),
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

fn mime_to_ext(mime: &str) -> Option<&'static str> {
    let base = mime.split(';').next().unwrap_or(mime).trim();
    match base {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/wav" => Some("wav"),
        "audio/silk" | "audio/amr" => Some("amr"),
        "video/mp4" => Some("mp4"),
        "application/octet-stream" => None,
        _ => None,
    }
}

fn media_download_limit(options: &serde_json::Value) -> u64 {
    options["media_download_max_mb"]
        .as_u64()
        .filter(|n| *n > 0)
        .map(|n| n.saturating_mul(1_000_000))
        .unwrap_or(MAX_INBOUND_MEDIA_SIZE)
}

async fn normalize_media_reference(reference: String, options: &serde_json::Value) -> String {
    use base64::Engine;
    use tokio::io::AsyncReadExt;

    if reference.starts_with("base64://") {
        return reference;
    }
    if reference.starts_with("data:") {
        return reference
            .split_once(";base64,")
            .map(|(_, data)| format!("base64://{data}"))
            .unwrap_or(reference);
    }
    if options["media_base64"] != true {
        return reference;
    }
    let path = if reference.starts_with("file:") {
        match url::Url::parse(&reference)
            .ok()
            .and_then(|u| u.to_file_path().ok())
        {
            Some(path) => path,
            None => return reference,
        }
    } else if reference.contains("://") {
        return reference;
    } else {
        PathBuf::from(&reference)
    };
    let max_bytes = options["media_base64_max_mb"]
        .as_u64()
        .filter(|n| *n > 0)
        .unwrap_or(10)
        .saturating_mul(1_000_000);
    let Ok(file) = tokio::fs::File::open(&path).await else {
        return reference;
    };
    if !file
        .metadata()
        .await
        .is_ok_and(|meta| meta.is_file() && meta.len() <= max_bytes)
    {
        return reference;
    }
    let mut bytes = Vec::new();
    if file
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .is_err()
        || bytes.len() as u64 > max_bytes
    {
        return reference;
    }
    format!(
        "base64://{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}
