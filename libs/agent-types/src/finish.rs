use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FinishReason {
    Stop,

    Length,

    ToolUse,

    ContentFilter,

    Error,

    Cancelled,

    Unknown(String),
}

impl FinishReason {
    pub fn is_stop(&self) -> bool {
        matches!(self, FinishReason::Stop)
    }

    pub fn is_tool_use(&self) -> bool {
        matches!(self, FinishReason::ToolUse)
    }

    pub fn is_terminal_error(&self) -> bool {
        matches!(self, FinishReason::Error | FinishReason::Cancelled)
    }

    pub fn from_provider(raw: &str) -> Self {
        match raw.to_lowercase().as_str() {
            "stop" | "end_turn" | "stop_sequence" => FinishReason::Stop,
            "length" | "max_tokens" => FinishReason::Length,
            "tool_calls" | "tool_use" => FinishReason::ToolUse,
            "content_filter" | "safety" | "recitation" => FinishReason::ContentFilter,
            "error" => FinishReason::Error,
            "cancelled" | "canceled" => FinishReason::Cancelled,
            _ => FinishReason::Unknown(raw.to_string()),
        }
    }
}

impl std::fmt::Display for FinishReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FinishReason::Stop => write!(f, "stop"),
            FinishReason::Length => write!(f, "length"),
            FinishReason::ToolUse => write!(f, "tool_use"),
            FinishReason::ContentFilter => write!(f, "content_filter"),
            FinishReason::Error => write!(f, "error"),
            FinishReason::Cancelled => write!(f, "cancelled"),
            FinishReason::Unknown(s) => write!(f, "{s}"),
        }
    }
}
