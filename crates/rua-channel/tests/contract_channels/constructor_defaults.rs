use rua_channel::core::channel::Channel;
use rua_channel::core::config::{BaseChannelConfig, ConsoleChannelConfig};
use rua_channel::dingtalk::DingTalkChannel;
use rua_channel::discord::{DiscordChannel, DiscordConfig};
use rua_channel::feishu::{FeishuChannel, FeishuConfig};
use rua_channel::mattermost::{MattermostChannel, MattermostConfig};
use rua_channel::onebot::{OneBotChannel, OneBotConfig};
use rua_channel::qq::QqChannel;
use rua_channel::telegram::{TelegramChannel, TelegramConfig};
use rua_channel::wechat::{WechatChannel, WechatConfig};
use rua_channel::wecom::{WecomBotConfig, WecomChannel};
use rua_channel::xiaoyi::{types as xiaoyi_types, XiaoyiChannel, XiaoyiConfig};
use rua_channel::yuanbao::{types as yuanbao_types, YuanbaoChannel, YuanbaoConfig};

fn assert_constructs_without_network(expected_id: &str, channel: impl Channel) {
    assert_eq!(channel.id(), expected_id);
}

#[test]
fn base_channel_config_defaults_match_shared_contract() {
    let base: BaseChannelConfig = serde_json::from_value(serde_json::json!({})).unwrap();

    assert!(base.enabled);
    assert!(base.show_thinking);
    assert!(base.show_tool_calls && base.show_tool_results);
    assert!(base.show_tool_details);
    assert!(!base.thread_follow_without_mention);
    assert!(!base.require_mention);
    assert!(base.dm_policy.is_empty());
    assert!(base.group_policy.is_empty());
    assert!(base.allow_from.is_empty());
    assert!(base.deny_from.is_empty());
    assert!(!base.access_control_dm);
    assert!(!base.access_control_group);
    assert!(base.language.is_empty());

    let console: ConsoleChannelConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(console.is_enabled());
}

#[test]
fn default_feature_adapter_configs_deserialize_with_stable_defaults() {
    let feishu: FeishuConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(feishu.webhook_url.is_empty());
    assert!(feishu.secret.is_none());
    assert!(feishu.app_id.is_none());
    assert!(feishu.app_secret.is_none());

    let telegram: TelegramConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(telegram.bot_token.is_empty());
    assert!(telegram.chat_id.is_empty());
    assert!(telegram.allowed_users.is_empty());

    let discord: DiscordConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(discord.webhook_url.is_empty());
    assert!(discord.bot_token.is_empty());

    let onebot: OneBotConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(onebot.http_url.is_empty());
    assert_eq!(onebot.ws_host, "127.0.0.1");
    assert_eq!(onebot.ws_port, 6199);
    assert!(onebot.access_token.is_none());

    let mattermost: MattermostConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(mattermost.url.is_empty());
    assert!(mattermost.bot_token.is_empty());

    let wechat: WechatConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(wechat.bot_token.is_none());
    assert!(wechat.base_url.is_none());

    let xiaoyi: XiaoyiConfig = serde_json::from_value(serde_json::json!({
        "access_key": "ak",
        "secret_key": "sk",
        "agent_id": "agent"
    }))
    .unwrap();
    assert_eq!(xiaoyi.ws_url, xiaoyi_types::DEFAULT_WS_URL);

    let xiaoyi_aliases: XiaoyiConfig = serde_json::from_value(serde_json::json!({
        "ak": "alias-ak",
        "sk": "alias-sk",
        "agent_id": "agent"
    }))
    .unwrap();
    assert_eq!(xiaoyi_aliases.access_key, "alias-ak");
    assert_eq!(xiaoyi_aliases.secret_key, "alias-sk");
    assert_eq!(xiaoyi_aliases.ws_url, xiaoyi_types::DEFAULT_WS_URL);

    let yuanbao: YuanbaoConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(!yuanbao.enabled);
    assert!(yuanbao.app_id.is_empty());
    assert!(yuanbao.app_secret.is_empty());
    assert_eq!(yuanbao.api_domain, yuanbao_types::DEFAULT_API_DOMAIN);

    let wecom = WecomBotConfig::new("bot".into(), "secret".into());
    assert_eq!(wecom.ws_url, rua_channel::wecom::types::DEFAULT_WS_URL);
}

#[test]
fn default_feature_adapters_construct_from_minimal_or_explicit_config_without_network() {
    assert_constructs_without_network("dingtalk", DingTalkChannel::new());
    assert_constructs_without_network(
        "feishu",
        FeishuChannel::new(FeishuConfig {
            webhook_url: "https://example.invalid/feishu".into(),
            secret: Some("signing-secret".into()),
            app_id: Some("app-id".into()),
            app_secret: Some("app-secret".into()),
            ..Default::default()
        }),
    );
    assert_constructs_without_network(
        "telegram",
        TelegramChannel::new(TelegramConfig {
            bot_token: "token".into(),
            chat_id: "chat".into(),
            allowed_users: vec!["alice".into()],
            ..Default::default()
        }),
    );
    assert_constructs_without_network(
        "discord",
        DiscordChannel::new(DiscordConfig {
            webhook_url: "https://example.invalid/discord".into(),
            bot_token: "bot-token".into(),
            ..Default::default()
        }),
    );
    assert_constructs_without_network(
        "onebot",
        OneBotChannel::new(OneBotConfig {
            http_url: "http://127.0.0.1:6700".into(),
            ws_host: "127.0.0.1".into(),
            ws_port: 6701,
            access_token: Some("access-token".into()),
            ..Default::default()
        }),
    );
    assert_constructs_without_network("qq", QqChannel::new("appid".into(), "secret".into(), true));
    assert_constructs_without_network(
        "mattermost",
        MattermostChannel::new(MattermostConfig {
            url: "https://mattermost.example.invalid/".into(),
            bot_token: "bot-token".into(),
            ..Default::default()
        }),
    );
    assert_constructs_without_network("wecom", WecomChannel::new("bot-id".into(), "secret".into()));
    assert_constructs_without_network(
        "wechat",
        WechatChannel::new(
            "token".into(),
            Some("https://wechat.example.invalid".into()),
        ),
    );
    assert_constructs_without_network(
        "xiaoyi",
        XiaoyiChannel::new(XiaoyiConfig {
            access_key: "ak".into(),
            secret_key: "sk".into(),
            agent_id: "agent".into(),
            ws_url: "ws://127.0.0.1:1".into(),
            backup_ws_url: String::new(),
            media_dir: String::new(),
        }),
    );
    assert_constructs_without_network(
        "yuanbao",
        YuanbaoChannel::new(YuanbaoConfig {
            enabled: true,
            app_id: "app".into(),
            app_secret: "secret".into(),
            api_domain: "yuanbao.example.invalid".into(),
            media_dir: String::new(),
        }),
    );
}
