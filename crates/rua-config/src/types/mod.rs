pub mod acp;
pub mod agents;
pub mod channels;
pub mod cron;
pub mod security;
pub mod session;
pub mod settings;
pub mod skill_manifest;
pub mod templates;
pub mod token_usage;
pub mod tool_call;
pub mod tools;
pub mod workspace;

pub use acp::{AcpAgentConfig, AcpConfig, ToolParseMode};
pub use agents::{
    ActiveHoursConfig, AgentProfileConfig, AgentProfileRef, AgentsConfig, AgentsRunningConfig,
    ChatRunningConfig, ContextCompactConfig, EmbeddingConfig, HeartbeatConfig, MemorySearchMode,
    MemorySummaryConfig, ModelSlotConfig, ToolDisclosureEnabled, ToolDisclosureRunningConfig,
    ToolResultCompactConfig,
};
pub use channels::{BaseChannelConfig, ChannelConfig};
pub use cron::{CronJobSpec, CronJobState, JobsFile, ScheduleSpec};
pub use security::{
    FileGuardConfig, SecurityConfig, SkillScannerConfig, ToolGuardConfig, ToolGuardRuleConfig,
};
pub use tools::{BuiltinToolConfig, ToolsConfig};

pub use session::{SessionPendingActionSnapshot, SessionPromptFileSnapshot};
pub use settings::SettingsFile;
pub use skill_manifest::{SkillEntry, SkillManifestFile, SkillMetadata, SkillRequirements};
pub use templates::{AgentTemplate, AgentTemplatesFile, SubagentTemplatesFile};
pub use token_usage::{TokenUsageEntry, TokenUsageFile};
pub use tool_call::ToolCallRecord;
pub use workspace::{
    ChannelCommonConfig, DingTalkConfig, DiscordConfig, FeishuConfig, IMessageConfig, MatrixConfig,
    MattermostConfig, MqttConfig, OneBotConfig, QqConfig, SipConfig, SlackConfig, TelegramConfig,
    TunnelConfig, WechatConfig, WecomConfig, WorkspaceChannelsConfig, WorkspaceConfig,
    XiaoyiConfig, YuanbaoConfig,
};

use crate::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};

fn default_mcp_value() -> serde_json::Value {
    serde_json::json!({ "clients": {} })
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ObservabilityConfig {
    #[serde(default)]
    pub metrics: MetricsConfig,
    #[serde(default)]
    pub log_level: Option<String>,
    #[serde(default)]
    pub log_format: Option<String>,
    #[serde(default)]
    pub rotate_size_mb: Option<u64>,
    #[serde(default)]
    pub rotate_keep: Option<u32>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "MetricsConfig::default_bind_addr")]
    pub bind_addr: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl MetricsConfig {
    fn default_bind_addr() -> String {
        "127.0.0.1:9090".to_string()
    }
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_addr: Self::default_bind_addr(),
            extra: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RootConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub channels: ChannelConfig,

    #[serde(
        default = "default_mcp_value",
        deserialize_with = "deserialize_null_default"
    )]
    pub mcp: serde_json::Value,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub tools: ToolsConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub agents: AgentsConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub observability: ObservabilityConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub acp: AcpConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub task_runtime: TaskRuntimeConfig,
    #[serde(default)]
    pub last_dispatch: Option<LastDispatchConfig>,
    #[serde(default)]
    pub show_tool_details: Option<bool>,
    #[serde(default)]
    pub user_timezone: Option<String>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRuntimeConfig {
    #[serde(default = "TaskRuntimeConfig::default_max_active_tasks_per_chat")]
    pub max_active_tasks_per_chat: usize,
    #[serde(default = "TaskRuntimeConfig::default_max_concurrent_tasks_per_chat")]
    pub max_concurrent_tasks_per_chat: usize,
    #[serde(default = "TaskRuntimeConfig::default_max_concurrent_tasks_per_runner")]
    pub max_concurrent_tasks_per_runner: usize,
    #[serde(default = "TaskRuntimeConfig::default_activity_channel_capacity")]
    pub activity_channel_capacity: usize,
    #[serde(default = "TaskRuntimeConfig::default_max_result_output_chars")]
    pub max_result_output_chars: usize,
    #[serde(default = "TaskRuntimeConfig::default_max_raw_result_bytes")]
    pub max_raw_result_bytes: usize,
    #[serde(default = "TaskRuntimeConfig::default_max_attempt_history")]
    pub max_attempt_history: usize,
    #[serde(default = "TaskRuntimeConfig::default_max_dependencies_per_task")]
    pub max_dependencies_per_task: usize,
    #[serde(default = "TaskRuntimeConfig::default_max_dependency_depth")]
    pub max_dependency_depth: usize,
    #[serde(default = "TaskRuntimeConfig::default_cancellation_grace_secs")]
    pub cancellation_grace_secs: u64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl TaskRuntimeConfig {
    fn default_max_active_tasks_per_chat() -> usize {
        128
    }

    fn default_max_concurrent_tasks_per_chat() -> usize {
        4
    }

    fn default_max_concurrent_tasks_per_runner() -> usize {
        8
    }

    fn default_activity_channel_capacity() -> usize {
        512
    }

    fn default_max_raw_result_bytes() -> usize {
        1024 * 1024
    }

    fn default_max_result_output_chars() -> usize {
        16_000
    }

    fn default_cancellation_grace_secs() -> u64 {
        5
    }

    fn default_max_attempt_history() -> usize {
        20
    }

    fn default_max_dependencies_per_task() -> usize {
        16
    }

    fn default_max_dependency_depth() -> usize {
        16
    }
}

impl Default for TaskRuntimeConfig {
    fn default() -> Self {
        Self {
            max_active_tasks_per_chat: Self::default_max_active_tasks_per_chat(),
            max_concurrent_tasks_per_chat: Self::default_max_concurrent_tasks_per_chat(),
            max_concurrent_tasks_per_runner: Self::default_max_concurrent_tasks_per_runner(),
            activity_channel_capacity: Self::default_activity_channel_capacity(),
            max_result_output_chars: Self::default_max_result_output_chars(),
            max_raw_result_bytes: Self::default_max_raw_result_bytes(),
            max_attempt_history: Self::default_max_attempt_history(),
            max_dependencies_per_task: Self::default_max_dependencies_per_task(),
            max_dependency_depth: Self::default_max_dependency_depth(),
            cancellation_grace_secs: Self::default_cancellation_grace_secs(),
            extra: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LastDispatchConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub channel: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub user_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub session_id: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
