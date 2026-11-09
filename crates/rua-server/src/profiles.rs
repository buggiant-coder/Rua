use std::str::FromStr;

use rua_sync::power_policy::SyncRuntimeHostProfile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ServerProfile {
    #[default]
    Desktop,

    MobileApk,

    Headless,

    Test,
}

impl ServerProfile {
    pub fn start_external_channels(&self) -> bool {
        matches!(self, Self::Desktop | Self::Headless)
    }

    pub fn start_cron_scheduler(&self) -> bool {
        matches!(self, Self::Desktop | Self::Headless)
    }

    pub fn scan_local_models(&self) -> bool {
        matches!(self, Self::Desktop | Self::Headless)
    }

    pub fn auto_start_mcp(&self) -> bool {
        matches!(self, Self::Desktop | Self::Headless)
    }

    pub fn serve_webui(&self) -> bool {
        matches!(self, Self::Desktop | Self::MobileApk)
    }

    pub fn scan_skills_on_startup(&self) -> bool {
        matches!(self, Self::Desktop | Self::Headless)
    }

    pub fn allow_network(&self) -> bool {
        !matches!(self, Self::Test)
    }

    pub fn sync_runtime_host_profile(&self) -> SyncRuntimeHostProfile {
        match self {
            Self::Desktop => SyncRuntimeHostProfile::Desktop,
            Self::MobileApk => SyncRuntimeHostProfile::MobileApk,
            Self::Headless => SyncRuntimeHostProfile::Headless,
            Self::Test => SyncRuntimeHostProfile::Test,
        }
    }
}

impl std::fmt::Display for ServerProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Desktop => write!(f, "desktop"),
            Self::MobileApk => write!(f, "mobile-apk"),
            Self::Headless => write!(f, "headless"),
            Self::Test => write!(f, "test"),
        }
    }
}

impl FromStr for ServerProfile {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "desktop" => Ok(Self::Desktop),
            "mobile" | "mobile-apk" => Ok(Self::MobileApk),
            "headless" => Ok(Self::Headless),
            "test" => Ok(Self::Test),
            _ => Err(format!("unknown profile: {s}")),
        }
    }
}
