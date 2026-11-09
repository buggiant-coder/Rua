use std::io::Cursor;
use std::path::{Path, PathBuf};

use futures::StreamExt;

use crate::manifest::{PluginManifest, PluginManifestError};

#[derive(Debug)]
pub struct Plugin {
    pub id: String,
    pub manifest: PluginManifest,
    pub path: PathBuf,
}

const DOWNLOAD_MAX_BYTES: u64 = 50 * 1024 * 1024;
const DOWNLOAD_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, thiserror::Error)]
pub enum PluginLoadError {
    #[error("manifest error: {0}")]
    Manifest(#[from] PluginManifestError),
    #[error("download failed for {url}: {detail}")]
    DownloadFailed { url: String, detail: String },
    #[error("download too large for {url}: {size} bytes (max {max})")]
    DownloadTooLarge { url: String, size: u64, max: u64 },
    #[error("zip extraction failed: {detail}")]
    ZipError { detail: String },
    #[error("zip-slip: entry '{entry}' escapes target directory")]
    ZipSlip { entry: String },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct PluginLoader;

impl PluginLoader {
    pub fn load(source: &Path) -> Result<Plugin, PluginLoadError> {
        let manifest_path = source.join("plugin.json");
        let manifest = PluginManifest::from_file(&manifest_path)?;
        let id = manifest.id.clone();
        Ok(Plugin {
            id,
            manifest,
            path: source.to_path_buf(),
        })
    }

    pub async fn from_url(url: &str, target_dir: &Path) -> Result<Plugin, PluginLoadError> {
        let data = Self::download(url).await?;

        if data.len() >= 4 && data[..4] == [0x50, 0x4B, 0x03, 0x04] {
            safe_extract_zip(&data, target_dir)?;
        } else {
            std::fs::create_dir_all(target_dir)?;
            std::fs::write(target_dir.join("plugin.tar.gz"), &data)?;
        }

        Self::load(target_dir)
    }

    async fn download(url: &str) -> Result<Vec<u8>, PluginLoadError> {
        let client = rua_http::build_client(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS)),
        )
        .map_err(|e| PluginLoadError::DownloadFailed {
            url: url.to_string(),
            detail: e.to_string(),
        })?;

        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| PluginLoadError::DownloadFailed {
                url: url.to_string(),
                detail: if e.is_timeout() {
                    format!("download timed out ({DOWNLOAD_TIMEOUT_SECS}s limit)")
                } else {
                    e.to_string()
                },
            })?;

        if !resp.status().is_success() {
            return Err(PluginLoadError::DownloadFailed {
                url: url.to_string(),
                detail: format!("HTTP {}", resp.status()),
            });
        }

        let header_len = resp
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());

        if let Some(len) = header_len.or_else(|| resp.content_length()) {
            if len > DOWNLOAD_MAX_BYTES {
                return Err(PluginLoadError::DownloadTooLarge {
                    url: url.to_string(),
                    size: len,
                    max: DOWNLOAD_MAX_BYTES,
                });
            }
        }

        let mut stream = resp.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| PluginLoadError::DownloadFailed {
                url: url.to_string(),
                detail: if e.is_timeout() {
                    format!("download timed out ({DOWNLOAD_TIMEOUT_SECS}s limit)")
                } else {
                    e.to_string()
                },
            })?;
            body.extend_from_slice(&chunk);
            if body.len() as u64 > DOWNLOAD_MAX_BYTES {
                return Err(PluginLoadError::DownloadTooLarge {
                    url: url.to_string(),
                    size: body.len() as u64,
                    max: DOWNLOAD_MAX_BYTES,
                });
            }
        }

        Ok(body)
    }
}

pub fn safe_extract_zip(data: &[u8], extract_dir: &Path) -> Result<(), PluginLoadError> {
    std::fs::create_dir_all(extract_dir)?;
    let canonical_extract_dir = std::fs::canonicalize(extract_dir)?;

    let reader = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| PluginLoadError::ZipError {
        detail: e.to_string(),
    })?;

    let mut created_paths: Vec<PathBuf> = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| PluginLoadError::ZipError {
            detail: e.to_string(),
        })?;

        let safe_name = match entry.enclosed_name() {
            Some(name) => name.to_path_buf(),
            None => {
                cleanup_created(&created_paths);
                return Err(PluginLoadError::ZipSlip {
                    entry: entry.name().to_string(),
                });
            }
        };

        let target = canonical_extract_dir.join(&safe_name);

        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            created_paths.push(target);
            continue;
        }

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut outfile = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut outfile)?;
        created_paths.push(target.clone());

        let canonical_target = std::fs::canonicalize(&target)?;
        if !canonical_target.starts_with(&canonical_extract_dir) {
            cleanup_created(&created_paths);
            return Err(PluginLoadError::ZipSlip {
                entry: entry.name().to_string(),
            });
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))?;
            }
        }
    }

    Ok(())
}

fn cleanup_created(paths: &[PathBuf]) {
    for path in paths.iter().rev() {
        if path.is_dir() {
            let _ = std::fs::remove_dir(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}
