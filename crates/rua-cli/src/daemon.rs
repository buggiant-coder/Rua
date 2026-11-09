use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use rua_config::dirs::Paths;

use crate::process_utils;

#[derive(Debug, Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub command: DaemonCommands,
}

#[derive(Debug, Subcommand)]
pub enum DaemonCommands {
    Status,

    Start,

    Stop,

    ReloadConfig,

    Logs {
        #[arg(short = 'n', long, default_value_t = 100)]
        lines: usize,
    },
}

fn pid_file_path(paths: &Paths) -> PathBuf {
    paths.rua_root.join("daemon.pid")
}

fn log_file_path(paths: &Paths) -> PathBuf {
    paths.rua_root.join("daemon.log")
}

fn read_pid(paths: &Paths) -> Option<u32> {
    fs::read_to_string(pid_file_path(paths))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}

fn is_process_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn run_status(paths: &Paths) -> anyhow::Result<()> {
    let pid_path = pid_file_path(paths);
    match read_pid(paths) {
        Some(pid) if is_process_alive(pid) => {
            println!("Daemon is running (pid={pid})");
            println!("  PID file: {}", pid_path.display());
        }
        Some(pid) => {
            println!("Daemon is not running (stale pid={pid})");
            let _ = fs::remove_file(&pid_path);
        }
        None => {
            println!("Daemon is not running (no PID file)");
        }
    }
    Ok(())
}

fn run_start(paths: &Paths) -> anyhow::Result<()> {
    if let Some(pid) = process_utils::is_rua_running(paths) {
        bail!(
            "Rua is already running (pid={pid}, workspace={})",
            paths.rua_root.display()
        );
    }

    if let Some(pid) = read_pid(paths) {
        if is_process_alive(pid) {
            println!("Daemon is already running (pid={pid})");
            return Ok(());
        }
        let _ = fs::remove_file(pid_file_path(paths));
    }

    let exe = std::env::current_exe().context("cannot determine executable path")?;

    fs::create_dir_all(&paths.rua_root)
        .with_context(|| format!("cannot create working dir: {}", paths.rua_root.display()))?;

    let log_path = log_file_path(paths);
    let log_file = fs::File::create(&log_path)
        .with_context(|| format!("cannot create log file: {}", log_path.display()))?;
    let err_file = log_file
        .try_clone()
        .context("cannot clone log file handle")?;

    let child = Command::new(&exe)
        .args(["server"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(err_file))
        .spawn()
        .with_context(|| format!("failed to spawn daemon: {}", exe.display()))?;

    let pid = child.id();
    fs::write(pid_file_path(paths), pid.to_string()).context("failed to write PID file")?;

    println!("Daemon started (pid={pid})");
    println!("  Log: {}", log_path.display());
    Ok(())
}

fn run_reload_config(paths: &Paths) -> anyhow::Result<()> {
    match read_pid(paths) {
        Some(pid) if is_process_alive(pid) => {
            let status = Command::new("kill")
                .args(["-HUP", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .context("failed to send SIGHUP")?;

            if status.success() {
                println!("Config reload signal sent to daemon (pid={pid})");
            } else {
                bail!("failed to send reload signal to daemon (pid={pid})");
            }
        }
        Some(pid) => {
            let _ = fs::remove_file(pid_file_path(paths));
            println!("Daemon is not running (stale pid={pid}, cleaned up)");
        }
        None => {
            println!("Daemon is not running (no PID file)");
        }
    }
    Ok(())
}

fn run_logs(paths: &Paths, lines: usize) -> anyhow::Result<()> {
    let lines = lines.clamp(1, 2000);
    let log_path = paths.rua_root.join("rua.log");

    if !log_path.is_file() {
        println!("(Log file not found: {})", log_path.display());
        return Ok(());
    }

    let meta = fs::metadata(&log_path)
        .with_context(|| format!("cannot read log file: {}", log_path.display()))?;
    let size = meta.len();

    if size == 0 {
        println!("(empty)");
        return Ok(());
    }

    const MAX_BYTES: u64 = 512 * 1024;

    let mut file = fs::File::open(&log_path)
        .with_context(|| format!("cannot open log file: {}", log_path.display()))?;

    let content = if size <= MAX_BYTES {
        let mut buf = String::new();
        file.read_to_string(&mut buf)
            .with_context(|| format!("cannot read log file: {}", log_path.display()))?;
        buf
    } else {
        file.seek(SeekFrom::End(-(MAX_BYTES as i64)))
            .with_context(|| "cannot seek in log file")?;
        let mut buf = vec![0u8; MAX_BYTES as usize];
        let n = file
            .read(&mut buf)
            .with_context(|| "cannot read log file")?;
        buf.truncate(n);
        let raw = String::from_utf8_lossy(&buf).into_owned();
        match raw.find('\n') {
            Some(pos) => raw[pos + 1..].to_string(),
            None => raw,
        }
    };

    let all_lines: Vec<&str> = content.lines().collect();
    let start = all_lines.len().saturating_sub(lines);
    for line in &all_lines[start..] {
        println!("{line}");
    }
    Ok(())
}

fn run_stop(paths: &Paths) -> anyhow::Result<()> {
    let pid_path = pid_file_path(paths);
    match read_pid(paths) {
        Some(pid) if is_process_alive(pid) => {
            let status = Command::new("kill")
                .args([&pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .context("failed to send kill signal")?;

            if status.success() {
                let _ = fs::remove_file(&pid_path);
                println!("Daemon stopped (pid={pid})");
            } else {
                bail!("failed to stop daemon (pid={pid})");
            }
        }
        Some(pid) => {
            let _ = fs::remove_file(&pid_path);
            println!("Daemon was not running (stale pid={pid}, cleaned up)");
        }
        None => {
            println!("Daemon is not running (no PID file)");
        }
    }
    Ok(())
}

pub fn run(args: DaemonArgs, paths: &Paths) -> anyhow::Result<()> {
    match args.command {
        DaemonCommands::Status => run_status(paths),
        DaemonCommands::Start => run_start(paths),
        DaemonCommands::Stop => run_stop(paths),
        DaemonCommands::ReloadConfig => run_reload_config(paths),
        DaemonCommands::Logs { lines } => run_logs(paths, lines),
    }
}
