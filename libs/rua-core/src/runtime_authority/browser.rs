//! System Chromium discovery. Discovery never installs or downloads a browser.
use std::path::{Path, PathBuf};

use super::{RuntimeAuthority, RuntimeCapability, RuntimeCapabilitySource};

impl RuntimeAuthority {
    pub fn selected_chromium(&self, selected: Option<&Path>) -> RuntimeCapability {
        let Some(path) = selected else {
            return self.system_chromium();
        };
        let mut capability = RuntimeCapability::available(
            "chromium",
            path.to_path_buf(),
            RuntimeCapabilitySource::Configured,
        );
        let diagnostic = if !path.is_absolute() {
            Some("Browser path must be absolute")
        } else if !path.exists() {
            Some("Selected browser path does not exist; select an installed Chromium executable")
        } else if !executable_file(path) {
            Some("Selected browser path is not an executable file")
        } else {
            None
        };
        if let Some(message) = diagnostic {
            capability.available = false;
            capability.diagnostics.push(message.into());
        }
        capability
    }

    pub fn system_chromium(&self) -> RuntimeCapability {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let windows_roots: Vec<PathBuf> = ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"]
            .iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .collect();
        discover_chromium(
            &self.inventory.os_family,
            home.as_deref(),
            &windows_roots,
            executable_file,
        )
    }
}

pub fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The probe and install roots are explicit so discovery can be verified on every host.
pub fn discover_chromium(
    os: &str,
    home: Option<&Path>,
    windows_roots: &[PathBuf],
    is_executable: impl Fn(&Path) -> bool,
) -> RuntimeCapability {
    let candidates: Vec<PathBuf> = match os {
        "macos" => [
            Some(PathBuf::from("/Applications")),
            home.map(|p| p.join("Applications")),
        ]
        .into_iter()
        .flatten()
        .flat_map(|root| {
            [
                "Google Chrome.app/Contents/MacOS/Google Chrome",
                "Chromium.app/Contents/MacOS/Chromium",
                "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
                "Brave Browser.app/Contents/MacOS/Brave Browser",
            ]
            .map(|suffix| root.join(suffix))
        })
        .collect(),
        "windows" => windows_roots
            .iter()
            .flat_map(|root| {
                [
                    "Google/Chrome/Application/chrome.exe",
                    "Chromium/Application/chrome.exe",
                    "Microsoft/Edge/Application/msedge.exe",
                    "BraveSoftware/Brave-Browser/Application/brave.exe",
                ]
                .map(|suffix| root.join(suffix))
            })
            .collect(),
        _ => {
            return RuntimeCapability::unavailable(
                "chromium",
                "Browser Use requires macOS or Windows Desktop",
            )
        }
    };
    match candidates.into_iter().find(|path| is_executable(path)) {
        Some(path) => RuntimeCapability::available("chromium", path, RuntimeCapabilitySource::RuntimeLayout),
        None => RuntimeCapability::unavailable("chromium", "No executable Chromium browser found. Install a Chromium browser or select its executable in device settings."),
    }
}
