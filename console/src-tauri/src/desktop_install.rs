use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::time::Duration;

use serde::Serialize;
use sysinfo::{Pid, Signal, System};
use tauri::Manager;

use crate::owner::{self, ProcessIdentity};
use crate::preferences;

const LEGACY_LABEL: &str = "com.rua.server";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyRuntimeStatus {
    pub legacy_runtime: bool,
    pub legacy_port: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct LegacyRuntime {
    pub port: u16,
    pub identity: ProcessIdentity,
    pub plist_path: PathBuf,
    pub rua_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct DesktopPaths {
    rua_root: PathBuf,
    legacy_bin: PathBuf,
    legacy_endpoint: PathBuf,
    legacy_plist: PathBuf,
}

impl DesktopPaths {
    pub fn resolve(app: &tauri::AppHandle) -> Result<Self, String> {
        let home = app
            .path()
            .home_dir()
            .map_err(|error| format!("failed to resolve home directory: {error}"))?;
        let rua_root = preferences::configured_rua_root(app)?;
        Ok(Self {
            legacy_bin: home.join(".local/bin").join(binary_name()),
            legacy_endpoint: home.join(".local/share/rua/cli-endpoint.env"),
            legacy_plist: home
                .join("Library/LaunchAgents")
                .join(format!("{LEGACY_LABEL}.plist")),
            rua_root,
        })
    }

    fn server_pid(&self) -> PathBuf {
        self.rua_root.join("server.pid")
    }
}

pub async fn status(app: &tauri::AppHandle) -> Result<LegacyRuntimeStatus, String> {
    let paths = DesktopPaths::resolve(app)?;
    let legacy = detect_legacy_runtime(&paths).await?;
    Ok(LegacyRuntimeStatus {
        legacy_runtime: legacy.is_some(),
        legacy_port: legacy.map(|runtime| runtime.port),
    })
}

pub async fn migrate_legacy_runtime(app: &tauri::AppHandle, confirmed: bool) -> Result<(), String> {
    require_migration_confirmation(confirmed)?;
    let paths = DesktopPaths::resolve(app)?;
    let legacy = detect_legacy_runtime(&paths)
        .await?
        .ok_or_else(|| "no healthy legacy launchd Runtime was detected".to_string())?;
    stop_and_unregister_legacy(&paths, &legacy).await?;
    crate::runtime::start(app).await
}

pub async fn detect_legacy_runtime(paths: &DesktopPaths) -> Result<Option<LegacyRuntime>, String> {
    if !cfg!(target_os = "macos") || !paths.legacy_plist.is_file() {
        return Ok(None);
    }
    let Some(port) = read_endpoint_port(&paths.legacy_endpoint) else {
        return Ok(None);
    };
    let pid = match fs::read_to_string(paths.server_pid())
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
    {
        Some(pid) => pid,
        None => return Ok(None),
    };
    let Some(identity) = owner::capture_process_identity(pid) else {
        return Ok(None);
    };
    if !legacy_identity_matches(paths, &identity) || !ready_on_port(port) {
        return Ok(None);
    }
    Ok(Some(LegacyRuntime {
        port,
        identity,
        plist_path: paths.legacy_plist.clone(),
        rua_root: paths.rua_root.clone(),
    }))
}

pub async fn stop_and_unregister_legacy(
    paths: &DesktopPaths,
    legacy: &LegacyRuntime,
) -> Result<(), String> {
    if !legacy_identity_matches(paths, &legacy.identity) {
        return Err("legacy Runtime process identity changed before migration".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let domain = format!("gui/{}", unsafe { libc::geteuid() });
        let service = format!("{domain}/{LEGACY_LABEL}");
        let _ = Command::new("launchctl")
            .args(["bootout", &service])
            .status();
        let _ = Command::new("launchctl")
            .args(["bootout", &domain])
            .arg(&legacy.plist_path)
            .status();
        let _ = Command::new("launchctl")
            .args(["remove", LEGACY_LABEL])
            .status();
    }

    let system = System::new_all();
    if let Some(process) = system.process(Pid::from_u32(legacy.identity.pid)) {
        let _ = process.kill_with(Signal::Term);
    }
    if !wait_for_exit(&legacy.identity, Duration::from_secs(15)).await {
        return Err(format!(
            "legacy Runtime pid {} did not exit after migration request",
            legacy.identity.pid
        ));
    }

    fs::remove_file(&legacy.plist_path)
        .map_err(|error| format!("failed to remove legacy launchd plist: {error}"))?;
    let _ = fs::remove_file(paths.server_pid());
    Ok(())
}

#[doc(hidden)]
pub fn legacy_identity_matches(paths: &DesktopPaths, identity: &ProcessIdentity) -> bool {
    let executable_matches = match (
        paths.legacy_bin.canonicalize(),
        identity.executable.canonicalize(),
    ) {
        (Ok(expected), Ok(actual)) => expected == actual,
        _ => paths.legacy_bin == identity.executable,
    };
    if !executable_matches {
        return false;
    }
    let system = System::new_all();
    let Some(process) = system.process(Pid::from_u32(identity.pid)) else {
        return false;
    };
    let command = process
        .cmd()
        .iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>();
    legacy_command_matches(&command, &paths.rua_root)
}

#[doc(hidden)]
pub fn legacy_command_matches<S: AsRef<str>>(command: &[S], rua_root: &Path) -> bool {
    command
        .windows(2)
        .any(|parts| parts[0].as_ref() == "server" && parts[1].as_ref() == "start")
        && command.windows(2).any(|parts| {
            parts[0].as_ref() == "--rua-root" && Path::new(parts[1].as_ref()) == rua_root
        })
}

#[doc(hidden)]
pub fn parse_endpoint_port(content: &str) -> Option<u16> {
    content.lines().find_map(|line| {
        let (key, value) = line.trim().split_once('=')?;
        (key.trim() == "RUA_PORT")
            .then(|| value.trim().parse::<u16>().ok())
            .flatten()
            .filter(|port| *port > 0)
    })
}

#[doc(hidden)]
pub fn require_migration_confirmation(confirmed: bool) -> Result<(), String> {
    if confirmed {
        Ok(())
    } else {
        Err("legacy Runtime migration requires explicit confirmation".to_string())
    }
}

fn read_endpoint_port(path: &Path) -> Option<u16> {
    parse_endpoint_port(&fs::read_to_string(path).ok()?)
}

fn ready_on_port(port: u16) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(500)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(800)));
    if write!(
        stream,
        "GET /readyz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .and_then(|_| stream.flush())
    .is_err()
    {
        return false;
    }
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 "))
}

async fn wait_for_exit(identity: &ProcessIdentity, timeout: Duration) -> bool {
    let start = tokio::time::Instant::now();
    while start.elapsed() < timeout {
        let Some(actual) = owner::capture_process_identity(identity.pid) else {
            return true;
        };
        if !owner::identity_matches(identity, &actual) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "rua.exe"
    } else {
        "rua"
    }
}
