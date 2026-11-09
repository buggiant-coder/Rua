use rua_config::dirs::Paths;
use sysinfo::{ProcessRefreshKind, RefreshKind, System};

pub fn is_rua_running(paths: &Paths) -> Option<u32> {
    let self_pid = std::process::id();
    let rua_root = paths.rua_root.to_string_lossy();

    let sys = System::new_with_specifics(
        RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
    );

    for (pid, proc) in sys.processes() {
        let pid_u32 = pid.as_u32();
        if pid_u32 == self_pid {
            continue;
        }

        let name = proc.name().to_string_lossy().into_owned();
        let cmd: Vec<String> = proc
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();

        if is_rua_process(&name, &cmd, &rua_root) {
            return Some(pid_u32);
        }
    }

    None
}

pub fn is_rua_process(name: &str, cmd: &[String], our_rua_root: &str) -> bool {
    let cmdline = cmd.join(" ");
    let is_rua = name == "rua" || name == "rua-server" || cmdline.contains("rua-server");
    if !is_rua {
        return false;
    }
    matches_rua_root(cmd, our_rua_root)
}

fn matches_rua_root(cmd: &[String], our_rua_root: &str) -> bool {
    let their_rua_root = extract_rua_root_arg(cmd);
    match their_rua_root {
        Some(root) => paths_equal(root, our_rua_root),
        None => {
            let default_root = rua_config::dirs::rua_root();
            paths_equal(our_rua_root, &default_root.to_string_lossy())
        }
    }
}

fn paths_equal(left: &str, right: &str) -> bool {
    lexical_normalize(left) == lexical_normalize(right)
}

fn lexical_normalize(path: &str) -> std::path::PathBuf {
    let mut normalized = std::path::PathBuf::new();
    for component in std::path::Path::new(path).components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn extract_rua_root_arg(cmd: &[String]) -> Option<&str> {
    let mut iter = cmd.iter();
    while let Some(arg) = iter.next() {
        if arg == "--rua-root" {
            return iter.next().map(|s| s.as_str());
        }
        if let Some(val) = arg.strip_prefix("--rua-root=") {
            return Some(val);
        }
    }
    None
}

pub fn kill_stale_rua(pid: u32) -> anyhow::Result<()> {
    kill_process(pid)
}

#[cfg(unix)]
fn kill_process(pid: u32) -> anyhow::Result<()> {
    use std::process::Command;
    let status = Command::new("kill").arg(pid.to_string()).status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("failed to kill process {pid}");
    }
}

#[cfg(windows)]
fn kill_process(pid: u32) -> anyhow::Result<()> {
    use std::process::Command;
    let status = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("failed to kill process {pid}");
    }
}
