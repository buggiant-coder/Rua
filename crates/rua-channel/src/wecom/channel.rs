use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use std::path::PathBuf;

use aes::cipher::{BlockDecrypt, KeyInit};
use aes::{Aes128, Aes192, Aes256};
use async_trait::async_trait;
use base64::Engine;
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::core::approval_card::{
    ApprovalCardCallback, ApprovalCardResolveError, ApprovalCardResolver,
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
use crate::wecom::client::WecomWsSender;
use rua_core::Result;

use super::approval_card;
use super::client;
use super::types::{WecomBotConfig, WsFrame};

const DEBOUNCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);
const MAX_INBOUND_MEDIA_SIZE: u64 = 50 * 1024 * 1024;
const STREAM_MIN_INTERVAL: Duration = Duration::from_millis(500);
const STREAM_MAX_CHARS: usize = 3000;
const PROCESSING_TEXT: &str = "🤔 Thinking...";
const PROCESSING_REFRESH_INTERVAL: Duration = Duration::from_secs(20);
const APPROVAL_CALLBACK_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

#[derive(Debug)]
struct WecomStreamInner {
    state: PlatformStreamState,
}

#[derive(Debug)]
pub struct WecomStreamHandler {
    sender: WecomWsSender,
    source: WsFrame,
    stream_id: String,
    policy: PlatformStreamPolicy,
    started_at: tokio::time::Instant,
    inner: Mutex<WecomStreamInner>,
    processing_consumed: Option<Arc<AtomicBool>>,
}

impl WecomStreamHandler {
    pub fn new(sender: WecomWsSender, source: WsFrame, policy: PlatformStreamPolicy) -> Self {
        let stream_id = processing_stream_id(&source);
        Self::with_stream_id(sender, source, policy, stream_id, None)
    }

    pub fn with_stream_id(
        sender: WecomWsSender,
        source: WsFrame,
        policy: PlatformStreamPolicy,
        stream_id: String,
        processing_consumed: Option<Arc<AtomicBool>>,
    ) -> Self {
        Self {
            sender,
            source,
            stream_id,
            policy,
            started_at: tokio::time::Instant::now(),
            inner: Mutex::new(WecomStreamInner {
                state: PlatformStreamState::default(),
            }),
            processing_consumed,
        }
    }
}

fn processing_stream_id(source: &WsFrame) -> String {
    let req_id = source.headers.req_id.trim();
    if req_id.is_empty() {
        format!("rua-{}", uuid::Uuid::new_v4())
    } else {
        format!("rua-{req_id}")
    }
}

impl WecomStreamHandler {
    async fn send_stream(&self, content: &str, finish: bool) -> Result<()> {
        if let Some(consumed) = &self.processing_consumed {
            consumed.store(true, Ordering::SeqCst);
        }
        self.sender
            .reply(
                &self.source,
                client::build_reply_stream_body(&self.stream_id, content, finish),
            )
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("wecom reply_stream failed: {err}"),
            })
    }

    async fn send_final_markdown(&self, content: &str) -> Result<()> {
        if let Some(consumed) = &self.processing_consumed {
            consumed.store(true, Ordering::SeqCst);
        }
        self.sender
            .reply(
                &self.source,
                serde_json::json!({
                    "msgtype": "markdown",
                    "markdown": { "content": content },
                }),
            )
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("wecom final reply failed: {err}"),
            })
    }

    async fn apply_action(&self, action: PlatformStreamAction) -> Result<()> {
        match action {
            PlatformStreamAction::Create { content, .. } => {
                self.send_stream(&content, false).await?;
            }
            PlatformStreamAction::Update {
                content,
                final_update,
                ..
            } => {
                self.send_stream(&content, final_update).await?;
            }
            PlatformStreamAction::SendFinal { content } => {
                self.send_final_markdown(&content).await?;
            }
            PlatformStreamAction::Skip => {}
        }
        Ok(())
    }
}

#[async_trait]
impl StreamHandler for WecomStreamHandler {
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

#[derive(Debug)]
pub struct WecomProcessingIndicator {
    sender: WecomWsSender,
    source: WsFrame,
    stream_id: String,
    consumed: Arc<AtomicBool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl WecomProcessingIndicator {
    pub fn new(
        sender: WecomWsSender,
        source: WsFrame,
        stream_id: String,
        consumed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            sender,
            source,
            stream_id,
            consumed,
            task: Mutex::new(None),
        }
    }

    async fn send_processing(&self, finish: bool) -> Result<()> {
        self.sender
            .reply(
                &self.source,
                client::build_reply_stream_body(&self.stream_id, PROCESSING_TEXT, finish),
            )
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("wecom processing indicator failed: {err}"),
            })
    }

    async fn abort_task(&self) {
        if let Some(handle) = self.task.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

#[async_trait]
impl ProcessingIndicator for WecomProcessingIndicator {
    async fn start(&self, _req: &AgentChannelRequest) -> Result<()> {
        self.abort_task().await;
        self.consumed.store(false, Ordering::SeqCst);
        self.send_processing(false).await?;

        let sender = self.sender.clone();
        let source = self.source.clone();
        let stream_id = self.stream_id.clone();
        let consumed = self.consumed.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(PROCESSING_REFRESH_INTERVAL).await;
                if consumed.load(Ordering::SeqCst) {
                    break;
                }
                if let Err(err) = sender
                    .reply(
                        &source,
                        client::build_reply_stream_body(&stream_id, PROCESSING_TEXT, false),
                    )
                    .await
                {
                    tracing::debug!(error = %err, "wecom processing keepalive failed");
                }
            }
        });
        *self.task.lock().await = Some(handle);
        Ok(())
    }

    async fn stop(&self, _req: &AgentChannelRequest) -> Result<()> {
        self.abort_task().await;
        if !self.consumed.load(Ordering::SeqCst) {
            let _ = self.send_processing(true).await;
        }
        Ok(())
    }
}

pub struct WecomChannel {
    channel_id: ChannelId,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    bot_id: String,
    secret: String,
    ws_url: String,
    ws_stop_tx: watch::Sender<bool>,
    ws_handle: RwLock<Option<JoinHandle<()>>>,
    ws_sender: RwLock<Option<WecomWsSender>>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    debouncer: BufferDebouncer<String, AgentRequest>,
    media_dir: String,
    approval_resolver: Arc<StdMutex<ApprovalCardResolver>>,
    event_deduper: Arc<StdMutex<EventDeduper>>,
}

impl WecomChannel {
    pub fn new(bot_id: String, secret: String) -> Self {
        let (ws_stop_tx, _) = watch::channel(false);
        let render_options = MessageRenderOptions::for_channel_kind(ChannelKind::WeCom);
        Self {
            channel_id: "wecom".into(),
            process: RwLock::new(None),
            started: RwLock::new(false),
            bot_id,
            secret,
            ws_url: super::types::DEFAULT_WS_URL.to_string(),
            ws_stop_tx,
            ws_handle: RwLock::new(None),
            ws_sender: RwLock::new(None),
            render_options,
            acl_metadata: serde_json::json!({}),
            debouncer: BufferDebouncer::new(DEBOUNCE_WINDOW),
            media_dir: String::new(),
            approval_resolver: Arc::new(StdMutex::new(ApprovalCardResolver::new(
                APPROVAL_CALLBACK_TTL,
            ))),
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

    pub fn with_ws_url(mut self, ws_url: String) -> Self {
        self.ws_url = ws_url;
        self
    }

    pub async fn download_inbound_media(&self, native: &mut serde_json::Value) -> Result<()> {
        download_wecom_inbound_media(&self.media_dir, native).await
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

    fn approval_card_body(to: &ToHandle, msg: &OutMessage) -> Option<serde_json::Value> {
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
        let session_id = to
            .metadata
            .get("session_id")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&to.conversation_id);
        let sender_id = to
            .sender_id
            .as_deref()
            .or_else(|| {
                to.metadata
                    .get("wecom_sender_id")
                    .and_then(|value| value.as_str())
            })
            .unwrap_or("");
        let card = approval_card::build_approval_card(
            request_id,
            tool_name,
            severity,
            serde_json::json!({
                "session_id": session_id,
                "sender_id": sender_id,
            }),
        )
        .ok()?;
        Some(serde_json::json!({
            "msgtype": "template_card",
            "template_card": card,
        }))
    }

    async fn send_outgoing_with_sender(
        sender: &WecomWsSender,
        to: ToHandle,
        msg: OutMessage,
    ) -> Result<()> {
        let chatid = to
            .metadata
            .get("wecom_chatid")
            .and_then(|v| v.as_str())
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                to.conversation_id
                    .strip_prefix("wecom:group:")
                    .filter(|value| !value.trim().is_empty())
            })
            .or_else(|| {
                to.conversation_id
                    .strip_prefix("group:")
                    .filter(|value| !value.trim().is_empty())
            });
        let source_frame = to
            .metadata
            .get("wecom_frame")
            .and_then(|value| serde_json::from_value::<WsFrame>(value.clone()).ok());
        let text = Self::render_text_and_unresolved_media(&msg.content);
        if !text.is_empty() {
            if let Some(body) = Self::approval_card_body(&to, &msg) {
                let result = if let Some(frame) = &source_frame {
                    sender.reply(frame, body).await
                } else if let Some(chatid) = chatid {
                    sender.send_message(chatid, body).await
                } else {
                    Ok(())
                };
                match result {
                    Ok(()) => return Ok(()),
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "wecom approval card send failed; falling back to markdown"
                        );
                    }
                }
            }
            let body = serde_json::json!({
                "msgtype": "markdown",
                "markdown": { "content": text },
            });
            if let Some(frame) = &source_frame {
                sender
                    .reply(frame, body)
                    .await
                    .map_err(|err| rua_core::RuaError::Runtime {
                        message: format!("wecom reply send failed: {err}"),
                    })?;
            } else if let Some(chatid) = chatid {
                sender.send_message(chatid, body).await.map_err(|err| {
                    rua_core::RuaError::Runtime {
                        message: format!("wecom proactive send failed: {err}"),
                    }
                })?;
            }
        }

        for part in &msg.content {
            if !Self::is_media_part(part) {
                continue;
            }
            match Self::send_media_part(sender, chatid, source_frame.as_ref(), part).await {
                Ok(true) | Ok(false) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "wecom media send failed; sending fallback text");
                    let fallback = Self::render_content(std::slice::from_ref(part));
                    if fallback.is_empty() {
                        continue;
                    }
                    let body = serde_json::json!({
                        "msgtype": "markdown",
                        "markdown": { "content": fallback },
                    });
                    if let Some(frame) = &source_frame {
                        sender.reply(frame, body).await.map_err(|err| {
                            rua_core::RuaError::Runtime {
                                message: format!("wecom fallback reply failed: {err}"),
                            }
                        })?;
                    } else if let Some(chatid) = chatid {
                        sender.send_message(chatid, body).await.map_err(|err| {
                            rua_core::RuaError::Runtime {
                                message: format!("wecom fallback send failed: {err}"),
                            }
                        })?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn send_media_part(
        sender: &WecomWsSender,
        chatid: Option<&str>,
        source_frame: Option<&WsFrame>,
        part: &OutgoingContentPart,
    ) -> Result<bool> {
        let Some((path, media_type)) = wecom_media_path_and_type(part) else {
            return Ok(false);
        };
        let media_id = sender
            .upload_media(path, media_type, std::time::Duration::from_secs(30))
            .await
            .map_err(|err| rua_core::RuaError::Runtime {
                message: format!("wecom media upload failed: {err}"),
            })?;
        let body = client::build_wecom_media_message(media_type, &media_id);
        if let Some(frame) = source_frame {
            sender
                .reply(frame, body)
                .await
                .map_err(|err| rua_core::RuaError::Runtime {
                    message: format!("wecom media reply failed: {err}"),
                })?;
            Ok(true)
        } else if let Some(chatid) = chatid {
            sender
                .send_message(chatid, body)
                .await
                .map_err(|err| rua_core::RuaError::Runtime {
                    message: format!("wecom media send failed: {err}"),
                })?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

fn wecom_media_path_and_type(part: &OutgoingContentPart) -> Option<(&Path, &'static str)> {
    match part {
        OutgoingContentPart::Image {
            path: Some(path), ..
        } => Some((path.as_path(), "image")),
        OutgoingContentPart::Video {
            path: Some(path), ..
        } => Some((path.as_path(), "video")),
        OutgoingContentPart::Audio {
            path: Some(path), ..
        } => {
            let media_type = if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("amr"))
            {
                "voice"
            } else {
                "file"
            };
            Some((path.as_path(), media_type))
        }
        OutgoingContentPart::File {
            path: Some(path), ..
        } => Some((path.as_path(), "file")),
        _ => None,
    }
}

fn parse_callback_frame(frame: &WsFrame, bot_id: Option<&str>) -> Option<AgentChannelRequest> {
    let body = frame.body.as_ref()?;
    if let Some(req) = parse_approval_card_frame(frame, body) {
        return Some(req);
    }
    let msgtype = body.get("msgtype")?.as_str()?;

    let from = body.get("from").and_then(|f| f.as_object());
    let sender_id = from
        .and_then(|f| f.get("userid"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let user_name = from
        .and_then(|f| f.get("name").or_else(|| f.get("username")))
        .and_then(|v| v.as_str())
        .unwrap_or(&sender_id)
        .to_string();
    let chatid = body
        .get("chatid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let chat_type = body
        .get("chattype")
        .and_then(|v| v.as_str())
        .unwrap_or("single");
    let msg_id = body
        .get("msgid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let is_group = chat_type == "group";
    let session_id = if is_group && !chatid.is_empty() {
        format!("wecom:group:{chatid}")
    } else if !sender_id.is_empty() {
        format!("wecom:{sender_id}")
    } else {
        format!("wecom:{chatid}")
    };

    let bot_mention = wecom_bot_mention(body, bot_id);
    let input = match msgtype {
        "text" => {
            let text = body
                .get("text")
                .and_then(|t| t.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("");
            text_input(&strip_wecom_bot_mention(text, &bot_mention))
        }
        "voice" => text_input(
            body.get("voice")
                .and_then(|v| v.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("[voice]"),
        ),
        "image" => vec![wecom_media_part(
            body.get("image"),
            "image",
            &["url", "media_id"],
        )],
        "file" => vec![wecom_media_part(
            body.get("file"),
            "file",
            &["filename", "file_name", "url", "media_id"],
        )],
        "video" => vec![wecom_media_part(
            body.get("video"),
            "video",
            &["filename", "file_name", "url", "media_id"],
        )],
        _ => vec![IncomingContentPart::Text {
            text: format!("[{msgtype}]"),
        }],
    };

    if input.is_empty() {
        return None;
    }

    let req_id = frame.headers.req_id.clone();
    let frame_value = serde_json::to_value(frame).unwrap_or_default();

    Some(AgentChannelRequest {
        agent_id: String::new(),
        user_id: if is_group {
            String::new()
        } else {
            sender_id.clone()
        },
        session_id,
        input,
        channel_id: "wecom".into(),
        conversation_id: chatid.clone(),
        message_id: if msg_id.is_empty() {
            None
        } else {
            Some(msg_id)
        },
        metadata: serde_json::json!({
            "wecom_sender_id": sender_id,
            "wecom_chatid": chatid,
            "wecom_chat_type": chat_type,
            "wecom_req_id": req_id,
            "wecom_frame": frame_value,
            "is_group": is_group,
            "is_dm": !is_group,
            "bot_mentioned": bot_mention.is_bot_mentioned,
            "mentioned": bot_mention.is_bot_mentioned,
            "user_name": user_name,
        }),
    })
}

#[derive(Debug, Default)]
struct WecomMention {
    is_bot_mentioned: bool,
    tokens: Vec<String>,
}

fn wecom_bot_mention(body: &serde_json::Value, bot_id: Option<&str>) -> WecomMention {
    let mut mention = WecomMention::default();
    let Some(bot_id) = bot_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return mention;
    };

    let mentioned = body
        .get("mentioned_list")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .any(|value| value == bot_id)
        || body
            .get("mentions")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .any(|item| {
                ["userid", "user_id", "id", "name"]
                    .into_iter()
                    .filter_map(|key| item.get(key).and_then(|value| value.as_str()))
                    .any(|value| value == bot_id)
            });
    if mentioned {
        mention.is_bot_mentioned = true;
        mention.tokens.push(format!("@{bot_id}"));
    }
    mention
}

fn strip_wecom_bot_mention(text: &str, mention: &WecomMention) -> String {
    if !mention.is_bot_mentioned {
        return text.to_string();
    }
    let mut stripped = text.to_string();
    for token in &mention.tokens {
        stripped = stripped.replace(token, " ");
    }
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn wecom_check_duplicate_request(
    deduper: &Arc<StdMutex<EventDeduper>>,
    req: &AgentChannelRequest,
) -> Result<bool> {
    if req
        .metadata
        .get("from_card_action")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        return Ok(false);
    }
    let key = req
        .message_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|message_id| format!("wecom:message:{message_id}"))
        .or_else(|| {
            req.metadata
                .get("wecom_req_id")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|req_id| format!("wecom:req:{req_id}"))
        });
    let Some(key) = key else {
        return Ok(false);
    };
    let mut deduper = deduper.lock().map_err(|_| rua_core::RuaError::Runtime {
        message: "wecom event deduper lock poisoned".into(),
    })?;
    Ok(deduper.check_and_mark(&key) == EventDedupDecision::Duplicate)
}

fn is_template_card_event_frame(frame: &WsFrame) -> bool {
    frame
        .body
        .as_ref()
        .and_then(|body| body.get("msgtype"))
        .and_then(|value| value.as_str())
        == Some("template_card_event")
        || frame
            .body
            .as_ref()
            .and_then(|body| body.pointer("/event/template_card_event"))
            .is_some()
}

async fn update_wecom_resolved_approval_card(
    sender: &WecomWsSender,
    frame: &WsFrame,
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
    let task_id = req
        .metadata
        .get("approval_task_id")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if task_id.trim().is_empty() {
        return;
    }
    let action = match req
        .metadata
        .get("approval_action")
        .and_then(|value| value.as_str())
        .unwrap_or("")
    {
        "approve" => crate::core::approval_card::ApprovalCardAction::Approve,
        "deny" => crate::core::approval_card::ApprovalCardAction::Deny,
        _ => return,
    };
    let tool_name = req
        .metadata
        .get("approval_tool_name")
        .and_then(|value| value.as_str())
        .unwrap_or("tool");
    let decided_by = req
        .metadata
        .get("wecom_sender_id")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let card = approval_card::build_resolved_card(task_id, tool_name, action, decided_by);
    if let Err(err) = sender.update_template_card(frame, card).await {
        tracing::warn!(error = %err, task_id, "wecom approval resolved card update failed");
    }
}

fn parse_callback_frame_with_resolver(
    frame: &WsFrame,
    resolver: &Arc<StdMutex<ApprovalCardResolver>>,
    bot_id: Option<&str>,
) -> Result<Option<AgentChannelRequest>> {
    let Some(body) = frame.body.as_ref() else {
        return Ok(None);
    };
    if let Some(req) = parse_approval_card_frame_with_resolver(frame, body, resolver)? {
        return Ok(Some(req));
    }
    Ok(parse_callback_frame(frame, bot_id))
}

fn parse_approval_card_frame_with_resolver(
    frame: &WsFrame,
    body: &serde_json::Value,
    resolver: &Arc<StdMutex<ApprovalCardResolver>>,
) -> Result<Option<AgentChannelRequest>> {
    let action = match approval_card::parse_card_event(body) {
        Ok(action) => action,
        Err(_) => return Ok(None),
    };
    resolver
        .lock()
        .map_err(|_| rua_core::RuaError::Runtime {
            message: "wecom approval resolver lock poisoned".into(),
        })?
        .resolve(ApprovalCardCallback {
            request_id: action.request_id.clone(),
            action: action.action.clone(),
            decided_by: action.user_id.clone(),
            platform: "wecom".into(),
        })
        .map_err(|err| match err {
            ApprovalCardResolveError::Duplicate => rua_core::RuaError::Validation {
                message: format!("duplicate approval callback: {}", action.request_id),
            },
            ApprovalCardResolveError::MissingRequestId => rua_core::RuaError::Validation {
                message: "approval callback missing request id".into(),
            },
            ApprovalCardResolveError::Stale => rua_core::RuaError::Validation {
                message: format!("stale approval callback: {}", action.request_id),
            },
        })?;
    Ok(parse_approval_card_frame(frame, body))
}

fn parse_approval_card_frame(
    frame: &WsFrame,
    body: &serde_json::Value,
) -> Option<AgentChannelRequest> {
    let action = approval_card::parse_card_event(body).ok()?;
    let from = body.get("from").and_then(|f| f.as_object());
    let sender_id = from
        .and_then(|f| f.get("userid"))
        .and_then(|v| v.as_str())
        .unwrap_or(&action.user_id)
        .to_string();
    let chatid = body
        .get("chatid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let chat_type = body
        .get("chattype")
        .and_then(|v| v.as_str())
        .unwrap_or("single");
    let is_group = chat_type == "group";
    let session_from_card = action
        .session_ctx
        .get("session_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty());
    let session_id = session_from_card.map(ToOwned::to_owned).unwrap_or_else(|| {
        if is_group && !chatid.is_empty() {
            format!("wecom:group:{chatid}")
        } else if !sender_id.is_empty() {
            format!("wecom:{sender_id}")
        } else {
            format!("wecom:{chatid}")
        }
    });
    let approval_action = match action.action {
        crate::core::approval_card::ApprovalCardAction::Approve => "approve",
        crate::core::approval_card::ApprovalCardAction::Deny => "deny",
    };
    let req_id = frame.headers.req_id.clone();
    let frame_value = serde_json::to_value(frame).unwrap_or_default();

    Some(AgentChannelRequest {
        agent_id: String::new(),
        user_id: if is_group {
            String::new()
        } else {
            sender_id.clone()
        },
        session_id,
        input: text_input(&action.command_text()),
        channel_id: "wecom".into(),
        conversation_id: chatid.clone(),
        message_id: None,
        metadata: serde_json::json!({
            "wecom_sender_id": sender_id,
            "wecom_chatid": chatid,
            "wecom_chat_type": chat_type,
            "wecom_req_id": req_id,
            "wecom_frame": frame_value,
            "is_group": is_group,
            "approval_request_id": action.request_id,
            "approval_action": approval_action,
            "approval_task_id": action.task_id,
            "approval_tool_name": action.tool_name,
            "approval_severity": action.severity,
            "from_card_action": true,
        }),
    })
}

fn text_input(text: &str) -> Vec<IncomingContentPart> {
    let text = text.trim();
    if text.is_empty() {
        Vec::new()
    } else {
        vec![IncomingContentPart::Text {
            text: text.to_string(),
        }]
    }
}

fn wecom_media_part(
    value: Option<&serde_json::Value>,
    label: &str,
    keys: &[&str],
) -> IncomingContentPart {
    if let Some(part) = wecom_local_media_part(value, label, keys) {
        return part;
    }
    IncomingContentPart::Text {
        text: wecom_media_placeholder(value, label, keys),
    }
}

fn wecom_local_media_part(
    value: Option<&serde_json::Value>,
    label: &str,
    keys: &[&str],
) -> Option<IncomingContentPart> {
    let value = value?;
    let path = extract_nonempty(value, &["local_path", "path"]).map(PathBuf::from)?;
    let file_id = extract_nonempty(value, &["url", "media_id"])
        .or_else(|| extract_nonempty(value, keys))
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let mime =
        extract_nonempty(value, &["mime", "mime_type", "content_type"]).map(ToOwned::to_owned);
    let name = extract_nonempty(value, &["filename", "file_name", "name"]).map(ToOwned::to_owned);
    match label {
        "image" => Some(IncomingContentPart::Image {
            file_id,
            path,
            mime,
        }),
        "video" => Some(IncomingContentPart::Video {
            file_id,
            path,
            mime,
            name,
        }),
        _ => Some(IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        }),
    }
}

fn wecom_media_placeholder(
    value: Option<&serde_json::Value>,
    label: &str,
    keys: &[&str],
) -> String {
    let detail = value.and_then(|value| extract_nonempty(value, keys));

    match detail {
        Some(detail) => format!("[{label}: {detail}]"),
        None => format!("[{label}: missing media]"),
    }
}

fn extract_nonempty<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    })
}

async fn download_wecom_inbound_media(
    media_dir: &str,
    native: &mut serde_json::Value,
) -> Result<()> {
    let Some(body) = native.get_mut("body") else {
        return Ok(());
    };
    let msgtype = body
        .get("msgtype")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    download_wecom_body_media(media_dir, &msgtype, body).await
}

async fn download_wecom_body_media(
    media_dir: &str,
    msgtype: &str,
    body: &mut serde_json::Value,
) -> Result<()> {
    match msgtype {
        "image" | "file" | "video" => {
            if let Some(value) = body.get_mut(msgtype) {
                download_wecom_media_object(media_dir, msgtype, value).await?;
            }
        }
        "mixed" => {
            if let Some(items) = body
                .get_mut("mixed")
                .and_then(|value| value.get_mut("msg_item"))
                .and_then(|value| value.as_array_mut())
            {
                for item in items {
                    let kind = item
                        .get("msgtype")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string();
                    if matches!(kind.as_str(), "image" | "file" | "video") {
                        if let Some(value) = item.get_mut(&kind) {
                            download_wecom_media_object(media_dir, &kind, value).await?;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

async fn download_wecom_media_object(
    media_dir: &str,
    kind: &str,
    value: &mut serde_json::Value,
) -> Result<()> {
    if extract_nonempty(value, &["local_path", "path"]).is_some() {
        return Ok(());
    }
    let Some(url) = extract_nonempty(value, &["url"]).map(ToOwned::to_owned) else {
        return Ok(());
    };
    let aes_key = extract_nonempty(value, &["aeskey", "aes_key", "aesKey"])
        .unwrap_or("")
        .to_string();
    let Some((bytes, mime)) = wecom_download_url(&url, &aes_key).await? else {
        return Ok(());
    };
    let filename = wecom_download_filename(value, kind, mime.as_deref());
    let dir = wecom_media_dir(media_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("create wecom media dir {}: {err}", dir.display()),
        })?;
    let final_path = dir.join(format!("{}_{}", uuid::Uuid::new_v4(), filename));
    atomic_write_file(&final_path, &bytes).await?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "local_path".to_string(),
            serde_json::Value::String(final_path.to_string_lossy().to_string()),
        );
        if let Some(mime) = mime {
            obj.insert("mime".to_string(), serde_json::Value::String(mime));
        }
        obj.entry("filename")
            .or_insert_with(|| serde_json::Value::String(filename));
    }
    Ok(())
}

async fn wecom_download_url(url: &str, aes_key: &str) -> Result<Option<(Vec<u8>, Option<String>)>> {
    let resp = match default_http_client()
        .get(url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%url, error = %err, "wecom media download failed");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(%url, status = %resp.status(), "wecom media download non-success");
        return Ok(None);
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_INBOUND_MEDIA_SIZE {
            tracing::warn!(%url, len, "wecom media too large");
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
            message: format!("wecom media download bytes failed: {err}"),
        })?;
    if bytes.len() as u64 > MAX_INBOUND_MEDIA_SIZE {
        tracing::warn!(%url, len = bytes.len(), "wecom media too large");
        return Ok(None);
    }
    let bytes = if aes_key.trim().is_empty() {
        bytes.to_vec()
    } else {
        match aes_ecb_decrypt_pkcs7(&bytes, aes_key) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(%url, error = %err, "wecom media decrypt failed");
                return Ok(None);
            }
        }
    };
    Ok(Some((bytes, mime)))
}

fn aes_ecb_decrypt_pkcs7(data: &[u8], key_hint: &str) -> std::result::Result<Vec<u8>, String> {
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err("ciphertext length must be a non-empty multiple of 16".into());
    }
    let key = parse_aes_key(key_hint)?;
    let mut out = data.to_vec();
    match key.len() {
        16 => {
            let cipher = Aes128::new_from_slice(&key).map_err(|err| err.to_string())?;
            for block in out.chunks_exact_mut(16) {
                cipher.decrypt_block(aes::cipher::generic_array::GenericArray::from_mut_slice(
                    block,
                ));
            }
        }
        24 => {
            let cipher = Aes192::new_from_slice(&key).map_err(|err| err.to_string())?;
            for block in out.chunks_exact_mut(16) {
                cipher.decrypt_block(aes::cipher::generic_array::GenericArray::from_mut_slice(
                    block,
                ));
            }
        }
        32 => {
            let cipher = Aes256::new_from_slice(&key).map_err(|err| err.to_string())?;
            for block in out.chunks_exact_mut(16) {
                cipher.decrypt_block(aes::cipher::generic_array::GenericArray::from_mut_slice(
                    block,
                ));
            }
        }
        len => return Err(format!("invalid AES key length: {len}")),
    }
    unpad_pkcs7(&out)
}

fn parse_aes_key(raw: &str) -> std::result::Result<Vec<u8>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("missing AES key".into());
    }
    if matches!(raw.len(), 32 | 48 | 64) && raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return decode_hex(raw);
    }
    let padded = {
        let mut value = raw.to_string();
        while !value.len().is_multiple_of(4) {
            value.push('=');
        }
        value
    };
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(padded.as_bytes())
        .unwrap_or_else(|_| raw.as_bytes().to_vec());
    if decoded.len() == 32 && decoded.iter().all(u8::is_ascii_hexdigit) {
        let hex = std::str::from_utf8(&decoded).map_err(|err| err.to_string())?;
        return decode_hex(hex);
    }
    Ok(decoded)
}

fn decode_hex(raw: &str) -> std::result::Result<Vec<u8>, String> {
    if !raw.len().is_multiple_of(2) {
        return Err("hex key length must be even".into());
    }
    let mut out = Vec::with_capacity(raw.len() / 2);
    for idx in (0..raw.len()).step_by(2) {
        let byte = u8::from_str_radix(&raw[idx..idx + 2], 16).map_err(|err| err.to_string())?;
        out.push(byte);
    }
    Ok(out)
}

fn unpad_pkcs7(data: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let Some(&pad) = data.last() else {
        return Err("empty plaintext".into());
    };
    let pad = pad as usize;
    if pad == 0 || pad > 16 || pad > data.len() {
        return Err("invalid PKCS7 padding".into());
    }
    if !data[data.len() - pad..]
        .iter()
        .all(|byte| *byte as usize == pad)
    {
        return Err("invalid PKCS7 padding bytes".into());
    }
    Ok(data[..data.len() - pad].to_vec())
}

fn wecom_media_dir(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    if media_dir.is_empty() {
        std::env::temp_dir().join("rua-wecom-media")
    } else {
        PathBuf::from(media_dir)
    }
}

fn wecom_download_filename(value: &serde_json::Value, kind: &str, mime: Option<&str>) -> String {
    let hint = extract_nonempty(value, &["filename", "file_name", "name"])
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            let ext = match mime.unwrap_or("") {
                "image/jpeg" => "jpg",
                "image/png" => "png",
                "application/pdf" => "pdf",
                "video/mp4" => "mp4",
                _ => match kind {
                    "image" => "jpg",
                    "video" => "mp4",
                    _ => "bin",
                },
            };
            format!("{kind}.{ext}")
        });
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
            .unwrap_or_else(|| "wecom-media".into()),
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

#[async_trait]
impl channel::Channel for WecomChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("wecom channel already started");
            return Ok(());
        }
        *self.process.write().await = Some(process.clone());
        *started = true;

        if self.bot_id.is_empty() || self.secret.is_empty() {
            tracing::warn!("wecom channel: bot_id or secret empty, WS disabled");
            return Ok(());
        }

        let mut config = WecomBotConfig::new(self.bot_id.clone(), self.secret.clone());
        config.ws_url = self.ws_url.clone();
        let process_for_ws = process;
        let debouncer = self.debouncer.clone();
        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::Markdown,
            self.render_options.clone(),
        ));
        let acl_metadata = Arc::new(self.acl_metadata.clone());
        let approval_resolver = self.approval_resolver.clone();
        let event_deduper = self.event_deduper.clone();
        let media_dir = self.media_dir.clone();
        let bot_id = self.bot_id.clone();
        let (ws_sender, outbound_rx) = client::create_ws_sender();
        *self.ws_sender.write().await = Some(ws_sender.clone());
        let ws_sender_for_send = ws_sender.clone();
        let send_fn: pipeline::SendFn = Arc::new(move |to, msg| {
            let ws_sender = ws_sender_for_send.clone();
            Box::pin(async move { Self::send_outgoing_with_sender(&ws_sender, to, msg).await })
        });

        let on_message: client::OnMessageFn = Arc::new(move |frame| {
            let cmd = frame.cmd.as_deref().unwrap_or("");
            if cmd == super::types::cmd::EVENT_CALLBACK && !is_template_card_event_frame(&frame) {
                tracing::info!("wecom: event callback received (enter_chat etc)");
                return;
            }

            let p = process_for_ws.clone();
            let d = debouncer.clone();
            let r = renderer.clone();
            let sf = send_fn.clone();
            let acl = acl_metadata.clone();
            let approvals = approval_resolver.clone();
            let event_deduper = event_deduper.clone();
            let media_dir = media_dir.clone();
            let stream_sender = ws_sender.clone();
            let bot_id = bot_id.clone();

            tokio::spawn(async move {
                let mut native = match serde_json::to_value(&frame) {
                    Ok(native) => native,
                    Err(err) => {
                        tracing::warn!(error = %err, "wecom: callback frame JSON conversion failed");
                        return;
                    }
                };
                if let Err(err) = download_wecom_inbound_media(&media_dir, &mut native).await {
                    tracing::warn!(error = %err, "wecom inbound media download failed; using placeholders");
                }
                let frame = match serde_json::from_value::<WsFrame>(native) {
                    Ok(frame) => frame,
                    Err(err) => {
                        tracing::warn!(error = %err, "wecom: callback frame JSON conversion failed after media download");
                        return;
                    }
                };
                let mut req =
                    match parse_callback_frame_with_resolver(&frame, &approvals, Some(&bot_id)) {
                        Ok(Some(req)) => req,
                        Ok(None) => {
                            tracing::debug!("wecom: callback frame has no parseable content");
                            return;
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "wecom approval callback rejected");
                            return;
                        }
                    };
                match wecom_check_duplicate_request(&event_deduper, &req) {
                    Ok(true) => {
                        tracing::debug!(
                            message_id = ?req.message_id,
                            "wecom duplicate event skipped"
                        );
                        return;
                    }
                    Ok(false) => {}
                    Err(err) => {
                        tracing::warn!(error = %err, "wecom event dedup failed");
                    }
                }
                update_wecom_resolved_approval_card(&stream_sender, &frame, &req).await;
                if let (Some(acl_obj), Some(meta)) = (acl.as_object(), req.metadata.as_object_mut())
                {
                    for (k, v) in acl_obj {
                        meta.insert(k.clone(), v.clone());
                    }
                }
                let processing_stream_id = processing_stream_id(&frame);
                let processing_consumed = Arc::new(AtomicBool::new(false));
                let stream_handler = Arc::new(WecomStreamHandler::with_stream_id(
                    stream_sender.clone(),
                    frame.clone(),
                    PlatformStreamPolicy::reply_stream(STREAM_MIN_INTERVAL, STREAM_MAX_CHARS),
                    processing_stream_id.clone(),
                    Some(processing_consumed.clone()),
                ));
                let processing_indicator = Arc::new(WecomProcessingIndicator::new(
                    stream_sender,
                    frame,
                    processing_stream_id,
                    processing_consumed,
                ));
                let sender_key = req.user_id.clone();
                let result = pipeline::run_inbound_streaming_with_indicator(
                    p,
                    req,
                    Some((d, sender_key)),
                    r,
                    sf,
                    Some(stream_handler),
                    Some(processing_indicator),
                )
                .await;
                if let Err(e) = result {
                    tracing::error!("wecom pipeline error: {e}");
                }
            });
        });

        self.ws_stop_tx.send_replace(false);
        let handle = client::spawn_ws_task_with_options(
            config,
            on_message,
            self.ws_stop_tx.subscribe(),
            outbound_rx,
            self.acl_metadata["max_reconnect_attempts"]
                .as_i64()
                .unwrap_or(-1),
            self.acl_metadata["welcome_text"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        *self.ws_handle.write().await = Some(handle);

        tracing::info!(
            "wecom aibot channel started (bot_id={})",
            &self.bot_id[..self.bot_id.len().min(12)]
        );
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut started = self.started.write().await;
        if !*started {
            return Ok(());
        }

        let _ = self.ws_stop_tx.send(true);
        if let Some(handle) = self.ws_handle.write().await.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }

        if let Ok(mut deduper) = self.event_deduper.lock() {
            deduper.clear();
        }
        *self.ws_sender.write().await = None;
        *self.process.write().await = None;
        *started = false;
        tracing::info!("wecom aibot channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        let Some(sender) = self.ws_sender.read().await.clone() else {
            let rendered = Self::render_content(&msg.content);
            if rendered.is_empty() {
                return Ok(());
            }
            return Err(rua_core::RuaError::Runtime {
                message: "wecom send requires an active WebSocket sender".into(),
            });
        };
        Self::send_outgoing_with_sender(&sender, to, msg).await
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        let frame: WsFrame =
            serde_json::from_value(native).map_err(|e| rua_core::RuaError::Validation {
                message: format!("invalid wecom WS frame: {e}"),
            })?;
        let req = parse_callback_frame_with_resolver(
            &frame,
            &self.approval_resolver,
            Some(&self.bot_id),
        )?
        .ok_or_else(|| rua_core::RuaError::Validation {
            message: "wecom callback frame has no parseable content".into(),
        })?;
        if wecom_check_duplicate_request(&self.event_deduper, &req)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate wecom event".into(),
            });
        }
        Ok(req)
    }
}
