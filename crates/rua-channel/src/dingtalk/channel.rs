use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;

use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::content::OutgoingContentPart;
use crate::core::event_dedup::{EventDedupDecision, EventDeduper};
use crate::core::pipeline;
use crate::core::renderer::{ChannelKind, MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::streaming::{StreamHandler, StreamType};
use crate::core::{BufferDebouncer, ChannelId};
use crate::dingtalk::ai_card::DingTalkCardSink;
use crate::dingtalk::emotion;
use crate::dingtalk::token::AccessTokenManager;
use crate::http_client::default_http_client;
use rua_core::Result;

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(500);
const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;

use crate::dingtalk::parser;
use crate::dingtalk::sender::DingTalkSender;
use crate::dingtalk::ws;

pub struct DingTalkChannel {
    channel_id: ChannelId,
    sender: DingTalkSender,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    enable_ws: bool,
    client_id: String,
    client_secret: String,
    bot_id: String,
    bot_name: String,
    robot_code: String,
    ai_card_enabled: bool,
    card_template_id: String,
    card_template_key: String,
    ws_stop_tx: watch::Sender<bool>,
    ws_handle: RwLock<Option<JoinHandle<()>>>,
    render_options: MessageRenderOptions,
    debouncer: BufferDebouncer<String, AgentRequest>,
    token_manager: Arc<AccessTokenManager>,
    acl_metadata: serde_json::Value,
    media_dir: String,
    open_api_base_url: String,
    upload_api_base_url: String,
    event_deduper: Arc<StdMutex<EventDeduper>>,
    routes: Option<Arc<super::routes::Routes>>,
}

impl DingTalkChannel {
    pub fn new() -> Self {
        let (ws_stop_tx, _) = watch::channel(false);
        let render_options = MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk);
        Self {
            channel_id: "dingtalk".into(),
            routes: None,
            sender: DingTalkSender::new(),
            process: RwLock::new(None),
            started: RwLock::new(false),
            enable_ws: true,
            client_id: String::new(),
            client_secret: String::new(),
            bot_id: String::new(),
            bot_name: String::new(),
            robot_code: String::new(),
            ai_card_enabled: false,
            card_template_id: String::new(),
            card_template_key: String::new(),
            ws_stop_tx,
            ws_handle: RwLock::new(None),
            render_options,
            debouncer: BufferDebouncer::new(DEBOUNCE_WINDOW),
            token_manager: Arc::new(AccessTokenManager::new(String::new(), String::new())),
            acl_metadata: serde_json::json!({}),
            media_dir: String::new(),
            open_api_base_url: "https://api.dingtalk.com".into(),
            upload_api_base_url: "https://oapi.dingtalk.com".into(),
            event_deduper: Arc::new(StdMutex::new(EventDeduper::new(
                EVENT_DEDUP_TTL,
                EVENT_DEDUP_CAPACITY,
            ))),
        }
    }

    pub fn debouncer(&self) -> &BufferDebouncer<String, AgentRequest> {
        &self.debouncer
    }

    pub fn render_options(&self) -> &MessageRenderOptions {
        &self.render_options
    }

    pub fn with_sender(mut self, sender: DingTalkSender) -> Self {
        self.sender = sender;
        self
    }

    pub fn with_token_manager(mut self, token_manager: Arc<AccessTokenManager>) -> Self {
        self.token_manager = token_manager;
        self
    }

    pub fn with_ws_config(
        mut self,
        client_id: String,
        client_secret: String,
        enable_ws: bool,
    ) -> Self {
        self.token_manager = Arc::new(AccessTokenManager::new(
            client_id.clone(),
            client_secret.clone(),
        ));
        self.client_id = client_id;
        self.client_secret = client_secret;
        self.enable_ws = enable_ws;
        self.load_routes();
        self
    }

    pub fn with_bot_config(mut self, bot_id: String, bot_name: String) -> Self {
        self.bot_id = bot_id;
        self.bot_name = bot_name;
        self
    }

    pub fn with_robot_code(mut self, robot_code: String) -> Self {
        self.robot_code = robot_code;
        self
    }

    pub fn with_media_dir(mut self, media_dir: String) -> Self {
        self.media_dir = media_dir;
        self.load_routes();
        self
    }

    fn load_routes(&mut self) {
        if !self.client_id.is_empty() {
            let id = crate::core::media::sanitize_filename(&self.client_id);
            self.routes = Some(Arc::new(super::routes::Routes::new(
                dingtalk_media_dir(&self.media_dir).join(format!("routes-{id}.json")),
            )));
        }
    }

    fn remember_route(&self, req: &AgentRequest) {
        if let Some(routes) = &self.routes {
            if let Err(error) = routes.remember(req) {
                tracing::warn!(%error, "DingTalk target persistence failed");
            }
        }
    }

    pub fn with_open_api_base_url(mut self, base_url: String) -> Self {
        self.open_api_base_url = base_url.trim_end_matches('/').to_string();
        self
    }

    pub fn with_upload_api_base_url(mut self, base_url: String) -> Self {
        self.upload_api_base_url = base_url.trim_end_matches('/').to_string();
        self
    }

    pub async fn download_inbound_media_with_token(
        &self,
        native: &mut serde_json::Value,
        token: &str,
    ) -> Result<()> {
        download_dingtalk_inbound_media_with(
            &self.open_api_base_url,
            token,
            &self.robot_code,
            &self.media_dir,
            native,
        )
        .await
    }

    pub fn with_card_config(
        mut self,
        ai_card_enabled: bool,
        card_template_id: String,
        card_template_key: String,
    ) -> Self {
        self.ai_card_enabled = ai_card_enabled;
        self.card_template_id = card_template_id;
        self.card_template_key = card_template_key;
        self
    }

    fn card_enabled(&self) -> bool {
        self.ai_card_enabled
            && !self.card_template_id.is_empty()
            && !self.card_template_key.is_empty()
            && !self.robot_code.is_empty()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_acl_config(
        mut self,
        dm_policy: String,
        group_policy: String,
        require_mention: bool,
        allow_from: Vec<String>,
        deny_from: Vec<String>,
        access_control_dm: bool,
        access_control_group: bool,
        language: String,
    ) -> Self {
        self.acl_metadata = serde_json::json!({
            "dm_policy": dm_policy,
            "group_policy": group_policy,
            "require_mention": require_mention,
            "allow_from": allow_from,
            "deny_from": deny_from,
            "access_control_dm": access_control_dm,
            "access_control_group": access_control_group,
            "language": language,
        });
        self
    }

    pub fn with_channel_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.acl_metadata = metadata;
        self
    }

    pub fn with_endpoint(mut self, endpoint: String) -> Self {
        if !endpoint.trim().is_empty() {
            let endpoint = endpoint.trim().trim_end_matches('/').to_string();
            self.open_api_base_url = endpoint.clone();
            self.upload_api_base_url = endpoint.clone();
            self.token_manager = Arc::new(
                AccessTokenManager::new(self.client_id.clone(), self.client_secret.clone())
                    .with_token_url(format!("{endpoint}/v1.0/oauth2/accessToken")),
            );
        }
        self
    }

    pub fn with_render_options(
        mut self,
        options: crate::core::renderer::MessageRenderOptions,
    ) -> Self {
        self.render_options = options;
        self
    }

    pub fn sender(&self) -> &DingTalkSender {
        &self.sender
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
                        let label = name.as_deref().unwrap_or("file");
                        segments.push(format!("[File: {label}]"));
                    }
                }
                OutgoingContentPart::Audio { url, path, name } => {
                    if url.is_none() && path.is_none() {
                        let label = name.as_deref().unwrap_or("audio");
                        segments.push(format!("[Audio: {label}]"));
                    }
                }
                OutgoingContentPart::Video { url, path, name } => {
                    if url.is_none() && path.is_none() {
                        let label = name.as_deref().unwrap_or("video");
                        segments.push(format!("[Video: {label}]"));
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

    async fn send_open_api(&self, to: &ToHandle, text: &str) -> Result<()> {
        if self.robot_code.is_empty() {
            return Err(rua_core::RuaError::Validation {
                message: "missing session_webhook or robot_code for DingTalk reply".into(),
            });
        }
        let (key, param) = if text.chars().count() > 3500 {
            ("sampleText", serde_json::json!({"content":text}))
        } else {
            (
                "sampleMarkdown",
                serde_json::json!({"title":"Reply","text":text}),
            )
        };
        self.send_open_api_message(to, key, param).await
    }

    async fn send_open_api_message(
        &self,
        to: &ToHandle,
        key: &str,
        param: serde_json::Value,
    ) -> Result<()> {
        let mut body = serde_json::json!({"robotCode":self.robot_code,"msgKey":key,"msgParam":param.to_string()});
        let path = if is_group_target(to) {
            body["openConversationId"] = serde_json::json!(to.conversation_id);
            "groupMessages/send"
        } else {
            let staff = to.metadata["sender_staff_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or(to.sender_id.as_deref())
                .ok_or_else(|| rua_core::RuaError::Validation {
                    message: "missing DingTalk staff ID".into(),
                })?;
            body["userIds"] = serde_json::json!([staff]);
            "oToMessages/batchSend"
        };
        let token = self.token_manager.current_token().await?;
        let response = default_http_client()
            .post(format!("{}/v1.0/robot/{path}", self.open_api_base_url))
            .header("x-acs-dingtalk-access-token", token)
            .json(&body)
            .send()
            .await
            .map_err(|error| rua_core::RuaError::Runtime {
                message: format!("DingTalk robot send: {error}"),
            })?;
        if !response.status().is_success() {
            return Err(rua_core::RuaError::Runtime {
                message: format!("DingTalk robot send returned {}", response.status()),
            });
        }
        Ok(())
    }

    async fn send_open_api_media(&self, to: &ToHandle, part: &OutgoingContentPart) -> Result<bool> {
        let (path, url, name, kind) = match part {
            OutgoingContentPart::Image { path, url, alt } => (path, url, alt, "image"),
            OutgoingContentPart::File { path, url, name }
            | OutgoingContentPart::Audio { path, url, name }
            | OutgoingContentPart::Video { path, url, name } => (path, url, name, "file"),
            _ => return Ok(false),
        };
        if kind == "image" {
            if let Some(url) = url.as_deref().filter(is_public_http_url) {
                self.send_open_api_message(
                    to,
                    "sampleImageMsg",
                    serde_json::json!({"photoURL":url}),
                )
                .await?;
                return Ok(true);
            }
        }
        let client = default_http_client();
        let loaded = if let Some(path) = path {
            Some((
                path.clone(),
                name.clone().unwrap_or_else(|| {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                }),
            ))
        } else {
            crate::core::media::resolve_outgoing_media(part, &client)
                .await?
                .map(|media| (media.path, media.name))
        };
        let Some((path, filename)) = loaded else {
            return Ok(false);
        };
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|error| rua_core::RuaError::Io {
                message: format!("DingTalk media file {}: {error}", path.display()),
            })?;
        let token = self.token_manager.current_token().await?;
        let media_id = crate::dingtalk::renderer::upload_media_with_base_url(
            &client,
            &self.upload_api_base_url,
            &token,
            &bytes,
            kind,
            Some(&filename),
        )
        .await?;
        let extension = std::path::Path::new(&filename)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin");
        // DingTalk's image template accepts a URL, not an uploaded media ID.
        // QwenPaw sends uploaded media through sampleFile, including images.
        self.send_open_api_message(
            to,
            "sampleFile",
            serde_json::json!({
                "mediaId":media_id, "fileName":filename, "fileType":extension,
            }),
        )
        .await?;
        Ok(true)
    }

    async fn send_media_part(&self, webhook_url: &str, part: &OutgoingContentPart) -> Result<bool> {
        match part {
            OutgoingContentPart::Image { url, path, alt } => {
                if let Some(url) = url.as_deref().filter(is_public_http_url) {
                    let body = serde_json::json!({
                        "msgtype": "image",
                        "image": { "picURL": url }
                    });
                    self.sender.send_to_webhook(webhook_url, body).await?;
                    return Ok(true);
                }
                if let Some(path) = path {
                    return self
                        .upload_and_send_file_like(webhook_url, path, "image", alt.as_deref())
                        .await;
                }
                return Ok(false);
            }
            OutgoingContentPart::File { path, name, .. } => {
                if let Some(path) = path {
                    return self
                        .upload_and_send_file_like(webhook_url, path, "file", name.as_deref())
                        .await;
                }
                return Ok(false);
            }
            OutgoingContentPart::Audio { path, name, .. } => {
                if let Some(path) = path {
                    return self
                        .upload_and_send_file_like(webhook_url, path, "file", name.as_deref())
                        .await;
                }
                return Ok(false);
            }
            OutgoingContentPart::Video { path, name, .. } => {
                if let Some(path) = path {
                    return self
                        .upload_and_send_file_like(webhook_url, path, "file", name.as_deref())
                        .await;
                }
                return Ok(false);
            }
            _ => {}
        }
        Ok(false)
    }

    async fn upload_and_send_file_like(
        &self,
        webhook_url: &str,
        path: &std::path::Path,
        upload_type: &str,
        name: Option<&str>,
    ) -> Result<bool> {
        let token = match self.token_manager.current_token().await {
            Ok(token) => token,
            Err(err) => {
                tracing::warn!(error = %err, "dingtalk media token unavailable");
                return Ok(false);
            }
        };
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "dingtalk media file read failed");
                return Ok(false);
            }
        };
        let filename = name
            .filter(|value| !value.trim().is_empty())
            .or_else(|| path.file_name().and_then(|value| value.to_str()));
        let media_id = match crate::dingtalk::renderer::upload_media_with_base_url(
            &default_http_client(),
            &self.upload_api_base_url,
            &token,
            &bytes,
            upload_type,
            filename,
        )
        .await
        {
            Ok(media_id) => media_id,
            Err(err) => {
                tracing::warn!(error = %err, "dingtalk media upload failed");
                return Ok(false);
            }
        };
        let body = if upload_type == "image" {
            DingTalkSender::build_markdown_image_body(
                "Image",
                filename.unwrap_or("image"),
                &media_id,
            )
        } else {
            DingTalkSender::build_file_body(&media_id)
        };
        self.sender.send_to_webhook(webhook_url, body).await?;
        Ok(true)
    }
}

fn is_public_http_url(url: &&str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
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

fn has_media_source(part: &OutgoingContentPart) -> bool {
    match part {
        OutgoingContentPart::Image { url, path, .. }
        | OutgoingContentPart::File { url, path, .. }
        | OutgoingContentPart::Audio { url, path, .. }
        | OutgoingContentPart::Video { url, path, .. } => url.is_some() || path.is_some(),
        _ => false,
    }
}

async fn download_dingtalk_inbound_media_with(
    open_api_base_url: &str,
    token: &str,
    robot_code: &str,
    media_dir: &str,
    native: &mut serde_json::Value,
) -> Result<()> {
    if robot_code.trim().is_empty() {
        return Ok(());
    }
    let msg_type = native
        .get("msgType")
        .or_else(|| native.get("msg_type"))
        .and_then(|value| value.as_str())
        .unwrap_or("file")
        .to_string();
    if let Some(content) = native.get_mut("content") {
        download_dingtalk_content_media(
            open_api_base_url,
            token,
            robot_code,
            media_dir,
            &msg_type,
            content,
        )
        .await?;
    }
    Ok(())
}

async fn download_dingtalk_content_media(
    open_api_base_url: &str,
    token: &str,
    robot_code: &str,
    media_dir: &str,
    msg_type: &str,
    content: &mut serde_json::Value,
) -> Result<()> {
    {
        let items = if content.get("richText").is_some() {
            content
                .get_mut("richText")
                .and_then(|value| value.as_array_mut())
        } else {
            content
                .get_mut("rich_text")
                .and_then(|value| value.as_array_mut())
        };
        if let Some(items) = items {
            for item in items {
                let kind = item
                    .get("type")
                    .or_else(|| item.get("msgType"))
                    .and_then(|value| value.as_str())
                    .unwrap_or(msg_type)
                    .to_string();
                download_dingtalk_media_object(
                    open_api_base_url,
                    token,
                    robot_code,
                    media_dir,
                    &kind,
                    item,
                )
                .await?;
            }
        }
    }
    download_dingtalk_media_object(
        open_api_base_url,
        token,
        robot_code,
        media_dir,
        msg_type,
        content,
    )
    .await
}

async fn download_dingtalk_media_object(
    open_api_base_url: &str,
    token: &str,
    robot_code: &str,
    media_dir: &str,
    kind: &str,
    value: &mut serde_json::Value,
) -> Result<()> {
    if value
        .get("local_path")
        .and_then(|value| value.as_str())
        .is_some()
    {
        return Ok(());
    }
    let Some(download_code) = value
        .get("downloadCode")
        .or_else(|| value.get("download_code"))
        .or_else(|| value.get("pictureDownloadCode"))
        .or_else(|| value.get("picture_download_code"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
    else {
        return Ok(());
    };
    let Some(download_url) = dingtalk_get_message_file_download_url(
        open_api_base_url,
        token,
        robot_code,
        &download_code,
    )
    .await?
    else {
        return Ok(());
    };
    let Some((bytes, mime)) = dingtalk_download_url(&download_url).await? else {
        return Ok(());
    };
    let filename = dingtalk_download_filename(value, &download_code, kind, mime.as_deref());
    let dir = dingtalk_media_dir(media_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("create dingtalk media dir {}: {err}", dir.display()),
        })?;
    let final_path = dir.join(filename);
    atomic_write_file(&final_path, &bytes).await?;
    if let Some(obj) = value.as_object_mut() {
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

async fn dingtalk_get_message_file_download_url(
    open_api_base_url: &str,
    token: &str,
    robot_code: &str,
    download_code: &str,
) -> Result<Option<String>> {
    let url = format!(
        "{}/v1.0/robot/messageFiles/download",
        open_api_base_url.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "downloadCode": download_code,
        "robotCode": robot_code,
    });
    let resp = match default_http_client()
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%download_code, error = %err, "dingtalk messageFiles/download failed");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(%download_code, status = %resp.status(), "dingtalk messageFiles/download non-success");
        return Ok(None);
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("dingtalk messageFiles/download JSON parse failed: {err}"),
        })?;
    if body
        .get("errcode")
        .and_then(|value| value.as_i64())
        .unwrap_or(0)
        != 0
    {
        return Ok(None);
    }
    Ok(body
        .get("downloadUrl")
        .or_else(|| body.get("download_url"))
        .or_else(|| body.pointer("/body/downloadUrl"))
        .or_else(|| body.pointer("/body/download_url"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned))
}

async fn dingtalk_download_url(url: &str) -> Result<Option<(Vec<u8>, Option<String>)>> {
    let resp = match default_http_client()
        .get(url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%url, error = %err, "dingtalk media download failed");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(%url, status = %resp.status(), "dingtalk media download non-success");
        return Ok(None);
    }
    if let Some(len) = resp.content_length() {
        if len > 50 * 1024 * 1024 {
            tracing::warn!(%url, len, "dingtalk media too large");
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
            message: format!("dingtalk media download bytes failed: {err}"),
        })?;
    if bytes.len() > 50 * 1024 * 1024 {
        tracing::warn!(%url, len = bytes.len(), "dingtalk media too large");
        return Ok(None);
    }
    Ok(Some((bytes.to_vec(), mime)))
}

fn dingtalk_media_dir(media_dir: &str) -> PathBuf {
    let media_dir = media_dir.trim();
    if media_dir.is_empty() {
        std::env::temp_dir().join("rua-dingtalk-media")
    } else {
        PathBuf::from(media_dir)
    }
}

fn dingtalk_download_filename(
    value: &serde_json::Value,
    download_code: &str,
    kind: &str,
    mime: Option<&str>,
) -> String {
    let hint = value
        .get("fileName")
        .or_else(|| value.get("file_name"))
        .or_else(|| value.get("filename"))
        .or_else(|| value.get("name"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            let ext = match mime.unwrap_or("") {
                "image/jpeg" => "jpg",
                "image/png" => "png",
                "application/pdf" => "pdf",
                "audio/amr" => "amr",
                _ => match kind {
                    "picture" | "image" => "jpg",
                    "voice" | "audio" => "amr",
                    "video" => "mp4",
                    _ => "bin",
                },
            };
            format!("{download_code}.{ext}")
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
            .unwrap_or_else(|| "dingtalk-media".into()),
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

impl Default for DingTalkChannel {
    fn default() -> Self {
        Self::new()
    }
}

fn dingtalk_check_duplicate_request(
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
        message: "dingtalk event deduper lock poisoned".into(),
    })?;
    Ok(
        deduper.check_and_mark(format!("dingtalk:message:{message_id}"))
            == EventDedupDecision::Duplicate,
    )
}

#[async_trait]
impl channel::Channel for DingTalkChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("dingtalk channel already started");
            return Ok(());
        }
        *self.process.write().await = Some(process.clone());
        *started = true;

        self.token_manager.start().await;

        let ws_config = ws::WsConfig {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            enable_ws: self.enable_ws,
        };
        let process_for_ws = process;
        let bot_id = self.bot_id.clone();
        let bot_name = self.bot_name.clone();
        let debouncer = self.debouncer.clone();
        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::Markdown,
            self.render_options.clone(),
        ));
        let outbound = Arc::new(
            Self::new()
                .with_sender(self.sender.clone())
                .with_token_manager(self.token_manager.clone())
                .with_robot_code(self.robot_code.clone())
                .with_media_dir(self.media_dir.clone())
                .with_open_api_base_url(self.open_api_base_url.clone())
                .with_upload_api_base_url(self.upload_api_base_url.clone())
                .with_channel_metadata(self.acl_metadata.clone()),
        );
        let send_fn: pipeline::SendFn = Arc::new(move |mut to, msg| {
            let outbound = outbound.clone();
            // Live card rendering is owned by the stream sink; this path sends its media and text fallback.
            to.metadata["cron_message_type"] = serde_json::json!("markdown");
            Box::pin(async move { channel::Channel::send(outbound.as_ref(), to, msg).await })
        });
        let robot_code = if self.robot_code.is_empty() {
            self.client_id.clone()
        } else {
            self.robot_code.clone()
        };
        let token_mgr = self.token_manager.clone();
        let card_enabled = self.card_enabled();
        let card_template_id = self.card_template_id.clone();
        let card_template_key = self.card_template_key.clone();
        let acl_metadata = Arc::new(self.acl_metadata.clone());
        let inbound_open_api_base_url = self.open_api_base_url.clone();
        let inbound_media_dir = self.media_dir.clone();
        let event_deduper = self.event_deduper.clone();
        let routes = self.routes.clone();
        let on_message: ws::OnMessageFn = Arc::new(move |native| {
            let p = process_for_ws.clone();
            let bid = bot_id.clone();
            let bname = bot_name.clone();
            let d = debouncer.clone();
            let r = renderer.clone();
            let sf = send_fn.clone();
            let rc = robot_code.clone();
            let tm = token_mgr.clone();
            let ce = card_enabled;
            let cti = card_template_id.clone();
            let ctk = card_template_key.clone();
            let acl = acl_metadata.clone();
            let open_api_base_url = inbound_open_api_base_url.clone();
            let media_dir = inbound_media_dir.clone();
            let event_deduper = event_deduper.clone();
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut native = native;
                match tm.current_token().await {
                    Ok(token) => {
                        if let Err(err) = download_dingtalk_inbound_media_with(
                            &open_api_base_url,
                            &token,
                            &rc,
                            &media_dir,
                            &mut native,
                        )
                        .await
                        {
                            tracing::warn!(error = %err, "dingtalk inbound media download failed; using placeholders");
                        }
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "dingtalk token fetch failed before inbound media download; using placeholders");
                    }
                }

                let sender_staff_id = native
                    .get("senderStaffId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                match crate::dingtalk::parser::parse_with_routing(native, &bid, &bname) {
                    Ok(Some(req)) => {
                        let mut req = req;
                        match dingtalk_check_duplicate_request(&event_deduper, &req) {
                            Ok(true) => {
                                tracing::debug!(
                                    message_id = ?req.message_id,
                                    "dingtalk duplicate event skipped"
                                );
                                return;
                            }
                            Ok(false) => {}
                            Err(err) => {
                                tracing::warn!(error = %err, "dingtalk event dedup failed");
                            }
                        }
                        if let (Some(acl_obj), Some(meta)) =
                            (acl.as_object(), req.metadata.as_object_mut())
                        {
                            for (k, v) in acl_obj {
                                meta.insert(k.clone(), v.clone());
                            }
                        }
                        req.metadata["sender_staff_id"] = serde_json::json!(sender_staff_id);
                        if let Some(routes) = routes {
                            if let Err(error) = routes.remember(&req) {
                                tracing::warn!(%error, "DingTalk target persistence failed");
                            }
                        }
                        let msg_id = req.message_id.clone().unwrap_or_default();
                        let conv_id = req.conversation_id.clone();
                        let is_group = req
                            .metadata
                            .get("conversation_type")
                            .and_then(|v| v.as_str())
                            == Some("2");

                        let use_card = ce && (is_group || !sender_staff_id.is_empty());

                        if use_card {
                            let sink = Arc::new(
                                DingTalkCardSink::new(
                                    tm.clone(),
                                    rc.clone(),
                                    cti.clone(),
                                    ctk.clone(),
                                    conv_id.clone(),
                                    sender_staff_id.clone(),
                                    is_group,
                                )
                                .with_api_base(open_api_base_url.clone())
                                .with_options(
                                    acl["card_auto_layout"] == true,
                                    acl["at_sender_on_reply"] == true,
                                ),
                            );
                            let sender_key = req.user_id.clone();
                            let result = pipeline::run_inbound_streaming(
                                p,
                                req,
                                Some((d, sender_key)),
                                r,
                                sf,
                                Some(sink),
                            )
                            .await;
                            if let Err(e) = result {
                                tracing::error!("dingtalk pipeline error: {e}");
                            }
                            return;
                        }

                        if ce && !is_group && sender_staff_id.is_empty() {
                            tracing::warn!(
                                conversation_id = %conv_id,
                                "dingtalk AI card: missing senderStaffId for p2p, \
                                 falling back to text reply"
                            );
                        }

                        let emotion_reply_url = format!(
                            "{}/v1.0/robot/emotion/reply",
                            open_api_base_url.trim_end_matches('/')
                        );
                        let emotion_recall_url = format!(
                            "{}/v1.0/robot/emotion/recall",
                            open_api_base_url.trim_end_matches('/')
                        );
                        let emotion_urls = emotion::EmotionEndpointUrls {
                            reply: &emotion_reply_url,
                            recall: &emotion_recall_url,
                        };
                        if !msg_id.is_empty() && !conv_id.is_empty() && !rc.is_empty() {
                            emotion::send_emotion_with_urls(
                                &tm,
                                &rc,
                                &msg_id,
                                &conv_id,
                                "🤔Thinking",
                                false,
                                emotion_urls,
                            )
                            .await;
                        }

                        let sender_key = req.user_id.clone();
                        let result =
                            pipeline::run_inbound(p, req, Some((d, sender_key)), r, sf).await;

                        if !msg_id.is_empty() && !conv_id.is_empty() && !rc.is_empty() {
                            emotion::send_emotion_with_urls(
                                &tm,
                                &rc,
                                &msg_id,
                                &conv_id,
                                "🤔Thinking",
                                true,
                                emotion_urls,
                            )
                            .await;
                            if result.is_ok() {
                                emotion::send_emotion_with_urls(
                                    &tm,
                                    &rc,
                                    &msg_id,
                                    &conv_id,
                                    "🥳Done",
                                    false,
                                    emotion_urls,
                                )
                                .await;
                            }
                        }

                        if let Err(e) = result {
                            tracing::error!("dingtalk pipeline error: {e}");
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::error!("dingtalk WS: parse failed: {e}");
                    }
                }
            });
        });
        self.ws_stop_tx.send_replace(false);
        let stop_rx = self.ws_stop_tx.subscribe();
        if let Some(handle) = ws::spawn_ws_task_with_endpoint(
            ws_config,
            on_message,
            stop_rx,
            format!(
                "{}/v1.0/gateway/connections/open",
                self.open_api_base_url.trim_end_matches('/')
            ),
        ) {
            *self.ws_handle.write().await = Some(handle);
        }

        tracing::info!("dingtalk channel started");
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

        self.token_manager.stop().await;

        if let Ok(mut deduper) = self.event_deduper.lock() {
            deduper.clear();
        }
        *self.process.write().await = None;
        *started = false;
        tracing::info!("dingtalk channel stopped");
        Ok(())
    }

    async fn send(&self, mut to: ToHandle, msg: OutMessage) -> Result<()> {
        if let Some(routes) = &self.routes {
            routes.restore(&mut to);
        }
        if !to.metadata.is_object() {
            to.metadata = serde_json::json!({});
        }
        for key in [
            "cron_message_type",
            "card_auto_layout",
            "at_sender_on_reply",
        ] {
            if to.metadata.get(key).is_none() {
                if let Some(value) = self.acl_metadata.get(key) {
                    to.metadata[key] = value.clone();
                }
            }
        }
        let text = Self::render_text_and_unresolved_media(&msg.content);
        let use_card = to.metadata["cron_message_type"] == "card"
            && !text.is_empty()
            && !self.card_template_id.is_empty()
            && !self.robot_code.is_empty();
        if use_card {
            let is_group = is_group_target(&to);
            let staff = to.metadata["sender_staff_id"]
                .as_str()
                .or(to.sender_id.as_deref())
                .unwrap_or_default();
            let sink = DingTalkCardSink::new(
                self.token_manager.clone(),
                self.robot_code.clone(),
                self.card_template_id.clone(),
                self.card_template_key.clone(),
                to.conversation_id.clone(),
                staff.into(),
                is_group,
            )
            .with_api_base(self.open_api_base_url.clone())
            .with_options(
                to.metadata["card_auto_layout"] == true,
                to.metadata["at_sender_on_reply"] == true,
            );
            sink.on_start(StreamType::Message).await?;
            sink.on_end(StreamType::Message, &text).await?;
            if !msg.content.iter().any(is_media_part) {
                return Ok(());
            }
        }
        let webhook_url = to.metadata["session_webhook"]
            .as_str()
            .filter(|url| !url.is_empty());
        if !text.is_empty() && !use_card {
            let delivered = if let Some(webhook_url) = webhook_url {
                let mut body = DingTalkSender::build_markdown_body("Reply", &text);
                add_sender_mention(&mut body, &to.metadata);
                match self.sender.send_to_webhook(webhook_url, body).await {
                    Ok(()) => true,
                    Err(error) if !self.robot_code.is_empty() => {
                        tracing::warn!(%error,"DingTalk session webhook failed; trying robot API");
                        false
                    }
                    Err(error) => return Err(error),
                }
            } else {
                false
            };
            if !delivered {
                self.send_open_api(&to, &text).await?;
            }
        }
        if !msg.content.iter().any(is_media_part) {
            return Ok(());
        }
        for part in &msg.content {
            if !has_media_source(part) {
                continue;
            }
            let mut sent = false;
            if let Some(webhook_url) = webhook_url {
                match self.send_media_part(webhook_url, part).await {
                    Ok(delivered) => sent = delivered,
                    Err(error) if !self.robot_code.is_empty() => {
                        tracing::warn!(%error, "DingTalk media webhook failed; trying robot API");
                    }
                    Err(error) => return Err(error),
                }
            }
            if !sent && !self.robot_code.is_empty() {
                sent = self.send_open_api_media(&to, part).await?;
            }
            if !sent {
                let fallback = Self::render_content(std::slice::from_ref(part));
                if !fallback.is_empty() {
                    if let Some(webhook_url) = webhook_url {
                        let body = DingTalkSender::build_markdown_body("Reply", &fallback);
                        self.sender.send_to_webhook(webhook_url, body).await?;
                    } else {
                        self.send_open_api(&to, &fallback).await?;
                    }
                }
            }
        }
        Ok(())
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        let req = parser::parse_native(native)?;
        if dingtalk_check_duplicate_request(&self.event_deduper, &req)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate dingtalk event".into(),
            });
        }
        self.remember_route(&req);
        Ok(req)
    }
}

fn is_group_target(to: &ToHandle) -> bool {
    if let Some(group) = to.metadata["is_group"].as_bool() {
        return group;
    }
    match to.metadata["conversation_type"].as_str() {
        Some("2" | "group") => true,
        Some("1" | "p2p" | "private") => false,
        _ => to.conversation_id.starts_with("cid"),
    }
}

fn add_sender_mention(body: &mut serde_json::Value, metadata: &serde_json::Value) {
    let group = metadata["is_group"] == true || metadata["conversation_type"] == "2";
    if metadata["at_sender_on_reply"] == true && group {
        if let Some(staff) = metadata["sender_staff_id"]
            .as_str()
            .filter(|s| !s.is_empty())
        {
            body["at"] = serde_json::json!({"atUserIds":[staff],"isAtAll":false});
        }
    }
}
