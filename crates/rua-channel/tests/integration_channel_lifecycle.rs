#[cfg(all(feature = "imessage", target_os = "macos"))]
#[path = "integration_channel_lifecycle/imessage.rs"]
mod imessage;
#[cfg(feature = "mqtt")]
#[path = "integration_channel_lifecycle/mqtt.rs"]
mod mqtt;
#[cfg(feature = "onebot")]
#[path = "integration_channel_lifecycle/onebot.rs"]
mod onebot;
#[cfg(feature = "slack")]
#[path = "integration_channel_lifecycle/slack.rs"]
mod slack;
#[path = "integration_channel_lifecycle/support.rs"]
mod support;

#[cfg(feature = "sip")]
#[path = "integration_channel_lifecycle/sip.rs"]
mod sip;

#[cfg(feature = "sip-livekit")]
#[path = "integration_channel_lifecycle/livekit.rs"]
mod livekit;

#[cfg(feature = "wechat")]
#[path = "integration_channel_lifecycle/wechat.rs"]
mod wechat;

#[cfg(feature = "wecom")]
#[path = "integration_channel_lifecycle/wecom.rs"]
mod wecom;

#[cfg(feature = "mattermost")]
#[path = "integration_channel_lifecycle/mattermost.rs"]
mod mattermost;

#[cfg(feature = "dingtalk")]
#[path = "integration_channel_lifecycle/dingtalk.rs"]
mod dingtalk;

#[cfg(feature = "discord")]
#[path = "integration_channel_lifecycle/discord.rs"]
mod discord;
