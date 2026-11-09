use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use tempfile::NamedTempFile;

use crate::error::SkillError;

pub fn write_json_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), SkillError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if !dir.as_os_str().is_empty() {
        std::fs::create_dir_all(dir)?;
    }

    let tmp = NamedTempFile::new_in(dir)?;
    {
        let mut handle = tmp.as_file();
        serde_json::to_writer_pretty(&mut handle, value)?;

        use std::io::Write;
        handle.write_all(b"\n")?;
        handle.flush()?;
    }

    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub fn next_manifest_version(prev_version: u64) -> u64 {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    prev_version.saturating_add(1).max(now_ms)
}

pub fn bump_manifest_version(payload: &mut serde_json::Map<String, Value>) {
    let prev = payload.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
    let next = next_manifest_version(prev);
    payload.insert("version".to_string(), Value::Number(next.into()));
}

pub fn write_manifest_atomic(
    path: &Path,
    payload: &mut serde_json::Map<String, Value>,
) -> Result<(), SkillError> {
    bump_manifest_version(payload);
    write_json_atomic(path, payload)
}
