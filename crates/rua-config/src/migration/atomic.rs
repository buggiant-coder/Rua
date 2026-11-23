use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Runs a short synchronous commit while excluding writes/deletes to these paths.
/// The closure must not await or write one of the locked paths recursively. It may
/// atomically write a different journal path. Directory aliases share one lock.
pub fn with_commit_barrier<T>(
    paths: &[PathBuf],
    commit: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<T> {
    static LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let mut paths = paths
        .iter()
        .map(|path| commit_path(path))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    paths.dedup();
    let locks = {
        let mut registry = LOCKS
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| std::io::Error::other("file commit registry is poisoned"))?;
        registry.retain(|_, entry| entry.strong_count() > 0);
        paths
            .into_iter()
            .map(|path| {
                if let Some(lock) = registry.get(&path).and_then(Weak::upgrade) {
                    lock
                } else {
                    let lock = Arc::new(Mutex::new(()));
                    registry.insert(path, Arc::downgrade(&lock));
                    lock
                }
            })
            .collect::<Vec<_>>()
    };
    let _guards = locks
        .iter()
        .map(|lock| {
            lock.lock()
                .map_err(|_| std::io::Error::other("file commit lock is poisoned"))
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    commit()
}

// Atomic rename acts on the directory entry, not the target of a leaf symlink.
// Canonicalize the existing parent, retaining missing descendants for first writes.
fn commit_path(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let name = absolute.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "file commit needs a file name",
        )
    })?;
    let mut parent = absolute.parent().unwrap_or(Path::new("/"));
    let mut missing = Vec::new();
    let mut resolved = loop {
        match fs::canonicalize(parent) {
            Ok(path) => break path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(parent.components().next_back().ok_or(error)?);
                parent = parent.parent().ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::NotFound, "no existing file ancestor")
                })?;
            }
            Err(error) => return Err(error),
        }
    };
    for component in missing.into_iter().rev() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => resolved.push(other.as_os_str()),
        }
    }
    resolved.push(name);
    Ok(resolved)
}

/// Removes the file and durably commits the directory change under the same lock
/// as atomic writes. NotFound is preserved for the caller's existing policy.
pub fn remove_file_committed(path: &Path) -> std::io::Result<()> {
    with_commit_barrier(&[path.to_path_buf()], || {
        fs::remove_file(path)?;
        sync_parent_dir(path.parent().unwrap_or(Path::new(".")))
    })
}

pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    with_commit_barrier(&[path.to_path_buf()], || atomic_write_locked(path, data))
}

/// Initializes a missing file without overwriting a concurrent committed value.
pub fn atomic_write_if_absent(path: &Path, data: &[u8]) -> std::io::Result<bool> {
    with_commit_barrier(&[path.to_path_buf()], || {
        match fs::symlink_metadata(path) {
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        atomic_write_locked(path, data)?;
        Ok(true)
    })
}

fn atomic_write_locked(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));

    fs::create_dir_all(parent)?;

    let temp_name = format!(
        ".{}.tmp.{}.{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let temp_path = parent.join(&temp_name);

    {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(data)?;
        file.sync_all()?;
    }

    fs::rename(&temp_path, path)?;

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

pub fn atomic_write_json(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let data = serde_json::to_vec_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    atomic_write(path, &data)
}
