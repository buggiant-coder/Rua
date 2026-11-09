#![forbid(unsafe_code)]

pub mod channel;
mod socket_mode;
pub mod types;

pub use channel::{SlackChannel, SlackStreamHandler};
pub use types::SlackConfig;
