use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, RwLock};

use crate::core::channel::{self, AgentRequest, OutMessage, Process, ToHandle};
use crate::core::content::{IncomingContentPart, OutgoingContentPart};
use crate::core::event_dedup::{EventDedupDecision, EventDeduper};
use crate::core::pipeline;
use crate::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use crate::core::ChannelId;
use crate::http_client::default_http_client;
use rua_core::Result;

use super::client::{self, WsHandle};
use super::types::{XiaoyiConfig, TEXT_CHUNK_LIMIT};

const EVENT_DEDUP_TTL: Duration = Duration::from_secs(15 * 60);
const EVENT_DEDUP_CAPACITY: usize = 4096;
const MAX_INBOUND_MEDIA_SIZE: usize = 50 * 1024 * 1024;

pub struct XiaoyiChannel {
    channel_id: ChannelId,
    config: XiaoyiConfig,
    process: RwLock<Option<Arc<dyn Process>>>,
    started: RwLock<bool>,
    ws_stop_tx: watch::Sender<bool>,
    ws_handle: Arc<RwLock<Option<WsHandle>>>,
    render_options: MessageRenderOptions,
    acl_metadata: serde_json::Value,
    event_deduper: Arc<StdMutex<EventDeduper>>,
    session_server_map: Arc<RwLock<HashMap<String, String>>>,
}

impl XiaoyiChannel {
    pub fn new(config: XiaoyiConfig) -> Self {
        let (ws_stop_tx, _) = watch::channel(false);
        Self {
            channel_id: "xiaoyi".into(),
            config,
            process: RwLock::new(None),
            started: RwLock::new(false),
            ws_stop_tx,
            ws_handle: Arc::new(RwLock::new(None)),
            render_options: MessageRenderOptions::default(),
            acl_metadata: serde_json::json!({}),
            event_deduper: Arc::new(StdMutex::new(EventDeduper::new(
                EVENT_DEDUP_TTL,
                EVENT_DEDUP_CAPACITY,
            ))),
            session_server_map: Arc::new(RwLock::new(HashMap::new())),
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

    pub fn render_text_and_unresolved_media(parts: &[OutgoingContentPart]) -> String {
        let mut segments = Vec::new();
        for part in parts {
            match part {
                OutgoingContentPart::Text { text } | OutgoingContentPart::Markdown { text } => {
                    segments.push(text.clone());
                }
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

    pub fn media_artifact_part(part: &OutgoingContentPart) -> Option<serde_json::Value> {
        match part {
            OutgoingContentPart::Image { url, path, alt } => {
                let uri = outgoing_uri(url.as_deref(), path.as_deref())?;
                Some(serde_json::json!({
                    "kind": "file",
                    "file": {
                        "name": alt.as_deref().unwrap_or("image"),
                        "mimeType": "image/png",
                        "uri": uri,
                    }
                }))
            }
            OutgoingContentPart::Video { url, path, name } => {
                let uri = outgoing_uri(url.as_deref(), path.as_deref())?;
                Some(serde_json::json!({
                    "kind": "file",
                    "file": {
                        "name": name.as_deref().unwrap_or("video"),
                        "mimeType": "video/mp4",
                        "uri": uri,
                    }
                }))
            }
            OutgoingContentPart::File { url, path, name } => {
                let uri = outgoing_uri(url.as_deref(), path.as_deref())?;
                Some(serde_json::json!({
                    "kind": "file",
                    "file": {
                        "name": name.as_deref().unwrap_or("file"),
                        "mimeType": "application/octet-stream",
                        "uri": uri,
                    }
                }))
            }
            _ => None,
        }
    }

    pub fn chunk_text(text: &str, limit: usize) -> Vec<String> {
        if limit == 0 || text.len() <= limit {
            return vec![text.to_string()];
        }

        let mut chunks = Vec::new();
        let mut current = String::new();

        for line in text.split('\n') {
            if line.len() > limit {
                if !current.is_empty() {
                    chunks.push(current);
                    current = String::new();
                }
                chunks.extend(split_xiaoyi_long_line(line, limit));
                continue;
            }

            let candidate = if current.is_empty() {
                line.to_string()
            } else {
                format!("{current}\n{line}")
            };
            if candidate.len() > limit {
                chunks.push(current);
                current = line.to_string();
            } else {
                current = candidate;
            }
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        chunks
    }
}

fn outgoing_uri(url: Option<&str>, path: Option<&std::path::Path>) -> Option<String> {
    path.map(|path| path.to_string_lossy().to_string())
        .or_else(|| {
            url.filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        })
}

fn split_xiaoyi_long_line(line: &str, limit: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < line.len() {
        let end = std::cmp::min(start + limit, line.len());
        let actual_end = if end >= line.len() {
            line.len()
        } else {
            let mut pos = end;
            while pos > start && !line.is_char_boundary(pos) {
                pos -= 1;
            }
            next_chunk_boundary(line, start, pos)
        };
        chunks.push(line[start..actual_end].to_string());
        start = actual_end;
    }
    chunks
}

fn next_chunk_boundary(text: &str, start: usize, candidate: usize) -> usize {
    if candidate > start {
        return candidate;
    }
    text[start..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| start + offset)
        .unwrap_or(text.len())
}

fn xiaoyi_file_part(part: &serde_json::Value) -> Vec<IncomingContentPart> {
    let file = part.get("file").unwrap_or(part);
    let uri = file
        .get("uri")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if uri.is_empty() {
        return Vec::new();
    }
    let name = file
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("file")
        .to_string();
    let mime = file
        .get("mimeType")
        .or_else(|| file.get("mime_type"))
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned);
    let local_path = file
        .get("local_path")
        .or_else(|| file.get("localPath"))
        .or_else(|| file.get("path"))
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty());
    let Some(local_path) = local_path else {
        return vec![IncomingContentPart::Text {
            text: format!("[{name}: download failed]"),
        }];
    };
    let path = PathBuf::from(local_path);
    if mime
        .as_deref()
        .is_some_and(|value| value.starts_with("image/"))
    {
        vec![IncomingContentPart::Image {
            file_id: uri.to_string(),
            path,
            mime,
        }]
    } else {
        vec![IncomingContentPart::File {
            file_id: uri.to_string(),
            path,
            mime,
            name: Some(name),
        }]
    }
}

async fn download_xiaoyi_inbound_media(
    media_dir: &str,
    native: &mut serde_json::Value,
) -> Result<()> {
    let Some(parts) = native
        .get_mut("params")
        .and_then(|params| params.get_mut("message"))
        .and_then(|message| message.get_mut("parts"))
        .and_then(|parts| parts.as_array_mut())
    else {
        return Ok(());
    };
    for part in parts {
        if part.get("kind").and_then(|v| v.as_str()) != Some("file") {
            continue;
        }
        let Some(file) = part.get_mut("file").and_then(|file| file.as_object_mut()) else {
            continue;
        };
        if file
            .get("local_path")
            .or_else(|| file.get("localPath"))
            .or_else(|| file.get("path"))
            .is_some()
        {
            continue;
        }
        let Some(uri) = file
            .get("uri")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        let filename = file
            .get("name")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("file")
            .to_string();
        if let Some((path, mime)) = download_xiaoyi_file(media_dir, &uri, &filename).await? {
            file.insert(
                "local_path".to_string(),
                serde_json::Value::String(path.to_string_lossy().to_string()),
            );
            if let Some(mime) = mime {
                file.insert("mimeType".to_string(), serde_json::Value::String(mime));
            }
        }
    }
    Ok(())
}

async fn download_xiaoyi_file(
    media_dir: &str,
    uri: &str,
    filename: &str,
) -> Result<Option<(PathBuf, Option<String>)>> {
    if !uri.starts_with("http://") && !uri.starts_with("https://") {
        return Ok(None);
    }
    let resp = match default_http_client()
        .get(uri)
        .timeout(Duration::from_secs(60))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(%uri, error = %err, "xiaoyi file download failed");
            return Ok(None);
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(%uri, status = %resp.status(), "xiaoyi file download non-success");
        return Ok(None);
    }
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_INBOUND_MEDIA_SIZE {
            tracing::warn!(%uri, len, "xiaoyi file too large");
            return Ok(None);
        }
    }
    let mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string())
        .filter(|value| !value.is_empty());
    let bytes = resp
        .bytes()
        .await
        .map_err(|err| rua_core::RuaError::Runtime {
            message: format!("xiaoyi file download bytes failed: {err}"),
        })?;
    if bytes.len() > MAX_INBOUND_MEDIA_SIZE {
        tracing::warn!(%uri, len = bytes.len(), "xiaoyi file too large");
        return Ok(None);
    }
    let dir = if media_dir.trim().is_empty() {
        std::env::temp_dir().join("rua-xiaoyi-media")
    } else {
        PathBuf::from(media_dir)
    };
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| rua_core::RuaError::Io {
            message: format!("create xiaoyi media dir {}: {err}", dir.display()),
        })?;
    let safe_name = crate::core::media::sanitize_filename(filename);
    let path = dir.join(format!("{}_{}", uuid::Uuid::new_v4(), safe_name));
    atomic_write_xiaoyi_file(&path, &bytes).await?;
    Ok(Some((path, mime)))
}

async fn atomic_write_xiaoyi_file(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| rua_core::RuaError::Io {
        message: format!("missing parent dir for {}", path.display()),
    })?;
    let tmp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| "xiaoyi-media".into()),
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

pub fn build_artifact_msg(
    agent_id: &str,
    session_id: &str,
    task_id: &str,
    message_id: &str,
    parts: Vec<serde_json::Value>,
    last_chunk: bool,
    r#final: bool,
) -> serde_json::Value {
    let artifact_id = format!("artifact_{}", uuid::Uuid::new_v4().as_simple());
    let json_rpc_response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": message_id,
        "result": {
            "taskId": task_id,
            "kind": "artifact-update",
            "append": true,
            "lastChunk": last_chunk,
            "final": r#final,
            "artifact": {
                "artifactId": artifact_id,
                "parts": parts,
            },
        },
    });
    serde_json::json!({
        "msgType": "agent_response",
        "agentId": agent_id,
        "sessionId": session_id,
        "taskId": task_id,
        "msgDetail": serde_json::to_string(&json_rpc_response).unwrap(),
    })
}

pub fn build_status_update_msg(
    agent_id: &str,
    session_id: &str,
    task_id: &str,
    message_id: &str,
) -> serde_json::Value {
    let json_rpc_response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": message_id,
        "result": {
            "taskId": task_id,
            "kind": "status-update",
            "final": false,
            "status": {
                "message": {
                    "role": "agent",
                    "parts": [
                        {"kind": "text", "text": ""},
                    ],
                },
                "state": "completed",
            },
        },
    });
    serde_json::json!({
        "msgType": "agent_response",
        "agentId": agent_id,
        "sessionId": session_id,
        "taskId": task_id,
        "msgDetail": serde_json::to_string(&json_rpc_response).unwrap(),
    })
}

fn parse_message_stream(msg: &serde_json::Value, agent_id: &str) -> Option<AgentRequest> {
    if let Some(incoming_agent_id) = msg.get("agentId").and_then(|v| v.as_str()) {
        if !incoming_agent_id.is_empty() && incoming_agent_id != agent_id {
            return None;
        }
    }

    let params = msg.get("params")?;
    let session_id = params
        .get("sessionId")
        .or_else(|| msg.get("sessionId"))
        .and_then(|v| v.as_str())?;
    let task_id = params
        .get("id")
        .or_else(|| msg.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or(session_id);

    let message = params.get("message")?;
    let parts = message.get("parts")?.as_array()?;

    let mut input = Vec::new();
    let mut text_parts: Vec<String> = Vec::new();
    for part in parts {
        let kind = part.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if kind == "text" {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                text_parts.push(text.to_string());
            }
        } else if kind == "file" {
            input.extend(xiaoyi_file_part(part));
        }
    }

    let text_content = text_parts.join(" ").trim().to_string();
    if !text_content.is_empty() {
        input.insert(0, IncomingContentPart::Text { text: text_content });
    }
    if input.is_empty() {
        return None;
    }

    let conversation_id = format!("xiaoyi:{session_id}");
    let message_id = msg
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(AgentRequest {
        agent_id: agent_id.to_string(),
        user_id: session_id.to_string(),
        session_id: conversation_id.clone(),
        input,
        channel_id: "xiaoyi".into(),
        conversation_id,
        message_id: Some(message_id),
        metadata: serde_json::json!({
            "session_id": session_id,
            "task_id": task_id,
        }),
    })
}

#[async_trait]
impl channel::Channel for XiaoyiChannel {
    fn id(&self) -> &ChannelId {
        &self.channel_id
    }

    async fn start(&self, process: Arc<dyn Process>) -> Result<()> {
        let mut started = self.started.write().await;
        if *started {
            tracing::warn!("xiaoyi channel already started");
            return Ok(());
        }
        *self.process.write().await = Some(process.clone());
        *started = true;

        let config = self.config.clone();
        let agent_id = config.agent_id.clone();
        let renderer = Arc::new(MessageRenderer::with_options(
            RenderStyle::Markdown,
            self.render_options.clone(),
        ));

        let send_fn: pipeline::SendFn = {
            let ws_handle = self.ws_handle.clone();
            let session_server_map = self.session_server_map.clone();
            let agent_id = agent_id.clone();
            Arc::new(move |to: ToHandle, msg: OutMessage| {
                let ws_handle = ws_handle.clone();
                let session_server_map = session_server_map.clone();
                let agent_id = agent_id.clone();
                Box::pin(async move {
                    let session_id = to
                        .metadata
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&to.conversation_id);
                    let task_id = to
                        .metadata
                        .get("task_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or(session_id);
                    let message_id = uuid::Uuid::new_v4().to_string();

                    let guard = ws_handle.read().await;
                    let handle = match guard.as_ref() {
                        Some(h) => h,
                        None => return Ok(()),
                    };
                    let server_name = session_server_map.read().await.get(session_id).cloned();

                    let text = XiaoyiChannel::render_text_and_unresolved_media(&msg.content);
                    if !text.is_empty() {
                        let chunks = XiaoyiChannel::chunk_text(&text, TEXT_CHUNK_LIMIT);
                        for chunk in &chunks {
                            let artifact = build_artifact_msg(
                                &agent_id,
                                session_id,
                                task_id,
                                &message_id,
                                vec![serde_json::json!({"kind": "text", "text": chunk})],
                                true,
                                false,
                            );
                            if let Err(e) = handle
                                .send_json_to_server(
                                    server_name.as_deref(),
                                    serde_json::to_string(&artifact).unwrap(),
                                )
                                .await
                            {
                                tracing::error!("xiaoyi send chunk failed: {e}");
                            }
                        }
                    }

                    for part in &msg.content {
                        let Some(artifact_part) = XiaoyiChannel::media_artifact_part(part) else {
                            continue;
                        };
                        let artifact = build_artifact_msg(
                            &agent_id,
                            session_id,
                            task_id,
                            &message_id,
                            vec![artifact_part],
                            true,
                            true,
                        );
                        if let Err(e) = handle
                            .send_json_to_server(
                                server_name.as_deref(),
                                serde_json::to_string(&artifact).unwrap(),
                            )
                            .await
                        {
                            tracing::error!("xiaoyi send media failed: {e}");
                        }
                    }

                    let status_msg =
                        build_status_update_msg(&agent_id, session_id, task_id, &message_id);
                    if let Err(e) = handle
                        .send_json_to_server(
                            server_name.as_deref(),
                            serde_json::to_string(&status_msg).unwrap(),
                        )
                        .await
                    {
                        tracing::error!("xiaoyi send status-update failed: {e}");
                    }

                    let final_msg = build_artifact_msg(
                        &agent_id,
                        session_id,
                        task_id,
                        &message_id,
                        vec![serde_json::json!({"kind": "text", "text": ""})],
                        true,
                        true,
                    );
                    if let Err(e) = handle
                        .send_json_to_server(
                            server_name.as_deref(),
                            serde_json::to_string(&final_msg).unwrap(),
                        )
                        .await
                    {
                        tracing::error!("xiaoyi send final failed: {e}");
                    }

                    Ok(())
                })
            })
        };

        let stop_rx = self.ws_stop_tx.subscribe();
        let ws_handle_ref = self.ws_handle.clone();
        let acl_metadata = Arc::new(self.acl_metadata.clone());
        let event_deduper = self.event_deduper.clone();
        let media_dir = config.media_dir.clone();
        let session_server_map = self.session_server_map.clone();

        let on_message: client::OnMessageFn = Arc::new(
            move |native: serde_json::Value, server_name: String| {
                let method = native.get("method").and_then(|v| v.as_str()).unwrap_or("");

                match method {
                    "message/stream" => {
                        let agent_id = agent_id.clone();
                        let event_deduper = event_deduper.clone();
                        let p = process.clone();
                        let r = renderer.clone();
                        let sf = send_fn.clone();
                        let acl = acl_metadata.clone();
                        let media_dir = media_dir.clone();
                        let session_server_map = session_server_map.clone();
                        tokio::spawn(async move {
                            let mut native = native;
                            if let Some(session_id) = native
                                .get("params")
                                .and_then(|params| params.get("sessionId"))
                                .or_else(|| native.get("sessionId"))
                                .and_then(|value| value.as_str())
                                .filter(|value| !value.trim().is_empty())
                            {
                                session_server_map
                                    .write()
                                    .await
                                    .insert(session_id.to_string(), server_name);
                            }
                            if let Err(err) =
                                download_xiaoyi_inbound_media(&media_dir, &mut native).await
                            {
                                tracing::warn!(error = %err, "xiaoyi inbound media download failed; using placeholders");
                            }
                            if let Some(mut req) = parse_message_stream(&native, &agent_id) {
                                match xiaoyi_check_duplicate_request(&event_deduper, &req) {
                                    Ok(true) => {
                                        tracing::debug!(
                                            message_id = ?req.message_id,
                                            "xiaoyi duplicate event skipped"
                                        );
                                        return;
                                    }
                                    Ok(false) => {}
                                    Err(err) => {
                                        tracing::warn!(error = %err, "xiaoyi event dedup failed");
                                    }
                                }
                                if let (Some(acl_obj), Some(meta)) =
                                    (acl.as_object(), req.metadata.as_object_mut())
                                {
                                    for (k, v) in acl_obj {
                                        meta.insert(k.clone(), v.clone());
                                    }
                                }
                                if let Err(e) = pipeline::run_inbound(p, req, None, r, sf).await {
                                    tracing::error!("xiaoyi run_inbound error: {e}");
                                }
                            }
                        });
                    }
                    "clearContext" | "tasks/cancel" => {
                        tracing::info!("xiaoyi: received {method}, acknowledged");
                    }
                    _ => {
                        tracing::debug!("xiaoyi: unhandled method={method}");
                    }
                }
            },
        );

        let handle = client::spawn_ws_client(config, on_message, stop_rx);
        *ws_handle_ref.write().await = Some(handle);

        tracing::info!("xiaoyi channel started (A2A WS)");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut started = self.started.write().await;
        if !*started {
            return Ok(());
        }
        let _ = self.ws_stop_tx.send(true);
        *self.ws_handle.write().await = None;
        if let Ok(mut deduper) = self.event_deduper.lock() {
            deduper.clear();
        }
        self.session_server_map.write().await.clear();
        *self.process.write().await = None;
        *started = false;
        tracing::info!("xiaoyi channel stopped");
        Ok(())
    }

    async fn send(&self, to: ToHandle, msg: OutMessage) -> Result<()> {
        let session_id = to
            .metadata
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&to.conversation_id);
        let task_id = to
            .metadata
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or(session_id);
        let message_id = uuid::Uuid::new_v4().to_string();

        let guard = self.ws_handle.read().await;
        let handle = match guard.as_ref() {
            Some(h) => h,
            None => {
                tracing::warn!("xiaoyi: cannot send — not connected");
                return Ok(());
            }
        };
        let server_name = self
            .session_server_map
            .read()
            .await
            .get(session_id)
            .cloned();

        let text = Self::render_text_and_unresolved_media(&msg.content);
        if !text.is_empty() {
            let chunks = Self::chunk_text(&text, TEXT_CHUNK_LIMIT);
            for chunk in &chunks {
                let artifact = build_artifact_msg(
                    &self.config.agent_id,
                    session_id,
                    task_id,
                    &message_id,
                    vec![serde_json::json!({"kind": "text", "text": chunk})],
                    true,
                    false,
                );
                if let Err(e) = handle
                    .send_json_to_server(
                        server_name.as_deref(),
                        serde_json::to_string(&artifact).unwrap(),
                    )
                    .await
                {
                    tracing::error!("xiaoyi send error: {e}");
                }
            }
        }

        for part in &msg.content {
            let Some(artifact_part) = Self::media_artifact_part(part) else {
                continue;
            };
            let artifact = build_artifact_msg(
                &self.config.agent_id,
                session_id,
                task_id,
                &message_id,
                vec![artifact_part],
                true,
                true,
            );
            if let Err(e) = handle
                .send_json_to_server(
                    server_name.as_deref(),
                    serde_json::to_string(&artifact).unwrap(),
                )
                .await
            {
                tracing::error!("xiaoyi send media error: {e}");
            }
        }

        let status_msg =
            build_status_update_msg(&self.config.agent_id, session_id, task_id, &message_id);
        if let Err(e) = handle
            .send_json_to_server(
                server_name.as_deref(),
                serde_json::to_string(&status_msg).unwrap(),
            )
            .await
        {
            tracing::error!("xiaoyi send status-update error: {e}");
        }

        let final_msg = build_artifact_msg(
            &self.config.agent_id,
            session_id,
            task_id,
            &message_id,
            vec![serde_json::json!({"kind": "text", "text": ""})],
            true,
            true,
        );
        if let Err(e) = handle
            .send_json_to_server(
                server_name.as_deref(),
                serde_json::to_string(&final_msg).unwrap(),
            )
            .await
        {
            tracing::error!("xiaoyi send final error: {e}");
        }

        Ok(())
    }

    fn build_request_from_native(&self, native: serde_json::Value) -> Result<AgentRequest> {
        let req = parse_message_stream(&native, &self.config.agent_id).ok_or_else(|| {
            rua_core::RuaError::Validation {
                message: "invalid xiaoyi A2A message/stream payload".into(),
            }
        })?;
        if xiaoyi_check_duplicate_request(&self.event_deduper, &req)? {
            return Err(rua_core::RuaError::Validation {
                message: "duplicate xiaoyi event".into(),
            });
        }
        Ok(req)
    }
}

fn xiaoyi_check_duplicate_request(
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
        message: "xiaoyi event deduper lock poisoned".into(),
    })?;
    Ok(
        deduper.check_and_mark(format!("xiaoyi:message:{message_id}"))
            == EventDedupDecision::Duplicate,
    )
}
