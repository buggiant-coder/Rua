use serde::{Deserialize, Serialize};

use crate::serde_helpers::{deserialize_null_default, deserialize_null_default_trimmed_string};

fn default_enable_ws() -> bool {
    true
}

fn default_show_tool_details() -> bool {
    true
}

fn default_mqtt_port() -> u16 {
    1883
}

fn deserialize_matrix_homeserver_url<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let trimmed = deserialize_null_default_trimmed_string(deserializer)?;
    Ok(trimmed.trim_end_matches('/').to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelCommonConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub dm_policy: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub group_policy: String,
    #[serde(default)]
    pub require_mention: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub allow_from: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub deny_from: Vec<String>,
    #[serde(default)]
    pub access_control_dm: bool,
    #[serde(default)]
    pub access_control_group: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub language: String,
    #[serde(default = "display_default_true")]
    pub show_thinking: bool,
    #[serde(default = "display_default_true")]
    pub show_tool_calls: bool,
    #[serde(default = "display_default_true")]
    pub show_tool_results: bool,
    #[serde(default = "default_show_tool_details")]
    pub show_tool_details: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_prefix: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn display_default_true() -> bool {
    true
}

impl Default for ChannelCommonConfig {
    fn default() -> Self {
        Self {
            dm_policy: Default::default(),
            group_policy: Default::default(),
            require_mention: Default::default(),
            allow_from: Default::default(),
            deny_from: Default::default(),
            access_control_dm: Default::default(),
            access_control_group: Default::default(),
            language: Default::default(),
            show_thinking: true,
            show_tool_calls: true,
            show_tool_results: true,
            show_tool_details: false,
            bot_prefix: Default::default(),
            extra: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DingTalkConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub client_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub client_secret: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_name: String,
    #[serde(default = "default_enable_ws")]
    pub enable_ws: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub robot_code: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub message_type: String,
    #[serde(default)]
    pub streaming_enabled: bool,
    #[serde(default)]
    pub ai_card_enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub card_template_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub card_template_key: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WecomConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub secret: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FeishuConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub webhook_url: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub secret: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub app_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub app_secret: Option<String>,
    #[serde(default)]
    pub share_session_in_group: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_open_id: Option<String>,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_token: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub chat_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub allowed_users: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_username: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub webhook_url: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_token: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_user_id: Option<String>,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OneBotConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub http_url: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub ws_host: String,
    #[serde(default)]
    pub ws_port: u16,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub access_token: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MattermostConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub url: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_token: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_token: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub app_token: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub signing_secret: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_user_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SipConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bind_addr: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub public_addr: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub rtp_bind_addr: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub username: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub domain: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub max_call_secs: u64,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QqConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub app_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub client_secret: String,
    #[serde(default)]
    pub sandbox: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatrixConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(
        default,
        rename = "homeserver",
        deserialize_with = "deserialize_matrix_homeserver_url"
    )]
    pub homeserver_url: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub user_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub password: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub access_token: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub default_room_id: String,
    #[serde(default, rename = "encryption")]
    pub encryption_enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub store_path: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MqttConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(
        default,
        rename = "host",
        deserialize_with = "deserialize_null_default_trimmed_string"
    )]
    pub broker_url: String,
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    #[serde(default, deserialize_with = "deserialize_null_default_trimmed_string")]
    pub subscribe_topic: String,
    #[serde(default, deserialize_with = "deserialize_null_default_trimmed_string")]
    pub publish_topic: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WechatConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub bot_token: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub base_url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct XiaoyiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub access_key: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub secret_key: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub agent_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub ws_url: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub backup_ws_url: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct YuanbaoConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub app_id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub app_secret: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub api_domain: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IMessageConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub db_path: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub poll_sec: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub allowed_senders: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub media_dir: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub max_decoded_size: Option<usize>,
    #[serde(flatten)]
    pub common: ChannelCommonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceChannelsConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub dingtalk: DingTalkConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub wecom: WecomConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub feishu: FeishuConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub telegram: TelegramConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub discord: DiscordConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub onebot: OneBotConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub mattermost: MattermostConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub slack: SlackConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub sip: SipConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub qq: QqConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub matrix: MatrixConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub mqtt: MqttConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub wechat: WechatConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub xiaoyi: XiaoyiConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub yuanbao: YuanbaoConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub imessage: IMessageConfig,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TunnelConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub local_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceConfig {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub channels: WorkspaceChannelsConfig,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub tunnel: TunnelConfig,
}
