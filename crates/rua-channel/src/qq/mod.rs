#![forbid(unsafe_code)]

mod api;
pub mod approval_card;
mod channel;
mod gateway;
pub mod types;

pub use channel::QqChannel;
