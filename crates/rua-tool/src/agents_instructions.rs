use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};

use rua_core::agents_instruction_discovery::{
    AgentsInstructionDiscovery, AgentsInstructionDiscoveryRequest, AgentsInstructionSource,
    AgentsInstructionTargetKind, AGENTS_INSTRUCTIONS_MAX_BYTES,
};
use rua_core::{Result, RuaError};
use tokio_util::sync::CancellationToken;

use crate::WorkspaceFs;

/// Discover only the target's ancestor chain on the Device owning this filesystem.
/// Authorization is checked before opening and against the opened file before reading.
pub async fn discover_agents_instructions<F, Fut>(
    fs: &WorkspaceFs,
    request: &AgentsInstructionDiscoveryRequest,
    cancel: &CancellationToken,
    can_read: F,
) -> Result<AgentsInstructionDiscovery>
where
    F: Fn(PathBuf, PathBuf) -> Fut + Send + Sync,
    Fut: Future<Output = bool> + Send,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(RuaError::Cancelled),
        discovery = discover(fs, request, cancel, &can_read) => Ok(discovery),
    }
}

async fn resolve(fs: &WorkspaceFs, path: &Path, for_create: bool) -> Option<PathBuf> {
    let fs = fs.clone();
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        if for_create {
            fs.resolve_for_create(path)
        } else {
            fs.resolve(path)
        }
    })
    .await
    .ok()?
    .ok()
}

async fn discover<F, Fut>(
    fs: &WorkspaceFs,
    request: &AgentsInstructionDiscoveryRequest,
    cancel: &CancellationToken,
    can_read: &F,
) -> AgentsInstructionDiscovery
where
    F: Fn(PathBuf, PathBuf) -> Fut + Send + Sync,
    Fut: Future<Output = bool> + Send,
{
    let mut result = AgentsInstructionDiscovery::default();
    let target = Path::new(&request.target_path);
    if request.device_id.is_empty() || !target.is_absolute() {
        result
            .diagnostics
            .push("AGENTS.md discovery requires a Device and absolute target path".into());
        return result;
    }
    let Some(backing) = resolve(fs, target, true).await else {
        result.diagnostics.push(format!(
            "AGENTS.md target could not be resolved: {}",
            target.display()
        ));
        return result;
    };
    let metadata = tokio::fs::metadata(&backing).await.ok();
    let directory = match request.target_kind {
        AgentsInstructionTargetKind::File => backing.parent().map(Path::to_owned),
        AgentsInstructionTargetKind::Directory => Some(backing),
        AgentsInstructionTargetKind::Auto => match metadata {
            Some(metadata) if metadata.is_dir() => Some(backing),
            Some(metadata) if metadata.is_file() => backing.parent().map(Path::to_owned),
            _ => None,
        },
    };
    let Some(directory) = directory.and_then(|path| fs.public_path_from_host(&path)) else {
        result.diagnostics.push(format!(
            "AGENTS.md target directory is unavailable: {}",
            target.display()
        ));
        return result;
    };
    let Some(directory_name) = directory.to_str() else {
        result
            .diagnostics
            .push("AGENTS.md target directory is not a UTF-8 path".into());
        return result;
    };
    result.target_directory = Some(directory_name.to_owned());

    // Walk public ancestors: Android backing ancestors can belong to the APK host.
    let mut chain = Vec::new();
    let mut found_root = false;
    for ancestor in directory.ancestors() {
        chain.push(ancestor.to_owned());
        if let Some(marker) = resolve(fs, &ancestor.join(".git"), false).await {
            if tokio::fs::metadata(marker)
                .await
                .is_ok_and(|meta| meta.is_dir() || meta.is_file())
            {
                found_root = true;
                break;
            }
        }
    }
    if found_root {
        chain.reverse();
    } else {
        chain.truncate(1);
    }
    let mut remaining = request.max_bytes.min(AGENTS_INSTRUCTIONS_MAX_BYTES);
    for scope in chain {
        let Some(scope_name) = scope.to_str() else {
            continue;
        };
        if request
            .checked_directories
            .iter()
            .any(|checked| checked == scope_name)
        {
            continue;
        }
        result.checked_directories.push(scope_name.to_owned());
        if remaining == 0 {
            result.truncated = true;
            result.diagnostics.push(format!(
                "AGENTS.md combined content budget exhausted at {scope_name}"
            ));
            continue;
        }
        for name in ["AGENTS.override.md", "AGENTS.md"] {
            let candidate = scope.join(name);
            let Some(backing) = resolve(fs, &candidate, false).await else {
                continue;
            };
            let Some(resolved) = fs.public_path_from_host(&backing) else {
                continue;
            };
            if !can_read(candidate.clone(), resolved.clone()).await {
                result.diagnostics.push(format!(
                    "AGENTS.md source was not readable: {}",
                    candidate.display()
                ));
                continue;
            }
            let opened = tokio::task::spawn_blocking(move || open_regular(&backing)).await;
            let Ok(Ok((file, actual, source_bytes))) = opened else {
                result.diagnostics.push(format!(
                    "AGENTS.md source could not be opened: {}",
                    candidate.display()
                ));
                continue;
            };
            let Some(actual) = fs.public_path_from_host(&actual) else {
                continue;
            };
            if WorkspaceFs::check_secret_dir(&actual).is_err()
                || !can_read(candidate.clone(), actual.clone()).await
            {
                continue;
            }
            let read_cancel = cancel.clone();
            let read =
                tokio::task::spawn_blocking(move || read_prefix(file, remaining, &read_cancel))
                    .await;
            let Ok(Ok(bytes)) = read else {
                result.diagnostics.push(format!(
                    "AGENTS.md source could not be read: {}",
                    candidate.display()
                ));
                continue;
            };
            if !can_read(candidate.clone(), actual.clone()).await {
                continue;
            }
            let text = match std::str::from_utf8(&bytes) {
                Ok(text) => Some(text),
                Err(error) if error.error_len().is_none() && source_bytes > bytes.len() as u64 => {
                    std::str::from_utf8(&bytes[..error.valid_up_to()]).ok()
                }
                Err(_) => None,
            };
            let Some(text) = text.filter(|_| !bytes.contains(&0)) else {
                result.diagnostics.push(format!(
                    "AGENTS.md source is not UTF-8 text: {}",
                    candidate.display()
                ));
                continue;
            };
            if text.trim().is_empty() && source_bytes <= bytes.len() as u64 {
                continue;
            }
            let mut end = remaining.min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let content = text[..end].trim().to_owned();
            let truncated = source_bytes > end as u64;
            if truncated {
                result.truncated = true;
                result.diagnostics.push(format!(
                    "AGENTS.md source truncated to the combined content budget: {}",
                    candidate.display()
                ));
            }
            if !content.is_empty() {
                if let Some(path) = actual.to_str() {
                    remaining -= content.len();
                    result.sources.push(AgentsInstructionSource {
                        device_id: request.device_id.clone(),
                        path: path.to_owned(),
                        scope_directory: scope_name.to_owned(),
                        content,
                        source_bytes,
                        truncated,
                        ..Default::default()
                    });
                }
            }
            // Budget exhaustion must not select a lower-priority file from this directory.
            break;
        }
    }
    result
}

fn open_regular(path: &Path) -> std::io::Result<(std::fs::File, PathBuf, u64)> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_NOCTTY);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let actual = crate::core::read_file::opened::opened_path(&file)?;
    if actual.as_os_str() != path.as_os_str() {
        return Err(std::io::Error::other(
            "instruction path changed while opening",
        ));
    }
    Ok((file, actual, metadata.len()))
}

fn read_prefix(
    mut file: std::fs::File,
    max_bytes: usize,
    cancel: &CancellationToken,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(max_bytes + 3);
    let mut buffer = [0; 4096];
    while bytes.len() < max_bytes + 3 {
        if cancel.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        let limit = buffer.len().min(max_bytes + 3 - bytes.len());
        let read = file.read(&mut buffer[..limit])?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok(bytes)
}
