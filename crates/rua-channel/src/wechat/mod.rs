pub mod channel;
pub mod client;
mod merge;
pub mod types;

pub use channel::{WechatChannel, WechatTypingIndicator};
pub use client::ILinkClient;
pub use types::WechatConfig;
