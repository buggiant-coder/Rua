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
use crate::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::streaming::{
    PlatformStreamAction, PlatformStreamPolicy, PlatformStreamState, ProcessingIndicator,
    StreamHandler, StreamType,
};
use crate::core::ChannelId;
use crate::http_client::default_http_client;
use rua_core::Result;

use crate::discord::gateway::{self, GatewayConfig, MessageCreateData, DEFAULT_INTENTS};
use crate::discord::types::DiscordConfig;

const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const DISCORD_MAX_MESSAGE_CHARS: usize = 2000;
const MAX_RETRIES: u32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INBOUND_MEDIA_SIZE: u64 = 50 * 1024 * 1024;
const TYPING_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(8);
const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

#[derive(Debug, Default)]
struct DiscordMention {
    is_bot_mentioned: bool,
    tokens: Vec<String>,
}

pub struct DiscordChannel {
    channel_id: ChannelId,
    config: DiscordConfig,
    client: reqwest::Client,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    ws_stop_tx: watch::Sender<bool>,
    ws_handle: RwLock<Option<JoinHandle<()>>>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    media_dir: String,
    api_base: String,
    gateway_url: String,
    event_deduper: Arc<StdMutex<EventDeduper>>,
}

struct DiscordStreamInner {
    state: PlatformStreamState,
    message_id: Option<String>,
}

pub struct DiscordStreamHandler {
    client: reqwest::Client,
    api_base: String,
    bot_token: String,
    channel_id: String,
    policy: PlatformStreamPolicy,
    started_at: tokio::time::Instant,
    inner: Mutex<DiscordStreamInner>,
}

pub struct DiscordTypingIndicator {
    client: reqwest::Client,
    api_base: String,
    bot_token: String,
    channel_id: String,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl DiscordTypingIndicator {
    pub fn new(
        client: reqwest::Client,
        api_base: String,
        bot_token: String,
        channel_id: String,
    ) -> Self {
        Self {
            client,
            api_base: api_base.trim_end_matches('/').to_string(),
            bot_token,
            channel_id,
            task: Mutex::new(None),
        }
    }

    fn typing_url(&self) -> String {
        format!("{}/channels/{}/typing", self.api_base, self.channel_id)
    }

    async fn send_typing(client: &reqwest::Client, url: &str, token: &str) -> Result<()> {
        let resp = client
            .post(url)
            .header("Authorization", format!("Bot {token}"))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("discord typing request failed: {err}"),
            })?;
        if !resp.status().is_success() {
            return Err(rua_core::RuaError::Runtime {
                message: format!("discord typing HTTP {}", resp.status()),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl ProcessingIndicator for DiscordTypingIndicator {
    async fn start(&self, _req: &AgentRequest) -> Result<()> {
        self.stop(_req).await?;
        let url = self.typing_url();
        Self::send_typing(&self.client, &url, &self.bot_token).await?;

        let client = self.client.clone();
        let token = self.bot_token.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(TYPING_KEEPALIVE_INTERVAL).await;
                if let Err(err) = Self::send_typing(&client, &url, &token).await {
                    tracing::debug!(error = %err, "discord typing keepalive failed");
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

impl DiscordStreamHandler {
    pub fn new(
        client: reqwest::Client,
        api_base: String,
        bot_token: String,
        channel_id: String,
        policy: PlatformStreamPolicy,
    ) -> Self {
        Self {
            client,
            api_base: api_base.trim_end_matches('/').to_string(),
            bot_token,
            channel_id,
            policy,
            started_at: tokio::time::Instant::now(),
            inner: Mutex::new(DiscordStreamInner {
                state: PlatformStreamState::default(),
                message_id: None,
            }),
        }
    }

    fn message_url(&self) -> String {
        format!("{}/channels/{}/messages", self.api_base, self.channel_id)
    }

    fn edit_url(&self, message_id: &str) -> String {
        format!(
            "{}/channels/{}/messages/{}",
            self.api_base, self.channel_id, message_id
        )
    }

    async fn post_json(&self, url: String, body: serde_json::Value) -> Result<serde_json::Value> {
        let resp = self
            .client
            .post(url)
            .header("Authorization", format!("Bot {}", self.bot_token))
            .json(&body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("discord streaming request failed: {err}"),
            })?;
        if !resp.status().is_success() {
            return Err(rua_core::RuaError::Runtime {
                message: format!("discord streaming HTTP {}", resp.status()),
            });
        }
        resp.json()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("discord streaming response parse failed: {err}"),
            })
    }

    async fn patch_json(&self, url: String, body: serde_json::Value) -> Result<serde_json::Value> {
        let resp = self
            .client
            .patch(url)
            .header("Authorization", format!("Bot {}", self.bot_token))
            .json(&body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("discord streaming edit failed: {err}"),
            })?;
        if !resp.status().is_success() {
            return Err(rua_core::RuaError::Runtime {
                message: format!("discord streaming edit HTTP {}", resp.status()),
            });
        }
        resp.json()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("discord streaming edit parse failed: {err}"),
            })
    }

    async fn apply_action(&self, action: PlatformStreamAction) -> Result<()> {
        match action {
            PlatformStreamAction::Create { content, .. } => {
                let response = self
                    .post_json(
                        self.message_url(),
                        serde_json::json!({ "content": content }),
                    )
                    .await?;
                let id = response
                    .get("id")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| rua_core::RuaError::Runtime {
                        message: format!("discord streaming create missing id: {response}"),
                    })?
                    .to_string();
                self.inner.lock().await.message_id = Some(id);
            }
            PlatformStreamAction::Update { content, .. } => {
                if let Some(message_id) = self.inner.lock().await.message_id.clone() {
                    let _ = self
                        .patch_json(
                            self.edit_url(&message_id),
                            serde_json::json!({ "content": content }),
                        )
                        .await?;
                }
            }
            PlatformStreamAction::SendFinal { content } => {
                let _ = self
                    .post_json(
                        self.message_url(),
                        serde_json::json!({ "content": content }),
                    )
                    .await?;
            }
            PlatformStreamAction::Skip => {}
        }
        Ok(())
    }
}

#[async_trait]
impl StreamHandler for DiscordStreamHandler {
    async fn on_start(&self, _stream_type: StreamType) -> Result<()> {
        Ok(())
    }

    async fn on_delta(&self, stream_type: StreamType, accumulated: &str) -> Result<()> {
        if stream_type != StreamType::Message {
            return Ok(());
        }
        let mut inner = self.inner.lock().await;
        let action = inner
            .state
            .on_delta(&self.policy, self.started_at.elapsed(), accumulated);
        drop(inner);
        self.apply_action(action).await
    }

    async fn on_end(&self, stream_type: StreamType, accumulated: &str) -> Result<()> {
        if stream_type != StreamType::Message {
            return Ok(());
        }
        let mut inner = self.inner.lock().await;
        let action = inner.state.on_end(&self.policy, accumulated);
        drop(inner);
        self.apply_action(action).await
    }
}

impl DiscordChannel {
    pub fn new(config: DiscordConfig) -> Self {
        let (ws_stop_tx, _) = watch::channel(false);
        let media_dir = config.media_dir.clone();
        Self {
            channel_id: "discord".into(),
            config,
            client: default_http_client(),
            process: RwLock::new(None),
            started: RwLock::new(false),
            ws_stop_tx,
            ws_handle: RwLock::new(None),
            render_options: MessageRenderOptions::default(),
            acl_metadata: serde_json::json!({}),
            media_dir,
            api_base: DISCORD_API_BASE.to_string(),
            gateway_url: "wss://gateway.discord.gg/?v=10&encoding=json".into(),
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

    pub fn with_api_base(mut self, api_base: String) -> Self {
        self.api_base = api_base.trim_end_matches('/').to_string();
        self
    }

    pub fn with_gateway_url(mut self, gateway_url: String) -> Self {
        self.gateway_url = gateway_url;
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
        let mut msg: MessageCreateData = serde_json::from_value(native.clone()).map_err(|err| {
            rua_core::RuaError::Validation {
                message: format!("invalid discord MESSAGE_CREATE payload: {err}"),
            }
        })?;
        download_discord_attachments(&self.client, &self.media_dir, &mut msg).await?;
        *native = serde_json::to_value(msg).map_err(|err| rua_core::RuaError::Runtime {
            message: format!("serialize discord MESSAGE_CREATE payload: {err}"),
        })?;
        Ok(())
    }

    fn normalize_incoming_parts_with_mention(
        msg: &MessageCreateData,
        mention: &DiscordMention,
    ) -> Vec<IncomingContentPart> {
        let mut input = Vec::new();
        let mut text = msg.content.trim().to_string();
        if mention.is_bot_mentioned {
            for token in &mention.tokens {
                text = text.replace(token, "");
            }
            text = text.trim().to_string();
        }
        if !text.is_empty() {
            input.push(IncomingContentPart::Text { text });
        }

        input.extend(msg.attachments.iter().filter_map(|attachment| {
            if let Some(part) = discord_local_media_part(attachment) {
                return Some(part);
            }
            let url = if attachment.url.trim().is_empty() {
                attachment.proxy_url.as_deref().unwrap_or("").trim()
            } else {
                attachment.url.trim()
            };
            if url.is_empty() {
                return None;
            }

            let title = if attachment.filename.trim().is_empty() {
                attachment.content_type.as_deref().unwrap_or("").trim()
            } else {
                attachment.filename.trim()
            };
            Some(IncomingContentPart::Url {
                url: url.to_string(),
                title: if title.is_empty() {
                    None
                } else {
                    Some(title.to_string())
                },
            })
        }));

        input
    }

    fn discord_mention(msg: &MessageCreateData, bot_user_id: Option<&str>) -> DiscordMention {
        let mut mention = DiscordMention {
            is_bot_mentioned: msg.mention_everyone,
            tokens: Vec::new(),
        };
        let Some(bot_user_id) = bot_user_id.map(str::trim).filter(|value| !value.is_empty()) else {
            return mention;
        };
        if msg.mentions.iter().any(|user| user.id == bot_user_id) {
            mention.is_bot_mentioned = true;
            mention.tokens.push(format!("<@{bot_user_id}>"));
            mention.tokens.push(format!("<@!{bot_user_id}>"));
        }
        mention
    }

    fn has_bot_token(&self) -> bool {
        !self.config.bot_token.is_empty()
    }

    pub async fn send_text(&self, channel_id: &str, text: &str) -> Result<()> {
        for chunk in chunk_text(text, DISCORD_MAX_MESSAGE_CHARS) {
            let body = serde_json::json!({ "content": chunk });
            if self.has_bot_token() {
                self.post_bot_message(channel_id, body).await?;
            } else {
                self.post_webhook(body).await?;
            }
        }
        Ok(())
    }

    async fn post_bot_message(&self, channel_id: &str, body: serde_json::Value) -> Result<()> {
        let url = format!("{}/channels/{channel_id}/messages", self.api_base);
        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << (attempt - 1));
                tracing::debug!(
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying discord bot API"
                );
                tokio::time::sleep(delay).await;
            }

            match self
                .client
                .post(&url)
                .header("Authorization", format!("Bot {}", self.config.bot_token))
                .json(&body)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(attempt, "discord bot API send succeeded");
                    return Ok(());
                }
                Ok(resp) => {
                    let status = resp.status();
                    let resp_body = resp.text().await.unwrap_or_default();
                    tracing::warn!(attempt, %status, body = %resp_body, "discord bot API non-success");
                    last_error = Some(format!("HTTP {status}: {resp_body}"));
                }
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "discord bot API failed");
                    last_error = Some(e.to_string());
                }
            }
        }

        Err(rua_core::RuaError::Runtime {
            message: format!(
                "discord bot API failed after {MAX_RETRIES} retries: {}",
                last_error.unwrap_or_default()
            ),
        })
    }

    async fn post_webhook(&self, body: serde_json::Value) -> Result<()> {
        let url = &self.config.webhook_url;
        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << (attempt - 1));
                tracing::debug!(
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying discord webhook"
                );
                tokio::time::sleep(delay).await;
            }

            match self
                .client
                .post(url)
                .json(&body)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(attempt, "discord webhook succeeded");
                    return Ok(());
                }
                Ok(resp) => {
                    let status = resp.status();
                    let resp_body = resp.text().await.unwrap_or_default();
                    tracing::warn!(attempt, %status, body = %resp_body, "discord webhook non-success");
                    last_error = Some(format!("HTTP {status}: {resp_body}"));
                }
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "discord webhook failed");
                    last_error = Some(e.to_string());
                }
            }
        }

        Err(rua_core::RuaError::Runtime {
            message: format!(
                "discord webhook failed after {MAX_RETRIES} retries: {}",
                last_error.unwrap_or_default()
            ),
        })
    }

    async fn post_webhook_multipart(
        &self,
        payload: serde_json::Value,
        filename: String,
        bytes: Vec<u8>,
    ) -> Result<()> {
        let form = reqwest::multipart::Form::new()
            .text("payload_json", payload.to_string())
            .part(
                "files[0]",
                reqwest::multipart::Part::bytes(bytes).file_name(filename),
            );
        self.post_multipart(
            &self.config.webhook_url,
            None,
            form,
            "discord webhook media",
        )
        .await
    }

    async fn post_bot_multipart(
        &self,
        channel_id: &str,
        payload: serde_json::Value,
        filename: String,
        bytes: Vec<u8>,
    ) -> Result<()> {
        let url = format!("{}/channels/{channel_id}/messages", self.api_base);
        let form = reqwest::multipart::Form::new()
            .text("payload_json", payload.to_string())
            .part(
                "files[0]",
                reqwest::multipart::Part::bytes(bytes).file_name(filename),
            );
        let auth = format!("Bot {}", self.config.bot_token);
        self.post_multipart(&url, Some(auth), form, "discord bot media")
            .await
    }

    async fn post_multipart(
        &self,
        url: &str,
        authorization: Option<String>,
        form: reqwest::multipart::Form,
        label: &str,
    ) -> Result<()> {
        let mut req = self
            .client
            .post(url)
            .multipart(form)
            .timeout(REQUEST_TIMEOUT);
        if let Some(auth) = authorization {
            req = req.header("Authorization", auth);
        }
        let resp = req
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("{label} request failed: {err}"),
            })?;
        if resp.status().is_success() {
            return Ok(());
        }
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(rua_core::RuaError::Runtime {
            message: format!("{label} HTTP {status}: {text}"),
        })
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
                OutgoingContentPart::Image { url, path, alt } => {
                    if url.is_none() && path.is_none() {
                        segments.push(alt.as_deref().unwrap_or("[Image]").to_string());
                    }
                }
                OutgoingContentPart::File { url, path, name } => {
                    if url.is_none() && path.is_none() {
                        let label = name.as_deref().unwrap_or("file");
                        segments.push(format!("[File: {label}]"));
                    }
                }
                OutgoingContentPart::Audio { url, path, name } => {
                    if url.is_none() && path.is_none() {
                        if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
                            segments.push(format!("[Audio: {label}]"));
                        } else {
                            segments.push("[Audio]".to_string());
                        }
                    }
                }
                OutgoingContentPart::Video { url, path, name } => {
                    if url.is_none() && path.is_none() {
                        if let Some(label) = name.as_deref().filter(|s| !s.is_empty()) {
                            segments.push(format!("[Video: {label}]"));
                        } else {
                            segments.push("[Video]".to_string());
                        }
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

    fn is_media_part(part: &OutgoingContentPart) -> bool {
        matches!(
            part,
            OutgoingContentPart::Image { .. }
                | OutgoingContentPart::File { .. }
                | OutgoingContentPart::Audio { .. }
                | OutgoingContentPart::Video { .. }
        )
    }

    async fn send_media_part(&self, channel_id: &str, part: &OutgoingContentPart) -> Result<bool> {
        let Some((filename, bytes)) = self.load_media_part(part).await? else {
            return Ok(false);
        };
        let payload = serde_json::json!({ "content": "" });
        if self.has_bot_token() {
            self.post_bot_multipart(channel_id, payload, filename, bytes)
                .await?;
        } else {
            self.post_webhook_multipart(payload, filename, bytes)
                .await?;
        }
        Ok(true)
    }

    async fn load_media_part(
        &self,
        part: &OutgoingContentPart,
    ) -> Result<Option<(String, Vec<u8>)>> {
        match part {
            OutgoingContentPart::Image { url, path, alt } => {
                self.load_media_location(url.as_deref(), path.as_deref(), alt.as_deref())
                    .await
            }
            OutgoingContentPart::File { url, path, name }
            | OutgoingContentPart::Audio { url, path, name }
            | OutgoingContentPart::Video { url, path, name } => {
                self.load_media_location(url.as_deref(), path.as_deref(), name.as_deref())
                    .await
            }
            _ => Ok(None),
        }
    }

    async fn load_media_location(
        &self,
        url: Option<&str>,
        path: Option<&Path>,
        label: Option<&str>,
    ) -> Result<Option<(String, Vec<u8>)>> {
        if let Some(path) = path {
            let bytes = tokio::fs::read(path)
                .await
                .map_err(|err| rua_core::RuaError::Io {
                    message: format!("read discord outbound media {}: {err}", path.display()),
                })?;
            let filename = label
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
                .or_else(|| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "file".to_string());
            return Ok(Some((filename, bytes)));
        }

        let Some(url) = url.filter(|s| !s.trim().is_empty()) else {
            return Ok(None);
        };
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Ok(None);
        }
        let resp = self
            .client
            .get(url)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("download discord outbound media failed: {err}"),
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(rua_core::RuaError::Runtime {
                message: format!("download discord outbound media HTTP {status}"),
            });
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("read discord outbound media response failed: {err}"),
            })?
            .to_vec();
        let filename = label
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                url.rsplit('/')
                    .next()
                    .filter(|name| !name.trim().is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "file".to_string());
        Ok(Some((filename, bytes)))
    }
}

#[async_trait]
impl channel::Channel for DiscordChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("discord channel already started");
            return Ok(());
        }
        *self.process.write().await = Some(process.clone());
        *started = true;

        if !self.has_bot_token() {
            tracing::info!("discord channel started (webhook-only, no bot_token)");
            return Ok(());
        }

        let gw_config = GatewayConfig {
            bot_token: self.config.bot_token.clone(),
            intents: DEFAULT_INTENTS,
        };

        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::Markdown,
            self.render_options.clone(),
        ));
        let acl_metadata = Arc::new(self.acl_metadata.clone());
        let bot_token = self.config.bot_token.clone();
        let api_base = self.api_base.clone();
        let media_dir = self.media_dir.clone();
        let send_client = self.client.clone();
        let send_fn: pipeline::SendFn = Arc::new(move |to, msg| {
            let token = bot_token.clone();
            let api_base = api_base.clone();
            let client = send_client.clone();
            let media_dir = media_dir.clone();
            Box::pin(async move {
                let sender = DiscordChannel::new(DiscordConfig {
                    bot_token: token,
                    media_dir,
                    ..DiscordConfig::default()
                })
                .with_client(client)
                .with_api_base(api_base);
                sender.send(to, msg).await
            })
        });

        let process_for_ws = process;
        let inbound_client = self.client.clone();
        let inbound_media_dir = self.media_dir.clone();
        let stream_client = self.client.clone();
        let stream_api_base = self.api_base.clone();
        let stream_bot_token = self.config.bot_token.clone();
        let bot_user_id = self.config.bot_user_id.clone();
        let event_deduper = self.event_deduper.clone();
        let on_message: gateway::OnMessageFn = Arc::new(move |msg_data: MessageCreateData| {
            let p = process_for_ws.clone();
            let r = renderer.clone();
            let sf = send_fn.clone();
            let acl = acl_metadata.clone();
            let client = inbound_client.clone();
            let media_dir = inbound_media_dir.clone();
            let stream_client = stream_client.clone();
            let stream_api_base = stream_api_base.clone();
            let stream_bot_token = stream_bot_token.clone();
            let bot_user_id = bot_user_id.clone();
            let event_deduper = event_deduper.clone();
            tokio::spawn(async move {
                if bot_user_id.as_deref() == Some(msg_data.author.id.as_str())
                    || (msg_data.author.bot.unwrap_or(false) && acl["accept_bot_messages"] != true)
                {
                    return;
                }
                let mut msg_data = msg_data;
                match discord_check_duplicate_event(&event_deduper, &msg_data.id) {
                    Ok(true) => {
                        tracing::debug!(message_id = %msg_data.id, "discord duplicate event skipped");
                        return;
                    }
                    Ok(false) => {}
                    Err(err) => {
                        tracing::warn!(error = %err, "discord event dedup failed");
                    }
                }
                if let Err(err) =
                    download_discord_attachments(&client, &media_dir, &mut msg_data).await
                {
                    tracing::warn!(error = %err, "discord inbound attachment download failed; using URL fallbacks");
                }
                let mention = Self::discord_mention(&msg_data, bot_user_id.as_deref());
                let mut req = AgentRequest {
                    agent_id: String::new(),
                    user_id: msg_data.author.id.clone(),
                    session_id: msg_data.channel_id.clone(),
                    input: Self::normalize_incoming_parts_with_mention(&msg_data, &mention),
                    channel_id: "discord".into(),
                    conversation_id: msg_data.channel_id.clone(),
                    message_id: Some(msg_data.id.clone()),
                    metadata: serde_json::json!({
                        "guild_id": msg_data.guild_id,
                        "username": msg_data.author.username,
                        "is_group": msg_data.guild_id.is_some(),
                        "bot_mentioned": mention.is_bot_mentioned,
                        "mentioned": mention.is_bot_mentioned,
                    }),
                };

                if let (Some(acl_obj), Some(meta)) = (acl.as_object(), req.metadata.as_object_mut())
                {
                    for (k, v) in acl_obj {
                        meta.insert(k.clone(), v.clone());
                    }
                }

                let handler = Arc::new(DiscordStreamHandler::new(
                    stream_client,
                    stream_api_base.clone(),
                    stream_bot_token.clone(),
                    msg_data.channel_id.clone(),
                    PlatformStreamPolicy::placeholder_edit(
                        Duration::from_secs(1),
                        DISCORD_MAX_MESSAGE_CHARS,
                    ),
                ));
                let indicator = Arc::new(DiscordTypingIndicator::new(
                    client,
                    stream_api_base,
                    stream_bot_token,
                    msg_data.channel_id,
                ));
                let result = pipeline::run_inbound_streaming_with_indicator(
                    p,
                    req,
                    None,
                    r,
                    sf,
                    Some(handler),
                    Some(indicator),
                )
                .await;
                if let Err(e) = result {
                    tracing::error!("discord pipeline error: {e}");
                }
            });
        });

        self.ws_stop_tx.send_replace(false);
        let stop_rx = self.ws_stop_tx.subscribe();
        let handle = gateway::spawn_gateway_task_with_client(
            gw_config,
            on_message,
            stop_rx,
            self.client.clone(),
            self.gateway_url.clone(),
        );
        *self.ws_handle.write().await = Some(handle);

        tracing::info!("discord channel started (bot gateway)");
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
        tracing::info!("discord channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        let text = Self::render_text_and_unresolved_media(&msg.content);
        if text.is_empty() {
            tracing::debug!("empty text content, checking discord media parts");
        }

        let channel_id = &to.conversation_id;
        if !text.is_empty() {
            if self.has_bot_token() {
                for chunk in chunk_text(&text, DISCORD_MAX_MESSAGE_CHARS) {
                    self.post_bot_message(channel_id, serde_json::json!({ "content": chunk }))
                        .await?;
                }
            } else {
                for chunk in chunk_text(&text, DISCORD_MAX_MESSAGE_CHARS) {
                    self.post_webhook(serde_json::json!({ "content": chunk }))
                        .await?;
                }
            }
        }

        for part in &msg.content {
            if !Self::is_media_part(part) {
                continue;
            }
            match self.send_media_part(channel_id, part).await {
                Ok(true) => {}
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "discord media send failed; sending fallback text");
                    let fallback = Self::render_content(std::slice::from_ref(part));
                    if !fallback.is_empty() {
                        self.send_text(channel_id, &fallback).await?;
                    }
                }
            }
        }
        Ok(())
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        let msg: MessageCreateData =
            serde_json::from_value(native).map_err(|e| rua_core::RuaError::Validation {
                message: format!("invalid discord MESSAGE_CREATE payload: {e}"),
            })?;
        if discord_check_duplicate_event(&self.event_deduper, &msg.id)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate discord event".into(),
            });
        }
        let mention = Self::discord_mention(&msg, self.config.bot_user_id.as_deref());

        Ok(AgentRequest {
            agent_id: String::new(),
            user_id: msg.author.id.clone(),
            session_id: msg.channel_id.clone(),
            input: Self::normalize_incoming_parts_with_mention(&msg, &mention),
            channel_id: "discord".into(),
            conversation_id: msg.channel_id,
            message_id: Some(msg.id),
            metadata: serde_json::json!({
                "guild_id": msg.guild_id,
                "username": msg.author.username,
                "is_group": msg.guild_id.is_some(),
                "bot_mentioned": mention.is_bot_mentioned,
                "mentioned": mention.is_bot_mentioned,
            }),
        })
    }
}

fn discord_check_duplicate_event(
    deduper: &Arc<StdMutex<EventDeduper>>,
    message_id: &str,
) -> Result<bool> {
    let message_id = message_id.trim();
    if message_id.is_empty() {
        return Ok(false);
    }
    let mut deduper = deduper.lock().map_err(|_| rua_core::RuaError::Runtime {
        message: "discord event deduper lock poisoned".into(),
    })?;
    Ok(
        deduper.check_and_mark(format!("discord:message:{message_id}"))
            == EventDedupDecision::Duplicate,
    )
}

fn discord_local_media_part(
    attachment: &gateway::MessageAttachment,
) -> Option<IncomingContentPart> {
    let path = attachment
        .local_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)?;
    let file_id = if attachment.id.trim().is_empty() {
        attachment.filename.trim()
    } else {
        attachment.id.trim()
    };
    if file_id.is_empty() {
        return None;
    }
    let mime = attachment
        .content_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let name = if attachment.filename.trim().is_empty() {
        None
    } else {
        Some(attachment.filename.trim().to_string())
    };

    if mime
        .as_deref()
        .is_some_and(|value| value.starts_with("image/"))
    {
        Some(IncomingContentPart::Image {
            file_id: file_id.to_string(),
            path,
            mime,
        })
    } else if mime
        .as_deref()
        .is_some_and(|value| value.starts_with("audio/"))
    {
        Some(IncomingContentPart::Audio {
            file_id: file_id.to_string(),
            path,
            mime,
            name,
        })
    } else if mime
        .as_deref()
        .is_some_and(|value| value.starts_with("video/"))
    {
        Some(IncomingContentPart::Video {
            file_id: file_id.to_string(),
            path,
            mime,
            name,
        })
    } else {
        Some(IncomingContentPart::File {
            file_id: file_id.to_string(),
            path,
            mime,
            name,
        })
    }
}

async fn download_discord_attachments(
    client: &reqwest::Client,
    media_dir: &str,
    msg: &mut MessageCreateData,
) -> Result<()> {
    for attachment in &mut msg.attachments {
        if attachment
            .local_path
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        {
            continue;
        }
        let url = if attachment.url.trim().is_empty() {
            attachment.proxy_url.as_deref().unwrap_or("").trim()
        } else {
            attachment.url.trim()
        };
        if url.is_empty() {
            continue;
        }
        let Some((bytes, mime)) = discord_download_attachment(client, url).await? else {
            continue;
        };
        let filename = discord_download_filename(attachment);
        let dir = discord_media_dir(media_dir);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("create discord media dir {}: {err}", dir.display()),
            })?;
        let final_path = dir.join(filename);
        atomic_write_file(&final_path, &bytes).await?;
        attachment.local_path = Some(final_path.to_string_lossy().to_string());
        if attachment.content_type.as_deref().unwrap_or("").is_empty() {
            attachment.content_type = mime;
        }
    }
    Ok(())
}

async fn discord_download_attachment(
    client: &reqwest::Client,
    url: &str,
) -> Result<Option<(Vec<u8>, Option<String>)>> {
    let resp = match client.get(url).timeout(REQUEST_TIMEOUT).send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(url = %url, error = %err, "discord attachment download failed");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(url = %url, status = %resp.status(), "discord attachment download non-success");
        return Ok(None);
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_INBOUND_MEDIA_SIZE {
            tracing::warn!(url = %url, len, "discord attachment too large");
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
            message: format!("discord attachment download bytes failed: {err}"),
        })?;
    if bytes.len() as u64 > MAX_INBOUND_MEDIA_SIZE {
        tracing::warn!(url = %url, len = bytes.len(), "discord attachment too large");
        return Ok(None);
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some((bytes.to_vec(), mime)))
}

fn discord_media_dir(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    if media_dir.is_empty() {
        std::env::temp_dir().join("rua-discord-media")
    } else {
        PathBuf::from(media_dir)
    }
}

fn discord_download_filename(attachment: &gateway::MessageAttachment) -> String {
    let hint = if attachment.filename.trim().is_empty() {
        attachment.id.trim()
    } else {
        attachment.filename.trim()
    };
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
            .unwrap_or_else(|| "discord-media".into()),
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

fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    if text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }

    const FENCE_CLOSE: &str = "\n```";
    let fence_close_chars = FENCE_CLOSE.chars().count();
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;
    let mut fence_open: Option<String> = None;

    for line in text.split('\n') {
        let line_with_newline = format!("{line}\n");
        let line_chars = line_with_newline.chars().count();
        let stripped = line.trim();
        let fence_before_line = fence_open.clone();
        let reserved = if fence_before_line.is_some() {
            fence_close_chars
        } else {
            0
        };
        if !current.is_empty() && current_chars + line_chars + reserved > max_chars {
            let saved_fence = fence_before_line.clone();
            flush_discord_chunk(&mut chunks, &mut current, saved_fence.as_deref());
            current_chars = 0;

            if let Some(reopener) = saved_fence {
                fence_open = Some(reopener.clone());
                current.push_str(&reopener);
                current.push('\n');
                current_chars += reopener.chars().count() + 1;
            }
        }

        if stripped.starts_with("```") {
            if fence_open.is_some() {
                fence_open = None;
            } else {
                fence_open = Some(stripped.to_string());
            }
        }

        if line_chars > max_chars {
            for part in split_discord_long_line(line, max_chars) {
                chunks.push(part);
            }
            continue;
        }

        current.push_str(&line_with_newline);
        current_chars += line_chars;
    }

    if !current.is_empty() {
        chunks.push(current.trim_end_matches('\n').to_string());
    }
    chunks
}

fn flush_discord_chunk(chunks: &mut Vec<String>, current: &mut String, fence_open: Option<&str>) {
    let mut body = current.trim_end_matches('\n').to_string();
    if fence_open.is_some() {
        body.push_str("\n```");
    }
    if !body.trim().is_empty() {
        chunks.push(body);
    }
    current.clear();
}

fn split_discord_long_line(line: &str, max_chars: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let split_at = byte_index_after_chars(rest, max_chars);
        parts.push(rest[..split_at].to_string());
        rest = &rest[split_at..];
    }
    parts
}

fn byte_index_after_chars(text: &str, count: usize) -> usize {
    text.char_indices()
        .nth(count)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}
