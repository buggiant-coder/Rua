use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::device_list::DeviceList;
use crate::network_migration_adapter::NetworkMigrationPayload;
use crate::SyncResult;

#[derive(Debug, Clone)]
pub struct SyncControlStore {
    sync_dir: PathBuf,
}

impl SyncControlStore {
    pub fn from_data_dir(data_dir: impl AsRef<Path>) -> Self {
        Self {
            sync_dir: data_dir.as_ref().join("sync"),
        }
    }

    pub fn from_sync_dir(sync_dir: impl AsRef<Path>) -> Self {
        Self {
            sync_dir: sync_dir.as_ref().to_path_buf(),
        }
    }

    pub fn enabled_path(&self) -> PathBuf {
        self.sync_dir.join("enabled")
    }

    pub fn read_enabled(&self) -> bool {
        std::fs::read_to_string(self.enabled_path())
            .map(|content| content.trim() == "true")
            .unwrap_or(false)
    }

    pub fn write_enabled(&self, enabled: bool) -> SyncResult<()> {
        atomic_write(
            self.enabled_path(),
            if enabled { b"true" } else { b"false" },
        )?;
        Ok(())
    }

    pub fn device_list_path(&self) -> PathBuf {
        self.sync_dir.join("device_list.json")
    }

    pub fn read_device_list(&self) -> SyncResult<DeviceList> {
        read_device_list_file(&self.device_list_path())
    }

    pub fn write_device_list(&self, device_list: &DeviceList) -> SyncResult<()> {
        write_json_file(self.device_list_path(), device_list)
    }

    pub fn remove_device_list(&self) -> SyncResult<()> {
        remove_file_if_exists(&self.device_list_path())
    }

    pub fn pending_migration_path(&self) -> PathBuf {
        self.sync_dir.join("pending_migration.json")
    }

    pub fn write_pending_migration(&self, payload: &NetworkMigrationPayload) -> SyncResult<()> {
        let migration_data = serde_json::json!({
            "target_namespace_id": payload.target_namespace_id,
            "target_namespace_secret_bytes": payload.target_namespace_secret_bytes,
            "migrated_at_ms": payload.migrated_at_ms,
            "detected_at_ms": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        });
        write_json_file(self.pending_migration_path(), &migration_data)
    }

    pub fn remove_pending_migration(&self) -> SyncResult<()> {
        remove_file_if_exists(&self.pending_migration_path())
    }
}

pub fn read_device_list_file(path: &Path) -> SyncResult<DeviceList> {
    if !path.exists() {
        return Ok(DeviceList::default());
    }
    let content = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

pub fn write_device_list_file(path: impl AsRef<Path>, device_list: &DeviceList) -> SyncResult<()> {
    write_json_file(path, device_list)
}

pub fn write_json_file(path: impl AsRef<Path>, value: &impl Serialize) -> SyncResult<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_write(path, &bytes)?;
    Ok(())
}

pub fn atomic_write(path: impl AsRef<Path>, bytes: &[u8]) -> std::io::Result<()> {
    let path = path.as_ref();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp_path = tmp_path_for(path);
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp_path, path)?;
    sync_parent_dir(parent)?;
    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir(parent: &Path) -> std::io::Result<()> {
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_dir(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

pub fn remove_file_if_exists(path: &Path) -> SyncResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sync-control");
    path.with_file_name(format!(
        ".{file_name}.tmp.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ))
}
