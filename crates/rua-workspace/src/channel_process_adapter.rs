use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use tokio::sync::Mutex;

use rua_agent::memory_signal::MemorySignal;
use rua_agent::memory_signal_note::{memory_signal_runtime_note, render_memory_signal_for_model};
use rua_channel::core::channel::Process;
use rua_channel::core::content::{AgentChannelRequest, IncomingContentPart};
use rua_contract::agent::{AgentInputMessage, AgentRuntime};
use rua_core::event::RuntimeEvent;
use rua_core::id::{AgentId, SessionId};
use rua_core::message::ContentPart;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
};

pub struct ChannelProcessAdapter {
    runtime: Arc<dyn AgentRuntime>,
    workspace_dir: Option<PathBuf>,
    memory_write_available: bool,
}

impl ChannelProcessAdapter {
    pub fn new(runtime: Arc<dyn AgentRuntime>) -> Self {
        Self {
            runtime,
            workspace_dir: None,
            memory_write_available: false,
        }
    }

    pub fn with_workspace_dir(mut self, workspace_dir: PathBuf) -> Self {
        self.workspace_dir = Some(workspace_dir);
        self
    }

    pub fn with_memory_write_available(mut self, available: bool) -> Self {
        self.memory_write_available = available;
        self
    }

    fn build_request(
        workspace_dir: Option<&Path>,
        req: &AgentChannelRequest,
        mut runtime_note_messages: Vec<AgentInputMessage>,
        memory_signal: Option<&MemorySignal>,
    ) -> rua_contract::agent::AgentRequest {
        let content_parts: Vec<ContentPart> = req
            .input
            .iter()
            .map(|part| match part {
                IncomingContentPart::Text { text } => ContentPart::Text { text: text.clone() },
                IncomingContentPart::Image { path, mime, .. } => ContentPart::Image {
                    url: format!("file://{}", path.display()),
                    media_type: mime.clone(),
                    alt: None,
                },
                IncomingContentPart::File {
                    path, mime, name, ..
                } => ContentPart::File {
                    url: format!("file://{}", path.display()),
                    name: name.clone().unwrap_or_default(),
                    media_type: mime.clone(),
                    size: None,
                },
                IncomingContentPart::Audio { path, mime, .. } => ContentPart::Audio {
                    url: format!("file://{}", path.display()),
                    media_type: mime.clone(),
                },
                IncomingContentPart::Video { path, mime, .. } => ContentPart::Video {
                    url: format!("file://{}", path.display()),
                    media_type: mime.clone(),
                },
                IncomingContentPart::Url { url, .. } => ContentPart::Text { text: url.clone() },
            })
            .collect();

        runtime_note_messages.push(AgentInputMessage {
            role: "user".to_string(),
            content: content_parts,
        });
        if let Some(signal) = memory_signal {
            runtime_note_messages.push(AgentInputMessage {
                role: "system".to_string(),
                content: vec![ContentPart::Text {
                    text: render_memory_signal_for_model(signal),
                }],
            });
        }

        rua_contract::agent::AgentRequest {
            session_id: SessionId::new(&req.session_id),
            user_id: req.user_id.clone(),
            agent_id: AgentId::new(&req.agent_id),
            messages: runtime_note_messages,
            model_override: None,
            tool_mode: "auto".to_string(),
            attachments: vec![],
            display_preferences: None,
            disabled_tools: Vec::new(),
            metadata: HashMap::new(),
            cancel: None,
            run_control: None,
            workspace_dir: workspace_dir.map(Path::to_path_buf),
            conversation_store_dir: workspace_dir.map(Path::to_path_buf),
            working_dir: None,
        }
    }
}

struct SessionPersistCtx {
    workspace_dir: PathBuf,
    agent_id: String,
    session_id: String,
    user_text: String,
    memory_signal: Option<MemorySignal>,
    text_acc: Mutex<String>,
    thinking_acc: Mutex<String>,
    tool_events: Mutex<Vec<serde_json::Value>>,
}

async fn ensure_im_session_header(
    workspace_dir: &Path,
    agent_id: &str,
    session_id: &str,
) -> Result<(), String> {
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    if store
        .load_chat_header(session_id)
        .await
        .map_err(|e| format!("load_chat_header: {e}"))?
        .is_some()
    {
        return Ok(());
    }

    let now = chrono::Utc::now().to_rfc3339();
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            title: "Channel chat".to_string(),
            created_at: now.clone(),
            last_activity_at: now,
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .map_err(|e| format!("save_chat_header: {e}"))
}

async fn save_im_session(
    workspace_dir: &Path,
    agent_id: &str,
    session_id: &str,
    user_text: &str,
    memory_signal: Option<&MemorySignal>,
    assistant_content: &[serde_json::Value],
) -> Result<(), String> {
    if user_text.is_empty() && memory_signal.is_none() && assistant_content.is_empty() {
        return Ok(());
    }

    ensure_im_session_header(workspace_dir, agent_id, session_id).await?;

    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let mut messages = Vec::new();
    if !user_text.is_empty() {
        let mut metadata = serde_json::Map::new();
        metadata.insert("name".to_string(), serde_json::json!("user"));
        messages.push(NewConversationMessage {
            message_id: format!("msg-{}", uuid::Uuid::new_v4()),
            role: "user".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            status: "completed".to_string(),
            content: serde_json::json!([{"type": "text", "text": user_text}]),
            provider: None,
            tool: None,
            metadata,
        });
    }

    if let Some(signal) = memory_signal {
        let runtime_note_id = format!("memory-signal-{}", uuid::Uuid::new_v4());
        messages.push(
            memory_signal_runtime_note(runtime_note_id, chrono::Utc::now().to_rfc3339(), signal)
                .into_new_conversation_message()
                .map_err(|error| error.to_string())?,
        );
    }

    if !assistant_content.is_empty() {
        let mut metadata = serde_json::Map::new();
        metadata.insert("name".to_string(), serde_json::json!(agent_id));
        messages.push(NewConversationMessage {
            message_id: format!("msg-{}", uuid::Uuid::new_v4()),
            role: "assistant".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            status: "completed".to_string(),
            content: serde_json::Value::Array(assistant_content.to_vec()),
            provider: None,
            tool: None,
            metadata,
        });
    }

    store
        .append_conversation_turn(
            session_id,
            NewConversationTurn {
                turn_id: format!("turn-{}", uuid::Uuid::new_v4()),
                lifecycle: TurnLifecycle {
                    user_submitted_at: chrono::Utc::now().to_rfc3339(),
                    assistant_started_at: None,
                    assistant_completed_at: (!assistant_content.is_empty())
                        .then(|| chrono::Utc::now().to_rfc3339()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages,
            },
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("append_conversation_turn: {e}"))
}

#[async_trait]
impl Process for ChannelProcessAdapter {
    async fn handle(
        &self,
        req: rua_channel::core::channel::AgentRequest,
    ) -> BoxStream<'static, RuntimeEvent> {
        if let Some(ref ws_dir) = self.workspace_dir {
            let agent_id = if req.agent_id.is_empty() {
                "default"
            } else {
                req.agent_id.as_str()
            };
            let sender_name = req
                .metadata
                .get("sender_nick")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if let Err(e) = crate::chats_store::ensure_chat_entry(
                ws_dir,
                &req.session_id,
                &req.channel_id,
                &req.user_id,
                sender_name.as_deref(),
            )
            .await
            {
                tracing::warn!(error = %e, "ensure_chat_entry failed, continuing");
            }
            if let Err(e) = ensure_im_session_header(ws_dir, agent_id, &req.session_id).await {
                tracing::warn!(error = %e, "ensure channel session header failed, continuing");
            }
        }

        let user_text: String = req
            .input
            .iter()
            .filter_map(|p| {
                if let IncomingContentPart::Text { text } = p {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let mut memory_signal = None;
        if let Some(ref ws_dir) = self.workspace_dir {
            let store = rua_session::WorkspaceSessionStore::new(ws_dir);
            match rua_agent::memory_signal_note::detect_memory_signal_for_user_input(
                &store,
                &req.session_id,
                &user_text,
                self.memory_write_available,
            )
            .await
            {
                Ok(signal) => memory_signal = signal,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session_id = %req.session_id,
                        "failed to detect channel memory signal"
                    );
                }
            }
        }

        let persist_ctx = self.workspace_dir.as_ref().map(|ws_dir| {
            let agent_id = if req.agent_id.is_empty() {
                "default".to_string()
            } else {
                req.agent_id.clone()
            };
            Arc::new(SessionPersistCtx {
                workspace_dir: ws_dir.clone(),
                agent_id,
                session_id: req.session_id.clone(),
                user_text,
                memory_signal: memory_signal.clone(),
                text_acc: Mutex::new(String::new()),
                thinking_acc: Mutex::new(String::new()),
                tool_events: Mutex::new(Vec::new()),
            })
        });

        let contract_req = Self::build_request(
            self.workspace_dir.as_deref(),
            &req,
            Vec::new(),
            memory_signal.as_ref(),
        );
        match self.runtime.stream(contract_req).await {
            Ok(s) => {
                let base = s.map(|r| r.unwrap_or_else(|e| RuntimeEvent::Error { error: e }));
                match persist_ctx {
                    Some(ctx) => base
                        .then(move |event| {
                            let ctx = ctx.clone();
                            async move {
                                match &event {
                                    RuntimeEvent::TextDelta { delta, .. } => {
                                        ctx.text_acc.lock().await.push_str(delta);
                                    }
                                    RuntimeEvent::ThinkingDelta { delta, .. } => {
                                        ctx.thinking_acc.lock().await.push_str(delta);
                                    }
                                    RuntimeEvent::ToolCall { call } => {
                                        ctx.tool_events.lock().await.push(serde_json::json!({
                                            "type": "tool_use",
                                            "id": call.id,
                                            "name": call.name,
                                            "input": call.arguments
                                        }));
                                    }
                                    RuntimeEvent::ToolResult { result } => {
                                        ctx.tool_events.lock().await.push(serde_json::json!({
                                            "type": "tool_result",
                                            "id": result.call_id,
                                            "name": result.name.clone().unwrap_or_default(),
                                            "output": [{"type": "text", "text": result.text_content()}]
                                        }));
                                    }
                                    RuntimeEvent::End { .. } => {
                                        let full_text = ctx.text_acc.lock().await.clone();
                                        let thinking = ctx.thinking_acc.lock().await.clone();
                                        let tool_events = ctx.tool_events.lock().await.clone();

                                        let mut assistant_content: Vec<serde_json::Value> = Vec::new();
                                        if !thinking.is_empty() {
                                            assistant_content.push(serde_json::json!({
                                                "type": "thinking",
                                                "thinking": thinking
                                            }));
                                        }
                                        assistant_content.extend(tool_events);
                                        if !full_text.is_empty() {
                                            assistant_content.push(serde_json::json!({
                                                "type": "text",
                                                "text": full_text
                                            }));
                                        }

                                        if let Err(e) = save_im_session(
                                            &ctx.workspace_dir,
                                            &ctx.agent_id,
                                            &ctx.session_id,
                                            &ctx.user_text,
                                            ctx.memory_signal.as_ref(),
                                            &assistant_content,
                                        )
                                        .await
                                        {
                                            tracing::warn!(error = %e, "IM session persist failed");
                                        }
                                    }
                                    _ => {}
                                }
                                event
                            }
                        })
                        .boxed(),
                    None => base.boxed(),
                }
            }
            Err(e) => stream::once(async move { RuntimeEvent::Error { error: e } }).boxed(),
        }
    }
}
