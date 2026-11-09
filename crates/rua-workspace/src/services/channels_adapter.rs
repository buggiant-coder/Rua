use std::sync::Arc;

use async_trait::async_trait;
use rua_channel::core::ChannelManager;
use rua_config::{
    ChannelCommonConfig, DingTalkConfig, DiscordConfig, FeishuConfig, MatrixConfig,
    MattermostConfig, MqttConfig, OneBotConfig, QqConfig, SipConfig, SlackConfig, TelegramConfig,
    WechatConfig, WecomConfig, XiaoyiConfig, YuanbaoConfig,
};

use crate::error::WorkspaceError;
use crate::service_manager::ServiceLifecycle;

#[derive(Debug)]
pub enum RegistrationError {
    IncompleteConfig(String),
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IncompleteConfig(msg) => write!(f, "incomplete config: {msg}"),
        }
    }
}

impl std::error::Error for RegistrationError {}

fn render_options(common: &ChannelCommonConfig) -> rua_channel::core::MessageRenderOptions {
    let mut options = rua_channel::core::MessageRenderOptions {
        show_thinking: common.show_thinking,
        show_tool_calls: common.show_tool_calls,
        show_tool_results: common.show_tool_results,
        show_tool_details: common.show_tool_details,
        ..Default::default()
    };
    options.apply_metadata(&serde_json::json!(common.extra));
    options
}

fn build_acl_metadata(common: &ChannelCommonConfig) -> serde_json::Value {
    let mut metadata = serde_json::json!({
        "dm_policy": common.dm_policy,
        "group_policy": common.group_policy,
        "require_mention": common.require_mention,
        "allow_from": common.allow_from,
        "deny_from": common.deny_from,
        "access_control_dm": common.access_control_dm,
        "access_control_group": common.access_control_group,
        "language": common.language,
        "bot_prefix": common.bot_prefix,
    });
    for key in [
        "tool_call_max_length",
        "tool_result_max_length",
        "dm_disabled",
        "group_disabled",
        "deny_message",
        "no_text_debounce",
        "input_prefix",
        "share_session_in_group",
        "streaming_enabled",
        "show_typing",
        "accept_bot_messages",
        "thread_follow_without_mention",
        "media_base64",
        "media_base64_max_mb",
        "media_download_max_mb",
        "markdown_enabled",
        "max_reconnect_attempts",
        "ack_message",
        "welcome_text",
        "message_merge_enabled",
        "message_merge_delay_ms",
        "cron_message_type",
        "card_auto_layout",
        "at_sender_on_reply",
    ] {
        if let Some(value) = common.extra.get(key) {
            metadata[key] = value.clone();
        }
    }
    metadata
}

fn string_option(common: &ChannelCommonConfig, key: &str) -> Option<String> {
    common
        .extra
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn channel_http_client(common: &ChannelCommonConfig) -> Result<reqwest::Client, RegistrationError> {
    let mut builder = reqwest::Client::builder();
    if let Some(url) =
        string_option(common, "http_proxy").or_else(|| string_option(common, "proxy"))
    {
        let mut proxy = reqwest::Proxy::all(&url).map_err(|error| {
            RegistrationError::IncompleteConfig(format!("invalid proxy: {error}"))
        })?;
        if let Some(auth) = string_option(common, "http_proxy_auth") {
            if let Some((username, password)) = auth.split_once(':') {
                proxy = proxy.basic_auth(username, password);
            }
        }
        builder = builder.proxy(proxy);
    }
    rua_http::build_client(builder)
        .map_err(|error| RegistrationError::IncompleteConfig(format!("HTTP client: {error}")))
}

pub struct ChannelsServiceAdapter {
    manager: Arc<ChannelManager>,
    dingtalk_config: Option<DingTalkConfig>,
    wecom_config: Option<WecomConfig>,
    feishu_config: Option<FeishuConfig>,
    telegram_config: Option<TelegramConfig>,
    discord_config: Option<DiscordConfig>,
    onebot_config: Option<OneBotConfig>,
    mattermost_config: Option<MattermostConfig>,
    slack_config: Option<SlackConfig>,
    sip_config: Option<SipConfig>,
    qq_config: Option<QqConfig>,
    matrix_config: Option<MatrixConfig>,
    mqtt_config: Option<MqttConfig>,
    wechat_config: Option<WechatConfig>,
    xiaoyi_config: Option<XiaoyiConfig>,
    yuanbao_config: Option<YuanbaoConfig>,
    imessage_config: Option<rua_config::IMessageConfig>,
}

impl ChannelsServiceAdapter {
    pub fn from_configs(
        manager: Arc<ChannelManager>,
        configs: crate::service_manager::ImPlatformConfigs,
    ) -> Self {
        let mut adapter = Self::new(manager);
        adapter.dingtalk_config = configs.dingtalk;
        adapter.wecom_config = configs.wecom;
        adapter.feishu_config = configs.feishu;
        adapter.telegram_config = configs.telegram;
        adapter.discord_config = configs.discord;
        adapter.onebot_config = configs.onebot;
        adapter.mattermost_config = configs.mattermost;
        adapter.slack_config = configs.slack;
        adapter.sip_config = configs.sip;
        adapter.qq_config = configs.qq;
        adapter.matrix_config = configs.matrix;
        adapter.mqtt_config = configs.mqtt;
        adapter.wechat_config = configs.wechat;
        adapter.xiaoyi_config = configs.xiaoyi;
        adapter.yuanbao_config = configs.yuanbao;
        adapter.imessage_config = configs.imessage;
        adapter
    }

    pub fn new(manager: Arc<ChannelManager>) -> Self {
        Self {
            manager,
            dingtalk_config: None,
            wecom_config: None,
            feishu_config: None,
            telegram_config: None,
            discord_config: None,
            onebot_config: None,
            mattermost_config: None,
            slack_config: None,
            sip_config: None,
            qq_config: None,
            matrix_config: None,
            mqtt_config: None,
            wechat_config: None,
            xiaoyi_config: None,
            yuanbao_config: None,
            imessage_config: None,
        }
    }

    pub fn with_dingtalk_config(mut self, config: DingTalkConfig) -> Self {
        self.dingtalk_config = Some(config);
        self
    }

    pub fn with_wecom_config(mut self, config: WecomConfig) -> Self {
        self.wecom_config = Some(config);
        self
    }

    pub fn with_feishu_config(mut self, config: FeishuConfig) -> Self {
        self.feishu_config = Some(config);
        self
    }

    pub fn with_telegram_config(mut self, config: TelegramConfig) -> Self {
        self.telegram_config = Some(config);
        self
    }

    pub fn with_discord_config(mut self, config: DiscordConfig) -> Self {
        self.discord_config = Some(config);
        self
    }

    pub fn with_onebot_config(mut self, config: OneBotConfig) -> Self {
        self.onebot_config = Some(config);
        self
    }

    pub fn with_mattermost_config(mut self, config: MattermostConfig) -> Self {
        self.mattermost_config = Some(config);
        self
    }

    pub fn with_slack_config(mut self, config: SlackConfig) -> Self {
        self.slack_config = Some(config);
        self
    }

    pub fn with_sip_config(mut self, config: SipConfig) -> Self {
        self.sip_config = Some(config);
        self
    }

    pub fn with_qq_config(mut self, config: QqConfig) -> Self {
        self.qq_config = Some(config);
        self
    }

    pub fn with_matrix_config(mut self, config: MatrixConfig) -> Self {
        self.matrix_config = Some(config);
        self
    }

    pub fn with_mqtt_config(mut self, config: MqttConfig) -> Self {
        self.mqtt_config = Some(config);
        self
    }

    pub fn with_wechat_config(mut self, config: WechatConfig) -> Self {
        self.wechat_config = Some(config);
        self
    }

    pub fn with_xiaoyi_config(mut self, config: XiaoyiConfig) -> Self {
        self.xiaoyi_config = Some(config);
        self
    }

    pub fn with_yuanbao_config(mut self, config: YuanbaoConfig) -> Self {
        self.yuanbao_config = Some(config);
        self
    }

    pub fn with_imessage_config(mut self, config: rua_config::IMessageConfig) -> Self {
        self.imessage_config = Some(config);
        self
    }

    #[cfg(all(feature = "imessage", target_os = "macos"))]
    async fn register_imessage_channel(&self) -> Result<(), RegistrationError> {
        let Some(config) = self
            .imessage_config
            .as_ref()
            .filter(|config| config.enabled)
        else {
            return Ok(());
        };
        let defaults = rua_channel::imessage::IMessageConfig::default();
        let channel =
            rua_channel::imessage::IMessageChannel::new(rua_channel::imessage::IMessageConfig {
                enabled: true,
                db_path: if config.db_path.is_empty() {
                    defaults.db_path
                } else {
                    config.db_path.clone()
                },
                poll_sec: config.poll_sec.unwrap_or(defaults.poll_sec),
                bot_prefix: config.common.bot_prefix.clone(),
                allowed_senders: config.allowed_senders.clone(),
                media_dir: config.media_dir.clone(),
                max_decoded_size: config.max_decoded_size.unwrap_or(defaults.max_decoded_size),
            })
            .with_acl_config(build_acl_metadata(&config.common))
            .with_render_options(render_options(&config.common));
        self.manager
            .register(Arc::new(channel))
            .await
            .map_err(|error| RegistrationError::IncompleteConfig(error.to_string()))
    }

    pub fn manager(&self) -> Arc<ChannelManager> {
        Arc::clone(&self.manager)
    }

    async fn register_feishu_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.feishu_config {
            Some(c) if !c.enabled => {
                tracing::debug!("Feishu channel disabled, skipping");
                return Ok(());
            }
            Some(c) => {
                let has_webhook = !c.webhook_url.is_empty();
                let has_app = c.app_id.as_ref().is_some_and(|id| !id.is_empty())
                    && c.app_secret.as_ref().is_some_and(|s| !s.is_empty());
                if !has_webhook && !has_app {
                    tracing::warn!(
                        "Feishu enabled but config incomplete: neither webhook_url nor app credentials"
                    );
                    return Err(RegistrationError::IncompleteConfig(
                        "neither webhook_url nor app credentials".into(),
                    ));
                }
                c
            }
            None => {
                tracing::debug!("Feishu channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering Feishu channel");

        let feishu_config = rua_channel::feishu::FeishuConfig {
            webhook_url: config.webhook_url.clone(),
            secret: config.secret.clone(),
            app_id: config.app_id.clone(),
            app_secret: config.app_secret.clone(),
            share_session_in_group: config.share_session_in_group,
            media_dir: config.media_dir.clone(),
            bot_open_id: config.bot_open_id.clone(),
        };
        let mut feishu = rua_channel::feishu::FeishuChannel::new(feishu_config)
            .with_render_options(render_options(&config.common))
            .with_acl_config(build_acl_metadata(&config.common));

        if let Some(domain) = string_option(&config.common, "domain") {
            feishu = feishu
                .with_domain(&domain)
                .map_err(|error| RegistrationError::IncompleteConfig(error.to_string()))?;
        }
        if let Err(e) = self.manager.register(Arc::new(feishu)).await {
            tracing::error!(error = %e, "failed to register Feishu channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_wecom_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.wecom_config {
            Some(c) if !c.enabled => {
                tracing::debug!("WeCom channel disabled, skipping");
                return Ok(());
            }
            Some(c) if !c.bot_id.is_empty() && !c.secret.is_empty() => c,
            Some(_) => {
                tracing::warn!("WeCom enabled but config incomplete: bot_id or secret empty");
                return Err(RegistrationError::IncompleteConfig(
                    "bot_id or secret empty".into(),
                ));
            }
            None => {
                tracing::debug!("WeCom channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering WeCom aibot channel");

        let mut wecom =
            rua_channel::wecom::WecomChannel::new(config.bot_id.clone(), config.secret.clone())
                .with_render_options(render_options(&config.common))
                .with_media_dir(config.media_dir.clone())
                .with_acl_config(build_acl_metadata(&config.common));

        if let Some(ws_url) = string_option(&config.common, "ws_url") {
            wecom = wecom.with_ws_url(ws_url);
        }
        if let Err(e) = self.manager.register(Arc::new(wecom)).await {
            tracing::error!(error = %e, "failed to register WeCom channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_telegram_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.telegram_config {
            Some(c) if !c.enabled => {
                tracing::debug!("Telegram channel disabled, skipping");
                return Ok(());
            }
            Some(c) if !c.bot_token.is_empty() => c,
            Some(_) => {
                tracing::warn!("Telegram enabled but config incomplete: bot_token empty");
                return Err(RegistrationError::IncompleteConfig(
                    "bot_token empty".into(),
                ));
            }
            None => {
                tracing::debug!("Telegram channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering Telegram channel with bot config");

        let telegram_config = rua_channel::telegram::TelegramConfig {
            bot_token: config.bot_token.clone(),
            chat_id: config.chat_id.clone(),
            allowed_users: config.allowed_users.clone(),
            bot_username: config.bot_username.clone(),
            media_dir: config.media_dir.clone(),
        };
        let mut telegram = rua_channel::telegram::TelegramChannel::new(telegram_config)
            .with_client(channel_http_client(&config.common)?)
            .with_render_options(render_options(&config.common))
            .with_acl_config(build_acl_metadata(&config.common));

        if let Some(base_url) = string_option(&config.common, "base_url") {
            telegram = telegram.with_base_url(base_url.trim_end_matches('/').to_string());
        }
        if let Err(e) = self.manager.register(Arc::new(telegram)).await {
            tracing::error!(error = %e, "failed to register Telegram channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_onebot_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.onebot_config {
            Some(c) if !c.enabled => {
                tracing::debug!("OneBot channel disabled, skipping");
                return Ok(());
            }
            Some(c) => c,
            None => {
                tracing::debug!("OneBot channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering OneBot channel with config");

        let onebot_config = rua_channel::onebot::OneBotConfig {
            http_url: config.http_url.clone(),
            ws_host: if config.ws_host.is_empty() {
                "127.0.0.1".to_string()
            } else {
                config.ws_host.clone()
            },
            ws_port: if config.ws_port == 0 {
                6199
            } else {
                config.ws_port
            },
            access_token: config.access_token.clone(),
            media_dir: config.media_dir.clone(),
        };
        let onebot = rua_channel::onebot::OneBotChannel::new(onebot_config)
            .with_render_options(render_options(&config.common))
            .with_acl_config(build_acl_metadata(&config.common));

        if let Err(e) = self.manager.register(Arc::new(onebot)).await {
            tracing::error!(error = %e, "failed to register OneBot channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_qq_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.qq_config {
            Some(c) if !c.enabled => {
                tracing::debug!("QQ channel disabled, skipping");
                return Ok(());
            }
            Some(c) if !c.app_id.is_empty() && !c.client_secret.is_empty() => c,
            Some(_) => {
                tracing::warn!("QQ enabled but config incomplete: app_id or client_secret empty");
                return Err(RegistrationError::IncompleteConfig(
                    "app_id or client_secret empty".into(),
                ));
            }
            None => {
                tracing::debug!("QQ channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering QQ channel with config");

        let qq = rua_channel::qq::QqChannel::new(
            config.app_id.clone(),
            config.client_secret.clone(),
            config.sandbox,
        )
        .with_media_dir(config.media_dir.clone())
        .with_render_options(render_options(&config.common))
        .with_media_dir(config.media_dir.clone())
        .with_acl_config(build_acl_metadata(&config.common));

        if let Err(e) = self.manager.register(Arc::new(qq)).await {
            tracing::error!(error = %e, "failed to register QQ channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_dingtalk_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.dingtalk_config {
            Some(c) if !c.enabled => {
                tracing::debug!("DingTalk channel disabled, skipping");
                return Ok(());
            }
            Some(c) if !c.client_id.is_empty() && !c.client_secret.is_empty() => c,
            Some(_) => {
                tracing::warn!(
                    "DingTalk enabled but config incomplete: client_id or client_secret empty"
                );
                return Err(RegistrationError::IncompleteConfig(
                    "client_id or client_secret empty".into(),
                ));
            }
            None => {
                tracing::debug!("DingTalk channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!(
            client_id_prefix = &config.client_id[..config.client_id.len().min(4)],
            "registering DingTalk channel with config"
        );

        let robot_code = if config.robot_code.is_empty() {
            config.client_id.clone()
        } else {
            config.robot_code.clone()
        };

        let dingtalk = rua_channel::dingtalk::DingTalkChannel::new()
            .with_ws_config(
                config.client_id.clone(),
                config.client_secret.clone(),
                config.enable_ws,
            )
            .with_bot_config(config.bot_id.clone(), config.bot_name.clone())
            .with_robot_code(robot_code)
            .with_render_options(render_options(&config.common))
            .with_card_config(
                config.ai_card_enabled || config.message_type == "card" || config.streaming_enabled,
                config.card_template_id.clone(),
                config.card_template_key.clone(),
            )
            .with_media_dir(config.media_dir.clone())
            .with_acl_config(
                config.common.dm_policy.clone(),
                config.common.group_policy.clone(),
                config.common.require_mention,
                config.common.allow_from.clone(),
                config.common.deny_from.clone(),
                config.common.access_control_dm,
                config.common.access_control_group,
                config.common.language.clone(),
            )
            .with_channel_metadata({
                let mut metadata = build_acl_metadata(&config.common);
                metadata["streaming_enabled"] = (config.streaming_enabled
                    || config.ai_card_enabled
                    || config.message_type == "card")
                    .into();
                metadata
            });

        let dingtalk =
            dingtalk.with_endpoint(string_option(&config.common, "endpoint").unwrap_or_default());
        if let Err(e) = self.manager.register(Arc::new(dingtalk)).await {
            tracing::error!(error = %e, "failed to register DingTalk channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_mattermost_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.mattermost_config {
            Some(c) if !c.enabled => {
                tracing::debug!("Mattermost channel disabled, skipping");
                return Ok(());
            }
            Some(c) if !c.url.is_empty() && !c.bot_token.is_empty() => c,
            Some(_) => {
                tracing::warn!("Mattermost enabled but config incomplete: url or bot_token empty");
                return Err(RegistrationError::IncompleteConfig(
                    "url or bot_token empty".into(),
                ));
            }
            None => {
                tracing::debug!("Mattermost channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering Mattermost channel with config");

        let mm_config = rua_channel::mattermost::MattermostConfig {
            url: config.url.clone(),
            bot_token: config.bot_token.clone(),
            media_dir: config.media_dir.clone(),
        };
        let mattermost = rua_channel::mattermost::MattermostChannel::new(mm_config)
            .with_render_options(render_options(&config.common))
            .with_acl_config(build_acl_metadata(&config.common));

        if let Err(e) = self.manager.register(Arc::new(mattermost)).await {
            tracing::error!(error = %e, "failed to register Mattermost channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_slack_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.slack_config {
            Some(c) if !c.enabled => {
                tracing::debug!("Slack channel disabled, skipping");
                return Ok(());
            }
            Some(c) if !c.bot_token.is_empty() => c,
            Some(_) => {
                tracing::warn!("Slack enabled but config incomplete: bot_token empty");
                return Err(RegistrationError::IncompleteConfig(
                    "bot_token empty".into(),
                ));
            }
            None => {
                tracing::debug!("Slack channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering Slack channel");

        let slack_config = rua_channel::slack::SlackConfig {
            bot_token: config.bot_token.clone(),
            app_token: config.app_token.clone(),
            signing_secret: config.signing_secret.clone(),
            bot_user_id: config.bot_user_id.clone(),
            media_dir: config.media_dir.clone(),
        };
        let mut slack = rua_channel::slack::SlackChannel::new(slack_config)
            .with_client(channel_http_client(&config.common)?)
            .with_render_options(render_options(&config.common))
            .with_acl_config(build_acl_metadata(&config.common));

        if let Some(api_base) = string_option(&config.common, "api_base") {
            slack = slack.with_api_base(api_base);
        }
        if let Err(e) = self.manager.register(Arc::new(slack)).await {
            tracing::error!(error = %e, "failed to register Slack channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_sip_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.sip_config {
            Some(c) if !c.enabled => {
                tracing::debug!("SIP channel disabled, skipping");
                return Ok(());
            }
            Some(c) => c,
            None => {
                tracing::debug!("SIP channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering SIP channel");

        let sip_defaults = rua_channel::sip::SipConfig::default();
        let sip_config = rua_channel::sip::SipConfig {
            bind_addr: if config.bind_addr.trim().is_empty() {
                sip_defaults.bind_addr
            } else {
                config.bind_addr.clone()
            },
            public_addr: config.public_addr.clone(),
            rtp_bind_addr: if config.rtp_bind_addr.trim().is_empty() {
                sip_defaults.rtp_bind_addr
            } else {
                config.rtp_bind_addr.clone()
            },
            username: config.username.clone(),
            domain: config.domain.clone(),
            media_dir: config.media_dir.clone(),
            max_call_secs: if config.max_call_secs == 0 {
                sip_defaults.max_call_secs
            } else {
                config.max_call_secs
            },
        };
        let options: rua_channel::sip::SipOptions =
            serde_json::from_value(serde_json::json!(config.common.extra)).map_err(|error| {
                RegistrationError::IncompleteConfig(format!("invalid SIP options: {error}"))
            })?;
        rua_config::channel_catalog::validate_channel_fields("sip", &serde_json::json!(config))
            .map_err(RegistrationError::IncompleteConfig)?;
        let sip = rua_channel::sip::SipChannel::new(sip_config).with_options(options);

        if let Err(e) = self.manager.register(Arc::new(sip)).await {
            tracing::error!(error = %e, "failed to register SIP channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_discord_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.discord_config {
            Some(c) if !c.enabled => {
                tracing::debug!("Discord channel disabled, skipping");
                return Ok(());
            }
            Some(c) if !c.webhook_url.is_empty() || !c.bot_token.is_empty() => c,
            Some(_) => {
                tracing::warn!(
                    "Discord enabled but config incomplete: no webhook_url or bot_token"
                );
                return Err(RegistrationError::IncompleteConfig(
                    "no webhook_url or bot_token".into(),
                ));
            }
            None => {
                tracing::debug!("Discord channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering Discord channel");

        let discord_config = rua_channel::discord::DiscordConfig {
            webhook_url: config.webhook_url.clone(),
            bot_token: config.bot_token.clone(),
            media_dir: config.media_dir.clone(),
            bot_user_id: config.bot_user_id.clone(),
        };
        let discord = rua_channel::discord::DiscordChannel::new(discord_config)
            .with_client(channel_http_client(&config.common)?)
            .with_render_options(render_options(&config.common))
            .with_acl_config(build_acl_metadata(&config.common));

        if let Err(e) = self.manager.register(Arc::new(discord)).await {
            tracing::error!(error = %e, "failed to register Discord channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    #[cfg(feature = "matrix")]
    async fn register_matrix_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.matrix_config {
            Some(c) if !c.enabled => {
                tracing::debug!("Matrix channel disabled, skipping");
                return Ok(());
            }
            Some(c) if !c.homeserver_url.is_empty() => c,
            Some(_) => {
                tracing::warn!("Matrix enabled but config incomplete: homeserver_url empty");
                return Err(RegistrationError::IncompleteConfig(
                    "homeserver_url empty".into(),
                ));
            }
            None => {
                tracing::debug!("Matrix channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering Matrix channel");

        let matrix_config = rua_channel::matrix::MatrixConfig {
            homeserver_url: config.homeserver_url.clone(),
            user_id: config.user_id.clone(),
            password: config.password.clone(),
            access_token: config.access_token.clone(),
            default_room_id: config.default_room_id.clone(),
            encryption_enabled: config.encryption_enabled,
            store_path: config.store_path.clone(),
            media_dir: config.media_dir.clone(),
        };
        let options =
            serde_json::from_value(serde_json::json!(config.common.extra)).map_err(|error| {
                RegistrationError::IncompleteConfig(format!("invalid Matrix options: {error}"))
            })?;
        let matrix = rua_channel::matrix::MatrixChannel::new(matrix_config)
            .with_options(options)
            .with_device(
                string_option(&config.common, "device_id")
                    .unwrap_or_default()
                    .to_string(),
                string_option(&config.common, "device_name").unwrap_or_else(|| "rua-bot".into()),
            )
            .with_render_options(render_options(&config.common))
            .with_acl_config(build_acl_metadata(&config.common));

        if let Err(e) = self.manager.register(Arc::new(matrix)).await {
            tracing::error!(error = %e, "failed to register Matrix channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    #[cfg(feature = "mqtt")]
    async fn register_mqtt_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.mqtt_config {
            Some(c) if !c.enabled => {
                tracing::debug!("MQTT channel disabled, skipping");
                return Ok(());
            }
            Some(c) if !c.broker_url.is_empty() => c,
            Some(_) => {
                tracing::warn!("MQTT enabled but config incomplete: broker_url empty");
                return Err(RegistrationError::IncompleteConfig(
                    "broker_url empty".into(),
                ));
            }
            None => {
                tracing::debug!("MQTT channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering MQTT channel");

        let qos = match config
            .common
            .extra
            .get("qos")
            .and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            })
            .unwrap_or(2)
        {
            0 => rua_channel::mqtt::types::MqttQos::AtMostOnce,
            1 => rua_channel::mqtt::types::MqttQos::AtLeastOnce,
            2 => rua_channel::mqtt::types::MqttQos::ExactlyOnce,
            _ => {
                return Err(RegistrationError::IncompleteConfig(
                    "MQTT qos must be 0, 1 or 2".into(),
                ))
            }
        };
        let mqtt_config = rua_channel::mqtt::MqttConfig {
            broker_url: config.broker_url.clone(),
            port: config.port,
            subscribe_topic: config.subscribe_topic.clone(),
            publish_topic: config.publish_topic.clone(),
            qos,
            credentials: string_option(&config.common, "username").map(|username| {
                rua_channel::mqtt::types::MqttCredentials {
                    username,
                    password: string_option(&config.common, "password").unwrap_or_default(),
                }
            }),
            client_id: string_option(&config.common, "client_id"),
            tls_enabled: config
                .common
                .extra
                .get("tls_enabled")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            tls_ca_cert: string_option(&config.common, "tls_ca_certs"),
            tls_client_cert: string_option(&config.common, "tls_certfile"),
            tls_client_key: string_option(&config.common, "tls_keyfile"),
            keep_alive_secs: config
                .common
                .extra
                .get("keep_alive_secs")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(30),
            ..Default::default()
        };
        let mqtt = rua_channel::mqtt::MqttChannel::new(mqtt_config)
            .with_transport(
                string_option(&config.common, "transport").unwrap_or_else(|| "tcp".into()),
            )
            .with_clean_session(
                config
                    .common
                    .extra
                    .get("clean_session")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
            )
            .with_render_options(render_options(&config.common))
            .with_acl_config(build_acl_metadata(&config.common));

        if let Err(e) = self.manager.register(Arc::new(mqtt)).await {
            tracing::error!(error = %e, "failed to register MQTT channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_wechat_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.wechat_config {
            Some(c) if !c.enabled => {
                tracing::debug!("WeChat channel disabled, skipping");
                return Ok(());
            }
            Some(c) => c,
            None => {
                tracing::debug!("WeChat channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering WeChat channel");

        let mut bot_token = config.bot_token.clone();
        if bot_token.trim().is_empty() {
            if let Some(path) = string_option(&config.common, "bot_token_file") {
                let path = if let Some(relative) = path.strip_prefix("~/") {
                    std::env::var_os("HOME")
                        .map(std::path::PathBuf::from)
                        .map(|home_dir| home_dir.join(relative))
                        .ok_or_else(|| {
                            RegistrationError::IncompleteConfig(
                                "cannot resolve token file home directory".into(),
                            )
                        })?
                } else {
                    std::path::PathBuf::from(path)
                };
                bot_token = tokio::fs::read_to_string(path)
                    .await
                    .map_err(|error| {
                        RegistrationError::IncompleteConfig(format!("WeChat token file: {error}"))
                    })?
                    .trim()
                    .to_string();
            }
        }
        if bot_token.is_empty() {
            return Err(RegistrationError::IncompleteConfig(
                "WeChat bot_token or bot_token_file is required".into(),
            ));
        }
        let wechat = rua_channel::wechat::WechatChannel::new(
            bot_token,
            config.base_url.clone().filter(|url| !url.trim().is_empty()),
        )
        .with_render_options(render_options(&config.common))
        .with_acl_config(build_acl_metadata(&config.common))
        .with_media_dir(config.media_dir.clone());

        if let Err(e) = self.manager.register(Arc::new(wechat)).await {
            tracing::error!(error = %e, "failed to register WeChat channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_xiaoyi_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.xiaoyi_config {
            Some(c) if !c.enabled => {
                tracing::debug!("Xiaoyi channel disabled, skipping");
                return Ok(());
            }
            Some(c)
                if !c.access_key.is_empty()
                    && !c.secret_key.is_empty()
                    && !c.agent_id.is_empty() =>
            {
                c
            }
            Some(_) => {
                tracing::warn!(
                    "Xiaoyi enabled but config incomplete: access_key, secret_key, or agent_id empty"
                );
                return Err(RegistrationError::IncompleteConfig(
                    "access_key, secret_key, or agent_id empty".into(),
                ));
            }
            None => {
                tracing::debug!("Xiaoyi channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering Xiaoyi channel");

        let xiaoyi_config = rua_channel::xiaoyi::XiaoyiConfig {
            access_key: config.access_key.clone(),
            secret_key: config.secret_key.clone(),
            agent_id: config.agent_id.clone(),
            ws_url: if config.ws_url.is_empty() {
                rua_channel::xiaoyi::types::DEFAULT_WS_URL.to_string()
            } else {
                config.ws_url.clone()
            },
            backup_ws_url: if config.backup_ws_url.is_empty() {
                rua_channel::xiaoyi::types::DEFAULT_WS_URL_BACKUP.to_string()
            } else {
                config.backup_ws_url.clone()
            },
            media_dir: config.media_dir.clone(),
        };
        let xiaoyi = rua_channel::xiaoyi::XiaoyiChannel::new(xiaoyi_config)
            .with_render_options(render_options(&config.common))
            .with_acl_config(build_acl_metadata(&config.common));

        if let Err(e) = self.manager.register(Arc::new(xiaoyi)).await {
            tracing::error!(error = %e, "failed to register Xiaoyi channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }

    async fn register_yuanbao_channel(&self) -> Result<(), RegistrationError> {
        let config = match &self.yuanbao_config {
            Some(c) if !c.enabled => {
                tracing::debug!("Yuanbao channel disabled, skipping");
                return Ok(());
            }
            Some(c) if !c.app_id.is_empty() && !c.app_secret.is_empty() => c,
            Some(_) => {
                tracing::warn!("Yuanbao enabled but config incomplete: app_id or app_secret empty");
                return Err(RegistrationError::IncompleteConfig(
                    "app_id or app_secret empty".into(),
                ));
            }
            None => {
                tracing::debug!("Yuanbao channel disabled, skipping");
                return Ok(());
            }
        };

        tracing::info!("registering Yuanbao channel");

        let yuanbao_config = rua_channel::yuanbao::YuanbaoConfig {
            enabled: true,
            app_id: config.app_id.clone(),
            app_secret: config.app_secret.clone(),
            api_domain: config.api_domain.clone(),
            media_dir: config.media_dir.clone(),
        };
        let yuanbao = rua_channel::yuanbao::YuanbaoChannel::new(yuanbao_config)
            .with_ws_url(
                string_option(&config.common, "ws_url")
                    .unwrap_or_default()
                    .to_string(),
            )
            .with_render_options(render_options(&config.common))
            .with_acl_config(build_acl_metadata(&config.common));

        if let Err(e) = self.manager.register(Arc::new(yuanbao)).await {
            tracing::error!(error = %e, "failed to register Yuanbao channel");
            return Err(RegistrationError::IncompleteConfig(e.to_string()));
        }
        Ok(())
    }
}

impl ChannelsServiceAdapter {
    pub async fn register_configured_channels(&self) -> Result<(), WorkspaceError> {
        let mut failed = Vec::new();
        if let Err(e) = self.register_dingtalk_channel().await {
            failed.push("dingtalk");
            tracing::warn!(error = %e, "DingTalk channel not registered, continuing without");
        }
        if let Err(e) = self.register_wecom_channel().await {
            failed.push("wecom");
            tracing::warn!(error = %e, "WeCom channel not registered, continuing without");
        }
        if let Err(e) = self.register_feishu_channel().await {
            failed.push("feishu");
            tracing::warn!(error = %e, "Feishu channel not registered, continuing without");
        }
        if let Err(e) = self.register_telegram_channel().await {
            failed.push("telegram");
            tracing::warn!(error = %e, "Telegram channel not registered, continuing without");
        }
        if let Err(e) = self.register_discord_channel().await {
            failed.push("discord");
            tracing::warn!(error = %e, "Discord channel not registered, continuing without");
        }
        if let Err(e) = self.register_onebot_channel().await {
            failed.push("onebot");
            tracing::warn!(error = %e, "OneBot channel not registered, continuing without");
        }
        if let Err(e) = self.register_mattermost_channel().await {
            failed.push("mattermost");
            tracing::warn!(error = %e, "Mattermost channel not registered, continuing without");
        }
        if let Err(e) = self.register_slack_channel().await {
            failed.push("slack");
            tracing::warn!(error = %e, "Slack channel not registered, continuing without");
        }
        if let Err(e) = self.register_sip_channel().await {
            failed.push("sip");
            tracing::warn!(error = %e, "SIP channel not registered, continuing without");
        }
        if let Err(e) = self.register_qq_channel().await {
            failed.push("qq");
            tracing::warn!(error = %e, "QQ channel not registered, continuing without");
        }
        #[cfg(feature = "matrix")]
        if let Err(e) = self.register_matrix_channel().await {
            failed.push("matrix");
            tracing::warn!(error = %e, "Matrix channel not registered, continuing without");
        }
        #[cfg(feature = "mqtt")]
        if let Err(e) = self.register_mqtt_channel().await {
            failed.push("mqtt");
            tracing::warn!(error = %e, "MQTT channel not registered, continuing without");
        }
        if let Err(e) = self.register_wechat_channel().await {
            failed.push("wechat");
            tracing::warn!(error = %e, "WeChat channel not registered, continuing without");
        }
        if let Err(e) = self.register_xiaoyi_channel().await {
            failed.push("xiaoyi");
            tracing::warn!(error = %e, "Xiaoyi channel not registered, continuing without");
        }
        if let Err(e) = self.register_yuanbao_channel().await {
            failed.push("yuanbao");
            tracing::warn!(error = %e, "Yuanbao channel not registered, continuing without");
        }
        #[cfg(all(feature = "imessage", target_os = "macos"))]
        if let Err(error) = self.register_imessage_channel().await {
            failed.push("imessage");
            tracing::warn!(%error, "iMessage channel not registered");
        }
        if !failed.is_empty() {
            return Err(WorkspaceError::component_init_failed(
                "channels",
                format!("Channel configuration failed: {}", failed.join(", ")),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl ServiceLifecycle for ChannelsServiceAdapter {
    async fn start(&self) -> Result<(), WorkspaceError> {
        let registration = self.register_configured_channels().await;
        let started =
            self.manager.start_all().await.map_err(|error| {
                WorkspaceError::component_init_failed("channels", error.to_string())
            });
        registration.and(started)
    }

    async fn stop(&self) -> Result<(), WorkspaceError> {
        self.manager.stop_all().await.map_err(|e| {
            WorkspaceError::component_shutdown_failed(
                "channels",
                format!("ChannelManager::stop_all failed: {e}"),
            )
        })
    }
}
