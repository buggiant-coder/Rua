#![forbid(unsafe_code)]

pub mod channel;
pub mod parser;
pub mod sender;
pub mod types;
pub mod ws;

pub use channel::{MattermostChannel, MattermostTypingIndicator};
pub use parser::parse_native;
pub use sender::MattermostSender;
pub use types::MattermostConfig;
