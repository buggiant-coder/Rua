use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use matrix_sdk::authentication::matrix::MatrixSession;
use matrix_sdk::config::SyncSettings;
use matrix_sdk::ruma::events::room::message::SyncRoomMessageEvent;
use matrix_sdk::ruma::{OwnedRoomId, RoomId};
use matrix_sdk::{Client, SessionMeta, SessionTokens};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock};

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

use super::inbound::MatrixInbound;
use super::outbound::MatrixReply;
use crate::matrix::types::{MatrixConfig, MatrixOptions};

pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const MAX_INBOUND_MEDIA_SIZE: u64 = 50 * 1024 * 1024;
const STREAM_MIN_INTERVAL: Duration = Duration::from_millis(500);
const STREAM_MAX_CHARS: usize = 3000;
const TYPING_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);
const TYPING_TIMEOUT_MS: u64 = 30_000;
const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

#[derive(Debug)]
struct MatrixStreamInner {
    state: PlatformStreamState,
    event_id: Option<String>,
}

pub struct MatrixStreamHandler {
    homeserver_url: String,
    access_token: String,
    room_id: String,
    policy: PlatformStreamPolicy,
    room: Option<matrix_sdk::Room>,
    reply: MatrixReply,
    started_at: tokio::time::Instant,
    inner: Mutex<MatrixStreamInner>,
}

impl MatrixStreamHandler {
    pub fn new(
        homeserver_url: String,
        access_token: String,
        room_id: String,
        policy: PlatformStreamPolicy,
    ) -> Self {
        Self {
            homeserver_url,
            access_token,
            room_id,
            policy,
            room: None,
            reply: MatrixReply::default(),
            started_at: tokio::time::Instant::now(),
            inner: Mutex::new(MatrixStreamInner {
                state: PlatformStreamState::default(),
                event_id: None,
            }),
        }
    }

    fn with_room(mut self, room: matrix_sdk::Room) -> Self {
        self.room = Some(room);
        self
    }

    fn with_reply(mut self, reply: MatrixReply) -> Self {
        self.reply = reply;
        self
    }

    async fn send_event(&self, event: serde_json::Value) -> Result<String> {
        let event = self.reply.decorate(event);
        if let Some(room) = &self.room {
            // SDK applies the room's encryption policy to streamed edits too.
            return room
                .send_raw("m.room.message", event)
                .await
                .map(|result| result.response.event_id.to_string())
                .map_err(|error| rua_core::RuaError::Runtime {
                    message: format!("matrix stream send failed: {error}"),
                });
        }
        send_matrix_room_event_return_event_id_with_base_url(
            &self.homeserver_url,
            &self.access_token,
            &self.room_id,
            event,
        )
        .await
    }

    async fn create_placeholder(&self, content: &str) -> Result<String> {
        self.send_event(matrix_text_event(content)).await
    }

    async fn edit_message(&self, event_id: &str, content: &str) -> Result<()> {
        self.send_event(matrix_replacement_event(event_id, content))
            .await
            .map(|_| ())
    }

    async fn send_final(&self, content: &str) -> Result<()> {
        self.send_event(matrix_text_event(content))
            .await
            .map(|_| ())
    }

    async fn apply_action(&self, action: PlatformStreamAction) -> Result<()> {
        match action {
            PlatformStreamAction::Create { content, .. } => {
                let event_id = self.create_placeholder(&content).await?;
                self.inner.lock().await.event_id = Some(event_id);
            }
            PlatformStreamAction::Update {
                content,
                final_update,
                ..
            } => {
                let event_id = self.inner.lock().await.event_id.clone();
                if let Some(event_id) = event_id {
                    self.edit_message(&event_id, &content).await?;
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
impl StreamHandler for MatrixStreamHandler {
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

pub struct MatrixTypingIndicator {
    homeserver_url: String,
    access_token: String,
    room_id: String,
    user_id: String,
    keepalive_interval: Duration,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    cancel: tokio_util::sync::CancellationToken,
}

impl MatrixTypingIndicator {
    pub fn new(
        homeserver_url: String,
        access_token: String,
        room_id: String,
        user_id: String,
    ) -> Self {
        Self::with_keepalive_interval(
            homeserver_url,
            access_token,
            room_id,
            user_id,
            TYPING_KEEPALIVE_INTERVAL,
        )
    }

    pub fn with_keepalive_interval(
        homeserver_url: String,
        access_token: String,
        room_id: String,
        user_id: String,
        keepalive_interval: Duration,
    ) -> Self {
        Self {
            homeserver_url,
            access_token,
            room_id,
            user_id,
            keepalive_interval,
            task: Mutex::new(None),
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    fn with_cancellation(mut self, cancel: tokio_util::sync::CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    async fn send_typing(&self, typing: bool) -> Result<()> {
        send_matrix_typing_with_base_url(
            &self.homeserver_url,
            &self.access_token,
            &self.room_id,
            &self.user_id,
            typing,
            TYPING_TIMEOUT_MS,
        )
        .await
    }

    async fn abort_task(&self) {
        if let Some(handle) = self.task.lock().await.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl Drop for MatrixTypingIndicator {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.get_mut().take() {
            task.abort();
        }
    }
}

#[async_trait]
impl ProcessingIndicator for MatrixTypingIndicator {
    async fn start(&self, _req: &AgentRequest) -> Result<()> {
        self.abort_task().await;
        self.send_typing(true).await?;
        let homeserver_url = self.homeserver_url.clone();
        let access_token = self.access_token.clone();
        let room_id = self.room_id.clone();
        let user_id = self.user_id.clone();
        let interval = self.keepalive_interval;
        let cancel = self.cancel.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(interval) => {},
                }
                if let Err(err) = send_matrix_typing_with_base_url(
                    &homeserver_url,
                    &access_token,
                    &room_id,
                    &user_id,
                    true,
                    TYPING_TIMEOUT_MS,
                )
                .await
                {
                    tracing::debug!(error = %err, "matrix typing keepalive failed");
                }
            }
        });
        *self.task.lock().await = Some(handle);
        Ok(())
    }

    async fn stop(&self, _req: &AgentRequest) -> Result<()> {
        self.abort_task().await;
        let _ = self.send_typing(false).await;
        Ok(())
    }
}

pub struct MatrixChannel {
    channel_id: ChannelId,
    config: MatrixConfig,
    options: MatrixOptions,
    device_id: String,
    device_name: String,
    client: RwLock<Option<Client>>,
    started: RwLock<bool>,
    sync_handle: RwLock<Option<tokio::task::JoinHandle<()>>>,
    sync_cancel: RwLock<tokio_util::sync::CancellationToken>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    event_deduper: Arc<StdMutex<EventDeduper>>,
}

impl MatrixChannel {
    pub fn new(config: MatrixConfig) -> Self {
        Self {
            channel_id: "matrix".into(),
            device_id: String::new(),
            device_name: "rua-bot".into(),
            config,
            options: MatrixOptions::default(),
            client: RwLock::new(None),
            started: RwLock::new(false),
            sync_handle: RwLock::new(None),
            sync_cancel: RwLock::new(tokio_util::sync::CancellationToken::new()),
            render_options: MessageRenderOptions::default(),
            acl_metadata: serde_json::json!({}),
            event_deduper: Arc::new(StdMutex::new(EventDeduper::new(
                EVENT_DEDUP_TTL,
                EVENT_DEDUP_CAPACITY,
            ))),
        }
    }

    pub fn with_options(mut self, options: MatrixOptions) -> Self {
        self.options = options;
        self
    }

    pub fn with_device(mut self, device_id: String, device_name: String) -> Self {
        self.device_id = device_id;
        if !device_name.is_empty() {
            self.device_name = device_name;
        }
        self
    }

    async fn access_token(&self) -> String {
        self.client
            .read()
            .await
            .as_ref()
            .and_then(Client::access_token)
            .unwrap_or_else(|| self.config.access_token.clone())
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
        download_matrix_inbound_media_with(
            &self.config.homeserver_url,
            &self.access_token().await,
            &self.config.media_dir,
            native,
        )
        .await
    }

    async fn get_client(&self) -> Result<Client> {
        let guard = self.client.read().await;
        guard.clone().ok_or_else(|| rua_core::RuaError::Runtime {
            message: "matrix client not initialized".into(),
        })
    }

    fn resolve_store_path(config: &MatrixConfig) -> std::path::PathBuf {
        if let Some(ref path) = config.store_path {
            std::path::PathBuf::from(path)
        } else {
            let base = std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::env::temp_dir());
            base.join(".rua-dev").join("matrix_store")
        }
    }

    async fn login(&self) -> Result<Client> {
        if self.config.access_token.is_empty() && self.config.password.is_empty() {
            return Err(rua_core::RuaError::Runtime {
                message: "matrix: neither password nor access_token provided".into(),
            });
        }
        let homeserver = url::Url::parse(&self.config.homeserver_url).map_err(|e| {
            rua_core::RuaError::Runtime {
                message: format!("invalid homeserver URL: {e}"),
            }
        })?;

        let store_path = Self::resolve_store_path(&self.config);
        let client = Client::builder()
            .homeserver_url(homeserver)
            .with_encryption_settings(matrix_sdk::encryption::EncryptionSettings {
                auto_enable_cross_signing: self.config.encryption_enabled,
                ..Default::default()
            })
            .sqlite_store(&store_path, None)
            .build()
            .await
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("matrix client build failed: {e}"),
            })?;

        if !self.config.access_token.is_empty() {
            #[derive(serde::Deserialize)]
            struct WhoAmI {
                user_id: matrix_sdk::ruma::OwnedUserId,
                device_id: Option<String>,
            }
            let identity: WhoAmI = default_http_client()
                .get(format!(
                    "{}/_matrix/client/v3/account/whoami",
                    self.config.homeserver_url.trim_end_matches('/')
                ))
                .bearer_auth(&self.config.access_token)
                .timeout(REQUEST_TIMEOUT)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|error| rua_core::RuaError::Runtime {
                    message: format!("matrix token authentication failed: {error}"),
                })?
                .json()
                .await
                .map_err(|error| rua_core::RuaError::Runtime {
                    message: format!("matrix identity response invalid: {error}"),
                })?;
            let device_id = identity
                .device_id
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| {
                    if self.device_id.is_empty() {
                        self.device_name.clone()
                    } else {
                        self.device_id.clone()
                    }
                });
            client
                .matrix_auth()
                .restore_session(
                    MatrixSession {
                        meta: SessionMeta {
                            user_id: identity.user_id,
                            device_id: device_id.into(),
                        },
                        tokens: SessionTokens {
                            access_token: self.config.access_token.clone(),
                            refresh_token: None,
                        },
                    },
                    Default::default(),
                )
                .await
                .map_err(|error| rua_core::RuaError::Runtime {
                    message: format!("matrix session restore failed: {error}"),
                })?;
        } else {
            let saved = tokio::fs::read(store_path.join("session.json"))
                .await
                .ok()
                .and_then(|bytes| serde_json::from_slice::<MatrixSession>(&bytes).ok())
                .filter(|session| {
                    session.meta.user_id.as_str() == self.config.user_id
                        || session.meta.user_id.localpart() == self.config.user_id
                })
                .filter(|session| {
                    self.device_id.is_empty() || session.meta.device_id.as_str() == self.device_id
                });
            if let Some(session) = saved {
                client
                    .matrix_auth()
                    .restore_session(session, Default::default())
                    .await
                    .map_err(|error| rua_core::RuaError::Runtime {
                        message: format!("matrix saved session restore failed: {error}"),
                    })?;
                return Ok(client);
            }
            let auth = client.matrix_auth();
            let mut login = auth
                .login_username(&self.config.user_id, &self.config.password)
                .initial_device_display_name(&self.device_name);
            if !self.device_id.is_empty() {
                login = login.device_id(&self.device_id);
            }
            login.await.map_err(|error| rua_core::RuaError::Runtime {
                message: format!("matrix login failed: {error}"),
            })?;
        }

        if let Some(session) = client.matrix_auth().session() {
            let bytes =
                serde_json::to_vec(&session).map_err(|error| rua_core::RuaError::Runtime {
                    message: format!("matrix session encoding failed: {error}"),
                })?;
            atomic_write_file(&store_path.join("session.json"), &bytes).await?;
        }
        Ok(client)
    }

    fn parse_room_id(room_str: &str) -> Result<OwnedRoomId> {
        <&RoomId>::try_from(room_str)
            .map(|r| r.to_owned())
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("invalid room id '{room_str}': {e}"),
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

    pub fn matrix_media_event_content(
        part: &OutgoingContentPart,
        mxc_uri: &str,
        file_path: &Path,
    ) -> Option<serde_json::Value> {
        let filename = matrix_media_filename(part, file_path);
        let mime = guess_matrix_mime(file_path);
        let size = std::fs::metadata(file_path)
            .ok()
            .map(|meta| meta.len())
            .unwrap_or(0);
        let msgtype = match part {
            OutgoingContentPart::Image { .. } => "m.image",
            OutgoingContentPart::Audio { .. } => "m.audio",
            OutgoingContentPart::Video { .. } => "m.video",
            OutgoingContentPart::File { .. } => "m.file",
            _ => return None,
        };
        Some(serde_json::json!({
            "msgtype": msgtype,
            "body": filename,
            "url": mxc_uri,
            "info": {
                "mimetype": mime,
                "size": size,
            }
        }))
    }
}

async fn send_matrix_attachment(
    room: &matrix_sdk::Room,
    part: &OutgoingContentPart,
    reply: &MatrixReply,
) -> Result<()> {
    let Some((path, _)) = matrix_media_path_and_name(part) else {
        return Ok(());
    };
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| rua_core::RuaError::Io {
            message: format!("read matrix attachment: {error}"),
        })?;
    let content_type = guess_matrix_mime(path).parse().expect("known MIME type");
    room.send_attachment(
        matrix_media_filename(part, path),
        &content_type,
        bytes,
        matrix_sdk::attachment::AttachmentConfig::new().reply(reply.attachment_reply()),
    )
    .await
    .map_err(|error| rua_core::RuaError::Runtime {
        message: format!("matrix attachment send failed: {error}"),
    })?;
    Ok(())
}

pub async fn upload_matrix_media_with_base_url(
    homeserver_url: &str,
    access_token: &str,
    path: &Path,
    name_hint: Option<&str>,
) -> Result<String> {
    if access_token.trim().is_empty() {
        return Err(rua_core::RuaError::Runtime {
            message: "matrix media upload requires access_token".into(),
        });
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("read matrix media {}: {err}", path.display()),
        })?;
    let filename = name_hint
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            path.file_name()
                .and_then(|value| value.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "file".to_string());
    let url = format!(
        "{}/_matrix/media/v3/upload?filename={}",
        homeserver_url.trim_end_matches('/'),
        percent_encode_path_segment(&filename)
    );
    let resp = default_http_client()
        .post(url)
        .bearer_auth(access_token)
        .header(reqwest::header::CONTENT_TYPE, guess_matrix_mime(path))
        .body(bytes)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("matrix media upload failed: {err}"),
        })?;
    if !resp.status().is_success() {
        return Err(rua_core::RuaError::Runtime {
            message: format!("matrix media upload HTTP {}", resp.status()),
        });
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("matrix media upload parse failed: {err}"),
        })?;
    body.get("content_uri")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| rua_core::RuaError::Runtime {
            message: format!("matrix media upload response missing content_uri: {body}"),
        })
}

pub async fn send_matrix_room_event_with_base_url(
    homeserver_url: &str,
    access_token: &str,
    room_id: &str,
    event: serde_json::Value,
) -> Result<()> {
    send_matrix_room_event_return_event_id_with_base_url(
        homeserver_url,
        access_token,
        room_id,
        event,
    )
    .await
    .map(|_| ())
}

pub async fn send_matrix_room_event_return_event_id_with_base_url(
    homeserver_url: &str,
    access_token: &str,
    room_id: &str,
    event: serde_json::Value,
) -> Result<String> {
    if access_token.trim().is_empty() {
        return Err(rua_core::RuaError::Runtime {
            message: "matrix room send requires access_token".into(),
        });
    }
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
        homeserver_url.trim_end_matches('/'),
        percent_encode_path_segment(room_id),
        uuid::Uuid::new_v4()
    );
    let resp = default_http_client()
        .put(url)
        .bearer_auth(access_token)
        .json(&event)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("matrix media event send failed: {err}"),
        })?;
    if !resp.status().is_success() {
        return Err(rua_core::RuaError::Runtime {
            message: format!("matrix media event send HTTP {}", resp.status()),
        });
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("matrix room send parse failed: {err}"),
        })?;
    body.get("event_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| rua_core::RuaError::Runtime {
            message: format!("matrix room send response missing event_id: {body}"),
        })
}

pub fn matrix_text_event(text: &str) -> serde_json::Value {
    serde_json::json!({
        "msgtype": "m.text",
        "body": text,
    })
}

pub fn matrix_replacement_event(event_id: &str, text: &str) -> serde_json::Value {
    serde_json::json!({
        "msgtype": "m.text",
        "body": format!("* {text}"),
        "m.new_content": matrix_text_event(text),
        "m.relates_to": {
            "rel_type": "m.replace",
            "event_id": event_id,
        },
    })
}

pub async fn send_matrix_typing_with_base_url(
    homeserver_url: &str,
    access_token: &str,
    room_id: &str,
    user_id: &str,
    typing: bool,
    timeout_ms: u64,
) -> Result<()> {
    if access_token.trim().is_empty() {
        return Err(rua_core::RuaError::Runtime {
            message: "matrix typing requires access_token".into(),
        });
    }
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/typing/{}",
        homeserver_url.trim_end_matches('/'),
        percent_encode_path_segment(room_id),
        percent_encode_path_segment(user_id)
    );
    let body = if typing {
        serde_json::json!({
            "typing": true,
            "timeout": timeout_ms,
        })
    } else {
        serde_json::json!({
            "typing": false,
        })
    };
    let resp = default_http_client()
        .put(url)
        .bearer_auth(access_token)
        .json(&body)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("matrix typing failed: {err}"),
        })?;
    if !resp.status().is_success() {
        return Err(rua_core::RuaError::Runtime {
            message: format!("matrix typing HTTP {}", resp.status()),
        });
    }
    Ok(())
}

fn matrix_media_path_and_name(part: &OutgoingContentPart) -> Option<(&Path, Option<&str>)> {
    match part {
        OutgoingContentPart::Image {
            path: Some(path),
            alt,
            ..
        } => Some((path.as_path(), alt.as_deref())),
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
        } => Some((path.as_path(), name.as_deref())),
        _ => None,
    }
}

fn matrix_media_filename(part: &OutgoingContentPart, path: &Path) -> String {
    match part {
        OutgoingContentPart::Image { alt, .. } => alt.as_deref(),
        OutgoingContentPart::File { name, .. }
        | OutgoingContentPart::Audio { name, .. }
        | OutgoingContentPart::Video { name, .. } => name.as_deref(),
        _ => None,
    }
    .filter(|value| !value.trim().is_empty())
    .map(str::to_string)
    .or_else(|| {
        path.file_name()
            .and_then(|value| value.to_str())
            .map(str::to_string)
    })
    .unwrap_or_else(|| "file".to_string())
}

fn guess_matrix_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

pub(super) fn percent_encode_path_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(&mut out, "%{byte:02X}");
            }
        }
    }
    out
}

#[async_trait]
impl channel::Channel for MatrixChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("matrix channel already started");
            return Ok(());
        }

        let client = self.login().await?;
        *self.client.write().await = Some(client.clone());

        let own_user_id = client
            .user_id()
            .map(ToString::to_string)
            .unwrap_or_default();
        let homeserver_url = self.config.homeserver_url.clone();
        let access_token = client.access_token().unwrap_or_default();

        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::Markdown,
            self.render_options.clone(),
        ));
        let acl_metadata = Arc::new(self.acl_metadata.clone());
        let event_deduper = self.event_deduper.clone();

        let sync_timeout_ms = self.options.sync_timeout_ms.clamp(5_000, 300_000);
        let options = self.options.clone();
        let inbound = MatrixInbound::new(
            self.config.clone(),
            options.clone(),
            (*acl_metadata).clone(),
        );
        let send_fn: pipeline::SendFn = Arc::new({
            let client = client.clone();
            let options = options.clone();
            move |to, msg| {
                let client = client.clone();
                let options = options.clone();
                Box::pin(async move {
                    let room_id = MatrixChannel::parse_room_id(&to.conversation_id)?;
                    let room =
                        client
                            .get_room(&room_id)
                            .ok_or_else(|| rua_core::RuaError::Runtime {
                                message: format!("room {room_id} not found"),
                            })?;
                    let reply = MatrixReply::from_metadata(&to.metadata, options);
                    send_matrix_message(&room, &msg, &reply).await
                })
            }
        });

        let encryption_enabled = self.config.encryption_enabled;
        let token_path = Self::resolve_store_path(&self.config).join("sync-token");
        let cancel = tokio_util::sync::CancellationToken::new();
        *self.sync_cancel.write().await = cancel.clone();
        let event_cancel = cancel.child_token();
        let handle = tokio::spawn(async move {
            let sync = async move {
                let _verification_guard =
                    encryption_enabled.then(|| super::verification::install(&client));
                let mut token = tokio::fs::read_to_string(&token_path).await.ok();
                // An initial catch-up populates SDK state without replaying historical messages.
                if token.is_none() {
                    loop {
                        match client
                            .sync_once(SyncSettings::default().full_state(true))
                            .await
                        {
                            Ok(response) => {
                                token = Some(response.next_batch.clone());
                                if let Err(error) =
                                    atomic_write_file(&token_path, response.next_batch.as_bytes())
                                        .await
                                {
                                    tracing::warn!(%error, "matrix sync token persistence failed");
                                }
                                join_invited_rooms(&client, response.rooms.invited.keys()).await;
                                break;
                            }
                            Err(error) => {
                                tracing::warn!(%error, "matrix initial sync failed");
                                tokio::time::sleep(Duration::from_secs(5)).await;
                            }
                        }
                    }
                }

                let event_client = client.clone();
                let event_handle = client.add_event_handler(move |ev: SyncRoomMessageEvent, room: matrix_sdk::Room| {
                let client = event_client.clone();
                let process = process.clone();
                let renderer = renderer.clone();
                let send_fn = send_fn.clone();
                let own_user_id = own_user_id.clone();
                let homeserver_url = homeserver_url.clone();
                let access_token = access_token.clone();
                let event_deduper = event_deduper.clone();
                let inbound = inbound.clone();
                let options = options.clone();
                let indicator_cancel = event_cancel.child_token();
                async move {
                    if ev.sender().as_str() == own_user_id || ev.as_original().is_none() {
                        return;
                    }
                    if matrix_check_duplicate_event(&event_deduper, ev.event_id().as_ref()).unwrap_or(false) {
                        return;
                    }
                    let mut native = match serde_json::to_value(&ev.as_original().expect("original event").content) {
                        Ok(value) => value,
                        Err(error) => {
                            tracing::warn!(%error, "matrix event decode failed");
                            return;
                        }
                    };
                    // Edits must not start another agent turn.
                    if native.pointer("/m.relates_to/rel_type").and_then(|v| v.as_str()) == Some("m.replace") {
                        return;
                    }
                    native["sender"] = serde_json::json!(ev.sender());
                    native["event_id"] = serde_json::json!(ev.event_id());
                    native["room_id"] = serde_json::json!(room.room_id());
                    let req = match inbound.prepare(&client, &room, native).await {
                        Ok(Some(req)) => req,
                        Ok(None) => return,
                        Err(error) => {
                            tracing::warn!(%error, "matrix incoming event failed");
                            return;
                        }
                    };
                    let receipt = room.send_single_receipt(
                        matrix_sdk::ruma::api::client::receipt::create_receipt::v3::ReceiptType::Read,
                        matrix_sdk::ruma::events::receipt::ReceiptThread::Unthreaded,
                        ev.event_id().to_owned(),
                    ).await;
                    if let Err(error) = receipt {
                        tracing::debug!(%error, "matrix read receipt failed");
                    }
                    let reply = MatrixReply::from_metadata(&req.metadata, options);
                    let stream_handler = Some(Arc::new(
                        MatrixStreamHandler::new(
                            homeserver_url.clone(), access_token.clone(),
                            room.room_id().to_string(),
                            PlatformStreamPolicy::placeholder_edit(STREAM_MIN_INTERVAL, STREAM_MAX_CHARS),
                        ).with_room(room.clone()).with_reply(reply),
                    ) as Arc<dyn StreamHandler>);
                    let indicator = Some(Arc::new(MatrixTypingIndicator::new(
                        homeserver_url, access_token, room.room_id().to_string(), own_user_id,
                    ).with_cancellation(indicator_cancel)) as Arc<dyn ProcessingIndicator>);
                    if let Err(error) = pipeline::run_inbound_streaming_with_indicator(
                        process, req, None, renderer, send_fn, stream_handler, indicator,
                    ).await {
                        tracing::error!(%error, "matrix pipeline error");
                    }
                }
            });

                let _event_guard = client.event_handler_drop_guard(event_handle);
                loop {
                    let settings = SyncSettings::default()
                        .timeout(Duration::from_millis(sync_timeout_ms))
                        .token(matrix_sdk::config::SyncToken::from_optional_token(
                            token.clone(),
                        ));
                    match client.sync_once(settings).await {
                        Ok(response) => {
                            token = Some(response.next_batch.clone());
                            if let Err(error) =
                                atomic_write_file(&token_path, response.next_batch.as_bytes()).await
                            {
                                tracing::warn!(%error, "matrix sync token persistence failed");
                            }
                            join_invited_rooms(&client, response.rooms.invited.keys()).await;
                        }
                        Err(error) => {
                            tracing::warn!(%error, "matrix sync retry");
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => {},
                _ = sync => {},
            }
        });

        *self.sync_handle.write().await = Some(handle);
        *started = true;
        tracing::info!("matrix channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut started = self.started.write().await;
        if !*started {
            return Ok(());
        }

        self.sync_cancel.read().await.cancel();
        if let Some(handle) = self.sync_handle.write().await.take() {
            let _ = handle.await;
        }

        if let Ok(mut deduper) = self.event_deduper.lock() {
            deduper.clear();
        }
        *self.client.write().await = None;
        *started = false;
        tracing::info!("matrix channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        let client = self.get_client().await?;

        let room_id_str = if to.conversation_id.is_empty() {
            &self.config.default_room_id
        } else {
            &to.conversation_id
        };

        if room_id_str.is_empty() {
            return Err(rua_core::RuaError::Runtime {
                message: "no target room id and no default_room_id configured".into(),
            });
        }

        let room_id = Self::parse_room_id(room_id_str)?;
        let room = client
            .get_room(&room_id)
            .ok_or_else(|| rua_core::RuaError::Runtime {
                message: format!("room {room_id} not found (not joined?)"),
            })?;

        let mut metadata = to.metadata;
        if let (Some(target), Some(message)) = (metadata.as_object_mut(), msg.metadata.as_object())
        {
            target.extend(message.clone());
        }
        let reply = MatrixReply::from_metadata(&metadata, self.options.clone());
        send_matrix_message(&room, &msg, &reply).await?;

        tracing::debug!(room = %room_id, "matrix message sent");
        Ok(())
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        if let Some(event_id) = native.get("event_id").and_then(|v| v.as_str()) {
            if matrix_check_duplicate_event(&self.event_deduper, event_id)? {
                return Err(rua_core::RuaError::Validation {
                    message: "duplicate matrix event".into(),
                });
            }
        }
        let native = flatten_matrix_event(native);
        matrix_request(&native, &self.config, &self.config.user_id)
    }
}

fn flatten_matrix_event(mut native: serde_json::Value) -> serde_json::Value {
    if let Some(content) = native.get("content").and_then(|v| v.as_object()).cloned() {
        native
            .as_object_mut()
            .expect("event object")
            .extend(content);
    }
    native
}

pub(super) fn matrix_request(
    native: &serde_json::Value,
    config: &MatrixConfig,
    own_user_id: &str,
) -> Result<AgentRequest> {
    let body = native
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let sender = native
        .get("sender")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let room_id = native
        .get("room_id")
        .and_then(|v| v.as_str())
        .unwrap_or(&config.default_room_id)
        .to_string();

    let mut mention = matrix_bot_mention(&body, own_user_id);
    mention.is_bot_mentioned = super::inbound::was_mentioned(native, own_user_id);
    let is_dm = native["is_dm"].as_bool().unwrap_or(false);
    let input = matrix_input_parts(native, &body, &mention);
    let thread = matrix_thread_relation(native);
    let session_id = thread
        .as_ref()
        .map(|thread| format!("matrix_thread:{room_id}:{}", thread.event_id))
        .unwrap_or_else(|| room_id.clone());
    let mut metadata = serde_json::json!({
        "sender_id": sender,
        "room_id": room_id,
        "event_id": native.get("event_id").and_then(|v| v.as_str()),
        "is_group": !is_dm,
        "is_dm": is_dm,
        "bot_mentioned": mention.is_bot_mentioned,
        "mentioned": mention.is_bot_mentioned,
    });
    if let Some(thread) = thread {
        metadata["matrix_rel_type"] = serde_json::json!(thread.rel_type);
        metadata["matrix_thread_root_event_id"] = serde_json::json!(thread.event_id);
    }

    Ok(AgentRequest {
        agent_id: "default".into(),
        user_id: metadata["sender_id"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        session_id,
        input,
        channel_id: "matrix".into(),
        conversation_id: room_id.clone(),
        message_id: native
            .get("event_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        metadata,
    })
}

#[derive(Debug, Default)]
struct MatrixMention {
    is_bot_mentioned: bool,
    bot_user_id: String,
}

#[derive(Debug)]
struct MatrixThreadRelation {
    rel_type: String,
    event_id: String,
}

fn matrix_input_parts(
    native: &serde_json::Value,
    body: &str,
    mention: &MatrixMention,
) -> Vec<IncomingContentPart> {
    if let Some(part) = matrix_local_media_part(native) {
        return vec![part];
    }
    vec![IncomingContentPart::Text {
        text: strip_matrix_bot_mention(body, mention),
    }]
}

fn matrix_bot_mention(body: &str, bot_user_id: &str) -> MatrixMention {
    let bot_user_id = bot_user_id.trim();
    if bot_user_id.is_empty() {
        return MatrixMention::default();
    }
    let is_bot_mentioned = body.split_whitespace().any(|token| {
        token.trim_matches(|ch: char| ch == ',' || ch == ':' || ch == ';') == bot_user_id
    });
    MatrixMention {
        bot_user_id: bot_user_id.to_string(),
        is_bot_mentioned,
    }
}

fn strip_matrix_bot_mention(body: &str, mention: &MatrixMention) -> String {
    if !mention.is_bot_mentioned {
        return body.to_string();
    }
    body.split_whitespace()
        .filter(|token| {
            token.trim_matches(|ch: char| ch == ',' || ch == ':' || ch == ';')
                != mention.bot_user_id
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn matrix_thread_relation(native: &serde_json::Value) -> Option<MatrixThreadRelation> {
    let rel = native
        .get("m.relates_to")
        .or_else(|| native.pointer("/content/m.relates_to"))?;
    let rel_type = rel
        .get("rel_type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if rel_type != "m.thread" {
        return None;
    }
    let event_id = rel
        .get("event_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())?;
    Some(MatrixThreadRelation {
        rel_type: rel_type.to_string(),
        event_id: event_id.to_string(),
    })
}

fn matrix_check_duplicate_event(
    deduper: &Arc<StdMutex<EventDeduper>>,
    event_id: &str,
) -> Result<bool> {
    let event_id = event_id.trim();
    if event_id.is_empty() {
        return Ok(false);
    }
    let mut deduper = deduper.lock().map_err(|_| rua_core::RuaError::Runtime {
        message: "matrix event deduper lock poisoned".into(),
    })?;
    Ok(deduper.check_and_mark(format!("matrix:event:{event_id}")) == EventDedupDecision::Duplicate)
}

fn matrix_local_media_part(native: &serde_json::Value) -> Option<IncomingContentPart> {
    let path = native
        .get("local_path")
        .or_else(|| native.get("path"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)?;
    let file_id = native
        .get("url")
        .or_else(|| native.pointer("/file/url"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let mime = matrix_mime(native);
    let name = native
        .get("body")
        .or_else(|| native.get("filename"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    if native["msgtype"] == "m.image"
        || mime
            .as_deref()
            .is_some_and(|value| value.starts_with("image/"))
    {
        Some(IncomingContentPart::Image {
            file_id,
            path,
            mime,
        })
    } else if native["msgtype"] == "m.audio"
        || mime
            .as_deref()
            .is_some_and(|value| value.starts_with("audio/"))
    {
        Some(IncomingContentPart::Audio {
            file_id,
            path,
            mime,
            name,
        })
    } else if native["msgtype"] == "m.video"
        || mime
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

async fn download_matrix_inbound_media_with(
    homeserver_url: &str,
    access_token: &str,
    media_dir: &str,
    native: &mut serde_json::Value,
) -> Result<()> {
    if native
        .get("local_path")
        .and_then(|value| value.as_str())
        .is_some()
    {
        return Ok(());
    }
    let Some(mxc) = native
        .get("url")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| value.starts_with("mxc://"))
        .map(ToOwned::to_owned)
    else {
        return Ok(());
    };
    let Some((server_name, media_id)) = parse_mxc_uri(&mxc) else {
        return Ok(());
    };
    let Some((bytes, mime)) =
        matrix_download_mxc(homeserver_url, access_token, &server_name, &media_id).await?
    else {
        return Ok(());
    };
    let filename = matrix_download_filename(native, &media_id);
    let dir = matrix_media_dir(media_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("create matrix media dir {}: {err}", dir.display()),
        })?;
    let final_path = dir.join(filename);
    atomic_write_file(&final_path, &bytes).await?;
    if let Some(obj) = native.as_object_mut() {
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

fn parse_mxc_uri(value: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix("mxc://")?;
    let (server_name, media_id) = rest.split_once('/')?;
    if server_name.is_empty() || media_id.is_empty() || media_id.contains('/') {
        return None;
    }
    Some((server_name.to_string(), media_id.to_string()))
}

async fn matrix_download_mxc(
    homeserver_url: &str,
    access_token: &str,
    server_name: &str,
    media_id: &str,
) -> Result<Option<(Vec<u8>, Option<String>)>> {
    let url = format!(
        "{}/_matrix/media/v3/download/{}/{}",
        homeserver_url.trim_end_matches('/'),
        server_name,
        media_id
    );
    let client = default_http_client();
    let mut request = client.get(url).timeout(REQUEST_TIMEOUT);
    if !access_token.trim().is_empty() {
        request = request.bearer_auth(access_token);
    }
    let resp = match request.send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%server_name, %media_id, error = %err, "matrix media download failed");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(%server_name, %media_id, status = %resp.status(), "matrix media download non-success");
        return Ok(None);
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_INBOUND_MEDIA_SIZE {
            tracing::warn!(%server_name, %media_id, len, "matrix media too large");
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
            message: format!("matrix media download bytes failed: {err}"),
        })?;
    if bytes.len() as u64 > MAX_INBOUND_MEDIA_SIZE {
        tracing::warn!(%server_name, %media_id, len = bytes.len(), "matrix media too large");
        return Ok(None);
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some((bytes.to_vec(), mime)))
}

fn matrix_mime(native: &serde_json::Value) -> Option<String> {
    native
        .get("mime")
        .or_else(|| native.pointer("/info/mimetype"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn matrix_media_dir(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    if media_dir.is_empty() {
        std::env::temp_dir().join("rua-matrix-media")
    } else {
        PathBuf::from(media_dir)
    }
}

pub(super) fn matrix_download_filename(native: &serde_json::Value, media_id: &str) -> String {
    let hint = native
        .get("body")
        .or_else(|| native.get("filename"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(media_id);
    crate::core::media::sanitize_filename(hint)
}

pub(super) async fn atomic_write_file(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| rua_core::RuaError::Io {
        message: format!("missing parent dir for {}", path.display()),
    })?;
    let tmp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "matrix-media".into()),
        uuid::Uuid::new_v4()
    ));
    let mut file =
        tokio::fs::File::create(&tmp_path)
            .await
            .map_err(|err| rua_core::RuaError::Io {
                message: format!("create temp media file {}: {err}", tmp_path.display()),
            })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|error| rua_core::RuaError::Io {
                message: error.to_string(),
            })?;
    }
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

async fn send_matrix_message(
    room: &matrix_sdk::Room,
    msg: &OutMessage,
    reply: &MatrixReply,
) -> Result<()> {
    let text = MatrixChannel::render_text_and_unresolved_media(&msg.content);
    if !text.is_empty() && text.trim() != "NO_REPLY" {
        room.send_raw("m.room.message", reply.decorate(matrix_text_event(&text)))
            .await
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("matrix send failed: {e}"),
            })?;
    }
    for part in &msg.content {
        if !MatrixChannel::is_media_part(part) {
            continue;
        }
        if let Err(error) = send_matrix_attachment(room, part, reply).await {
            tracing::warn!(%error, "matrix media send failed");
            let fallback = MatrixChannel::render_content(std::slice::from_ref(part));
            room.send_raw(
                "m.room.message",
                reply.decorate(matrix_text_event(&fallback)),
            )
            .await
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("matrix fallback send failed: {e}"),
            })?;
        }
    }
    Ok(())
}

async fn join_invited_rooms<'a>(client: &Client, rooms: impl Iterator<Item = &'a OwnedRoomId>) {
    for room_id in rooms {
        if let Err(error) = client.join_room_by_id(room_id).await {
            tracing::warn!(%error, %room_id, "matrix auto-join failed");
        }
    }
}
