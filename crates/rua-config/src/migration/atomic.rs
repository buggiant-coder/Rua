use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
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
