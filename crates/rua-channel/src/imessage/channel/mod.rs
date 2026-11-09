#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::IMessageChannel;

#[cfg(not(target_os = "macos"))]
mod stub;

#[cfg(not(target_os = "macos"))]
pub use stub::IMessageChannel;
