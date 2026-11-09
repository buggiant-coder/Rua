use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{SyncError, SyncResult};

const POLICY_FILE: &str = "power_policy.json";
const DEFAULT_PERIODIC_INTERVAL_SECS: u64 = 600;
const MOBILE_BACKGROUND_FLUSH_GRACE_SECS: u64 = 60;
const PROCESS_RUNNING_BACKGROUND_FLUSH_GRACE_SECS: u64 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyncRuntimeHostProfile {
    MobileApk,
    Desktop,
    Headless,
    Test,
}

impl SyncRuntimeHostProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MobileApk => "mobile-apk",
            Self::Desktop => "desktop",
            Self::Headless => "headless",
            Self::Test => "test",
        }
    }

    fn is_mobile(self) -> bool {
        matches!(self, Self::MobileApk)
    }
}

impl fmt::Display for SyncRuntimeHostProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncPowerPolicyMode {
    PowerSaver,
    Balanced,
    Realtime,
    Custom,
}

impl SyncPowerPolicyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PowerSaver => "power_saver",
            Self::Balanced => "balanced",
            Self::Realtime => "realtime",
            Self::Custom => "custom",
        }
    }
}

impl FromStr for SyncPowerPolicyMode {
    type Err = PowerPolicyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "power_saver" => Ok(Self::PowerSaver),
            "balanced" => Ok(Self::Balanced),
            "realtime" => Ok(Self::Realtime),
            "custom" => Ok(Self::Custom),
            _ => Err(PowerPolicyError::InvalidMode(s.to_string())),
        }
    }
}

impl fmt::Display for SyncPowerPolicyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPowerPolicyConfig {
    pub mode: SyncPowerPolicyMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub periodic_interval_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_flush_grace_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_background_runtime: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_background_scheduled_sync: Option<bool>,
}

impl SyncPowerPolicyConfig {
    pub fn default_for_host(host_profile: SyncRuntimeHostProfile) -> Self {
        Self {
            mode: default_mode_for_host(host_profile),
            periodic_interval_secs: None,
            background_flush_grace_secs: None,
            allow_background_runtime: None,
            allow_background_scheduled_sync: None,
        }
    }

    pub fn normalize(
        &self,
        host_profile: SyncRuntimeHostProfile,
    ) -> Result<EffectiveSyncPowerPolicy, PowerPolicyError> {
        normalize_policy(Some(self), host_profile)
    }

    pub fn validate(&self) -> Result<(), PowerPolicyError> {
        if let Some(interval) = self.periodic_interval_secs {
            if interval == 0 {
                return Err(PowerPolicyError::InvalidParameter(
                    "periodic_interval_secs must be greater than 0".to_string(),
                ));
            }
        }
        if self.mode == SyncPowerPolicyMode::Custom {
            let missing = [
                (
                    self.periodic_interval_secs.is_none(),
                    "periodic_interval_secs",
                ),
                (
                    self.background_flush_grace_secs.is_none(),
                    "background_flush_grace_secs",
                ),
                (
                    self.allow_background_runtime.is_none(),
                    "allow_background_runtime",
                ),
                (
                    self.allow_background_scheduled_sync.is_none(),
                    "allow_background_scheduled_sync",
                ),
            ]
            .into_iter()
            .filter_map(|(is_missing, name)| is_missing.then_some(name))
            .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(PowerPolicyError::InvalidParameter(format!(
                    "custom mode requires {}",
                    missing.join(", ")
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveSyncPowerPolicy {
    pub mode: SyncPowerPolicyMode,
    pub periodic_interval_secs: u64,
    pub background_flush_grace_secs: u64,
    pub allow_background_runtime: bool,
    pub allow_background_scheduled_sync: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedSyncPowerPolicy {
    pub host_profile: SyncRuntimeHostProfile,
    pub stored_policy: Option<SyncPowerPolicyConfig>,
    pub effective_policy: EffectiveSyncPowerPolicy,
}

impl LoadedSyncPowerPolicy {
    pub fn load(data_dir: &Path, host_profile: SyncRuntimeHostProfile) -> SyncResult<Self> {
        let path = policy_path(data_dir);
        let stored_policy = if path.exists() {
            let bytes = fs::read(&path)?;
            let policy: SyncPowerPolicyConfig = serde_json::from_slice(&bytes)?;
            policy
                .validate()
                .map_err(|e| SyncError::Store(e.to_string()))?;
            Some(policy)
        } else {
            None
        };
        let effective_policy = normalize_policy(stored_policy.as_ref(), host_profile)
            .map_err(|e| SyncError::Store(e.to_string()))?;
        Ok(Self {
            host_profile,
            stored_policy,
            effective_policy,
        })
    }

    pub fn save(
        data_dir: &Path,
        host_profile: SyncRuntimeHostProfile,
        policy: SyncPowerPolicyConfig,
    ) -> SyncResult<Self> {
        policy
            .validate()
            .map_err(|e| SyncError::Store(e.to_string()))?;
        let effective_policy = policy
            .normalize(host_profile)
            .map_err(|e| SyncError::Store(e.to_string()))?;
        write_policy_file(&policy_path(data_dir), &policy)?;
        Ok(Self {
            host_profile,
            stored_policy: Some(policy),
            effective_policy,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PowerPolicyError {
    #[error("invalid sync power policy mode: {0}")]
    InvalidMode(String),
    #[error("invalid sync power policy parameter: {0}")]
    InvalidParameter(String),
}

fn default_mode_for_host(host_profile: SyncRuntimeHostProfile) -> SyncPowerPolicyMode {
    if host_profile.is_mobile() {
        SyncPowerPolicyMode::PowerSaver
    } else {
        SyncPowerPolicyMode::Realtime
    }
}

fn normalize_policy(
    policy: Option<&SyncPowerPolicyConfig>,
    host_profile: SyncRuntimeHostProfile,
) -> Result<EffectiveSyncPowerPolicy, PowerPolicyError> {
    let default_policy;
    let policy = match policy {
        Some(policy) => policy,
        None => {
            default_policy = SyncPowerPolicyConfig::default_for_host(host_profile);
            &default_policy
        }
    };
    policy.validate()?;

    let defaults = mode_defaults(policy.mode, host_profile);
    Ok(EffectiveSyncPowerPolicy {
        mode: policy.mode,
        periodic_interval_secs: policy
            .periodic_interval_secs
            .unwrap_or(defaults.periodic_interval_secs),
        background_flush_grace_secs: policy
            .background_flush_grace_secs
            .unwrap_or(defaults.background_flush_grace_secs),
        allow_background_runtime: policy
            .allow_background_runtime
            .unwrap_or(defaults.allow_background_runtime),
        allow_background_scheduled_sync: policy
            .allow_background_scheduled_sync
            .unwrap_or(defaults.allow_background_scheduled_sync),
    })
}

fn mode_defaults(
    mode: SyncPowerPolicyMode,
    host_profile: SyncRuntimeHostProfile,
) -> EffectiveSyncPowerPolicy {
    let process_running = !host_profile.is_mobile();
    match mode {
        SyncPowerPolicyMode::PowerSaver => EffectiveSyncPowerPolicy {
            mode,
            periodic_interval_secs: DEFAULT_PERIODIC_INTERVAL_SECS,
            background_flush_grace_secs: if host_profile.is_mobile() {
                MOBILE_BACKGROUND_FLUSH_GRACE_SECS
            } else {
                PROCESS_RUNNING_BACKGROUND_FLUSH_GRACE_SECS
            },
            allow_background_runtime: process_running,
            allow_background_scheduled_sync: false,
        },
        SyncPowerPolicyMode::Balanced => EffectiveSyncPowerPolicy {
            mode,
            periodic_interval_secs: DEFAULT_PERIODIC_INTERVAL_SECS,
            background_flush_grace_secs: if host_profile.is_mobile() {
                MOBILE_BACKGROUND_FLUSH_GRACE_SECS
            } else {
                PROCESS_RUNNING_BACKGROUND_FLUSH_GRACE_SECS
            },
            allow_background_runtime: process_running,
            allow_background_scheduled_sync: true,
        },
        SyncPowerPolicyMode::Realtime => EffectiveSyncPowerPolicy {
            mode,
            periodic_interval_secs: DEFAULT_PERIODIC_INTERVAL_SECS,
            background_flush_grace_secs: if host_profile.is_mobile() {
                MOBILE_BACKGROUND_FLUSH_GRACE_SECS
            } else {
                PROCESS_RUNNING_BACKGROUND_FLUSH_GRACE_SECS
            },
            allow_background_runtime: true,
            allow_background_scheduled_sync: true,
        },
        SyncPowerPolicyMode::Custom => EffectiveSyncPowerPolicy {
            mode,
            periodic_interval_secs: DEFAULT_PERIODIC_INTERVAL_SECS,
            background_flush_grace_secs: if host_profile.is_mobile() {
                MOBILE_BACKGROUND_FLUSH_GRACE_SECS
            } else {
                PROCESS_RUNNING_BACKGROUND_FLUSH_GRACE_SECS
            },
            allow_background_runtime: process_running,
            allow_background_scheduled_sync: process_running,
        },
    }
}

pub fn policy_path(data_dir: &Path) -> PathBuf {
    data_dir.join("sync").join(POLICY_FILE)
}

fn write_policy_file(path: &Path, policy: &SyncPowerPolicyConfig) -> SyncResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| SyncError::Store("policy path has no parent directory".to_string()))?;
    fs::create_dir_all(parent)?;

    let tmp_path = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp_path)?;
        serde_json::to_writer_pretty(&mut file, policy)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    sync_parent_dir(parent)?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir(parent: &Path) -> std::io::Result<()> {
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_dir(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}
