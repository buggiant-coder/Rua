use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use rua_core::runtime_layout::RuntimeLayout;
use rua_core::tool::{ToolContext, ToolResult};
use rua_core::RuaError;
use serde_json::Value;

use super::ReadFileTool;
use crate::fs::WorkspaceFs;

/// A single local file. Policy can inspect its names before handing the same handle to the reader.
#[derive(Debug)]
pub struct OpenedRegularFile {
    public_path: PathBuf,
    resolved_public_path: PathBuf,
    pub(super) backing_path: PathBuf,
    pub(super) file: tokio::fs::File,
}

impl OpenedRegularFile {
    pub fn public_path(&self) -> &Path {
        &self.public_path
    }

    pub fn resolved_public_path(&self) -> &Path {
        &self.resolved_public_path
    }

    pub fn backing_path(&self) -> &Path {
        &self.backing_path
    }
}

impl ReadFileTool {
    /// Open a locally selected path so target policy can inspect the file's resolved names.
    pub async fn open_regular_file(
        &self,
        public_path: &Path,
        working_dir: &Path,
    ) -> rua_core::Result<OpenedRegularFile> {
        let path = public_path.to_str().ok_or_else(|| RuaError::Validation {
            message: "Expected a UTF-8 file path".into(),
        })?;
        if !public_path.is_absolute() || path.starts_with("//") || path.starts_with(r"\\") {
            return Err(RuaError::Validation {
                message: "Expected an absolute local file path".into(),
            });
        }
        let public_path = public_path.to_owned();
        let working_dir = working_dir.to_owned();
        tokio::task::spawn_blocking(move || {
            let layout = RuntimeLayout::from_current_process(None);
            let workspace = WorkspaceFs::for_execution(&working_dir, layout.clone())?;
            let resolved = workspace.resolve(&public_path)?;
            #[cfg(windows)]
            let drive_path = local_drive_path(&resolved)?;
            // Skip known devices and FIFOs before opening. Check the actual handle below.
            if !std::fs::metadata(&resolved)?.is_file() {
                return Err(RuaError::Validation {
                    message: "Expected a regular file".into(),
                });
            }
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_NOCTTY);
            }
            let file = options.open(&resolved)?;
            if !file.metadata()?.is_file() {
                return Err(RuaError::Validation {
                    message: "Expected a regular file".into(),
                });
            }
            let backing_path = opened_path(&file)?;
            if backing_path.as_os_str() != resolved.as_os_str() {
                return Err(RuaError::Security {
                    message: "The selected file changed while opening it".into(),
                });
            }
            let resolved_public_path = if let Some(guest) = &layout.linux_compat {
                guest
                    .map_host_path(&backing_path)
                    .ok_or_else(|| RuaError::Security {
                        message: "Opened file has no execution-environment path".into(),
                    })?
            } else {
                #[cfg(windows)]
                {
                    drive_path.to_owned()
                }
                #[cfg(not(windows))]
                backing_path.clone()
            };
            WorkspaceFs::check_secret_dir(&resolved_public_path)?;
            Ok(OpenedRegularFile {
                public_path,
                resolved_public_path,
                backing_path,
                file: tokio::fs::File::from_std(file),
            })
        })
        .await
        .map_err(|e| RuaError::Runtime {
            message: format!("File open task failed: {e}"),
        })?
    }

    /// Consume the checked file handle and apply the requested read window.
    pub async fn read_opened_file(
        &self,
        file: OpenedRegularFile,
        args: Value,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let device = ctx
            .local_device_id
            .as_deref()
            .filter(|id| !id.trim().is_empty());
        if device.is_none()
            || args["device_id"].as_str() != device
            || args["file_path"].as_str() != file.public_path().to_str()
        {
            return Err(RuaError::Validation {
                message: "Read arguments must identify the selected file and local device".into(),
            });
        }
        crate::device_file::read_opened_file(args, ctx, file).await
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn opened_path(file: &File) -> io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

#[cfg(target_vendor = "apple")]
fn opened_path(file: &File) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Ok(std::ffi::OsString::from_vec(rustix::fs::getpath(file)?.into_bytes()).into())
}

#[cfg(windows)]
fn local_drive_path(path: &Path) -> rua_core::Result<&Path> {
    // The kernel reports a verbatim name. Policy also needs the drive path behind aliases.
    path.to_str()
        .and_then(|path| path.strip_prefix(r"\\?\"))
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| RuaError::Security {
            message: "Selected file has no local drive path".into(),
        })
}

#[cfg(windows)]
fn opened_path(file: &File) -> io::Result<PathBuf> {
    winx::file::get_file_path(file)
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    windows
)))]
fn opened_path(_: &File) -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Cannot verify the path of an open file on this platform",
    ))
}
