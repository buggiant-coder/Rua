use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_shell::ShellExt;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveDesktopFileRequest {
    suggested_name: String,
    base64: String,
    mime: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveDesktopFileStatus {
    Saved,
    Cancelled,
}

#[derive(Debug, Serialize)]
pub struct SaveDesktopFileResponse {
    status: SaveDesktopFileStatus,
}

#[tauri::command]
pub async fn save_desktop_file(
    app: tauri::AppHandle,
    request: SaveDesktopFileRequest,
) -> Result<SaveDesktopFileResponse, String> {
    validate_suggested_name(&request.suggested_name)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&request.base64)
        .map_err(|error| format!("invalid file payload: {error}"))?;

    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save Rua Export")
        .set_file_name(&request.suggested_name);
    if let Some(extension) = extension(&request.suggested_name) {
        let description = if request.mime.trim().is_empty() {
            "File"
        } else {
            request.mime.as_str()
        };
        dialog = dialog.add_filter(description, &[extension]);
    }

    let (sender, receiver) = tokio::sync::oneshot::channel();
    dialog.save_file(move |path| {
        let _ = sender.send(path);
    });
    let selected = receiver
        .await
        .map_err(|_| "native Save dialog closed unexpectedly".to_string())?;
    let Some(selected) = selected else {
        return Ok(SaveDesktopFileResponse {
            status: SaveDesktopFileStatus::Cancelled,
        });
    };
    let path = selected
        .into_path()
        .map_err(|error| format!("selected Save path is invalid: {error}"))?;
    atomic_write(&path, &bytes)?;
    Ok(SaveDesktopFileResponse {
        status: SaveDesktopFileStatus::Saved,
    })
}

#[tauri::command]
pub fn open_external_link(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let url = validate_external_url(&url)?;
    #[allow(deprecated)]
    app.shell()
        .open(url, None)
        .map_err(|error| format!("failed to open external link: {error}"))
}

#[tauri::command]
pub fn open_llm_debug_capture_directory(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let path = validate_llm_debug_capture_directory(&path)?;
    #[allow(deprecated)]
    app.shell()
        .open(path.to_string_lossy(), None)
        .map_err(|error| format!("failed to open LLM debug capture directory: {error}"))
}

#[tauri::command]
pub fn open_project_resource_directory(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let path = validate_project_resource_directory(&path)?;
    #[allow(deprecated)]
    app.shell()
        .open(path.to_string_lossy(), None)
        .map_err(|error| format!("Failed to open resource directory: {error}"))
}

#[doc(hidden)]
pub fn validate_project_resource_directory(raw: &str) -> Result<std::path::PathBuf, String> {
    let path = Path::new(raw);
    if !path.is_absolute() || !path.is_dir() {
        return Err("Resource path must be an existing absolute directory".into());
    }
    path.canonicalize().map_err(|error| error.to_string())
}

fn validate_llm_debug_capture_directory(raw: &str) -> Result<std::path::PathBuf, String> {
    let path = Path::new(raw);
    if !path.is_absolute() || !path.is_dir() {
        return Err("LLM debug capture path must be an existing absolute directory".to_string());
    }
    let capture_id = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "LLM debug capture path has no capture id".to_string())?;
    let valid_id = capture_id.len() == 36
        && capture_id.chars().enumerate().all(|(index, ch)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                ch == '-'
            } else {
                ch.is_ascii_hexdigit()
            }
        });
    let parent_is_capture_root = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        == Some("llm-captures");
    if !valid_id || !parent_is_capture_root {
        return Err("path is not an LLM debug capture directory".to_string());
    }
    Ok(path.to_path_buf())
}

#[doc(hidden)]
pub fn validate_external_url(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("external link is empty".to_string());
    }
    if raw.trim() != raw {
        return Err("external link has leading or trailing whitespace".to_string());
    }
    if raw.chars().any(char::is_control) {
        return Err("external link contains control characters".to_string());
    }
    let lowercase = raw.to_ascii_lowercase();
    if (lowercase.starts_with("http:") && !lowercase.starts_with("http://"))
        || (lowercase.starts_with("https:") && !lowercase.starts_with("https://"))
    {
        return Err("external link uses an ambiguous HTTP URL".to_string());
    }

    let parsed =
        reqwest::Url::parse(raw).map_err(|error| format!("invalid external link: {error}"))?;
    match parsed.scheme() {
        "http" | "https" if parsed.host_str().is_some() => {}
        "mailto" | "tel" if !parsed.path().is_empty() => {}
        "http" | "https" => return Err("external link is missing a host".to_string()),
        "mailto" | "tel" => return Err("external link target is empty".to_string()),
        _ => return Err("external link protocol is not supported".to_string()),
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("external links with embedded credentials are not supported".to_string());
    }
    Ok(parsed.to_string())
}

#[doc(hidden)]
pub fn validate_suggested_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.trim() != name {
        return Err("suggested filename is empty or padded".to_string());
    }
    if name.chars().any(char::is_control) {
        return Err("suggested filename contains control characters".to_string());
    }
    let path = Path::new(name);
    if path.file_name().is_none() || path.components().count() != 1 {
        return Err("suggested filename must not contain a path".to_string());
    }
    Ok(())
}

#[doc(hidden)]
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "selected Save path has no parent directory".to_string())?;
    let temp = parent.join(format!(
        ".{}.tmp.{}.{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        replace_file(&temp, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "failed to write selected file {}: {error}",
            path.display()
        ));
    }
    Ok(())
}

fn replace_file(temp: &Path, target: &Path) -> std::io::Result<()> {
    match fs::rename(temp, target) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::remove_file(target)?;
            fs::rename(temp, target)
        }
        Err(error) => Err(error),
    }
}

fn extension(name: &str) -> Option<&str> {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
}
