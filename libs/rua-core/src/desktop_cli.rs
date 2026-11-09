//! Device-local installation of the unified rua command, shared by all viewers.
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Result, RuaError};

#[derive(Clone)]
pub struct DesktopCliInstallation {
    source: PathBuf,
    destination: PathBuf,
    marker: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct DesktopCliStatus {
    pub path: PathBuf,
    pub installed: bool,
    pub conflict: bool,
}

#[derive(Serialize, Deserialize)]
struct InstallMarker {
    version: u8,
    destination: PathBuf,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl DesktopCliInstallation {
    pub fn new(source: PathBuf, home: &Path) -> Self {
        let name = if cfg!(windows) { "rua.exe" } else { "rua" };
        Self {
            source,
            destination: home.join(".local/bin").join(name),
            marker: home.join(".local/share/rua/desktop-cli-install.json"),
        }
    }

    pub fn status(&self) -> DesktopCliStatus {
        let exists = self.destination.symlink_metadata().is_ok();
        let owned = fs::read(&self.marker)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<InstallMarker>(&bytes).ok())
            .is_some_and(|marker| marker.version == 1 && marker.destination == self.destination);
        DesktopCliStatus {
            path: self.destination.clone(),
            installed: self.destination.is_file() && owned,
            conflict: exists && !owned,
        }
    }

    pub fn install(&self) -> Result<DesktopCliStatus> {
        if self.status().conflict {
            return Err(RuaError::Conflict {
                resource: "command-line tool".into(),
                detail: format!(
                    "{} already exists and is not owned by Rua",
                    self.destination.display()
                ),
            });
        }
        #[cfg(target_os = "macos")]
        install_runtime_link(&self.source, &self.destination)?;
        #[cfg(not(target_os = "macos"))]
        {
            let bytes = fs::read(&self.source)?;
            atomic_write(&self.destination, &bytes, true)?;
        }
        let mut marker = fs::read(&self.marker)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<InstallMarker>(&bytes).ok())
            .unwrap_or_else(|| InstallMarker {
                version: 1,
                destination: self.destination.clone(),
                extra: BTreeMap::new(),
            });
        marker.version = 1;
        marker.destination = self.destination.clone();
        let bytes = serde_json::to_vec_pretty(&marker).map_err(|error| RuaError::Config {
            message: error.to_string(),
        })?;
        atomic_write(&self.marker, &bytes, false)?;
        Ok(self.status())
    }
}

/// A copied executable loses its bundled drivers. Keep the CLI attached to the
/// installed Runtime, so its canonical path and macOS launch identity survive.
#[cfg(target_os = "macos")]
fn install_runtime_link(source: &Path, destination: &Path) -> Result<()> {
    let source = source.canonicalize()?;
    let parent = destination.parent().ok_or_else(|| RuaError::Config {
        message: "Install destination has no parent".into(),
    })?;
    fs::create_dir_all(parent)?;
    let actual_destination = parent.canonicalize()?.join("rua");
    if source == actual_destination {
        return Ok(());
    }
    let temp = parent.join(format!(".rua-install-{}", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        std::os::unix::fs::symlink(source, &temp)?;
        fs::File::open(parent)?.sync_all()?;
        fs::rename(&temp, destination)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(Into::into)
}

fn atomic_write(path: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    let parent = path.parent().ok_or_else(|| RuaError::Config {
        message: "Install destination has no parent".into(),
    })?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".rua-install-{}", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(if executable {
                0o755
            } else {
                0o600
            }))?;
        }
        #[cfg(not(unix))]
        let _ = executable;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(Into::into)
}
