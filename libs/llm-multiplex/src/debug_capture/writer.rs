//! One bounded, ordered disk worker per enabled capture. Enqueue is not durability.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;

const MAX_PENDING_BYTES: usize = 64 * 1024 * 1024;
const MAX_PENDING_ARTIFACTS: usize = 256;

type WriteArtifact = dyn Fn(&Path, &[u8], bool) -> io::Result<()> + Send;

pub(super) struct CaptureWriter {
    shared: Arc<Shared>,
    pub(super) samplers: tokio_util::task::TaskTracker,
}

struct Shared {
    root: PathBuf,
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Clone, Default, Serialize)]
pub(super) struct WriterStatus {
    pub writer_id: String,
    pub accepted_artifacts: u64,
    pub completed_artifacts: u64,
    pub failed_artifacts: u64,
    pub rejected_artifacts: u64,
    pub pending_artifacts: usize,
    pub pending_bytes: usize,
    pub background_write_ms: f64,
    pub max_background_write_ms: f64,
    pub max_queue_wait_ms: f64,
    pub closed: bool,
    pub last_error: Option<String>,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Artifact>,
    status: WriterStatus,
    revision: u64,
    persisted_revision: u64,
    reported_completed_artifacts: u64,
    stopped: bool,
}

struct Artifact {
    path: PathBuf,
    bytes: Vec<u8>,
    only_if_missing: bool,
    queued_at: Instant,
}

struct RegisteredWriter {
    handle: Weak<CaptureWriter>,
    shared: Arc<Shared>,
}

fn writers() -> &'static Mutex<Vec<RegisteredWriter>> {
    static WRITERS: OnceLock<Mutex<Vec<RegisteredWriter>>> = OnceLock::new();
    WRITERS.get_or_init(Mutex::default)
}

impl CaptureWriter {
    pub(super) fn start(root: PathBuf) -> io::Result<Arc<Self>> {
        Self::start_with_writer(root, Box::new(atomic_write))
    }

    pub(super) fn start_with_writer(
        root: PathBuf,
        write: Box<WriteArtifact>,
    ) -> io::Result<Arc<Self>> {
        let mut writers = writers().lock().unwrap_or_else(|e| e.into_inner());
        // Restore/re-enable may overlap a trace finishing after disable. Reuse
        // its worker so snapshots for the same capture can never overtake one another.
        for existing in writers.iter().filter(|entry| entry.shared.root == root) {
            if let Some(handle) = existing.handle.upgrade() {
                if !handle.status().closed {
                    return Ok(handle);
                }
            }
            if let Err(error) = existing
                .shared
                .flush(Instant::now() + Duration::from_secs(5))
            {
                if error.kind() == io::ErrorKind::TimedOut {
                    return Err(error);
                }
                // Earlier failures remain in that writer's status artifact; they
                // do not permanently prevent an explicitly re-enabled capture.
                tracing::warn!(%error, "previous LLM capture writer drained with errors");
            }
        }
        writers.retain(|entry| {
            entry.handle.strong_count() > 0
                || !entry
                    .shared
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .stopped
        });
        let shared = Arc::new(Shared {
            root,
            state: Mutex::new(State {
                status: WriterStatus {
                    writer_id: uuid::Uuid::new_v4().to_string(),
                    ..WriterStatus::default()
                },
                ..State::default()
            }),
            changed: Condvar::new(),
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("llm-capture-writer".into())
            .spawn(move || worker.run(write))?;
        let writer = Arc::new(Self {
            shared: shared.clone(),
            samplers: tokio_util::task::TaskTracker::new(),
        });
        writers.push(RegisteredWriter {
            handle: Arc::downgrade(&writer),
            shared,
        });
        Ok(writer)
    }

    /// Only serialization and bounded enqueue run on the caller. No filesystem I/O.
    pub(super) fn record<T: Serialize>(
        &self,
        path: PathBuf,
        value: &T,
        only_if_missing: bool,
    ) -> bool {
        let mut bytes = LimitedBuffer(Vec::new());
        if let Err(error) = serde_json::to_writer_pretty(&mut bytes, value) {
            return self.reject(error.to_string());
        }
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.status.closed
            || state.status.pending_artifacts >= MAX_PENDING_ARTIFACTS
            || state.status.pending_bytes + bytes.0.len() > MAX_PENDING_BYTES
        {
            drop(state);
            return self.reject("capture writer closed or backlog limit reached".into());
        }
        state.status.accepted_artifacts += 1;
        state.status.pending_artifacts += 1;
        state.status.pending_bytes += bytes.0.len();
        state.revision += 1;
        state.queue.push_back(Artifact {
            path,
            bytes: bytes.0,
            only_if_missing,
            queued_at: Instant::now(),
        });
        self.shared.changed.notify_all();
        true
    }

    fn reject(&self, error: String) -> bool {
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        state.status.rejected_artifacts += 1;
        state.status.last_error = Some(error.clone());
        if !state.stopped {
            state.revision += 1;
        }
        self.shared.changed.notify_all();
        tracing::warn!(%error, capture_path = %self.shared.root.display(), "LLM debug capture incomplete: artifact not queued");
        false
    }

    pub(super) fn status(&self) -> WriterStatus {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
            .clone()
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        self.shared.close();
    }
}

impl Shared {
    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.status.closed {
            return;
        }
        state.status.closed = true;
        state.revision += 1;
        self.changed.notify_all();
    }

    fn run(&self, write: Box<WriteArtifact>) {
        let mut last_status_write = Instant::now();
        loop {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let report_status = state.persisted_revision < state.revision
                && (state.queue.is_empty()
                    || last_status_write.elapsed() >= Duration::from_secs(1));
            if !report_status && !state.queue.is_empty() {
                let artifact = state.queue.pop_front().unwrap();
                drop(state);
                let started = Instant::now();
                let queue_wait_ms = artifact.queued_at.elapsed().as_secs_f64() * 1000.0;
                let result = write(&artifact.path, &artifact.bytes, artifact.only_if_missing);
                let write_ms = started.elapsed().as_secs_f64() * 1000.0;
                state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.status.background_write_ms += write_ms;
                state.status.max_background_write_ms =
                    state.status.max_background_write_ms.max(write_ms);
                state.status.max_queue_wait_ms = state.status.max_queue_wait_ms.max(queue_wait_ms);
                state.status.completed_artifacts += 1;
                state.status.pending_artifacts -= 1;
                state.status.pending_bytes -= artifact.bytes.len();
                if let Err(error) = result {
                    state.status.failed_artifacts += 1;
                    state.status.last_error = Some(error.to_string());
                    tracing::warn!(%error, path = %artifact.path.display(), "LLM debug capture incomplete: background write failed");
                }
                state.revision += 1;
                self.changed.notify_all();
                continue;
            }
            if report_status {
                let revision = state.revision;
                let status = state.status.clone();
                drop(state);
                let result = serde_json::to_vec_pretty(&serde_json::json!({
                    "schema_version": 1,
                    "mode": "background_bounded",
                    "updated_at_unix_ms": std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(),
                    "max_pending_bytes": MAX_PENDING_BYTES,
                    "max_pending_artifacts": MAX_PENDING_ARTIFACTS,
                    "process_id": std::process::id(),
                    "status": status,
                }))
                .map_err(io::Error::other)
                .and_then(|bytes| {
                    atomic_write(
                        &self
                            .root
                            .join(format!("capture-writer-{}.json", status.writer_id)),
                        &bytes,
                        false,
                    )
                });
                state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Err(error) = result {
                    state.status.last_error = Some(error.to_string());
                    tracing::warn!(%error, capture_path = %self.root.display(), "failed to persist LLM capture writer status");
                }
                state.persisted_revision = revision;
                state.reported_completed_artifacts = status.completed_artifacts;
                last_status_write = Instant::now();
                self.changed.notify_all();
                continue;
            }
            if state.status.closed {
                state.stopped = true;
                self.changed.notify_all();
                return;
            }
            drop(self.changed.wait(state).unwrap_or_else(|e| e.into_inner()));
        }
    }

    fn flush(&self, deadline: Instant) -> io::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let accepted = state.status.accepted_artifacts;
        let revision = state.revision;
        // A closed drain is terminal: no worker may persist late rejection status afterwards.
        while state.reported_completed_artifacts < accepted
            || state.persisted_revision < revision
            || (state.status.closed && !state.stopped)
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "LLM capture drain timed out",
                ));
            }
            state = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if let Some(error) = &state.status.last_error {
            return Err(io::Error::other(format!("LLM capture incomplete: {error}")));
        }
        Ok(())
    }
}

/// Wait for accepted artifacts, including atomic file and parent-directory fsyncs.
/// This is an explicit off-hot-path barrier, not part of recording an artifact.
pub fn flush_capture(path: &Path, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    for writer in live_writers()
        .into_iter()
        .filter(|writer| writer.root == path)
    {
        writer.flush(deadline)?;
    }
    Ok(())
}

/// Called after producers stop during process shutdown. Late writes are rejected,
/// never silently accepted after the drain barrier. All captures share one deadline.
pub(super) fn shutdown_capture_writers(timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    let writers = live_writers();
    for writer in &writers {
        writer.close();
    }
    let mut result = Ok(());
    for writer in writers {
        if let Err(error) = writer.flush(deadline) {
            result = Err(error);
        }
    }
    result
}

pub(super) async fn stop_samplers(deadline: tokio::time::Instant) -> io::Result<()> {
    let handles: Vec<_> = writers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(|entry| entry.handle.upgrade())
        .collect();
    for handle in handles {
        handle.samplers.close();
        tokio::time::timeout_at(deadline, handle.samplers.wait())
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "LLM capture sampler shutdown timed out",
                )
            })?;
    }
    Ok(())
}

fn live_writers() -> Vec<Arc<Shared>> {
    writers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|entry| entry.shared.clone())
        .collect()
}

// Do not allocate arbitrarily large capture payloads before checking the byte bound.
struct LimitedBuffer(Vec<u8>);

impl Write for LimitedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_PENDING_BYTES {
            return Err(io::Error::other(
                "capture artifact exceeds backlog byte limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn atomic_write(path: &Path, bytes: &[u8], only_if_missing: bool) -> io::Result<()> {
    // Checked on the ordered worker, not before enqueue: a real projection may
    // already be queued but not yet visible on disk.
    if only_if_missing && path.try_exists()? {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("artifact has no parent"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
