use crate::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseChannelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_prefix: String,
    #[serde(default = "display_default_true")]
    pub show_thinking: bool,
    #[serde(default = "display_default_true")]
    pub show_tool_calls: bool,
    #[serde(default = "display_default_true")]
    pub show_tool_results: bool,
    #[serde(
        default = "default_policy",
        deserialize_with = "deserialize_null_default"
    )]
    pub dm_policy: String,
    #[serde(
        default = "default_policy",
        deserialize_with = "deserialize_null_default"
    )]
    pub group_policy: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub allow_from: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub deny_message: String,
    #[serde(default)]
    pub require_mention: bool,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn display_default_true() -> bool {
    true
}

impl Default for BaseChannelConfig {
    fn default() -> Self {
        Self {
            enabled: Default::default(),
            bot_prefix: Default::default(),
            show_thinking: true,
            show_tool_calls: true,
            show_tool_results: true,
            dm_policy: Default::default(),
            group_policy: Default::default(),
            allow_from: Default::default(),
            deny_message: Default::default(),
            require_mention: Default::default(),
            extra: Default::default(),
        }
    }
}

fn default_policy() -> String {
    "open".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub imessage: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub discord: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub dingtalk: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub feishu: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub qq: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub telegram: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub mattermost: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub slack: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub sip: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub mqtt: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub console: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub matrix: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub voice: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub wecom: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub xiaoyi: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub weixin: BaseChannelConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub onebot: BaseChannelConfig,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
