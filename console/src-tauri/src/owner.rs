use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sysinfo::{Pid, Signal, System};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub const OWNER_RECORD_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub executable: PathBuf,
    pub start_time: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerRecord {
    pub version: u8,
    pub shell: ProcessIdentity,
    pub runtime: ProcessIdentity,
    pub port: u16,
    pub shutdown_token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileDecision {
    RemoveStaleRecord,
    PreserveActiveOwner,
    StopOrphan,
}

pub fn capture_process_identity(pid: u32) -> Option<ProcessIdentity> {
    let system = System::new_all();
    let process = system.process(Pid::from_u32(pid))?;
    let executable = process.exe()?.to_path_buf();
    Some(ProcessIdentity {
        pid,
        executable: executable.canonicalize().unwrap_or(executable),
        start_time: process.start_time(),
    })
}

pub fn identity_matches(expected: &ProcessIdentity, actual: &ProcessIdentity) -> bool {
    expected.pid == actual.pid
        && expected.start_time == actual.start_time
        && paths_match(&expected.executable, &actual.executable)
}

pub fn reconcile_decision(
    record: &OwnerRecord,
    live_shell: Option<&ProcessIdentity>,
    live_runtime: Option<&ProcessIdentity>,
) -> ReconcileDecision {
    if live_runtime.is_none_or(|actual| !identity_matches(&record.runtime, actual)) {
        return ReconcileDecision::RemoveStaleRecord;
    }
    if live_shell.is_some_and(|actual| identity_matches(&record.shell, actual)) {
        return ReconcileDecision::PreserveActiveOwner;
    }
    ReconcileDecision::StopOrphan
}

pub fn read(path: &Path) -> Option<OwnerRecord> {
    let bytes = fs::read(path).ok()?;
    let record = serde_json::from_slice::<OwnerRecord>(&bytes).ok()?;
    (record.version == OWNER_RECORD_VERSION).then_some(record)
}

pub fn write(path: &Path, record: &OwnerRecord) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create owner record directory: {error}"))?;
    let temp = parent.join(format!(
        ".{}.tmp.{}.{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let data = serde_json::to_vec_pretty(record)
        .map_err(|error| format!("failed to serialize owner record: {error}"))?;

    let result = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&data)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(format!("failed to persist owner record: {error}"));
    }
    Ok(())
}

pub fn remove(path: &Path) {
    let _ = fs::remove_file(path);
}

pub fn terminate_if_identity_matches(expected: &ProcessIdentity) -> bool {
    let Some(actual) = capture_process_identity(expected.pid) else {
        return true;
    };
    if !identity_matches(expected, &actual) {
        return false;
    }

    let system = System::new_all();
    system
        .process(Pid::from_u32(expected.pid))
        .and_then(|process| process.kill_with(Signal::Kill))
        .unwrap_or(false)
}

fn paths_match(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}
