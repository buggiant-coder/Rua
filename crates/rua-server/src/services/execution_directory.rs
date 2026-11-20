use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchRequest, ToolchainIntent,
};
use rua_core::runtime_layout::RuntimeLayout;
use rua_project::{ProjectDirectoryEntry, ProjectDirectoryEntryKind, ProjectDirectoryListing};

pub(crate) async fn validate(path: &Path, rua_root: &Path) -> Result<PathBuf, String> {
    let path = path.to_owned();
    let rua_root = rua_root.to_owned();
    tokio::task::spawn_blocking(move || {
        let layout = RuntimeLayout::from_current_process(None);
        let host = layout
            .execution_host_path(&path)
            .map_err(|error| error.to_string())?;
        rua_project::validate_project_directory(&host).map_err(|error| error.to_string())?;
        if layout.linux_compat.is_some() {
            let authority =
                RuntimeAuthority::from_current_process(None, &rua_root, rua_root.join("cache"));
            authority
                .plan(
                    ProcessShape::Direct,
                    ToolchainIntent::GenericCommand {
                        command: "/bin/sh".into(),
                    },
                    RuntimeLaunchRequest {
                        cwd: Some(host),
                        ..Default::default()
                    },
                )
                .process()
                .map_err(|error| error.to_string())?;
        }
        Ok(path)
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) async fn browse(path: &str) -> Result<ProjectDirectoryListing, String> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let layout = RuntimeLayout::from_current_process(None);
        if !layout.host_profile.is_mobile() && layout.os_family != "android" {
            return rua_project::list_project_directory(&path).map_err(|error| error.to_string());
        }
        let guest = layout
            .linux_compat
            .as_ref()
            .ok_or("Linux Compatibility Runtime is unavailable")?;
        let path = match path.as_str() {
            "~" => guest.guest_home.clone(),
            path if path.starts_with("~/") => guest.guest_home.join(&path[2..]),
            path => PathBuf::from(path),
        };
        let host = layout
            .execution_host_path(&path)
            .map_err(|error| error.to_string())?;
        // Include mounted roots even when the unmounted Debian directory has no corresponding entry.
        let mut children = BTreeMap::new();
        for entry in std::fs::read_dir(&host)
            .map_err(|e| format!("Cannot browse {}: {e}", path.display()))?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            children.insert(
                entry.file_name(),
                entry.file_type().is_ok_and(|kind| kind.is_symlink()),
            );
        }
        for bind in &guest.binds {
            if bind.guest.parent() == Some(path.as_path()) && bind.host.exists() {
                if let Some(name) = bind.guest.file_name() {
                    children.insert(name.to_owned(), false);
                }
            }
        }
        let mut entries = Vec::new();
        let mut omitted_entries = 0;
        for (name, is_symlink) in children {
            let Ok(name) = name.into_string() else {
                omitted_entries += 1;
                continue;
            };
            let child = path.join(&name);
            let Some(metadata) = layout
                .execution_host_path(&child)
                .ok()
                .and_then(|host| std::fs::metadata(host).ok())
            else {
                omitted_entries += 1;
                continue;
            };
            let kind = if metadata.is_dir() {
                ProjectDirectoryEntryKind::Directory
            } else if metadata.is_file() {
                ProjectDirectoryEntryKind::File
            } else {
                ProjectDirectoryEntryKind::Other
            };
            entries.push(ProjectDirectoryEntry {
                name,
                path: child,
                kind,
                is_symlink,
            });
        }
        entries.sort_by_cached_key(|entry| {
            (
                entry.kind != ProjectDirectoryEntryKind::Directory,
                entry.name.to_lowercase(),
            )
        });
        let roots =
            [Path::new("/"), guest.guest_home.as_path()].map(|root| ProjectDirectoryEntry {
                name: root.to_string_lossy().into(),
                path: root.into(),
                kind: ProjectDirectoryEntryKind::Directory,
                is_symlink: false,
            });
        Ok(ProjectDirectoryListing {
            path,
            entries,
            omitted_entries,
            roots: roots.into(),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) fn select_draft(
    extra: &mut serde_json::Map<String, serde_json::Value>,
    default: Option<&rua_core::ExecutionDirectorySelection>,
    local_device_id: &str,
) -> Result<String, String> {
    let target = extra
        .get("target_device_id")
        .map(|value| {
            value
                .as_str()
                .filter(|v| !v.trim().is_empty())
                .ok_or("Invalid target Device ID")
        })
        .transpose()?;
    let selection = if let Some(value) = extra.get(rua_session::EXECUTION_DIRECTORY_KEY) {
        let selection =
            serde_json::from_value::<rua_core::ExecutionDirectorySelection>(value.clone())
                .map_err(|e| format!("Invalid execution_directory: {e}"))?;
        if target.is_some_and(|device| device != selection.device_id) {
            return Err("Execution directory and target Device disagree".into());
        }
        selection
    } else if let Some(default) = default.filter(|_| target.is_none()) {
        default.clone()
    } else {
        rua_core::ExecutionDirectorySelection {
            device_id: target.unwrap_or(local_device_id).to_owned(),
            path: None,
            extra: Default::default(),
        }
    };
    selection.validate().map_err(|e| e.to_string())?;
    extra.insert(
        rua_session::EXECUTION_DIRECTORY_KEY.into(),
        serde_json::json!({"device_id":selection.device_id,"path":selection.path}),
    );
    extra.insert(
        "target_device_id".into(),
        serde_json::json!(selection.device_id),
    );
    Ok(selection.device_id)
}
