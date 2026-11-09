use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model_dialogue_item::ModelDialogueItem;
use crate::tool::ToolDefinition;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Function(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningMode {
    Effort,
    Budget,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningSummary {
    Auto,
    Concise,
    Detailed,
}

impl ReasoningSummary {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Concise => "concise",
            Self::Detailed => "detailed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReasoningConfig {
    pub mode: ReasoningMode,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<ReasoningSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredOutput {
    pub schema: serde_json::Value,

    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub strict: Option<bool>,

    #[serde(default)]
    pub fallback_to_tool_call: bool,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl StructuredOutput {
    pub fn new(schema: serde_json::Value) -> Self {
        Self {
            schema,
            name: None,
            strict: None,
            fallback_to_tool_call: false,
            extra: HashMap::new(),
        }
    }

    pub fn tool_name(&self) -> &str {
        self.name.as_deref().unwrap_or("generate_structured_output")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ModelDialogueItem>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,

    #[serde(default)]
    pub tool_choice: Option<ToolChoice>,

    #[serde(default)]
    pub model: Option<String>,

    #[serde(default)]
    pub temperature: Option<f32>,

    #[serde(default)]
    pub max_tokens: Option<u32>,

    #[serde(default)]
    pub stream: bool,

    #[serde(default)]
    pub structured_output: Option<StructuredOutput>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,

    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, serde_json::Value>,

    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub provider_options: serde_json::Map<String, serde_json::Value>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl ChatRequest {
    pub fn new(messages: Vec<ModelDialogueItem>) -> Self {
        Self {
            messages,
            tools: Vec::new(),
            tool_choice: None,
            model: None,
            temperature: None,
            max_tokens: None,
            stream: false,
            structured_output: None,
            reasoning: None,
            metadata: serde_json::Map::new(),
            provider_options: serde_json::Map::new(),
            extra: HashMap::new(),
        }
    }

    pub fn streaming(messages: Vec<ModelDialogueItem>) -> Self {
        Self {
            stream: true,
            ..Self::new(messages)
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_reasoning(mut self, reasoning: ReasoningConfig) -> Self {
        self.reasoning = Some(reasoning);
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = Some(tool_choice);
        self
    }

    pub fn with_structured_output(mut self, structured_output: StructuredOutput) -> Self {
        self.structured_output = Some(structured_output);
        self
    }

    pub fn with_provider_option(
        mut self,
        key: impl Into<String>,
        value: serde_json::Value,
    ) -> Self {
        self.provider_options.insert(key.into(), value);
        self
    }
}
