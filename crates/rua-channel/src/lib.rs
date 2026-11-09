#![forbid(unsafe_code)]
#![recursion_limit = "512"]

pub mod core;
mod http_client;

#[cfg(feature = "dingtalk")]
pub mod dingtalk;

#[cfg(feature = "feishu")]
pub mod feishu;

#[cfg(feature = "telegram")]
pub mod telegram;

#[cfg(feature = "discord")]
pub mod discord;

#[cfg(feature = "onebot")]
pub mod onebot;

#[cfg(feature = "qq")]
pub mod qq;

#[cfg(feature = "mqtt")]
pub mod mqtt;

#[cfg(feature = "mattermost")]
pub mod mattermost;

#[cfg(feature = "slack")]
pub mod slack;

#[cfg(feature = "sip")]
pub mod sip;

#[cfg(feature = "matrix")]
pub mod matrix;

#[cfg(feature = "wecom")]
pub mod wecom;

#[cfg(all(feature = "imessage", target_os = "macos"))]
pub mod imessage;

#[cfg(feature = "wechat")]
pub mod wechat;

#[cfg(feature = "xiaoyi")]
pub mod xiaoyi;

#[cfg(feature = "yuanbao")]
pub mod yuanbao;
