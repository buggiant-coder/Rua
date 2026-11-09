//! Shared desktop platform facts; independent of the window shell.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionSettings {
    Accessibility,
    ScreenRecording,
}

/// The pinned Cua Driver needs macOS 13+; Windows support is checked by its driver.
pub fn computer_os_supported() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|version| version.trim().split('.').next()?.parse::<u32>().ok())
            .is_some_and(|major| major >= 13)
    }
    #[cfg(not(target_os = "macos"))]
    {
        cfg!(target_os = "windows")
    }
}
