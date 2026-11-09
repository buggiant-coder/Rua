use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::core::approval_card::{
    build_callback_data, parse_callback_data, ApprovalCardAction, ApprovalCardCallback,
    ApprovalCardResolveError, ApprovalCardResolver,
};
use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::content::{AgentChannelRequest, IncomingContentPart, OutgoingContentPart};
use crate::core::event_dedup::{EventDedupDecision, EventDeduper};
use crate::core::pipeline;
use crate::core::renderer::{ChannelKind, MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::streaming::{
    PlatformStreamAction, PlatformStreamPolicy, PlatformStreamState, ProcessingIndicator,
    StreamHandler, StreamType,
};
use crate::core::{BufferDebouncer, ChannelId};
use crate::http_client::default_http_client;
use crate::telegram::format_html::markdown_to_telegram_html;
use crate::telegram::types::TelegramConfig;
use rua_core::Result;

const MAX_RETRIES: u32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_TIMEOUT: u64 = 20;
const POLL_READ_TIMEOUT: Duration = Duration::from_secs(POLL_TIMEOUT + 10);
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(500);
const SEND_MESSAGE_CHUNK_BYTES: usize = 4000;
const MAX_INBOUND_MEDIA_SIZE: u64 = 50 * 1024 * 1024;
const TYPING_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(4);
const APPROVAL_CALLBACK_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

pub struct TelegramChannel {
    channel_id: ChannelId,
    config: TelegramConfig,
    client: reqwest::Client,
    base_url: String,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    poll_stop_tx: watch::Sender<bool>,
    poll_handle: RwLock<Option<JoinHandle<()>>>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    debouncer: BufferDebouncer<String, AgentChannelRequest>,
    approval_resolver: Arc<std::sync::Mutex<ApprovalCardResolver>>,
    event_deduper: Arc<std::sync::Mutex<EventDeduper>>,
}

struct TelegramStreamInner {
    state: PlatformStreamState,
    message_id: Option<i64>,
}

pub struct TelegramTypingIndicator {
    client: reqwest::Client,
    base_url: String,
    bot_token: String,
    chat_id: String,
    message_thread_id: Option<serde_json::Value>,
    keepalive_interval: Duration,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl TelegramTypingIndicator {
    pub fn new(
        client: reqwest::Client,
        base_url: String,
        bot_token: String,
        chat_id: String,
        message_thread_id: Option<serde_json::Value>,
    ) -> Self {
        Self::with_keepalive_interval(
            client,
            base_url,
            bot_token,
            chat_id,
            message_thread_id,
            TYPING_KEEPALIVE_INTERVAL,
        )
    }

    pub fn with_keepalive_interval(
        client: reqwest::Client,
        base_url: String,
        bot_token: String,
        chat_id: String,
        message_thread_id: Option<serde_json::Value>,
        keepalive_interval: Duration,
    ) -> Self {
        Self {
            client,
            base_url,
            bot_token,
            chat_id,
            message_thread_id,
            keepalive_interval,
            task: Mutex::new(None),
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.base_url, self.bot_token, method)
    }

    fn action_body(&self) -> serde_json::Value {
        let mut body = serde_json::json!({
            "chat_id": self.chat_id,
            "action": "typing",
        });
        if let Some(thread_id) = &self.message_thread_id {
            body["message_thread_id"] = thread_id.clone();
        }
        body
    }

    async fn send_action(
        client: &reqwest::Client,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        let resp = client
            .post(url)
            .json(body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("telegram sendChatAction failed: {err}"),
            })?;
        if !resp.status().is_success() {
            return Err(rua_core::RuaError::Runtime {
                message: format!("telegram sendChatAction HTTP {}", resp.status()),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl ProcessingIndicator for TelegramTypingIndicator {
    async fn start(&self, _req: &AgentChannelRequest) -> Result<()> {
        self.stop(_req).await?;
        let url = self.api_url("sendChatAction");
        let body = self.action_body();
        Self::send_action(&self.client, &url, &body).await?;

        let client = self.client.clone();
        let task_url = url.clone();
        let task_body = body.clone();
        let keepalive_interval = self.keepalive_interval;
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(keepalive_interval).await;
                if let Err(err) = Self::send_action(&client, &task_url, &task_body).await {
                    tracing::debug!(error = %err, "telegram typing keepalive failed");
                }
            }
        });
        *self.task.lock().await = Some(handle);
        Ok(())
    }

    async fn stop(&self, _req: &AgentChannelRequest) -> Result<()> {
        if let Some(handle) = self.task.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }
        Ok(())
    }
}

pub struct TelegramStreamHandler {
    client: reqwest::Client,
    base_url: String,
    bot_token: String,
    chat_id: String,
    message_thread_id: Option<serde_json::Value>,
    policy: PlatformStreamPolicy,
    started_at: tokio::time::Instant,
    inner: Mutex<TelegramStreamInner>,
}

impl TelegramStreamHandler {
    pub fn new(
        client: reqwest::Client,
        base_url: String,
        bot_token: String,
        chat_id: String,
        message_thread_id: Option<serde_json::Value>,
        policy: PlatformStreamPolicy,
    ) -> Self {
        Self {
            client,
            base_url,
            bot_token,
            chat_id,
            message_thread_id,
            policy,
            started_at: tokio::time::Instant::now(),
            inner: Mutex::new(TelegramStreamInner {
                state: PlatformStreamState::default(),
                message_id: None,
            }),
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.base_url, self.bot_token, method)
    }

    async fn post_api_json(
        &self,
        method: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let resp = self
            .client
            .post(self.api_url(method))
            .json(&body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("telegram streaming {method} failed: {err}"),
            })?;
        if !resp.status().is_success() {
            return Err(rua_core::RuaError::Runtime {
                message: format!("telegram streaming {method} HTTP {}", resp.status()),
            });
        }
        resp.json()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("telegram streaming {method} parse failed: {err}"),
            })
    }

    fn message_body(&self, text: &str) -> serde_json::Value {
        let mut body = serde_json::json!({
            "chat_id": self.chat_id,
            "text": markdown_to_telegram_html(text),
            "parse_mode": "HTML",
        });
        if let Some(thread_id) = &self.message_thread_id {
            body["message_thread_id"] = thread_id.clone();
        }
        body
    }

    async fn create_placeholder(&self, content: &str) -> Result<i64> {
        let body = self.message_body(content);
        let response = self.post_api_json("sendMessage", body).await?;
        response
            .get("result")
            .and_then(|value| value.get("message_id"))
            .and_then(|value| value.as_i64())
            .ok_or_else(|| rua_core::RuaError::Runtime {
                message: format!("telegram streaming sendMessage missing message_id: {response}"),
            })
    }

    async fn edit_message(&self, message_id: i64, content: &str) -> Result<()> {
        let mut body = self.message_body(content);
        body["message_id"] = serde_json::json!(message_id);
        self.post_api_json("editMessageText", body).await?;
        Ok(())
    }

    async fn send_final(&self, content: &str) -> Result<()> {
        self.post_api_json("sendMessage", self.message_body(content))
            .await?;
        Ok(())
    }

    async fn apply_action(&self, action: PlatformStreamAction) -> Result<()> {
        match action {
            PlatformStreamAction::Create { content, .. } => {
                let message_id = self.create_placeholder(&content).await?;
                self.inner.lock().await.message_id = Some(message_id);
            }
            PlatformStreamAction::Update {
                content,
                final_update,
                ..
            } => {
                if let Some(message_id) = self.inner.lock().await.message_id {
                    self.edit_message(message_id, &content).await?;
                } else if final_update {
                    self.send_final(&content).await?;
                }
            }
            PlatformStreamAction::SendFinal { content } => {
                self.send_final(&content).await?;
            }
            PlatformStreamAction::Skip => {}
        }
        Ok(())
    }
}

#[async_trait]
impl StreamHandler for TelegramStreamHandler {
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

impl TelegramChannel {
    pub fn new(config: TelegramConfig) -> Self {
        let (poll_stop_tx, _) = watch::channel(false);
        let render_options = MessageRenderOptions::for_channel_kind(ChannelKind::Telegram);
        Self {
            channel_id: "telegram".into(),
            config,
            client: default_http_client(),
            base_url: "https://api.telegram.org".into(),
            process: RwLock::new(None),
            started: RwLock::new(false),
            poll_stop_tx,
            poll_handle: RwLock::new(None),
            render_options,
            acl_metadata: serde_json::json!({}),
            debouncer: BufferDebouncer::new(DEBOUNCE_WINDOW),
            approval_resolver: Arc::new(std::sync::Mutex::new(ApprovalCardResolver::new(
                APPROVAL_CALLBACK_TTL,
            ))),
            event_deduper: Arc::new(std::sync::Mutex::new(EventDeduper::new(
                EVENT_DEDUP_TTL,
                EVENT_DEDUP_CAPACITY,
            ))),
        }
    }

    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
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

    pub async fn download_inbound_media(&self, update: &mut serde_json::Value) -> Result<()> {
        download_inbound_media_with(
            &self.client,
            &self.base_url,
            &self.config.bot_token,
            &self.config.media_dir,
            update,
        )
        .await
    }

    fn api_url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.base_url, self.config.bot_token, method)
    }

    pub fn build_send_message_body(
        chat_id: &str,
        text: &str,
        approval_request_id: Option<&str>,
    ) -> serde_json::Value {
        let html = markdown_to_telegram_html(text);
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            "text": html,
            "parse_mode": "HTML"
        });
        if let Some(request_id) = approval_request_id.filter(|id| !id.is_empty()) {
            body["reply_markup"] = serde_json::json!({
                "inline_keyboard": [[
                    {
                        "text": "批准",
                        "callback_data": build_callback_data(ApprovalCardAction::Approve, request_id)
                    },
                    {
                        "text": "拒绝",
                        "callback_data": build_callback_data(ApprovalCardAction::Deny, request_id)
                    }
                ]]
            });
        }
        body
    }

    pub fn approval_resolved_text(
        tool_name: &str,
        action: ApprovalCardAction,
        decided_by: &str,
    ) -> String {
        let status = match action {
            ApprovalCardAction::Approve => "✅ Approved",
            ApprovalCardAction::Deny => "🚫 Denied",
        };
        if decided_by.is_empty() {
            format!("{status} | Tool: {tool_name}")
        } else {
            format!("{status} by {decided_by} | Tool: {tool_name}")
        }
    }

    pub async fn answer_approval_callback(
        &self,
        callback_query_id: &str,
        text: &str,
    ) -> Result<()> {
        self.post_api(
            "answerCallbackQuery",
            serde_json::json!({
                "callback_query_id": callback_query_id,
                "text": text,
                "show_alert": false,
            }),
        )
        .await
    }

    pub async fn update_approval_message(
        &self,
        chat_id: &str,
        message_id: i64,
        text: &str,
    ) -> Result<()> {
        self.post_api(
            "editMessageText",
            serde_json::json!({
                "chat_id": chat_id,
                "message_id": message_id,
                "text": text,
            }),
        )
        .await
    }

    pub async fn send_document(
        &self,
        file_path: &std::path::Path,
        caption: Option<&str>,
    ) -> Result<()> {
        let url = self.api_url("sendDocument");
        let file_name = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        let file_bytes =
            tokio::fs::read(file_path)
                .await
                .map_err(|e| rua_core::RuaError::Runtime {
                    message: format!("failed to read file {}: {e}", file_path.display()),
                })?;

        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << (attempt - 1));
                tracing::debug!(
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying telegram sendDocument"
                );
                tokio::time::sleep(delay).await;
            }

            let mut form = reqwest::multipart::Form::new()
                .text("chat_id", self.config.chat_id.clone())
                .part(
                    "document",
                    reqwest::multipart::Part::bytes(file_bytes.clone())
                        .file_name(file_name.clone()),
                );
            if let Some(cap) = caption {
                form = form.text("caption", cap.to_string());
            }

            match self
                .client
                .post(&url)
                .multipart(form)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(attempt, "telegram sendDocument succeeded");
                    return Ok(());
                }
                Ok(resp) => {
                    let status = resp.status();
                    let resp_body = resp.text().await.unwrap_or_default();
                    tracing::warn!(attempt, %status, body = %resp_body, "telegram sendDocument non-success");
                    last_error = Some(format!("HTTP {status}: {resp_body}"));
                }
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "telegram sendDocument failed");
                    last_error = Some(e.to_string());
                }
            }
        }

        Err(rua_core::RuaError::Runtime {
            message: format!(
                "telegram sendDocument failed after {MAX_RETRIES} retries: {}",
                last_error.unwrap_or_default()
            ),
        })
    }

    async fn send_media_multipart(
        &self,
        method: &str,
        payload_name: &str,
        file_path: &std::path::Path,
        chat_id: &str,
        message_thread_id: Option<&serde_json::Value>,
    ) -> Result<()> {
        let url = self.api_url(method);
        let file_name = file_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| payload_name.to_string());
        let file_bytes =
            tokio::fs::read(file_path)
                .await
                .map_err(|err| rua_core::RuaError::Runtime {
                    message: format!("failed to read file {}: {err}", file_path.display()),
                })?;
        let mut form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part(
                payload_name.to_string(),
                reqwest::multipart::Part::bytes(file_bytes).file_name(file_name),
            );
        if let Some(thread_id) = message_thread_id {
            form = form.text("message_thread_id", thread_id.to_string());
        }
        self.post_multipart(method, &url, form).await
    }

    async fn send_media_url(
        &self,
        method: &str,
        payload_name: &str,
        url_value: &str,
        chat_id: &str,
        message_thread_id: Option<&serde_json::Value>,
    ) -> Result<()> {
        let mut body = serde_json::json!({
            "chat_id": chat_id,
            payload_name: url_value,
        });
        if let Some(thread_id) = message_thread_id {
            body["message_thread_id"] = thread_id.clone();
        }
        self.post_api(method, body).await
    }

    async fn post_multipart(
        &self,
        method: &str,
        url: &str,
        form: reqwest::multipart::Form,
    ) -> Result<()> {
        match self
            .client
            .post(url)
            .multipart(form)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!("telegram {method} multipart succeeded");
                Ok(())
            }
            Ok(resp) => {
                let status = resp.status();
                let resp_body = resp.text().await.unwrap_or_default();
                tracing::warn!(%status, body = %resp_body, "telegram {method} multipart non-success");
                Err(rua_core::RuaError::Runtime {
                    message: format!(
                        "telegram {method} multipart failed: HTTP {status}: {resp_body}"
                    ),
                })
            }
            Err(err) => {
                tracing::warn!(error = %err, "telegram {method} multipart failed");
                Err(rua_core::RuaError::Runtime {
                    message: format!("telegram {method} multipart failed: {err}"),
                })
            }
        }
    }

    async fn post_api(&self, method: &str, body: serde_json::Value) -> Result<()> {
        self.post_api_json(method, body).await.map(|_| ())
    }

    async fn post_api_json(
        &self,
        method: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let url = self.api_url(method);
        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << (attempt - 1));
                tracing::debug!(
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying telegram {method}"
                );
                tokio::time::sleep(delay).await;
            }

            match self
                .client
                .post(&url)
                .json(&body)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(attempt, "telegram {method} succeeded");
                    return resp
                        .json()
                        .await
                        .map_err(|err| rua_core::RuaError::Runtime {
                            message: format!("telegram {method} response parse failed: {err}"),
                        });
                }
                Ok(resp) => {
                    let status = resp.status();
                    let resp_body = resp.text().await.unwrap_or_default();
                    tracing::warn!(attempt, %status, body = %resp_body, "telegram {method} non-success");
                    last_error = Some(format!("HTTP {status}: {resp_body}"));
                }
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "telegram {method} failed");
                    last_error = Some(e.to_string());
                }
            }
        }

        Err(rua_core::RuaError::Runtime {
            message: format!(
                "telegram {method} failed after {MAX_RETRIES} retries: {}",
                last_error.unwrap_or_default()
            ),
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

    fn render_text_and_fallback_parts(parts: &[OutgoingContentPart]) -> String {
        let mut segments = Vec::new();
        for part in parts {
            match part {
                OutgoingContentPart::Text { text } => segments.push(text.clone()),
                OutgoingContentPart::Markdown { text } => segments.push(text.clone()),
                OutgoingContentPart::Refusal { text } => segments.push(text.clone()),
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

    async fn send_media_part(
        &self,
        part: &OutgoingContentPart,
        chat_id: &str,
        message_thread_id: Option<&serde_json::Value>,
    ) -> Result<()> {
        let (method, payload_name, url, path) = match part {
            OutgoingContentPart::Image { url, path, .. } => {
                ("sendPhoto", "photo", url.as_deref(), path.as_deref())
            }
            OutgoingContentPart::File { url, path, .. } => {
                ("sendDocument", "document", url.as_deref(), path.as_deref())
            }
            OutgoingContentPart::Audio { url, path, .. } => {
                ("sendAudio", "audio", url.as_deref(), path.as_deref())
            }
            OutgoingContentPart::Video { url, path, .. } => {
                ("sendVideo", "video", url.as_deref(), path.as_deref())
            }
            _ => return Ok(()),
        };
        if let Some(path) = path {
            return self
                .send_media_multipart(method, payload_name, path, chat_id, message_thread_id)
                .await;
        }
        if let Some(url) = url.filter(|value| !value.trim().is_empty()) {
            return self
                .send_media_url(method, payload_name, url, chat_id, message_thread_id)
                .await;
        }
        Ok(())
    }

    pub fn parse_update(
        update: &serde_json::Value,
        allowed_users: &[String],
    ) -> Option<AgentChannelRequest> {
        Self::parse_update_with_bot_username(update, allowed_users, None)
    }

    fn parse_update_with_bot_username(
        update: &serde_json::Value,
        allowed_users: &[String],
        bot_username: Option<&str>,
    ) -> Option<AgentChannelRequest> {
        if let Some(callback) = update.get("callback_query") {
            return Self::parse_callback_query(callback, allowed_users);
        }

        let message = update.get("message")?;
        let (input, bot_mentioned) = telegram_input_parts(message, bot_username);
        if input.is_empty() {
            return None;
        }

        let chat = message.get("chat")?;
        let chat_id = chat.get("id")?.as_i64()?.to_string();
        let chat_type = chat
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("private")
            .to_string();
        let is_group = is_group_chat_type(&chat_type);

        let from = message.get("from")?;
        let from_id = from.get("id")?.as_i64()?.to_string();
        let user_name = telegram_user_name(from);

        if !allowed_users.is_empty() && !allowed_users.contains(&from_id) {
            tracing::debug!(
                user_id = %from_id,
                "telegram user not in allowed_users, skipping"
            );
            return None;
        }

        let message_id = message
            .get("message_id")
            .and_then(|v| v.as_i64())
            .map(|id| id.to_string());
        let message_thread_id = message.get("message_thread_id").cloned();
        let has_bot_command = telegram_has_bot_command(message);

        let mut metadata = serde_json::json!({
            "chat_id": chat_id,
            "chat_type": chat_type,
            "is_group": is_group,
            "has_bot_command": has_bot_command,
            "bot_mentioned": bot_mentioned,
            "mentioned": bot_mentioned,
            "user_name": user_name,
        });
        if let Some(thread_id) = message_thread_id {
            metadata["message_thread_id"] = thread_id;
        }

        Some(AgentChannelRequest {
            agent_id: String::new(),
            user_id: from_id,
            session_id: chat_id.clone(),
            input,
            channel_id: "telegram".into(),
            conversation_id: chat_id.clone(),
            message_id,
            metadata,
        })
    }

    fn parse_update_with_resolver_and_bot_username(
        update: &serde_json::Value,
        allowed_users: &[String],
        resolver: &Arc<std::sync::Mutex<ApprovalCardResolver>>,
        bot_username: Option<&str>,
    ) -> Result<Option<AgentChannelRequest>> {
        if let Some(callback) = update.get("callback_query") {
            return Self::parse_callback_query_with_resolver(
                callback,
                allowed_users,
                Some(resolver),
            );
        }
        Ok(Self::parse_update_with_bot_username(
            update,
            allowed_users,
            bot_username,
        ))
    }

    fn parse_callback_query(
        callback: &serde_json::Value,
        allowed_users: &[String],
    ) -> Option<AgentChannelRequest> {
        Self::parse_callback_query_with_resolver(callback, allowed_users, None)
            .ok()
            .flatten()
    }

    fn parse_callback_query_with_resolver(
        callback: &serde_json::Value,
        allowed_users: &[String],
        resolver: Option<&Arc<std::sync::Mutex<ApprovalCardResolver>>>,
    ) -> Result<Option<AgentChannelRequest>> {
        let Some(data) = callback.get("data").and_then(|value| value.as_str()) else {
            return Ok(None);
        };

        let Some(from) = callback.get("from") else {
            return Ok(None);
        };
        let Some(from_id) = from.get("id").and_then(|value| value.as_i64()) else {
            return Ok(None);
        };
        let from_id = from_id.to_string();
        if !allowed_users.is_empty() && !allowed_users.contains(&from_id) {
            return Ok(None);
        }
        let user_name = telegram_user_name(from);
        let parsed = match parse_callback_data("telegram", data, &from_id) {
            Ok(parsed) => parsed,
            Err(_) => return Ok(None),
        };
        let text = if let Some(resolver) = resolver {
            let resolution = resolver
                .lock()
                .map_err(|_| rua_core::RuaError::Runtime {
                    message: "telegram approval resolver lock poisoned".into(),
                })?
                .resolve(ApprovalCardCallback {
                    request_id: parsed.request_id.clone(),
                    action: parsed.action.clone(),
                    decided_by: from_id.clone(),
                    platform: "telegram".into(),
                })
                .map_err(|err| match err {
                    ApprovalCardResolveError::Duplicate => rua_core::RuaError::Validation {
                        message: format!("duplicate approval callback: {}", parsed.request_id),
                    },
                    ApprovalCardResolveError::MissingRequestId => rua_core::RuaError::Validation {
                        message: "approval callback missing request id".into(),
                    },
                    ApprovalCardResolveError::Stale => rua_core::RuaError::Validation {
                        message: format!("stale approval callback: {}", parsed.request_id),
                    },
                })?;
            resolution.command_text()
        } else {
            match parsed.action {
                ApprovalCardAction::Approve => format!("/approval approve {}", parsed.request_id),
                ApprovalCardAction::Deny => format!("/approval deny {}", parsed.request_id),
            }
        };

        let Some(message) = callback.get("message") else {
            return Ok(None);
        };
        let Some(chat) = message.get("chat") else {
            return Ok(None);
        };
        let Some(chat_id) = chat.get("id").and_then(|value| value.as_i64()) else {
            return Ok(None);
        };
        let chat_id = chat_id.to_string();
        let chat_type = chat
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("private")
            .to_string();
        let is_group = is_group_chat_type(&chat_type);
        let message_id = message
            .get("message_id")
            .and_then(|v| v.as_i64())
            .map(|id| id.to_string());
        let message_thread_id = message.get("message_thread_id").cloned();

        let mut metadata = serde_json::json!({
            "chat_id": chat_id,
            "chat_type": chat_type,
            "is_group": is_group,
            "telegram_callback_data": data,
            "telegram_callback_query_id": callback.get("id").and_then(|value| value.as_str()).unwrap_or(""),
            "approval_request_id": parsed.request_id,
            "approval_action": match parsed.action {
                ApprovalCardAction::Approve => "approve",
                ApprovalCardAction::Deny => "deny",
            },
            "approval_tool_name": "tool",
            "user_name": user_name,
            "from_card_action": true,
        });
        if let Some(thread_id) = message_thread_id {
            metadata["message_thread_id"] = thread_id;
        }

        Ok(Some(AgentChannelRequest {
            agent_id: String::new(),
            user_id: from_id,
            session_id: chat_id.clone(),
            input: vec![IncomingContentPart::Text { text }],
            channel_id: "telegram".into(),
            conversation_id: chat_id.clone(),
            message_id,
            metadata,
        }))
    }
}

fn is_group_chat_type(chat_type: &str) -> bool {
    matches!(chat_type, "group" | "supergroup")
}

fn telegram_check_duplicate_event(
    deduper: &Arc<std::sync::Mutex<EventDeduper>>,
    update: &serde_json::Value,
) -> Result<bool> {
    let Some(key) = telegram_event_key(update) else {
        return Ok(false);
    };
    let mut deduper = deduper.lock().map_err(|_| rua_core::RuaError::Runtime {
        message: "telegram event deduper lock poisoned".into(),
    })?;
    Ok(deduper.check_and_mark(&key) == EventDedupDecision::Duplicate)
}

fn telegram_event_key(update: &serde_json::Value) -> Option<String> {
    if update.get("callback_query").is_some() {
        return None;
    }
    if let Some(update_id) = update.get("update_id").and_then(|value| value.as_i64()) {
        return Some(format!("telegram:update:{update_id}"));
    }
    let message = update.get("message")?;
    let chat_id = message.pointer("/chat/id")?;
    let message_id = message.get("message_id")?;
    Some(format!("telegram:message:{chat_id}:{message_id}"))
}

fn telegram_has_bot_command(message: &serde_json::Value) -> bool {
    ["entities", "caption_entities"]
        .into_iter()
        .filter_map(|key| message.get(key).and_then(|v| v.as_array()))
        .flatten()
        .any(|entity| entity.get("type").and_then(|v| v.as_str()) == Some("bot_command"))
}

async fn acknowledge_telegram_approval_callback(
    client: &reqwest::Client,
    base_url: &str,
    bot_token: &str,
    req: &AgentChannelRequest,
) {
    if req
        .metadata
        .get("from_card_action")
        .and_then(|value| value.as_bool())
        != Some(true)
    {
        return;
    }
    let callback_query_id = req
        .metadata
        .get("telegram_callback_query_id")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if callback_query_id.trim().is_empty() {
        return;
    }
    let action = match req
        .metadata
        .get("approval_action")
        .and_then(|value| value.as_str())
        .unwrap_or("")
    {
        "approve" => ApprovalCardAction::Approve,
        "deny" => ApprovalCardAction::Deny,
        _ => return,
    };
    let status_text = match &action {
        ApprovalCardAction::Approve => "Approved",
        ApprovalCardAction::Deny => "Denied",
    };
    let chat_id = req
        .metadata
        .get("chat_id")
        .and_then(|value| value.as_str())
        .unwrap_or(&req.conversation_id);
    let Some(message_id) = req
        .message_id
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return;
    };
    let tool_name = req
        .metadata
        .get("approval_tool_name")
        .and_then(|value| value.as_str())
        .unwrap_or("tool");
    let decided_by = req
        .metadata
        .get("user_name")
        .and_then(|value| value.as_str())
        .unwrap_or(&req.user_id);
    let resolved_text = TelegramChannel::approval_resolved_text(tool_name, action, decided_by);
    if let Err(err) = post_telegram_bot_api(
        client,
        base_url,
        bot_token,
        "answerCallbackQuery",
        serde_json::json!({
            "callback_query_id": callback_query_id,
            "text": status_text,
            "show_alert": false,
        }),
    )
    .await
    {
        tracing::warn!(error = %err, "telegram approval callback ack failed");
    }
    if let Err(err) = post_telegram_bot_api(
        client,
        base_url,
        bot_token,
        "editMessageText",
        serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": resolved_text,
        }),
    )
    .await
    {
        tracing::warn!(error = %err, "telegram approval message update failed");
    }
}

async fn post_telegram_bot_api(
    client: &reqwest::Client,
    base_url: &str,
    bot_token: &str,
    method: &str,
    body: serde_json::Value,
) -> Result<()> {
    let url = format!("{}/bot{}/{}", base_url, bot_token, method);
    let resp = client
        .post(url)
        .json(&body)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("telegram {method} failed: {err}"),
        })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(rua_core::RuaError::Runtime {
            message: format!("telegram {method} returned {status}: {text}"),
        });
    }
    Ok(())
}

fn telegram_input_text(message: &serde_json::Value, bot_username: Option<&str>) -> (String, bool) {
    let mut parts = Vec::new();
    let mut bot_mentioned = false;
    if let Some(text) = message
        .get("text")
        .or_else(|| message.get("caption"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let (text, mentioned) = strip_telegram_bot_mentions(message, text, bot_username);
        bot_mentioned |= mentioned;
        if !text.is_empty() {
            parts.push(text);
        }
    }

    if let Some(file_id) = message
        .get("photo")
        .and_then(|v| v.as_array())
        .and_then(|photos| photos.last())
        .and_then(|photo| photo.get("file_id"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parts.push(format!("[photo: {file_id}]"));
    }

    for (key, label) in [
        ("document", "document"),
        ("video", "video"),
        ("audio", "audio"),
        ("voice", "voice"),
    ] {
        if let Some(value) = telegram_media_label(message.get(key), label) {
            parts.push(value);
        }
    }

    (parts.join("\n"), bot_mentioned)
}

fn telegram_input_parts(
    message: &serde_json::Value,
    bot_username: Option<&str>,
) -> (Vec<IncomingContentPart>, bool) {
    let media_parts = telegram_local_media_parts(message);
    if media_parts.is_empty() {
        let (text, bot_mentioned) = telegram_input_text(message, bot_username);
        if text.is_empty() {
            (Vec::new(), bot_mentioned)
        } else {
            (vec![IncomingContentPart::Text { text }], bot_mentioned)
        }
    } else {
        let mut parts = Vec::new();
        let (caption, bot_mentioned) = telegram_caption_or_text(message, bot_username);
        if let Some(text) = caption {
            parts.push(IncomingContentPart::Text { text });
        }
        parts.extend(media_parts);
        (parts, bot_mentioned)
    }
}

fn telegram_caption_or_text(
    message: &serde_json::Value,
    bot_username: Option<&str>,
) -> (Option<String>, bool) {
    let Some(text) = message
        .get("text")
        .or_else(|| message.get("caption"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return (None, false);
    };
    let (text, bot_mentioned) = strip_telegram_bot_mentions(message, text, bot_username);
    (Some(text).filter(|s| !s.is_empty()), bot_mentioned)
}

fn strip_telegram_bot_mentions(
    message: &serde_json::Value,
    text: &str,
    bot_username: Option<&str>,
) -> (String, bool) {
    let Some(bot_username) = normalized_telegram_bot_username(bot_username) else {
        return (text.to_string(), false);
    };
    let entity_key = if message
        .get("text")
        .and_then(|value| value.as_str())
        .is_some()
    {
        "entities"
    } else {
        "caption_entities"
    };
    let mut ranges = message
        .get(entity_key)
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter(|entity| entity.get("type").and_then(|value| value.as_str()) == Some("mention"))
        .filter_map(|entity| {
            let offset = entity.get("offset").and_then(|value| value.as_u64())? as usize;
            let length = entity.get("length").and_then(|value| value.as_u64())? as usize;
            let (start, end) = utf16_range_to_byte_range(text, offset, length)?;
            let mention = text.get(start..end)?;
            mention
                .strip_prefix('@')
                .filter(|name| name.eq_ignore_ascii_case(&bot_username))
                .map(|_| (start, end))
        })
        .collect::<Vec<_>>();
    if ranges.is_empty() {
        return (text.to_string(), false);
    }
    ranges.sort_unstable_by_key(|(start, _)| *start);
    let mut stripped = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in ranges {
        if start < cursor {
            continue;
        }
        stripped.push_str(&text[cursor..start]);
        cursor = end;
    }
    stripped.push_str(&text[cursor..]);
    (collapse_removed_mention_spacing(&stripped), true)
}

fn normalized_telegram_bot_username(bot_username: Option<&str>) -> Option<String> {
    bot_username
        .map(str::trim)
        .map(|value| value.trim_start_matches('@'))
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn utf16_range_to_byte_range(text: &str, offset: usize, length: usize) -> Option<(usize, usize)> {
    let end = offset.checked_add(length)?;
    let mut start_byte = None;
    let mut end_byte = None;
    let mut units = 0usize;
    for (idx, ch) in text.char_indices() {
        if units == offset {
            start_byte = Some(idx);
        }
        if units == end {
            end_byte = Some(idx);
            break;
        }
        units += ch.len_utf16();
    }
    if start_byte.is_none() && units == offset {
        start_byte = Some(text.len());
    }
    if end_byte.is_none() && units == end {
        end_byte = Some(text.len());
    }
    Some((start_byte?, end_byte?))
}

fn collapse_removed_mention_spacing(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn telegram_local_media_parts(message: &serde_json::Value) -> Vec<IncomingContentPart> {
    let mut parts = Vec::new();
    if let Some(photo) = message
        .get("photo")
        .and_then(|v| v.as_array())
        .and_then(|photos| photos.last())
    {
        if let Some(part) = telegram_local_media_part(photo, "photo") {
            parts.push(part);
        }
    }
    for (key, kind) in [
        ("document", "document"),
        ("video", "video"),
        ("audio", "audio"),
        ("voice", "voice"),
    ] {
        if let Some(part) = message
            .get(key)
            .and_then(|media| telegram_local_media_part(media, kind))
        {
            parts.push(part);
        }
    }
    parts
}

fn telegram_local_media_part(media: &serde_json::Value, kind: &str) -> Option<IncomingContentPart> {
    let file_id = media
        .get("file_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let path = media
        .get("local_path")
        .or_else(|| media.get("path"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)?;
    let mime = media
        .get("mime")
        .or_else(|| media.get("mime_type"))
        .or_else(|| media.get("content_type"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let name = media
        .get("file_name")
        .or_else(|| media.get("name"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    match kind {
        "photo" => Some(IncomingContentPart::Image {
            file_id,
            path,
            mime,
        }),
        "document" => Some(IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        }),
        "audio" | "voice" => Some(IncomingContentPart::Audio {
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
        _ => None,
    }
}

async fn download_inbound_media_with(
    client: &reqwest::Client,
    base_url: &str,
    bot_token: &str,
    media_dir: &str,
    update: &mut serde_json::Value,
) -> Result<()> {
    let Some(message) = update.get_mut("message") else {
        return Ok(());
    };

    if let Some(photo) = message
        .get_mut("photo")
        .and_then(|v| v.as_array_mut())
        .and_then(|photos| photos.last_mut())
    {
        download_media_node(client, base_url, bot_token, media_dir, photo).await?;
    }

    for key in ["document", "video", "audio", "voice"] {
        if let Some(media) = message.get_mut(key) {
            download_media_node(client, base_url, bot_token, media_dir, media).await?;
        }
    }

    Ok(())
}

async fn download_media_node(
    client: &reqwest::Client,
    base_url: &str,
    bot_token: &str,
    media_dir: &str,
    media: &mut serde_json::Value,
) -> Result<()> {
    if media.get("local_path").and_then(|v| v.as_str()).is_some() {
        return Ok(());
    }
    let Some(file_id) = media
        .get("file_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
    else {
        return Ok(());
    };

    let Some(file_path) = telegram_get_file_path(client, base_url, bot_token, &file_id).await
    else {
        return Ok(());
    };
    let Some((bytes, mime)) =
        telegram_download_file(client, base_url, bot_token, &file_path).await?
    else {
        return Ok(());
    };

    let filename = telegram_download_filename(media, &file_path, &file_id);
    let dir = telegram_media_dir(media_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("create telegram media dir {}: {err}", dir.display()),
        })?;
    let final_path = dir.join(filename);
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

async fn telegram_get_file_path(
    client: &reqwest::Client,
    base_url: &str,
    bot_token: &str,
    file_id: &str,
) -> Option<String> {
    let url = format!(
        "{}/bot{}/getFile",
        base_url.trim_end_matches('/'),
        bot_token
    );
    let resp = match client
        .get(url)
        .query(&[("file_id", file_id)])
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%file_id, error = %err, "telegram getFile failed");
            return None;
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(%file_id, status = %resp.status(), "telegram getFile non-success");
        return None;
    }
    let body = match resp.json::<serde_json::Value>().await {
        Ok(body) => body,
        Err(err) => {
            tracing::warn!(%file_id, error = %err, "telegram getFile JSON parse failed");
            return None;
        }
    };
    if body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        tracing::warn!(%file_id, body = %body, "telegram getFile returned ok=false");
        return None;
    }
    body.pointer("/result/file_path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
}

async fn telegram_download_file(
    client: &reqwest::Client,
    base_url: &str,
    bot_token: &str,
    file_path: &str,
) -> Result<Option<(Vec<u8>, Option<String>)>> {
    let url = format!(
        "{}/file/bot{}/{}",
        base_url.trim_end_matches('/'),
        bot_token,
        file_path.trim_start_matches('/')
    );
    let resp = match client.get(url).timeout(REQUEST_TIMEOUT).send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%file_path, error = %err, "telegram media download failed");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(%file_path, status = %resp.status(), "telegram media download non-success");
        return Ok(None);
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_INBOUND_MEDIA_SIZE {
            tracing::warn!(%file_path, len, "telegram media too large");
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
            message: format!("telegram media download bytes failed: {err}"),
        })?;
    if bytes.len() as u64 > MAX_INBOUND_MEDIA_SIZE {
        tracing::warn!(%file_path, len = bytes.len(), "telegram media too large");
        return Ok(None);
    }
    Ok(Some((bytes.to_vec(), mime)))
}

fn telegram_media_dir(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    if media_dir.is_empty() {
        std::env::temp_dir().join("rua-telegram-media")
    } else {
        PathBuf::from(media_dir)
    }
}

fn telegram_download_filename(media: &serde_json::Value, file_path: &str, file_id: &str) -> String {
    let hint = media
        .get("file_name")
        .or_else(|| media.get("name"))
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(file_path);
    let safe = crate::core::media::sanitize_filename(hint);
    if safe == "file" {
        crate::core::media::sanitize_filename(file_id)
    } else {
        safe
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
            .unwrap_or_else(|| "telegram-media".into()),
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

fn telegram_media_label(value: Option<&serde_json::Value>, label: &str) -> Option<String> {
    let value = value?;
    let name_or_id = ["file_name", "file_id"].into_iter().find_map(|key| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    })?;
    Some(format!("[{label}: {name_or_id}]"))
}

fn telegram_user_name(from: &serde_json::Value) -> String {
    let username = from
        .get("username")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(username) = username {
        return username.to_string();
    }

    let first = from
        .get("first_name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    let last = from
        .get("last_name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    let full = format!("{first} {last}").trim().to_string();
    if full.is_empty() {
        from.get("id")
            .and_then(|v| v.as_i64())
            .map(|id| id.to_string())
            .unwrap_or_default()
    } else {
        full
    }
}

#[async_trait]
impl channel::Channel for TelegramChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("telegram channel already started");
            return Ok(());
        }
        *self.process.write().await = Some(process.clone());
        *started = true;

        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::Markdown,
            self.render_options.clone(),
        ));
        let debouncer = self.debouncer.clone();
        let config = self.config.clone();
        let base_url = self.base_url.clone();
        let client = self.client.clone();
        let acl_metadata = Arc::new(self.acl_metadata.clone());

        let send_fn: pipeline::SendFn = Arc::new({
            let client = client.clone();
            let base_url = base_url.clone();
            let bot_token = config.bot_token.clone();
            move |to, msg| {
                let client = client.clone();
                let base_url = base_url.clone();
                let bot_token = bot_token.clone();
                Box::pin(async move {
                    let chat_id = to
                        .metadata
                        .get("chat_id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| rua_core::RuaError::Validation {
                            message: "missing chat_id in target metadata".into(),
                        })?;
                    let text = Self::render_content(&msg.content);
                    if text.is_empty() {
                        return Ok(());
                    }
                    let html = markdown_to_telegram_html(&text);
                    let chunks = crate::core::content::split_text(&html, 4000);
                    let url = format!("{}/bot{}/sendMessage", base_url, bot_token);
                    for chunk in chunks {
                        let body = serde_json::json!({
                            "chat_id": chat_id,
                            "text": chunk,
                            "parse_mode": "HTML"
                        });
                        let resp = client
                            .post(&url)
                            .json(&body)
                            .timeout(REQUEST_TIMEOUT)
                            .send()
                            .await
                            .map_err(|e| rua_core::RuaError::Runtime {
                                message: format!("telegram sendMessage failed: {e}"),
                            })?;
                        if !resp.status().is_success() {
                            let status = resp.status();
                            let resp_body = resp.text().await.unwrap_or_default();
                            tracing::warn!(%status, body = %resp_body, "telegram sendMessage non-success");
                        }
                    }
                    Ok(())
                })
            }
        });

        let mut stop_rx = self.poll_stop_tx.subscribe();
        let allowed_users = config.allowed_users.clone();
        let bot_token = config.bot_token.clone();
        let bot_username = config.bot_username.clone();
        let approval_resolver = self.approval_resolver.clone();
        let event_deduper = self.event_deduper.clone();

        let handle = tokio::spawn(async move {
            let mut offset: i64 = 0;
            let poll_url = format!("{}/bot{}/getUpdates", base_url, bot_token);

            loop {
                if *stop_rx.borrow() {
                    break;
                }

                let resp = tokio::select! {
                    _ = stop_rx.changed() => break,
                    result = client.get(&poll_url)
                        .query(&[
                            ("offset", offset.to_string()),
                            ("timeout", POLL_TIMEOUT.to_string()),
                            (
                                "allowed_updates",
                                "[\"message\",\"callback_query\"]".to_string(),
                            ),
                        ])
                        .timeout(POLL_READ_TIMEOUT)
                        .send() => result,
                };

                let resp = match resp {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(error = %e, "telegram getUpdates request failed, retrying");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };

                let body: serde_json::Value = match resp.json().await {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(error = %e, "telegram getUpdates parse failed, retrying");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };

                let ok = body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                if !ok {
                    tracing::warn!(body = %body, "telegram getUpdates returned ok=false");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }

                let updates = match body.get("result").and_then(|v| v.as_array()) {
                    Some(arr) => arr,
                    None => continue,
                };

                for update in updates {
                    let update_id = update
                        .get("update_id")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    if update_id >= offset {
                        offset = update_id + 1;
                    }

                    let mut update = update.clone();
                    match telegram_check_duplicate_event(&event_deduper, &update) {
                        Ok(true) => {
                            tracing::debug!(update_id, "telegram duplicate update skipped");
                            continue;
                        }
                        Ok(false) => {}
                        Err(err) => {
                            tracing::warn!(error = %err, "telegram event dedup failed");
                        }
                    }
                    if let Err(err) = download_inbound_media_with(
                        &client,
                        &base_url,
                        &bot_token,
                        &config.media_dir,
                        &mut update,
                    )
                    .await
                    {
                        tracing::warn!(error = %err, "telegram inbound media download failed; using fallback placeholders");
                    }

                    let mut req = match Self::parse_update_with_resolver_and_bot_username(
                        &update,
                        &allowed_users,
                        &approval_resolver,
                        Some(&bot_username),
                    ) {
                        Ok(Some(req)) => req,
                        Ok(None) => continue,
                        Err(err) => {
                            tracing::warn!(error = %err, "telegram approval callback rejected");
                            continue;
                        }
                    };
                    acknowledge_telegram_approval_callback(&client, &base_url, &bot_token, &req)
                        .await;
                    {
                        if let (Some(acl_obj), Some(meta)) =
                            (acl_metadata.as_object(), req.metadata.as_object_mut())
                        {
                            for (k, v) in acl_obj {
                                meta.insert(k.clone(), v.clone());
                            }
                        }
                        let p = process.clone();
                        let d = debouncer.clone();
                        let r = renderer.clone();
                        let sf = send_fn.clone();
                        let stream_handler = Arc::new(TelegramStreamHandler::new(
                            client.clone(),
                            base_url.clone(),
                            bot_token.clone(),
                            req.metadata
                                .get("chat_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or(&config.chat_id)
                                .to_string(),
                            req.metadata.get("message_thread_id").cloned(),
                            PlatformStreamPolicy::placeholder_edit(
                                Duration::from_secs(1),
                                SEND_MESSAGE_CHUNK_BYTES,
                            ),
                        ));
                        let typing_indicator = Arc::new(TelegramTypingIndicator::new(
                            client.clone(),
                            base_url.clone(),
                            bot_token.clone(),
                            req.metadata
                                .get("chat_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or(&config.chat_id)
                                .to_string(),
                            req.metadata.get("message_thread_id").cloned(),
                        ));
                        let sender_key = req.user_id.clone();
                        tokio::spawn(async move {
                            let result = pipeline::run_inbound_streaming_with_indicator(
                                p,
                                req,
                                Some((d, sender_key)),
                                r,
                                sf,
                                Some(stream_handler),
                                Some(typing_indicator),
                            )
                            .await;
                            if let Err(e) = result {
                                tracing::error!("telegram pipeline error: {e}");
                            }
                        });
                    }
                }
            }

            tracing::info!("telegram polling task stopped");
        });

        *self.poll_handle.write().await = Some(handle);
        tracing::info!("telegram channel started (inbound polling + outbound HTML)");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut started = self.started.write().await;
        if !*started {
            return Ok(());
        }

        let _ = self.poll_stop_tx.send(true);
        if let Some(handle) = self.poll_handle.write().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }

        if let Ok(mut deduper) = self.event_deduper.lock() {
            deduper.clear();
        }
        *self.process.write().await = None;
        *started = false;
        tracing::info!("telegram channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        let chat_id = to
            .metadata
            .get("chat_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.config.chat_id);

        let approval_request_id = msg
            .metadata
            .get("approval_request_id")
            .and_then(|v| v.as_str())
            .filter(|_| {
                msg.metadata.get("message_type").and_then(|v| v.as_str())
                    == Some("tool_guard_approval")
            });
        let message_thread_id = to.metadata.get("message_thread_id").cloned();
        let text = Self::render_text_and_fallback_parts(&msg.content);
        let html = markdown_to_telegram_html(&text);
        if !html.is_empty() {
            for chunk in crate::core::content::split_text(&html, SEND_MESSAGE_CHUNK_BYTES) {
                let mut body = serde_json::json!({
                    "chat_id": chat_id,
                    "text": chunk,
                    "parse_mode": "HTML"
                });
                if let Some(thread_id) = &message_thread_id {
                    body["message_thread_id"] = thread_id.clone();
                }
                if let Some(request_id) = approval_request_id {
                    body["reply_markup"] = serde_json::json!({
                        "inline_keyboard": [[
                            {
                                "text": "批准",
                                "callback_data": build_callback_data(ApprovalCardAction::Approve, request_id)
                            },
                            {
                                "text": "拒绝",
                                "callback_data": build_callback_data(ApprovalCardAction::Deny, request_id)
                            }
                        ]]
                    });
                }
                self.post_api("sendMessage", body).await?;
            }
        }
        for part in &msg.content {
            self.send_media_part(part, chat_id, message_thread_id.as_ref())
                .await?;
        }
        Ok(())
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        if telegram_check_duplicate_event(&self.event_deduper, &native)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate telegram event".into(),
            });
        }
        Self::parse_update_with_resolver_and_bot_username(
            &native,
            &self.config.allowed_users,
            &self.approval_resolver,
            Some(&self.config.bot_username),
        )?
        .ok_or_else(|| rua_core::RuaError::Runtime {
            message: "failed to parse telegram update: missing or non-text message".into(),
        })
    }
}
