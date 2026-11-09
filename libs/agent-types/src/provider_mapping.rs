use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::event::AgentEvent;
use crate::finish::FinishReason;
use crate::tool::ToolCall;
use crate::usage::TokenUsage;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenAiDelta {
    #[serde(default)]
    pub content: Option<String>,

    #[serde(default)]
    pub reasoning_content: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OpenAiToolCallDelta>,

    #[serde(default)]
    pub finish_reason: Option<String>,

    #[serde(default)]
    pub usage: Option<OpenAiUsage>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiToolCallDelta {
    pub index: usize,

    #[serde(default)]
    pub id: Option<String>,

    #[serde(default)]
    pub function: Option<OpenAiFunction>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiFunction {
    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens_details: Option<OpenAiCompletionTokensDetails>,
    #[serde(default)]
    pub prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiCompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiPromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<u64>,
}

impl OpenAiDelta {
    pub fn to_agent_events(&self) -> Vec<AgentEvent> {
        let mut events = Vec::new();

        if let Some(ref rc) = self.reasoning_content {
            if !rc.is_empty() {
                events.push(AgentEvent::ThinkingDelta {
                    delta: rc.clone(),
                    signature: None,
                    metadata: serde_json::Value::Null,
                });
            }
        }

        if let Some(ref content) = self.content {
            if !content.is_empty() {
                events.push(AgentEvent::TextDelta {
                    delta: content.clone(),
                    metadata: serde_json::Value::Null,
                });
            }
        }

        for tc in &self.tool_calls {
            let id = tc.id.clone().unwrap_or_else(|| format!("tc-{}", tc.index));
            let name = tc.function.as_ref().and_then(|f| f.name.clone());
            let args_delta = tc
                .function
                .as_ref()
                .and_then(|f| f.arguments.clone())
                .unwrap_or_default();
            events.push(AgentEvent::ToolCallDelta {
                id,
                index: tc.index,
                name,
                arguments_delta: args_delta,
                metadata: serde_json::Value::Null,
            });
        }

        if let Some(ref u) = self.usage {
            events.push(AgentEvent::Usage {
                usage: u.to_token_usage(),
            });
        }

        events
    }
}

impl OpenAiUsage {
    pub fn to_token_usage(&self) -> TokenUsage {
        TokenUsage {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            reasoning_tokens: self
                .completion_tokens_details
                .as_ref()
                .and_then(|d| d.reasoning_tokens),
            cached_tokens: self
                .prompt_tokens_details
                .as_ref()
                .and_then(|d| d.cached_tokens),
            cache_write_tokens: None,
            total_tokens: self.total_tokens,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AnthropicContentBlock {
    Text {
        text: String,
    },

    Thinking {
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
    },

    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

impl AnthropicContentBlock {
    pub fn to_agent_event(&self) -> AgentEvent {
        match self {
            AnthropicContentBlock::Text { text } => AgentEvent::TextDelta {
                delta: text.clone(),
                metadata: serde_json::Value::Null,
            },
            AnthropicContentBlock::Thinking {
                thinking,
                signature,
            } => AgentEvent::ThinkingDelta {
                delta: thinking.clone(),
                signature: signature.clone(),
                metadata: serde_json::Value::Null,
            },
            AnthropicContentBlock::ToolUse { id, name, input } => AgentEvent::ToolCall {
                call: ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: input.clone(),
                    raw_arguments: None,
                    provider_call_id: Some(id.clone()),
                    metadata: serde_json::Map::new(),
                },
            },
        }
    }
}

pub fn anthropic_stop_reason(reason: &str) -> FinishReason {
    FinishReason::from_provider(reason)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum GeminiPart {
    Text(String),

    FunctionCall {
        name: String,
        args: serde_json::Value,
    },

    FunctionResponse {
        name: String,
        response: serde_json::Value,
    },
}

impl GeminiPart {
    pub fn to_agent_event(&self) -> AgentEvent {
        match self {
            GeminiPart::Text(text) => AgentEvent::TextDelta {
                delta: text.clone(),
                metadata: serde_json::Value::Null,
            },
            GeminiPart::FunctionCall { name, args } => AgentEvent::ToolCall {
                call: ToolCall {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: name.clone(),
                    arguments: args.clone(),
                    raw_arguments: None,
                    provider_call_id: None,
                    metadata: serde_json::Map::new(),
                },
            },
            GeminiPart::FunctionResponse { name, response } => AgentEvent::ToolResult {
                result: crate::tool::ToolResult {
                    call_id: String::new(),
                    name: Some(name.clone()),
                    content: vec![crate::content::ContentPart::Data {
                        value: response.clone(),
                    }],
                    is_error: false,
                    duration_ms: None,
                    attachments: None,
                    truncation: None,
                    metadata: serde_json::Map::new(),
                },
            },
        }
    }
}

pub fn gemini_finish_reason(reason: &str) -> FinishReason {
    FinishReason::from_provider(reason)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiUsageMetadata {
    #[serde(default)]
    pub prompt_token_count: u64,
    #[serde(default)]
    pub candidates_token_count: u64,
    #[serde(default)]
    pub total_token_count: u64,
    #[serde(default)]
    pub cached_content_token_count: Option<u64>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl GeminiUsageMetadata {
    pub fn to_token_usage(&self) -> TokenUsage {
        TokenUsage {
            prompt_tokens: self.prompt_token_count,
            completion_tokens: self.candidates_token_count,
            reasoning_tokens: None,
            cached_tokens: self.cached_content_token_count,
            cache_write_tokens: None,
            total_tokens: Some(self.total_token_count),
        }
    }
}
