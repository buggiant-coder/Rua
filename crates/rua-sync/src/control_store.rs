use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

use crate::device_list::{DeviceList, DeviceNameRecord};
use crate::hlc::HlcTimestamp;
use crate::network_migration_adapter::NetworkMigrationPayload;
use crate::{SyncError, SyncResult};

static DEVICE_LIST_WRITE_LOCK: Mutex<()> = Mutex::new(());

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
        write_device_list_file(self.device_list_path(), device_list)
    }

    /// Commit the projection and its revision together, even without a bound network.
    pub fn commit_device_name(
        &self,
        device_id: &[u8; 8],
        friendly_name: &str,
        author: [u8; 8],
    ) -> SyncResult<Option<DeviceNameRecord>> {
        update_device_list_file(&self.device_list_path(), |list| {
            if !list.contains_device(device_id) {
                return Ok((None, false));
            }
            let id = hex::encode(device_id);
            let mut record = list.name_records.get(&id).cloned().unwrap_or_default();
            let previous = HlcTimestamp::from_sortable_bytes(&record.revision);
            let wall_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let (wall_ms, counter) = if wall_ms > previous.wall_ms {
                (wall_ms, 0)
            } else if let Some(counter) = previous.counter.checked_add(1) {
                (previous.wall_ms, counter)
            } else {
                (
                    previous
                        .wall_ms
                        .checked_add(1)
                        .ok_or_else(|| SyncError::Store("device name revision exhausted".into()))?,
                    0,
                )
            };
            record.friendly_name = friendly_name.to_owned();
            record.revision = HlcTimestamp {
                wall_ms,
                counter,
                device_id: author,
            }
            .to_sortable_bytes();
            list.rename_device(device_id, friendly_name);
            list.name_records.insert(id, record.clone());
            Ok((Some(record), true))
        })
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
    update_device_list_file(path, |list| Ok((list.clone(), false)))
}

fn read_device_list_file_raw(path: &Path) -> SyncResult<DeviceList> {
    if !path.exists() {
        return Ok(DeviceList::default());
    }
    let content = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

pub fn write_device_list_file(path: impl AsRef<Path>, device_list: &DeviceList) -> SyncResult<()> {
    let _guard = DEVICE_LIST_WRITE_LOCK
        .lock()
        .map_err(|_| SyncError::Store("device list write lock poisoned".into()))?;
    let mut device_list = device_list.clone();
    device_list.migrate_saved_name_records();
    write_json_file(path, &device_list)
}

pub(crate) fn update_device_list_file<T>(
    path: &Path,
    update: impl FnOnce(&mut DeviceList) -> SyncResult<(T, bool)>,
) -> SyncResult<T> {
    let _guard = DEVICE_LIST_WRITE_LOCK
        .lock()
        .map_err(|_| SyncError::Store("device list write lock poisoned".into()))?;
    let mut list = read_device_list_file_raw(path)?;
    let migrated = list.migrate_saved_name_records();
    let (result, changed) = update(&mut list)?;
    if changed || migrated {
        write_json_file(path, &list)?;
    }
    Ok(result)
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
