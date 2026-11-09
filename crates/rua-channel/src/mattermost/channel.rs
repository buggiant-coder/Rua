use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::content::OutgoingContentPart;
use crate::core::event_dedup::{EventDedupDecision, EventDeduper};
use crate::core::pipeline;
use crate::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::streaming::ProcessingIndicator;
use crate::core::{BufferDebouncer, ChannelId};
use crate::http_client::default_http_client;
use rua_core::Result;

use crate::mattermost::parser;
use crate::mattermost::sender::MattermostSender;
use crate::mattermost::ws;

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(500);
const MAX_INBOUND_MEDIA_SIZE: u64 = 50 * 1024 * 1024;
const TYPING_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(4);
const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

pub struct MattermostTypingIndicator {
    sender: MattermostSender,
    bot_id: String,
    channel_id: String,
    root_id: String,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl MattermostTypingIndicator {
    pub fn new(
        sender: MattermostSender,
        bot_id: String,
        channel_id: String,
        root_id: String,
    ) -> Self {
        Self {
            sender,
            bot_id,
            channel_id,
            root_id,
            task: Mutex::new(None),
        }
    }

    async fn send_typing(&self) -> Result<()> {
        self.sender
            .post_typing(&self.bot_id, &self.channel_id, &self.root_id)
            .await
    }
}

#[async_trait]
impl ProcessingIndicator for MattermostTypingIndicator {
    async fn start(&self, _req: &AgentRequest) -> Result<()> {
        self.stop(_req).await?;
        self.send_typing().await?;
        let sender = self.sender.clone();
        let bot_id = self.bot_id.clone();
        let channel_id = self.channel_id.clone();
        let root_id = self.root_id.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(TYPING_KEEPALIVE_INTERVAL).await;
                if let Err(err) = sender.post_typing(&bot_id, &channel_id, &root_id).await {
                    tracing::debug!(error = %err, "mattermost typing keepalive failed");
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
        Ok(())
    }
}

pub struct MattermostChannel {
    channel_id: ChannelId,
    sender: MattermostSender,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    base_url: String,
    bot_token: String,
    bot_id: RwLock<String>,
    bot_username: RwLock<String>,
    ws_stop_tx: watch::Sender<bool>,
    ws_handle: RwLock<Option<JoinHandle<()>>>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    debouncer: BufferDebouncer<String, AgentRequest>,
    media_dir: String,
    event_deduper: Arc<std::sync::Mutex<EventDeduper>>,
}

impl MattermostChannel {
    pub fn new(config: crate::mattermost::types::MattermostConfig) -> Self {
        let (ws_stop_tx, _) = watch::channel(false);
        let sender = MattermostSender::new(&config.url, &config.bot_token);
        Self {
            channel_id: "mattermost".into(),
            sender,
            process: RwLock::new(None),
            started: RwLock::new(false),
            base_url: config.url.trim_end_matches('/').to_string(),
            bot_token: config.bot_token,
            bot_id: RwLock::new(String::new()),
            bot_username: RwLock::new(String::new()),
            ws_stop_tx,
            ws_handle: RwLock::new(None),
            render_options: MessageRenderOptions::default(),
            acl_metadata: serde_json::json!({}),
            debouncer: BufferDebouncer::new(DEBOUNCE_WINDOW),
            media_dir: config.media_dir,
            event_deduper: Arc::new(std::sync::Mutex::new(EventDeduper::new(
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

    pub async fn download_inbound_media(&self, native: &mut serde_json::Value) -> Result<()> {
        download_mattermost_inbound_media_with(
            &self.base_url,
            &self.bot_token,
            &self.media_dir,
            native,
        )
        .await
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
                | OutgoingContentPart::Refusal { text } => {
                    if !text.is_empty() {
                        segments.push(text.clone());
                    }
                }
                OutgoingContentPart::Tool { name, status, .. } => {
                    segments.push(format!("[Tool: {name} - {status:?}]"));
                }
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
                OutgoingContentPart::Raw { value } => {
                    segments.push(value.to_string());
                }
                OutgoingContentPart::Thinking { .. } => {}
            }
        }
        segments.join("\n")
    }
}

async fn download_mattermost_inbound_media_with(
    base_url: &str,
    bot_token: &str,
    media_dir: &str,
    native: &mut serde_json::Value,
) -> Result<()> {
    let data = if native.get("data").is_some() {
        native.get_mut("data").expect("checked data")
    } else {
        native
    };
    let raw_post = data.get("post").and_then(|v| v.as_str()).unwrap_or("{}");
    let mut post: serde_json::Value =
        serde_json::from_str(raw_post).unwrap_or_else(|_| serde_json::json!({}));

    let file_ids = post
        .get("file_ids")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    if file_ids.is_empty() {
        return Ok(());
    }

    for file_id in file_ids
        .iter()
        .filter_map(|id| id.as_str().map(str::trim).filter(|id| !id.is_empty()))
    {
        let Some(file_meta) = mattermost_file_meta_mut(&mut post, file_id) else {
            continue;
        };
        if file_meta
            .get("local_path")
            .and_then(|value| value.as_str())
            .is_some()
        {
            continue;
        }
        let Some((bytes, mime)) = mattermost_download_file(base_url, bot_token, file_id).await?
        else {
            continue;
        };
        let filename = mattermost_download_filename(file_meta, file_id);
        let dir = mattermost_media_dir(media_dir);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("create mattermost media dir {}: {err}", dir.display()),
            })?;
        let final_path = dir.join(filename);
        atomic_write_file(&final_path, &bytes).await?;

        if let Some(obj) = file_meta.as_object_mut() {
            obj.insert(
                "local_path".to_string(),
                serde_json::Value::String(final_path.to_string_lossy().to_string()),
            );
            if let Some(mime) = mime {
                obj.insert("mime_type".to_string(), serde_json::Value::String(mime));
            }
        }
    }

    data["post"] = serde_json::Value::String(post.to_string());
    Ok(())
}

fn mattermost_file_meta_mut<'a>(
    post: &'a mut serde_json::Value,
    file_id: &str,
) -> Option<&'a mut serde_json::Value> {
    let files = post.pointer_mut("/metadata/files")?.as_array_mut()?;
    files
        .iter_mut()
        .find(|file| file.get("id").and_then(|value| value.as_str()) == Some(file_id))
}

async fn mattermost_download_file(
    base_url: &str,
    bot_token: &str,
    file_id: &str,
) -> Result<Option<(Vec<u8>, Option<String>)>> {
    let url = format!(
        "{}/api/v4/files/{}",
        base_url.trim_end_matches('/'),
        file_id
    );
    let client = default_http_client();
    let resp = match client
        .get(url)
        .header("Authorization", format!("Bearer {bot_token}"))
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%file_id, error = %err, "mattermost file download failed");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(%file_id, status = %resp.status(), "mattermost file download non-success");
        return Ok(None);
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_INBOUND_MEDIA_SIZE {
            tracing::warn!(%file_id, len, "mattermost file too large");
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
            message: format!("mattermost file download bytes failed: {err}"),
        })?;
    if bytes.len() as u64 > MAX_INBOUND_MEDIA_SIZE {
        tracing::warn!(%file_id, len = bytes.len(), "mattermost file too large");
        return Ok(None);
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some((bytes.to_vec(), mime)))
}

fn mattermost_media_dir(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    if media_dir.is_empty() {
        std::env::temp_dir().join("rua-mattermost-media")
    } else {
        PathBuf::from(media_dir)
    }
}

fn mattermost_download_filename(file_meta: &serde_json::Value, file_id: &str) -> String {
    let hint = file_meta
        .get("name")
        .or_else(|| file_meta.get("filename"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(file_id);
    crate::core::media::sanitize_filename(hint)
}

async fn atomic_write_file(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| rua_core::RuaError::Io {
        message: format!("missing parent dir for {}", path.display()),
    })?;
    let tmp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "mattermost-media".into()),
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

fn mattermost_media_path(part: &OutgoingContentPart) -> Option<&std::path::PathBuf> {
    match part {
        OutgoingContentPart::Image {
            path: Some(path), ..
        }
        | OutgoingContentPart::File {
            path: Some(path), ..
        }
        | OutgoingContentPart::Audio {
            path: Some(path), ..
        }
        | OutgoingContentPart::Video {
            path: Some(path), ..
        } => Some(path),
        _ => None,
    }
}

fn mattermost_check_duplicate_request(
    deduper: &Arc<std::sync::Mutex<EventDeduper>>,
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
        message: "mattermost event deduper lock poisoned".into(),
    })?;
    Ok(
        deduper.check_and_mark(format!("mattermost:post:{message_id}"))
            == EventDedupDecision::Duplicate,
    )
}

#[async_trait]
impl channel::Channel for MattermostChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("mattermost channel already started");
            return Ok(());
        }
        *self.process.write().await = Some(process.clone());
        *started = true;

        match self.sender.get_bot_info().await {
            Ok((id, username)) => {
                tracing::info!(bot_id = %id, bot_username = %username, "mattermost bot info fetched");
                *self.bot_id.write().await = id;
                *self.bot_username.write().await = username;
            }
            Err(e) => {
                tracing::error!(error = %e, "mattermost: failed to fetch bot info, WS will not filter self-messages");
            }
        }

        let ws_config = ws::WsConfig {
            base_url: self.base_url.clone(),
            bot_token: self.bot_token.clone(),
        };
        let process_for_ws = process;
        let bot_id = self.bot_id.read().await.clone();
        let bot_username = self.bot_username.read().await.clone();
        let debouncer = self.debouncer.clone();
        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::Markdown,
            self.render_options.clone(),
        ));
        let acl_metadata = Arc::new(self.acl_metadata.clone());
        let inbound_base_url = self.base_url.clone();
        let inbound_bot_token = self.bot_token.clone();
        let inbound_media_dir = self.media_dir.clone();
        let inbound_sender = self.sender.clone();
        let event_deduper = self.event_deduper.clone();
        let participated = Arc::new(std::sync::Mutex::new(
            std::collections::VecDeque::<String>::new(),
        ));
        let send_fn: pipeline::SendFn = Arc::new({
            let sender = MattermostSender::new(&self.base_url, &self.bot_token);
            let sender = Arc::new(sender);
            move |to, msg| {
                let sender = sender.clone();
                Box::pin(async move {
                    let mm_channel_id = to
                        .metadata
                        .get("mm_channel_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| rua_core::RuaError::Validation {
                        message: "missing mm_channel_id in target metadata".into(),
                    })?;
                    let root_id = to
                        .metadata
                        .get("root_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let text = MattermostChannel::render_text_and_unresolved_media(&msg.content);
                    if !text.is_empty() {
                        sender.post_message(mm_channel_id, &text, root_id).await?;
                    }
                    let mut file_ids = Vec::new();
                    for part in &msg.content {
                        let Some(path) = mattermost_media_path(part) else {
                            continue;
                        };
                        match sender.upload_file(mm_channel_id, path).await {
                            Ok(file_id) => file_ids.push(file_id),
                            Err(err) => {
                                tracing::warn!(error = %err, "mattermost media upload failed");
                                let fallback =
                                    MattermostChannel::render_content(std::slice::from_ref(part));
                                if !fallback.is_empty() {
                                    sender
                                        .post_message(mm_channel_id, &fallback, root_id)
                                        .await?;
                                }
                            }
                        }
                    }
                    if !file_ids.is_empty() {
                        sender
                            .post_message_with_files(mm_channel_id, "", root_id, &file_ids)
                            .await?;
                    }
                    Ok(())
                })
            }
        });
        let on_message: ws::OnMessageFn = Arc::new(move |native| {
            let p = process_for_ws.clone();
            let bid = bot_id.clone();
            let bname = bot_username.clone();
            let d = debouncer.clone();
            let r = renderer.clone();
            let sf = send_fn.clone();
            let acl = acl_metadata.clone();
            let base_url = inbound_base_url.clone();
            let bot_token = inbound_bot_token.clone();
            let media_dir = inbound_media_dir.clone();
            let indicator_sender = inbound_sender.clone();
            let event_deduper = event_deduper.clone();
            let participated = participated.clone();
            tokio::spawn(async move {
                let mut native = native;
                if let Err(err) = download_mattermost_inbound_media_with(
                    &base_url,
                    &bot_token,
                    &media_dir,
                    &mut native,
                )
                .await
                {
                    tracing::warn!(error = %err, "mattermost inbound media download failed; using fallback placeholders");
                }
                let data = native.get("data").unwrap_or(&native);
                let post: serde_json::Value =
                    serde_json::from_str(data["post"].as_str().unwrap_or("{}")).unwrap_or_default();
                let root = post["root_id"].as_str().unwrap_or_default();
                let followed = acl["thread_follow_without_mention"] == true
                    && !root.is_empty()
                    && participated
                        .lock()
                        .is_ok_and(|threads| threads.iter().any(|id| id == root));
                match parser::parse_with_thread_policy(native, &bid, &bname, followed) {
                    Ok(Some(mut req)) => {
                        match mattermost_check_duplicate_request(&event_deduper, &req) {
                            Ok(true) => {
                                tracing::debug!(
                                    message_id = ?req.message_id,
                                    "mattermost duplicate event skipped"
                                );
                                return;
                            }
                            Ok(false) => {}
                            Err(err) => {
                                tracing::warn!(error = %err, "mattermost event dedup failed");
                            }
                        }
                        if let (Some(acl_obj), Some(meta)) =
                            (acl.as_object(), req.metadata.as_object_mut())
                        {
                            for (k, v) in acl_obj {
                                meta.insert(k.clone(), v.clone());
                            }
                        }
                        let process: Arc<dyn Process> = Arc::new(ThreadProcess {
                            inner: p,
                            participated: participated.clone(),
                        });
                        let sender_key = req.user_id.clone();
                        let mm_channel_id = req
                            .metadata
                            .get("mm_channel_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        let root_id = req
                            .metadata
                            .get("root_id")
                            .and_then(|value| value.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !root_id.is_empty() {
                            match indicator_sender.fetch_thread_context(&root_id, 8).await {
                                Ok(context) if !context.is_empty() => {
                                    if let Some(meta) = req.metadata.as_object_mut() {
                                        meta.insert(
                                            "mattermost_thread_context".into(),
                                            serde_json::json!(context),
                                        );
                                    }
                                }
                                Ok(_) => {}
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        root_id,
                                        "mattermost thread context fetch failed"
                                    );
                                }
                            }
                        }
                        let indicator = if bid.is_empty() || mm_channel_id.is_empty() {
                            None
                        } else {
                            Some(Arc::new(MattermostTypingIndicator::new(
                                indicator_sender,
                                bid.clone(),
                                mm_channel_id,
                                root_id,
                            ))
                                as Arc<dyn ProcessingIndicator>)
                        };
                        if let Err(e) = pipeline::run_inbound_streaming_with_indicator(
                            process,
                            req,
                            Some((d, sender_key)),
                            r,
                            sf,
                            None,
                            indicator,
                        )
                        .await
                        {
                            tracing::error!("mattermost pipeline error: {e}");
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::error!("mattermost WS: parse failed: {e}");
                    }
                }
            });
        });
        self.ws_stop_tx.send_replace(false);
        let stop_rx = self.ws_stop_tx.subscribe();
        if let Some(handle) = ws::spawn_ws_task(ws_config, on_message, stop_rx) {
            *self.ws_handle.write().await = Some(handle);
        }

        tracing::info!("mattermost channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut started = self.started.write().await;
        if !*started {
            return Ok(());
        }

        let _ = self.ws_stop_tx.send(true);
        if let Some(handle) = self.ws_handle.write().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }

        if let Ok(mut deduper) = self.event_deduper.lock() {
            deduper.clear();
        }
        *self.process.write().await = None;
        *started = false;
        tracing::info!("mattermost channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        let mm_channel_id = to
            .metadata
            .get("mm_channel_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| rua_core::RuaError::Validation {
                message: "missing mm_channel_id in target metadata".into(),
            })?;

        let root_id = to
            .metadata
            .get("root_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let text = Self::render_text_and_unresolved_media(&msg.content);
        if !text.is_empty() {
            self.sender
                .post_message(mm_channel_id, &text, root_id)
                .await?;
        }

        let mut file_ids = Vec::new();
        for part in &msg.content {
            let Some(path) = mattermost_media_path(part) else {
                continue;
            };
            match self.sender.upload_file(mm_channel_id, path).await {
                Ok(file_id) => file_ids.push(file_id),
                Err(err) => {
                    tracing::warn!(error = %err, "mattermost media upload failed");
                    let fallback = Self::render_content(std::slice::from_ref(part));
                    if !fallback.is_empty() {
                        self.sender
                            .post_message(mm_channel_id, &fallback, root_id)
                            .await?;
                    }
                }
            }
        }
        if !file_ids.is_empty() {
            self.sender
                .post_message_with_files(mm_channel_id, "", root_id, &file_ids)
                .await?;
        }

        Ok(())
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        let req = parser::parse_native(native)?;
        if mattermost_check_duplicate_request(&self.event_deduper, &req)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate mattermost event".into(),
            });
        }
        Ok(req)
    }

    async fn refresh_webhook_or_token(&self) -> Result<()> {
        Ok(())
    }
}

// Mark participation only after the shared pipeline admits the request.
struct ThreadProcess {
    inner: Arc<dyn Process>,
    participated: Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
}

#[async_trait]
impl Process for ThreadProcess {
    async fn handle(
        &self,
        req: AgentRequest,
    ) -> futures::stream::BoxStream<'static, rua_core::RuntimeEvent> {
        if let Some(root) = req.metadata["root_id"].as_str().filter(|id| !id.is_empty()) {
            if let Ok(mut threads) = self.participated.lock() {
                threads.retain(|id| id != root);
                threads.push_back(root.to_owned());
                while threads.len() > 1000 {
                    threads.pop_front();
                }
            }
        }
        self.inner.handle(req).await
    }
}
