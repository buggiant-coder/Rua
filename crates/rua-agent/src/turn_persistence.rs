use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use agent_types::conversation_message_identity::{
    tool_result_message_id, tool_use_message_id, CANONICAL_MESSAGE_ID_METADATA_KEY,
    REASONING_MESSAGE_ID_METADATA_KEY, TEXT_MESSAGE_ID_METADATA_KEY,
};
use agent_types::{ContentPart, ModelDialogueItem, Role, TokenUsage, ToolCall};
use async_trait::async_trait;
use futures::StreamExt;
use react_agent::{AgentError, AgentHook};
use rua_contract::agent::{AgentRequest, RuntimeCommandStream};
use rua_core::{RuaError, RuntimeEvent};
use rua_session::{NewConversationMessage, TurnWriter, WorkspaceSessionStore};

pub const LOADED_MESSAGE_COUNT_METADATA_KEY: &str = "loaded_message_count";
pub const AUTHORITY_DEVICE_ID_METADATA_KEY: &str = "authority_device_id";
pub const AUTHORITY_EPOCH_METADATA_KEY: &str = "authority_epoch";
pub const RUA_TOKEN_USAGE_METADATA_KEY: &str = "rua_token_usage";
pub const REGENERATE_TURN_ID_METADATA_KEY: &str = "regenerate_turn_id";
pub const ACP_ORIGIN_RUNNER_METADATA_KEY: &str = "acp_origin_runner";

pub struct TurnPersistenceHook {
    conversation_store_dir: PathBuf,
    working_dir: PathBuf,
    display_media: Mutex<rua_session::display_media::DisplayMediaArchive>,
    chat_id: String,
    turn_id: String,
    agent_id: String,
    device_id: String,
    authority_device_id: String,
    authority_epoch: u64,
    acp_origin_runner: Option<String>,
    max_input_length: Option<u64>,
    regenerate_variant_started: Mutex<bool>,
    iteration_limit_reached: AtomicBool,
}

impl TurnPersistenceHook {
    pub fn from_request(request: &AgentRequest) -> Option<Self> {
        let conversation_store_dir = request.conversation_store_dir()?.to_path_buf();
        let turn_id = request.metadata.get("turn_id")?.as_str()?.to_string();
        let regenerate = request
            .metadata
            .get(REGENERATE_TURN_ID_METADATA_KEY)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|regenerate_turn_id| regenerate_turn_id == turn_id);
        if !regenerate
            && request
                .metadata
                .get(LOADED_MESSAGE_COUNT_METADATA_KEY)
                .and_then(serde_json::Value::as_u64)
                .is_none()
        {
            return None;
        }
        Some(Self {
            working_dir: request
                .working_dir()
                .unwrap_or(&conversation_store_dir)
                .to_path_buf(),
            display_media: Mutex::new(Default::default()),
            conversation_store_dir,
            chat_id: request.session_id.to_string(),
            turn_id,
            agent_id: request.agent_id.to_string(),
            device_id: request
                .metadata
                .get("device_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            authority_device_id: request
                .metadata
                .get(AUTHORITY_DEVICE_ID_METADATA_KEY)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            authority_epoch: request
                .metadata
                .get(AUTHORITY_EPOCH_METADATA_KEY)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
            acp_origin_runner: request
                .metadata
                .get(ACP_ORIGIN_RUNNER_METADATA_KEY)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            max_input_length: None,
            regenerate_variant_started: Mutex::new(!regenerate),
            iteration_limit_reached: AtomicBool::new(false),
        })
    }

    pub fn with_max_input_length(mut self, max_input_length: Option<u64>) -> Self {
        self.max_input_length = max_input_length;
        self
    }

    fn conversation_messages(
        &self,
        messages: &[ModelDialogueItem],
    ) -> Result<Vec<NewConversationMessage>, AgentError> {
        let mut conversation_messages = Vec::new();
        for message in messages.iter().filter(|message| is_persistable(message)) {
            for item in message.split_assistant_messages() {
                conversation_messages.extend(self.conversation_messages_for_item(&item)?);
            }
        }
        Ok(conversation_messages)
    }

    fn attach_terminal_usage(
        &self,
        messages: &mut [NewConversationMessage],
        total_usage: &TokenUsage,
        last_call_usage: Option<&TokenUsage>,
    ) {
        if total_usage.is_empty() && last_call_usage.is_none() {
            return;
        }
        let Some(message) = messages
            .iter_mut()
            .rev()
            .find(|message| message.role == "assistant")
        else {
            return;
        };
        message.metadata.insert(
            RUA_TOKEN_USAGE_METADATA_KEY.to_string(),
            serde_json::json!({
                "usage": usage_value(total_usage),
                "last_call_usage": last_call_usage.map(usage_value),
                "max_input_length": self.max_input_length,
            }),
        );
    }

    fn conversation_messages_for_item(
        &self,
        message: &ModelDialogueItem,
    ) -> Result<Vec<NewConversationMessage>, AgentError> {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
            Role::System | Role::Developer => "system",
            _ => "system",
        };
        let mut metadata = message.metadata.clone();
        metadata.entry("name".to_string()).or_insert_with(|| {
            serde_json::Value::String(match message.role {
                Role::User => "user".to_string(),
                Role::Assistant => self.agent_id.clone(),
                _ => "system".to_string(),
            })
        });
        if !self.device_id.is_empty() {
            metadata
                .entry("device_id".to_string())
                .or_insert_with(|| serde_json::Value::String(self.device_id.clone()));
        }
        if !self.authority_device_id.is_empty() && self.authority_epoch > 0 {
            metadata
                .entry(AUTHORITY_DEVICE_ID_METADATA_KEY.to_string())
                .or_insert_with(|| serde_json::Value::String(self.authority_device_id.clone()));
            metadata
                .entry(AUTHORITY_EPOCH_METADATA_KEY.to_string())
                .or_insert_with(|| serde_json::json!(self.authority_epoch));
        }
        if let Some(runner) = self.acp_origin_runner.as_ref() {
            metadata.entry("origin".to_string()).or_insert_with(|| {
                serde_json::json!({
                    "kind": "acp",
                    "runner": runner,
                })
            });
        }
        let created_at = message
            .created_at
            .unwrap_or_else(chrono::Utc::now)
            .to_rfc3339();
        let mut groups: Vec<(String, Vec<ContentPart>)> = Vec::new();
        for part in &message.content {
            let message_id = canonical_content_message_id(message, part)?;
            if let Some((last_message_id, parts)) = groups.last_mut() {
                if last_message_id == &message_id {
                    parts.push(part.clone());
                    continue;
                }
            }
            if groups
                .iter()
                .any(|(existing_message_id, _)| existing_message_id == &message_id)
            {
                return Err(AgentError::Internal(format!(
                    "non-contiguous content reuses canonical message_id {message_id}"
                )));
            }
            groups.push((message_id, vec![part.clone()]));
        }

        groups
            .into_iter()
            .map(|(message_id, content)| {
                let mut message_metadata = metadata.clone();
                message_metadata.insert(
                    CANONICAL_MESSAGE_ID_METADATA_KEY.to_string(),
                    serde_json::Value::String(message_id.clone()),
                );
                Ok(NewConversationMessage {
                    message_id,
                    role: role.to_string(),
                    created_at: created_at.clone(),
                    status: "completed".to_string(),
                    content: serde_json::to_value(content)
                        .map_err(|error| AgentError::Internal(error.to_string()))?,
                    provider: None,
                    tool: None,
                    metadata: message_metadata,
                })
            })
            .collect()
    }
}

fn canonical_content_message_id(
    item: &ModelDialogueItem,
    part: &ContentPart,
) -> Result<String, AgentError> {
    let metadata_id = |key: &str| {
        item.metadata
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let item_id = || {
        item.id
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| metadata_id(CANONICAL_MESSAGE_ID_METADATA_KEY))
    };
    let message_id = match (item.role, part) {
        (Role::Assistant, ContentPart::Thinking { .. }) => {
            metadata_id(REASONING_MESSAGE_ID_METADATA_KEY)
        }
        (Role::Assistant, ContentPart::ToolUse { call }) if !call.id.is_empty() => {
            Some(tool_use_message_id(&call.id))
        }
        (Role::Assistant, ContentPart::ToolResult { result }) if !result.call_id.is_empty() => {
            Some(tool_result_message_id(&result.call_id))
        }
        (Role::Assistant, _) => metadata_id(TEXT_MESSAGE_ID_METADATA_KEY),
        (Role::Tool, ContentPart::ToolResult { result }) if !result.call_id.is_empty() => {
            Some(tool_result_message_id(&result.call_id))
        }
        _ => item_id(),
    };
    message_id.ok_or_else(|| {
        AgentError::Internal(format!(
            "{:?} output is missing canonical message_id",
            item.role
        ))
    })
}

#[async_trait]
impl AgentHook for TurnPersistenceHook {
    async fn on_iteration_limit(&self, max_iterations: u32) -> Result<(), AgentError> {
        let store = WorkspaceSessionStore::new(&self.conversation_store_dir);
        let note = rua_session::RuntimeNote {
            runtime_note_id: format!(
                "runtime-note:conversation:{}:{}:max_iterations",
                self.chat_id, self.turn_id
            ),
            created_at: chrono::Utc::now().to_rfc3339(),
            payload: rua_session::RuntimeNotePayload::ConversationRuntimeTerminal(
                rua_session::ConversationRuntimeTerminalRuntimeNote {
                    agent_id: self.agent_id.clone(),
                    chat_id: self.chat_id.clone(),
                    turn_id: self.turn_id.clone(),
                    terminal_status: "interrupted".into(),
                    reason: "max_iterations".into(),
                    summary: format!(
                        "本轮执行达到配置的 {max_iterations} 个工具调用轮次上限，已停止。\n\
                         Execution stopped at the configured limit of {max_iterations} tool rounds. \
                         This does not mean the user's goal is complete. On continuation, resume \
                         unfinished work from the latest progress."
                    ),
                },
            ),
        };
        store
            .append_runtime_note(&self.chat_id, &self.turn_id, note)
            .await
            .map_err(|error| AgentError::Internal(error.to_string()))?;
        // The loop invokes this after the previous Tool group is durable.
        store
            .flush_pending_runtime_notes(&self.chat_id, &self.turn_id)
            .await
            .map_err(|error| AgentError::Internal(error.to_string()))?;
        self.iteration_limit_reached.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn after_model_call(
        &self,
        messages: &[ModelDialogueItem],
        _iteration: u32,
        total_usage: &TokenUsage,
        last_call_usage: Option<&TokenUsage>,
        terminal: bool,
    ) -> Result<(), AgentError> {
        let mut trace = llm_multiplex::debug_capture::CaptureTrace::current("persistence");
        trace.set("message_count", messages.len());
        trace.set(
            "message_role",
            messages
                .first()
                .map(|message| format!("{:?}", message.role)),
        );
        trace.mark("message_conversion_started");
        let mut conversation_messages = self.conversation_messages(messages)?;
        {
            let filesystem = rua_tool::fs::WorkspaceFs::new_unchecked(self.working_dir.clone());
            let mut archive = self
                .display_media
                .lock()
                .map_err(|_| AgentError::Internal("display media lock poisoned".into()))?;
            for message in &mut conversation_messages {
                if message.role != "assistant" {
                    continue;
                }
                let mut imported = Vec::new();
                for part in message.content.as_array_mut().into_iter().flatten() {
                    if part.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                        let Some(text) = part.get("text").and_then(serde_json::Value::as_str)
                        else {
                            continue;
                        };
                        let (rewritten, media) = archive.rewrite(
                            text, &self.conversation_store_dir, &self.working_dir, &self.device_id,
                            |path| {
                                if self.device_id != self.authority_device_id {
                                    return Err(std::io::Error::other("display image execution Device differs from Conversation Authority"));
                                }
                                filesystem.resolve(path).map_err(|error| std::io::Error::other(error.to_string()))
                            },
                        ).map_err(|error| AgentError::Internal(format!("display image could not be archived: {error}")))?;
                        part["text"] = rewritten.into();
                        imported.extend(media);
                    }
                }
                if !imported.is_empty() {
                    message.metadata.insert(
                        "display_media".into(),
                        serde_json::to_value(imported)
                            .map_err(|error| AgentError::Internal(error.to_string()))?,
                    );
                }
            }
        }
        trace.mark("message_conversion_finished");
        if conversation_messages.is_empty() {
            trace.finish("empty");
            return Ok(());
        }
        self.attach_terminal_usage(&mut conversation_messages, total_usage, last_call_usage);
        let assistant_started_at = conversation_messages
            .iter()
            .find(|message| message.role == "assistant")
            .map(|message| message.created_at.clone());
        let store = WorkspaceSessionStore::new(&self.conversation_store_dir);
        let writer = TurnWriter::new(&store, &self.chat_id, &self.turn_id);
        let start_variant = {
            let mut started = self
                .regenerate_variant_started
                .lock()
                .expect("Turn persistence variant lock poisoned");
            if *started {
                false
            } else {
                *started = true;
                true
            }
        };
        // The parent iteration checkpoints a pending save after 100 ms. Keep the
        // child detail until append finishes instead of fsyncing before every save.
        trace.mark("turn_writer_started");
        let appended = if start_variant {
            writer
                .append_new_variant_in_progress(
                    conversation_messages,
                    assistant_started_at.as_deref(),
                )
                .await
        } else {
            writer
                .append_in_progress_output(conversation_messages, assistant_started_at.as_deref())
                .await
        };
        trace.mark("turn_writer_finished");
        if appended.is_err() {
            trace.finish("error");
        }
        let appended = appended.map_err(|error| AgentError::Internal(error.to_string()))?;
        trace.mark("runtime_notes_flush_started");
        let flushed = store
            .flush_pending_runtime_notes(&self.chat_id, &self.turn_id)
            .await;
        trace.mark("runtime_notes_flush_finished");
        if flushed.is_err() {
            trace.finish("error");
        }
        flushed.map_err(|error| AgentError::Internal(error.to_string()))?;
        if terminal && self.iteration_limit_reached.load(Ordering::Relaxed) {
            if let Some(turn) = appended {
                if turn.lifecycle.status == "in_progress" {
                    let mut lifecycle = turn.lifecycle;
                    lifecycle.status = "interrupted".into();
                    lifecycle.assistant_completed_at = Some(chrono::Utc::now().to_rfc3339());
                    store
                        .set_turn_lifecycle(&self.chat_id, &self.turn_id, lifecycle)
                        .await
                        .map_err(|error| AgentError::Internal(error.to_string()))?;
                }
            }
        }
        trace.finish("completed");
        Ok(())
    }
}

fn usage_value(usage: &TokenUsage) -> serde_json::Value {
    serde_json::json!({
        "prompt_tokens": usage.prompt_tokens,
        "completion_tokens": usage.completion_tokens,
        "total_tokens": usage.total(),
        "cache_read_tokens": usage.cached_tokens,
        "cache_write_tokens": usage.cache_write_tokens,
    })
}

pub fn persist_runtime_stream(
    request: &AgentRequest,
    mut stream: RuntimeCommandStream,
) -> RuntimeCommandStream {
    let Some(persistence) = TurnPersistenceHook::from_request(request) else {
        return stream;
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<std::result::Result<RuntimeEvent, RuaError>>(64);
    tokio::spawn(async move {
        let mut accumulator = RuntimeMessageAccumulator::default();
        let mut append_sequence = 0;
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => {
                    if let Err(error) = validate_runtime_message_identity(&event) {
                        let _ = tx.send(Err(error)).await;
                        return;
                    }
                    let terminal =
                        matches!(event, RuntimeEvent::End { .. } | RuntimeEvent::Error { .. });
                    let completed = accumulator.observe(&event);
                    if !completed.is_empty() {
                        if let Err(error) = persistence
                            .after_model_call(
                                &completed,
                                append_sequence,
                                &accumulator.usage,
                                accumulator.last_usage.as_ref(),
                                terminal,
                            )
                            .await
                        {
                            let _ = tx
                                .send(Err(RuaError::Runtime {
                                    message: format!("failed to persist runtime output: {error}"),
                                }))
                                .await;
                            return;
                        }
                        append_sequence += 1;
                    }
                    if tx.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let completed = accumulator.finish();
                    if !completed.is_empty() {
                        if let Err(persist_error) = persistence
                            .after_model_call(
                                &completed,
                                append_sequence,
                                &accumulator.usage,
                                accumulator.last_usage.as_ref(),
                                true,
                            )
                            .await
                        {
                            let _ = tx
                                .send(Err(RuaError::Runtime {
                                    message: format!(
                                        "failed to persist partial runtime output: {persist_error}"
                                    ),
                                }))
                                .await;
                            return;
                        }
                    }
                    let _ = tx.send(Err(error)).await;
                    return;
                }
            }
        }
        let completed = accumulator.finish();
        if !completed.is_empty() {
            if let Err(error) = persistence
                .after_model_call(
                    &completed,
                    append_sequence,
                    &accumulator.usage,
                    accumulator.last_usage.as_ref(),
                    true,
                )
                .await
            {
                let _ = tx
                    .send(Err(RuaError::Runtime {
                        message: format!("failed to persist final runtime output: {error}"),
                    }))
                    .await;
            }
        }
    });
    Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    }))
}

#[derive(Default)]
struct RuntimeMessageAccumulator {
    assistant_parts: Vec<ContentPart>,
    usage: TokenUsage,
    last_usage: Option<TokenUsage>,
    text_message_id: Option<String>,
    reasoning_message_id: Option<String>,
}

impl RuntimeMessageAccumulator {
    fn observe(&mut self, event: &RuntimeEvent) -> Vec<ModelDialogueItem> {
        match event {
            RuntimeEvent::TextDelta {
                delta, message_id, ..
            } => {
                self.text_message_id = message_id.as_ref().map(ToString::to_string);
                append_delta(&mut self.assistant_parts, false, delta, None);
                Vec::new()
            }
            RuntimeEvent::ThinkingDelta {
                delta,
                signature,
                message_id,
                ..
            } => {
                self.reasoning_message_id = message_id.as_ref().map(ToString::to_string);
                append_delta(&mut self.assistant_parts, true, delta, signature.as_ref());
                Vec::new()
            }
            RuntimeEvent::ModelCallReset { .. } => {
                self.assistant_parts.clear();
                self.text_message_id = None;
                self.reasoning_message_id = None;
                Vec::new()
            }
            RuntimeEvent::ToolCallDelta {
                id,
                name,
                arguments_delta,
                ..
            } => {
                upsert_tool_call_delta(
                    &mut self.assistant_parts,
                    id,
                    name.as_deref(),
                    arguments_delta,
                );
                Vec::new()
            }
            RuntimeEvent::ToolCall { call } => {
                upsert_tool_call(&mut self.assistant_parts, call.clone());
                Vec::new()
            }
            RuntimeEvent::ToolResult { result } => {
                let mut completed = self.finish();
                completed.push(ModelDialogueItem {
                    id: None,
                    role: Role::Tool,
                    content: vec![ContentPart::ToolResult {
                        result: result.clone(),
                    }],
                    name: result.name.clone(),
                    created_at: None,
                    metadata: serde_json::Map::new(),
                });
                completed
            }
            RuntimeEvent::Usage {
                input_tokens,
                output_tokens,
                total_tokens,
                cache_read_tokens,
                cache_write_tokens,
            } => {
                let usage = TokenUsage {
                    prompt_tokens: input_tokens.unwrap_or_default(),
                    completion_tokens: output_tokens.unwrap_or_default(),
                    reasoning_tokens: None,
                    cached_tokens: *cache_read_tokens,
                    cache_write_tokens: *cache_write_tokens,
                    total_tokens: *total_tokens,
                };
                self.usage = usage.clone();
                self.last_usage = Some(usage);
                Vec::new()
            }
            RuntimeEvent::End { .. } | RuntimeEvent::Error { .. } => self.finish(),
            RuntimeEvent::Start { .. }
            | RuntimeEvent::ToolResultDelta { .. }
            | RuntimeEvent::ContextCompaction { .. }
            | RuntimeEvent::ToolApprovalPending { .. }
            | RuntimeEvent::IterationStart { .. }
            | RuntimeEvent::IterationEnd { .. }
            | RuntimeEvent::AskUser { .. }
            | RuntimeEvent::ToolApprovalRequest { .. } => Vec::new(),
        }
    }

    fn finish(&mut self) -> Vec<ModelDialogueItem> {
        if self.assistant_parts.is_empty() {
            return Vec::new();
        }
        let mut metadata = serde_json::Map::new();
        if let Some(message_id) = self.reasoning_message_id.take() {
            metadata.insert(
                REASONING_MESSAGE_ID_METADATA_KEY.to_string(),
                serde_json::Value::String(message_id),
            );
        }
        if let Some(message_id) = self.text_message_id.take() {
            metadata.insert(
                TEXT_MESSAGE_ID_METADATA_KEY.to_string(),
                serde_json::Value::String(message_id),
            );
        }
        vec![ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: std::mem::take(&mut self.assistant_parts),
            name: None,
            created_at: None,
            metadata,
        }]
    }
}

fn validate_runtime_message_identity(event: &RuntimeEvent) -> Result<(), RuaError> {
    let missing = match event {
        RuntimeEvent::TextDelta { message_id, .. } if message_id.is_none() => Some("text delta"),
        RuntimeEvent::ThinkingDelta { message_id, .. } if message_id.is_none() => {
            Some("thinking delta")
        }
        RuntimeEvent::ToolCallDelta {
            id,
            arguments_delta,
            ..
        } if id.is_empty() && !arguments_delta.is_empty() => Some("tool call delta"),
        RuntimeEvent::ToolCall { call } if call.id.is_empty() => Some("tool call"),
        RuntimeEvent::ToolResultDelta { id, .. } if id.is_empty() => Some("tool result delta"),
        RuntimeEvent::ToolResult { result } if result.call_id.is_empty() => Some("tool result"),
        _ => None,
    };
    if let Some(kind) = missing {
        return Err(RuaError::Runtime {
            message: format!("{kind} is missing canonical message_id"),
        });
    }
    Ok(())
}

fn append_delta(
    parts: &mut Vec<ContentPart>,
    thinking: bool,
    delta: &str,
    signature: Option<&String>,
) {
    if let Some(last) = parts.last_mut() {
        match last {
            ContentPart::Text { text } if !thinking => {
                text.push_str(delta);
                return;
            }
            ContentPart::Thinking {
                text,
                signature: current_signature,
            } if thinking => {
                text.push_str(delta);
                if signature.is_some() {
                    *current_signature = signature.cloned();
                }
                return;
            }
            _ => {}
        }
    }
    if thinking {
        parts.push(ContentPart::Thinking {
            text: delta.to_string(),
            signature: signature.cloned(),
        });
    } else {
        parts.push(ContentPart::Text {
            text: delta.to_string(),
        });
    }
}

fn tool_arguments_from_raw(raw: &str) -> serde_json::Value {
    if raw.trim().is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

fn upsert_tool_call_delta(
    parts: &mut Vec<ContentPart>,
    id: &str,
    name: Option<&str>,
    arguments_delta: &str,
) {
    if id.trim().is_empty() {
        return;
    }
    for part in parts.iter_mut() {
        let ContentPart::ToolUse { call } = part else {
            continue;
        };
        if call.id != id {
            continue;
        }
        if let Some(name) = name.filter(|value| !value.is_empty()) {
            call.name = name.to_string();
        }
        let mut raw_arguments = call.raw_arguments.clone().unwrap_or_default();
        raw_arguments.push_str(arguments_delta);
        call.arguments = tool_arguments_from_raw(&raw_arguments);
        call.raw_arguments = Some(raw_arguments);
        return;
    }
    let mut call = ToolCall::new(
        id.to_string(),
        name.unwrap_or_default().to_string(),
        tool_arguments_from_raw(arguments_delta),
    );
    call.raw_arguments = Some(arguments_delta.to_string());
    parts.push(ContentPart::ToolUse { call });
}

fn upsert_tool_call(parts: &mut Vec<ContentPart>, incoming: ToolCall) {
    for part in parts.iter_mut() {
        let ContentPart::ToolUse { call } = part else {
            continue;
        };
        if call.id == incoming.id {
            *call = incoming;
            return;
        }
    }
    parts.push(ContentPart::ToolUse { call: incoming });
}

fn is_persistable(message: &ModelDialogueItem) -> bool {
    match message.role {
        Role::System | Role::Developer => message
            .content
            .iter()
            .any(|part| matches!(part, ContentPart::ToolResult { .. })),
        Role::User => {
            !message.content.is_empty()
                && !message.content.iter().all(
                    |part| matches!(part, ContentPart::Text { text } if text.trim().is_empty()),
                )
        }
        _ => true,
    }
}
