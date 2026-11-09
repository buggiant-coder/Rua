#[path = "contract_diagnostics/clock.rs"]
mod clock;
#[cfg(target_os = "macos")]
#[path = "contract_diagnostics/socket.rs"]
mod socket;
