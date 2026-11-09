use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::core::channel::{self, AgentRequest, Channel, OutMessage, Process, ToHandle};
use crate::core::content::{render_parts_to_string, IncomingContentPart, OutgoingContentPart};
use crate::core::event_dedup::{EventDedupDecision, EventDeduper};
use crate::core::media::sanitize_filename;
use crate::core::pipeline;
use crate::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::streaming::{
    PlatformStreamAction, PlatformStreamPolicy, PlatformStreamState, StreamHandler, StreamType,
};
use crate::core::ChannelId;
use crate::http_client::default_http_client;
use rua_core::Result;

use super::types::SlackConfig;

const SLACK_API_BASE: &str = "https://slack.com/api";
const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub struct SlackChannel {
    channel_id: ChannelId,
    pub(super) config: SlackConfig,
    pub(super) client: reqwest::Client,
    pub(super) api_base: String,
    process: tokio::sync::RwLock<Option<Arc<dyn Process>>>,
    started: tokio::sync::RwLock<bool>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    event_deduper: Arc<StdMutex<EventDeduper>>,
    socket_task: Mutex<Option<(CancellationToken, tokio::task::JoinHandle<()>)>>,
}

pub struct SlackStreamHandler {
    client: reqwest::Client,
    api_base: String,
    bot_token: String,
    channel: String,
    thread_ts: Option<String>,
    policy: PlatformStreamPolicy,
    started_at: tokio::time::Instant,
    state: Mutex<SlackStreamState>,
}

#[derive(Default)]
struct SlackStreamState {
    stream: PlatformStreamState,
    ts: Option<String>,
}

impl SlackChannel {
    pub fn new(config: SlackConfig) -> Self {
        Self {
            channel_id: "slack".into(),
            config,
            client: default_http_client(),
            api_base: SLACK_API_BASE.into(),
            process: tokio::sync::RwLock::new(None),
            started: tokio::sync::RwLock::new(false),
            render_options: MessageRenderOptions::default(),
            acl_metadata: serde_json::json!({}),
            socket_task: Mutex::new(None),
            event_deduper: Arc::new(StdMutex::new(EventDeduper::new(
                EVENT_DEDUP_TTL,
                EVENT_DEDUP_CAPACITY,
            ))),
        }
    }

    pub fn with_api_base(mut self, api_base: String) -> Self {
        self.api_base = api_base.trim_end_matches('/').to_string();
        self
    }

    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    pub async fn handle_socket_payload(
        &self,
        process: Arc<dyn Process>,
        payload: serde_json::Value,
    ) -> Result<()> {
        let request = self
            .build_request_from_native_with_media_download(payload)
            .await?;
        let mut sender = SlackChannel::new(self.config.clone())
            .with_api_base(self.api_base.clone())
            .with_client(self.client.clone());
        sender.render_options = self.render_options.clone();
        let sender = Arc::new(sender);
        let streaming = self
            .acl_metadata
            .get("streaming_enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let target = ToHandle {
            conversation_id: request.conversation_id.clone(),
            reply_to: request
                .metadata
                .get("thread_ts")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            metadata: request.metadata.clone(),
            agent_id: request.agent_id.clone(),
            sender_id: Some(request.user_id.clone()),
        };
        let handler =
            streaming.then(|| Arc::new(sender.stream_handler(&target)) as Arc<dyn StreamHandler>);
        let send_fn: pipeline::SendFn = Arc::new(move |mut to, msg| {
            let sender = sender.clone();
            Box::pin(async move {
                // Slack event IDs are deduplication keys, never thread timestamps.
                to.reply_to = to
                    .metadata
                    .get("thread_ts")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                sender.send(to, msg).await
            })
        });
        pipeline::run_inbound_streaming(
            process,
            request,
            None,
            Arc::new(MessageRenderer::with_options(
                RenderStyle::Markdown,
                self.render_options.clone(),
            )),
            send_fn,
            handler,
        )
        .await
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

    pub fn stream_handler(&self, to: &ToHandle) -> SlackStreamHandler {
        SlackStreamHandler::new(
            self.client.clone(),
            self.api_base.clone(),
            self.config.bot_token.clone(),
            to.conversation_id.clone(),
            to.reply_to.clone().or_else(|| {
                to.metadata
                    .get("thread_ts")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            }),
        )
    }

    pub async fn build_request_from_native_with_media_download(
        &self,
        native: serde_json::Value,
    ) -> Result<AgentRequest> {
        let mut req = slack_parse_native(&native, &self.config.bot_user_id, &self.acl_metadata)?;
        if slack_check_duplicate_request(&self.event_deduper, &req)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate slack event".into(),
            });
        }

        let event = native.get("event").unwrap_or(&native);
        if let Some(files) = event.get("files").and_then(|value| value.as_array()) {
            for file in files {
                let url = file
                    .get("url_private_download")
                    .or_else(|| file.get("url_private"))
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let name = file
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("file");
                let mime = file
                    .get("mimetype")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if let Some(part) = self.download_slack_file_part(url, name, mime).await? {
                    req.input.retain(|existing| {
                        !matches!(existing, IncomingContentPart::Url { url: existing_url, .. } if existing_url == url)
                    });
                    req.input.push(part);
                }
            }
        }

        Ok(req)
    }

    async fn download_slack_file_part(
        &self,
        url: &str,
        filename: &str,
        mime: &str,
    ) -> Result<Option<IncomingContentPart>> {
        if url.trim().is_empty() {
            return Ok(None);
        }
        let resp = self
            .client
            .get(url)
            .bearer_auth(&self.config.bot_token)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("slack file download request failed: {err}"),
            })?;
        if !resp.status().is_success() {
            tracing::warn!(status = %resp.status(), "slack file download failed");
            return Ok(None);
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or(mime)
            .split(';')
            .next()
            .unwrap_or(mime)
            .trim()
            .to_string();
        if content_type.contains("text/html") {
            tracing::warn!("slack file download returned HTML; keeping URL fallback");
            return Ok(None);
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("slack file download body failed: {err}"),
            })?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let dir = if self.config.media_dir.trim().is_empty() {
            std::env::temp_dir().join("rua-slack-media")
        } else {
            PathBuf::from(&self.config.media_dir)
        };
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("create slack media dir {}: {err}", dir.display()),
            })?;
        let safe_name = sanitize_filename(filename);
        let file_id = format!("slack-{}", uuid::Uuid::new_v4().as_simple());
        let path = dir.join(format!("{file_id}-{safe_name}"));
        let tmp_path = path.with_extension(format!(
            "{}.tmp",
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
        ));
        let mut file =
            tokio::fs::File::create(&tmp_path)
                .await
                .map_err(|err| rua_core::RuaError::Io {
                    message: format!("create slack media temp {}: {err}", tmp_path.display()),
                })?;
        file.write_all(&bytes)
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("write slack media temp {}: {err}", tmp_path.display()),
            })?;
        file.sync_all()
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("fsync slack media temp {}: {err}", tmp_path.display()),
            })?;
        drop(file);
        tokio::fs::rename(&tmp_path, &path)
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!(
                    "rename slack media temp {} -> {}: {err}",
                    tmp_path.display(),
                    path.display()
                ),
            })?;

        let mime_value = (!content_type.is_empty()).then_some(content_type);
        let part = if mime_value
            .as_deref()
            .is_some_and(|value| value.starts_with("image/"))
        {
            IncomingContentPart::Image {
                file_id,
                path,
                mime: mime_value,
            }
        } else if mime_value
            .as_deref()
            .is_some_and(|value| value.starts_with("audio/"))
        {
            IncomingContentPart::Audio {
                file_id,
                path,
                mime: mime_value,
                name: Some(safe_name),
            }
        } else if mime_value
            .as_deref()
            .is_some_and(|value| value.starts_with("video/"))
        {
            IncomingContentPart::Video {
                file_id,
                path,
                mime: mime_value,
                name: Some(safe_name),
            }
        } else {
            IncomingContentPart::File {
                file_id,
                path,
                mime: mime_value,
                name: Some(safe_name),
            }
        };
        Ok(Some(part))
    }

    async fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<String> {
        slack_post_message(
            &self.client,
            &self.api_base,
            &self.config.bot_token,
            channel,
            text,
            thread_ts,
        )
        .await
    }

    async fn upload_file(&self, channel: &str, path: &Path, thread_ts: Option<&str>) -> Result<()> {
        slack_upload_file(
            &self.client,
            &self.api_base,
            &self.config.bot_token,
            channel,
            path,
            thread_ts,
        )
        .await
    }

    async fn upload_remote_url(
        &self,
        channel: &str,
        url: &str,
        filename: &str,
        thread_ts: Option<&str>,
    ) -> Result<()> {
        slack_upload_remote_url(
            &self.client,
            &self.api_base,
            &self.config.bot_token,
            channel,
            url,
            filename,
            thread_ts,
        )
        .await
    }

    pub async fn fetch_thread_context(
        &self,
        channel: &str,
        thread_ts: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let resp = self
            .client
            .get(format!("{}/conversations.replies", self.api_base))
            .bearer_auth(&self.config.bot_token)
            .query(&[
                ("channel", channel),
                ("ts", thread_ts),
                ("limit", &limit.to_string()),
            ])
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("slack conversations.replies request failed: {err}"),
            })?;
        let status = resp.status();
        let value = resp
            .json::<serde_json::Value>()
            .await
            .unwrap_or_else(|_| serde_json::json!({ "ok": status.is_success() }));
        if !status.is_success() || value.get("ok").and_then(|value| value.as_bool()) == Some(false)
        {
            return Err(rua_core::RuaError::Runtime {
                message: format!("slack conversations.replies failed: {value}"),
            });
        }
        Ok(value
            .get("messages")
            .and_then(|value| value.as_array())
            .map(|messages| {
                messages
                    .iter()
                    .filter_map(|message| message.get("text").and_then(|value| value.as_str()))
                    .take(limit)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default())
    }

    fn render_text_and_unresolved_media(parts: &[OutgoingContentPart]) -> String {
        let text_parts = parts
            .iter()
            .filter(|part| {
                matches!(
                    part,
                    OutgoingContentPart::Text { .. }
                        | OutgoingContentPart::Markdown { .. }
                        | OutgoingContentPart::Refusal { .. }
                        | OutgoingContentPart::Tool { .. }
                        | OutgoingContentPart::Thinking { .. }
                        | OutgoingContentPart::Raw { .. }
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut rendered = render_parts_to_string(&text_parts);
        for part in parts {
            let fallback = match part {
                OutgoingContentPart::Image { path: Some(_), .. }
                | OutgoingContentPart::File { path: Some(_), .. }
                | OutgoingContentPart::Audio { path: Some(_), .. }
                | OutgoingContentPart::Video { path: Some(_), .. } => None,
                OutgoingContentPart::Image { url: Some(url), .. }
                    if slack_upload_url_allowed(url) =>
                {
                    None
                }
                OutgoingContentPart::File { url: Some(url), .. }
                | OutgoingContentPart::Audio { url: Some(url), .. }
                | OutgoingContentPart::Video { url: Some(url), .. }
                    if slack_upload_url_allowed(url) =>
                {
                    None
                }
                OutgoingContentPart::Image { url: Some(url), .. } => Some(format!("[Image] {url}")),
                OutgoingContentPart::File {
                    url: Some(url),
                    name,
                    ..
                } => Some(format!(
                    "[File: {}] {url}",
                    name.as_deref().unwrap_or("file")
                )),
                OutgoingContentPart::Audio {
                    url: Some(url),
                    name,
                    ..
                } => Some(format!(
                    "[Audio: {}] {url}",
                    name.as_deref().unwrap_or("audio")
                )),
                OutgoingContentPart::Video {
                    url: Some(url),
                    name,
                    ..
                } => Some(format!(
                    "[Video: {}] {url}",
                    name.as_deref().unwrap_or("video")
                )),
                OutgoingContentPart::Image {
                    path: None, alt, ..
                } => Some(alt.as_deref().unwrap_or("[Image]").to_string()),
                OutgoingContentPart::File {
                    path: None, name, ..
                } => Some(format!("[File: {}]", name.as_deref().unwrap_or("file"))),
                OutgoingContentPart::Audio {
                    path: None, name, ..
                } => Some(format!("[Audio: {}]", name.as_deref().unwrap_or("audio"))),
                OutgoingContentPart::Video {
                    path: None, name, ..
                } => Some(format!("[Video: {}]", name.as_deref().unwrap_or("video"))),
                _ => None,
            };
            if let Some(fallback) = fallback {
                if !fallback.trim().is_empty() && !rendered.contains(&fallback) {
                    if !rendered.is_empty() {
                        rendered.push('\n');
                    }
                    rendered.push_str(&fallback);
                }
            }
        }
        rendered
    }
}

#[async_trait]
impl channel::Channel for SlackChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            return Ok(());
        }
        if self.config.bot_token.is_empty() || self.config.app_token.is_empty() {
            return Err(rua_core::RuaError::Validation {
                message: "Slack Socket Mode requires bot_token and app_token".into(),
            });
        }
        let mut worker = SlackChannel::new(self.config.clone())
            .with_api_base(self.api_base.clone())
            .with_client(self.client.clone())
            .with_acl_config(self.acl_metadata.clone());
        worker.render_options = self.render_options.clone();
        worker.event_deduper = self.event_deduper.clone();
        if worker.config.bot_user_id.is_empty() {
            let auth = super::socket_mode::api_call(
                &self.client,
                &self.api_base,
                "auth.test",
                &self.config.bot_token,
            )
            .await?;
            worker.config.bot_user_id = auth["user_id"].as_str().unwrap_or_default().to_string();
        }
        let url = super::socket_mode::connection_url(
            &self.client,
            &self.api_base,
            &self.config.app_token,
        )
        .await?;
        let cancel = CancellationToken::new();
        let task = tokio::spawn(super::socket_mode::run(
            Arc::new(worker),
            process.clone(),
            url,
            cancel.clone(),
        ));
        *self.socket_task.lock().await = Some((cancel, task));
        *self.process.write().await = Some(process);
        *started = true;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        if let Some((cancel, mut task)) = self.socket_task.lock().await.take() {
            cancel.cancel();
            if tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        *self.process.write().await = None;
        *self.started.write().await = false;
        if let Ok(mut deduper) = self.event_deduper.lock() {
            deduper.clear();
        }
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        let thread_ts = to.reply_to.as_deref().or_else(|| {
            to.metadata
                .get("thread_ts")
                .and_then(|value| value.as_str())
        });
        let text = Self::render_text_and_unresolved_media(&msg.content);
        if !text.trim().is_empty() {
            self.post_message(&to.conversation_id, &text, thread_ts)
                .await?;
        }
        for part in &msg.content {
            if let Some((url, filename)) = slack_remote_media(part) {
                self.upload_remote_url(&to.conversation_id, url, &filename, thread_ts)
                    .await?;
                continue;
            }
            let path = match part {
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
                } => path.as_path(),
                _ => continue,
            };
            self.upload_file(&to.conversation_id, path, thread_ts)
                .await?;
        }
        Ok(())
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        let req = slack_parse_native(&native, &self.config.bot_user_id, &self.acl_metadata)?;
        if slack_check_duplicate_request(&self.event_deduper, &req)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate slack event".into(),
            });
        }
        Ok(req)
    }
}

impl SlackStreamHandler {
    pub fn new(
        client: reqwest::Client,
        api_base: String,
        bot_token: String,
        channel: String,
        thread_ts: Option<String>,
    ) -> Self {
        Self {
            client,
            api_base: api_base.trim_end_matches('/').to_string(),
            bot_token,
            channel,
            thread_ts,
            policy: PlatformStreamPolicy::placeholder_edit(Duration::from_millis(0), 3500),
            started_at: tokio::time::Instant::now(),
            state: Mutex::new(SlackStreamState::default()),
        }
    }
}

#[async_trait]
impl StreamHandler for SlackStreamHandler {
    async fn on_start(&self, _stream_type: StreamType) -> Result<()> {
        Ok(())
    }

    async fn on_delta(&self, stream_type: StreamType, accumulated: &str) -> Result<()> {
        if stream_type != StreamType::Message {
            return Ok(());
        }
        let mut state = self.state.lock().await;
        let action = state
            .stream
            .on_delta(&self.policy, self.started_at.elapsed(), accumulated);
        match action {
            PlatformStreamAction::Create { content, .. } => {
                let ts = slack_post_message(
                    &self.client,
                    &self.api_base,
                    &self.bot_token,
                    &self.channel,
                    if content.is_empty() { "..." } else { &content },
                    self.thread_ts.as_deref(),
                )
                .await?;
                state.ts = Some(ts);
            }
            PlatformStreamAction::Update { content, .. } => {
                if let Some(ts) = state.ts.as_deref() {
                    slack_update_message(
                        &self.client,
                        &self.api_base,
                        &self.bot_token,
                        &self.channel,
                        ts,
                        &content,
                    )
                    .await?;
                }
            }
            PlatformStreamAction::SendFinal { content } => {
                let ts = slack_post_message(
                    &self.client,
                    &self.api_base,
                    &self.bot_token,
                    &self.channel,
                    &content,
                    self.thread_ts.as_deref(),
                )
                .await?;
                state.ts = Some(ts);
            }
            PlatformStreamAction::Skip => {}
        }
        Ok(())
    }

    async fn on_end(&self, stream_type: StreamType, accumulated: &str) -> Result<()> {
        if stream_type != StreamType::Message {
            return Ok(());
        }
        let mut state = self.state.lock().await;
        let action = state.stream.on_end(&self.policy, accumulated);
        match action {
            PlatformStreamAction::Update { content, .. } => {
                if let Some(ts) = state.ts.as_deref() {
                    slack_update_message(
                        &self.client,
                        &self.api_base,
                        &self.bot_token,
                        &self.channel,
                        ts,
                        &content,
                    )
                    .await?;
                }
            }
            PlatformStreamAction::SendFinal { content }
            | PlatformStreamAction::Create { content, .. } => {
                let ts = slack_post_message(
                    &self.client,
                    &self.api_base,
                    &self.bot_token,
                    &self.channel,
                    &content,
                    self.thread_ts.as_deref(),
                )
                .await?;
                state.ts = Some(ts);
            }
            PlatformStreamAction::Skip => {}
        }
        Ok(())
    }
}

fn slack_parse_native(
    native: &serde_json::Value,
    bot_user_id: &str,
    acl_metadata: &serde_json::Value,
) -> Result<AgentRequest> {
    let event = native.get("event").unwrap_or(native);
    let event_type = event
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if !matches!(event_type, "message" | "app_mention") {
        return Err(rua_core::RuaError::Validation {
            message: "slack native payload has no parseable message event".into(),
        });
    }
    if event.get("bot_id").is_some()
        || event.get("subtype").and_then(|value| value.as_str()) == Some("bot_message")
    {
        return Err(rua_core::RuaError::Validation {
            message: "slack bot message skipped".into(),
        });
    }
    let channel = event
        .get("channel")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let user = event
        .get("user")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let ts = event
        .get("ts")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let thread_ts = event
        .get("thread_ts")
        .and_then(|value| value.as_str())
        .unwrap_or(ts);
    let channel_type = event
        .get("channel_type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let raw_text = event
        .get("text")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let bot_mentioned = event_type == "app_mention"
        || (!bot_user_id.trim().is_empty() && raw_text.contains(&format!("<@{bot_user_id}>")));
    let text = strip_slack_bot_mention(raw_text, bot_user_id);
    let is_dm = channel_type == "im";
    let session_id = if !thread_ts.is_empty() && thread_ts != ts {
        format!("slack_thread:{channel}:{thread_ts}")
    } else if is_dm {
        format!("slack_dm:{channel}")
    } else {
        format!("slack:{channel}")
    };
    let mut input = Vec::new();
    if !text.trim().is_empty() {
        input.push(IncomingContentPart::Text { text });
    }
    if let Some(files) = event.get("files").and_then(|value| value.as_array()) {
        for file in files {
            let url = file
                .get("url_private_download")
                .or_else(|| file.get("url_private"))
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if !url.is_empty() {
                input.push(IncomingContentPart::Url {
                    url: url.to_string(),
                    title: file
                        .get("name")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                });
            }
        }
    }
    if input.is_empty() {
        return Err(rua_core::RuaError::Validation {
            message: "slack native payload has no parseable content".into(),
        });
    }
    let message_id = native
        .get("event_id")
        .or_else(|| event.get("client_msg_id"))
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| (!ts.is_empty()).then(|| ts.to_string()));
    let mut metadata = serde_json::json!({
        "channel": channel,
        "slack_channel_id": channel,
        "sender_id": user,
        "slack_user_id": user,
        "channel_type": channel_type,
        "thread_ts": thread_ts,
        "ts": ts,
        "is_dm": is_dm,
        "is_group": !is_dm,
        "mentioned": bot_mentioned,
        "bot_mentioned": bot_mentioned,
    });
    if let (Some(acl_obj), Some(meta)) = (acl_metadata.as_object(), metadata.as_object_mut()) {
        for (key, value) in acl_obj {
            meta.insert(key.clone(), value.clone());
        }
    }
    Ok(AgentRequest {
        agent_id: String::new(),
        user_id: user.to_string(),
        session_id: session_id.clone(),
        input,
        channel_id: "slack".into(),
        conversation_id: channel.to_string(),
        message_id,
        metadata,
    })
}

fn strip_slack_bot_mention(text: &str, bot_user_id: &str) -> String {
    if bot_user_id.trim().is_empty() {
        return text.trim().to_string();
    }
    text.replace(&format!("<@{bot_user_id}>"), " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn slack_check_duplicate_request(
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
        message: "slack event deduper lock poisoned".into(),
    })?;
    Ok(
        deduper.check_and_mark(format!("slack:message:{message_id}"))
            == EventDedupDecision::Duplicate,
    )
}

async fn slack_post_message(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    channel: &str,
    text: &str,
    thread_ts: Option<&str>,
) -> Result<String> {
    let mut body = serde_json::json!({
        "channel": channel,
        "text": text,
        "mrkdwn": true,
    });
    if let Some(thread_ts) = thread_ts.filter(|value| !value.is_empty()) {
        body["thread_ts"] = serde_json::json!(thread_ts);
    }
    let resp = client
        .post(format!("{api_base}/chat.postMessage"))
        .bearer_auth(token)
        .json(&body)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("slack postMessage request failed: {err}"),
        })?;
    let status = resp.status();
    let value = resp
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|_| serde_json::json!({ "ok": status.is_success() }));
    if !status.is_success() || value.get("ok").and_then(|value| value.as_bool()) == Some(false) {
        return Err(rua_core::RuaError::Runtime {
            message: format!("slack postMessage failed: {value}"),
        });
    }
    Ok(value
        .get("ts")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string())
}

async fn slack_update_message(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    channel: &str,
    ts: &str,
    text: &str,
) -> Result<()> {
    let body = serde_json::json!({
        "channel": channel,
        "ts": ts,
        "text": text,
        "mrkdwn": true,
    });
    let resp = client
        .post(format!("{api_base}/chat.update"))
        .bearer_auth(token)
        .json(&body)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("slack update request failed: {err}"),
        })?;
    if !resp.status().is_success() {
        return Err(rua_core::RuaError::Runtime {
            message: format!("slack update HTTP {}", resp.status()),
        });
    }
    Ok(())
}

fn slack_upload_url_allowed(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::from_str(url) else {
        return false;
    };
    if !matches!(parsed.scheme(), "https" | "http") {
        return false;
    }
    let Some(host) = parsed.host_str().map(|host| host.to_ascii_lowercase()) else {
        return false;
    };
    [".slack.com", ".slack-edge.com", ".slack-files.com"]
        .iter()
        .any(|suffix| {
            let bare = suffix.trim_start_matches('.');
            host == bare || host.ends_with(suffix)
        })
}

fn slack_remote_media(part: &OutgoingContentPart) -> Option<(&str, String)> {
    let (url, name) = match part {
        OutgoingContentPart::Image {
            url: Some(url),
            alt,
            ..
        } => (url.as_str(), alt.as_deref().unwrap_or("image")),
        OutgoingContentPart::File {
            url: Some(url),
            name,
            ..
        } => (url.as_str(), name.as_deref().unwrap_or("file")),
        OutgoingContentPart::Audio {
            url: Some(url),
            name,
            ..
        } => (url.as_str(), name.as_deref().unwrap_or("audio")),
        OutgoingContentPart::Video {
            url: Some(url),
            name,
            ..
        } => (url.as_str(), name.as_deref().unwrap_or("video")),
        _ => return None,
    };
    if !slack_upload_url_allowed(url) {
        return None;
    }
    Some((url, slack_filename_from_url(url, name)))
}

fn slack_filename_from_url(url: &str, fallback: &str) -> String {
    reqwest::Url::from_str(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()
                .and_then(|mut segments| segments.next_back().map(str::to_string))
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

async fn slack_upload_file(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    channel: &str,
    path: &Path,
    thread_ts: Option<&str>,
) -> Result<()> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("read slack upload file {}: {err}", path.display()),
        })?;
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("file")
        .to_string();
    slack_upload_bytes(
        client, api_base, token, channel, bytes, &filename, thread_ts,
    )
    .await
}

async fn slack_upload_bytes(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    channel: &str,
    bytes: Vec<u8>,
    filename: &str,
    thread_ts: Option<&str>,
) -> Result<()> {
    let response = client
        .post(format!("{api_base}/files.getUploadURLExternal"))
        .bearer_auth(token)
        .form(&[
            ("filename", filename.to_string()),
            ("length", bytes.len().to_string()),
        ])
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| rua_core::RuaError::Runtime {
            message: format!("Slack upload URL: {error}"),
        })?;
    let value = slack_api_response(response).await?;
    let upload_url = value["upload_url"]
        .as_str()
        .ok_or_else(|| rua_core::RuaError::Runtime {
            message: "Slack upload response has no upload_url".into(),
        })?;
    let file_id = value["file_id"]
        .as_str()
        .ok_or_else(|| rua_core::RuaError::Runtime {
            message: "Slack upload response has no file_id".into(),
        })?;
    // The upload URL is pre-signed. Never send the bot token to its host.
    let response = client
        .post(upload_url)
        .body(bytes)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| rua_core::RuaError::Runtime {
            message: format!("Slack file upload: {error}"),
        })?;
    if !response.status().is_success() {
        return Err(rua_core::RuaError::Runtime {
            message: format!("Slack file upload HTTP {}", response.status()),
        });
    }
    let mut body =
        serde_json::json!({"files": [{"id": file_id, "title": filename}], "channel_id": channel});
    if let Some(thread) = thread_ts.filter(|thread| !thread.is_empty()) {
        body["thread_ts"] = thread.into();
    }
    let response = client
        .post(format!("{api_base}/files.completeUploadExternal"))
        .bearer_auth(token)
        .json(&body)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| rua_core::RuaError::Runtime {
            message: format!("Slack complete upload: {error}"),
        })?;
    slack_api_response(response).await.map(|_| ())
}

async fn slack_api_response(response: reqwest::Response) -> Result<serde_json::Value> {
    let status = response.status();
    let value: serde_json::Value =
        response
            .json()
            .await
            .map_err(|error| rua_core::RuaError::Runtime {
                message: format!("Slack API response: {error}"),
            })?;
    if !status.is_success() || value["ok"] != true {
        return Err(rua_core::RuaError::Runtime {
            message: format!("Slack API HTTP {status}: {}", value["error"]),
        });
    }
    Ok(value)
}

async fn slack_upload_remote_url(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    channel: &str,
    url: &str,
    filename: &str,
    thread_ts: Option<&str>,
) -> Result<()> {
    let response = client
        .get(url)
        .bearer_auth(token)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| rua_core::RuaError::Runtime {
            message: format!("Slack remote file: {error}"),
        })?;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| rua_core::RuaError::Runtime {
            message: format!("Slack remote file body: {error}"),
        })?;
    slack_upload_bytes(
        client,
        api_base,
        token,
        channel,
        bytes.to_vec(),
        filename,
        thread_ts,
    )
    .await
}
