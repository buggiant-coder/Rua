#![forbid(unsafe_code)]

pub mod channel;
mod inbound;
mod outbound;
pub mod types;
mod verification;

pub use channel::{MatrixChannel, MatrixTypingIndicator};
pub use types::{MatrixConfig, MatrixOptions};
