use serde::{Deserialize, Serialize};

use crate::finish::FinishReason;
use crate::tool::{ToolCall, ToolResult};
use crate::usage::TokenUsage;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentEvent {
    Start {
        run_id: String,

        #[serde(default)]
        model: Option<String>,

        #[serde(default)]
        metadata: serde_json::Value,
    },

    TextDelta {
        delta: String,

        #[serde(default, skip_serializing_if = "is_null_value")]
        metadata: serde_json::Value,
    },

    ThinkingDelta {
        delta: String,

        #[serde(default)]
        signature: Option<String>,

        #[serde(default, skip_serializing_if = "is_null_value")]
        metadata: serde_json::Value,
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

        #[serde(default, skip_serializing_if = "is_null_value")]
        metadata: serde_json::Value,
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

        #[serde(default, skip_serializing_if = "is_null_value")]
        metadata: serde_json::Value,
    },

    ToolApprovalPending {
        call_id: String,

        tool_name: String,

        #[serde(default)]
        request_id: Option<String>,

        #[serde(default, skip_serializing_if = "is_null_value")]
        metadata: serde_json::Value,
    },

    ContextCompaction {
        status: String,
        #[serde(default)]
        mode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },

    Usage {
        usage: TokenUsage,
    },

    Error {
        #[serde(default)]
        code: Option<String>,

        message: String,

        #[serde(default)]
        retryable: bool,

        #[serde(default, skip_serializing_if = "is_null_value")]
        metadata: serde_json::Value,
    },

    End {
        finish_reason: FinishReason,

        #[serde(default)]
        usage: Option<TokenUsage>,

        #[serde(default, skip_serializing_if = "is_null_value")]
        metadata: serde_json::Value,
    },

    IterationStart {
        iteration: u32,
    },

    IterationEnd {
        iteration: u32,

        has_tool_calls: bool,
    },
}

pub type ChatStreamEvent = AgentEvent;

impl AgentEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(self, AgentEvent::End { .. } | AgentEvent::Error { .. })
    }

    pub fn is_start(&self) -> bool {
        matches!(self, AgentEvent::Start { .. })
    }

    pub fn is_content(&self) -> bool {
        matches!(
            self,
            AgentEvent::TextDelta { .. }
                | AgentEvent::ThinkingDelta { .. }
                | AgentEvent::ToolCallDelta { .. }
                | AgentEvent::ToolCall { .. }
                | AgentEvent::ToolResultDelta { .. }
                | AgentEvent::ToolResult { .. }
        )
    }
}

fn is_null_value(v: &serde_json::Value) -> bool {
    v.is_null()
}

#[derive(Debug, Clone)]
pub struct ToolCallDeltaAccumulator {
    pub id: String,

    pub name: Option<String>,

    pub arguments_buffer: String,
}

impl ToolCallDeltaAccumulator {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            arguments_buffer: String::new(),
        }
    }

    pub fn push(&mut self, name: Option<&str>, arguments_delta: &str) {
        if let Some(n) = name {
            if self.name.is_none() {
                self.name = Some(n.to_string());
            }
        }
        self.arguments_buffer.push_str(arguments_delta);
    }

    pub fn finalize(self) -> ToolCall {
        let arguments = serde_json::from_str(&self.arguments_buffer)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
        let raw = if arguments.is_null() || !self.arguments_buffer.is_empty() {
            Some(self.arguments_buffer)
        } else {
            None
        };
        ToolCall {
            id: self.id,
            name: self.name.unwrap_or_default(),
            arguments,
            raw_arguments: raw,
            provider_call_id: None,
            metadata: serde_json::Map::new(),
        }
    }
}
