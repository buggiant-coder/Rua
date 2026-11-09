pub mod android;
pub mod desktop;
pub mod unsupported;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolProfile {
    DesktopFull,

    ServerFull,

    MobileDefault,

    MobileWithProot,
}

impl ToolProfile {
    pub fn has_search(&self) -> bool {
        true
    }

    pub fn has_web(&self) -> bool {
        true
    }

    pub fn has_full_media(&self) -> bool {
        matches!(self, Self::DesktopFull | Self::ServerFull)
    }

    pub fn has_tasks(&self) -> bool {
        true
    }

    pub fn has_shell(&self) -> bool {
        matches!(
            self,
            Self::DesktopFull | Self::ServerFull | Self::MobileWithProot
        )
    }

    pub fn platform_name(&self) -> &'static str {
        match self {
            Self::DesktopFull => "desktop",
            Self::ServerFull => "server",
            Self::MobileDefault => "android-default",
            Self::MobileWithProot => "android-proot",
        }
    }

    pub fn detect() -> Self {
        #[cfg(target_os = "android")]
        {
            Self::MobileDefault
        }
        #[cfg(not(target_os = "android"))]
        {
            if std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok() {
                Self::DesktopFull
            } else {
                Self::ServerFull
            }
        }
    }
}

impl std::fmt::Display for ToolProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.platform_name())
    }
}

pub fn check_tool_available(
    tool_name: &str,
    profile: ToolProfile,
    required_capability: &str,
    available: bool,
) -> Result<(), crate::error::ToolError> {
    if !available {
        Err(crate::error::ToolError::UnsupportedOnPlatform {
            tool: tool_name.to_string(),
            platform: profile.platform_name().to_string(),
            reason: format!(
                "{required_capability} is not available on {profile}. \
                 Consider using a different platform profile or enabling the required feature."
            ),
        })
    } else {
        Ok(())
    }
}
