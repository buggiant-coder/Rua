use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fs2::FileExt;

static LOCK_WARNED: OnceLock<()> = OnceLock::new();

pub struct ManifestLock {
    file: Option<File>,
}

impl ManifestLock {
    pub fn acquire(lock_path: &Path) -> io::Result<Self> {
        if let Some(parent) = lock_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;

        match FileExt::lock_exclusive(&file) {
            Ok(()) => Ok(Self { file: Some(file) }),
            Err(err) => {
                LOCK_WARNED.get_or_init(|| {
                    tracing::debug!(
                        "flock unavailable on {} ({}), falling back to lockless mode",
                        lock_path.display(),
                        err
                    );
                });
                Ok(Self { file: None })
            }
        }
    }

    pub fn is_locked(&self) -> bool {
        self.file.is_some()
    }
}

impl Drop for ManifestLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = FileExt::unlock(&file);
        }
    }
}

pub fn lock_path_for(json_path: &Path) -> PathBuf {
    let parent = json_path.parent().unwrap_or_else(|| Path::new("."));
    let name = json_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("manifest.json");
    parent.join(format!(".{name}.lock"))
}
