use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Result, RuaError};

pub const SEED_MANIFEST_REL_PATH: &str = "sync/seed-manifest.json";
pub const SEED_MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SeedManifest {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub seeded_at: String,
    #[serde(default)]
    pub entries: BTreeMap<String, SeedManifestEntry>,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SeedManifestEntry {
    pub producer: String,
    pub materialized_at: String,
    #[serde(default)]
    pub paths: Vec<String>,
}

pub fn manifest_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SEED_MANIFEST_REL_PATH)
}

pub fn read_manifest(data_dir: &Path) -> Result<Option<SeedManifest>> {
    let path = manifest_path(data_dir);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let manifest = serde_json::from_slice(&bytes).map_err(|e| RuaError::Config {
                message: format!("Failed to parse {}: {e}", path.display()),
            })?;
            Ok(Some(manifest))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(RuaError::Io {
            message: format!("Failed to read {}: {e}", path.display()),
        }),
    }
}

pub fn record_seed_entry(
    data_dir: &Path,
    entry_id: &str,
    producer: &str,
    paths: &[&str],
) -> Result<SeedManifest> {
    if entry_id.is_empty() {
        return Err(RuaError::Validation {
            message: "seed entry id must not be empty".to_string(),
        });
    }
    if producer.is_empty() {
        return Err(RuaError::Validation {
            message: "seed producer must not be empty".to_string(),
        });
    }

    let mut normalized_paths = Vec::with_capacity(paths.len());
    let mut file_hashes = BTreeMap::new();
    for path in paths {
        let normalized = normalize_seed_relative_path(path)?;
        let content = std::fs::read(data_dir.join(&normalized)).map_err(|e| RuaError::Io {
            message: format!("Failed to read seeded file {normalized}: {e}"),
        })?;
        file_hashes.insert(normalized.clone(), sha256_hex(&content));
        normalized_paths.push(normalized);
    }
    normalized_paths.sort();
    normalized_paths.dedup();

    let now = chrono::Utc::now().to_rfc3339();
    let mut manifest = read_manifest(data_dir)?.unwrap_or_else(|| SeedManifest {
        version: SEED_MANIFEST_VERSION,
        ..Default::default()
    });
    manifest.version = SEED_MANIFEST_VERSION;
    manifest.seeded_at = now.clone();
    manifest
        .entries
        .entry(entry_id.to_string())
        .and_modify(|entry| {
            entry.producer = producer.to_string();
            entry.paths.extend(normalized_paths.iter().cloned());
            entry.paths.sort();
            entry.paths.dedup();
        })
        .or_insert_with(|| SeedManifestEntry {
            producer: producer.to_string(),
            materialized_at: now,
            paths: normalized_paths,
        });
    for (path, hash) in file_hashes {
        manifest.files.insert(path, hash);
    }

    write_manifest(data_dir, &manifest)?;
    Ok(manifest)
}

pub fn is_entry_materialized(data_dir: &Path, entry_id: &str) -> Result<bool> {
    Ok(read_manifest(data_dir)?
        .map(|manifest| manifest.entries.contains_key(entry_id))
        .unwrap_or(false))
}

pub fn is_file_untouched_seed(data_dir: &Path, relative_path: &str) -> Result<bool> {
    let Some(manifest) = read_manifest(data_dir)? else {
        return Ok(false);
    };
    let relative_path = normalize_seed_relative_path(relative_path)?;
    is_file_untouched_seed_in_manifest(data_dir, &manifest, &relative_path)
}

pub fn is_entity_untouched_seed(data_dir: &Path, relative_dir: &str) -> Result<bool> {
    let Some(manifest) = read_manifest(data_dir)? else {
        return Ok(false);
    };
    let relative_dir = normalize_seed_relative_path(relative_dir)?;
    let root = data_dir.join(&relative_dir);
    if !root.is_dir() {
        return Ok(false);
    }

    let mut files = Vec::new();
    collect_files(&root, &mut files).map_err(|e| RuaError::Io {
        message: format!(
            "Failed to collect seed entity files {}: {e}",
            root.display()
        ),
    })?;
    if files.is_empty() {
        return Ok(false);
    }

    for file in files {
        let rel = file
            .strip_prefix(data_dir)
            .map_err(|e| RuaError::Validation {
                message: format!("Failed to relativize seed entity path: {e}"),
            })?
            .to_string_lossy()
            .replace('\\', "/");
        if !is_file_untouched_seed_in_manifest(data_dir, &manifest, &rel)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn normalize_seed_relative_path(path: &str) -> Result<String> {
    let path = path.replace('\\', "/");
    let rel = Path::new(&path);
    if rel.as_os_str().is_empty() || rel.is_absolute() {
        return Err(invalid_seed_path(&path));
    }
    let mut parts = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(part) => {
                let Some(part) = part.to_str() else {
                    return Err(invalid_seed_path(&path));
                };
                if part.is_empty() {
                    return Err(invalid_seed_path(&path));
                }
                parts.push(part.to_string());
            }
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(invalid_seed_path(&path));
            }
        }
    }
    if parts.is_empty() {
        return Err(invalid_seed_path(&path));
    }
    Ok(parts.join("/"))
}

fn is_file_untouched_seed_in_manifest(
    data_dir: &Path,
    manifest: &SeedManifest,
    relative_path: &str,
) -> Result<bool> {
    let Some(expected) = manifest.files.get(relative_path) else {
        return Ok(false);
    };
    let path = data_dir.join(relative_path);
    let content = match std::fs::read(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(RuaError::Io {
                message: format!("Failed to read {}: {e}", path.display()),
            })
        }
    };
    Ok(sha256_hex(&content) == *expected)
}

fn collect_files(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    out.sort();
    Ok(())
}

fn write_manifest(data_dir: &Path, manifest: &SeedManifest) -> Result<()> {
    let path = manifest_path(data_dir);
    let parent = path.parent().ok_or_else(|| RuaError::Validation {
        message: format!("seed manifest path has no parent: {}", path.display()),
    })?;
    std::fs::create_dir_all(parent).map_err(|e| RuaError::Io {
        message: format!("Failed to create {}: {e}", parent.display()),
    })?;

    let bytes = serde_json::to_vec_pretty(manifest).map_err(|e| RuaError::Config {
        message: format!("Failed to serialize seed manifest: {e}"),
    })?;
    atomic_write_bytes(&path, &bytes)
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| RuaError::Validation {
        message: format!("atomic write path has no parent: {}", path.display()),
    })?;
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("json")
    ));
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| RuaError::Io {
            message: format!("Failed to create {}: {e}", tmp.display()),
        })?;
        use std::io::Write;
        file.write_all(bytes).map_err(|e| RuaError::Io {
            message: format!("Failed to write {}: {e}", tmp.display()),
        })?;
        file.sync_all().map_err(|e| RuaError::Io {
            message: format!("Failed to fsync {}: {e}", tmp.display()),
        })?;
    }
    std::fs::rename(&tmp, path).map_err(|e| RuaError::Io {
        message: format!(
            "Failed to rename {} to {}: {e}",
            tmp.display(),
            path.display()
        ),
    })?;
    fsync_dir(parent)
}

fn fsync_dir(path: &Path) -> Result<()> {
    crate::fs::sync_directory(path).map_err(|e| RuaError::Io {
        message: format!("Failed to fsync directory {}: {e}", path.display()),
    })
}

fn invalid_seed_path(path: &str) -> RuaError {
    RuaError::Validation {
        message: format!("invalid seed relative path: {path}"),
    }
}

fn default_version() -> u32 {
    SEED_MANIFEST_VERSION
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
}
