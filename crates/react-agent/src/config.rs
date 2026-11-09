use std::time::Duration;

use serde::{Deserialize, Serialize};

use agent_types::ReasoningConfig;

use crate::tool_disclosure::ToolDisclosureConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactConfig {
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,

    #[serde(default = "default_total_timeout_secs")]
    pub total_timeout_secs: u64,

    /// Inactivity per provider attempt; excludes retry backoff and rate-limit queueing.
    /// Zero disables this timeout. The optional total run deadline remains independent.
    #[serde(default = "default_model_timeout_secs")]
    pub model_timeout_secs: u64,

    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u64,

    #[serde(default = "default_parallel_tool_calls")]
    pub parallel_tool_calls: bool,

    #[serde(default = "default_max_parallel_tools")]
    pub max_parallel_tools: usize,

    #[serde(default)]
    pub auto_continue_on_text_only: bool,

    #[serde(default = "default_compact_on_overflow")]
    pub compact_on_overflow: bool,

    #[serde(default = "default_max_compaction_retries")]
    pub max_compaction_retries: u32,

    #[serde(default = "default_emit_thinking")]
    pub emit_thinking: bool,

    #[serde(default = "default_emit_tool_deltas")]
    pub emit_tool_deltas: bool,

    #[serde(default = "default_auto_continue_max_retries")]
    pub auto_continue_max_retries: u32,

    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub provider_options: serde_json::Map<String, serde_json::Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,

    #[serde(default)]
    pub tool_disclosure: ToolDisclosureConfig,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_max_iterations() -> u32 {
    0
}
fn default_total_timeout_secs() -> u64 {
    0
}
fn default_model_timeout_secs() -> u64 {
    120
}
fn default_tool_timeout_secs() -> u64 {
    0
}
fn default_parallel_tool_calls() -> bool {
    true
}
fn default_max_parallel_tools() -> usize {
    5
}
fn default_compact_on_overflow() -> bool {
    true
}
fn default_max_compaction_retries() -> u32 {
    1
}
fn default_emit_thinking() -> bool {
    true
}
fn default_emit_tool_deltas() -> bool {
    true
}
fn default_auto_continue_max_retries() -> u32 {
    2
}
impl Default for ReactConfig {
    fn default() -> Self {
        Self {
            max_iterations: default_max_iterations(),
            total_timeout_secs: default_total_timeout_secs(),
            model_timeout_secs: default_model_timeout_secs(),
            tool_timeout_secs: default_tool_timeout_secs(),
            parallel_tool_calls: default_parallel_tool_calls(),
            max_parallel_tools: default_max_parallel_tools(),
            auto_continue_on_text_only: false,
            compact_on_overflow: default_compact_on_overflow(),
            max_compaction_retries: default_max_compaction_retries(),
            emit_thinking: default_emit_thinking(),
            emit_tool_deltas: default_emit_tool_deltas(),
            auto_continue_max_retries: default_auto_continue_max_retries(),
            provider_options: serde_json::Map::new(),
            reasoning: None,
            tool_disclosure: ToolDisclosureConfig::default(),
            extra: serde_json::Map::new(),
        }
    }
}

impl ReactConfig {
    pub fn total_timeout(&self) -> Duration {
        Duration::from_secs(self.total_timeout_secs)
    }

    pub fn model_timeout(&self) -> Duration {
        Duration::from_secs(self.model_timeout_secs)
    }

    pub fn tool_timeout(&self) -> Duration {
        Duration::from_secs(self.tool_timeout_secs)
    }
}
