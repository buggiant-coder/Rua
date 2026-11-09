use std::path::Path;

use tracing::{debug, info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalModelStatus {
    Running,
    Stopped,
}

#[derive(Debug, Clone)]
pub struct ResumedLocalModel {
    pub model_id: String,
    pub pid: u32,
    pub status: LocalModelStatus,
}

#[derive(Debug, Clone)]
pub struct RestartResult {
    pub port: u16,
    pub base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeOutcome {
    AlreadyRunning { pid: u32 },
    Restarted { port: u16 },
    RestartFailed { error: String },
}

#[derive(Debug, Clone)]
pub struct ResumeRestartEntry {
    pub model_id: String,
    pub outcome: ResumeOutcome,
}

pub fn resume_local_models(working_dir: &Path) -> Vec<ResumedLocalModel> {
    let local_models_dir = working_dir.join("local_models");
    if !local_models_dir.is_dir() {
        debug!(path = %local_models_dir.display(), "local_models directory not found, skipping resume");
        return Vec::new();
    }

    let entries = match std::fs::read_dir(&local_models_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "failed to read local_models directory");
            return Vec::new();
        }
    };

    let mut results = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("pid") {
            continue;
        }

        let model_id = match path.file_stem().and_then(|s| s.to_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };

        let pid = match read_pid_file(&path) {
            Some(p) => p,
            None => {
                warn!(path = %path.display(), "skipping invalid PID file");
                continue;
            }
        };

        let alive = is_process_alive(pid);
        let status = if alive {
            info!(model_id = %model_id, pid, "local model process is alive");
            LocalModelStatus::Running
        } else {
            info!(model_id = %model_id, pid, "local model process is dead, cleaning up PID file");
            if let Err(e) = std::fs::remove_file(&path) {
                warn!(error = %e, path = %path.display(), "failed to remove stale PID file");
            }
            LocalModelStatus::Stopped
        };

        results.push(ResumedLocalModel {
            model_id,
            pid,
            status,
        });
    }

    results
}

fn read_pid_file(path: &Path) -> Option<u32> {
    let content = std::fs::read_to_string(path).ok()?;
    content.trim().parse::<u32>().ok()
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    false
}
