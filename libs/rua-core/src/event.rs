use serde::{Deserialize, Serialize};

use crate::error::RuaError;
use crate::id::{AgentId, MessageId, SessionId};
use crate::tool::{ToolCall, ToolResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskQuestion {
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub url: String,
    pub name: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HistoryMessage {
    User {
        #[serde(default)]
        id: Option<MessageId>,
        content: String,
        #[serde(default)]
        attachments: Vec<AttachmentMeta>,
        #[serde(default)]
        metadata: serde_json::Map<String, serde_json::Value>,
    },
    Assistant {
        #[serde(default)]
        id: Option<MessageId>,
        content: String,
        #[serde(default)]
        metadata: serde_json::Map<String, serde_json::Value>,
    },
    Reasoning {
        #[serde(default)]
        id: Option<MessageId>,
        content: String,
        #[serde(default)]
        signature: Option<String>,
    },
    PluginCall {
        #[serde(default)]
        id: Option<MessageId>,
        call: ToolCall,
    },
    PluginCallOutput {
        #[serde(default)]
        id: Option<MessageId>,
        result: ToolResult,
    },
    Error {
        #[serde(default)]
        id: Option<MessageId>,
        detail: String,
    },
    Cancel {
        #[serde(default)]
        id: Option<MessageId>,
        #[serde(default)]
        reason: Option<String>,
    },
}

impl HistoryMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            id: None,
            content: content.into(),
            attachments: vec![],
            metadata: serde_json::Map::new(),
        }
    }

    pub fn user_with_attachments(
        content: impl Into<String>,
        attachments: Vec<AttachmentMeta>,
    ) -> Self {
        Self::User {
            id: None,
            content: content.into(),
            attachments,
            metadata: serde_json::Map::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            id: None,
            content: content.into(),
            metadata: serde_json::Map::new(),
        }
    }

    pub fn reasoning(content: impl Into<String>) -> Self {
        Self::Reasoning {
            id: None,
            content: content.into(),
            signature: None,
        }
    }

    pub fn plugin_call(call: ToolCall) -> Self {
        Self::PluginCall { id: None, call }
    }

    pub fn plugin_call_output(result: ToolResult) -> Self {
        Self::PluginCallOutput { id: None, result }
    }

    pub fn error(detail: impl Into<String>) -> Self {
        Self::Error {
            id: None,
            detail: detail.into(),
        }
    }

    pub fn cancel(reason: Option<String>) -> Self {
        Self::Cancel { id: None, reason }
    }

    pub fn message_type(&self) -> &'static str {
        match self {
            Self::User { .. } => "MESSAGE",
            Self::Assistant { .. } => "MESSAGE",
            Self::Reasoning { .. } => "REASONING",
            Self::PluginCall { .. } => "PLUGIN_CALL",
            Self::PluginCallOutput { .. } => "PLUGIN_CALL_OUTPUT",
            Self::Error { .. } => "ERROR",
            Self::Cancel { .. } => "CANCEL",
        }
    }
}

#[derive(Debug, Default)]
pub struct HistoryManager {
    messages: Vec<HistoryMessage>,
    pending_text: String,
    pending_thinking: String,
    pending_thinking_signature: Option<String>,
}

impl HistoryManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process_event(&mut self, event: &RuntimeEvent) {
        match event {
            RuntimeEvent::TextDelta { delta, .. } => {
                self.pending_text.push_str(delta);
            }
            RuntimeEvent::ThinkingDelta { delta, .. } => {
                self.pending_thinking.push_str(delta);
            }
            RuntimeEvent::ModelCallReset { .. } => {
                self.pending_text.clear();
                self.pending_thinking.clear();
                self.pending_thinking_signature = None;
            }
            RuntimeEvent::ToolCall { call } => {
                self.flush_pending();
                self.messages
                    .push(HistoryMessage::plugin_call(call.clone()));
            }
            RuntimeEvent::ToolResult { result } => {
                self.messages
                    .push(HistoryMessage::plugin_call_output(result.clone()));
            }
            RuntimeEvent::Error { error } => {
                self.flush_pending();
                self.messages.push(HistoryMessage::error(error.to_string()));
            }
            RuntimeEvent::End { .. } => {
                self.flush_pending();
            }
            RuntimeEvent::Start { .. }
            | RuntimeEvent::ToolCallDelta { .. }
            | RuntimeEvent::ToolResultDelta { .. }
            | RuntimeEvent::Usage { .. }
            | RuntimeEvent::ContextCompaction { .. }
            | RuntimeEvent::ToolApprovalPending { .. }
            | RuntimeEvent::IterationStart { .. }
            | RuntimeEvent::IterationEnd { .. }
            | RuntimeEvent::AskUser { .. }
            | RuntimeEvent::ToolApprovalRequest { .. } => {}
        }
    }

    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.messages.push(HistoryMessage::user(content));
    }

    pub fn add_cancel(&mut self, reason: Option<String>) {
        self.flush_pending();
        self.messages.push(HistoryMessage::cancel(reason));
    }

    fn flush_pending(&mut self) {
        if !self.pending_thinking.is_empty() {
            let mut msg = HistoryMessage::reasoning(std::mem::take(&mut self.pending_thinking));
            if let HistoryMessage::Reasoning { signature, .. } = &mut msg {
                *signature = self.pending_thinking_signature.take();
            }
            self.messages.push(msg);
        }
        if !self.pending_text.is_empty() {
            self.messages.push(HistoryMessage::assistant(std::mem::take(
                &mut self.pending_text,
            )));
        }
    }

    pub fn into_messages(mut self) -> Vec<HistoryMessage> {
        self.flush_pending();
        self.messages
    }

    pub fn messages(&mut self) -> &[HistoryMessage] {
        self.flush_pending();
        &self.messages
    }

    pub fn len(&self) -> usize {
        let mut count = self.messages.len();
        if !self.pending_text.is_empty() {
            count += 1;
        }
        if !self.pending_thinking.is_empty() {
            count += 1;
        }
        count
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.pending_text.is_empty() && self.pending_thinking.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    Start {
        session_id: SessionId,
        #[serde(default)]
        agent_id: Option<AgentId>,
    },

    TextDelta {
        delta: String,
        #[serde(default)]
        message_id: Option<MessageId>,
    },

    ThinkingDelta {
        delta: String,
        #[serde(default)]
        signature: Option<String>,
        #[serde(default)]
        message_id: Option<MessageId>,
    },

    ModelCallReset {
        model_call_id: String,
    },

    ToolCallDelta {
        id: String,

        #[serde(default)]
        index: usize,

        #[serde(default)]
        name: Option<String>,

        arguments_delta: String,
    },

    ToolCall {
        call: ToolCall,
    },

    ToolResult {
        result: ToolResult,
    },

    ToolResultDelta {
        id: String,
        #[serde(default)]
        name: Option<String>,
        output_delta: String,
    },

    ToolApprovalPending {
        call_id: String,
        tool_name: String,
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
    },

    ContextCompaction {
        status: String,
        #[serde(default)]
        mode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },

    Usage {
        #[serde(default)]
        input_tokens: Option<u64>,
        #[serde(default)]
        output_tokens: Option<u64>,
        #[serde(default)]
        total_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_read_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_write_tokens: Option<u64>,
    },

    Error {
        error: RuaError,
    },

    End {
        #[serde(default)]
        finish_reason: Option<String>,
    },

    IterationStart {
        iteration: u32,
    },

    IterationEnd {
        iteration: u32,

        has_tool_calls: bool,
    },

    AskUser {
        request_id: String,
        questions: Vec<AskQuestion>,
    },

    ToolApprovalRequest {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
}
