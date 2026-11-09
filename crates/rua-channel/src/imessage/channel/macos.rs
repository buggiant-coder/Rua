use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use crate::core::pipeline;
use crate::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use async_trait::async_trait;
use base64::Engine;
use rusqlite::Connection;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::content::{AgentChannelRequest, IncomingContentPart, OutgoingContentPart};
use crate::core::ChannelId;
use crate::http_client::default_http_client;
use rua_core::{Result, RuaError};

use crate::imessage::config::IMessageConfig;

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

pub struct IMessageChannel {
    channel_id: ChannelId,
    config: IMessageConfig,
    db_path: PathBuf,
    imsg_path: RwLock<Option<PathBuf>>,
    poll_task: Mutex<Option<(CancellationToken, tokio::task::JoinHandle<()>)>>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
}

impl IMessageChannel {
    pub fn new(config: IMessageConfig) -> Self {
        let db_path = expand_tilde(&config.db_path);
        Self {
            channel_id: "imessage".into(),
            config,
            db_path,
            imsg_path: RwLock::new(None),
            poll_task: Mutex::new(None),
            render_options: MessageRenderOptions::default(),
            acl_metadata: serde_json::json!({}),
        }
    }

    pub fn with_acl_config(mut self, metadata: serde_json::Value) -> Self {
        self.acl_metadata = metadata;
        self
    }

    pub fn with_render_options(
        mut self,
        options: crate::core::renderer::MessageRenderOptions,
    ) -> Self {
        self.render_options = options;
        self
    }

    /// Override the imsg executable, for packaged installations and local transport tests.
    pub fn with_imsg_path(self, path: PathBuf) -> Self {
        Self {
            imsg_path: RwLock::new(Some(path)),
            ..self
        }
    }

    fn find_imsg() -> Result<PathBuf> {
        let output = Command::new("which")
            .arg("imsg")
            .output()
            .map_err(|e| RuaError::Runtime {
                message: format!("failed to locate imsg binary: {e}"),
            })?;
        if !output.status.success() {
            return Err(RuaError::Runtime {
                message: "imsg not found. Install with: brew install steipete/tap/imsg".into(),
            });
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(PathBuf::from(path))
    }

    async fn send_via_imsg(imsg_path: &std::path::Path, to: &str, text: &str) -> Result<()> {
        let mut cmd = tokio::process::Command::new(imsg_path);
        cmd.kill_on_drop(true);
        cmd.arg("send").arg("--to").arg(to);
        if !text.is_empty() {
            cmd.arg("--text").arg(text);
        }
        let output = cmd.output().await.map_err(|e| RuaError::Runtime {
            message: format!("failed to execute imsg send: {e}"),
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RuaError::Runtime {
                message: format!(
                    "imsg send failed (code {:?}): {}",
                    output.status.code(),
                    stderr.trim()
                ),
            });
        }
        Ok(())
    }

    async fn send_file_via_imsg(
        imsg_path: &std::path::Path,
        to: &str,
        file_path: &std::path::Path,
    ) -> Result<()> {
        let output = tokio::process::Command::new(imsg_path)
            .kill_on_drop(true)
            .arg("send")
            .arg("--to")
            .arg(to)
            .arg("--file")
            .arg(file_path)
            .output()
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("failed to execute imsg send --file: {e}"),
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RuaError::Runtime {
                message: format!(
                    "imsg send --file failed (code {:?}): {}",
                    output.status.code(),
                    stderr.trim()
                ),
            });
        }
        Ok(())
    }

    fn is_sender_allowed(&self, sender: &str) -> bool {
        if self.config.allowed_senders.is_empty() {
            return true;
        }
        self.config.allowed_senders.iter().any(|s| s == sender)
    }

    pub fn render_text_and_unresolved_media(parts: &[OutgoingContentPart]) -> String {
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

    pub fn local_media_path(part: &OutgoingContentPart) -> Option<&std::path::Path> {
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
            } => Some(path.as_path()),
            _ => None,
        }
    }

    fn media_dir(&self) -> PathBuf {
        if self.config.media_dir.trim().is_empty() {
            std::env::temp_dir().join("rua-imessage-media")
        } else {
            expand_tilde(&self.config.media_dir)
        }
    }

    fn media_url_and_name(
        part: &OutgoingContentPart,
    ) -> (Option<&str>, Option<&std::path::Path>, &str) {
        match part {
            OutgoingContentPart::Image { url, path, alt } => (
                url.as_deref(),
                path.as_deref(),
                alt.as_deref().unwrap_or("image"),
            ),
            OutgoingContentPart::File { url, path, name } => (
                url.as_deref(),
                path.as_deref(),
                name.as_deref().unwrap_or("file"),
            ),
            OutgoingContentPart::Audio { url, path, name } => (
                url.as_deref(),
                path.as_deref(),
                name.as_deref().unwrap_or("audio"),
            ),
            OutgoingContentPart::Video { url, path, name } => (
                url.as_deref(),
                path.as_deref(),
                name.as_deref().unwrap_or("video"),
            ),
            _ => (None, None, "media"),
        }
    }

    pub async fn resolve_media_path(&self, part: &OutgoingContentPart) -> Result<Option<PathBuf>> {
        if let Some(path) = Self::local_media_path(part) {
            if path.exists() {
                return Ok(Some(path.to_path_buf()));
            }
        }
        let (url, _path, name_hint) = Self::media_url_and_name(part);
        let Some(url) = url.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        if let Some(file_url) = url.strip_prefix("file://") {
            let path = PathBuf::from(file_url);
            if path.exists() {
                return Ok(Some(path));
            }
            return Ok(None);
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            return self.download_remote_media(url, name_hint).await.map(Some);
        }
        if url.starts_with("data:") {
            return self.decode_data_url_media(url, name_hint).await.map(Some);
        }
        let path = expand_tilde(url);
        if path.exists() {
            return Ok(Some(path));
        }
        Ok(None)
    }

    async fn download_remote_media(&self, url: &str, name_hint: &str) -> Result<PathBuf> {
        let resp = default_http_client()
            .get(url)
            .send()
            .await
            .map_err(|err| RuaError::Io {
                message: format!("imessage media download failed: {err}"),
            })?;
        if !resp.status().is_success() {
            return Err(RuaError::Io {
                message: format!("imessage media download HTTP {}", resp.status()),
            });
        }
        if let Some(len) = resp.content_length() {
            if len as usize > self.config.max_decoded_size {
                return Err(RuaError::Validation {
                    message: format!(
                        "imessage media too large: {len} bytes (max {})",
                        self.config.max_decoded_size
                    ),
                });
            }
        }
        let bytes = resp.bytes().await.map_err(|err| RuaError::Io {
            message: format!("imessage media download body failed: {err}"),
        })?;
        if bytes.len() > self.config.max_decoded_size {
            return Err(RuaError::Validation {
                message: format!(
                    "imessage media too large: {} bytes (max {})",
                    bytes.len(),
                    self.config.max_decoded_size
                ),
            });
        }
        let filename = url
            .split('?')
            .next()
            .and_then(|value| value.rsplit('/').next())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(name_hint);
        self.write_media_file(filename, &bytes).await
    }

    async fn decode_data_url_media(&self, url: &str, name_hint: &str) -> Result<PathBuf> {
        let Some((header, encoded)) = url
            .strip_prefix("data:")
            .and_then(|rest| rest.split_once(";base64,"))
        else {
            return Err(RuaError::Validation {
                message: "imessage data URL must use ;base64,".into(),
            });
        };
        let approx_size = encoded.len().saturating_mul(3) / 4;
        if approx_size > self.config.max_decoded_size {
            return Err(RuaError::Validation {
                message: format!(
                    "imessage data URL too large: ~{approx_size} bytes (max {})",
                    self.config.max_decoded_size
                ),
            });
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .map_err(|err| RuaError::Validation {
                message: format!("invalid imessage data URL base64: {err}"),
            })?;
        if bytes.len() > self.config.max_decoded_size {
            return Err(RuaError::Validation {
                message: format!(
                    "imessage data URL too large: {} bytes (max {})",
                    bytes.len(),
                    self.config.max_decoded_size
                ),
            });
        }
        let ext = match header.split(';').next().unwrap_or_default() {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "audio/mpeg" => "mp3",
            "video/mp4" => "mp4",
            _ => "bin",
        };
        let mut filename = crate::core::media::sanitize_filename(name_hint);
        if std::path::Path::new(&filename).extension().is_none() {
            filename.push('.');
            filename.push_str(ext);
        }
        self.write_media_file(&filename, &bytes).await
    }

    async fn write_media_file(&self, filename: &str, bytes: &[u8]) -> Result<PathBuf> {
        let dir = self.media_dir();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|err| RuaError::Io {
                message: format!("create imessage media dir {}: {err}", dir.display()),
            })?;
        let safe = crate::core::media::sanitize_filename(filename);
        let final_path = dir.join(format!("{}_{}", uuid::Uuid::new_v4(), safe));
        let tmp_path = dir.join(format!(
            ".{}.{}.tmp",
            final_path
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_else(|| "imessage-media".into()),
            uuid::Uuid::new_v4()
        ));
        let mut file = tokio::fs::File::create(&tmp_path)
            .await
            .map_err(|err| RuaError::Io {
                message: format!(
                    "create temp imessage media file {}: {err}",
                    tmp_path.display()
                ),
            })?;
        file.write_all(bytes).await.map_err(|err| RuaError::Io {
            message: format!(
                "write temp imessage media file {}: {err}",
                tmp_path.display()
            ),
        })?;
        file.sync_all().await.map_err(|err| RuaError::Io {
            message: format!(
                "sync temp imessage media file {}: {err}",
                tmp_path.display()
            ),
        })?;
        drop(file);
        tokio::fs::rename(&tmp_path, &final_path)
            .await
            .map_err(|err| RuaError::Io {
                message: format!(
                    "rename temp imessage media file {} to {}: {err}",
                    tmp_path.display(),
                    final_path.display()
                ),
            })?;
        let _ = std::fs::File::open(&dir).and_then(|file| file.sync_all());
        Ok(final_path)
    }

    async fn poll_loop(
        self: Arc<Self>,
        conn: Connection,
        mut last_rowid: i64,
        process: Arc<dyn Process>,
        cancel: CancellationToken,
    ) {
        let config = &self.config;
        let poll_dur = Duration::from_secs_f64(config.poll_sec);
        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::PlainText,
            self.render_options.clone(),
        ));
        let mut tasks = tokio::task::JoinSet::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tasks.join_next(), if !tasks.is_empty() => continue,
                _ = tokio::time::sleep(poll_dur) => {}
            }

            let rows = match conn.prepare(
                "SELECT m.ROWID, m.text, m.is_from_me, h.id as sender \
                 FROM message m \
                 JOIN chat_message_join cmj ON cmj.message_id = m.ROWID \
                 JOIN chat c ON c.ROWID = cmj.chat_id \
                 LEFT JOIN handle h ON h.ROWID = m.handle_id \
                 WHERE m.ROWID > ?1 \
                 ORDER BY m.ROWID ASC",
            ) {
                Ok(mut stmt) => {
                    let result: Vec<(i64, Option<String>, i32, Option<String>)> = stmt
                        .query_map([last_rowid], |row| {
                            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                        })
                        .ok()
                        .map(|rows| rows.filter_map(|r| r.ok()).collect())
                        .unwrap_or_default();
                    result
                }
                Err(e) => {
                    tracing::warn!(error = %e, "imessage poll query failed");
                    continue;
                }
            };

            for (rowid, text, is_from_me, sender) in rows {
                last_rowid = rowid;
                if is_from_me == 1 {
                    continue;
                }

                let sender = match sender {
                    Some(s) if !s.trim().is_empty() => s.trim().to_string(),
                    _ => continue,
                };

                if !config.allowed_senders.is_empty()
                    && !config.allowed_senders.iter().any(|a| a == &sender)
                {
                    tracing::info!(sender = %sender, "imessage sender not in allow list, skipping");
                    continue;
                }

                let raw_text = text.unwrap_or_default();
                let input_prefix = self.acl_metadata["input_prefix"].as_str().unwrap_or("");
                let processed_text = if input_prefix.is_empty() {
                    raw_text
                } else if let Some(text) = raw_text.strip_prefix(input_prefix) {
                    text.trim().to_string()
                } else {
                    continue;
                };

                let mut input = Vec::new();
                if !processed_text.is_empty() {
                    input.push(IncomingContentPart::Text {
                        text: processed_text,
                    });
                }

                if let Ok(mut att_stmt) = conn.prepare(
                    "SELECT a.filename, a.mime_type, a.transfer_name \
                     FROM message_attachment_join maj \
                     JOIN attachment a ON a.ROWID = maj.attachment_id \
                     WHERE maj.message_id = ?1",
                ) {
                    if let Ok(att_rows) = att_stmt.query_map([rowid], |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    }) {
                        for att in att_rows.flatten() {
                            let (filename, mime_type, transfer_name) = att;
                            if let Some(ref fpath) = filename {
                                let expanded = expand_tilde(fpath);
                                if !expanded.exists() {
                                    tracing::debug!(
                                        path = %expanded.display(),
                                        "attachment file not found, skipping"
                                    );
                                    continue;
                                }
                                let file_id =
                                    transfer_name.clone().unwrap_or_else(|| rowid.to_string());
                                let is_image = mime_type
                                    .as_deref()
                                    .is_some_and(|m| m.starts_with("image/"));
                                if is_image {
                                    input.push(IncomingContentPart::Image {
                                        file_id,
                                        path: expanded,
                                        mime: mime_type,
                                    });
                                } else {
                                    input.push(IncomingContentPart::File {
                                        file_id,
                                        path: expanded,
                                        mime: mime_type,
                                        name: transfer_name,
                                    });
                                }
                            }
                        }
                    }
                }

                if input.is_empty() {
                    continue;
                }

                let request = AgentChannelRequest {
                    agent_id: String::new(),
                    user_id: sender.clone(),
                    session_id: sender.clone(),
                    input,
                    channel_id: "imessage".into(),
                    conversation_id: sender,
                    message_id: Some(rowid.to_string()),
                    metadata: {
                        let mut metadata = self.acl_metadata.clone();
                        metadata["rowid"] = rowid.into();
                        metadata["is_group"] = false.into();
                        metadata
                    },
                };

                tracing::info!(rowid, "imessage received message");
                let sender = self.clone();
                let send: pipeline::SendFn = Arc::new(move |to, message| {
                    let sender = sender.clone();
                    Box::pin(
                        async move { channel::Channel::send(sender.as_ref(), to, message).await },
                    )
                });
                let renderer = renderer.clone();
                let process = process.clone();
                tasks.spawn(async move {
                    if let Err(error) =
                        pipeline::run_inbound(process, request, None, renderer, send).await
                    {
                        tracing::warn!(%error, "iMessage processing failed");
                    }
                });
            }
        }

        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        tracing::info!("imessage poll loop exited");
    }
}

#[async_trait]
impl channel::Channel for IMessageChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        if !self.config.enabled {
            tracing::debug!("imessage channel disabled");
            return Ok(());
        }

        let mut poll_task = self.poll_task.lock().await;
        if poll_task.is_some() {
            return Ok(());
        }
        if !self.config.poll_sec.is_finite() || self.config.poll_sec <= 0.0 {
            return Err(RuaError::Validation {
                message: "iMessage poll_sec must be positive".into(),
            });
        }
        let imsg = match self.imsg_path.read().await.clone() {
            Some(path) => path,
            None => tokio::task::spawn_blocking(Self::find_imsg)
                .await
                .map_err(|error| RuaError::Runtime {
                    message: error.to_string(),
                })??,
        };
        let db_path = self.db_path.clone();
        let (conn, last_rowid) = tokio::task::spawn_blocking(move || {
            let conn = Connection::open_with_flags(
                db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )
            .map_err(|error| RuaError::Io {
                message: format!("open iMessage database: {error}"),
            })?;
            let rowid = conn
                .query_row("SELECT IFNULL(MAX(ROWID),0) FROM message", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|error| RuaError::Io {
                    message: format!("read iMessage database: {error}"),
                })?;
            Ok::<_, RuaError>((conn, rowid))
        })
        .await
        .map_err(|error| RuaError::Runtime {
            message: error.to_string(),
        })??;
        *self.imsg_path.write().await = Some(imsg.clone());
        let mut worker = Self::new(self.config.clone()).with_imsg_path(imsg);
        worker.render_options = self.render_options.clone();
        worker.acl_metadata = self.acl_metadata.clone();
        let cancel = CancellationToken::new();
        let task =
            tokio::spawn(Arc::new(worker).poll_loop(conn, last_rowid, process, cancel.clone()));
        *poll_task = Some((cancel, task));

        tracing::info!("imessage channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        if let Some((cancel, mut task)) = self.poll_task.lock().await.take() {
            cancel.cancel();
            if tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        tracing::info!("imessage channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let imsg = self.imsg_path.read().await;
        let imsg_path = imsg.as_ref().ok_or_else(|| RuaError::Runtime {
            message: "imessage channel not started (imsg path missing)".into(),
        })?;

        let recipient = to.sender_id.as_deref().unwrap_or(&to.conversation_id);

        let text = Self::render_text_and_unresolved_media(&msg.content);

        if !text.is_empty() {
            Self::send_via_imsg(imsg_path, recipient, &text).await?;
        }

        for part in &msg.content {
            if let Some(path) = self.resolve_media_path(part).await? {
                Self::send_file_via_imsg(imsg_path, recipient, &path).await?;
            }
        }

        Ok(())
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        let sender_id = native
            .get("sender_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let text = native
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let rowid = native
            .get("rowid")
            .and_then(|v| v.as_i64())
            .map(|r| r.to_string());

        if !self.is_sender_allowed(&sender_id) {
            return Err(RuaError::Security {
                message: format!("sender {sender_id} not in allow list"),
            });
        }

        Ok(AgentChannelRequest {
            agent_id: String::new(),
            user_id: sender_id.clone(),
            session_id: sender_id.clone(),
            input: vec![IncomingContentPart::Text { text }],
            channel_id: "imessage".into(),
            conversation_id: sender_id,
            message_id: rowid,
            metadata: native,
        })
    }
}
