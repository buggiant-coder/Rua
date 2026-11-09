use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BaseChannelConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_true")]
    pub show_thinking: bool,
    #[serde(default = "default_true")]
    pub show_tool_calls: bool,
    #[serde(default = "default_true")]
    pub show_tool_results: bool,

    #[serde(default = "default_true")]
    pub show_tool_details: bool,

    #[serde(default)]
    pub thread_follow_without_mention: bool,

    #[serde(default)]
    pub require_mention: bool,

    #[serde(default)]
    pub dm_policy: String,

    #[serde(default)]
    pub group_policy: String,

    #[serde(default)]
    pub allow_from: Vec<String>,

    #[serde(default)]
    pub deny_from: Vec<String>,

    #[serde(default)]
    pub access_control_dm: bool,

    #[serde(default)]
    pub access_control_group: bool,

    #[serde(default)]
    pub language: String,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for BaseChannelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            show_thinking: true,
            show_tool_calls: true,
            show_tool_results: true,
            show_tool_details: true,
            thread_follow_without_mention: false,
            require_mention: false,
            dm_policy: String::new(),
            group_policy: String::new(),
            allow_from: Vec::new(),
            deny_from: Vec::new(),
            access_control_dm: false,
            access_control_group: false,
            language: String::new(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ConsoleChannelConfig {
    #[serde(flatten)]
    pub base: BaseChannelConfig,
}

impl ConsoleChannelConfig {
    pub fn is_enabled(&self) -> bool {
        self.base.enabled
    }
}
