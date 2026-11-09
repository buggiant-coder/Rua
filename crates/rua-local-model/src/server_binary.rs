use std::path::{Path, PathBuf};

use rua_core::{Result, RuaError};
use tokio::sync::mpsc;

use crate::cuda::{CudaInfo, DetectedArch, DetectedOs};
use crate::download_ipc::{DownloadStatus, ErrorUpdate, IpcMessage, StatusUpdate};
use crate::robustness;

const GITHUB_RELEASE_BASE: &str = "https://github.com/ggml-org/llama.cpp/releases/download";
const GITHUB_API_LATEST: &str = "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Backend {
    Cpu,
    Cuda { version: String },
}

fn normalize_cuda_version(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw.split('.').collect();
    let major = *parts.first()?;
    let minor: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    match major {
        "12" if minor >= 4 => Some("12.4".into()),
        "13" => Some("13.1".into()),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct ServerBinaryDownloader {
    os: DetectedOs,
    arch: DetectedArch,
    cuda: CudaInfo,
    target_dir: PathBuf,
    release_base_url: String,
    api_url: String,
}

impl ServerBinaryDownloader {
    pub fn new(
        os: DetectedOs,
        arch: DetectedArch,
        cuda: CudaInfo,
        target_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            os,
            arch,
            cuda,
            target_dir: target_dir.into(),
            release_base_url: GITHUB_RELEASE_BASE.into(),
            api_url: GITHUB_API_LATEST.into(),
        }
    }

    pub fn with_release_base_url(mut self, url: impl Into<String>) -> Self {
        self.release_base_url = url.into();
        self
    }

    pub fn with_api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = url.into();
        self
    }

    fn resolve_backend(&self) -> Backend {
        if self.os != DetectedOs::Windows || !self.cuda.available {
            return Backend::Cpu;
        }
        if let Some(ref ver) = self.cuda.version {
            if let Some(normalized) = normalize_cuda_version(ver) {
                return Backend::Cuda {
                    version: normalized,
                };
            }
        }
        Backend::Cpu
    }

    fn arch_label(&self) -> &'static str {
        match self.arch {
            DetectedArch::X86_64 => "x64",
            DetectedArch::Aarch64 => "arm64",
            DetectedArch::Unknown => "unknown",
        }
    }

    pub fn build_asset_filename(&self, tag: &str) -> Result<String> {
        let backend = self.resolve_backend();
        let arch = self.arch_label();
        match self.os {
            DetectedOs::MacOs => Ok(format!("llama-{tag}-bin-macos-{arch}.tar.gz")),
            DetectedOs::Linux => Ok(format!("llama-{tag}-bin-ubuntu-{arch}.tar.gz")),
            DetectedOs::Windows => match backend {
                Backend::Cuda { ref version } => {
                    if self.arch != DetectedArch::X86_64 {
                        return Err(RuaError::Validation {
                            message: "Windows CUDA package only supported for x64".into(),
                        });
                    }
                    Ok(format!("llama-{tag}-bin-win-cuda-{version}-{arch}.zip"))
                }
                Backend::Cpu => Ok(format!("llama-{tag}-bin-win-cpu-{arch}.zip")),
            },
            DetectedOs::Unknown => Err(RuaError::Validation {
                message: "unsupported operating system".into(),
            }),
        }
    }

    pub async fn fetch_latest_tag(&self, client: &reqwest::Client) -> Result<String> {
        let resp = client
            .get(&self.api_url)
            .header("User-Agent", "rua")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| RuaError::Io {
                message: format!("GitHub API request failed: {e}"),
            })?;

        if !resp.status().is_success() {
            return Err(RuaError::Io {
                message: format!("GitHub API returned HTTP {}", resp.status()),
            });
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| RuaError::Io {
            message: format!("failed to parse GitHub API response: {e}"),
        })?;

        body["tag_name"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| RuaError::Runtime {
                message: "GitHub API response missing tag_name".into(),
            })
    }

    pub async fn download(
        &self,
        progress_tx: Option<mpsc::Sender<IpcMessage>>,
        expected_sha256: Option<&str>,
    ) -> Result<PathBuf> {
        let id = "server-binary".to_string();
        let client = rua_http::default_client_or_default("rua-local-model");

        let tag = self.fetch_latest_tag(&client).await?;

        let filename = self.build_asset_filename(&tag)?;
        let url = format!("{}/{tag}/{filename}", self.release_base_url);

        send_status(&progress_tx, &id, DownloadStatus::Started, Some(&filename)).await;

        let server_dir = self.target_dir.join("server");
        tokio::fs::create_dir_all(&server_dir)
            .await
            .map_err(|e| RuaError::Io {
                message: format!("failed to create server dir: {e}"),
            })?;

        let staging_dir = self.target_dir.join(format!(".server-staging-{}", nonce()));
        tokio::fs::create_dir_all(&staging_dir)
            .await
            .map_err(|e| RuaError::Io {
                message: format!("failed to create staging dir: {e}"),
            })?;

        let archive_path = staging_dir.join(&filename);
        if let Err(e) = robustness::resume_download(
            &client,
            &url,
            &archive_path,
            expected_sha256,
            &id,
            &progress_tx,
        )
        .await
        {
            let error = e.to_string();
            send_error(
                &progress_tx,
                &id,
                &error,
                !is_terminal_package_status_error(&error),
            )
            .await;
            let _ = tokio::fs::remove_dir_all(&staging_dir).await;
            return Err(e);
        }

        if let Err(e) = extract_archive(&archive_path, &server_dir).await {
            send_error(&progress_tx, &id, &e.to_string(), false).await;
            let _ = tokio::fs::remove_dir_all(&staging_dir).await;
            return Err(e);
        }

        let binary_name = if self.os == DetectedOs::Windows {
            "llama-server.exe"
        } else {
            "llama-server"
        };
        let binary_path = server_dir.join(binary_name);
        chmod_executable(&binary_path).await?;

        let version_path = server_dir.join("version.txt");
        let _ = tokio::fs::write(&version_path, &tag).await;

        let _ = tokio::fs::remove_dir_all(&staging_dir).await;

        send_status(
            &progress_tx,
            &id,
            DownloadStatus::Completed,
            Some(&binary_path.display().to_string()),
        )
        .await;

        Ok(binary_path)
    }
}

async fn extract_archive(archive: &Path, dest_dir: &Path) -> Result<()> {
    let staging = dest_dir
        .parent()
        .unwrap_or(dest_dir)
        .join(format!(".extract-staging-{}", nonce()));
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|e| RuaError::Io {
            message: format!("failed to create extraction staging dir: {e}"),
        })?;

    let result = run_extract_command(archive, &staging).await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }

    let merge_result = merge_extracted(&staging, dest_dir).await;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    merge_result
}

async fn run_extract_command(archive: &Path, dest: &Path) -> Result<()> {
    let name = archive.file_name().and_then(|n| n.to_str()).unwrap_or("");

    let status = if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        tokio::process::Command::new("tar")
            .arg("xzf")
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()
            .await
    } else if name.ends_with(".zip") {
        if cfg!(target_os = "windows") {
            tokio::process::Command::new("tar")
                .arg("-xf")
                .arg(archive)
                .arg("-C")
                .arg(dest)
                .status()
                .await
        } else {
            tokio::process::Command::new("unzip")
                .arg("-o")
                .arg(archive)
                .arg("-d")
                .arg(dest)
                .status()
                .await
        }
    } else {
        return Err(RuaError::Validation {
            message: format!("unsupported archive format: {name}"),
        });
    };

    let status = status.map_err(|e| RuaError::Io {
        message: format!("extraction command failed: {e}"),
    })?;

    if !status.success() {
        return Err(RuaError::Runtime {
            message: format!("extraction failed with exit code {status}"),
        });
    }
    Ok(())
}

async fn merge_extracted(staging: &Path, dest_dir: &Path) -> Result<()> {
    let staging_owned = staging.to_path_buf();
    let dest_owned = dest_dir.to_path_buf();

    tokio::task::spawn_blocking(move || merge_extracted_sync(&staging_owned, &dest_owned))
        .await
        .map_err(|e| RuaError::Runtime {
            message: format!("merge task panicked: {e}"),
        })?
}

fn merge_extracted_sync(staging: &Path, dest: &Path) -> Result<()> {
    let entries: Vec<_> = std::fs::read_dir(staging)
        .map_err(|e| RuaError::Io {
            message: format!("failed to read staging dir: {e}"),
        })?
        .filter_map(|e| e.ok())
        .collect();

    let source_root = if entries.len() == 1
        && entries[0]
            .file_type()
            .map(|ft| ft.is_dir())
            .unwrap_or(false)
    {
        entries[0].path()
    } else {
        staging.to_path_buf()
    };

    for entry in std::fs::read_dir(&source_root).map_err(|e| RuaError::Io {
        message: format!("failed to read source root: {e}"),
    })? {
        let entry = entry.map_err(|e| RuaError::Io {
            message: format!("failed to read entry: {e}"),
        })?;
        let src = entry.path();
        let dst = dest.join(entry.file_name());
        merge_path(&src, &dst)?;
    }

    Ok(())
}

fn merge_path(src: &Path, dst: &Path) -> Result<()> {
    if src.is_symlink() {
        if dst.exists() || dst.is_symlink() {
            let _ = std::fs::remove_file(dst);
        }
        #[cfg(unix)]
        {
            let target = std::fs::read_link(src).map_err(|e| RuaError::Io {
                message: format!("failed to read symlink: {e}"),
            })?;
            std::os::unix::fs::symlink(&target, dst).map_err(|e| RuaError::Io {
                message: format!("failed to create symlink: {e}"),
            })?;
        }
        return Ok(());
    }

    if src.is_dir() {
        std::fs::create_dir_all(dst).map_err(|e| RuaError::Io {
            message: format!("failed to create dir {}: {e}", dst.display()),
        })?;
        for entry in std::fs::read_dir(src).map_err(|e| RuaError::Io {
            message: format!("failed to read dir {}: {e}", src.display()),
        })? {
            let entry = entry.map_err(|e| RuaError::Io {
                message: format!("failed to read entry: {e}"),
            })?;
            merge_path(&entry.path(), &dst.join(entry.file_name()))?;
        }
        return Ok(());
    }

    std::fs::copy(src, dst).map_err(|e| RuaError::Io {
        message: format!("failed to copy {} → {}: {e}", src.display(), dst.display()),
    })?;
    Ok(())
}

#[cfg(unix)]
async fn chmod_executable(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    use std::os::unix::fs::PermissionsExt;
    let meta = tokio::fs::metadata(path).await.map_err(|e| RuaError::Io {
        message: format!("failed to read metadata: {e}"),
    })?;
    let mut perms = meta.permissions();
    perms.set_mode(perms.mode() | 0o755);
    tokio::fs::set_permissions(path, perms)
        .await
        .map_err(|e| RuaError::Io {
            message: format!("chmod failed: {e}"),
        })?;
    Ok(())
}

#[cfg(not(unix))]
async fn chmod_executable(_path: &Path) -> Result<()> {
    Ok(())
}

async fn send_status(
    tx: &Option<mpsc::Sender<IpcMessage>>,
    id: &str,
    status: DownloadStatus,
    detail: Option<&str>,
) {
    if let Some(ref tx) = tx {
        let _ = tx
            .send(IpcMessage::Status(StatusUpdate {
                id: id.to_string(),
                status,
                detail: detail.map(|s| s.to_string()),
            }))
            .await;
    }
}

fn is_terminal_package_status_error(error: &str) -> bool {
    error.contains("HTTP 403") || error.contains("HTTP 404")
}

async fn send_error(tx: &Option<mpsc::Sender<IpcMessage>>, id: &str, error: &str, retryable: bool) {
    if let Some(ref tx) = tx {
        let _ = tx
            .send(IpcMessage::Error(ErrorUpdate {
                id: id.to_string(),
                error: error.to_string(),
                retryable: Some(retryable),
            }))
            .await;
    }
}

fn nonce() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{ns:032x}")
}
