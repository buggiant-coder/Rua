use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;

use crate::core::approval_card::{
    ApprovalCardCallback, ApprovalCardResolveError, ApprovalCardResolver,
};
use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::content::{
    render_parts_to_string, AgentChannelRequest, IncomingContentPart, OutgoingContentPart,
};
use crate::core::event_dedup::{EventDedupDecision, EventDeduper};
use crate::core::pipeline;
use crate::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::{BufferDebouncer, ChannelId};
use crate::http_client::default_http_client;
use crate::qq::api::{self, QqSender, QqTokenManager, SendBodyParams, SendMediaParams, SendParams};
use crate::qq::approval_card;
use crate::qq::gateway;
use crate::qq::types::get_event_spec;
use rua_core::Result;

const DEBOUNCE_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_INBOUND_MEDIA_SIZE: u64 = 50 * 1024 * 1024;
const APPROVAL_CALLBACK_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const EVENT_DEDUP_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

pub struct QqChannel {
    channel_id: ChannelId,
    app_id: String,
    sandbox: bool,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    ws_stop_tx: watch::Sender<bool>,
    ws_handle: RwLock<Option<JoinHandle<()>>>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    debouncer: BufferDebouncer<String, AgentRequest>,
    token_manager: Arc<QqTokenManager>,
    sender: Arc<QqSender>,
    media_dir: String,
    approval_resolver: Arc<Mutex<ApprovalCardResolver>>,
    event_deduper: Arc<Mutex<EventDeduper>>,
}

impl QqChannel {
    pub fn new(app_id: String, client_secret: String, sandbox: bool) -> Self {
        let (ws_stop_tx, _) = watch::channel(false);
        let render_options = MessageRenderOptions::default();
        let token_manager = Arc::new(QqTokenManager::new(app_id.clone(), client_secret.clone()));
        let sender = api::new_sender(sandbox);

        Self {
            channel_id: "qq".into(),
            app_id,
            sandbox,
            process: RwLock::new(None),
            started: RwLock::new(false),
            ws_stop_tx,
            ws_handle: RwLock::new(None),
            render_options,
            acl_metadata: serde_json::json!({}),
            debouncer: BufferDebouncer::new(DEBOUNCE_WINDOW),
            token_manager,
            sender,
            media_dir: String::new(),
            approval_resolver: Arc::new(Mutex::new(ApprovalCardResolver::new(
                APPROVAL_CALLBACK_TTL,
            ))),
            event_deduper: Arc::new(Mutex::new(EventDeduper::new(
                EVENT_DEDUP_TTL,
                EVENT_DEDUP_CAPACITY,
            ))),
        }
    }

    pub fn new_with_api_base_and_token_url(
        app_id: String,
        client_secret: String,
        sandbox: bool,
        api_base: String,
        token_url: String,
    ) -> Self {
        let (ws_stop_tx, _) = watch::channel(false);
        let render_options = MessageRenderOptions::default();
        let token_manager = Arc::new(QqTokenManager::new_with_token_url(
            app_id.clone(),
            client_secret,
            token_url,
        ));
        let sender = Arc::new(QqSender::new_with_api_base(api_base));

        Self {
            channel_id: "qq".into(),
            app_id,
            sandbox,
            process: RwLock::new(None),
            started: RwLock::new(false),
            ws_stop_tx,
            ws_handle: RwLock::new(None),
            render_options,
            acl_metadata: serde_json::json!({}),
            debouncer: BufferDebouncer::new(DEBOUNCE_WINDOW),
            token_manager,
            sender,
            media_dir: String::new(),
            approval_resolver: Arc::new(Mutex::new(ApprovalCardResolver::new(
                APPROVAL_CALLBACK_TTL,
            ))),
            event_deduper: Arc::new(Mutex::new(EventDeduper::new(
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
        download_qq_inbound_media_with(&self.media_dir, native).await
    }

    pub async fn handle_gateway_message(
        self: &Arc<Self>,
        process: Arc<dyn Process>,
        event_type: &str,
        mut native: serde_json::Value,
    ) -> Result<()> {
        let Some(spec) = get_event_spec(event_type) else {
            return Ok(());
        };
        native["message_type"] = serde_json::json!(spec.message_type);
        native["event_type"] = serde_json::json!(event_type);
        self.download_inbound_media(&mut native).await?;
        let mut req = channel::Channel::build_request_from_native(self.as_ref(), native)?;
        if req.user_id.is_empty() || req.input.is_empty() {
            return Ok(());
        }
        if let Some(options) = self.acl_metadata.as_object() {
            req.metadata
                .as_object_mut()
                .expect("QQ metadata")
                .extend(options.clone());
        }
        let indicator = Arc::new(QqAckIndicator {
            sender: self.sender.clone(),
            token: self.token_manager.clone(),
            text: self.acl_metadata["ack_message"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        });
        let sender = self.clone();
        let send_fn: pipeline::SendFn = Arc::new(move |to, msg| {
            let sender = sender.clone();
            Box::pin(async move { channel::Channel::send(sender.as_ref(), to, msg).await })
        });
        let sender_key = req.user_id.clone();
        pipeline::run_inbound_streaming_with_indicator(
            process,
            req,
            Some((self.debouncer.clone(), sender_key)),
            Arc::new(MessageRenderer::with_options(
                RenderStyle::Markdown,
                self.render_options.clone(),
            )),
            send_fn,
            None,
            Some(indicator),
        )
        .await
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

    fn media_file_type(part: &OutgoingContentPart) -> Option<u8> {
        match part {
            OutgoingContentPart::Image { .. } => Some(1),
            OutgoingContentPart::Video { .. } => Some(2),
            OutgoingContentPart::Audio { url, path, .. } => {
                let source = path
                    .as_deref()
                    .and_then(Path::to_str)
                    .or(url.as_deref())
                    .unwrap_or_default();
                if matches!(
                    Path::new(source).extension().and_then(|ext| ext.to_str()),
                    Some("amr" | "silk" | "slk")
                ) {
                    Some(3)
                } else {
                    Some(4)
                }
            }
            OutgoingContentPart::File { .. } => Some(4),
            _ => None,
        }
    }

    async fn send_media_part(
        &self,
        token: &str,
        route: &QqRoute<'_>,
        part: &OutgoingContentPart,
    ) -> Result<bool> {
        let Some(file_type) = Self::media_file_type(part) else {
            return Ok(false);
        };
        let (url, path, name) = match part {
            OutgoingContentPart::Image { url, path, alt } => {
                (url.as_deref(), path.as_deref(), alt.as_deref())
            }
            OutgoingContentPart::File { url, path, name }
            | OutgoingContentPart::Audio { url, path, name }
            | OutgoingContentPart::Video { url, path, name } => {
                (url.as_deref(), path.as_deref(), name.as_deref())
            }
            _ => (None, None, None),
        };
        let file_bytes = if url.is_none() {
            if let Some(path) = path {
                Some(
                    tokio::fs::read(path)
                        .await
                        .map_err(|err| rua_core::RuaError::Io {
                            message: format!("read qq outbound media {}: {err}", path.display()),
                        })?,
                )
            } else {
                None
            }
        } else {
            None
        };
        let file_name = name
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                path.and_then(|path| path.file_name())
                    .and_then(|name| name.to_str())
            })
            .or_else(|| {
                url.and_then(|url| {
                    url.split('?')
                        .next()
                        .and_then(|clean| clean.rsplit('/').next())
                        .filter(|name| !name.trim().is_empty())
                })
            });

        self.sender
            .send_media(SendMediaParams {
                token,
                message_type: route.message_type,
                sender_id: route.sender_id,
                channel_id: route.channel_id,
                group_openid: route.group_openid,
                guild_id: route.guild_id,
                file_type,
                url,
                file_data: file_bytes.as_deref(),
                file_name,
                msg_id: route.msg_id,
            })
            .await
            .map_err(|e| rua_core::RuaError::Io {
                message: format!("qq media send error: {e}"),
            })
    }
}

struct QqRoute<'a> {
    message_type: &'a str,
    sender_id: &'a str,
    channel_id: Option<&'a str>,
    group_openid: Option<&'a str>,
    guild_id: Option<&'a str>,
    msg_id: Option<&'a str>,
}

fn build_approval_message_body(
    to: &ToHandle,
    msg: &OutMessage,
    route: &QqRoute<'_>,
    text: &str,
) -> Option<serde_json::Value> {
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
    approval_card::build_approval_message_body(
        request_id,
        tool_name,
        severity,
        text,
        serde_json::json!({
            "sid": session_id,
            "sender": route.sender_id,
            "mt": route.message_type,
            "goid": route.group_openid.unwrap_or(""),
            "cid": route.channel_id.unwrap_or(""),
            "gid": route.guild_id.unwrap_or(""),
            "mid": route.msg_id.unwrap_or(""),
        }),
    )
    .ok()
}

async fn ack_qq_interaction_from_request(
    sender: &QqSender,
    token_mgr: &QqTokenManager,
    req: &AgentChannelRequest,
    code: i64,
) {
    let interaction_id = req
        .metadata
        .get("approval_interaction_id")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    ack_qq_interaction(sender, token_mgr, interaction_id, code).await;
}

async fn ack_qq_interaction(
    sender: &QqSender,
    token_mgr: &QqTokenManager,
    interaction_id: &str,
    code: i64,
) {
    if interaction_id.trim().is_empty() {
        return;
    }
    let token = match token_mgr.get_access_token().await {
        Ok(token) => token,
        Err(err) => {
            tracing::warn!(error = %err, "qq approval interaction ack token fetch failed");
            return;
        }
    };
    if let Err(err) = sender.ack_interaction(&token, interaction_id, code).await {
        tracing::warn!(error = %err, interaction_id, code, "qq approval interaction ack failed");
    }
}

async fn send_qq_resolved_approval_status(
    sender: &QqSender,
    token_mgr: &QqTokenManager,
    req: &AgentChannelRequest,
) {
    let metadata = &req.metadata;
    let tool_name = metadata
        .get("approval_tool_name")
        .and_then(|value| value.as_str())
        .unwrap_or("tool");
    let action = match metadata
        .get("approval_action")
        .and_then(|value| value.as_str())
        .unwrap_or("")
    {
        "approve" => crate::core::approval_card::ApprovalCardAction::Approve,
        "deny" => crate::core::approval_card::ApprovalCardAction::Deny,
        _ => return,
    };
    let operator_display = metadata
        .get("approval_operator_id")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .chars()
        .rev()
        .take(6)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    let text = approval_card::build_resolved_text(tool_name, action, &operator_display);
    let token = match token_mgr.get_access_token().await {
        Ok(token) => token,
        Err(err) => {
            tracing::warn!(error = %err, "qq approval resolved status token fetch failed");
            return;
        }
    };
    let message_type = metadata
        .get("message_type")
        .and_then(|value| value.as_str())
        .unwrap_or("c2c");
    let sender_id = metadata
        .get("sender_id")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let channel_id = metadata.get("channel_id").and_then(|value| value.as_str());
    let group_openid = metadata
        .get("group_openid")
        .and_then(|value| value.as_str());
    let guild_id = metadata.get("guild_id").and_then(|value| value.as_str());
    let msg_id = metadata.get("message_id").and_then(|value| value.as_str());
    if let Err(err) = sender
        .send_message(SendParams {
            token: &token,
            message_type,
            sender_id,
            channel_id,
            group_openid,
            guild_id,
            text: &text,
            msg_id,
        })
        .await
    {
        tracing::warn!(error = %err, "qq approval resolved status send failed");
    }
}

fn qq_check_duplicate_event(deduper: &Arc<Mutex<EventDeduper>>, msg_id: &str) -> Result<bool> {
    let msg_id = msg_id.trim();
    if msg_id.is_empty() {
        return Ok(false);
    }
    let mut deduper = deduper.lock().map_err(|_| rua_core::RuaError::Runtime {
        message: "qq event deduper lock poisoned".into(),
    })?;
    Ok(deduper.check_and_mark(format!("qq:message:{msg_id}")) == EventDedupDecision::Duplicate)
}

#[async_trait]
impl channel::Channel for QqChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("qq channel already started");
            return Ok(());
        }
        *self.process.write().await = Some(process.clone());
        *started = true;

        let process_for_ws = process;
        let debouncer = self.debouncer.clone();
        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::Markdown,
            self.render_options.clone(),
        ));
        let sender = self.sender.clone();
        let token_mgr = self.token_manager.clone();
        let approval_resolver = self.approval_resolver.clone();

        let send_fn: pipeline::SendFn =
            Arc::new({
                let sender = sender.clone();
                let token_mgr = token_mgr.clone();
                move |to, msg| {
                    let sender = sender.clone();
                    let token_mgr = token_mgr.clone();
                    Box::pin(async move {
                        let text = render_parts_to_string(&msg.content);
                        if text.is_empty() {
                            return Ok(());
                        }

                        let token = token_mgr.get_access_token().await.map_err(|e| {
                            rua_core::RuaError::Io {
                                message: format!("qq token error: {e}"),
                            }
                        })?;

                        let message_type = to
                            .metadata
                            .get("message_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("c2c");
                        let sender_id = to
                            .metadata
                            .get("sender_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let channel_id = to.metadata.get("channel_id").and_then(|v| v.as_str());
                        let group_openid = to.metadata.get("group_openid").and_then(|v| v.as_str());
                        let guild_id = to.metadata.get("guild_id").and_then(|v| v.as_str());
                        let msg_id = to.metadata.get("message_id").and_then(|v| v.as_str());
                        let route = QqRoute {
                            message_type,
                            sender_id,
                            channel_id,
                            group_openid,
                            guild_id,
                            msg_id,
                        };

                        if let Some(body) = build_approval_message_body(&to, &msg, &route, &text) {
                            match sender
                                .send_message_body(SendBodyParams {
                                    token: &token,
                                    message_type,
                                    sender_id,
                                    channel_id,
                                    group_openid,
                                    guild_id,
                                    body,
                                    msg_id,
                                })
                                .await
                            {
                                Ok(()) => return Ok(()),
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        "qq approval card send failed; falling back to text"
                                    );
                                }
                            }
                        }

                        sender
                            .send_message(SendParams {
                                token: &token,
                                message_type,
                                sender_id,
                                channel_id,
                                group_openid,
                                guild_id,
                                text: &text,
                                msg_id,
                            })
                            .await
                            .map_err(|e| rua_core::RuaError::Io {
                                message: format!("qq send error: {e}"),
                            })
                    })
                }
            });

        let acl_metadata = Arc::new(self.acl_metadata.clone());
        let interaction_sender = sender.clone();
        let interaction_token_mgr = token_mgr.clone();
        let mut worker = Self::new(self.app_id.clone(), String::new(), self.sandbox)
            .with_acl_config(self.acl_metadata.clone())
            .with_media_dir(self.media_dir.clone());
        worker.token_manager = self.token_manager.clone();
        worker.sender = self.sender.clone();
        worker.event_deduper = self.event_deduper.clone();
        worker.render_options = self.render_options.clone();
        let worker = Arc::new(worker);
        let on_message: gateway::OnMessageFn = Arc::new(move |event_type, data| {
            let p = process_for_ws.clone();
            let d = debouncer.clone();
            let r = renderer.clone();
            let sf = send_fn.clone();
            let acl = acl_metadata.clone();
            let approvals = approval_resolver.clone();
            let interaction_sender = interaction_sender.clone();
            let interaction_token_mgr = interaction_token_mgr.clone();
            let worker = worker.clone();
            tokio::spawn(async move {
                if event_type == "INTERACTION_CREATE" {
                    match resolve_approval_interaction(&data, &approvals) {
                        Ok(Some(mut req)) => {
                            ack_qq_interaction_from_request(
                                &interaction_sender,
                                &interaction_token_mgr,
                                &req,
                                0,
                            )
                            .await;
                            send_qq_resolved_approval_status(
                                &interaction_sender,
                                &interaction_token_mgr,
                                &req,
                            )
                            .await;
                            if let (Some(acl_obj), Some(meta)) =
                                (acl.as_object(), req.metadata.as_object_mut())
                            {
                                for (k, v) in acl_obj {
                                    meta.insert(k.clone(), v.clone());
                                }
                            }
                            let sender_key = req.user_id.clone();
                            let result =
                                pipeline::run_inbound(p, req, Some((d, sender_key)), r, sf).await;
                            if let Err(e) = result {
                                tracing::error!("qq approval interaction pipeline error: {e}");
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            if let Ok(parsed) = approval_card::parse_interaction_event(&data) {
                                ack_qq_interaction(
                                    &interaction_sender,
                                    &interaction_token_mgr,
                                    &parsed.interaction_id,
                                    3,
                                )
                                .await;
                            }
                            tracing::warn!(error = %err, "qq approval interaction rejected");
                        }
                    }
                    return;
                }

                if let Err(error) = worker.handle_gateway_message(p, &event_type, data).await {
                    tracing::debug!(%error, "QQ event skipped or failed");
                }
            });
        });

        let stop_rx = self.ws_stop_tx.subscribe();
        self.ws_stop_tx.send_replace(false);
        let handle = gateway::spawn_ws_task_with_limit(
            self.token_manager.clone(),
            self.sandbox,
            on_message,
            stop_rx,
            self.acl_metadata["max_reconnect_attempts"]
                .as_i64()
                .unwrap_or(100),
        );
        *self.ws_handle.write().await = Some(handle);

        tracing::info!("qq channel started (app_id={})", self.app_id);
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
        *self.process.write().await = None;
        *started = false;
        tracing::info!("qq channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        let text = Self::render_text_and_unresolved_media(&msg.content);
        if text.is_empty() && !msg.content.iter().any(Self::is_media_part) {
            return Ok(());
        }

        let token =
            self.token_manager
                .get_access_token()
                .await
                .map_err(|e| rua_core::RuaError::Io {
                    message: format!("qq token error: {e}"),
                })?;

        let message_type = to
            .metadata
            .get("message_type")
            .and_then(|v| v.as_str())
            .unwrap_or("c2c");
        let sender_id = to
            .metadata
            .get("sender_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let channel_id = to.metadata.get("channel_id").and_then(|v| v.as_str());
        let group_openid = to.metadata.get("group_openid").and_then(|v| v.as_str());
        let guild_id = to.metadata.get("guild_id").and_then(|v| v.as_str());
        let msg_id = to.metadata.get("message_id").and_then(|v| v.as_str());
        let route = QqRoute {
            message_type,
            sender_id,
            channel_id,
            group_openid,
            guild_id,
            msg_id,
        };

        if !text.is_empty() {
            if let Some(body) = build_approval_message_body(&to, &msg, &route, &text) {
                match self
                    .sender
                    .send_message_body(SendBodyParams {
                        token: &token,
                        message_type: route.message_type,
                        sender_id: route.sender_id,
                        channel_id: route.channel_id,
                        group_openid: route.group_openid,
                        guild_id: route.guild_id,
                        body,
                        msg_id: route.msg_id,
                    })
                    .await
                {
                    Ok(()) => return Ok(()),
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "qq approval card send failed; falling back to text"
                        );
                    }
                }
            }
            self.sender
                .send_message_with_format(
                    SendParams {
                        token: &token,
                        message_type: route.message_type,
                        sender_id: route.sender_id,
                        channel_id: route.channel_id,
                        group_openid: route.group_openid,
                        guild_id: route.guild_id,
                        text: &text,
                        msg_id: route.msg_id,
                    },
                    to.metadata["markdown_enabled"]
                        .as_bool()
                        .or_else(|| self.acl_metadata["markdown_enabled"].as_bool())
                        .unwrap_or(true),
                )
                .await
                .map_err(|e| rua_core::RuaError::Io {
                    message: format!("qq send error: {e}"),
                })?;
        }

        for part in &msg.content {
            if !Self::is_media_part(part) {
                continue;
            }
            match self.send_media_part(&token, &route, part).await {
                Ok(true) => {}
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "qq media send failed; sending fallback text");
                    let fallback = render_parts_to_string(std::slice::from_ref(part));
                    if !fallback.is_empty() {
                        self.sender
                            .send_message(SendParams {
                                token: &token,
                                message_type: route.message_type,
                                sender_id: route.sender_id,
                                channel_id: route.channel_id,
                                group_openid: route.group_openid,
                                guild_id: route.guild_id,
                                text: &fallback,
                                msg_id: route.msg_id,
                            })
                            .await
                            .map_err(|e| rua_core::RuaError::Io {
                                message: format!("qq fallback send error: {e}"),
                            })?;
                    }
                }
            }
        }

        Ok(())
    }

    fn build_request_from_native(&self, mut native: serde_json::Value) -> Result<AgentRequest> {
        if let Some(spec) = native["event_type"].as_str().and_then(get_event_spec) {
            native["message_type"] = serde_json::json!(spec.message_type);
        }
        if let Some(req) = resolve_approval_interaction(&native, &self.approval_resolver)? {
            return Ok(req);
        }

        let message_type = native
            .get("message_type")
            .and_then(|v| v.as_str())
            .unwrap_or("c2c")
            .to_string();
        let sender_id = resolve_native_sender_id(&native, &message_type);
        let text = native
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let (text, mut bot_mentioned) = strip_qq_bot_mention(&native, &text, &self.app_id);
        bot_mentioned |= matches!(
            native["event_type"].as_str(),
            Some("GROUP_AT_MESSAGE_CREATE" | "AT_MESSAGE_CREATE")
        );
        let msg_id = native
            .get("msg_id")
            .or_else(|| native.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if qq_check_duplicate_event(&self.event_deduper, &msg_id)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate qq event".into(),
            });
        }
        let group_openid = native
            .get("group_openid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let channel_id = native
            .get("channel_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let guild_id = native
            .get("guild_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let user_name = native
            .get("user_name")
            .or_else(|| native.get("username"))
            .or_else(|| native.pointer("/author/username"))
            .or_else(|| native.pointer("/author/nick"))
            .and_then(|v| v.as_str())
            .unwrap_or(&sender_id)
            .to_string();
        let is_group = matches!(message_type.as_str(), "group" | "guild");

        let mut metadata = serde_json::json!({
            "message_type": message_type,
            "sender_id": sender_id,
            "message_id": msg_id,
            "user_name": user_name,
            "is_group": is_group,
            "is_dm": !is_group,
            "bot_mentioned": bot_mentioned,
            "mentioned": bot_mentioned,
        });
        if !group_openid.is_empty() {
            metadata["group_openid"] = serde_json::json!(group_openid);
        }
        if !channel_id.is_empty() {
            metadata["channel_id"] = serde_json::json!(channel_id);
        }
        if !guild_id.is_empty() {
            metadata["guild_id"] = serde_json::json!(guild_id);
        }

        let conversation_id = resolve_native_conversation_id(
            &message_type,
            &sender_id,
            metadata.get("group_openid").and_then(|v| v.as_str()),
            metadata.get("channel_id").and_then(|v| v.as_str()),
            metadata.get("guild_id").and_then(|v| v.as_str()),
        );
        let mut input = Vec::new();
        if !text.is_empty() || extract_native_attachment_urls(&native).is_empty() {
            input.push(IncomingContentPart::Text { text });
        }
        input.extend(extract_native_attachment_urls(&native));

        Ok(AgentChannelRequest {
            agent_id: String::new(),
            user_id: sender_id.clone(),
            session_id: conversation_id.clone(),
            input,
            channel_id: "qq".into(),
            conversation_id,
            message_id: if msg_id.is_empty() {
                None
            } else {
                Some(msg_id)
            },
            metadata,
        })
    }
}

fn strip_qq_bot_mention(native: &serde_json::Value, text: &str, bot_id: &str) -> (String, bool) {
    let bot_id = bot_id.trim();
    if bot_id.is_empty() || !qq_mentions_bot(native, bot_id) {
        return (text.to_string(), false);
    }
    let mut stripped = text.to_string();
    for token in [
        format!("<@{bot_id}>"),
        format!("<@!{bot_id}>"),
        format!("@{bot_id}"),
    ] {
        stripped = stripped.replace(&token, " ");
    }
    (
        stripped.split_whitespace().collect::<Vec<_>>().join(" "),
        true,
    )
}

fn qq_mentions_bot(native: &serde_json::Value, bot_id: &str) -> bool {
    native
        .get("mentions")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .any(|mention| {
            ["id", "user_id", "openid", "bot_id", "app_id"]
                .into_iter()
                .filter_map(|key| mention.get(key).and_then(|value| value.as_str()))
                .any(|value| value == bot_id)
        })
}

fn resolve_approval_interaction(
    native: &serde_json::Value,
    resolver: &Arc<Mutex<ApprovalCardResolver>>,
) -> Result<Option<AgentChannelRequest>> {
    let event_type = native
        .get("event_type")
        .or_else(|| native.get("t"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if event_type != "INTERACTION_CREATE"
        && native.pointer("/data/resolved/button_data").is_none()
        && native.get("button_data").is_none()
    {
        return Ok(None);
    }

    let parsed = match approval_card::parse_interaction_event(native) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(None),
    };
    let resolution = resolver
        .lock()
        .map_err(|_| rua_core::RuaError::Runtime {
            message: "qq approval resolver lock poisoned".into(),
        })?
        .resolve(ApprovalCardCallback {
            request_id: parsed.request_id.clone(),
            action: parsed.action.clone(),
            decided_by: parsed.operator_id.clone(),
            platform: "qq".into(),
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
    let ctx = parsed.session_ctx.as_object();
    let message_type = ctx
        .and_then(|ctx| ctx.get("mt"))
        .and_then(|value| value.as_str())
        .unwrap_or("c2c")
        .to_string();
    let sender_id = ctx
        .and_then(|ctx| ctx.get("sender"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&parsed.operator_id)
        .to_string();
    let group_openid = ctx
        .and_then(|ctx| ctx.get("goid"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let channel_id = ctx
        .and_then(|ctx| ctx.get("cid"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let guild_id = ctx
        .and_then(|ctx| ctx.get("gid"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let message_id = ctx
        .and_then(|ctx| ctx.get("mid"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let conversation_id = ctx
        .and_then(|ctx| ctx.get("sid"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            resolve_native_conversation_id(
                &message_type,
                &sender_id,
                (!group_openid.is_empty()).then_some(group_openid.as_str()),
                (!channel_id.is_empty()).then_some(channel_id.as_str()),
                (!guild_id.is_empty()).then_some(guild_id.as_str()),
            )
        });
    let approval_action = match parsed.action {
        crate::core::approval_card::ApprovalCardAction::Approve => "approve",
        crate::core::approval_card::ApprovalCardAction::Deny => "deny",
    };
    let command_text = resolution.command_text();
    let mut metadata = serde_json::json!({
        "message_type": message_type.clone(),
        "sender_id": sender_id.clone(),
        "message_id": message_id,
        "approval_request_id": parsed.request_id.clone(),
        "approval_action": approval_action,
        "approval_tool_name": parsed.tool_name.clone(),
        "approval_severity": parsed.severity.clone(),
        "approval_interaction_id": parsed.interaction_id.clone(),
        "approval_operator_id": parsed.operator_id.clone(),
        "from_card_action": true,
    });
    if !group_openid.is_empty() {
        metadata["group_openid"] = serde_json::json!(group_openid);
    }
    if !channel_id.is_empty() {
        metadata["channel_id"] = serde_json::json!(channel_id);
    }
    if !guild_id.is_empty() {
        metadata["guild_id"] = serde_json::json!(guild_id);
    }

    Ok(Some(AgentChannelRequest {
        agent_id: String::new(),
        user_id: sender_id,
        session_id: conversation_id.clone(),
        input: vec![IncomingContentPart::Text { text: command_text }],
        channel_id: "qq".into(),
        conversation_id,
        message_id: None,
        metadata,
    }))
}

fn resolve_native_sender_id(native: &serde_json::Value, message_type: &str) -> String {
    if let Some(sender_id) = native.get("sender_id").and_then(|v| v.as_str()) {
        return sender_id.to_string();
    }

    let author = native.get("author").unwrap_or(&serde_json::Value::Null);
    let keys: &[&str] = match message_type {
        "group" => &["member_openid", "id"],
        "c2c" => &["user_openid", "id"],
        _ => &["id", "username"],
    };

    keys.iter()
        .find_map(|key| author.get(*key).and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string()
}

fn resolve_native_conversation_id(
    message_type: &str,
    sender_id: &str,
    group_openid: Option<&str>,
    channel_id: Option<&str>,
    guild_id: Option<&str>,
) -> String {
    match message_type {
        "group" => group_openid
            .filter(|value| !value.is_empty())
            .map(|value| format!("group:{value}"))
            .unwrap_or_else(|| sender_id.to_string()),
        "guild" => channel_id
            .filter(|value| !value.is_empty())
            .map(|value| format!("channel:{value}"))
            .unwrap_or_else(|| sender_id.to_string()),
        "dm" => guild_id
            .filter(|value| !value.is_empty())
            .map(|value| format!("dm:{value}"))
            .unwrap_or_else(|| sender_id.to_string()),
        _ => sender_id.to_string(),
    }
}

fn extract_native_attachment_urls(native: &serde_json::Value) -> Vec<IncomingContentPart> {
    native
        .get("attachments")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|attachment| {
            if let Some(part) = qq_local_media_part(attachment) {
                return Some(part);
            }
            let url = attachment.get("url").and_then(|v| v.as_str())?;
            if url.is_empty() {
                return None;
            }
            let title = attachment
                .get("filename")
                .or_else(|| attachment.get("name"))
                .and_then(|v| v.as_str())
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            Some(IncomingContentPart::Url {
                url: url.to_string(),
                title,
            })
        })
        .collect()
}

async fn download_qq_inbound_media_with(
    media_dir: &str,
    native: &mut serde_json::Value,
) -> Result<()> {
    let Some(attachments) = native
        .get_mut("attachments")
        .and_then(|value| value.as_array_mut())
    else {
        return Ok(());
    };
    let client = default_http_client();
    for attachment in attachments {
        if attachment
            .get("local_path")
            .and_then(|value| value.as_str())
            .is_some()
        {
            continue;
        }
        let url = attachment
            .get("url")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| value.starts_with("http://") || value.starts_with("https://"));
        let Some(url) = url else {
            continue;
        };
        let Some((bytes, mime)) = qq_download_attachment(&client, url).await? else {
            continue;
        };
        let filename = qq_download_filename(attachment);
        let dir = qq_media_dir(media_dir);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("create qq media dir {}: {err}", dir.display()),
            })?;
        let final_path = dir.join(filename);
        atomic_write_file(&final_path, &bytes).await?;
        if let Some(obj) = attachment.as_object_mut() {
            obj.insert(
                "local_path".to_string(),
                serde_json::Value::String(final_path.to_string_lossy().to_string()),
            );
            if let Some(mime) = mime {
                obj.insert("mime".to_string(), serde_json::Value::String(mime));
            }
        }
    }
    Ok(())
}

async fn qq_download_attachment(
    client: &reqwest::Client,
    url: &str,
) -> Result<Option<(Vec<u8>, Option<String>)>> {
    let resp = match client.get(url).timeout(REQUEST_TIMEOUT).send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%url, error = %err, "qq attachment download failed");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(%url, status = %resp.status(), "qq attachment download non-success");
        return Ok(None);
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_INBOUND_MEDIA_SIZE {
            tracing::warn!(%url, len, "qq attachment too large");
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
            message: format!("qq attachment download bytes failed: {err}"),
        })?;
    if bytes.len() as u64 > MAX_INBOUND_MEDIA_SIZE {
        tracing::warn!(%url, len = bytes.len(), "qq attachment too large");
        return Ok(None);
    }
    Ok(Some((bytes.to_vec(), mime)))
}

fn qq_local_media_part(attachment: &serde_json::Value) -> Option<IncomingContentPart> {
    let path = attachment
        .get("local_path")
        .or_else(|| attachment.get("path"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)?;
    let file_id = attachment
        .get("filename")
        .or_else(|| attachment.get("name"))
        .or_else(|| attachment.get("id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("attachment")
        .to_string();
    let mime = attachment
        .get("mime")
        .or_else(|| attachment.get("content_type"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let name = attachment
        .get("filename")
        .or_else(|| attachment.get("name"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if mime
        .as_deref()
        .is_some_and(|value| value.starts_with("image/"))
    {
        Some(IncomingContentPart::Image {
            file_id,
            path,
            mime,
        })
    } else if mime
        .as_deref()
        .is_some_and(|value| value.starts_with("audio/"))
    {
        Some(IncomingContentPart::Audio {
            file_id,
            path,
            mime,
            name,
        })
    } else if mime
        .as_deref()
        .is_some_and(|value| value.starts_with("video/"))
    {
        Some(IncomingContentPart::Video {
            file_id,
            path,
            mime,
            name,
        })
    } else {
        Some(IncomingContentPart::File {
            file_id,
            path,
            mime,
            name,
        })
    }
}

fn qq_media_dir(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    if media_dir.is_empty() {
        std::env::temp_dir().join("rua-qq-media")
    } else {
        PathBuf::from(media_dir)
    }
}

fn qq_download_filename(attachment: &serde_json::Value) -> String {
    let hint = attachment
        .get("filename")
        .or_else(|| attachment.get("name"))
        .or_else(|| attachment.get("id"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("attachment");
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
            .unwrap_or_else(|| "qq-media".into()),
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

struct QqAckIndicator {
    sender: Arc<QqSender>,
    token: Arc<QqTokenManager>,
    text: String,
}

#[async_trait]
impl crate::core::streaming::ProcessingIndicator for QqAckIndicator {
    async fn start(&self, req: &AgentRequest) -> Result<()> {
        if !self.text.is_empty() {
            let meta = &req.metadata;
            let result = async {
                let token = self.token.get_access_token().await?;
                self.sender
                    .send_message(SendParams {
                        token: &token,
                        message_type: meta["message_type"].as_str().unwrap_or("c2c"),
                        sender_id: meta["sender_id"].as_str().unwrap_or_default(),
                        channel_id: meta["channel_id"].as_str(),
                        group_openid: meta["group_openid"].as_str(),
                        guild_id: meta["guild_id"].as_str(),
                        text: &self.text,
                        msg_id: meta["message_id"].as_str(),
                    })
                    .await
            }
            .await;
            if let Err(error) = result {
                tracing::debug!(%error, "QQ acknowledgement failed");
            }
        }
        Ok(())
    }

    async fn stop(&self, _: &AgentRequest) -> Result<()> {
        Ok(())
    }
}
