//! Device-local browser preference; deliberately outside Agent and sync configuration.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DesktopAutomationPreferences {
    #[serde(default)]
    pub browser_path: Option<PathBuf>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

pub fn preferences_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("desktop-automation/preferences.json")
}

impl DesktopAutomationPreferences {
    pub fn load(path: &Path) -> std::io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        crate::atomic_write(path, &bytes)
    }
}
