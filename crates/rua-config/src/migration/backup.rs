use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

pub fn create_backup(file_path: &Path, base_dir: &Path) -> std::io::Result<PathBuf> {
    if !file_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Cannot backup: file not found: {}", file_path.display()),
        ));
    }

    let real_file = file_path.canonicalize()?;
    let real_base = base_dir
        .canonicalize()
        .unwrap_or_else(|_| base_dir.to_path_buf());
    if !real_file.starts_with(&real_base) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "Backup refused: path {} escapes base directory {}",
                file_path.display(),
                base_dir.display()
            ),
        ));
    }

    let timestamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let backup_dir = base_dir.join(".backup").join(&timestamp);
    fs::create_dir_all(&backup_dir)?;

    let relative = file_path
        .strip_prefix(base_dir)
        .unwrap_or(file_path.file_name().map(Path::new).unwrap_or(file_path));

    let backup_path = backup_dir.join(relative);
    if let Some(parent) = backup_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::copy(file_path, &backup_path)?;

    Ok(backup_path)
}

pub fn rollback(backup_path: &Path, target_path: &Path) -> std::io::Result<()> {
    if !backup_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Backup not found: {}", backup_path.display()),
        ));
    }

    let data = fs::read(backup_path)?;
    super::atomic::atomic_write(target_path, &data)
}

pub fn list_backups(base_dir: &Path) -> std::io::Result<Vec<String>> {
    let backup_root = base_dir.join(".backup");
    if !backup_root.exists() {
        return Ok(Vec::new());
    }

    let mut timestamps: Vec<String> = fs::read_dir(&backup_root)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| entry.file_name().to_str().map(String::from))
        .collect();

    timestamps.sort_by(|a, b| b.cmp(a));
    Ok(timestamps)
}
