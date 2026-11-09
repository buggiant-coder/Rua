use rua_channel::core::channel::Channel;
use rua_channel::dingtalk::DingTalkChannel;
use rua_channel::discord::{DiscordChannel, DiscordConfig};
use rua_channel::feishu::{FeishuChannel, FeishuConfig};
use rua_channel::mattermost::{MattermostChannel, MattermostConfig};
use rua_channel::onebot::{OneBotChannel, OneBotConfig};
use rua_channel::qq::QqChannel;
use rua_channel::telegram::{TelegramChannel, TelegramConfig};
use rua_channel::wechat::WechatChannel;
use rua_channel::wecom::WecomChannel;
use rua_channel::xiaoyi::{XiaoyiChannel, XiaoyiConfig};
use rua_channel::yuanbao::{YuanbaoChannel, YuanbaoConfig};

fn assert_modern_channel<T: Channel>() {}

#[test]
fn default_feature_channels_implement_modern_channel_trait() {
    assert_modern_channel::<DingTalkChannel>();
    assert_modern_channel::<FeishuChannel>();
    assert_modern_channel::<TelegramChannel>();
    assert_modern_channel::<DiscordChannel>();
    assert_modern_channel::<OneBotChannel>();
    assert_modern_channel::<QqChannel>();
    assert_modern_channel::<MattermostChannel>();
    assert_modern_channel::<WecomChannel>();
    assert_modern_channel::<WechatChannel>();
    assert_modern_channel::<XiaoyiChannel>();
    assert_modern_channel::<YuanbaoChannel>();
}

#[test]
fn default_feature_channels_expose_stable_ids() {
    let cases: Vec<(&str, Box<dyn Channel>)> = vec![
        ("dingtalk", Box::new(DingTalkChannel::new())),
        (
            "feishu",
            Box::new(FeishuChannel::new(FeishuConfig::default())),
        ),
        (
            "telegram",
            Box::new(TelegramChannel::new(TelegramConfig::default())),
        ),
        (
            "discord",
            Box::new(DiscordChannel::new(DiscordConfig::default())),
        ),
        (
            "onebot",
            Box::new(OneBotChannel::new(OneBotConfig::default())),
        ),
        (
            "qq",
            Box::new(QqChannel::new(String::new(), String::new(), false)),
        ),
        (
            "mattermost",
            Box::new(MattermostChannel::new(MattermostConfig::default())),
        ),
        (
            "wecom",
            Box::new(WecomChannel::new(String::new(), String::new())),
        ),
        ("wechat", Box::new(WechatChannel::new(String::new(), None))),
        (
            "xiaoyi",
            Box::new(XiaoyiChannel::new(XiaoyiConfig {
                access_key: String::new(),
                secret_key: String::new(),
                agent_id: String::new(),
                ws_url: "ws://127.0.0.1:1".to_string(),
                backup_ws_url: String::new(),
                media_dir: String::new(),
            })),
        ),
        (
            "yuanbao",
            Box::new(YuanbaoChannel::new(YuanbaoConfig::default())),
        ),
    ];

    for (expected, channel) in cases {
        assert_eq!(
            channel.id(),
            expected,
            "channel contract id changed for {expected}"
        );
    }
}
