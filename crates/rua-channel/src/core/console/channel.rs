use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use futures::stream::StreamExt;
use futures::Stream;
use tracing::{debug, error, warn};

use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime};
use rua_core::message::ContentPart;
use rua_core::{RunControl, RuntimeEvent};

#[derive(Debug, Clone)]
pub struct ConsoleChatRequest {
    pub message: String,
    pub chat_id: Option<String>,
    pub model: Option<String>,
    pub is_reconnect: bool,
    pub attachments: Vec<AttachmentInfo>,

    pub input: Option<Vec<serde_json::Value>>,

    pub messages: Option<Vec<serde_json::Value>>,

    pub extra: serde_json::Map<String, serde_json::Value>,
    pub channel_kind: Option<crate::core::renderer::ChannelKind>,
}

#[derive(Debug, Clone)]
pub struct AttachmentInfo {
    pub url: String,
    pub name: String,
    pub mime_type: Option<String>,
    pub stored_name: Option<String>,
    pub size: Option<u64>,
}

impl ConsoleChatRequest {
    pub fn ordered_user_content_parts(&self) -> Vec<ContentPart> {
        ordered_user_content_parts(
            &self.message,
            self.input.as_ref(),
            self.messages.as_ref(),
            &self.attachments,
        )
    }
}

pub fn ordered_user_content_parts(
    message: &str,
    input: Option<&Vec<serde_json::Value>>,
    messages: Option<&Vec<serde_json::Value>>,
    attachments: &[AttachmentInfo],
) -> Vec<ContentPart> {
    let source = input
        .filter(|items| !items.is_empty())
        .or_else(|| messages.filter(|items| !items.is_empty()));
    let mut parts = Vec::new();
    if let Some(items) = source {
        for message in items {
            if message.get("role").and_then(serde_json::Value::as_str) != Some("user") {
                continue;
            }
            let Some(content) = message.get("content") else {
                continue;
            };
            if let Some(text) = content.as_str() {
                if !text.is_empty() {
                    parts.push(ContentPart::Text {
                        text: text.to_string(),
                    });
                }
            } else if let Some(content) = content.as_array() {
                parts.extend(content.iter().filter_map(content_part_from_request_json));
            }
        }
    } else if !message.is_empty() {
        parts.push(ContentPart::Text {
            text: message.to_string(),
        });
    }
    parts.extend(attachments.iter().map(content_part_from_attachment));
    parts
}

fn content_part_from_request_json(part: &serde_json::Value) -> Option<ContentPart> {
    if let Some(text) = part.as_str() {
        return (!text.is_empty()).then(|| ContentPart::Text {
            text: text.to_string(),
        });
    }
    let kind = part.get("type").and_then(serde_json::Value::as_str)?;
    if part.get("media_ref").is_some() {
        let parsed = agent_types::content::content_part_from_value(part.clone());
        return (!matches!(parsed, ContentPart::Data { .. })).then_some(parsed);
    }
    let string = |keys: &[&str]| {
        keys.iter().find_map(|key| {
            part.get(*key)
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
    };
    match kind {
        "text" => string(&["text"]).map(|text| ContentPart::Text { text }),
        "file" => string(&["file_url", "url"]).map(|url| ContentPart::File {
            url,
            name: string(&["file_name", "name"]).unwrap_or_default(),
            media_type: string(&["mime_type"]),
            size: part.get("size").and_then(serde_json::Value::as_u64),
        }),
        "image" => string(&["image_url", "url"]).map(|url| ContentPart::Image {
            url,
            media_type: string(&["mime_type"]),
            alt: string(&["file_name", "name"]),
        }),
        "video" => string(&["video_url", "url"]).map(|url| ContentPart::Video {
            url,
            media_type: string(&["mime_type"]),
        }),
        "audio" => string(&["audio_url", "data", "url"]).map(|url| ContentPart::Audio {
            url,
            media_type: string(&["mime_type"]),
        }),
        _ => None,
    }
}

fn content_part_from_attachment(attachment: &AttachmentInfo) -> ContentPart {
    let mime = attachment
        .mime_type
        .clone()
        .or_else(|| mime_type_from_extension(&attachment.name));
    if mime
        .as_deref()
        .is_some_and(|value| value.starts_with("image/"))
    {
        ContentPart::Image {
            url: attachment.url.clone(),
            media_type: mime,
            alt: Some(attachment.name.clone()),
        }
    } else {
        ContentPart::File {
            url: attachment.url.clone(),
            name: attachment.name.clone(),
            media_type: mime,
            size: attachment.size,
        }
    }
}

fn mime_type_from_extension(filename: &str) -> Option<String> {
    match filename.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("image/jpeg".into()),
        "png" => Some("image/png".into()),
        "gif" => Some("image/gif".into()),
        "webp" => Some("image/webp".into()),
        "svg" => Some("image/svg+xml".into()),
        "bmp" => Some("image/bmp".into()),
        "mp3" => Some("audio/mpeg".into()),
        "wav" => Some("audio/wav".into()),
        "ogg" => Some("audio/ogg".into()),
        "mp4" => Some("video/mp4".into()),
        "webm" => Some("video/webm".into()),
        "pdf" => Some("application/pdf".into()),
        "txt" | "md" | "log" => Some("text/plain".into()),
        "json" => Some("application/json".into()),
        "csv" => Some("text/csv".into()),
        _ => None,
    }
}

pub type SseOutputStream = Pin<Box<dyn Stream<Item = Result<String, Infallible>> + Send + 'static>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleStopKind {
    Local,
    RemoteControl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleStopOutcome {
    pub stopped: bool,
    pub kind: ConsoleStopKind,
    pub detail: Option<String>,
    pub failure_kind: Option<String>,
    pub persist_started_turn_marker: bool,
}

impl ConsoleStopOutcome {
    pub fn local(stopped: bool) -> Self {
        Self {
            stopped,
            kind: ConsoleStopKind::Local,
            detail: None,
            failure_kind: None,
            persist_started_turn_marker: false,
        }
    }

    pub fn local_acp_passthrough(stopped: bool) -> Self {
        Self {
            stopped,
            kind: ConsoleStopKind::Local,
            detail: None,
            failure_kind: None,
            persist_started_turn_marker: true,
        }
    }

    pub fn remote_control(
        stopped: bool,
        detail: Option<String>,
        failure_kind: Option<String>,
    ) -> Self {
        Self {
            stopped,
            kind: ConsoleStopKind::RemoteControl,
            detail,
            failure_kind,
            persist_started_turn_marker: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleSubmitKind {
    Started,
    Queued,
    Unconfirmed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleSubmitOutcome {
    pub accepted: bool,
    pub kind: ConsoleSubmitKind,
    pub detail: Option<String>,
    pub chat_id: Option<String>,
    pub input_id: Option<String>,
    pub submission_id: Option<String>,
    pub device_delivery_id: Option<String>,
    pub target_device_id: Option<String>,
    pub device_delivery_sequence: Option<u64>,
    pub accepted_at: Option<String>,
    pub failure_kind: Option<String>,
    pub control: Option<serde_json::Value>,
}

impl ConsoleSubmitOutcome {
    pub fn local(accepted: bool) -> Self {
        Self {
            accepted,
            kind: ConsoleSubmitKind::Started,
            detail: None,
            chat_id: None,
            input_id: None,
            submission_id: None,
            device_delivery_id: None,
            target_device_id: None,
            device_delivery_sequence: None,
            accepted_at: None,
            failure_kind: None,
            control: None,
        }
    }

    pub fn started(chat_id: impl Into<String>) -> Self {
        Self {
            accepted: true,
            kind: ConsoleSubmitKind::Started,
            detail: None,
            chat_id: Some(chat_id.into()),
            input_id: None,
            submission_id: None,
            device_delivery_id: None,
            target_device_id: None,
            device_delivery_sequence: None,
            accepted_at: None,
            failure_kind: None,
            control: None,
        }
    }

    pub fn queued(chat_id: impl Into<String>, input_id: Option<String>) -> Self {
        Self {
            accepted: true,
            kind: ConsoleSubmitKind::Queued,
            detail: None,
            chat_id: Some(chat_id.into()),
            input_id,
            submission_id: None,
            device_delivery_id: None,
            target_device_id: None,
            device_delivery_sequence: None,
            accepted_at: None,
            failure_kind: None,
            control: None,
        }
    }

    pub fn device_delivery(
        accepted: bool,
        detail: Option<String>,
        chat_id: Option<String>,
        device_delivery_id: Option<String>,
        target_device_id: Option<String>,
        device_delivery_sequence: Option<u64>,
        accepted_at: Option<String>,
    ) -> Self {
        Self {
            accepted,
            kind: ConsoleSubmitKind::Rejected,
            detail,
            chat_id,
            input_id: None,
            submission_id: None,
            device_delivery_id,
            target_device_id,
            device_delivery_sequence,
            accepted_at,
            failure_kind: None,
            control: None,
        }
    }
}

#[async_trait]
pub trait ConsoleChatFacade: Send + Sync {
    async fn submit_chat(
        &self,
        _agent_id: &str,
        _request: ConsoleChatRequest,
    ) -> ConsoleSubmitOutcome {
        ConsoleSubmitOutcome::local(false)
    }

    async fn submit_scoped_chat(
        &self,
        _address: rua_core::ChatAddress,
        _execution_agent_id: &str,
        _request: ConsoleChatRequest,
    ) -> ConsoleSubmitOutcome {
        ConsoleSubmitOutcome::local(false)
    }

    async fn connect_chat_activity(
        &self,
        _agent_id: &str,
        _chat_id: &str,
        _viewer_id: &str,
        _position: Option<rua_core::ConversationActivityPosition>,
    ) -> SseOutputStream {
        Box::pin(futures::stream::empty())
    }

    async fn connect_scoped_chat_activity(
        &self,
        _address: rua_core::ChatAddress,
        _execution_agent_id: &str,
        _viewer_id: &str,
        _position: Option<rua_core::ConversationActivityPosition>,
    ) -> SseOutputStream {
        Box::pin(futures::stream::empty())
    }

    async fn stop_chat(&self, agent_id: &str, chat_id: &str) -> bool;

    async fn stop_chat_outcome(&self, agent_id: &str, chat_id: &str) -> ConsoleStopOutcome {
        ConsoleStopOutcome::local(self.stop_chat(agent_id, chat_id).await)
    }
}

#[derive(Debug, Clone)]
struct ToolCallDeltaState {
    id: String,
    name: String,
    started: bool,
    delta_index: u64,
}

pub struct WebUiSseAdapter {
    chat_id: String,
    response_id: String,
    sequence: u64,
    text_msg_id: Option<String>,
    text_accumulator: String,
    text_msg_started: bool,
    text_content_index: u64,
    thinking_msg_id: Option<String>,
    reasoning_accumulator: String,
    thinking_msg_started: bool,
    thinking_content_index: u64,
    tool_call_names: HashMap<String, String>,

    tool_call_delta_state: HashMap<usize, ToolCallDeltaState>,

    tool_call_delta_opened: HashSet<String>,
    tool_result_output_state: HashMap<String, String>,
    tool_result_delta_opened: HashSet<String>,
    completed_messages: Vec<serde_json::Value>,
    usage_data: Option<(Option<u64>, Option<u64>, Option<u64>)>,
    response_started: bool,
}

impl WebUiSseAdapter {
    pub fn new(chat_id: String) -> Self {
        let response_id = format!("resp-{chat_id}");
        Self {
            chat_id,
            response_id,
            sequence: 0,
            text_msg_id: None,
            text_accumulator: String::new(),
            text_msg_started: false,
            text_content_index: 0,
            thinking_msg_id: None,
            reasoning_accumulator: String::new(),
            thinking_msg_started: false,
            thinking_content_index: 0,
            tool_call_names: HashMap::new(),
            tool_call_delta_state: HashMap::new(),
            tool_call_delta_opened: HashSet::new(),
            tool_result_output_state: HashMap::new(),
            tool_result_delta_opened: HashSet::new(),
            completed_messages: Vec::new(),
            usage_data: None,
            response_started: false,
        }
    }

    fn next_seq(&mut self) -> u64 {
        let s = self.sequence;
        self.sequence += 1;
        s
    }

    fn emit(&self, json: serde_json::Value) -> String {
        json.to_string()
    }

    fn ensure_response_started(&mut self, events: &mut Vec<String>) {
        if self.response_started {
            return;
        }
        self.response_started = true;
        let seq = self.next_seq();
        events.push(self.emit(serde_json::json!({
            "sequence_number": seq,
            "object": "response",
            "status": "created",
            "error": null,
            "id": &self.response_id,
            "created_at": 0,
            "completed_at": null,
            "output": null,
            "usage": null,
            "chat_id": &self.chat_id
        })));
        let seq = self.next_seq();
        events.push(self.emit(serde_json::json!({
            "sequence_number": seq,
            "object": "response",
            "status": "in_progress",
            "error": null,
            "id": &self.response_id,
            "created_at": 0,
            "completed_at": null,
            "output": null,
            "usage": null,
            "chat_id": &self.chat_id
        })));
    }

    fn close_reasoning(&mut self, events: &mut Vec<String>) {
        if !self.thinking_msg_started || self.reasoning_accumulator.is_empty() {
            return;
        }
        let Some(thinking_msg_id) = self.thinking_msg_id.clone() else {
            return;
        };
        let idx = self.thinking_content_index;
        let seq = self.next_seq();
        events.push(self.emit(serde_json::json!({
            "sequence_number": seq,
            "object": "content",
            "status": "completed",
            "error": null,
            "type": "text",
            "index": idx,
            "delta": null,
            "msg_id": &thinking_msg_id,
            "text": &self.reasoning_accumulator
        })));
        let seq = self.next_seq();
        let msg_completed = serde_json::json!({
            "sequence_number": seq,
            "object": "message",
            "status": "completed",
            "error": null,
            "id": &thinking_msg_id,
            "type": "reasoning",
            "role": "assistant",
            "content": [{
                "sequence_number": null, "object": "content",
                "status": "completed", "error": null,
                "type": "text", "index": idx, "delta": null,
                "msg_id": &thinking_msg_id,
                "text": &self.reasoning_accumulator
            }],
            "code": null, "message": null, "usage": null, "metadata": null
        });
        events.push(self.emit(msg_completed.clone()));
        self.completed_messages.push(msg_completed);
        self.thinking_content_index += 1;
        self.thinking_msg_started = false;
        self.reasoning_accumulator.clear();
        self.thinking_msg_id = None;
    }

    fn close_text(&mut self, events: &mut Vec<String>) {
        if !self.text_msg_started || self.text_accumulator.is_empty() {
            return;
        }
        let Some(text_msg_id) = self.text_msg_id.clone() else {
            return;
        };
        let idx = self.text_content_index;
        let seq = self.next_seq();
        events.push(self.emit(serde_json::json!({
            "sequence_number": seq,
            "object": "content",
            "status": "completed",
            "error": null,
            "type": "text",
            "index": idx,
            "delta": null,
            "msg_id": &text_msg_id,
            "text": &self.text_accumulator
        })));
        let seq = self.next_seq();
        let msg_completed = serde_json::json!({
            "sequence_number": seq,
            "object": "message",
            "status": "completed",
            "error": null,
            "id": &text_msg_id,
            "type": "message",
            "role": "assistant",
            "content": [{
                "sequence_number": null, "object": "content",
                "status": "completed", "error": null,
                "type": "text", "index": idx, "delta": null,
                "msg_id": &text_msg_id,
                "text": &self.text_accumulator
            }],
            "code": null, "message": null, "usage": null, "metadata": null
        });
        events.push(self.emit(msg_completed.clone()));
        self.completed_messages.push(msg_completed);
        self.text_content_index += 1;
        self.text_msg_started = false;
        self.text_accumulator.clear();
        self.text_msg_id = None;
    }

    pub fn map_multi(&mut self, result: Result<RuntimeEvent, String>) -> Vec<String> {
        match result {
            Ok(event) => self.map_event(event),
            Err(e) => {
                vec![serde_json::json!({"type": "error", "message": e}).to_string()]
            }
        }
    }

    pub fn map_event(&mut self, event: RuntimeEvent) -> Vec<String> {
        let mut events = Vec::new();
        match event {
            RuntimeEvent::Start { .. } => {
                self.ensure_response_started(&mut events);
            }

            RuntimeEvent::ModelCallReset { model_call_id } => {
                self.text_msg_id = None;
                self.text_accumulator.clear();
                self.text_msg_started = false;
                self.thinking_msg_id = None;
                self.reasoning_accumulator.clear();
                self.thinking_msg_started = false;
                self.tool_call_delta_state.clear();
                self.tool_call_delta_opened.clear();
                let seq = self.next_seq();
                events.push(self.emit(serde_json::json!({
                    "sequence_number": seq,
                    "type": "model_call_reset",
                    "model_call_id": model_call_id,
                })));
            }

            RuntimeEvent::TextDelta { delta, message_id } => {
                let Some(message_id) = message_id.map(|id| id.to_string()) else {
                    return vec![missing_canonical_message_id("text_delta")];
                };
                self.ensure_response_started(&mut events);
                self.close_reasoning(&mut events);
                if self.text_msg_id.as_deref() != Some(message_id.as_str()) {
                    self.close_text(&mut events);
                    self.text_msg_id = Some(message_id.clone());
                }
                if !self.text_msg_started {
                    self.text_msg_started = true;
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "message",
                        "status": "in_progress",
                        "error": null,
                        "id": &message_id,
                        "type": "message",
                        "role": "assistant",
                        "content": null,
                        "code": null, "message": null, "usage": null, "metadata": null
                    })));
                }
                self.text_accumulator.push_str(&delta);
                let seq = self.next_seq();
                events.push(self.emit(serde_json::json!({
                    "sequence_number": seq,
                    "object": "content",
                    "status": "in_progress",
                    "error": null,
                    "type": "text",
                    "index": self.text_content_index,
                    "delta": true,
                    "msg_id": &message_id,
                    "text": &delta
                })));
            }

            RuntimeEvent::ThinkingDelta {
                delta, message_id, ..
            } => {
                let Some(message_id) = message_id.map(|id| id.to_string()) else {
                    return vec![missing_canonical_message_id("thinking_delta")];
                };
                self.ensure_response_started(&mut events);
                self.close_text(&mut events);
                if self.thinking_msg_id.as_deref() != Some(message_id.as_str()) {
                    self.close_reasoning(&mut events);
                    self.thinking_msg_id = Some(message_id.clone());
                }
                if !self.thinking_msg_started {
                    self.thinking_msg_started = true;
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "message",
                        "status": "in_progress",
                        "error": null,
                        "id": &message_id,
                        "type": "reasoning",
                        "role": "assistant",
                        "content": null,
                        "code": null, "message": null, "usage": null, "metadata": null
                    })));
                }
                self.reasoning_accumulator.push_str(&delta);
                let seq = self.next_seq();
                events.push(self.emit(serde_json::json!({
                    "sequence_number": seq,
                    "object": "content",
                    "status": "in_progress",
                    "error": null,
                    "type": "text",
                    "index": self.thinking_content_index,
                    "delta": true,
                    "msg_id": &message_id,
                    "text": &delta
                })));
            }

            RuntimeEvent::ToolCall { call } => {
                self.ensure_response_started(&mut events);
                self.close_reasoning(&mut events);
                self.close_text(&mut events);
                let id = call.id.as_str().to_string();
                if id.is_empty() {
                    return vec![missing_canonical_message_id("tool_call")];
                }
                let msg_id = canonical_message_id(&call.metadata).unwrap_or_else(|| {
                    agent_types::conversation_message_identity::tool_use_message_id(&id)
                });
                let name = call.name.clone();
                let arguments = call.arguments.to_string();
                let widget_already_opened = self.tool_call_delta_opened.contains(&id)
                    || self.tool_call_names.contains_key(&id);
                self.tool_call_names.insert(id.clone(), name.clone());

                if !widget_already_opened {
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "message",
                        "status": "in_progress",
                        "error": null,
                        "id": &msg_id,
                        "type": "plugin_call",
                        "role": "assistant",
                        "content": null,
                        "code": null, "message": null, "usage": null, "metadata": null
                    })));
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "content",
                        "status": "in_progress",
                        "error": null,
                        "type": "data",
                        "index": null,
                        "delta": false,
                        "msg_id": &msg_id,
                        "data": { "call_id": &id, "name": &name, "arguments": "" }
                    })));
                }
                let seq = self.next_seq();
                events.push(self.emit(serde_json::json!({
                    "sequence_number": seq,
                    "object": "content",
                    "status": "completed",
                    "error": null,
                    "type": "data",
                    "index": 0,
                    "delta": false,
                    "msg_id": &msg_id,
                    "data": { "call_id": &id, "name": &name, "arguments": &arguments }
                })));
                let seq = self.next_seq();
                let msg_completed = serde_json::json!({
                    "sequence_number": seq,
                    "object": "message",
                    "status": "completed",
                    "error": null,
                    "id": &msg_id,
                    "type": "plugin_call",
                    "role": "assistant",
                    "content": [{
                        "sequence_number": null, "object": "content",
                        "status": "completed", "error": null,
                        "type": "data", "index": 0, "delta": null,
                        "msg_id": &msg_id,
                        "data": { "call_id": &id, "name": &name, "arguments": &arguments }
                    }],
                    "code": null, "message": null, "usage": null, "metadata": null
                });
                events.push(self.emit(msg_completed.clone()));
                upsert_completed_message(&mut self.completed_messages, msg_completed);
            }

            RuntimeEvent::ToolResult { result } => {
                let id = result.call_id.as_str().to_string();
                if id.is_empty() {
                    return vec![missing_canonical_message_id("tool_result")];
                }
                let msg_id = canonical_message_id(&result.metadata).unwrap_or_else(|| {
                    agent_types::conversation_message_identity::tool_result_message_id(&id)
                });
                let content = result.text_content();
                let tool_name = self.tool_call_names.get(&id).cloned().unwrap_or_default();
                let already_opened = self.tool_result_delta_opened.contains(&id);
                if !already_opened {
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "message",
                        "status": "in_progress",
                        "error": null,
                        "id": &msg_id,
                        "type": "plugin_call_output",
                        "role": "tool",
                        "content": null,
                        "code": null, "message": null, "usage": null, "metadata": null
                    })));
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "content",
                        "status": "in_progress",
                        "error": null,
                        "type": "data",
                        "index": 0,
                        "delta": false,
                        "msg_id": &msg_id,
                        "data": { "call_id": &id, "name": &tool_name, "output": "" }
                    })));
                }
                let seq = self.next_seq();
                events.push(self.emit(serde_json::json!({
                    "sequence_number": seq,
                    "object": "content",
                    "status": "completed",
                    "error": null,
                    "type": "data",
                    "index": 0,
                    "delta": false,
                    "msg_id": &msg_id,
                    "data": { "call_id": &id, "name": &tool_name, "output": &content }
                })));
                let seq = self.next_seq();
                let msg_completed = serde_json::json!({
                    "sequence_number": seq,
                    "object": "message",
                    "status": "completed",
                    "error": null,
                    "id": &msg_id,
                    "type": "plugin_call_output",
                    "role": "tool",
                    "content": [{
                        "sequence_number": null, "object": "content",
                        "status": "completed", "error": null,
                        "type": "data", "index": 0, "delta": null,
                        "msg_id": &msg_id,
                        "data": { "call_id": &id, "name": &tool_name, "output": &content }
                    }],
                    "code": null, "message": null, "usage": null, "metadata": null
                });
                events.push(self.emit(msg_completed.clone()));
                self.completed_messages.push(msg_completed);
                self.tool_result_output_state.remove(&id);
                self.tool_result_delta_opened.remove(&id);

                self.emit_media_message(&id, &result.content, &mut events);

                self.text_msg_id = None;
                self.text_msg_started = false;
                self.text_content_index = 0;
                self.thinking_msg_id = None;
                self.thinking_msg_started = false;
                self.thinking_content_index = 0;
            }

            RuntimeEvent::ToolResultDelta {
                id,
                name,
                output_delta,
            } => {
                if output_delta.is_empty() {
                    return events;
                }
                self.ensure_response_started(&mut events);
                self.close_reasoning(&mut events);
                self.close_text(&mut events);

                let tool_name = name
                    .or_else(|| self.tool_call_names.get(&id).cloned())
                    .unwrap_or_default();
                let msg_id =
                    agent_types::conversation_message_identity::tool_result_message_id(&id);
                if self.tool_result_delta_opened.insert(id.clone()) {
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "message",
                        "status": "in_progress",
                        "error": null,
                        "id": &msg_id,
                        "type": "plugin_call_output",
                        "role": "tool",
                        "content": null,
                        "code": null, "message": null, "usage": null, "metadata": null
                    })));
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "content",
                        "status": "in_progress",
                        "error": null,
                        "type": "data",
                        "index": 0,
                        "delta": false,
                        "msg_id": &msg_id,
                        "data": { "call_id": &id, "name": &tool_name, "output": "" }
                    })));
                }

                let output = {
                    let entry = self.tool_result_output_state.entry(id.clone()).or_default();
                    entry.push_str(&output_delta);
                    entry.clone()
                };
                let seq = self.next_seq();
                events.push(self.emit(serde_json::json!({
                    "sequence_number": seq,
                    "object": "content",
                    "status": "in_progress",
                    "error": null,
                    "type": "data",
                    "index": 0,
                    "delta": false,
                    "msg_id": &msg_id,
                    "data": { "call_id": &id, "name": &tool_name, "output": &output }
                })));
            }

            RuntimeEvent::Usage {
                input_tokens,
                output_tokens,
                total_tokens,
                ..
            } => {
                self.usage_data = Some((input_tokens, output_tokens, total_tokens));
            }

            RuntimeEvent::Error { error } => {
                self.ensure_response_started(&mut events);
                let error_detail = error.to_string();
                let error_code = serde_json::to_value(error)
                    .ok()
                    .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(String::from))
                    .unwrap_or_else(|| "unknown".to_string());
                let msg_id = format!("msg-error-{}", uuid::Uuid::new_v4());
                let seq = self.next_seq();
                events.push(self.emit(serde_json::json!({
                    "sequence_number": seq,
                    "object": "message",
                    "id": msg_id,
                    "type": "error",
                    "role": "system",
                    "status": "completed",
                    "content": [{"type": "text", "text": &error_detail}],
                    "code": error_code,
                    "message": &error_detail,
                    "error": null,
                    "usage": null,
                    "metadata": null
                })));
            }

            RuntimeEvent::ToolApprovalPending {
                call_id,
                tool_name,
                request_id,
                prompt,
            } => {
                self.ensure_response_started(&mut events);
                let msg_id = format!("msg-approval-{call_id}");
                let seq = self.next_seq();
                events.push(self.emit(serde_json::json!({
                    "sequence_number": seq,
                    "object": "message",
                    "status": "in_progress",
                    "error": null,
                    "id": &msg_id,
                    "type": "tool_approval_request",
                    "role": "assistant",
                    "content": [{"type": "tool_approval_request", "data": {
                        "id": request_id.as_deref().unwrap_or(&call_id),
                        "approval_request_id": request_id.as_deref().unwrap_or(&call_id),
                        "call_id": &call_id,
                        "name": &tool_name,
                        "prompt": prompt,
                        "status": "waiting"
                    }}],
                    "code": null, "message": null, "usage": null, "metadata": null
                })));
            }

            RuntimeEvent::AskUser {
                request_id,
                questions,
            } => {
                self.ensure_response_started(&mut events);
                self.close_reasoning(&mut events);
                self.close_text(&mut events);
                let msg_id = format!("msg-ask-{request_id}");
                let prompt = questions.first().map(|question| question.question.clone());
                let ask_data = serde_json::json!({
                    "id": &request_id,
                    "request_id": &request_id,
                    "prompt": prompt,
                    "questions": &questions,
                    "status": "waiting",
                });
                let seq = self.next_seq();
                events.push(self.emit(serde_json::json!({
                    "sequence_number": seq,
                    "object": "message",
                    "status": "in_progress",
                    "error": null,
                    "id": &msg_id,
                    "type": "ask_user_request",
                    "role": "assistant",
                    "content": null,
                    "code": null, "message": null, "usage": null, "metadata": null
                })));
                let seq = self.next_seq();
                events.push(self.emit(serde_json::json!({
                    "sequence_number": seq,
                    "object": "content",
                    "status": "in_progress",
                    "error": null,
                    "type": "ask_user_request",
                    "index": null,
                    "delta": false,
                    "msg_id": &msg_id,
                    "data": &ask_data
                })));
                let seq = self.next_seq();
                events.push(self.emit(serde_json::json!({
                    "sequence_number": seq,
                    "object": "content",
                    "status": "completed",
                    "error": null,
                    "type": "ask_user_request",
                    "index": 0,
                    "delta": false,
                    "msg_id": &msg_id,
                    "data": &ask_data
                })));
                let seq = self.next_seq();
                let msg_completed = serde_json::json!({
                    "sequence_number": seq,
                    "object": "message",
                    "status": "completed",
                    "error": null,
                    "id": &msg_id,
                    "type": "ask_user_request",
                    "role": "assistant",
                    "content": [{
                        "sequence_number": null, "object": "content",
                        "status": "completed", "error": null,
                        "type": "ask_user_request", "index": 0, "delta": null,
                        "msg_id": &msg_id,
                        "data": &ask_data
                    }],
                    "code": null, "message": null, "usage": null, "metadata": null
                });
                events.push(self.emit(msg_completed.clone()));
                self.completed_messages.push(msg_completed);
            }

            RuntimeEvent::End { finish_reason } => {
                let status = match finish_reason.as_deref() {
                    Some("cancelled") | Some("canceled") => "canceled",
                    Some("interjected" | "max_iterations") => "interrupted",
                    Some("error") => "failed",
                    _ => "completed",
                };
                if status == "completed" || finish_reason.as_deref() == Some("max_iterations") {
                    self.close_reasoning(&mut events);
                    self.close_text(&mut events);
                }
                let usage = match self.usage_data {
                    Some((input, output, total)) => serde_json::json!({
                        "prompt_tokens": input,
                        "completion_tokens": output,
                        "total_tokens": total
                    }),
                    None => serde_json::Value::Null,
                };
                let seq = self.next_seq();
                let mut body = serde_json::json!({
                    "sequence_number": seq,
                    "object": "response",
                    "status": status,
                    "error": null,
                    "id": &self.response_id,
                    "created_at": 0,
                    "completed_at": 0,
                    "output": &self.completed_messages,
                    "usage": usage,
                    "chat_id": &self.chat_id
                });
                if let Some(reason) = finish_reason {
                    body["finish_reason"] = serde_json::Value::String(reason);
                }
                events.push(self.emit(body));
            }

            RuntimeEvent::IterationStart { .. } => {
                self.close_reasoning(&mut events);
                self.close_text(&mut events);
                self.text_msg_id = None;
                self.text_msg_started = false;
                self.text_content_index = 0;
                self.thinking_msg_id = None;
                self.thinking_msg_started = false;
                self.thinking_content_index = 0;
            }

            RuntimeEvent::IterationEnd { .. } => {}

            RuntimeEvent::ToolCallDelta {
                id,
                index,
                name,
                arguments_delta,
            } => {
                self.ensure_response_started(&mut events);
                self.close_reasoning(&mut events);
                self.close_text(&mut events);

                let (
                    resolved_id,
                    resolved_name,
                    just_started,
                    name_changed_after_start,
                    delta_idx_for_emit,
                ) = {
                    let entry = self.tool_call_delta_state.entry(index).or_insert_with(|| {
                        ToolCallDeltaState {
                            id: String::new(),
                            name: String::new(),
                            started: false,
                            delta_index: 0,
                        }
                    });
                    if !id.is_empty() {
                        entry.id = id;
                    }
                    let mut name_changed_after_start = false;
                    if let Some(n) = name {
                        if !n.is_empty() && entry.name != n {
                            name_changed_after_start = entry.started;
                            entry.name = n;
                        }
                    }
                    let just_started = if !entry.started && !entry.id.is_empty() {
                        entry.started = true;
                        true
                    } else {
                        false
                    };
                    let delta_idx_for_emit = if !arguments_delta.is_empty() {
                        let idx = entry.delta_index;
                        entry.delta_index += 1;
                        Some(idx)
                    } else {
                        None
                    };
                    (
                        entry.id.clone(),
                        entry.name.clone(),
                        just_started,
                        name_changed_after_start,
                        delta_idx_for_emit,
                    )
                };

                if resolved_id.is_empty() {
                    return events;
                }

                let msg_id =
                    agent_types::conversation_message_identity::tool_use_message_id(&resolved_id);
                if !resolved_name.is_empty() {
                    self.tool_call_names
                        .insert(resolved_id.clone(), resolved_name.clone());
                }
                if just_started {
                    self.tool_call_delta_opened.insert(resolved_id.clone());
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "message",
                        "status": "in_progress",
                        "error": null,
                        "id": &msg_id,
                        "type": "plugin_call",
                        "role": "assistant",
                        "content": null,
                        "code": null, "message": null, "usage": null, "metadata": null
                    })));
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "content",
                        "status": "in_progress",
                        "error": null,
                        "type": "data",
                        "index": 0,
                        "delta": false,
                        "msg_id": &msg_id,
                        "data": {
                            "call_id": &resolved_id,
                            "name": &resolved_name,
                            "arguments": ""
                        }
                    })));
                } else if name_changed_after_start {
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "content",
                        "status": "in_progress",
                        "error": null,
                        "type": "data",
                        "index": 0,
                        "delta": false,
                        "msg_id": &msg_id,
                        "data": {
                            "call_id": &resolved_id,
                            "name": &resolved_name
                        }
                    })));
                }

                if let Some(delta_idx) = delta_idx_for_emit {
                    let seq = self.next_seq();
                    events.push(self.emit(serde_json::json!({
                        "sequence_number": seq,
                        "object": "content",
                        "status": "in_progress",
                        "error": null,
                        "type": "data",
                        "index": delta_idx,
                        "delta": true,
                        "msg_id": &msg_id,
                        "data": {
                            "call_id": &resolved_id,
                            "name": &resolved_name,
                            "arguments_delta": &arguments_delta
                        }
                    })));
                }
            }

            RuntimeEvent::ToolApprovalRequest {
                id,
                name,
                arguments,
            } => {
                self.ensure_response_started(&mut events);
                let msg_id = format!("msg-approval-{id}");
                let seq = self.next_seq();
                let approval_msg = serde_json::json!({
                    "sequence_number": seq,
                    "object": "message",
                    "status": "completed",
                    "error": null,
                    "id": &msg_id,
                    "type": "tool_approval_request",
                    "role": "assistant",
                    "content": [{
                        "type": "tool_approval_request",
                        "data": {
                            "id": id,
                            "name": name,
                            "arguments": arguments
                        }
                    }],
                    "code": null, "message": null, "usage": null, "metadata": null
                });
                events.push(self.emit(approval_msg.clone()));
                self.completed_messages.push(approval_msg);
            }

            RuntimeEvent::ContextCompaction {
                status,
                mode,
                message,
            } => {
                events.push(self.emit(serde_json::json!({
                    "type": "context_compaction",
                    "status": status,
                    "mode": mode,
                    "message": message,
                })));
            }
        }
        events
    }

    fn emit_media_message(
        &mut self,
        tool_call_id: &str,
        parts: &[ContentPart],
        events: &mut Vec<String>,
    ) {
        let media_parts: Vec<&ContentPart> = parts
            .iter()
            .filter(|p| matches!(p, ContentPart::Image { .. } | ContentPart::Video { .. }))
            .collect();
        if media_parts.is_empty() {
            return;
        }

        let media_msg_id =
            agent_types::conversation_message_identity::tool_result_attachment_message_id(
                tool_call_id,
            );

        let seq = self.next_seq();
        events.push(self.emit(serde_json::json!({
            "sequence_number": seq,
            "object": "message",
            "status": "in_progress",
            "error": null,
            "id": &media_msg_id,
            "type": "message",
            "role": "assistant",
            "content": null,
            "code": null, "message": null, "usage": null, "metadata": null
        })));

        let mut completed_contents = Vec::new();

        for (i, part) in media_parts.iter().enumerate() {
            let content_json = match part {
                ContentPart::Image { url, .. } => {
                    serde_json::json!({
                        "sequence_number": null,
                        "object": "content",
                        "status": "completed",
                        "error": null,
                        "type": "image",
                        "index": i,
                        "delta": false,
                        "msg_id": &media_msg_id,
                        "image_url": url
                    })
                }
                ContentPart::Video { url, .. } => {
                    serde_json::json!({
                        "sequence_number": null,
                        "object": "content",
                        "status": "completed",
                        "error": null,
                        "type": "video",
                        "index": i,
                        "delta": false,
                        "msg_id": &media_msg_id,
                        "video_url": url
                    })
                }
                _ => continue,
            };

            let seq = self.next_seq();
            let mut streamed = content_json.clone();
            streamed["sequence_number"] = serde_json::json!(seq);
            events.push(self.emit(streamed));

            completed_contents.push(content_json);
        }

        let seq = self.next_seq();
        let msg_completed = serde_json::json!({
            "sequence_number": seq,
            "object": "message",
            "status": "completed",
            "error": null,
            "id": &media_msg_id,
            "type": "message",
            "role": "assistant",
            "content": completed_contents,
            "code": null, "message": null, "usage": null, "metadata": null
        });
        events.push(self.emit(msg_completed.clone()));
        self.completed_messages.push(msg_completed);
    }
}

pub fn make_response_done_json(chat_id: &str, finish_reason: Option<&str>) -> String {
    let mut body = serde_json::json!({
        "object": "response",
        "status": "completed",
        "error": null,
        "id": format!("resp-{chat_id}"),
        "created_at": 0,
        "completed_at": null,
        "output": null,
        "usage": null,
        "chat_id": chat_id,
    });
    if let Some(reason) = finish_reason {
        body["finish_reason"] = serde_json::Value::String(reason.into());
    }
    body.to_string()
}

fn upsert_completed_message(
    completed_messages: &mut Vec<serde_json::Value>,
    incoming: serde_json::Value,
) {
    let incoming_id = incoming.get("id").and_then(serde_json::Value::as_str);
    if let Some(incoming_id) = incoming_id {
        if let Some(existing) = completed_messages.iter_mut().find(|message| {
            message.get("id").and_then(serde_json::Value::as_str) == Some(incoming_id)
        }) {
            *existing = incoming;
            return;
        }
    }
    completed_messages.push(incoming);
}

fn missing_canonical_message_id(event_type: &str) -> String {
    serde_json::json!({
        "type": "error",
        "message": format!("{event_type} is missing canonical message_id"),
    })
    .to_string()
}

fn canonical_message_id(metadata: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    metadata
        .get(agent_types::conversation_message_identity::CANONICAL_MESSAGE_ID_METADATA_KEY)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[derive(Debug, Clone, Default)]
pub struct ProcessOutput {
    pub finish_reason: Option<String>,
    pub assistant_started_at: Option<String>,
    pub usage: Option<serde_json::Value>,
    pub last_call_usage: Option<serde_json::Value>,
    pending_call_usage: Option<serde_json::Value>,
}

pub struct ProcessRequest {
    pub agent_id: String,
    pub chat_id: String,
    pub messages: Vec<AgentInputMessage>,
    pub model_override: Option<String>,
    pub workspace_dir: Option<PathBuf>,
    pub conversation_store_dir: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
    pub cancel_rx: tokio::sync::watch::Receiver<RunControl>,
    pub user_message: String,
    pub persistence: Arc<dyn SessionPersistence>,
    pub disabled_tools: Vec<String>,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[async_trait]
pub trait SessionPersistence: Send + Sync {
    async fn save_messages(
        &self,
        agent_id: &str,
        chat_id: &str,
        messages: &[AgentInputMessage],
    ) -> rua_core::Result<()>;

    async fn save_messages_with_assistant_started_at(
        &self,
        agent_id: &str,
        chat_id: &str,
        messages: &[AgentInputMessage],
        _assistant_started_at: Option<&str>,
    ) -> rua_core::Result<()> {
        self.save_messages(agent_id, chat_id, messages).await
    }

    async fn save_messages_with_token_usage(
        &self,
        agent_id: &str,
        chat_id: &str,
        messages: &[AgentInputMessage],
        assistant_started_at: Option<&str>,
        _usage: Option<serde_json::Value>,
        _last_call_usage: Option<serde_json::Value>,
    ) -> rua_core::Result<()> {
        self.save_messages_with_assistant_started_at(
            agent_id,
            chat_id,
            messages,
            assistant_started_at,
        )
        .await
    }

    async fn finalize_turn(
        &self,
        _agent_id: &str,
        _chat_id: &str,
        _assistant_started_at: Option<&str>,
        _usage: Option<serde_json::Value>,
        _last_call_usage: Option<serde_json::Value>,
    ) -> rua_core::Result<()> {
        Ok(())
    }

    async fn save_turn_aborted(&self, _agent_id: &str, _chat_id: &str) -> rua_core::Result<()> {
        Ok(())
    }

    async fn save_turn_interjected(&self, _agent_id: &str, _chat_id: &str) -> rua_core::Result<()> {
        Ok(())
    }

    async fn save_chat_entry(
        &self,
        agent_id: &str,
        chat_id: &str,
        chat_name: &str,
    ) -> rua_core::Result<()>;
}

#[derive(Clone)]
pub struct ConsoleChannelService {
    runtime: Arc<dyn AgentRuntime>,
}

impl ConsoleChannelService {
    pub fn new(runtime: Arc<dyn AgentRuntime>) -> Self {
        Self { runtime }
    }

    pub fn process(
        &self,
        req: ProcessRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<RuntimeEvent, String>> + Send + 'static>> {
        let runtime = Arc::clone(&self.runtime);
        let agent_id = req.agent_id;
        let chat_id = req.chat_id;
        let user_message = req.user_message;
        let messages = req.messages;
        let model_override = req.model_override;
        let workspace_dir = req.workspace_dir;
        let conversation_store_dir = req.conversation_store_dir;
        let working_dir = req.working_dir;
        let control_rx = req.cancel_rx;
        let persistence = req.persistence;
        let disabled_tools = req.disabled_tools;
        let metadata = req.metadata;

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<RuntimeEvent, String>>(32);
        let panic_tx = tx.clone();
        let panic_agent_id = agent_id.clone();
        let panic_chat_id = chat_id.clone();
        let panic_user_message = user_message.clone();
        let panic_persistence = Arc::clone(&persistence);

        let process_task = tokio::spawn(async move {
            let (agent_cancel_tx, agent_cancel_rx) =
                tokio::sync::watch::channel(control_rx.borrow().is_stop());
            let session_id = rua_core::id::SessionId::new(&chat_id);
            let agent_req = AgentRequest {
                session_id,
                user_id: "console".to_string(),
                agent_id: rua_core::id::AgentId::new(&agent_id),
                messages,
                model_override,
                tool_mode: "auto".to_string(),
                attachments: vec![],
                display_preferences: None,
                disabled_tools,
                metadata,
                cancel: Some(agent_cancel_rx),
                run_control: Some(control_rx.clone()),
                workspace_dir,
                conversation_store_dir,
                working_dir,
            };

            let mut event_stream = match runtime.stream(agent_req).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "failed to start agent runtime stream");
                    let _ = tx
                        .send(Ok(RuntimeEvent::Start {
                            session_id: rua_core::id::SessionId::new(&chat_id),
                            agent_id: Some(rua_core::id::AgentId::new(&agent_id)),
                        }))
                        .await;
                    if let Err(persist_error) = persistence
                        .finalize_turn(&agent_id, &chat_id, None, None, None)
                        .await
                    {
                        warn!(
                            error = %persist_error,
                            "failed to finalize Turn after runtime startup failure"
                        );
                    }
                    if let Err(persist_error) = persistence
                        .save_chat_entry(&agent_id, &chat_id, &user_message)
                        .await
                    {
                        warn!(
                            error = %persist_error,
                            "failed to save chat entry after runtime startup failure"
                        );
                    }
                    let _ = tx
                        .send(Ok(RuntimeEvent::Error {
                            error: rua_core::RuaError::Runtime {
                                message: format!("Agent runtime error: {e}"),
                            },
                        }))
                        .await;
                    let _ = tx
                        .send(Ok(RuntimeEvent::End {
                            finish_reason: Some("error".into()),
                        }))
                        .await;
                    return;
                }
            };

            debug!(
                chat_id = %chat_id,
                "ConsoleChannelService.process: streaming started"
            );

            let mut control_rx = control_rx;
            let mut control_open = true;
            let mut output = ProcessOutput::default();
            let mut terminal_event: Option<Result<RuntimeEvent, String>> = None;

            loop {
                tokio::select! {
                biased;
                changed = control_rx.changed(), if control_open => {
                    match changed {
                        Ok(()) => {
                            let control = *control_rx.borrow();
                            if control.is_terminal() {
                                debug!(chat_id = %chat_id, control = ?control, "process stream controlled");
                                if control.is_stop() {
                                    let _ = agent_cancel_tx.send(true);
                                    output.finish_reason = Some("cancelled".into());
                                    terminal_event = Some(Ok(RuntimeEvent::End {
                                        finish_reason: Some("cancelled".into()),
                                    }));
                                    break;
                                }
                            }
                        }
                        Err(_) => control_open = false,
                    }
                }
                item = event_stream.next() => {
                    match item {
                        Some(Ok(event)) => {
                            let is_end = matches!(
                                &event,
                                RuntimeEvent::End { .. } | RuntimeEvent::Error { .. }
                            );
                            if matches!(
                                &event,
                                RuntimeEvent::ThinkingDelta { .. }
                                    | RuntimeEvent::TextDelta { .. }
                                    | RuntimeEvent::ToolCall { .. }
                                    | RuntimeEvent::ToolCallDelta { .. }
                            ) {
                                mark_assistant_started(&mut output);
                            }
                            if let RuntimeEvent::Usage {
                                input_tokens,
                                output_tokens,
                                total_tokens,
                                cache_read_tokens,
                                cache_write_tokens,
                            } = &event
                            {
                                let usage = serde_json::json!({
                                    "prompt_tokens": input_tokens.unwrap_or(0),
                                    "completion_tokens": output_tokens.unwrap_or(0),
                                    "total_tokens": total_tokens.unwrap_or_else(|| {
                                        input_tokens.unwrap_or(0) + output_tokens.unwrap_or(0)
                                    }),
                                    "cache_read_tokens": cache_read_tokens,
                                    "cache_write_tokens": cache_write_tokens,
                                });
                                output.usage = Some(usage.clone());
                                output.pending_call_usage = Some(usage);
                            }
                            if let RuntimeEvent::IterationEnd { .. } = event {
                                if let Some(usage) = output.pending_call_usage.take() {
                                    output.last_call_usage = Some(usage);
                                }
                            }
                            if is_end {
                                output.finish_reason = match &event {
                                    RuntimeEvent::End { finish_reason } => {
                                        finish_reason.clone()
                                    }
                                    RuntimeEvent::Error { .. } => {
                                        Some("error".into())
                                    }
                                    _ => None,
                                };
                                terminal_event = Some(Ok(event));
                                break;
                            }
                            if tx.send(Ok(event)).await.is_err() {
                                break;
                            }
                        }
                        Some(Err(e)) => {
                            warn!(error = %e, "runtime stream error");
                            output.finish_reason = Some("error".into());
                            terminal_event = Some(Err(e.to_string()));
                            break;
                        }
                        None => {
                            output.finish_reason = Some("stop".into());
                            terminal_event = Some(Ok(RuntimeEvent::End {
                                finish_reason: Some("stop".into()),
                            }));
                            break;
                        }
                    }
                }
                }
            }

            if output.last_call_usage.is_none() {
                output.last_call_usage = output.pending_call_usage.clone();
            }

            if let Err(e) = persistence
                .finalize_turn(
                    &agent_id,
                    &chat_id,
                    output.assistant_started_at.as_deref(),
                    output.usage.clone(),
                    output.last_call_usage.clone(),
                )
                .await
            {
                warn!(error = %e, "failed to finalize Conversation Turn");
            }
            if output.finish_reason.as_deref() == Some("cancelled") {
                if let Err(e) = persistence.save_turn_aborted(&agent_id, &chat_id).await {
                    warn!(error = %e, "failed to save user stop marker");
                }
            }
            if output.finish_reason.as_deref() == Some("interjected") {
                if let Err(e) = persistence.save_turn_interjected(&agent_id, &chat_id).await {
                    warn!(error = %e, "failed to save interjection marker");
                }
            }

            if let Err(e) = persistence
                .save_chat_entry(&agent_id, &chat_id, &user_message)
                .await
            {
                warn!(error = %e, "failed to save chat entry");
            }
            if let Some(event) = terminal_event {
                let _ = tx.send(event).await;
            }
        });

        tokio::spawn(async move {
            if let Err(join_error) = process_task.await {
                let error_detail = if join_error.is_panic() {
                    let payload = join_error.into_panic();
                    format!(
                        "console process task panicked: {}",
                        panic_payload_message(payload.as_ref())
                    )
                } else {
                    format!("console process task failed: {join_error}")
                };
                error!(
                    agent_id = %panic_agent_id,
                    chat_id = %panic_chat_id,
                    error = %error_detail,
                    "console process task panicked"
                );
                if let Err(persist_error) = panic_persistence
                    .finalize_turn(&panic_agent_id, &panic_chat_id, None, None, None)
                    .await
                {
                    warn!(
                        error = %persist_error,
                        agent_id = %panic_agent_id,
                        chat_id = %panic_chat_id,
                        "failed to finalize Turn after console process panic"
                    );
                }
                if let Err(persist_error) = panic_persistence
                    .save_chat_entry(&panic_agent_id, &panic_chat_id, &panic_user_message)
                    .await
                {
                    warn!(
                        error = %persist_error,
                        agent_id = %panic_agent_id,
                        chat_id = %panic_chat_id,
                        "failed to save chat entry after console process panic"
                    );
                }
                let _ = panic_tx
                    .send(Ok(RuntimeEvent::Error {
                        error: rua_core::RuaError::Runtime {
                            message: error_detail,
                        },
                    }))
                    .await;
                let _ = panic_tx
                    .send(Ok(RuntimeEvent::End {
                        finish_reason: Some("error".into()),
                    }))
                    .await;
            }
        });

        Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        }))
    }
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .map(|message| message.as_str())
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic")
        .to_string()
}

fn mark_assistant_started(output: &mut ProcessOutput) {
    output
        .assistant_started_at
        .get_or_insert_with(|| Utc::now().to_rfc3339());
}
