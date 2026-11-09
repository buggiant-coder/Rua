use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use sysinfo::Disks;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::download_ipc::{IpcMessage, ProgressUpdate};

const SAFETY_MARGIN: u64 = 100 * 1024 * 1024;

pub fn resume_offset(path: &Path) -> u64 {
    match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

pub fn compute_sha256(path: &Path) -> rua_core::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn check_disk_space(path: &Path, required_bytes: u64) -> rua_core::Result<()> {
    let check_path = if path.exists() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(Path::new("/")).to_path_buf()
    };
    let target = check_path.canonicalize().unwrap_or(check_path);
    let disks = Disks::new_with_refreshed_list();

    let disk = disks
        .iter()
        .filter(|d| target.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len());

    let available = match disk {
        Some(d) => d.available_space(),
        None => {
            return Err(rua_core::RuaError::Io {
                message: format!("cannot determine disk for path: {}", path.display()),
            });
        }
    };

    let needed = required_bytes.saturating_add(SAFETY_MARGIN);
    if available < needed {
        return Err(rua_core::RuaError::Validation {
            message: format!(
                "insufficient disk space: need {} bytes (including 100MB safety margin), available {} bytes",
                needed, available,
            ),
        });
    }
    Ok(())
}

pub async fn resume_download(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    id: &str,
    progress_tx: &Option<mpsc::Sender<IpcMessage>>,
) -> rua_core::Result<()> {
    let local_size = resume_offset(dest);

    let mut req = client
        .get(url)
        .header("User-Agent", "rua")
        .header("Accept", "*/*");

    if local_size > 0 {
        req = req.header("Range", format!("bytes={local_size}-"));
    }

    let resp = req.send().await.map_err(|e| rua_core::RuaError::Io {
        message: format!("download request failed: {e}"),
    })?;

    let status = resp.status();
    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(rua_core::RuaError::Io {
            message: format!("download failed: HTTP {status}"),
        });
    }

    let (mut downloaded, open_opts_append) = if status == reqwest::StatusCode::PARTIAL_CONTENT {
        (local_size, true)
    } else {
        (0u64, false)
    };

    let total = resp.content_length().map(|cl| cl + downloaded).unwrap_or(0);

    let mut file = if open_opts_append {
        tokio::fs::OpenOptions::new().append(true).open(dest).await
    } else {
        tokio::fs::File::create(dest).await
    }
    .map_err(|e| rua_core::RuaError::Io {
        message: format!("failed to open file: {e}"),
    })?;

    let mut resp = resp;
    while let Some(chunk) = resp.chunk().await.map_err(|e| rua_core::RuaError::Io {
        message: format!("download stream error: {e}"),
    })? {
        file.write_all(&chunk)
            .await
            .map_err(|e| rua_core::RuaError::Io {
                message: format!("write error: {e}"),
            })?;
        downloaded += chunk.len() as u64;

        if let Some(ref tx) = progress_tx {
            let _ = tx
                .send(IpcMessage::Progress(ProgressUpdate {
                    id: id.to_string(),
                    bytes_downloaded: downloaded,
                    bytes_total: total,
                    eta_secs: None,
                }))
                .await;
        }
    }

    file.flush().await.map_err(|e| rua_core::RuaError::Io {
        message: format!("flush error: {e}"),
    })?;
    drop(file);

    if let Some(expected) = expected_sha256 {
        let dest_owned = dest.to_path_buf();
        let actual = tokio::task::spawn_blocking(move || compute_sha256(&dest_owned))
            .await
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("sha256 task panicked: {e}"),
            })??;

        if actual != expected {
            let _ = tokio::fs::remove_file(dest).await;
            return Err(rua_core::RuaError::Validation {
                message: format!("SHA256 mismatch: expected {expected}, got {actual}"),
            });
        }
    }

    Ok(())
}
