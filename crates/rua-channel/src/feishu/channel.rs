use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::core::approval_card::{
    ApprovalCardCallback, ApprovalCardResolveError, ApprovalCardResolver,
};
use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::content::{render_parts_to_string, IncomingContentPart, OutgoingContentPart};
use crate::core::event_dedup::{EventDedupDecision, EventDeduper};
use crate::core::pipeline;
use crate::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::streaming::{
    PlatformStreamAction, PlatformStreamPolicy, PlatformStreamState, StreamHandler, StreamType,
};
use crate::core::{BufferDebouncer, ChannelId};
use crate::http_client::default_http_client;
use rua_core::Result;

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(500);
const STREAM_MIN_INTERVAL: Duration = Duration::from_millis(500);
const STREAM_MAX_CHARS: usize = 3000;
const FEISHU_STREAM_ELEMENT_ID: &str = "rua_stream_text";

use crate::feishu::api::{self, FeishuTokenManager};
use crate::feishu::approval_card;
use crate::feishu::types::FeishuConfig;
use crate::feishu::ws;

const MAX_RETRIES: u32 = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INBOUND_MEDIA_SIZE: u64 = 50 * 1024 * 1024;
const APPROVAL_CALLBACK_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

#[derive(Debug)]
struct FeishuStreamInner {
    state: PlatformStreamState,
    card_id: Option<String>,
    sequence: i32,
}

pub struct FeishuStreamHandler {
    client: reqwest::Client,
    open_api_base_url: String,
    token_manager: Arc<FeishuTokenManager>,
    receive_id_type: String,
    receive_id: String,
    policy: PlatformStreamPolicy,
    started_at: tokio::time::Instant,
    inner: Mutex<FeishuStreamInner>,
}

impl FeishuStreamHandler {
    pub fn new(
        client: reqwest::Client,
        open_api_base_url: String,
        token_manager: Arc<FeishuTokenManager>,
        receive_id_type: String,
        receive_id: String,
        policy: PlatformStreamPolicy,
    ) -> Self {
        Self {
            client,
            open_api_base_url: open_api_base_url.trim_end_matches('/').to_string(),
            token_manager,
            receive_id_type,
            receive_id,
            policy,
            started_at: tokio::time::Instant::now(),
            inner: Mutex::new(FeishuStreamInner {
                state: PlatformStreamState::default(),
                card_id: None,
                sequence: 0,
            }),
        }
    }

    fn build_streaming_card(content: &str) -> serde_json::Value {
        serde_json::json!({
            "schema": "2.0",
            "config": { "streaming_mode": true },
            "body": {
                "elements": [{
                    "tag": "markdown",
                    "content": content,
                    "element_id": FEISHU_STREAM_ELEMENT_ID
                }]
            }
        })
    }

    async fn token(&self) -> Result<String> {
        self.token_manager.get_token().await
    }

    async fn create_card(&self, content: &str) -> Result<String> {
        let token = self.token().await?;
        let card_id = api::create_card_entity_with_base_url(
            &self.client,
            &self.open_api_base_url,
            &token,
            Self::build_streaming_card(content),
        )
        .await?;
        api::send_im_v1_with_base_url(
            &self.client,
            &self.open_api_base_url,
            &token,
            &self.receive_id_type,
            &self.receive_id,
            "interactive",
            serde_json::json!({
                "type": "card",
                "data": { "card_id": card_id }
            })
            .to_string(),
        )
        .await?;
        Ok(card_id)
    }

    async fn update_card(&self, card_id: &str, content: &str, sequence: i32) -> Result<()> {
        let token = self.token().await?;
        api::stream_card_element_content_with_base_url(
            &self.client,
            &self.open_api_base_url,
            &token,
            card_id,
            FEISHU_STREAM_ELEMENT_ID,
            content,
            sequence,
        )
        .await
    }

    async fn finalize_card(&self, card_id: &str, content: &str, sequence: i32) -> Result<()> {
        let preview = if content.trim().is_empty() {
            "OK".to_string()
        } else {
            content.chars().take(80).collect()
        };
        let token = self.token().await?;
        api::update_card_settings_with_base_url(
            &self.client,
            &self.open_api_base_url,
            &token,
            card_id,
            serde_json::json!({
                "config": {
                    "streaming_mode": false,
                    "summary": { "content": preview }
                }
            }),
            sequence,
        )
        .await
    }

    async fn send_final_text(&self, content: &str) -> Result<()> {
        let token = self.token().await?;
        api::send_im_v1_with_base_url(
            &self.client,
            &self.open_api_base_url,
            &token,
            &self.receive_id_type,
            &self.receive_id,
            "text",
            serde_json::json!({ "text": content }).to_string(),
        )
        .await
    }

    async fn apply_action(&self, action: PlatformStreamAction) -> Result<()> {
        match action {
            PlatformStreamAction::Create { content, .. } => {
                let card_id = self.create_card(&content).await?;
                self.inner.lock().await.card_id = Some(card_id);
            }
            PlatformStreamAction::Update {
                content,
                final_update,
                ..
            } => {
                let mut inner = self.inner.lock().await;
                let Some(card_id) = inner.card_id.clone() else {
                    drop(inner);
                    if final_update {
                        self.send_final_text(&content).await?;
                    }
                    return Ok(());
                };
                inner.sequence += 1;
                let update_sequence = inner.sequence;
                if final_update {
                    inner.sequence += 1;
                }
                let finalize_sequence = inner.sequence;
                drop(inner);

                self.update_card(&card_id, &content, update_sequence)
                    .await?;
                if final_update {
                    self.finalize_card(&card_id, &content, finalize_sequence)
                        .await?;
                }
            }
            PlatformStreamAction::SendFinal { content } => {
                self.send_final_text(&content).await?;
            }
            PlatformStreamAction::Skip => {}
        }
        Ok(())
    }
}

#[async_trait]
impl StreamHandler for FeishuStreamHandler {
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

pub struct FeishuChannel {
    channel_id: ChannelId,
    config: FeishuConfig,
    client: reqwest::Client,
    token_manager: Option<Arc<FeishuTokenManager>>,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    ws_stop_tx: watch::Sender<bool>,
    ws_handle: RwLock<Option<JoinHandle<()>>>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    debouncer: BufferDebouncer<String, AgentRequest>,
    open_api_base_url: String,
    approval_resolver: Arc<StdMutex<ApprovalCardResolver>>,
    event_deduper: Arc<StdMutex<EventDeduper>>,
}

impl FeishuChannel {
    pub fn new(config: FeishuConfig) -> Self {
        let (ws_stop_tx, _) = watch::channel(false);
        let client = default_http_client();
        let token_manager = match (config.app_id.as_ref(), config.app_secret.as_ref()) {
            (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => Some(
                FeishuTokenManager::new(id.clone(), secret.clone(), client.clone()),
            ),
            _ => None,
        };
        Self {
            channel_id: "feishu".into(),
            config,
            client,
            token_manager,
            process: RwLock::new(None),
            started: RwLock::new(false),
            ws_stop_tx,
            ws_handle: RwLock::new(None),
            render_options: MessageRenderOptions::default(),
            acl_metadata: serde_json::json!({}),
            debouncer: BufferDebouncer::new(DEBOUNCE_WINDOW),
            open_api_base_url: "https://open.feishu.cn".into(),
            approval_resolver: Arc::new(StdMutex::new(ApprovalCardResolver::new(
                APPROVAL_CALLBACK_TTL,
            ))),
            event_deduper: Arc::new(StdMutex::new(EventDeduper::new(
                EVENT_DEDUP_TTL,
                EVENT_DEDUP_CAPACITY,
            ))),
        }
    }

    pub fn with_domain(mut self, domain: &str) -> Result<Self> {
        let base = match domain {
            "" | "feishu" => "https://open.feishu.cn",
            "lark" => "https://open.larksuite.com",
            value if value.starts_with("http://") || value.starts_with("https://") => value,
            _ => {
                return Err(rua_core::RuaError::Validation {
                    message: "Feishu domain must be feishu, lark, or an HTTP(S) URL".into(),
                })
            }
        };
        self.open_api_base_url = base.trim_end_matches('/').to_string();
        if let (Some(app_id), Some(app_secret)) = (&self.config.app_id, &self.config.app_secret) {
            self.token_manager = Some(FeishuTokenManager::new_with_token_url(
                app_id.clone(),
                app_secret.clone(),
                self.client.clone(),
                format!(
                    "{}/open-apis/auth/v3/tenant_access_token/internal",
                    self.open_api_base_url
                ),
            ));
        }
        Ok(self)
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

    pub fn debouncer(&self) -> &BufferDebouncer<String, AgentRequest> {
        &self.debouncer
    }

    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    pub fn with_token_manager(mut self, tm: Arc<FeishuTokenManager>) -> Self {
        self.token_manager = Some(tm);
        self
    }

    pub fn with_open_api_base_url(mut self, base_url: String) -> Self {
        self.open_api_base_url = base_url.trim_end_matches('/').to_string();
        self
    }

    pub async fn download_inbound_media_with_token(
        &self,
        native: &mut serde_json::Value,
        token: &str,
    ) -> Result<()> {
        download_feishu_inbound_media_with(
            &self.client,
            &self.open_api_base_url,
            token,
            &self.config.media_dir,
            native,
        )
        .await
    }

    fn compute_sign(secret: &str, timestamp: i64) -> String {
        let string_to_sign = format!("{}\n{}", timestamp, secret);
        let mut mac = Hmac::<Sha256>::new_from_slice(string_to_sign.as_bytes())
            .expect("HMAC accepts any key size");
        mac.update(&[]);
        let result = mac.finalize().into_bytes();
        base64::engine::general_purpose::STANDARD.encode(result)
    }

    pub fn build_text_body(text: &str, secret: Option<&str>) -> serde_json::Value {
        let mut body = serde_json::json!({
            "msg_type": "text",
            "content": {
                "text": text
            }
        });
        if let Some(secret) = secret {
            let timestamp = chrono::Utc::now().timestamp();
            let sign = Self::compute_sign(secret, timestamp);
            body["timestamp"] = serde_json::json!(timestamp.to_string());
            body["sign"] = serde_json::json!(sign);
        }
        body
    }

    fn approval_card_content(msg: &OutMessage, text: &str) -> Option<String> {
        let request_id = msg
            .metadata
            .get("approval_request_id")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())?;
        if msg
            .metadata
            .get("message_type")
            .and_then(|value| value.as_str())
            != Some("tool_guard_approval")
        {
            return None;
        }
        let tool_name = msg
            .metadata
            .get("tool_name")
            .and_then(|value| value.as_str())
            .unwrap_or("tool");
        let severity = msg
            .metadata
            .get("severity")
            .and_then(|value| value.as_str())
            .unwrap_or("medium");
        Some(approval_card::build_approval_card(
            request_id,
            tool_name,
            severity,
            text,
            serde_json::json!({}),
        ))
    }

    async fn post_webhook(&self, body: serde_json::Value) -> Result<()> {
        if self.config.webhook_url.is_empty() {
            tracing::warn!("feishu webhook_url is empty, skipping send");
            return Ok(());
        }

        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_secs(1 << (attempt - 1));
                tracing::debug!(
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying feishu webhook POST"
                );
                tokio::time::sleep(delay).await;
            }

            match self
                .client
                .post(&self.config.webhook_url)
                .json(&body)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(attempt, "feishu webhook POST succeeded");
                    return Ok(());
                }
                Ok(resp) => {
                    let status = resp.status();
                    let resp_body = resp.text().await.unwrap_or_default();
                    tracing::warn!(attempt, %status, body = %resp_body, "feishu webhook POST non-success status");
                    last_error = Some(format!("HTTP {status}: {resp_body}"));
                }
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "feishu webhook POST failed");
                    last_error = Some(e.to_string());
                }
            }
        }

        Err(rua_core::RuaError::Runtime {
            message: format!(
                "feishu webhook POST failed after {MAX_RETRIES} retries: {}",
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

    async fn send_media_part(
        &self,
        token: &str,
        receive_id_type: &str,
        receive_id: &str,
        part: &OutgoingContentPart,
    ) -> Result<bool> {
        match part {
            OutgoingContentPart::Image {
                path: Some(path), ..
            } => {
                let image_key = match self.upload_feishu_image(token, path).await {
                    Ok(key) => key,
                    Err(err) => {
                        tracing::warn!(error = %err, "feishu image upload failed");
                        return Ok(false);
                    }
                };
                api::send_im_v1_with_base_url(
                    &self.client,
                    &self.open_api_base_url,
                    token,
                    receive_id_type,
                    receive_id,
                    "image",
                    serde_json::json!({"image_key": image_key}).to_string(),
                )
                .await?;
                Ok(true)
            }
            OutgoingContentPart::File {
                path: Some(path),
                name,
                ..
            }
            | OutgoingContentPart::Audio {
                path: Some(path),
                name,
                ..
            }
            | OutgoingContentPart::Video {
                path: Some(path),
                name,
                ..
            } => {
                let file_key = match self.upload_feishu_file(token, path, name.as_deref()).await {
                    Ok(key) => key,
                    Err(err) => {
                        tracing::warn!(error = %err, "feishu file upload failed");
                        return Ok(false);
                    }
                };
                api::send_im_v1_with_base_url(
                    &self.client,
                    &self.open_api_base_url,
                    token,
                    receive_id_type,
                    receive_id,
                    "file",
                    serde_json::json!({"file_key": file_key}).to_string(),
                )
                .await?;
                Ok(true)
            }
            OutgoingContentPart::Image { .. }
            | OutgoingContentPart::File { .. }
            | OutgoingContentPart::Audio { .. }
            | OutgoingContentPart::Video { .. } => Ok(true),
            _ => Ok(false),
        }
    }

    async fn upload_feishu_image(&self, token: &str, path: &std::path::Path) -> Result<String> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("failed to read image {}: {err}", path.display()),
            })?;
        let url = format!(
            "{}/open-apis/im/v1/images",
            self.open_api_base_url.trim_end_matches('/')
        );
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("image.png")
            .to_string();
        let form = reqwest::multipart::Form::new()
            .text("image_type", "message")
            .part(
                "image",
                reqwest::multipart::Part::bytes(bytes).file_name(filename),
            );
        let body = self.post_feishu_multipart(token, &url, form).await?;
        body.pointer("/data/image_key")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| rua_core::RuaError::Runtime {
                message: format!("feishu image upload missing image_key: {body}"),
            })
    }

    async fn upload_feishu_file(
        &self,
        token: &str,
        path: &std::path::Path,
        name: Option<&str>,
    ) -> Result<String> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("failed to read file {}: {err}", path.display()),
            })?;
        let url = format!(
            "{}/open-apis/im/v1/files",
            self.open_api_base_url.trim_end_matches('/')
        );
        let filename = name
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| "file.bin".to_string());
        let form = reqwest::multipart::Form::new()
            .text("file_type", feishu_file_type(&filename))
            .text("file_name", filename.clone())
            .part(
                "file",
                reqwest::multipart::Part::bytes(bytes).file_name(filename),
            );
        let body = self.post_feishu_multipart(token, &url, form).await?;
        body.pointer("/data/file_key")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| rua_core::RuaError::Runtime {
                message: format!("feishu file upload missing file_key: {body}"),
            })
    }

    async fn post_feishu_multipart(
        &self,
        token: &str,
        url: &str,
        form: reqwest::multipart::Form,
    ) -> Result<serde_json::Value> {
        let resp = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {token}"))
            .multipart(form)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("feishu multipart request failed: {err}"),
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(rua_core::RuaError::Runtime {
                message: format!("feishu multipart HTTP {status}: {text}"),
            });
        }
        let body: serde_json::Value =
            resp.json()
                .await
                .map_err(|err| rua_core::RuaError::Runtime {
                    message: format!("feishu multipart response parse failed: {err}"),
                })?;
        if body
            .get("code")
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
            != 0
        {
            return Err(rua_core::RuaError::Runtime {
                message: format!("feishu multipart error: {body}"),
            });
        }
        Ok(body)
    }
}

fn feishu_file_type(filename: &str) -> String {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "pdf" | "doc" | "xls" | "ppt" => ext,
        "docx" => "doc".into(),
        "xlsx" => "xls".into(),
        "pptx" => "ppt".into(),
        "ogg" | "opus" => "opus".into(),
        _ => "stream".into(),
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

#[async_trait]
impl channel::Channel for FeishuChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("feishu channel already started");
            return Ok(());
        }
        self.ws_stop_tx.send_replace(false);
        *self.process.write().await = Some(process.clone());
        *started = true;

        if let (Some(app_id), Some(app_secret)) =
            (self.config.app_id.as_ref(), self.config.app_secret.as_ref())
        {
            if !app_id.is_empty() && !app_secret.is_empty() {
                let ws_config = ws::WsConfig {
                    app_id: app_id.clone(),
                    app_secret: app_secret.clone(),
                };
                let process_for_ws = process;
                let debouncer = self.debouncer.clone();
                let renderer = Arc::new(MessageRenderer::with_options(
                    RenderStyle::Markdown,
                    self.render_options.clone(),
                ));
                let acl_metadata = Arc::new(self.acl_metadata.clone());
                let share_session_in_group = self.config.share_session_in_group;
                let inbound_client = self.client.clone();
                let inbound_open_api_base_url = self.open_api_base_url.clone();
                let inbound_media_dir = self.config.media_dir.clone();
                let inbound_token_manager = self.token_manager.clone();
                let approval_resolver = self.approval_resolver.clone();
                let event_deduper = self.event_deduper.clone();
                let bot_open_id = self.config.bot_open_id.clone();
                let send_fn: pipeline::SendFn = Arc::new({
                    let client = self.client.clone();
                    let webhook_url = self.config.webhook_url.clone();
                    let secret = self.config.secret.clone();
                    let tm = self.token_manager.clone();
                    let open_api_base_url = self.open_api_base_url.clone();
                    move |to, msg| {
                        let client = client.clone();
                        let webhook_url = webhook_url.clone();
                        let secret = secret.clone();
                        let tm = tm.clone();
                        let open_api_base_url = open_api_base_url.clone();
                        Box::pin(async move {
                            let text = render_parts_to_string(&msg.content);
                            if text.is_empty() {
                                return Ok(());
                            }

                            if let Some(ref tm) = tm {
                                if let Some((rid_type, rid)) = api::resolve_receive_id(&to.metadata)
                                {
                                    match tm.get_token().await {
                                        Ok(token) => {
                                            if let Some(card) =
                                                FeishuChannel::approval_card_content(&msg, &text)
                                            {
                                                return api::send_im_v1_with_base_url(
                                                    &client,
                                                    &open_api_base_url,
                                                    &token,
                                                    rid_type,
                                                    &rid,
                                                    "interactive",
                                                    card,
                                                )
                                                .await;
                                            }
                                            return api::send_im_v1(
                                                &client, &token, rid_type, &rid, &text,
                                            )
                                            .await;
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                "feishu token fetch failed, falling back to webhook: {e}"
                                            );
                                        }
                                    }
                                }
                            }

                            let body = FeishuChannel::build_text_body(&text, secret.as_deref());
                            if webhook_url.is_empty() {
                                tracing::warn!("feishu webhook_url is empty, skipping send");
                                return Ok(());
                            }
                            let mut last_error = None;
                            for attempt in 0..=MAX_RETRIES {
                                if attempt > 0 {
                                    let delay = Duration::from_secs(1 << (attempt - 1));
                                    tokio::time::sleep(delay).await;
                                }
                                match client
                                    .post(&webhook_url)
                                    .json(&body)
                                    .timeout(REQUEST_TIMEOUT)
                                    .send()
                                    .await
                                {
                                    Ok(resp) if resp.status().is_success() => {
                                        return Ok(());
                                    }
                                    Ok(resp) => {
                                        let status = resp.status();
                                        let resp_body = resp.text().await.unwrap_or_default();
                                        last_error = Some(format!("HTTP {status}: {resp_body}"));
                                    }
                                    Err(e) => {
                                        last_error = Some(e.to_string());
                                    }
                                }
                            }
                            Err(rua_core::RuaError::Runtime {
                                message: format!(
                                    "feishu webhook POST failed after {MAX_RETRIES} retries: {}",
                                    last_error.unwrap_or_default()
                                ),
                            })
                        })
                    }
                });
                let on_message: ws::OnMessageFn = Arc::new(move |native| {
                    let p = process_for_ws.clone();
                    let d = debouncer.clone();
                    let r = renderer.clone();
                    let sf = send_fn.clone();
                    let acl = acl_metadata.clone();
                    let approvals = approval_resolver.clone();
                    let client = inbound_client.clone();
                    let open_api_base_url = inbound_open_api_base_url.clone();
                    let media_dir = inbound_media_dir.clone();
                    let tm = inbound_token_manager.clone();
                    let bot_open_id = bot_open_id.clone();
                    let event_deduper = event_deduper.clone();
                    tokio::spawn(async move {
                        let mut native = native;
                        match feishu_check_duplicate_event(&event_deduper, &native) {
                            Ok(true) => {
                                tracing::debug!("feishu duplicate event skipped");
                                return;
                            }
                            Ok(false) => {}
                            Err(err) => {
                                tracing::warn!(error = %err, "feishu event dedup failed");
                            }
                        }
                        if let Some(tm_ref) = tm.as_ref() {
                            match tm_ref.get_token().await {
                                Ok(token) => {
                                    if let Err(err) = download_feishu_inbound_media_with(
                                        &client,
                                        &open_api_base_url,
                                        &token,
                                        &media_dir,
                                        &mut native,
                                    )
                                    .await
                                    {
                                        tracing::warn!(error = %err, "feishu inbound media download failed; using fallback placeholders");
                                    }
                                }
                                Err(err) => {
                                    tracing::warn!(error = %err, "feishu token fetch failed before inbound media download; using fallback placeholders");
                                }
                            }
                        }
                        match parse_feishu_event_with_resolver(
                            native,
                            share_session_in_group,
                            bot_open_id.as_deref(),
                            &approvals,
                        ) {
                            Ok(Some(req)) => {
                                let mut req = req;
                                if let (Some(acl_obj), Some(meta)) =
                                    (acl.as_object(), req.metadata.as_object_mut())
                                {
                                    for (k, v) in acl_obj {
                                        meta.insert(k.clone(), v.clone());
                                    }
                                }
                                let sender_key = req.user_id.clone();
                                let stream_handler = tm.as_ref().and_then(|tm| {
                                    api::resolve_receive_id(&req.metadata).map(|(rid_type, rid)| {
                                        Arc::new(FeishuStreamHandler::new(
                                            client.clone(),
                                            open_api_base_url.clone(),
                                            tm.clone(),
                                            rid_type.to_string(),
                                            rid,
                                            PlatformStreamPolicy::card_update(
                                                STREAM_MIN_INTERVAL,
                                                STREAM_MAX_CHARS,
                                            ),
                                        ))
                                            as Arc<dyn StreamHandler>
                                    })
                                });
                                if let Err(e) = pipeline::run_inbound_streaming(
                                    p,
                                    req,
                                    Some((d, sender_key)),
                                    r,
                                    sf,
                                    stream_handler,
                                )
                                .await
                                {
                                    tracing::error!("feishu pipeline error: {e}");
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                tracing::error!("feishu WS: parse failed: {e}");
                            }
                        }
                    });
                });
                let on_callback: ws::OnCallbackFn =
                    Arc::new(move |native| build_feishu_card_action_response(&native));
                let stop_rx = self.ws_stop_tx.subscribe();
                let handle = ws::spawn_ws_task_with_base_url(
                    ws_config,
                    self.open_api_base_url.clone(),
                    on_message,
                    Some(on_callback),
                    stop_rx,
                );
                *self.ws_handle.write().await = Some(handle);
                tracing::info!("feishu channel started with WS inbound");
            } else {
                tracing::info!("feishu channel started (outbound only, empty app credentials)");
            }
        } else {
            tracing::info!("feishu channel started (outbound only, no app credentials)");
        }

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut started = self.started.write().await;
        if !*started {
            return Ok(());
        }

        let _ = self.ws_stop_tx.send(true);
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
        *self.process.write().await = None;
        *started = false;
        tracing::info!("feishu channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        if let Some(ref tm) = self.token_manager {
            if let Some((rid_type, rid)) = api::resolve_receive_id(&to.metadata) {
                match tm.get_token().await {
                    Ok(token) => {
                        let text = Self::render_text_and_unresolved_media(&msg.content);
                        if !text.is_empty() {
                            if let Some(card) = Self::approval_card_content(&msg, &text) {
                                api::send_im_v1_with_base_url(
                                    &self.client,
                                    &self.open_api_base_url,
                                    &token,
                                    rid_type,
                                    &rid,
                                    "interactive",
                                    card,
                                )
                                .await?;
                            } else {
                                api::send_im_v1_with_base_url(
                                    &self.client,
                                    &self.open_api_base_url,
                                    &token,
                                    rid_type,
                                    &rid,
                                    "text",
                                    serde_json::json!({"text": text}).to_string(),
                                )
                                .await?;
                            }
                        }
                        for part in &msg.content {
                            if !is_media_part(part) {
                                continue;
                            }
                            let sent = self.send_media_part(&token, rid_type, &rid, part).await?;
                            if !sent {
                                let fallback = Self::render_content(std::slice::from_ref(part));
                                if !fallback.is_empty() {
                                    api::send_im_v1_with_base_url(
                                        &self.client,
                                        &self.open_api_base_url,
                                        &token,
                                        rid_type,
                                        &rid,
                                        "text",
                                        serde_json::json!({"text": fallback}).to_string(),
                                    )
                                    .await?;
                                }
                            }
                        }
                        return Ok(());
                    }
                    Err(e) => {
                        tracing::warn!("feishu token fetch failed, falling back to webhook: {e}");
                    }
                }
            }
        }

        let text = Self::render_content(&msg.content);
        if text.is_empty() {
            tracing::debug!("empty content, skipping feishu send");
            return Ok(());
        }
        let body = Self::build_text_body(&text, self.config.secret.as_deref());
        self.post_webhook(body).await
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        if feishu_check_duplicate_event(&self.event_deduper, &native)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate feishu event".into(),
            });
        }
        parse_feishu_event_with_resolver(
            native,
            self.config.share_session_in_group,
            self.config.bot_open_id.as_deref(),
            &self.approval_resolver,
        )?
        .ok_or_else(|| rua_core::RuaError::Validation {
            message: "feishu event produced no request (bot message or unsupported type)".into(),
        })
    }
}

fn parse_feishu_event_with_options(
    native: serde_json::Value,
    share_session_in_group: bool,
    bot_open_id: Option<&str>,
) -> Result<Option<AgentRequest>> {
    let event = native
        .get("event")
        .ok_or_else(|| rua_core::RuaError::Validation {
            message: "feishu event missing 'event' field".into(),
        })?;

    let sender_type = event
        .pointer("/sender/sender_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if sender_type == "bot" {
        tracing::debug!("feishu: skipping bot message");
        return Ok(None);
    }

    if let Some(req) = parse_feishu_approval_action_event(event) {
        return Ok(Some(req));
    }

    let open_id = event
        .pointer("/sender/sender_id/open_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let sender_name = event
        .pointer("/sender/sender_name")
        .or_else(|| event.pointer("/sender/name"))
        .or_else(|| event.pointer("/sender/sender_id/user_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let message = event
        .get("message")
        .ok_or_else(|| rua_core::RuaError::Validation {
            message: "feishu event missing 'event.message' field".into(),
        })?;

    let message_id = message
        .get("message_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let chat_id = message
        .get("chat_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let chat_type = message
        .get("chat_type")
        .and_then(|v| v.as_str())
        .unwrap_or("p2p")
        .to_string();
    let msg_type = message
        .get("message_type")
        .and_then(|v| v.as_str())
        .unwrap_or("text")
        .to_string();
    let thread_id = message
        .get("thread_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let content_raw = message
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let mention = feishu_bot_mention(message, content_raw, bot_open_id);
    let input = extract_message_parts_with_mention(&msg_type, content_raw, &mention);
    if input.is_empty() {
        return Ok(None);
    }

    let conversation_id = if !chat_id.is_empty() {
        chat_id.clone()
    } else {
        open_id.clone()
    };

    let thread_user_id = feishu_thread_user_id(&thread_id);
    let user_id = if let Some(thread_user_id) = &thread_user_id {
        thread_user_id.clone()
    } else if share_session_in_group && chat_type == "group" {
        "group".to_string()
    } else {
        open_id.clone()
    };
    let session_id = thread_user_id
        .clone()
        .unwrap_or_else(|| conversation_id.clone());
    let user_name = if sender_name.is_empty() {
        open_id.clone()
    } else {
        sender_name.clone()
    };
    let is_group = chat_type == "group";
    let receive_id_type = if is_group && !chat_id.is_empty() {
        "chat_id"
    } else {
        "open_id"
    };

    let mut metadata = serde_json::json!({
        "sender_id": open_id,
        "is_group": is_group,
        "chat_type": chat_type,
        "feishu_chat_id": chat_id,
        "feishu_chat_type": chat_type,
        "feishu_sender_id": open_id,
        "feishu_sender_name": sender_name,
        "feishu_message_id": message_id,
        "feishu_receive_id": conversation_id,
        "feishu_receive_id_type": receive_id_type,
        "user_name": user_name,
        "msg_type": msg_type,
    });
    if !thread_id.is_empty() {
        metadata["feishu_thread_id"] = serde_json::json!(thread_id);
    }
    if mention.is_bot_mentioned {
        metadata["bot_mentioned"] = serde_json::json!(true);
        metadata["mentioned"] = serde_json::json!(true);
    }

    Ok(Some(AgentRequest {
        agent_id: String::new(),
        user_id: user_id.clone(),
        session_id,
        input,
        channel_id: "feishu".into(),
        conversation_id,
        message_id: if message_id.is_empty() {
            None
        } else {
            Some(message_id)
        },
        metadata,
    }))
}

fn feishu_check_duplicate_event(
    deduper: &Arc<StdMutex<EventDeduper>>,
    native: &serde_json::Value,
) -> Result<bool> {
    let Some(key) = feishu_event_key(native) else {
        return Ok(false);
    };
    let mut deduper = deduper.lock().map_err(|_| rua_core::RuaError::Runtime {
        message: "feishu event deduper lock poisoned".into(),
    })?;
    Ok(deduper.check_and_mark(&key) == EventDedupDecision::Duplicate)
}

fn feishu_event_key(native: &serde_json::Value) -> Option<String> {
    let event_type = native
        .pointer("/header/event_type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if event_type == "card.action.trigger" || native.pointer("/event/action").is_some() {
        return None;
    }
    if let Some(event_id) = native
        .pointer("/header/event_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
    {
        return Some(format!("feishu:event:{event_id}"));
    }
    native
        .pointer("/event/message/message_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(|message_id| format!("feishu:message:{message_id}"))
}

fn build_feishu_card_action_response(native: &serde_json::Value) -> Option<serde_json::Value> {
    let event = native.get("event")?;
    let action_value = event.pointer("/action/value")?;
    let action = approval_card::parse_action_value(action_value).ok()?;
    let operator_open_id = event
        .pointer("/operator/open_id")
        .or_else(|| event.pointer("/operator/operator_id/open_id"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    approval_card::build_callback_response(&action, operator_open_id).ok()
}

fn parse_feishu_event_with_resolver(
    native: serde_json::Value,
    share_session_in_group: bool,
    bot_open_id: Option<&str>,
    resolver: &Arc<StdMutex<ApprovalCardResolver>>,
) -> Result<Option<AgentRequest>> {
    let event = native
        .get("event")
        .ok_or_else(|| rua_core::RuaError::Validation {
            message: "feishu event missing 'event' field".into(),
        })?;
    let sender_type = event
        .pointer("/sender/sender_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if sender_type == "bot" {
        tracing::debug!("feishu: skipping bot message");
        return Ok(None);
    }
    if let Some(req) = parse_feishu_approval_action_event_with_resolver(event, resolver)? {
        return Ok(Some(req));
    }
    parse_feishu_event_with_options(native, share_session_in_group, bot_open_id)
}

fn parse_feishu_approval_action_event_with_resolver(
    event: &serde_json::Value,
    resolver: &Arc<StdMutex<ApprovalCardResolver>>,
) -> Result<Option<AgentRequest>> {
    let Some(action_value) = event.pointer("/action/value") else {
        return Ok(None);
    };
    let parsed = match approval_card::parse_action_value(action_value) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(None),
    };
    let operator_open_id = event
        .pointer("/operator/open_id")
        .or_else(|| event.pointer("/operator/operator_id/open_id"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    resolver
        .lock()
        .map_err(|_| rua_core::RuaError::Runtime {
            message: "feishu approval resolver lock poisoned".into(),
        })?
        .resolve(ApprovalCardCallback {
            request_id: parsed.request_id.clone(),
            action: parsed.action.clone(),
            decided_by: operator_open_id,
            platform: "feishu".into(),
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
    Ok(parse_feishu_approval_action_event(event))
}

fn parse_feishu_approval_action_event(event: &serde_json::Value) -> Option<AgentRequest> {
    let action_value = event.pointer("/action/value")?;
    let parsed = approval_card::parse_action_value(action_value).ok()?;
    let session_ctx = parsed.session_ctx.as_object();
    let operator_open_id = event
        .pointer("/operator/open_id")
        .or_else(|| event.pointer("/operator/operator_id/open_id"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let sender_id = session_ctx
        .and_then(|ctx| ctx.get("sender_id"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&operator_open_id)
        .to_string();
    let session_id = session_ctx
        .and_then(|ctx| ctx.get("session_id"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&sender_id)
        .to_string();
    let receive_id = session_ctx
        .and_then(|ctx| ctx.get("receive_id"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let receive_id_type = session_ctx
        .and_then(|ctx| ctx.get("receive_id_type"))
        .and_then(|value| value.as_str())
        .unwrap_or("open_id")
        .to_string();
    let chat_id = session_ctx
        .and_then(|ctx| ctx.get("chat_id"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let chat_type = session_ctx
        .and_then(|ctx| ctx.get("chat_type"))
        .and_then(|value| value.as_str())
        .unwrap_or("p2p")
        .to_string();
    let is_group = session_ctx
        .and_then(|ctx| ctx.get("is_group"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let conversation_id = if !chat_id.is_empty() {
        chat_id.clone()
    } else if !receive_id.is_empty() {
        receive_id.clone()
    } else {
        session_id.clone()
    };
    let approval_action = match parsed.action {
        crate::core::approval_card::ApprovalCardAction::Approve => "approve",
        crate::core::approval_card::ApprovalCardAction::Deny => "deny",
    };

    Some(AgentRequest {
        agent_id: String::new(),
        user_id: sender_id.clone(),
        session_id,
        input: vec![IncomingContentPart::Text {
            text: parsed.command_text(),
        }],
        channel_id: "feishu".into(),
        conversation_id,
        message_id: None,
        metadata: serde_json::json!({
            "sender_id": sender_id,
            "is_group": is_group,
            "chat_type": chat_type,
            "feishu_chat_id": chat_id,
            "feishu_chat_type": chat_type,
            "feishu_sender_id": sender_id,
            "feishu_receive_id": receive_id,
            "feishu_receive_id_type": receive_id_type,
            "feishu_operator_open_id": operator_open_id,
            "approval_request_id": parsed.request_id,
            "approval_action": approval_action,
            "approval_tool_name": parsed.tool_name,
            "from_card_action": true,
        }),
    })
}

fn feishu_thread_user_id(thread_id: &str) -> Option<String> {
    if thread_id.is_empty() {
        return None;
    }
    let short_id = thread_id.rsplit('_').next().unwrap_or(thread_id);
    Some(format!("thread:{short_id}"))
}

fn parse_content_json(content: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(content).ok()
}

fn extract_json_key(content: &str, keys: &[&str]) -> Option<String> {
    let data = parse_content_json(content)?;
    let obj = data.as_object()?;
    extract_json_key_from_obj(obj, keys)
}

fn extract_json_key_from_obj(
    obj: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<String> {
    for key in keys {
        if let Some(value) = obj.get(*key).or_else(|| obj.get(&key.replace('_', ""))) {
            if let Some(text) = value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                return Some(text.to_string());
            }
        }
    }
    None
}

#[derive(Debug, Default)]
struct FeishuMention {
    is_bot_mentioned: bool,
    keys: Vec<String>,
}

fn feishu_bot_mention(
    message: &serde_json::Value,
    content_raw: &str,
    bot_open_id: Option<&str>,
) -> FeishuMention {
    let mut mention = FeishuMention {
        is_bot_mentioned: content_raw.contains("@_all"),
        keys: Vec::new(),
    };
    let Some(bot_open_id) = bot_open_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return mention;
    };
    let Some(mentions) = message.get("mentions").and_then(|value| value.as_array()) else {
        return mention;
    };
    for item in mentions {
        let mentioned_open_id = item
            .pointer("/id/open_id")
            .or_else(|| item.get("open_id"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if mentioned_open_id == bot_open_id {
            mention.is_bot_mentioned = true;
            if let Some(key) = item
                .get("key")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                mention.keys.push(key.to_string());
            }
        }
    }
    mention
}

fn extract_message_parts_with_mention(
    msg_type: &str,
    content: &str,
    mention: &FeishuMention,
) -> Vec<IncomingContentPart> {
    if let Some(part) = extract_local_media_part(msg_type, content) {
        return vec![part];
    }

    let mut text = extract_message_text(msg_type, content);
    if msg_type == "text" && mention.is_bot_mentioned {
        for key in &mention.keys {
            text = text.replace(key, "");
        }
        text = text.trim().to_string();
    }
    if text.is_empty() {
        Vec::new()
    } else {
        vec![IncomingContentPart::Text { text }]
    }
}

fn extract_local_media_part(msg_type: &str, content: &str) -> Option<IncomingContentPart> {
    let data = parse_content_json(content)?;
    let obj = data.as_object()?;
    let path = extract_json_key_from_obj(obj, &["local_path", "path"])?;
    let file_id = match msg_type {
        "image" => extract_json_key_from_obj(obj, &["image_key", "file_key", "file_id"])?,
        "file" | "media" | "audio" => {
            extract_json_key_from_obj(obj, &["file_key", "file_id", "image_key"])?
        }
        _ => return None,
    };
    let mime = extract_json_key_from_obj(obj, &["mime", "mime_type", "content_type"]);
    let name = extract_json_key_from_obj(obj, &["file_name", "name"]);
    let path = PathBuf::from(path);

    match msg_type {
        "image" => Some(IncomingContentPart::Image {
            file_id,
            path,
            mime,
        }),
        "file" => Some(IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        }),
        "audio" => Some(IncomingContentPart::Audio {
            file_id,
            path,
            mime,
            name,
        }),
        "media" => Some(IncomingContentPart::Video {
            file_id,
            path,
            mime,
            name,
        }),
        _ => None,
    }
}

async fn download_feishu_inbound_media_with(
    client: &reqwest::Client,
    open_api_base_url: &str,
    token: &str,
    media_dir: &str,
    native: &mut serde_json::Value,
) -> Result<()> {
    let Some(message) = native.pointer_mut("/event/message") else {
        return Ok(());
    };
    let message_id = message
        .get("message_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string();
    let msg_type = message
        .get("message_type")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if message_id.is_empty() {
        return Ok(());
    }
    let content_raw = message
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let Some(mut content) = parse_content_json(content_raw) else {
        return Ok(());
    };
    if content.get("local_path").and_then(|v| v.as_str()).is_some() {
        return Ok(());
    }
    let Some(file_key) = feishu_media_file_key(&msg_type, &content) else {
        return Ok(());
    };
    let resource_type = if msg_type == "image" { "image" } else { "file" };
    let Some((bytes, mime)) = feishu_download_message_resource(
        client,
        open_api_base_url,
        token,
        &message_id,
        &file_key,
        resource_type,
    )
    .await?
    else {
        return Ok(());
    };

    let filename = feishu_download_filename(&content, &message_id, &file_key, &msg_type);
    let dir = feishu_media_dir(media_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("create feishu media dir {}: {err}", dir.display()),
        })?;
    let final_path = dir.join(filename);
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
    message["content"] = serde_json::Value::String(content.to_string());
    Ok(())
}

fn feishu_media_file_key(msg_type: &str, content: &serde_json::Value) -> Option<String> {
    let obj = content.as_object()?;
    let keys: &[&str] = match msg_type {
        "image" => &["image_key", "file_key", "file_id"],
        "file" | "media" | "audio" => &["file_key", "file_id", "image_key"],
        _ => return None,
    };
    extract_json_key_from_obj(obj, keys)
}

async fn feishu_download_message_resource(
    client: &reqwest::Client,
    open_api_base_url: &str,
    token: &str,
    message_id: &str,
    file_key: &str,
    resource_type: &str,
) -> Result<Option<(Vec<u8>, Option<String>)>> {
    let url = format!(
        "{}/open-apis/im/v1/messages/{}/resources/{}",
        open_api_base_url.trim_end_matches('/'),
        message_id,
        file_key
    );
    let resp = match client
        .get(url)
        .query(&[("type", resource_type)])
        .header("Authorization", format!("Bearer {token}"))
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%message_id, %file_key, error = %err, "feishu media download failed");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(%message_id, %file_key, status = %resp.status(), "feishu media download non-success");
        return Ok(None);
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_INBOUND_MEDIA_SIZE {
            tracing::warn!(%message_id, %file_key, len, "feishu media too large");
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
            message: format!("feishu media download bytes failed: {err}"),
        })?;
    if bytes.len() as u64 > MAX_INBOUND_MEDIA_SIZE {
        tracing::warn!(%message_id, %file_key, len = bytes.len(), "feishu media too large");
        return Ok(None);
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some((bytes.to_vec(), mime)))
}

fn feishu_media_dir(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    if media_dir.is_empty() {
        std::env::temp_dir().join("rua-feishu-media")
    } else {
        PathBuf::from(media_dir)
    }
}

fn feishu_download_filename(
    content: &serde_json::Value,
    message_id: &str,
    file_key: &str,
    msg_type: &str,
) -> String {
    let hint = content
        .get("file_name")
        .or_else(|| content.get("name"))
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{message_id}_{file_key}_{msg_type}"));
    crate::core::media::sanitize_filename(&hint)
}

async fn atomic_write_file(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| rua_core::RuaError::Io {
        message: format!("missing parent dir for {}", path.display()),
    })?;
    let tmp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "feishu-media".into()),
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

fn extract_message_text(msg_type: &str, content: &str) -> String {
    match msg_type {
        "text" => extract_json_key(content, &["text"]).unwrap_or_default(),
        "post" => extract_structured_text(content, "content").unwrap_or_default(),
        "interactive" => extract_structured_text(content, "elements").unwrap_or_default(),
        "image" => extract_json_key(content, &["image_key"])
            .map(|key| format!("[image: {key}]"))
            .unwrap_or_else(|| "[image: missing key]".to_string()),
        "file" => extract_json_key(content, &["file_name", "file_key"])
            .map(|key| format!("[file: {key}]"))
            .unwrap_or_else(|| "[file: missing key]".to_string()),
        "media" => extract_json_key(content, &["file_key"])
            .map(|key| format!("[media: {key}]"))
            .unwrap_or_else(|| "[media: missing key]".to_string()),
        "audio" => extract_json_key(content, &["file_key"])
            .map(|key| format!("[audio: {key}]"))
            .unwrap_or_else(|| "[audio: missing key]".to_string()),
        other => format!("[{other}]"),
    }
}

fn extract_structured_text(content: &str, blocks_key: &str) -> Option<String> {
    let data = parse_content_json(content)?;
    let obj = data.as_object()?;
    let mut parts = Vec::new();

    if let Some(title) = extract_card_title(obj) {
        parts.push(title);
    }

    if let Some(blocks) = obj.get(blocks_key).or_else(|| {
        obj.get("body")
            .and_then(|body| body.as_object())
            .and_then(|body| body.get(blocks_key))
    }) {
        collect_text_parts(blocks, &mut parts);
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn extract_card_title(obj: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    if let Some(title) = obj.get("title").and_then(|v| v.as_str()) {
        let title = title.trim();
        if !title.is_empty() {
            return Some(title.to_string());
        }
    }

    let header_title = obj
        .get("header")
        .and_then(|v| v.as_object())
        .and_then(|header| header.get("title"));
    match header_title {
        Some(serde_json::Value::String(title)) if !title.trim().is_empty() => {
            Some(title.trim().to_string())
        }
        Some(serde_json::Value::Object(title)) => title
            .get("content")
            .or_else(|| title.get("text"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned),
        _ => None,
    }
}

fn collect_text_parts(value: &serde_json::Value, parts: &mut Vec<String>) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                collect_text_parts(item, parts);
            }
        }
        serde_json::Value::Object(item) => {
            match item.get("tag").and_then(|v| v.as_str()).unwrap_or("") {
                "text" | "code_block" | "md" | "plain_text" | "lark_md" | "markdown" | "button" => {
                    collect_plain_text(item, parts)
                }
                "a" => collect_link_text(item, parts),
                "at" => {
                    let name = item
                        .get("user_name")
                        .or_else(|| item.get("user_id"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    if let Some(name) = name {
                        parts.push(format!("@{name}"));
                    }
                }
                "table" => collect_table_text(item, parts),
                _ => {}
            }

            for key in ["elements", "columns", "body", "content", "actions"] {
                if let Some(children) = item.get(key) {
                    collect_text_parts(children, parts);
                }
            }
        }
        _ => {}
    }
}

fn collect_plain_text(item: &serde_json::Map<String, serde_json::Value>, parts: &mut Vec<String>) {
    let text_value = item.get("text").or_else(|| item.get("content"));
    let text = match text_value {
        Some(serde_json::Value::String(text)) => Some(text.as_str()),
        Some(serde_json::Value::Object(obj)) => obj
            .get("content")
            .or_else(|| obj.get("text"))
            .and_then(|v| v.as_str()),
        _ => None,
    };
    if let Some(text) = text.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(text.to_string());
    }
}

fn collect_link_text(item: &serde_json::Map<String, serde_json::Value>, parts: &mut Vec<String>) {
    let text = item
        .get("text")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    let href = item
        .get("href")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    if !text.is_empty() && !href.is_empty() {
        parts.push(format!("[{text}]({href})"));
    } else if !text.is_empty() {
        parts.push(text.to_string());
    } else if !href.is_empty() {
        parts.push(href.to_string());
    }
}

fn collect_table_text(item: &serde_json::Map<String, serde_json::Value>, parts: &mut Vec<String>) {
    let columns = item
        .get("columns")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut names = Vec::new();
    let mut headers = Vec::new();
    for column in columns {
        if let Some(column) = column.as_object() {
            if let Some(name) = column.get("name").and_then(|v| v.as_str()) {
                names.push(name.to_string());
            }
            if let Some(header) = column.get("display_name").and_then(|v| v.as_str()) {
                headers.push(header.to_string());
            }
        }
    }
    if !headers.is_empty() {
        parts.push(headers.join(" | "));
    }

    if let Some(rows) = item.get("rows").and_then(|v| v.as_array()) {
        for row in rows {
            if let Some(row) = row.as_object() {
                let cells = names
                    .iter()
                    .map(|name| {
                        row.get(name)
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string()
                    })
                    .collect::<Vec<_>>();
                if cells.iter().any(|cell| !cell.is_empty()) {
                    parts.push(cells.join(" | "));
                }
            }
        }
    }
}
