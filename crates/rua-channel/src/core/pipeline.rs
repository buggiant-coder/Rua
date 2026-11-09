use std::sync::Arc;

use futures::future::BoxFuture;

use crate::core::acl_store::{acl_message, AccessControlStore};
use crate::core::allowlist::ChannelAcl;
use crate::core::channel::Process;
use crate::core::config::BaseChannelConfig;
use crate::core::content::{
    split_text, AgentChannelRequest, ChannelTarget, IncomingContentPart, OutgoingContentPart,
    OutgoingMessage,
};
use crate::core::debouncer::{content_allows_immediate, BufferDebouncer};
use crate::core::media;
use crate::core::mention;
use crate::core::renderer::MessageRenderer;
use crate::core::streaming::{collect_reply_with_handler, ProcessingIndicator, StreamHandler};
use crate::http_client::default_http_client;
use rua_core::Result;

pub type SendFn =
    Arc<dyn Fn(ChannelTarget, OutgoingMessage) -> BoxFuture<'static, Result<()>> + Send + Sync>;

const SPLIT_MAX_LEN: usize = 3000;

fn prefix_text(prefix: &str, text: &str) -> String {
    if prefix.is_empty() || text.is_empty() {
        text.to_string()
    } else {
        format!("{prefix}  {text}")
    }
}

struct PrefixedStreamHandler {
    inner: Arc<dyn StreamHandler>,
    prefix: String,
}

#[async_trait::async_trait]
impl StreamHandler for PrefixedStreamHandler {
    async fn on_start(&self, kind: crate::core::streaming::StreamType) -> Result<()> {
        self.inner.on_start(kind).await
    }

    async fn on_delta(&self, kind: crate::core::streaming::StreamType, text: &str) -> Result<()> {
        let text = if kind == crate::core::streaming::StreamType::Message {
            prefix_text(&self.prefix, text)
        } else {
            text.to_string()
        };
        self.inner.on_delta(kind, &text).await
    }

    async fn on_end(&self, kind: crate::core::streaming::StreamType, text: &str) -> Result<()> {
        let text = if kind == crate::core::streaming::StreamType::Message {
            prefix_text(&self.prefix, text)
        } else {
            text.to_string()
        };
        self.inner.on_end(kind, &text).await
    }
}

struct ProcessingIndicatorGuard {
    indicator: Option<Arc<dyn ProcessingIndicator>>,
    req: AgentChannelRequest,
}

impl ProcessingIndicatorGuard {
    fn new(indicator: Arc<dyn ProcessingIndicator>, req: AgentChannelRequest) -> Self {
        Self {
            indicator: Some(indicator),
            req,
        }
    }

    async fn stop(mut self) {
        if let Some(indicator) = self.indicator.take() {
            if let Err(err) = indicator.stop(&self.req).await {
                tracing::warn!(error = %err, "processing indicator stop failed");
            }
        }
    }
}

impl Drop for ProcessingIndicatorGuard {
    fn drop(&mut self) {
        let Some(indicator) = self.indicator.take() else {
            return;
        };
        let req = self.req.clone();
        tokio::spawn(async move {
            if let Err(err) = indicator.stop(&req).await {
                tracing::warn!(error = %err, "processing indicator cancellation cleanup failed");
            }
        });
    }
}

pub fn merge_requests(batch: Vec<AgentChannelRequest>) -> AgentChannelRequest {
    let mut iter = batch.into_iter();
    let mut merged = iter.next().expect("batch is never empty");
    for req in iter {
        let mentioned = metadata_bool(&merged.metadata, "mentioned")
            || metadata_bool(&merged.metadata, "bot_mentioned")
            || metadata_bool(&req.metadata, "mentioned")
            || metadata_bool(&req.metadata, "bot_mentioned");
        let has_bot_command = metadata_bool(&merged.metadata, "has_bot_command")
            || metadata_bool(&req.metadata, "has_bot_command");
        merged.input.extend(req.input);
        merged.metadata = req.metadata;
        set_metadata_bool(&mut merged.metadata, "mentioned", mentioned);
        set_metadata_bool(&mut merged.metadata, "has_bot_command", has_bot_command);
        merged.message_id = req.message_id;
    }
    merged
}

fn metadata_bool(metadata: &serde_json::Value, key: &str) -> bool {
    metadata.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn set_metadata_bool(metadata: &mut serde_json::Value, key: &str, value: bool) {
    if value {
        metadata[key] = serde_json::Value::Bool(true);
    }
}

pub async fn run_inbound(
    process: Arc<dyn Process>,
    req: AgentChannelRequest,
    debouncer: Option<(BufferDebouncer<String, AgentChannelRequest>, String)>,
    renderer: Arc<MessageRenderer>,
    send_fn: SendFn,
) -> Result<()> {
    run_inbound_streaming(process, req, debouncer, renderer, send_fn, None).await
}

pub async fn run_inbound_streaming(
    process: Arc<dyn Process>,
    req: AgentChannelRequest,
    debouncer: Option<(BufferDebouncer<String, AgentChannelRequest>, String)>,
    renderer: Arc<MessageRenderer>,
    send_fn: SendFn,
    handler: Option<Arc<dyn StreamHandler>>,
) -> Result<()> {
    run_inbound_streaming_with_indicator(process, req, debouncer, renderer, send_fn, handler, None)
        .await
}

pub async fn run_inbound_streaming_with_indicator(
    process: Arc<dyn Process>,
    req: AgentChannelRequest,
    debouncer: Option<(BufferDebouncer<String, AgentChannelRequest>, String)>,
    renderer: Arc<MessageRenderer>,
    send_fn: SendFn,
    handler: Option<Arc<dyn StreamHandler>>,
    indicator: Option<Arc<dyn ProcessingIndicator>>,
) -> Result<()> {
    run_inbound_acl_with_indicator(
        process, req, debouncer, renderer, send_fn, handler, None, indicator,
    )
    .await
}

pub async fn run_inbound_acl(
    process: Arc<dyn Process>,
    req: AgentChannelRequest,
    debouncer: Option<(BufferDebouncer<String, AgentChannelRequest>, String)>,
    renderer: Arc<MessageRenderer>,
    send_fn: SendFn,
    handler: Option<Arc<dyn StreamHandler>>,
    acl_store: Option<Arc<AccessControlStore>>,
) -> Result<()> {
    run_inbound_acl_with_indicator(
        process, req, debouncer, renderer, send_fn, handler, acl_store, None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn run_inbound_acl_with_indicator(
    process: Arc<dyn Process>,
    req: AgentChannelRequest,
    debouncer: Option<(BufferDebouncer<String, AgentChannelRequest>, String)>,
    renderer: Arc<MessageRenderer>,
    send_fn: SendFn,
    handler: Option<Arc<dyn StreamHandler>>,
    acl_store: Option<Arc<AccessControlStore>>,
    indicator: Option<Arc<dyn ProcessingIndicator>>,
) -> Result<()> {
    let config = &req.metadata;
    let handler = if config
        .get("streaming_enabled")
        .and_then(serde_json::Value::as_bool)
        == Some(false)
    {
        None
    } else {
        handler
    };
    let indicator = if config
        .get("show_typing")
        .and_then(serde_json::Value::as_bool)
        == Some(false)
    {
        None
    } else {
        indicator
    };
    let mut options = renderer.options.clone();
    options.apply_metadata(config);
    let renderer = Arc::new(MessageRenderer::with_options(renderer.style, options));
    let prefix = config
        .get("bot_prefix")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let handler = handler.map(|inner| {
        Arc::new(PrefixedStreamHandler {
            inner,
            prefix: prefix.clone(),
        }) as Arc<dyn StreamHandler>
    });
    let mut req = req;
    let has_thread = [
        "thread_ts",
        "feishu_thread_id",
        "message_thread_id",
        "matrix_thread_root_event_id",
        "root_id",
    ]
    .iter()
    .any(|key| {
        req.metadata.get(key).is_some_and(|value| {
            value.as_str().is_some_and(|value| !value.is_empty()) || value.is_number()
        })
    });
    if mention::is_group_message(&req) && !has_thread {
        if let Some(shared) = req.metadata["share_session_in_group"].as_bool() {
            req.session_id = if shared {
                format!("{}:group:{}", req.channel_id, req.conversation_id)
            } else {
                format!(
                    "{}:group:{}:{}",
                    req.channel_id, req.conversation_id, req.user_id
                )
            };
        }
    }
    let req = if let Some((debouncer, key)) = debouncer {
        if content_allows_immediate(&req.input) {
            tracing::debug!("pure media/audio input — bypassing debounce window");
            req
        } else {
            let key = format!(
                "{}:{}:{}:{key}",
                req.agent_id, req.channel_id, req.session_id
            );
            match debouncer.push(key, req).await {
                Some(batch) => merge_requests(batch),
                None => return Ok(()),
            }
        }
    } else {
        req
    };

    let gate_cfg: BaseChannelConfig =
        serde_json::from_value(req.metadata.clone()).unwrap_or_default();
    let is_group = mention::is_group_message(&req);

    let disabled_key = if is_group {
        "group_disabled"
    } else {
        "dm_disabled"
    };
    if req
        .metadata
        .get(disabled_key)
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return Ok(());
    }

    let acl_sender = acl_sender_id(&req);
    let acl = ChannelAcl::from_config(&gate_cfg);
    if !acl.is_allowed(acl_sender, is_group) {
        tracing::debug!(
            user_id = %req.user_id,
            acl_sender_id = %acl_sender,
            is_group,
            "inbound request blocked by ACL policy"
        );
        if let Some(message) = req
            .metadata
            .get("deny_message")
            .and_then(serde_json::Value::as_str)
            .filter(|message| !message.is_empty())
        {
            send_deny(&send_fn, &req, message.to_string()).await?;
        }
        return Ok(());
    }

    if let Some(store) = acl_store.as_ref() {
        let channel = &req.channel_id;
        let sender = acl_sender;
        if store.is_blacklisted(channel, sender) {
            tracing::info!(user_id = %sender, channel = %channel, "inbound blocked: blacklisted");
            let text = acl_message("blocked", &gate_cfg.language, sender);
            send_deny(&send_fn, &req, text).await?;
            return Ok(());
        }
        let ac_enabled = if is_group {
            gate_cfg.access_control_group
        } else {
            gate_cfg.access_control_dm
        };
        if ac_enabled && !store.is_whitelisted(channel, sender) {
            let first_message = first_input_text(&req.input);
            let username = req
                .metadata
                .get("user_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            store.record_pending(channel, sender, &first_message, username);
            tracing::info!(user_id = %sender, channel = %channel, "inbound pending approval");
            let text = acl_message("pending", &gate_cfg.language, sender);
            send_deny(&send_fn, &req, text).await?;
            return Ok(());
        }
    }

    if !mention::should_respond(&req, &gate_cfg) {
        tracing::debug!(
            user_id = %req.user_id,
            "group message without mention, discarding"
        );
        return Ok(());
    }

    let target = ChannelTarget {
        agent_id: req.agent_id.clone(),
        conversation_id: req.conversation_id.clone(),
        sender_id: Some(req.user_id.clone()),
        reply_to: req.message_id.clone(),
        metadata: req.metadata.clone(),
    };
    let channel_id = req.channel_id.clone();

    let indicator_guard = if let Some(indicator) = indicator.as_ref() {
        indicator.start(&req).await?;
        Some(ProcessingIndicatorGuard::new(
            indicator.clone(),
            req.clone(),
        ))
    } else {
        None
    };

    let result = async {
        let stream = process.handle(req.clone()).await;
        let outcome = collect_reply_with_handler(stream, &renderer, handler.as_deref()).await;
        let parts = outcome.parts;

        if parts.is_empty() {
            return Ok(());
        }

        let mut thinking_buf = String::new();
        let mut tool_texts: Vec<String> = Vec::new();
        let mut text_buf = String::new();
        let mut non_text_parts: Vec<OutgoingContentPart> = Vec::new();

        for part in parts {
            match part {
                OutgoingContentPart::Text { text } | OutgoingContentPart::Markdown { text } => {
                    text_buf.push_str(&text);
                }
                OutgoingContentPart::Image { .. }
                | OutgoingContentPart::File { .. }
                | OutgoingContentPart::Audio { .. }
                | OutgoingContentPart::Video { .. } => non_text_parts.push(part),
                OutgoingContentPart::Tool {
                    name,
                    status,
                    input,
                    output,
                } => {
                    let mut s = format!("[Tool: {name} - {status:?}]");
                    if renderer.options.show_tool_details {
                        if let Some(inp) = input {
                            s.push_str(&format!("\nInput: {inp}"));
                        }
                        if let Some(out) = output {
                            s.push_str(&format!("\nOutput: {out}"));
                        }
                    }
                    tool_texts.push(s);
                }
                OutgoingContentPart::Thinking { text } => {
                    thinking_buf.push_str(&text);
                }
                OutgoingContentPart::Refusal { text } => {
                    text_buf.push_str(&text);
                }
                OutgoingContentPart::Raw { value } => {
                    text_buf.push_str(&value.to_string());
                }
            }
        }

        if !thinking_buf.is_empty() && !outcome.message_streamed {
            let prefixed = format!("💭 {thinking_buf}");
            let chunks = split_text(&prefixed, SPLIT_MAX_LEN);
            for chunk in chunks {
                let msg = OutgoingMessage {
                    channel_id: channel_id.clone(),
                    conversation_id: target.conversation_id.clone(),
                    receiver_id: target.sender_id.clone(),
                    content: vec![OutgoingContentPart::Markdown { text: chunk }],
                    reply_to: target.reply_to.clone(),
                    metadata: target.metadata.clone(),
                };
                send_fn(target.clone(), msg).await?;
            }
        }

        if !tool_texts.is_empty() {
            for tool_text in &tool_texts {
                let msg = OutgoingMessage {
                    channel_id: channel_id.clone(),
                    conversation_id: target.conversation_id.clone(),
                    receiver_id: target.sender_id.clone(),
                    content: vec![OutgoingContentPart::Markdown {
                        text: tool_text.clone(),
                    }],
                    reply_to: target.reply_to.clone(),
                    metadata: target.metadata.clone(),
                };
                send_fn(target.clone(), msg).await?;
            }
        }

        if !text_buf.is_empty() && !outcome.message_streamed {
            let text_buf = prefix_text(&prefix, &text_buf);
            let chunks = split_text(&text_buf, SPLIT_MAX_LEN);
            for chunk in chunks {
                let msg = OutgoingMessage {
                    channel_id: channel_id.clone(),
                    conversation_id: target.conversation_id.clone(),
                    receiver_id: target.sender_id.clone(),
                    content: vec![OutgoingContentPart::Markdown { text: chunk }],
                    reply_to: target.reply_to.clone(),
                    metadata: target.metadata.clone(),
                };
                send_fn(target.clone(), msg).await?;
            }
        }

        if !non_text_parts.is_empty() {
            let http = default_http_client();
            let mut resolved_parts = Vec::with_capacity(non_text_parts.len());
            for part in non_text_parts {
                match media::resolve_outgoing_media(&part, &http).await {
                    Ok(Some(resolved)) => {
                        let new_part = match &part {
                            OutgoingContentPart::Image { alt, .. } => OutgoingContentPart::Image {
                                url: None,
                                path: Some(resolved.path),
                                alt: alt.clone(),
                            },
                            OutgoingContentPart::File { .. } => OutgoingContentPart::File {
                                url: None,
                                path: Some(resolved.path),
                                name: Some(resolved.name),
                            },
                            OutgoingContentPart::Audio { .. } => OutgoingContentPart::Audio {
                                url: None,
                                path: Some(resolved.path),
                                name: Some(resolved.name),
                            },
                            OutgoingContentPart::Video { .. } => OutgoingContentPart::Video {
                                url: None,
                                path: Some(resolved.path),
                                name: Some(resolved.name),
                            },
                            _ => part,
                        };
                        resolved_parts.push(new_part);
                    }
                    Ok(None) => resolved_parts.push(part),
                    Err(e) => {
                        tracing::warn!(error = %e, "media resolve failed, sending part as-is");
                        resolved_parts.push(part);
                    }
                }
            }

            let msg = OutgoingMessage {
                channel_id,
                conversation_id: target.conversation_id.clone(),
                receiver_id: target.sender_id.clone(),
                content: resolved_parts,
                reply_to: target.reply_to.clone(),
                metadata: target.metadata.clone(),
            };
            send_fn(target, msg).await?;
        }

        Ok(())
    }
    .await;

    if let Some(guard) = indicator_guard {
        guard.stop().await;
    }

    result
}

fn acl_sender_id(req: &AgentChannelRequest) -> &str {
    const KEYS: &[&str] = &[
        "acl_sender_id",
        "sender_id",
        "feishu_sender_id",
        "wecom_sender_id",
        "qq_sender_id",
        "telegram_user_id",
    ];
    KEYS.iter()
        .find_map(|key| {
            req.metadata
                .get(*key)
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or(req.user_id.as_str())
}

async fn send_deny(send_fn: &SendFn, req: &AgentChannelRequest, text: String) -> Result<()> {
    let target = ChannelTarget {
        agent_id: req.agent_id.clone(),
        conversation_id: req.conversation_id.clone(),
        sender_id: Some(req.user_id.clone()),
        reply_to: req.message_id.clone(),
        metadata: req.metadata.clone(),
    };
    let msg = OutgoingMessage {
        channel_id: req.channel_id.clone(),
        conversation_id: req.conversation_id.clone(),
        receiver_id: Some(req.user_id.clone()),
        content: vec![OutgoingContentPart::Text { text }],
        reply_to: req.message_id.clone(),
        metadata: req.metadata.clone(),
    };
    send_fn(target, msg).await
}

fn first_input_text(parts: &[IncomingContentPart]) -> String {
    parts
        .iter()
        .find_map(|p| match p {
            IncomingContentPart::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}
