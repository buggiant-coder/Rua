use std::path::Path;

use serde::Serialize;

use crate::registry::{register_check, CheckContext, CheckResult, CheckStatus};

const WARN_THRESHOLD: u64 = 500 * 1024 * 1024;
const FAIL_THRESHOLD: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct DiskDetails {
    pub volumes: Vec<VolumeInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VolumeInfo {
    pub label: String,
    pub path: String,
    pub free_bytes: u64,
    pub free_display: String,
}

pub fn status_from_free(free_bytes: u64) -> CheckStatus {
    if free_bytes < FAIL_THRESHOLD {
        CheckStatus::Fail
    } else if free_bytes < WARN_THRESHOLD {
        CheckStatus::Warn
    } else {
        CheckStatus::Pass
    }
}

fn format_gib(bytes: u64) -> String {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    format!("{gib:.2} GiB")
}

fn resolve_existing_ancestor(path: &Path) -> Option<&Path> {
    let mut current = path;
    loop {
        if current.exists() {
            return Some(current);
        }
        current = current.parent()?;
    }
}

fn get_free_bytes(path: &Path) -> Result<u64, String> {
    let target = resolve_existing_ancestor(path)
        .ok_or_else(|| format!("no existing ancestor for {}", path.display()))?;
    fs2::available_space(target).map_err(|e| format!("available_space: {e}"))
}

#[cfg(unix)]
fn device_id(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let target = resolve_existing_ancestor(path)?;
    std::fs::metadata(target).ok().map(|m| m.dev())
}

#[cfg(windows)]
fn device_id(path: &Path) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let target = resolve_existing_ancestor(path)?;
    let prefix = target.components().next()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    prefix.as_os_str().hash(&mut h);
    Some(h.finish())
}

fn check_disk(ctx: &CheckContext) -> CheckResult {
    let working_dir = &ctx.paths.rua_root;
    let secret_dir = &ctx.paths.secret_dir;
    let mut volumes = Vec::new();

    match get_free_bytes(working_dir) {
        Ok(free) => {
            volumes.push(VolumeInfo {
                label: "working dir".into(),
                path: working_dir.display().to_string(),
                free_bytes: free,
                free_display: format_gib(free),
            });
        }
        Err(e) => {
            return CheckResult {
                name: "disk".into(),
                status: CheckStatus::Fail,
                message: format!("cannot read disk info for working dir: {e}"),
                details: None,
            };
        }
    }

    let same_volume =
        device_id(working_dir) == device_id(secret_dir) && device_id(working_dir).is_some();

    if !same_volume {
        if let Ok(free) = get_free_bytes(secret_dir) {
            volumes.push(VolumeInfo {
                label: "secret dir".into(),
                path: secret_dir.display().to_string(),
                free_bytes: free,
                free_display: format_gib(free),
            });
        }
    }

    let worst = volumes
        .iter()
        .map(|v| v.free_bytes)
        .min()
        .unwrap_or(u64::MAX);
    let status = status_from_free(worst);

    let msg = volumes
        .iter()
        .map(|v| format!("disk free ({} volume): {}", v.label, v.free_display))
        .collect::<Vec<_>>()
        .join("; ");

    CheckResult {
        name: "disk".into(),
        status,
        message: msg,
        details: Some(serde_json::to_value(&DiskDetails { volumes }).unwrap()),
    }
}

inventory::submit! { register_check("disk", check_disk) }
