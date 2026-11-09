use std::process::Command;

use serde::Serialize;

use crate::registry::{register_check, CheckContext, CheckResult, CheckStatus};

const LLAMA_SERVER_BINARY: &str = "llama-server";

#[derive(Debug, Clone, Serialize)]
pub struct LlamaCppInstallDetails {
    pub installed: bool,
    pub binary_path: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlamaCppServerDetails {
    pub running: bool,
    pub pid: Option<String>,
    pub listen_address: Option<String>,
}

fn find_binary(name: &str) -> Option<String> {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    let output = Command::new(cmd).arg(name).output().ok()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            None
        } else {
            Some(path)
        }
    } else {
        None
    }
}

fn get_version(binary_path: &str) -> Option<String> {
    let output = Command::new(binary_path).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            None
        } else {
            Some(stderr)
        }
    } else {
        Some(text)
    }
}

fn check_llamacpp_installation(_ctx: &CheckContext) -> CheckResult {
    let binary_path = find_binary(LLAMA_SERVER_BINARY);

    let (installed, version) = match &binary_path {
        Some(path) => (true, get_version(path)),
        None => (false, None),
    };

    let details = LlamaCppInstallDetails {
        installed,
        binary_path: binary_path.clone(),
        version: version.clone(),
    };

    let (status, message) = if !installed {
        (
            CheckStatus::Warn,
            format!("{LLAMA_SERVER_BINARY} not found in PATH"),
        )
    } else {
        let ver = version.as_deref().unwrap_or("unknown");
        (
            CheckStatus::Pass,
            format!("{} ({})", binary_path.as_deref().unwrap_or(""), ver),
        )
    };

    CheckResult {
        name: "llamacpp_installation".into(),
        status,
        message,
        details: Some(serde_json::to_value(&details).unwrap()),
    }
}

fn find_llamacpp_server_process() -> Option<(String, Option<String>)> {
    if cfg!(windows) {
        return None;
    }

    let output = Command::new("pgrep")
        .args(["-f", LLAMA_SERVER_BINARY])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let pids = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let pid = pids.lines().next()?.to_string();
    if pid.is_empty() {
        return None;
    }

    let listen = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-p", &pid])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout).to_string();
                text.lines().skip(1).find_map(|line| {
                    let fields: Vec<&str> = line.split_whitespace().collect();
                    fields.get(8).map(|s| s.to_string())
                })
            } else {
                None
            }
        });

    Some((pid, listen))
}

fn check_llamacpp_server_status(_ctx: &CheckContext) -> CheckResult {
    let process_info = find_llamacpp_server_process();

    let details = match &process_info {
        Some((pid, listen)) => LlamaCppServerDetails {
            running: true,
            pid: Some(pid.clone()),
            listen_address: listen.clone(),
        },
        None => LlamaCppServerDetails {
            running: false,
            pid: None,
            listen_address: None,
        },
    };

    let (status, message) = if details.running {
        let pid = details.pid.as_deref().unwrap_or("?");
        let addr = details.listen_address.as_deref().unwrap_or("unknown port");
        (CheckStatus::Pass, format!("running (pid={pid}, {addr})"))
    } else {
        (CheckStatus::Warn, "llama-server not running".to_string())
    };

    CheckResult {
        name: "llamacpp_server_status".into(),
        status,
        message,
        details: Some(serde_json::to_value(&details).unwrap()),
    }
}

inventory::submit! {
    register_check("llamacpp_installation", check_llamacpp_installation)
}

inventory::submit! {
    register_check("llamacpp_server_status", check_llamacpp_server_status)
}
