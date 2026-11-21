use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::services::chat_transfer::{ChatTransferAuthorization, ChatTransferContext};
use crate::services::device_command_bus::{
    deliver_sync_transfer_device_command_via_session, list_device_directory,
    SyncTransferDeviceAction, SyncTransferDeviceCommand, SyncTransferDeviceResponse,
    SyncTransferDirectoryListing, SyncTransferFileFingerprint,
};
use crate::services::foreground_operation::ForegroundOperationRegistry;
use rua_core::{ChatAddress, RunControl};
use rua_session::chat_media::ChatMediaManifest;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const TRANSFER_RECOVERY_ATTEMPTS: usize = 3;
const TRANSFER_RETRY_BACKOFF: Duration = Duration::from_millis(100);
const TRANSFER_IO_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSFER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSFER_OPERATION_TTL: Duration = Duration::from_secs(10 * 60);
const SOURCE_TRANSFER_CONTROL_TTL: Duration = Duration::from_secs(30);
const MAX_TRANSFER_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_SYNC_BLOB_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TRANSFER_FILES: usize = 512;
const MAX_TRANSFER_FRAME_BYTES: usize = 2 * 1024 * 1024;
static TRANSFER_COMMIT_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub struct SyncTransferListClient {
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferManifest {
    pub entries: Vec<TransferManifestEntry>,
    pub total_bytes: u64,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferManifestEntry {
    /// Canonical source-device path captured while constructing the manifest.
    pub source_path: PathBuf,
    pub relative_path: PathBuf,
    pub kind: TransferManifestEntryKind,
    pub size: u64,
    pub sha256: String,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferManifestEntryKind {
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveConflictPolicy {
    Rename,
    Fail,
    Overwrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivePlan {
    pub target_dir: PathBuf,
    pub entries: Vec<ReceivePlanEntry>,
    pub conflict_policy: ReceiveConflictPolicy,
    root_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivePlanEntry {
    pub manifest_index: usize,
    pub target_path: PathBuf,
    pub temp_path: PathBuf,
    pub final_relative_path: PathBuf,
    expected_target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferExecutionOptions {
    pub chunk_size: usize,
}

impl Default for TransferExecutionOptions {
    fn default() -> Self {
        Self {
            chunk_size: MAX_TRANSFER_CHUNK_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferExecutionProgress {
    pub manifest_index: usize,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub completed_files: Vec<TransferFileOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferExecutionOutcome {
    pub files_written: usize,
    pub bytes_written: u64,
    pub files: Vec<TransferFileOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferFileOutcome {
    pub target_path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub overwritten: bool,
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferJobStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferJobSnapshot {
    pub job_id: String,
    pub status: TransferJobStatus,
    pub source_device_id: String,
    pub target_device_id: String,
    pub target_dir: PathBuf,
    pub files_written: usize,
    pub bytes_written: u64,
    pub total_bytes: u64,
    #[serde(default)]
    pub files: Vec<TransferFileOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct TransferJobState {
    snapshot: TransferJobSnapshot,
    cancel_tx: tokio::sync::watch::Sender<RunControl>,
    foreground_lease_id: Option<String>,
}

pub struct TransferJobStore {
    jobs: Mutex<HashMap<String, TransferJobState>>,
    foreground_operations: Option<Arc<ForegroundOperationRegistry>>,
}

impl TransferJobStore {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            foreground_operations: None,
        }
    }

    pub fn with_foreground_operations(
        foreground_operations: Arc<ForegroundOperationRegistry>,
    ) -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            foreground_operations: Some(foreground_operations),
        }
    }

    pub fn start(
        &self,
        source_device_id: String,
        target_device_id: String,
        target_dir: PathBuf,
        total_bytes: u64,
    ) -> (
        TransferJobSnapshot,
        tokio::sync::watch::Receiver<RunControl>,
    ) {
        let job_id = format!("transfer-{}", uuid::Uuid::new_v4());
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
        let foreground_lease_id = self.foreground_operations.as_ref().and_then(|registry| {
            let lease_id = format!("job:{job_id}");
            registry
                .acquire_transfer(
                    lease_id.clone(),
                    job_id.clone(),
                    source_device_id.clone(),
                    target_device_id.clone(),
                    total_bytes,
                )
                .then_some(lease_id)
        });
        let snapshot = TransferJobSnapshot {
            job_id: job_id.clone(),
            status: TransferJobStatus::Running,
            source_device_id,
            target_device_id,
            target_dir,
            files_written: 0,
            bytes_written: 0,
            total_bytes,
            files: Vec::new(),
            error: None,
        };
        self.jobs.lock().expect("transfer jobs lock").insert(
            job_id,
            TransferJobState {
                snapshot: snapshot.clone(),
                cancel_tx,
                foreground_lease_id,
            },
        );
        (snapshot, cancel_rx)
    }

    pub fn update_progress(&self, job_id: &str, bytes_written: u64) {
        let foreground_update = {
            let mut jobs = self.jobs.lock().expect("transfer jobs lock");
            let Some(job) = jobs.get_mut(job_id) else {
                return;
            };
            job.snapshot.bytes_written = bytes_written;
            job.foreground_lease_id
                .clone()
                .map(|lease_id| (lease_id, job.snapshot.total_bytes))
        };
        if let (Some(registry), Some((lease_id, total_bytes))) =
            (&self.foreground_operations, foreground_update)
        {
            registry.update_transfer(&lease_id, bytes_written, total_bytes);
        }
    }

    pub fn update_total_bytes(&self, job_id: &str, total_bytes: u64) {
        let foreground_update = {
            let mut jobs = self.jobs.lock().expect("transfer jobs lock");
            let Some(job) = jobs.get_mut(job_id) else {
                return;
            };
            job.snapshot.total_bytes = total_bytes;
            job.foreground_lease_id
                .clone()
                .map(|lease_id| (lease_id, job.snapshot.bytes_written))
        };
        if let (Some(registry), Some((lease_id, bytes_written))) =
            (&self.foreground_operations, foreground_update)
        {
            registry.update_transfer(&lease_id, bytes_written, total_bytes);
        }
    }

    pub fn update_progress_with_files(&self, job_id: &str, progress: &TransferExecutionProgress) {
        self.update_progress(job_id, progress.bytes_transferred);
        if let Some(job) = self
            .jobs
            .lock()
            .expect("transfer jobs lock")
            .get_mut(job_id)
        {
            merge_completion_receipts(&mut job.snapshot.files, &progress.completed_files);
            job.snapshot.files_written = job.snapshot.files.len();
        }
    }

    pub fn succeed(&self, job_id: &str, outcome: TransferExecutionOutcome) {
        let foreground_lease_id = {
            let mut jobs = self.jobs.lock().expect("transfer jobs lock");
            let Some(job) = jobs.get_mut(job_id) else {
                return;
            };
            if job.snapshot.status != TransferJobStatus::Running {
                return;
            }
            job.snapshot.status = TransferJobStatus::Succeeded;
            job.snapshot.files_written = outcome.files_written;
            job.snapshot.bytes_written = outcome.bytes_written;
            job.snapshot.files = outcome.files;
            job.snapshot.error = None;
            job.foreground_lease_id.take()
        };
        if let (Some(registry), Some(lease_id)) = (&self.foreground_operations, foreground_lease_id)
        {
            registry.release(&lease_id);
        }
    }

    pub fn fail(&self, job_id: &str, error: String) {
        self.finish_error(job_id, error, TransferJobStatus::Failed);
    }

    pub fn mark_unknown(&self, job_id: &str, error: String) {
        self.finish_error(job_id, error, TransferJobStatus::Unknown);
    }

    fn finish_error(&self, job_id: &str, error: String, status: TransferJobStatus) {
        let foreground_lease_id = {
            let mut jobs = self.jobs.lock().expect("transfer jobs lock");
            let Some(job) = jobs.get_mut(job_id) else {
                return;
            };
            if job.snapshot.status != TransferJobStatus::Running {
                return;
            }
            job.snapshot.status = status;
            job.snapshot.error = Some(error);
            job.foreground_lease_id.take()
        };
        if let (Some(registry), Some(lease_id)) = (&self.foreground_operations, foreground_lease_id)
        {
            registry.release(&lease_id);
        }
    }

    pub fn cancel(&self, job_id: &str) -> Option<TransferJobSnapshot> {
        let (snapshot, foreground_lease_id) = {
            let mut jobs = self.jobs.lock().expect("transfer jobs lock");
            let job = jobs.get_mut(job_id)?;
            let _ = job.cancel_tx.send(RunControl::Stop);
            let foreground_lease_id = if job.snapshot.status == TransferJobStatus::Running {
                job.snapshot.status = TransferJobStatus::Cancelled;
                job.foreground_lease_id.take()
            } else {
                None
            };
            (job.snapshot.clone(), foreground_lease_id)
        };
        if let (Some(registry), Some(lease_id)) = (&self.foreground_operations, foreground_lease_id)
        {
            registry.release(&lease_id);
        }
        Some(snapshot)
    }

    pub fn get(&self, job_id: &str) -> Option<TransferJobSnapshot> {
        self.jobs
            .lock()
            .expect("transfer jobs lock")
            .get(job_id)
            .map(|job| job.snapshot.clone())
    }

    pub fn list(&self) -> Vec<TransferJobSnapshot> {
        let mut jobs = self
            .jobs
            .lock()
            .expect("transfer jobs lock")
            .values()
            .map(|job| job.snapshot.clone())
            .collect::<Vec<_>>();
        jobs.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        jobs
    }
}

impl Default for TransferJobStore {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct TransferStreamSendRequest {
    pub operation_id: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub manifest: TransferManifest,
    pub target_dir: PathBuf,
    pub conflict_policy: ReceiveConflictPolicy,
    pub options: TransferExecutionOptions,
    pub chat_context: Option<ChatTransferContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TransferStreamHeader {
    operation_id: String,
    source_device_id: String,
    target_device_id: String,
    conflict_policy: TransferStreamConflictPolicy,
    target_dir: PathBuf,
    entries: Vec<TransferStreamEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chat_context: Option<ChatTransferContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TransferStreamEntry {
    relative_path: PathBuf,
    size: u64,
    sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransferStreamConflictPolicy {
    Rename,
    Fail,
    Overwrite,
}

impl From<ReceiveConflictPolicy> for TransferStreamConflictPolicy {
    fn from(value: ReceiveConflictPolicy) -> Self {
        match value {
            ReceiveConflictPolicy::Rename => Self::Rename,
            ReceiveConflictPolicy::Fail => Self::Fail,
            ReceiveConflictPolicy::Overwrite => Self::Overwrite,
        }
    }
}

impl From<TransferStreamConflictPolicy> for ReceiveConflictPolicy {
    fn from(value: TransferStreamConflictPolicy) -> Self {
        match value {
            TransferStreamConflictPolicy::Rename => Self::Rename,
            TransferStreamConflictPolicy::Fail => Self::Fail,
            TransferStreamConflictPolicy::Overwrite => Self::Overwrite,
        }
    }
}

impl TransferStreamHeader {
    fn from_manifest(
        operation_id: String,
        source_device_id: String,
        target_device_id: String,
        manifest: &TransferManifest,
        target_dir: PathBuf,
        conflict_policy: ReceiveConflictPolicy,
    ) -> Self {
        Self {
            operation_id,
            source_device_id,
            target_device_id,
            conflict_policy: conflict_policy.into(),
            target_dir,
            entries: manifest
                .entries
                .iter()
                .map(|entry| TransferStreamEntry {
                    relative_path: entry.relative_path.clone(),
                    size: entry.size,
                    sha256: entry.sha256.clone(),
                })
                .collect(),
            chat_context: None,
        }
    }

    fn to_receive_manifest(&self) -> Result<TransferManifest, SyncTransferPlanError> {
        check_manifest_count(self.entries.len())?;
        let mut entries = Vec::with_capacity(self.entries.len());
        let mut total_bytes: u64 = 0;
        for entry in &self.entries {
            validate_relative_path(&entry.relative_path)?;
            if !valid_digest(&entry.sha256) {
                return Err(SyncTransferPlanError::IntegrityMismatch(
                    entry.relative_path.clone(),
                ));
            }
            total_bytes = total_bytes.checked_add(entry.size).ok_or_else(|| {
                SyncTransferPlanError::Remote("transfer manifest size overflow".to_string())
            })?;
            entries.push(TransferManifestEntry {
                source_path: PathBuf::new(),
                relative_path: entry.relative_path.clone(),
                kind: TransferManifestEntryKind::File,
                size: entry.size,
                sha256: entry.sha256.clone(),
                extra: Default::default(),
            });
        }
        Ok(TransferManifest {
            entries,
            total_bytes,
            extra: Default::default(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "purpose", rename_all = "snake_case")]
enum ChatMediaRead {
    Content,
    Metadata,
    FirstSubmission { project_id: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TransferStreamRequest {
    Open {
        header: Box<TransferStreamHeader>,
        #[serde(default)]
        resume_only: bool,
    },
    Chunk {
        operation_id: String,
        manifest_index: usize,
        offset: u64,
        len: usize,
    },
    Finish {
        operation_id: String,
    },
    Abort {
        operation_id: String,
    },
    MediaRead {
        address: ChatAddress,
        media_id: String,
        purpose: ChatMediaRead,
    },
    SyncBlobRead {
        hash: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TransferStreamResponse {
    Resume {
        operation_id: String,
        manifest_index: usize,
        offset: u64,
        files_written: usize,
        bytes_written: u64,
        complete: bool,
        completed_files: Vec<TransferFileOutcome>,
    },
    Acknowledged {
        operation_id: String,
        manifest_index: usize,
        offset: u64,
        files_written: usize,
        bytes_written: u64,
        complete: bool,
        completed_files: Vec<TransferFileOutcome>,
    },
    Complete {
        operation_id: String,
        files_written: usize,
        bytes_written: u64,
        files: Vec<TransferFileOutcome>,
    },
    Aborted {
        operation_id: String,
    },
    MediaManifest {
        manifest: Box<ChatMediaManifest>,
    },
    MediaMissing,
    MediaReadReady {
        mime_type: String,
        byte_size: u64,
    },
    MediaReadChunk {
        offset: u64,
        len: usize,
    },
    MediaReadComplete,
    SyncBlobReadReady {
        byte_size: u64,
    },
    SyncBlobReadChunk {
        offset: u64,
        len: usize,
    },
    SyncBlobReadComplete,
    Error {
        detail: String,
    },
    OutcomeUnknown {
        operation_id: String,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTransferOperationSnapshot {
    pub operation_id: String,
    pub status: TransferJobStatus,
    pub files_written: usize,
    pub bytes_written: u64,
    pub total_bytes: u64,
    #[serde(default)]
    pub files: Vec<TransferFileOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct SourceTransferOperationState {
    snapshot: SourceTransferOperationSnapshot,
    cancel_tx: tokio::sync::watch::Sender<RunControl>,
    last_control_at: Instant,
    foreground_lease_id: Option<String>,
    started: bool,
}

pub struct SourceTransferOperationStore {
    operations: Mutex<HashMap<String, SourceTransferOperationState>>,
    foreground_operations: Option<Arc<ForegroundOperationRegistry>>,
}

struct ReceiveTransferOperation {
    header: TransferStreamHeader,
    manifest: TransferManifest,
    plan: ReceivePlan,
    manifest_index: usize,
    offset: u64,
    files_written: usize,
    bytes_written: u64,
    files: Vec<TransferFileOutcome>,
    outcome_unknown: Option<String>,
    complete: bool,
    active: bool,
    in_flight: bool,
    prepared: bool,
    confirmed_invocation: bool,
    cancelled: bool,
    last_used: Instant,
    foreground_lease_id: Option<String>,
}

#[derive(Clone)]
pub struct SyncTransferBulkHandler {
    local_device_id: String,
    operations: Arc<Mutex<HashMap<String, ReceiveTransferOperation>>>,
    open_lock: Arc<tokio::sync::Mutex<()>>,
    foreground_operations: Option<Arc<ForegroundOperationRegistry>>,
    media_transfers: Option<Arc<crate::services::chat_media_transfer::ChatMediaTransferRegistry>>,
    chat_media_root: Option<PathBuf>,
    sync_blobs: Option<rua_sync::endpoint::BlobsApiStore>,
    chat_authorization: Option<Arc<dyn ChatTransferAuthorization>>,
}

struct ReceiveOperationGuard {
    operation_id: String,
    operations: Arc<Mutex<HashMap<String, ReceiveTransferOperation>>>,
}

struct ReceiveChunkGuard {
    operation_id: String,
    operations: Arc<Mutex<HashMap<String, ReceiveTransferOperation>>>,
}

struct PreparedReceiveChunk {
    plan: ReceivePlan,
    entry: ReceivePlanEntry,
    manifest: TransferManifestEntry,
    parent: TransferParent,
    file: std::fs::File,
    fingerprint: Option<String>,
    context: Option<ChatTransferContext>,
    confirmed: bool,
    _guard: ReceiveChunkGuard,
}

impl Drop for ReceiveChunkGuard {
    fn drop(&mut self) {
        if let Some(operation) = self
            .operations
            .lock()
            .expect("receive operations lock")
            .get_mut(&self.operation_id)
        {
            operation.in_flight = false;
        }
    }
}

#[derive(Debug)]
pub enum SyncTransferPlanError {
    MissingSource(PathBuf),
    UnsupportedSource(PathBuf),
    SourceMetadata {
        path: PathBuf,
        source: std::io::Error,
    },
    WalkDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidRelativePath(PathBuf),
    TargetConflict(PathBuf),
    TargetNotDirectory(PathBuf),
    TargetChanged(PathBuf),
    IntegrityMismatch(PathBuf),
    OutcomeUnknown {
        operation_id: String,
        detail: String,
    },
    Cancelled,
    Remote(String),
    RecoveryExhausted(String),
    Io(std::io::Error),
}

impl std::fmt::Display for SyncTransferPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSource(path) => {
                write!(f, "source path does not exist: {}", path.display())
            }
            Self::UnsupportedSource(path) => {
                write!(f, "unsupported source path type: {}", path.display())
            }
            Self::SourceMetadata { path, source } => {
                write!(f, "read source metadata {}: {source}", path.display())
            }
            Self::WalkDirectory { path, source } => {
                write!(f, "walk source directory {}: {source}", path.display())
            }
            Self::InvalidRelativePath(path) => {
                write!(f, "invalid relative transfer path: {}", path.display())
            }
            Self::TargetConflict(path) => {
                write!(f, "target path already exists: {}", path.display())
            }
            Self::TargetNotDirectory(path) => {
                write!(f, "target directory is not a directory: {}", path.display())
            }
            Self::TargetChanged(path) => write!(
                f,
                "transfer target changed before commit: {}",
                path.display()
            ),
            Self::IntegrityMismatch(path) => {
                write!(f, "transfer size or SHA-256 mismatch: {}", path.display())
            }
            Self::OutcomeUnknown {
                operation_id,
                detail,
            } => write!(f, "transfer {operation_id} completion is unknown: {detail}"),
            Self::Cancelled => write!(f, "transfer cancelled"),
            Self::Remote(detail) => write!(f, "remote transfer rejected: {detail}"),
            Self::RecoveryExhausted(detail) => {
                write!(f, "transfer recovery exhausted: {detail}")
            }
            Self::Io(error) => write!(f, "io: {error}"),
        }
    }
}

impl std::error::Error for SyncTransferPlanError {}

impl From<std::io::Error> for SyncTransferPlanError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn build_transfer_manifest(
    source_paths: &[PathBuf],
) -> Result<TransferManifest, SyncTransferPlanError> {
    let mut manifest = build_transfer_manifest_metadata(source_paths)?;
    capture_transfer_digests(&mut manifest)?;
    Ok(manifest)
}

pub fn capture_transfer_digests(
    manifest: &mut TransferManifest,
) -> Result<(), SyncTransferPlanError> {
    capture_transfer_digests_inner(manifest, None)
}

pub fn capture_transfer_digests_with_cancel(
    manifest: &mut TransferManifest,
    cancel: &tokio::sync::watch::Receiver<RunControl>,
) -> Result<(), SyncTransferPlanError> {
    capture_transfer_digests_inner(manifest, Some(cancel))
}

fn capture_transfer_digests_inner(
    manifest: &mut TransferManifest,
    cancel: Option<&tokio::sync::watch::Receiver<RunControl>>,
) -> Result<(), SyncTransferPlanError> {
    for entry in &mut manifest.entries {
        if cancel.is_some_and(|cancel| cancel.borrow().is_terminal()) {
            return Err(SyncTransferPlanError::Cancelled);
        }
        let mut file = open_source_file(&entry.source_path)?;
        let before = metadata_fingerprint(&file.metadata()?);
        let digest = sha256_reader(&mut file, entry.size, &entry.source_path, cancel)?;
        if file.metadata()?.len() != entry.size || metadata_fingerprint(&file.metadata()?) != before
        {
            return Err(SyncTransferPlanError::IntegrityMismatch(
                entry.source_path.clone(),
            ));
        }
        entry.sha256 = digest;
    }
    Ok(())
}

pub fn build_transfer_manifest_metadata(
    source_paths: &[PathBuf],
) -> Result<TransferManifest, SyncTransferPlanError> {
    let mut entries = Vec::new();
    for source_path in source_paths {
        let metadata = std::fs::symlink_metadata(source_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SyncTransferPlanError::MissingSource(source_path.clone())
            } else {
                SyncTransferPlanError::SourceMetadata {
                    path: source_path.clone(),
                    source: error,
                }
            }
        })?;
        if metadata.is_file() {
            let relative_path = source_path
                .file_name()
                .map(PathBuf::from)
                .ok_or_else(|| SyncTransferPlanError::InvalidRelativePath(source_path.clone()))?;
            push_manifest_file(&mut entries, source_path, relative_path, metadata.len())?;
        } else if metadata.is_dir() {
            let base_name = source_path
                .file_name()
                .map(PathBuf::from)
                .ok_or_else(|| SyncTransferPlanError::InvalidRelativePath(source_path.clone()))?;
            collect_directory_manifest_entries(source_path, &base_name, &mut entries)?;
        } else {
            return Err(SyncTransferPlanError::UnsupportedSource(
                source_path.clone(),
            ));
        }
    }

    entries.sort_by(|a, b| {
        a.relative_path
            .to_string_lossy()
            .cmp(&b.relative_path.to_string_lossy())
    });
    let total_bytes = entries.iter().map(|entry| entry.size).sum();
    Ok(TransferManifest {
        entries,
        total_bytes,
        extra: Default::default(),
    })
}

pub fn build_transfer_inventory(
    source_paths: &[PathBuf],
) -> Result<Vec<SyncTransferFileFingerprint>, SyncTransferPlanError> {
    build_transfer_manifest(source_paths)?
        .entries
        .into_iter()
        .map(|entry| {
            Ok(SyncTransferFileFingerprint {
                relative_path: entry.relative_path,
                size: entry.size,
                sha256: entry.sha256,
            })
        })
        .collect()
}

pub fn build_gap_transfer_manifest(
    source_paths: &[PathBuf],
    target_inventory: &[SyncTransferFileFingerprint],
) -> Result<TransferManifest, SyncTransferPlanError> {
    let mut manifest = build_transfer_manifest(source_paths)?;
    let target = target_inventory
        .iter()
        .map(|entry| {
            validate_relative_path(&entry.relative_path)?;
            Ok((
                entry.relative_path.clone(),
                (entry.size, entry.sha256.as_str()),
            ))
        })
        .collect::<Result<HashMap<_, _>, SyncTransferPlanError>>()?;
    manifest.entries.retain(|entry| {
        if is_chat_permission_file(&entry.relative_path) {
            return false;
        }
        let Some((target_size, target_sha256)) = target.get(&entry.relative_path) else {
            return true;
        };
        if *target_size != entry.size {
            return true;
        }
        entry.sha256 != *target_sha256
    });
    manifest.total_bytes = manifest.entries.iter().map(|entry| entry.size).sum();
    Ok(manifest)
}

// Chat rules have their own versioned Sync record. Copying a Session directory
// during history repair must not replace a newer rule or undo its revocation.
fn is_chat_permission_file(relative: &Path) -> bool {
    relative.components().count() == 2 && relative.ends_with("permission_rules.json")
}

pub fn plan_receive(
    target_dir: &Path,
    manifest: &TransferManifest,
    conflict_policy: ReceiveConflictPolicy,
) -> Result<ReceivePlan, SyncTransferPlanError> {
    check_manifest_count(manifest.entries.len())?;
    let target_dir = target_dir.canonicalize()?;
    if !target_dir.is_dir() {
        return Err(SyncTransferPlanError::TargetNotDirectory(target_dir));
    }
    let mut planned_targets = BTreeSet::new();
    let mut entries = Vec::with_capacity(manifest.entries.len());
    for (index, entry) in manifest.entries.iter().enumerate() {
        validate_relative_path(&entry.relative_path)?;
        let candidate = target_dir.join(&entry.relative_path);
        let target_path = resolve_conflict(candidate, conflict_policy, &planned_targets)?;
        let final_relative_path = target_path
            .strip_prefix(&target_dir)
            .map(PathBuf::from)
            .map_err(|_| SyncTransferPlanError::InvalidRelativePath(target_path.clone()))?;
        planned_targets.insert(target_path.clone());
        let expected_target = target_fingerprint(&target_path)?;
        let temp_path =
            target_path.with_file_name(format!(".{}.rua-transfer-tmp", uuid::Uuid::new_v4()));
        entries.push(ReceivePlanEntry {
            manifest_index: index,
            target_path,
            temp_path,
            final_relative_path,
            expected_target,
        });
    }
    let paths = entries
        .iter()
        .map(|entry| &entry.target_path)
        .collect::<Vec<_>>();
    let path_bytes = serde_json::to_vec(&paths)
        .map_err(|error| SyncTransferPlanError::Remote(error.to_string()))?
        .len();
    if path_bytes.saturating_add(entries.len() * 256 + 1024) > MAX_TRANSFER_FRAME_BYTES {
        return Err(SyncTransferPlanError::Remote(
            "transfer completion receipt would exceed the bounded frame; select fewer files".into(),
        ));
    }
    Ok(ReceivePlan {
        root_identity: directory_identity(&std::fs::metadata(&target_dir)?),
        target_dir,
        entries,
        conflict_policy,
    })
}

pub async fn execute_local_transfer_plan<F>(
    manifest: &TransferManifest,
    plan: &ReceivePlan,
    options: TransferExecutionOptions,
    cancel_rx: tokio::sync::watch::Receiver<RunControl>,
    on_progress: F,
) -> Result<TransferExecutionOutcome, SyncTransferPlanError>
where
    F: FnMut(TransferExecutionProgress),
{
    execute_local_transfer_plan_inner(manifest, plan, options, cancel_rx, on_progress, None).await
}

pub struct LocalTransferAuthorization<'a> {
    pub context: &'a ChatTransferContext,
    pub confirmed_invocation: bool,
    pub authorization: &'a dyn ChatTransferAuthorization,
}

pub async fn execute_local_chat_transfer_plan<F>(
    manifest: &TransferManifest,
    plan: &ReceivePlan,
    options: TransferExecutionOptions,
    cancel_rx: tokio::sync::watch::Receiver<RunControl>,
    on_progress: F,
    authorization: &LocalTransferAuthorization<'_>,
) -> Result<TransferExecutionOutcome, SyncTransferPlanError>
where
    F: FnMut(TransferExecutionProgress),
{
    execute_local_transfer_plan_inner(
        manifest,
        plan,
        options,
        cancel_rx,
        on_progress,
        Some(authorization),
    )
    .await
}

async fn execute_local_transfer_plan_inner<F>(
    manifest: &TransferManifest,
    plan: &ReceivePlan,
    options: TransferExecutionOptions,
    cancel_rx: tokio::sync::watch::Receiver<RunControl>,
    mut on_progress: F,
    authorization: Option<&LocalTransferAuthorization<'_>>,
) -> Result<TransferExecutionOutcome, SyncTransferPlanError>
where
    F: FnMut(TransferExecutionProgress),
{
    let mut bytes_written = 0;
    let mut files = Vec::new();
    for entry in &plan.entries {
        if cancel_rx.borrow().is_terminal() {
            return Err(SyncTransferPlanError::Cancelled);
        }
        let source_entry = manifest.entries.get(entry.manifest_index).ok_or_else(|| {
            SyncTransferPlanError::InvalidRelativePath(entry.final_relative_path.clone())
        })?;
        if !valid_digest(&source_entry.sha256) {
            return Err(SyncTransferPlanError::IntegrityMismatch(
                source_entry.source_path.clone(),
            ));
        }
        if let Some(authorization) = authorization {
            authorization
                .authorization
                .check_receive(
                    authorization.context,
                    &entry.target_path,
                    authorization.confirmed_invocation,
                )
                .await?;
        }
        let parent = TransferParent::open(plan, entry, true)?;
        let cleanup = TransferTempCleanup {
            parent,
            path: entry.temp_path.clone(),
            remove: true,
        };
        let mut temp = tokio::fs::File::from_std(cleanup.parent.open_temp(&entry.temp_path, true)?);
        let mut source = tokio::fs::File::from_std(open_source_file(&source_entry.source_path)?);
        let mut buffer = vec![0u8; options.chunk_size.clamp(1, MAX_TRANSFER_CHUNK_BYTES)];
        let mut file_bytes = 0_u64;
        loop {
            if cancel_rx.borrow().is_terminal() {
                return Err(SyncTransferPlanError::Cancelled);
            }
            let read = source.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            file_bytes += read as u64;
            if file_bytes > source_entry.size {
                return Err(SyncTransferPlanError::IntegrityMismatch(
                    source_entry.source_path.clone(),
                ));
            }
            temp.write_all(&buffer[..read]).await?;
            bytes_written += read as u64;
            on_progress(TransferExecutionProgress {
                manifest_index: entry.manifest_index,
                bytes_transferred: bytes_written,
                total_bytes: manifest.total_bytes,
                completed_files: Vec::new(),
            });
        }
        temp.flush().await?;
        if cancel_rx.borrow().is_terminal() {
            return Err(SyncTransferPlanError::Cancelled);
        }
        let mut file = temp.into_std().await;
        let verifying_entry = source_entry.clone();
        let (file, fingerprint, cleanup) = tokio::task::spawn_blocking(move || {
            let fingerprint = verify_received_file(&mut file, &verifying_entry)?;
            Ok::<_, SyncTransferPlanError>((file, fingerprint, cleanup))
        })
        .await
        .map_err(|error| SyncTransferPlanError::Remote(error.to_string()))??;
        if let Some(authorization) = authorization {
            authorization
                .authorization
                .check_receive(
                    authorization.context,
                    &entry.target_path,
                    authorization.confirmed_invocation,
                )
                .await?;
        }
        let plan = plan.clone();
        let manifest_index = entry.manifest_index;
        let entry = entry.clone();
        let source_entry = source_entry.clone();
        let cancel = cancel_rx.clone();
        // Publication and directory fsync finish even if the caller is dropped.
        let outcome = tokio::task::spawn_blocking(move || {
            if cancel.borrow().is_terminal() {
                return Err(SyncTransferPlanError::Cancelled);
            }
            commit_received_file(
                &plan,
                &entry,
                &source_entry,
                &cleanup.parent,
                &file,
                &fingerprint,
            )
        })
        .await
        .map_err(|error| SyncTransferPlanError::OutcomeUnknown {
            operation_id: String::new(),
            detail: error.to_string(),
        })??;
        on_progress(TransferExecutionProgress {
            manifest_index,
            bytes_transferred: bytes_written,
            total_bytes: manifest.total_bytes,
            completed_files: vec![outcome.clone()],
        });
        files.push(outcome);
    }
    Ok(TransferExecutionOutcome {
        files_written: files.len(),
        bytes_written,
        files,
    })
}

impl SourceTransferOperationStore {
    pub fn new() -> Self {
        Self {
            operations: Mutex::new(HashMap::new()),
            foreground_operations: None,
        }
    }

    pub fn with_foreground_operations(
        foreground_operations: Arc<ForegroundOperationRegistry>,
    ) -> Self {
        Self {
            operations: Mutex::new(HashMap::new()),
            foreground_operations: Some(foreground_operations),
        }
    }

    pub fn start(
        &self,
        operation_id: String,
        total_bytes: u64,
        source_device_id: String,
        target_device_id: String,
    ) -> Result<
        (
            SourceTransferOperationSnapshot,
            Option<tokio::sync::watch::Receiver<RunControl>>,
        ),
        String,
    > {
        if operation_id.trim().is_empty() {
            return Err("transfer operation id must not be empty".to_string());
        }
        let mut operations = self.operations.lock().expect("source operations lock");
        operations.retain(|_, operation| {
            operation.started || operation.last_control_at.elapsed() < SOURCE_TRANSFER_CONTROL_TTL
        });
        if let Some(operation) = operations.get(&operation_id) {
            return Ok((operation.snapshot.clone(), None));
        }
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
        let foreground_lease_id = self.foreground_operations.as_ref().and_then(|registry| {
            let lease_id = format!("source:{operation_id}");
            registry
                .acquire_transfer(
                    lease_id.clone(),
                    operation_id.clone(),
                    source_device_id,
                    target_device_id,
                    total_bytes,
                )
                .then_some(lease_id)
        });
        let snapshot = SourceTransferOperationSnapshot {
            operation_id: operation_id.clone(),
            status: TransferJobStatus::Running,
            files_written: 0,
            bytes_written: 0,
            total_bytes,
            files: Vec::new(),
            error: None,
        };
        operations.insert(
            operation_id,
            SourceTransferOperationState {
                snapshot: snapshot.clone(),
                cancel_tx,
                last_control_at: Instant::now(),
                foreground_lease_id,
                started: true,
            },
        );
        Ok((snapshot, Some(cancel_rx)))
    }

    pub fn update_progress(&self, operation_id: &str, bytes_written: u64) {
        let foreground_update = {
            let mut operations = self.operations.lock().expect("source operations lock");
            let Some(operation) = operations.get_mut(operation_id) else {
                return;
            };
            operation.snapshot.bytes_written = bytes_written;
            operation
                .foreground_lease_id
                .clone()
                .map(|lease_id| (lease_id, operation.snapshot.total_bytes))
        };
        if let (Some(registry), Some((lease_id, total_bytes))) =
            (&self.foreground_operations, foreground_update)
        {
            registry.update_transfer(&lease_id, bytes_written, total_bytes);
        }
    }

    pub fn update_progress_with_files(
        &self,
        operation_id: &str,
        progress: &TransferExecutionProgress,
    ) {
        self.update_progress(operation_id, progress.bytes_transferred);
        if let Some(operation) = self
            .operations
            .lock()
            .expect("source operations lock")
            .get_mut(operation_id)
        {
            merge_completion_receipts(&mut operation.snapshot.files, &progress.completed_files);
            operation.snapshot.files_written = operation.snapshot.files.len();
        }
    }

    pub fn succeed(&self, operation_id: &str, outcome: TransferExecutionOutcome) {
        let foreground_lease_id = {
            let mut operations = self.operations.lock().expect("source operations lock");
            let Some(operation) = operations.get_mut(operation_id) else {
                return;
            };
            if operation.snapshot.status != TransferJobStatus::Running {
                return;
            }
            operation.snapshot.status = TransferJobStatus::Succeeded;
            operation.snapshot.files_written = outcome.files_written;
            operation.snapshot.bytes_written = outcome.bytes_written;
            operation.snapshot.files = outcome.files;
            operation.snapshot.error = None;
            operation.foreground_lease_id.take()
        };
        if let (Some(registry), Some(lease_id)) = (&self.foreground_operations, foreground_lease_id)
        {
            registry.release(&lease_id);
        }
    }

    pub fn fail(&self, operation_id: &str, error: String) {
        self.finish_error(operation_id, error, TransferJobStatus::Failed);
    }

    pub fn mark_unknown(&self, operation_id: &str, error: String) {
        self.finish_error(operation_id, error, TransferJobStatus::Unknown);
    }

    fn finish_error(&self, operation_id: &str, error: String, status: TransferJobStatus) {
        let foreground_lease_id = {
            let mut operations = self.operations.lock().expect("source operations lock");
            let Some(operation) = operations.get_mut(operation_id) else {
                return;
            };
            if operation.snapshot.status != TransferJobStatus::Running {
                return;
            }
            operation.snapshot.status = status;
            operation.snapshot.error = Some(error);
            operation.foreground_lease_id.take()
        };
        if let (Some(registry), Some(lease_id)) = (&self.foreground_operations, foreground_lease_id)
        {
            registry.release(&lease_id);
        }
    }

    pub fn mark_cancelled(&self, operation_id: &str) {
        self.finish_error(
            operation_id,
            "transfer cancelled".into(),
            TransferJobStatus::Cancelled,
        );
    }

    pub fn cancel(&self, operation_id: &str) -> Option<SourceTransferOperationSnapshot> {
        let mut operations = self.operations.lock().expect("source operations lock");
        let operation = operations
            .entry(operation_id.to_string())
            .or_insert_with(|| {
                let (cancel_tx, _) = tokio::sync::watch::channel(RunControl::Stop);
                SourceTransferOperationState {
                    snapshot: SourceTransferOperationSnapshot {
                        operation_id: operation_id.to_string(),
                        status: TransferJobStatus::Cancelled,
                        files_written: 0,
                        bytes_written: 0,
                        total_bytes: 0,
                        files: Vec::new(),
                        error: None,
                    },
                    cancel_tx,
                    last_control_at: Instant::now(),
                    foreground_lease_id: None,
                    started: false,
                }
            });
        let _ = operation.cancel_tx.send(RunControl::Stop);
        Some(operation.snapshot.clone())
    }

    pub fn get(&self, operation_id: &str) -> Option<SourceTransferOperationSnapshot> {
        let mut operations = self.operations.lock().expect("source operations lock");
        let operation = operations.get_mut(operation_id)?;
        operation.last_control_at = Instant::now();
        Some(operation.snapshot.clone())
    }

    pub fn is_running(&self, operation_id: &str) -> bool {
        self.operations
            .lock()
            .expect("source operations lock")
            .get(operation_id)
            .is_some_and(|operation| operation.snapshot.status == TransferJobStatus::Running)
    }

    pub fn cancel_if_control_expired(&self, operation_id: &str) -> bool {
        let mut operations = self.operations.lock().expect("source operations lock");
        let Some(operation) = operations.get_mut(operation_id) else {
            return false;
        };
        if operation.snapshot.status != TransferJobStatus::Running
            || operation.last_control_at.elapsed() < SOURCE_TRANSFER_CONTROL_TTL
        {
            return false;
        }
        let _ = operation.cancel_tx.send(RunControl::Stop);
        true
    }
}

impl Default for SourceTransferOperationStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncTransferBulkHandler {
    pub fn new(local_device_id: String) -> Self {
        Self {
            local_device_id,
            operations: Arc::new(Mutex::new(HashMap::new())),
            open_lock: Arc::new(tokio::sync::Mutex::new(())),
            foreground_operations: None,
            media_transfers: None,
            chat_media_root: None,
            sync_blobs: None,
            chat_authorization: None,
        }
    }

    pub fn with_media_transfers(
        mut self,
        media_transfers: Arc<crate::services::chat_media_transfer::ChatMediaTransferRegistry>,
    ) -> Self {
        self.media_transfers = Some(media_transfers);
        self
    }

    pub fn with_chat_media_root(mut self, chat_media_root: PathBuf) -> Self {
        self.chat_media_root = Some(chat_media_root);
        self
    }

    pub fn with_sync_blobs(mut self, sync_blobs: rua_sync::endpoint::BlobsApiStore) -> Self {
        self.sync_blobs = Some(sync_blobs);
        self
    }

    pub fn with_foreground_operations(
        mut self,
        foreground_operations: Arc<ForegroundOperationRegistry>,
    ) -> Self {
        self.foreground_operations = Some(foreground_operations);
        self
    }

    pub fn with_chat_authorization(
        mut self,
        authorization: Arc<dyn ChatTransferAuthorization>,
    ) -> Self {
        self.chat_authorization = Some(authorization);
        self
    }

    pub async fn prepare_chat_receive(
        &self,
        request: TransferStreamSendRequest,
        confirmed_invocation: bool,
    ) -> Result<ReceivePlan, SyncTransferPlanError> {
        let _open = self.open_lock.lock().await;
        self.prune_stale_operations().await;
        let context = request.chat_context.as_ref().ok_or_else(|| {
            SyncTransferPlanError::Remote("Chat transfer context required".into())
        })?;
        if context.operation_id != request.operation_id
            || request.target_device_id != self.local_device_id
        {
            return Err(SyncTransferPlanError::Remote(
                "Chat transfer preparation identity mismatch".into(),
            ));
        }
        let mut header = TransferStreamHeader::from_manifest(
            request.operation_id.clone(),
            request.source_device_id,
            request.target_device_id,
            &request.manifest,
            request.target_dir,
            request.conflict_policy,
        );
        header.chat_context = request.chat_context;
        if let Some(existing) = self
            .operations
            .lock()
            .expect("receive operations lock")
            .get(&request.operation_id)
        {
            if existing.header != header {
                return Err(SyncTransferPlanError::Remote(
                    "Chat transfer preparation changed".into(),
                ));
            }
            return Ok(existing.plan.clone());
        }
        let plan = plan_receive(
            &header.target_dir,
            &request.manifest,
            request.conflict_policy,
        )?;
        let authorization = self.chat_authorization.as_ref().ok_or_else(|| {
            SyncTransferPlanError::Remote("Chat transfer authorization unavailable".into())
        })?;
        for entry in &plan.entries {
            authorization
                .check_receive(
                    header.chat_context.as_ref().unwrap(),
                    &entry.target_path,
                    confirmed_invocation,
                )
                .await?;
        }
        self.operations
            .lock()
            .expect("receive operations lock")
            .insert(
                request.operation_id,
                ReceiveTransferOperation {
                    header,
                    manifest: request.manifest,
                    plan: plan.clone(),
                    manifest_index: 0,
                    offset: 0,
                    files_written: 0,
                    bytes_written: 0,
                    files: Vec::new(),
                    outcome_unknown: None,
                    complete: false,
                    active: false,
                    in_flight: false,
                    prepared: true,
                    confirmed_invocation,
                    cancelled: false,
                    last_used: Instant::now(),
                    foreground_lease_id: None,
                },
            );
        Ok(plan)
    }

    pub fn prepared_chat_plan(
        &self,
        context: &ChatTransferContext,
    ) -> Result<ReceivePlan, SyncTransferPlanError> {
        let operations = self.operations.lock().expect("receive operations lock");
        let operation = operations.get(&context.operation_id).ok_or_else(|| {
            SyncTransferPlanError::Remote("Chat receive preparation is unavailable".into())
        })?;
        if operation.header.chat_context.as_ref() != Some(context) {
            return Err(SyncTransferPlanError::Remote(
                "Chat transfer context changed".into(),
            ));
        }
        if operation.cancelled {
            return Err(SyncTransferPlanError::Cancelled);
        }
        Ok(operation.plan.clone())
    }

    pub async fn cancel_chat_receive(
        &self,
        context: &ChatTransferContext,
    ) -> Result<(), SyncTransferPlanError> {
        let _open = self.open_lock.lock().await;
        {
            let operations = self.operations.lock().expect("receive operations lock");
            if operations
                .get(&context.operation_id)
                .is_some_and(|operation| {
                    operation
                        .header
                        .chat_context
                        .as_ref()
                        .is_none_or(|prepared| !same_chat_invocation(prepared, context))
                })
            {
                return Err(SyncTransferPlanError::Remote(
                    "Chat transfer context changed".into(),
                ));
            }
        }
        self.abort_operation_inner(&context.operation_id).await;
        Ok(())
    }

    async fn open_operation(
        &self,
        peer_device_id: &str,
        mut header: TransferStreamHeader,
        resume_only: bool,
    ) -> Result<(ReceiveOperationGuard, TransferStreamResponse), SyncTransferPlanError> {
        let _open_guard = self.open_lock.lock().await;
        self.prune_stale_operations().await;
        if header.operation_id.trim().is_empty()
            || header.source_device_id.trim().is_empty()
            || header.target_device_id.trim().is_empty()
        {
            return Err(SyncTransferPlanError::Remote(
                "transfer identity fields must not be empty".to_string(),
            ));
        }
        if header.source_device_id != peer_device_id {
            return Err(SyncTransferPlanError::Remote(format!(
                "transfer source {} does not match authenticated peer {peer_device_id}",
                header.source_device_id
            )));
        }
        if header.target_device_id != self.local_device_id {
            return Err(SyncTransferPlanError::Remote(format!(
                "transfer target {} does not match this Device {}",
                header.target_device_id, self.local_device_id
            )));
        }
        if let Some(context) = &header.chat_context {
            if context.operation_id != header.operation_id
                || context
                    .call
                    .arguments
                    .get("source_device_id")
                    .and_then(serde_json::Value::as_str)
                    != Some(header.source_device_id.as_str())
                || context
                    .call
                    .arguments
                    .get("target_device_id")
                    .and_then(serde_json::Value::as_str)
                    != Some(header.target_device_id.as_str())
            {
                return Err(SyncTransferPlanError::Remote(
                    "Chat transfer identity does not match its ToolCall".into(),
                ));
            }
            let prepared = {
                let operations = self.operations.lock().expect("receive operations lock");
                let operation = operations.get(&header.operation_id).ok_or_else(|| SyncTransferPlanError::OutcomeUnknown { operation_id: header.operation_id.clone(), detail: "Chat receive preparation is unavailable; refusing a new destination plan".into() })?;
                operation.prepared.then(|| {
                    (
                        operation.header.clone(),
                        operation.plan.clone(),
                        operation.confirmed_invocation,
                    )
                })
            };
            if let Some((prepared_header, plan, confirmed)) = prepared {
                let mut metadata_header = header.clone();
                for entry in &mut metadata_header.entries {
                    entry.sha256.clear();
                }
                let mut expected_header = prepared_header;
                for entry in &mut expected_header.entries {
                    entry.sha256.clear();
                }
                if metadata_header != expected_header {
                    return Err(SyncTransferPlanError::Remote(
                        "Chat transfer manifest changed after preparation".into(),
                    ));
                }
                let manifest = header.to_receive_manifest()?;
                let authorization = self.chat_authorization.as_ref().ok_or_else(|| {
                    SyncTransferPlanError::Remote("Chat transfer authorization unavailable".into())
                })?;
                for entry in &plan.entries {
                    authorization
                        .check_receive(context, &entry.target_path, confirmed)
                        .await?;
                }
                prepare_temp_files(&plan)?;
                let mut operations = self.operations.lock().expect("receive operations lock");
                let operation = operations.get_mut(&header.operation_id).ok_or_else(|| {
                    SyncTransferPlanError::Remote("Chat preparation disappeared".into())
                })?;
                operation.header = header.clone();
                operation.manifest = manifest;
                operation.prepared = false;
                operation.complete = operation.manifest.entries.is_empty();
            }
        }
        let authorized_media = if let Some(registry) = &self.media_transfers {
            if header.entries.len() == 1 {
                registry
                    .validate_upload(
                        &header.operation_id,
                        &header.source_device_id,
                        &header.target_device_id,
                        &header.entries[0].relative_path,
                        header.entries[0].size,
                    )
                    .map_err(SyncTransferPlanError::Remote)?
            } else {
                None
            }
        } else {
            None
        };
        if header.operation_id.starts_with("chat-media-") && authorized_media.is_none() {
            return Err(SyncTransferPlanError::Remote(
                "Chat Media transfer is not authorized".to_string(),
            ));
        }
        if authorized_media.is_some()
            && !matches!(header.conflict_policy, TransferStreamConflictPolicy::Fail)
        {
            return Err(SyncTransferPlanError::Remote(
                "Media transfer must use fail-on-conflict policy".to_string(),
            ));
        }
        if let Some(upload) = authorized_media {
            header.target_dir = upload.staging.path().to_owned();
        }

        for _ in 0..40 {
            let active = self
                .operations
                .lock()
                .expect("receive operations lock")
                .get(&header.operation_id)
                .is_some_and(|operation| operation.active || operation.in_flight);
            if !active {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        if let Some(response) = {
            let mut operations = self.operations.lock().expect("receive operations lock");
            operations.get_mut(&header.operation_id).map(|operation| {
                if operation.cancelled {
                    return Err(SyncTransferPlanError::Cancelled);
                }
                if let Some(detail) = &operation.outcome_unknown {
                    return Err(SyncTransferPlanError::OutcomeUnknown {
                        operation_id: header.operation_id.clone(),
                        detail: detail.clone(),
                    });
                }
                if operation.header != header {
                    return Err(SyncTransferPlanError::Remote(
                        "transfer operation was reopened with a different manifest or target"
                            .to_string(),
                    ));
                }
                if operation.active || operation.in_flight {
                    return Err(SyncTransferPlanError::Remote(
                        "transfer operation already has an active Bulk stream".to_string(),
                    ));
                }
                operation.active = true;
                operation.last_used = Instant::now();
                if !operation.complete && operation.foreground_lease_id.is_none() {
                    operation.foreground_lease_id =
                        self.foreground_operations.as_ref().and_then(|registry| {
                            let lease_id = format!("receive:{}", header.operation_id);
                            registry
                                .acquire_transfer(
                                    lease_id.clone(),
                                    header.operation_id.clone(),
                                    header.source_device_id.clone(),
                                    header.target_device_id.clone(),
                                    operation.manifest.total_bytes,
                                )
                                .then_some(lease_id)
                        });
                }
                Ok(TransferStreamResponse::Resume {
                    operation_id: header.operation_id.clone(),
                    manifest_index: operation.manifest_index,
                    offset: operation.offset,
                    files_written: operation.files_written,
                    bytes_written: operation.bytes_written,
                    complete: operation.complete,
                    completed_files: operation.files.clone(),
                })
            })
        } {
            return Ok((
                ReceiveOperationGuard {
                    operation_id: header.operation_id,
                    operations: Arc::clone(&self.operations),
                },
                response?,
            ));
        }

        if resume_only {
            return Err(SyncTransferPlanError::OutcomeUnknown {
                operation_id: header.operation_id.clone(),
                detail: "receiver no longer has this operation; refusing to repeat its commits"
                    .to_string(),
            });
        }
        let manifest = header.to_receive_manifest()?;
        if header.operation_id.starts_with("conversation-history-gap-")
            && manifest
                .entries
                .iter()
                .any(|entry| is_chat_permission_file(&entry.relative_path))
        {
            return Err(SyncTransferPlanError::Remote(
                "Chat permission rules must use settings sync".into(),
            ));
        }
        let plan = plan_receive(&header.target_dir, &manifest, header.conflict_policy.into())?;
        prepare_temp_files(&plan)?;
        let operation_id = header.operation_id.clone();
        let complete = manifest.entries.is_empty();
        let foreground_lease_id = (!complete)
            .then(|| {
                self.foreground_operations.as_ref().and_then(|registry| {
                    let lease_id = format!("receive:{operation_id}");
                    registry
                        .acquire_transfer(
                            lease_id.clone(),
                            operation_id.clone(),
                            header.source_device_id.clone(),
                            header.target_device_id.clone(),
                            manifest.total_bytes,
                        )
                        .then_some(lease_id)
                })
            })
            .flatten();
        self.operations
            .lock()
            .expect("receive operations lock")
            .insert(
                operation_id.clone(),
                ReceiveTransferOperation {
                    header,
                    manifest,
                    plan,
                    manifest_index: 0,
                    offset: 0,
                    files_written: 0,
                    bytes_written: 0,
                    files: Vec::new(),
                    outcome_unknown: None,
                    complete,
                    active: true,
                    in_flight: false,
                    prepared: false,
                    confirmed_invocation: false,
                    cancelled: false,
                    last_used: Instant::now(),
                    foreground_lease_id,
                },
            );
        let cleanup_handler = self.clone();
        let cleanup_operation_id = operation_id.clone();
        tokio::spawn(async move {
            cleanup_handler
                .expire_operation_when_stale(cleanup_operation_id)
                .await;
        });
        Ok((
            ReceiveOperationGuard {
                operation_id: operation_id.clone(),
                operations: Arc::clone(&self.operations),
            },
            TransferStreamResponse::Resume {
                operation_id,
                manifest_index: 0,
                offset: 0,
                files_written: 0,
                bytes_written: 0,
                complete,
                completed_files: Vec::new(),
            },
        ))
    }

    async fn accept_chunk(
        &self,
        operation_id: &str,
        manifest_index: usize,
        offset: u64,
        payload: &[u8],
    ) -> Result<TransferStreamResponse, SyncTransferPlanError> {
        if payload.len() > MAX_TRANSFER_CHUNK_BYTES {
            return Err(SyncTransferPlanError::Remote(format!(
                "transfer chunk exceeds {MAX_TRANSFER_CHUNK_BYTES} bytes"
            )));
        }
        let operation_id = operation_id.to_string();
        let shared = Arc::clone(&self.operations);
        let writing_id = operation_id.clone();
        let payload_len = payload.len();
        let payload = payload.to_vec();
        let prepared = tokio::task::spawn_blocking(move || {
            let (plan, entry, manifest, context, confirmed) = {
                let mut operations = shared.lock().expect("receive operations lock");
                let operation = operations.get_mut(&writing_id).ok_or_else(|| {
                    SyncTransferPlanError::Remote("transfer operation not found".into())
                })?;
                if operation.complete
                    || operation.in_flight
                    || operation.manifest_index != manifest_index
                    || operation.offset != offset
                {
                    return Err(SyncTransferPlanError::Remote(
                        "transfer offset does not match the acknowledged cursor".into(),
                    ));
                }
                let manifest = operation
                    .manifest
                    .entries
                    .get(manifest_index)
                    .ok_or_else(|| {
                        SyncTransferPlanError::Remote("transfer manifest index out of range".into())
                    })?
                    .clone();
                if offset.saturating_add(payload.len() as u64) > manifest.size {
                    return Err(SyncTransferPlanError::Remote(
                        "transfer chunk exceeds declared file size".into(),
                    ));
                }
                let entry = operation
                    .plan
                    .entries
                    .get(manifest_index)
                    .ok_or_else(|| {
                        SyncTransferPlanError::Remote(
                            "transfer receive plan index out of range".into(),
                        )
                    })?
                    .clone();
                operation.in_flight = true;
                (
                    operation.plan.clone(),
                    entry,
                    manifest,
                    operation.header.chat_context.clone(),
                    operation.confirmed_invocation,
                )
            };
            let guard = ReceiveChunkGuard {
                operation_id: writing_id,
                operations: shared,
            };
            let parent = TransferParent::open(&plan, &entry, false)?;
            let mut file = parent.open_temp(&entry.temp_path, false)?;
            if file.metadata()?.len() < offset {
                return Err(SyncTransferPlanError::Remote(
                    "partial file is shorter than the acknowledged offset".into(),
                ));
            }
            file.set_len(offset)?;
            std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(offset))?;
            std::io::Write::write_all(&mut file, &payload)?;
            let fingerprint = if offset + payload.len() as u64 == manifest.size {
                match verify_received_file(&mut file, &manifest) {
                    Ok(fingerprint) => Some(fingerprint),
                    Err(error) => {
                        parent.remove_temp(&entry.temp_path);
                        return Err(error);
                    }
                }
            } else {
                None
            };
            Ok::<_, SyncTransferPlanError>(PreparedReceiveChunk {
                plan,
                entry,
                manifest,
                parent,
                file,
                fingerprint,
                context,
                confirmed,
                _guard: guard,
            })
        })
        .await
        .map_err(|error| SyncTransferPlanError::Remote(error.to_string()))??;
        // Recheck after potentially long hashing, immediately before publication.
        if prepared.fingerprint.is_some() {
            if let Some(context) = &prepared.context {
                self.chat_authorization
                    .as_ref()
                    .ok_or_else(|| {
                        SyncTransferPlanError::Remote(
                            "Chat transfer authorization unavailable".into(),
                        )
                    })?
                    .check_receive(context, &prepared.entry.target_path, prepared.confirmed)
                    .await?;
            }
        }
        let shared = Arc::clone(&self.operations);
        let unknown_id = operation_id.clone();
        let (response, foreground_update) = tokio::task::spawn_blocking(move || {
            // Publication and its completion receipt finish even if the stream is dropped.
            let _chunk_guard = prepared._guard;
            let mut operations = shared.lock().expect("receive operations lock");
            let operation = operations.get_mut(&operation_id).ok_or_else(|| {
                SyncTransferPlanError::OutcomeUnknown {
                    operation_id: operation_id.clone(),
                    detail: "receiver operation disappeared during publication".into(),
                }
            })?;
            if operation.cancelled {
                return Err(SyncTransferPlanError::Cancelled);
            }
            let committed = prepared.fingerprint.as_ref().map(|fingerprint| {
                commit_received_file(
                    &prepared.plan,
                    &prepared.entry,
                    &prepared.manifest,
                    &prepared.parent,
                    &prepared.file,
                    fingerprint,
                )
            });
            let mut completed_files = Vec::new();
            if let Some(committed) = committed {
                match committed {
                    Ok(file) => {
                        operation.files.push(file.clone());
                        completed_files.push(file);
                    }
                    Err(SyncTransferPlanError::OutcomeUnknown { detail, .. }) => {
                        operation.outcome_unknown = Some(detail.clone());
                        return Err(SyncTransferPlanError::OutcomeUnknown {
                            operation_id,
                            detail,
                        });
                    }
                    Err(error) => {
                        prepared.parent.remove_temp(&prepared.entry.temp_path);
                        return Err(error);
                    }
                }
            }
            operation.bytes_written += payload_len as u64;
            if prepared.fingerprint.is_some() {
                operation.files_written += 1;
                operation.manifest_index += 1;
                operation.offset = 0;
                operation.complete = operation.manifest_index == operation.manifest.entries.len();
            } else {
                operation.offset = offset + payload_len as u64;
            }
            operation.last_used = Instant::now();
            Ok((
                TransferStreamResponse::Acknowledged {
                    operation_id,
                    manifest_index: operation.manifest_index,
                    offset: operation.offset,
                    files_written: operation.files_written,
                    bytes_written: operation.bytes_written,
                    complete: operation.complete,
                    completed_files,
                },
                operation.foreground_lease_id.clone().map(|lease| {
                    (
                        lease,
                        operation.bytes_written,
                        operation.manifest.total_bytes,
                    )
                }),
            ))
        })
        .await
        .map_err(|error| SyncTransferPlanError::OutcomeUnknown {
            operation_id: unknown_id,
            detail: error.to_string(),
        })??;
        if let (Some(registry), Some((lease, written, total))) =
            (&self.foreground_operations, foreground_update)
        {
            registry.update_transfer(&lease, written, total);
        }
        Ok(response)
    }

    async fn abort_operation(&self, operation_id: &str) {
        let _open = self.open_lock.lock().await;
        self.abort_operation_inner(operation_id).await;
    }

    async fn abort_operation_inner(&self, operation_id: &str) {
        let operation = {
            let mut operations = self.operations.lock().expect("receive operations lock");
            if let Some(operation) = operations.get_mut(operation_id) {
                if operation.complete || operation.outcome_unknown.is_some() {
                    return;
                }
                operation.cancelled = true;
                if operation.in_flight {
                    return;
                }
            }
            operations.remove(operation_id)
        };
        if let Some(operation) = operation {
            if let (Some(registry), Some(lease_id)) = (
                &self.foreground_operations,
                operation.foreground_lease_id.as_deref(),
            ) {
                registry.release(lease_id);
            }
            for entry in &operation.plan.entries {
                if let Ok(parent) = TransferParent::open(&operation.plan, entry, false) {
                    parent.remove_temp(&entry.temp_path);
                }
            }
        }
    }

    async fn prune_stale_operations(&self) {
        let stale = {
            let mut operations = self.operations.lock().expect("receive operations lock");
            let stale_ids = operations
                .iter()
                .filter(|(_, operation)| {
                    !operation.active
                        && !operation.in_flight
                        && operation.last_used.elapsed() >= TRANSFER_OPERATION_TTL
                })
                .map(|(operation_id, _)| operation_id.clone())
                .collect::<Vec<_>>();
            stale_ids
                .into_iter()
                .filter_map(|operation_id| operations.remove(&operation_id))
                .collect::<Vec<_>>()
        };
        for operation in stale {
            if let (Some(registry), Some(lease_id)) = (
                &self.foreground_operations,
                operation.foreground_lease_id.as_deref(),
            ) {
                registry.release(lease_id);
            }
            for entry in &operation.plan.entries {
                if let Ok(parent) = TransferParent::open(&operation.plan, entry, false) {
                    parent.remove_temp(&entry.temp_path);
                }
            }
        }
    }

    async fn expire_operation_when_stale(&self, operation_id: String) {
        loop {
            tokio::time::sleep(TRANSFER_OPERATION_TTL).await;
            let _open_guard = self.open_lock.lock().await;
            let stale = self
                .operations
                .lock()
                .expect("receive operations lock")
                .get(&operation_id)
                .is_some_and(|operation| {
                    !operation.active
                        && !operation.in_flight
                        && operation.last_used.elapsed() >= TRANSFER_OPERATION_TTL
                });
            if stale {
                self.abort_operation_inner(&operation_id).await;
                return;
            }
            if !self
                .operations
                .lock()
                .expect("receive operations lock")
                .contains_key(&operation_id)
            {
                return;
            }
        }
    }
}

impl Drop for ReceiveOperationGuard {
    fn drop(&mut self) {
        if let Some(operation) = self
            .operations
            .lock()
            .expect("receive operations lock")
            .get_mut(&self.operation_id)
        {
            operation.active = false;
            operation.last_used = Instant::now();
        }
    }
}

#[async_trait::async_trait]
impl rua_device_network::DeviceSessionBulkTransferHandler for SyncTransferBulkHandler {
    async fn handle_bulk_transfer(
        &self,
        mut stream: rua_device_network::DeviceSessionBulkStream,
    ) -> Result<(), String> {
        let request: TransferStreamRequest = read_json_frame(stream.recv_mut())
            .await
            .map_err(|error| error.to_string())?;
        if let TransferStreamRequest::MediaRead {
            address,
            media_id,
            purpose,
        } = request
        {
            return self
                .handle_chat_media_read(stream, address, media_id, purpose)
                .await;
        }
        if let TransferStreamRequest::SyncBlobRead { hash } = request {
            return self.handle_sync_blob_read(stream, hash).await;
        }
        let TransferStreamRequest::Open {
            header,
            resume_only,
        } = request
        else {
            write_json_frame(
                stream.send_mut(),
                &TransferStreamResponse::Error {
                    detail: "Bulk Transfer stream must begin with open".to_string(),
                },
            )
            .await
            .map_err(|error| error.to_string())?;
            stream.finish().map_err(|error| error.to_string())?;
            return Ok(());
        };
        let operation_id = header.operation_id.clone();
        let (operation_guard, response) = match self
            .open_operation(&stream.peer_device_id(), *header, resume_only)
            .await
        {
            Ok(opened) => opened,
            Err(error) => {
                write_json_frame(stream.send_mut(), &transfer_error_response(error))
                    .await
                    .map_err(|error| error.to_string())?;
                stream.finish().map_err(|error| error.to_string())?;
                return Ok(());
            }
        };
        write_json_frame(stream.send_mut(), &response)
            .await
            .map_err(|error| error.to_string())?;

        loop {
            let request: TransferStreamRequest = match read_json_frame(stream.recv_mut()).await {
                Ok(request) => request,
                Err(_) => return Ok(()),
            };
            match request {
                TransferStreamRequest::Chunk {
                    operation_id: request_operation_id,
                    manifest_index,
                    offset,
                    len,
                } => {
                    if request_operation_id != operation_id {
                        write_transfer_error(&mut stream, "transfer operation id changed").await?;
                        return Ok(());
                    }
                    if len > MAX_TRANSFER_CHUNK_BYTES {
                        write_transfer_error(
                            &mut stream,
                            &format!("transfer chunk exceeds {MAX_TRANSFER_CHUNK_BYTES} bytes"),
                        )
                        .await?;
                        return Ok(());
                    }
                    let mut payload = vec![0; len];
                    if let Err(error) = stream.recv_mut().read_exact(&mut payload).await {
                        return Err(error.to_string());
                    }
                    match self
                        .accept_chunk(&operation_id, manifest_index, offset, payload.as_slice())
                        .await
                    {
                        Ok(response) => {
                            write_json_frame(stream.send_mut(), &response)
                                .await
                                .map_err(|error| error.to_string())?;
                        }
                        Err(error) => {
                            write_json_frame(stream.send_mut(), &transfer_error_response(error))
                                .await
                                .map_err(|error| error.to_string())?;
                            return Ok(());
                        }
                    }
                }
                TransferStreamRequest::Finish {
                    operation_id: request_operation_id,
                } => {
                    if request_operation_id != operation_id {
                        write_transfer_error(&mut stream, "transfer operation id changed").await?;
                        return Ok(());
                    }
                    let (outcome, foreground_lease_id) = {
                        let mut operations =
                            self.operations.lock().expect("receive operations lock");
                        let operation = operations.get_mut(&operation_id).ok_or_else(|| {
                            format!("transfer operation not found: {operation_id}")
                        })?;
                        (
                            operation.complete.then_some(TransferExecutionOutcome {
                                files_written: operation.files_written,
                                bytes_written: operation.bytes_written,
                                files: operation.files.clone(),
                            }),
                            if operation.complete {
                                operation.foreground_lease_id.take()
                            } else {
                                None
                            },
                        )
                    };
                    let Some(outcome) = outcome else {
                        write_transfer_error(
                            &mut stream,
                            "transfer finished before all manifest entries were acknowledged",
                        )
                        .await?;
                        return Ok(());
                    };
                    if let (Some(registry), Some(lease_id)) =
                        (&self.foreground_operations, foreground_lease_id)
                    {
                        registry.release(&lease_id);
                    }
                    if operation_id.starts_with("chat-media-") {
                        let registry = self
                            .media_transfers
                            .as_ref()
                            .ok_or("Media transfer is not authorized")?;
                        if let Err(detail) = registry.complete_upload(&operation_id).await {
                            registry.abort(&operation_id);
                            write_transfer_error(&mut stream, &detail).await?;
                            return Ok(());
                        }
                    }
                    write_json_frame(
                        stream.send_mut(),
                        &TransferStreamResponse::Complete {
                            operation_id: operation_id.clone(),
                            files_written: outcome.files_written,
                            bytes_written: outcome.bytes_written,
                            files: outcome.files,
                        },
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                    stream.finish().map_err(|error| error.to_string())?;
                    drop(operation_guard);
                    return Ok(());
                }
                TransferStreamRequest::Abort {
                    operation_id: request_operation_id,
                } => {
                    if request_operation_id != operation_id {
                        write_transfer_error(&mut stream, "transfer operation id changed").await?;
                        return Ok(());
                    }
                    drop(operation_guard);
                    self.abort_operation(&operation_id).await;
                    if let Some(registry) = &self.media_transfers {
                        registry.abort(&operation_id);
                    }
                    write_json_frame(
                        stream.send_mut(),
                        &TransferStreamResponse::Aborted {
                            operation_id: operation_id.clone(),
                        },
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                    stream.finish().map_err(|error| error.to_string())?;
                    return Ok(());
                }
                TransferStreamRequest::Open { .. } => {
                    write_transfer_error(&mut stream, "transfer stream was opened twice").await?;
                    return Ok(());
                }
                TransferStreamRequest::MediaRead { .. } => {
                    write_transfer_error(&mut stream, "Media read must begin a new stream").await?;
                    return Ok(());
                }
                TransferStreamRequest::SyncBlobRead { .. } => {
                    write_transfer_error(&mut stream, "Sync Blob read must begin a new stream")
                        .await?;
                    return Ok(());
                }
            }
        }
    }
}

impl SyncTransferBulkHandler {
    async fn handle_sync_blob_read(
        &self,
        mut stream: rua_device_network::DeviceSessionBulkStream,
        hash: String,
    ) -> Result<(), String> {
        let blobs = self
            .sync_blobs
            .as_ref()
            .ok_or_else(|| "Sync Blob read is unavailable".to_string())?;
        let hash: rua_sync::endpoint::BlobHash = hash
            .parse()
            .map_err(|error| format!("invalid Sync Blob hash: {error}"))?;
        let reader = blobs.reader(hash);
        let mut bytes = Vec::new();
        reader
            .take(MAX_SYNC_BLOB_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| format!("read Sync Blob: {error}"))?;
        if bytes.len() as u64 > MAX_SYNC_BLOB_BYTES {
            return Err(format!("Sync Blob exceeds {MAX_SYNC_BLOB_BYTES} bytes"));
        }
        let byte_size = bytes.len() as u64;
        write_json_frame(
            stream.send_mut(),
            &TransferStreamResponse::SyncBlobReadReady { byte_size },
        )
        .await
        .map_err(|error| error.to_string())?;
        let mut offset = 0u64;
        for chunk in bytes.chunks(MAX_TRANSFER_CHUNK_BYTES) {
            let len = chunk.len();
            write_json_frame(
                stream.send_mut(),
                &TransferStreamResponse::SyncBlobReadChunk { offset, len },
            )
            .await
            .map_err(|error| error.to_string())?;
            stream
                .send_mut()
                .write_all(chunk)
                .await
                .map_err(|error| error.to_string())?;
            offset += len as u64;
        }
        write_json_frame(
            stream.send_mut(),
            &TransferStreamResponse::SyncBlobReadComplete,
        )
        .await
        .map_err(|error| error.to_string())?;
        stream.finish().map_err(|error| error.to_string())
    }

    async fn handle_chat_media_read(
        &self,
        mut stream: rua_device_network::DeviceSessionBulkStream,
        address: ChatAddress,
        media_id: String,
        purpose: ChatMediaRead,
    ) -> Result<(), String> {
        use crate::services::chat_media::{
            local_chat_media_manifest, ChatMediaError, ChatMediaService,
        };
        let Some(root) = self.chat_media_root.as_deref() else {
            return write_transfer_error(&mut stream, "Chat Media read is unavailable").await;
        };
        let manifest = match local_chat_media_manifest(root, &address, &media_id).await {
            Ok(manifest) => manifest,
            Err(ChatMediaError::NotFound) => {
                write_json_frame(stream.send_mut(), &TransferStreamResponse::MediaMissing)
                    .await
                    .map_err(|e| e.to_string())?;
                return stream.finish().map_err(|e| e.to_string());
            }
            Err(error) => return write_transfer_error(&mut stream, &error.to_string()).await,
        };
        if let ChatMediaRead::FirstSubmission { project_id } = &purpose {
            let dir = rua_chat::ChatService::new(root).media_root();
            match crate::services::first_chat_submission::validate_staged_media(
                &dir,
                &media_id,
                &address,
                Some(project_id.as_deref()),
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    let Some(header) = crate::services::chat_context::load_chat_context(
                        root,
                        address.chat_id().as_str(),
                    )
                    .await
                    .map_err(|e| e.to_string())?
                    else {
                        return write_transfer_error(
                            &mut stream,
                            "Device has no staged attachment for this first submission",
                        )
                        .await;
                    };
                    if let Err(error) = crate::services::chat_context::check_project_membership(
                        header.project_id.as_deref(),
                        Some(project_id.as_deref()),
                    ) {
                        return write_transfer_error(&mut stream, &error.to_string()).await;
                    }
                }
                Err(detail) => return write_transfer_error(&mut stream, &detail).await,
            }
        }
        if !matches!(purpose, ChatMediaRead::Content) {
            write_json_frame(
                stream.send_mut(),
                &TransferStreamResponse::MediaManifest {
                    manifest: Box::new(manifest),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
            return stream.finish().map_err(|e| e.to_string());
        }
        let opened = match ChatMediaService.open(root, address, &media_id).await {
            Ok(opened) => opened,
            Err(error) => return write_transfer_error(&mut stream, &error.to_string()).await,
        };
        write_json_frame(
            stream.send_mut(),
            &TransferStreamResponse::MediaReadReady {
                mime_type: opened.mime_type,
                byte_size: opened.byte_size,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        let mut file = tokio::fs::File::open(opened.path)
            .await
            .map_err(|error| error.to_string())?;
        let mut offset = 0u64;
        let mut buffer = vec![0u8; MAX_TRANSFER_CHUNK_BYTES];
        loop {
            let len = file
                .read(&mut buffer)
                .await
                .map_err(|error| error.to_string())?;
            if len == 0 {
                break;
            }
            write_json_frame(
                stream.send_mut(),
                &TransferStreamResponse::MediaReadChunk { offset, len },
            )
            .await
            .map_err(|error| error.to_string())?;
            stream
                .send_mut()
                .write_all(&buffer[..len])
                .await
                .map_err(|error| error.to_string())?;
            offset += len as u64;
        }
        write_json_frame(
            stream.send_mut(),
            &TransferStreamResponse::MediaReadComplete,
        )
        .await
        .map_err(|error| error.to_string())?;
        stream.finish().map_err(|error| error.to_string())
    }
}

enum TransferAttemptError {
    Retryable(String),
    Fatal(SyncTransferPlanError),
}

async fn write_transfer_frame<T, W>(writer: &mut W, value: &T) -> Result<(), TransferAttemptError>
where
    T: Serialize,
    W: tokio::io::AsyncWrite + Unpin,
{
    tokio::time::timeout(TRANSFER_IO_TIMEOUT, write_json_frame(writer, value))
        .await
        .map_err(|_| TransferAttemptError::Retryable("Bulk Transfer write timed out".to_string()))?
        .map_err(|error| TransferAttemptError::Retryable(error.to_string()))
}

async fn read_transfer_frame<T, R>(reader: &mut R) -> Result<T, TransferAttemptError>
where
    T: for<'de> Deserialize<'de>,
    R: tokio::io::AsyncRead + Unpin,
{
    tokio::time::timeout(TRANSFER_IO_TIMEOUT, read_json_frame(reader))
        .await
        .map_err(|_| TransferAttemptError::Retryable("Bulk Transfer read timed out".to_string()))?
        .map_err(|error| TransferAttemptError::Retryable(error.to_string()))
}

fn classify_bulk_open_error(error: rua_device_network::DeviceSessionError) -> TransferAttemptError {
    match error {
        rua_device_network::DeviceSessionError::Timeout { .. }
        | rua_device_network::DeviceSessionError::Connect(_)
        | rua_device_network::DeviceSessionError::Closed(_)
        | rua_device_network::DeviceSessionError::Io(_)
        | rua_device_network::DeviceSessionError::Replaced => {
            TransferAttemptError::Retryable(error.to_string())
        }
        rua_device_network::DeviceSessionError::BulkTransferRejected(detail) => {
            TransferAttemptError::Fatal(SyncTransferPlanError::Remote(detail))
        }
        other => TransferAttemptError::Fatal(SyncTransferPlanError::Remote(other.to_string())),
    }
}

async fn open_chat_media_request(
    manager: &rua_device_network::DeviceSessionManager,
    device_id: &str,
    address: &ChatAddress,
    media_id: &str,
    purpose: ChatMediaRead,
) -> Result<
    (
        rua_device_network::DeviceSessionBulkStream,
        TransferStreamResponse,
    ),
    String,
> {
    let mut stream = manager
        .open_bulk_transfer_to(device_id, TRANSFER_IO_TIMEOUT)
        .await
        .map_err(|e| e.to_string())?;
    write_json_frame(
        stream.send_mut(),
        &TransferStreamRequest::MediaRead {
            address: address.clone(),
            media_id: media_id.to_owned(),
            purpose,
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    let response = read_json_frame(stream.recv_mut())
        .await
        .map_err(|e| e.to_string())?;
    Ok((stream, response))
}

pub(crate) async fn request_chat_media_manifest(
    manager: &rua_device_network::DeviceSessionManager,
    device_id: &str,
    address: &ChatAddress,
    media_id: &str,
    first_submission_project: Option<Option<&str>>,
) -> Result<Option<ChatMediaManifest>, String> {
    let purpose = match first_submission_project {
        Some(project_id) => ChatMediaRead::FirstSubmission {
            project_id: project_id.map(str::to_owned),
        },
        None => ChatMediaRead::Metadata,
    };
    let (_stream, response) =
        open_chat_media_request(manager, device_id, address, media_id, purpose).await?;
    match response {
        TransferStreamResponse::MediaManifest { manifest } => Ok(Some(*manifest)),
        TransferStreamResponse::MediaMissing => Ok(None),
        TransferStreamResponse::Error { detail } => Err(detail),
        _ => Err("invalid Chat Media manifest response".into()),
    }
}

pub(crate) async fn read_chat_media_bytes(
    manager: &rua_device_network::DeviceSessionManager,
    device_id: &str,
    address: &ChatAddress,
    media_id: &str,
    manifest: &ChatMediaManifest,
) -> Result<Vec<u8>, String> {
    let (mut stream, ready) = open_chat_media_request(
        manager,
        device_id,
        address,
        media_id,
        ChatMediaRead::Content,
    )
    .await?;
    match ready {
        TransferStreamResponse::MediaReadReady {
            mime_type,
            byte_size,
        } if mime_type == manifest.mime_type && byte_size == manifest.byte_size => {}
        TransferStreamResponse::Error { detail } => return Err(detail),
        _ => return Err("source has no matching Chat Media copy".into()),
    }
    let mut bytes = Vec::with_capacity(manifest.byte_size as usize);
    loop {
        match read_json_frame(stream.recv_mut())
            .await
            .map_err(|e| e.to_string())?
        {
            TransferStreamResponse::MediaReadChunk { offset, len }
                if offset == bytes.len() as u64
                    && (1..=MAX_TRANSFER_CHUNK_BYTES).contains(&len)
                    && offset + len as u64 <= manifest.byte_size =>
            {
                bytes.resize(offset as usize + len, 0);
                stream
                    .recv_mut()
                    .read_exact(&mut bytes[offset as usize..])
                    .await
                    .map_err(|e| e.to_string())?;
            }
            TransferStreamResponse::MediaReadComplete
                if bytes.len() as u64 == manifest.byte_size =>
            {
                return Ok(bytes)
            }
            TransferStreamResponse::Error { detail } => return Err(detail),
            _ => return Err("incomplete or invalid Chat Media content".into()),
        }
    }
}

pub async fn read_sync_blob_via_session(
    session_manager: &rua_device_network::DeviceSessionManager,
    target_device_id: &str,
    hash: rua_sync::endpoint::BlobHash,
) -> Result<Vec<u8>, String> {
    let mut stream = session_manager
        .open_bulk_transfer_to(target_device_id, TRANSFER_IO_TIMEOUT)
        .await
        .map_err(|error| error.to_string())?;
    write_json_frame(
        stream.send_mut(),
        &TransferStreamRequest::SyncBlobRead {
            hash: hash.to_string(),
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    let ready: TransferStreamResponse = read_json_frame(stream.recv_mut())
        .await
        .map_err(|error| error.to_string())?;
    let TransferStreamResponse::SyncBlobReadReady { byte_size } = ready else {
        return Err(format!("unexpected Sync Blob read response: {ready:?}"));
    };
    if byte_size > MAX_SYNC_BLOB_BYTES {
        return Err(format!(
            "Sync Blob exceeds {MAX_SYNC_BLOB_BYTES} bytes: {byte_size}"
        ));
    }
    let capacity = usize::try_from(byte_size)
        .map_err(|_| format!("Sync Blob is too large: {byte_size} bytes"))?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut expected_offset = 0u64;
    loop {
        let response: TransferStreamResponse = read_json_frame(stream.recv_mut())
            .await
            .map_err(|error| error.to_string())?;
        match response {
            TransferStreamResponse::SyncBlobReadChunk { offset, len }
                if offset == expected_offset && len <= MAX_TRANSFER_CHUNK_BYTES =>
            {
                let mut chunk = vec![0u8; len];
                stream
                    .recv_mut()
                    .read_exact(&mut chunk)
                    .await
                    .map_err(|error| error.to_string())?;
                bytes.extend_from_slice(&chunk);
                expected_offset += len as u64;
            }
            TransferStreamResponse::SyncBlobReadComplete if expected_offset == byte_size => break,
            TransferStreamResponse::Error { detail } => return Err(detail),
            other => return Err(format!("invalid Sync Blob read response: {other:?}")),
        }
    }
    let actual = rua_sync::endpoint::BlobHash::new(&bytes);
    if actual != hash {
        return Err(format!(
            "Sync Blob hash mismatch: expected {hash}, received {actual}"
        ));
    }
    Ok(bytes)
}

pub async fn send_transfer_stream<F>(
    session_manager: &rua_device_network::DeviceSessionManager,
    request: TransferStreamSendRequest,
    cancel_rx: tokio::sync::watch::Receiver<RunControl>,
    mut on_progress: F,
) -> Result<TransferExecutionOutcome, SyncTransferPlanError>
where
    F: FnMut(TransferExecutionProgress),
{
    if request
        .manifest
        .entries
        .iter()
        .any(|entry| !valid_digest(&entry.sha256))
    {
        return Err(SyncTransferPlanError::Remote(
            "transfer digests must be captured before sending".into(),
        ));
    }
    let mut last_error = "Bulk Transfer did not start".to_string();
    let mut state = TransferSendState::default();
    for attempt in 0..=TRANSFER_RECOVERY_ATTEMPTS {
        if cancel_rx.borrow().is_terminal() {
            abort_transfer_operation(session_manager, &request).await;
            if state.commit_possible {
                return Err(SyncTransferPlanError::OutcomeUnknown {
                    operation_id: request.operation_id.clone(),
                    detail: "cancelled while a destination commit was unconfirmed".into(),
                });
            }
            return Err(SyncTransferPlanError::Cancelled);
        }
        match send_transfer_attempt(
            session_manager,
            &request,
            &cancel_rx,
            &mut on_progress,
            &mut state,
        )
        .await
        {
            Ok(outcome) => return Ok(outcome),
            Err(TransferAttemptError::Fatal(error)) => {
                if !matches!(error, SyncTransferPlanError::OutcomeUnknown { .. }) {
                    abort_transfer_operation(session_manager, &request).await;
                }
                return Err(error);
            }
            Err(TransferAttemptError::Retryable(error)) => {
                last_error = error;
                if attempt == TRANSFER_RECOVERY_ATTEMPTS {
                    break;
                }
                tokio::time::sleep(TRANSFER_RETRY_BACKOFF).await;
            }
        }
    }
    if state.commit_possible {
        return Err(SyncTransferPlanError::OutcomeUnknown {
            operation_id: request.operation_id.clone(),
            detail: last_error,
        });
    }
    abort_transfer_operation(session_manager, &request).await;
    Err(SyncTransferPlanError::RecoveryExhausted(last_error))
}

#[derive(Default)]
struct TransferSendState {
    must_resume: bool,
    commit_possible: bool,
}

async fn send_transfer_attempt<F>(
    session_manager: &rua_device_network::DeviceSessionManager,
    request: &TransferStreamSendRequest,
    cancel_rx: &tokio::sync::watch::Receiver<RunControl>,
    on_progress: &mut F,
    state: &mut TransferSendState,
) -> Result<TransferExecutionOutcome, TransferAttemptError>
where
    F: FnMut(TransferExecutionProgress),
{
    let mut stream = session_manager
        .open_bulk_transfer_to(&request.target_device_id, Duration::from_secs(5))
        .await
        .map_err(classify_bulk_open_error)?;
    let mut header = TransferStreamHeader::from_manifest(
        request.operation_id.clone(),
        request.source_device_id.clone(),
        request.target_device_id.clone(),
        &request.manifest,
        request.target_dir.clone(),
        request.conflict_policy,
    );
    header.chat_context = request.chat_context.clone();
    write_transfer_frame(
        stream.send_mut(),
        &TransferStreamRequest::Open {
            header: Box::new(header),
            resume_only: state.must_resume,
        },
    )
    .await?;
    let response: TransferStreamResponse = read_transfer_frame(stream.recv_mut()).await?;
    let (mut manifest_index, mut offset, bytes_written, complete, completed_files) = match response
    {
        TransferStreamResponse::Resume {
            operation_id,
            manifest_index,
            offset,
            bytes_written,
            complete,
            completed_files,
            ..
        } if operation_id == request.operation_id => (
            manifest_index,
            offset,
            bytes_written,
            complete,
            completed_files,
        ),
        TransferStreamResponse::Error { detail } => {
            if detail.contains("already has an active Bulk stream") {
                return Err(TransferAttemptError::Retryable(detail));
            }
            if state.commit_possible {
                return Err(TransferAttemptError::Fatal(
                    SyncTransferPlanError::OutcomeUnknown {
                        operation_id: request.operation_id.clone(),
                        detail,
                    },
                ));
            }
            return Err(TransferAttemptError::Fatal(SyncTransferPlanError::Remote(
                detail,
            )));
        }
        TransferStreamResponse::OutcomeUnknown {
            operation_id,
            detail,
        } => {
            return Err(TransferAttemptError::Fatal(
                SyncTransferPlanError::OutcomeUnknown {
                    operation_id,
                    detail,
                },
            ))
        }
        other => {
            return Err(TransferAttemptError::Fatal(SyncTransferPlanError::Remote(
                format!("unexpected Bulk Transfer open response: {other:?}"),
            )))
        }
    };
    validate_resume_cursor(&request.manifest, manifest_index, offset)
        .map_err(TransferAttemptError::Fatal)?;
    state.commit_possible = false;
    if bytes_written > 0 || complete || !completed_files.is_empty() {
        on_progress(TransferExecutionProgress {
            manifest_index: manifest_index.min(request.manifest.entries.len().saturating_sub(1)),
            bytes_transferred: bytes_written,
            total_bytes: request.manifest.total_bytes,
            completed_files,
        });
    }

    let chunk_size = request
        .options
        .chunk_size
        .clamp(1, MAX_TRANSFER_CHUNK_BYTES);
    let mut buffer = vec![0u8; chunk_size];
    while manifest_index < request.manifest.entries.len() {
        if cancel_rx.borrow().is_terminal() {
            return Err(TransferAttemptError::Fatal(
                SyncTransferPlanError::Cancelled,
            ));
        }
        let entry = &request.manifest.entries[manifest_index];
        let mut source = tokio::fs::File::from_std(
            open_source_file(&entry.source_path).map_err(TransferAttemptError::Fatal)?,
        );
        if source
            .metadata()
            .await
            .map_err(|error| TransferAttemptError::Fatal(error.into()))?
            .len()
            != entry.size
        {
            return Err(TransferAttemptError::Fatal(
                SyncTransferPlanError::IntegrityMismatch(entry.source_path.clone()),
            ));
        }
        source
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|error| TransferAttemptError::Fatal(error.into()))?;
        let remaining = entry.size.saturating_sub(offset);
        let len = if remaining == 0 {
            0
        } else {
            remaining.min(buffer.len() as u64) as usize
        };
        if len > 0 {
            source
                .read_exact(&mut buffer[..len])
                .await
                .map_err(|error| TransferAttemptError::Fatal(error.into()))?;
        }
        state.must_resume = true;
        state.commit_possible |= offset + len as u64 == entry.size;
        write_transfer_frame(
            stream.send_mut(),
            &TransferStreamRequest::Chunk {
                operation_id: request.operation_id.clone(),
                manifest_index,
                offset,
                len,
            },
        )
        .await?;
        tokio::time::timeout(
            TRANSFER_IO_TIMEOUT,
            stream.send_mut().write_all(&buffer[..len]),
        )
        .await
        .map_err(|_| TransferAttemptError::Retryable("Bulk Transfer write timed out".to_string()))?
        .map_err(|error| TransferAttemptError::Retryable(error.to_string()))?;
        let response: TransferStreamResponse = read_transfer_frame(stream.recv_mut()).await?;
        let (next_manifest_index, next_offset, acknowledged_bytes, complete, completed_files) =
            match response {
                TransferStreamResponse::Acknowledged {
                    operation_id,
                    manifest_index,
                    offset,
                    bytes_written,
                    complete,
                    completed_files,
                    ..
                } if operation_id == request.operation_id => (
                    manifest_index,
                    offset,
                    bytes_written,
                    complete,
                    completed_files,
                ),
                TransferStreamResponse::Error { detail } => {
                    state.commit_possible = false;
                    return Err(TransferAttemptError::Fatal(SyncTransferPlanError::Remote(
                        detail,
                    )));
                }
                TransferStreamResponse::OutcomeUnknown {
                    operation_id,
                    detail,
                } => {
                    return Err(TransferAttemptError::Fatal(
                        SyncTransferPlanError::OutcomeUnknown {
                            operation_id,
                            detail,
                        },
                    ))
                }
                other => {
                    return Err(TransferAttemptError::Fatal(SyncTransferPlanError::Remote(
                        format!("unexpected Bulk Transfer chunk response: {other:?}"),
                    )))
                }
            };
        validate_resume_cursor(&request.manifest, next_manifest_index, next_offset)
            .map_err(TransferAttemptError::Fatal)?;
        state.commit_possible = false;
        manifest_index = next_manifest_index;
        offset = next_offset;
        on_progress(TransferExecutionProgress {
            manifest_index: manifest_index.min(request.manifest.entries.len().saturating_sub(1)),
            bytes_transferred: acknowledged_bytes,
            total_bytes: request.manifest.total_bytes,
            completed_files,
        });
        if complete {
            break;
        }
        tokio::task::yield_now().await;
    }

    state.commit_possible = true;
    write_transfer_frame(
        stream.send_mut(),
        &TransferStreamRequest::Finish {
            operation_id: request.operation_id.clone(),
        },
    )
    .await?;
    let response: TransferStreamResponse = read_transfer_frame(stream.recv_mut()).await?;
    match response {
        TransferStreamResponse::Complete {
            operation_id,
            files_written,
            bytes_written,
            files,
        } if operation_id == request.operation_id => {
            if files_written != request.manifest.entries.len()
                || bytes_written != request.manifest.total_bytes
                || files.len() != files_written
                || files
                    .iter()
                    .zip(&request.manifest.entries)
                    .any(|(file, entry)| {
                        file.sha256 != entry.sha256
                            || file.bytes != entry.size
                            || !file
                                .target_path
                                .to_str()
                                .is_some_and(rua_core::execution_directory::is_device_absolute_path)
                    })
            {
                return Err(TransferAttemptError::Fatal(
                    SyncTransferPlanError::OutcomeUnknown {
                        operation_id,
                        detail: "completion receipt does not match the captured manifest".into(),
                    },
                ));
            }
            let _ = stream.finish();
            Ok(TransferExecutionOutcome {
                files_written,
                bytes_written,
                files,
            })
        }
        TransferStreamResponse::Error { detail } => Err(TransferAttemptError::Fatal(
            SyncTransferPlanError::Remote(detail),
        )),
        TransferStreamResponse::OutcomeUnknown {
            operation_id,
            detail,
        } => Err(TransferAttemptError::Fatal(
            SyncTransferPlanError::OutcomeUnknown {
                operation_id,
                detail,
            },
        )),
        other => Err(TransferAttemptError::Fatal(SyncTransferPlanError::Remote(
            format!("unexpected Bulk Transfer completion response: {other:?}"),
        ))),
    }
}

async fn abort_transfer_operation(
    session_manager: &rua_device_network::DeviceSessionManager,
    request: &TransferStreamSendRequest,
) {
    let abort = async {
        let mut stream = session_manager
            .open_bulk_transfer_to(&request.target_device_id, Duration::from_secs(1))
            .await
            .ok()?;
        let mut header = TransferStreamHeader::from_manifest(
            request.operation_id.clone(),
            request.source_device_id.clone(),
            request.target_device_id.clone(),
            &request.manifest,
            request.target_dir.clone(),
            request.conflict_policy,
        );
        header.chat_context = request.chat_context.clone();
        write_json_frame(
            stream.send_mut(),
            &TransferStreamRequest::Open {
                header: Box::new(header),
                resume_only: true,
            },
        )
        .await
        .ok()?;
        let _: TransferStreamResponse = read_json_frame(stream.recv_mut()).await.ok()?;
        write_json_frame(
            stream.send_mut(),
            &TransferStreamRequest::Abort {
                operation_id: request.operation_id.clone(),
            },
        )
        .await
        .ok()?;
        let _: TransferStreamResponse = read_json_frame(stream.recv_mut()).await.ok()?;
        Some(())
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), abort).await;
}

fn validate_resume_cursor(
    manifest: &TransferManifest,
    manifest_index: usize,
    offset: u64,
) -> Result<(), SyncTransferPlanError> {
    if manifest_index == manifest.entries.len() {
        if offset == 0 {
            return Ok(());
        }
        return Err(SyncTransferPlanError::Remote(
            "completed transfer cursor has a non-zero offset".to_string(),
        ));
    }
    let entry = manifest.entries.get(manifest_index).ok_or_else(|| {
        SyncTransferPlanError::Remote(format!(
            "transfer resume manifest index out of range: {manifest_index}"
        ))
    })?;
    if offset > entry.size {
        return Err(SyncTransferPlanError::Remote(format!(
            "transfer resume offset {offset} exceeds file size {}",
            entry.size
        )));
    }
    Ok(())
}

async fn write_transfer_error(
    stream: &mut rua_device_network::DeviceSessionBulkStream,
    detail: &str,
) -> Result<(), String> {
    write_json_frame(
        stream.send_mut(),
        &TransferStreamResponse::Error {
            detail: detail.to_string(),
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    stream.finish().map_err(|error| error.to_string())
}

fn transfer_error_response(error: SyncTransferPlanError) -> TransferStreamResponse {
    match error {
        SyncTransferPlanError::OutcomeUnknown {
            operation_id,
            detail,
        } => TransferStreamResponse::OutcomeUnknown {
            operation_id,
            detail,
        },
        other => TransferStreamResponse::Error {
            detail: other.to_string(),
        },
    }
}

pub async fn command_remote_source_send_to<F, M>(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: SyncTransferDeviceCommand,
    cancel_rx: tokio::sync::watch::Receiver<RunControl>,
    mut on_manifest: M,
    mut on_progress: F,
) -> Result<TransferExecutionOutcome, SyncTransferPlanError>
where
    F: FnMut(TransferExecutionProgress),
    M: FnMut(u64),
{
    command_remote_source_send_to_inner(
        session_manager,
        command,
        cancel_rx,
        &mut on_manifest,
        &mut on_progress,
    )
    .await
    .map(|(outcome, _)| outcome)
}

pub async fn command_remote_chat_gap_to(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: SyncTransferDeviceCommand,
    cancel_rx: tokio::sync::watch::Receiver<RunControl>,
) -> Result<
    (
        TransferExecutionOutcome,
        crate::services::chat_metadata_store::ChatMetadataRecord,
    ),
    SyncTransferPlanError,
> {
    let (outcome, source_chat) = command_remote_source_send_to_inner(
        session_manager,
        command,
        cancel_rx,
        &mut |_| {},
        &mut |_| {},
    )
    .await?;
    source_chat
        .map(|source_chat| (outcome, source_chat))
        .ok_or_else(|| {
            SyncTransferPlanError::Remote(
                "Chat gap transfer did not return source Chat metadata".to_string(),
            )
        })
}

async fn command_remote_source_send_to_inner<F, M>(
    session_manager: &rua_device_network::DeviceSessionManager,
    command: SyncTransferDeviceCommand,
    cancel_rx: tokio::sync::watch::Receiver<RunControl>,
    on_manifest: &mut M,
    on_progress: &mut F,
) -> Result<
    (
        TransferExecutionOutcome,
        Option<crate::services::chat_metadata_store::ChatMetadataRecord>,
    ),
    SyncTransferPlanError,
>
where
    F: FnMut(TransferExecutionProgress),
    M: FnMut(u64),
{
    let operation_id = match &command.action {
        SyncTransferDeviceAction::StartSend { operation_id, .. }
        | SyncTransferDeviceAction::StartChatGapSend { operation_id, .. } => operation_id.clone(),
        _ => {
            return Err(SyncTransferPlanError::Remote(
                "remote source command must start a transfer operation".to_string(),
            ))
        }
    };
    let source_device_id = command.target_device_id.clone();
    let coordinator_device_id = command.source_device_id.clone();
    let response = deliver_sync_transfer_device_command_via_session(
        session_manager,
        &command,
        TRANSFER_COMMAND_TIMEOUT,
    )
    .await
    .map_err(|error| SyncTransferPlanError::Remote(error.to_string()))?;
    let (mut operation, source_chat) = source_operation_from_response(response)?;
    let mut consecutive_poll_failures = 0;
    loop {
        on_manifest(operation.total_bytes);
        on_progress(TransferExecutionProgress {
            manifest_index: 0,
            bytes_transferred: operation.bytes_written,
            total_bytes: operation.total_bytes,
            completed_files: operation.files.clone(),
        });
        match operation.status {
            TransferJobStatus::Succeeded => {
                return Ok((
                    TransferExecutionOutcome {
                        files_written: operation.files_written,
                        bytes_written: operation.bytes_written,
                        files: operation.files,
                    },
                    source_chat,
                ));
            }
            TransferJobStatus::Failed => {
                return Err(SyncTransferPlanError::Remote(
                    operation
                        .error
                        .unwrap_or_else(|| "remote source transfer failed".to_string()),
                ));
            }
            TransferJobStatus::Cancelled => return Err(SyncTransferPlanError::Cancelled),
            TransferJobStatus::Unknown => {
                return Err(SyncTransferPlanError::OutcomeUnknown {
                    operation_id: operation.operation_id,
                    detail: operation
                        .error
                        .unwrap_or_else(|| "remote completion is unknown".into()),
                })
            }
            TransferJobStatus::Running => {}
        }
        if cancel_rx.borrow().is_terminal() {
            let cancel_command = SyncTransferDeviceCommand {
                command_id: format!("sync-transfer-cancel-{}", uuid::Uuid::new_v4()),
                source_device_id: coordinator_device_id.clone(),
                target_device_id: source_device_id.clone(),
                action: SyncTransferDeviceAction::CancelSend {
                    operation_id: operation_id.clone(),
                },
            };
            let _ = deliver_sync_transfer_device_command_via_session(
                session_manager,
                &cancel_command,
                TRANSFER_COMMAND_TIMEOUT,
            )
            .await;
            return Err(SyncTransferPlanError::Cancelled);
        }
        tokio::time::sleep(TRANSFER_RETRY_BACKOFF).await;
        let status_command = SyncTransferDeviceCommand {
            command_id: format!("sync-transfer-status-{}", uuid::Uuid::new_v4()),
            source_device_id: coordinator_device_id.clone(),
            target_device_id: source_device_id.clone(),
            action: SyncTransferDeviceAction::GetSendStatus {
                operation_id: operation_id.clone(),
            },
        };
        match deliver_sync_transfer_device_command_via_session(
            session_manager,
            &status_command,
            TRANSFER_COMMAND_TIMEOUT,
        )
        .await
        {
            Ok(response) => {
                operation = source_operation_from_response(response)?.0;
                consecutive_poll_failures = 0;
            }
            Err(error) => {
                consecutive_poll_failures += 1;
                if consecutive_poll_failures > TRANSFER_RECOVERY_ATTEMPTS {
                    let cancel_command = SyncTransferDeviceCommand {
                        command_id: format!("sync-transfer-cancel-{}", uuid::Uuid::new_v4()),
                        source_device_id: coordinator_device_id.clone(),
                        target_device_id: source_device_id.clone(),
                        action: SyncTransferDeviceAction::CancelSend {
                            operation_id: operation_id.clone(),
                        },
                    };
                    let _ = tokio::time::timeout(
                        Duration::from_secs(2),
                        deliver_sync_transfer_device_command_via_session(
                            session_manager,
                            &cancel_command,
                            Duration::from_secs(2),
                        ),
                    )
                    .await;
                    return Err(SyncTransferPlanError::OutcomeUnknown {
                        operation_id,
                        detail: error.to_string(),
                    });
                }
            }
        }
    }
}

fn source_operation_from_response(
    response: SyncTransferDeviceResponse,
) -> Result<
    (
        SourceTransferOperationSnapshot,
        Option<crate::services::chat_metadata_store::ChatMetadataRecord>,
    ),
    SyncTransferPlanError,
> {
    match response {
        SyncTransferDeviceResponse::TransferOperation { operation } => Ok((operation, None)),
        SyncTransferDeviceResponse::ChatGapTransferOperation {
            operation,
            source_chat,
        } => Ok((operation, Some(*source_chat))),
        SyncTransferDeviceResponse::DirEntries { .. } => Err(SyncTransferPlanError::Remote(
            "directory listing returned for transfer operation command".to_string(),
        )),
    }
}

fn collect_directory_manifest_entries(
    dir: &Path,
    relative_base: &Path,
    entries: &mut Vec<TransferManifestEntry>,
) -> Result<(), SyncTransferPlanError> {
    for entry in std::fs::read_dir(dir).map_err(|error| SyncTransferPlanError::WalkDirectory {
        path: dir.to_path_buf(),
        source: error,
    })? {
        let entry = entry.map_err(|error| SyncTransferPlanError::WalkDirectory {
            path: dir.to_path_buf(),
            source: error,
        })?;
        let path = entry.path();
        let relative_path = relative_base.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            SyncTransferPlanError::SourceMetadata {
                path: path.clone(),
                source: error,
            }
        })?;
        if metadata.is_file() {
            push_manifest_file(entries, &path, relative_path, metadata.len())?;
        } else if metadata.is_dir() {
            collect_directory_manifest_entries(&path, &relative_path, entries)?;
        } else {
            return Err(SyncTransferPlanError::UnsupportedSource(path));
        }
    }
    Ok(())
}

fn open_source_file(path: &Path) -> Result<std::fs::File, SyncTransferPlanError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(search_directory_flags() | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open("/")?;
        let relative = path
            .strip_prefix("/")
            .map_err(|_| SyncTransferPlanError::InvalidRelativePath(path.to_path_buf()))?;
        let components = relative.components().collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            if !matches!(component, std::path::Component::Normal(_)) {
                return Err(SyncTransferPlanError::InvalidRelativePath(
                    path.to_path_buf(),
                ));
            }
            directory = open_relative(
                &directory,
                component.as_os_str(),
                if index + 1 == components.len() {
                    libc::O_RDONLY
                } else {
                    search_directory_flags()
                },
            )?;
        }
        if !directory.metadata()?.is_file() {
            return Err(SyncTransferPlanError::UnsupportedSource(path.to_path_buf()));
        }
        Ok(directory)
    }
    #[cfg(not(unix))]
    {
        if path.canonicalize()? != path || !std::fs::symlink_metadata(path)?.is_file() {
            return Err(SyncTransferPlanError::UnsupportedSource(path.to_path_buf()));
        }
        Ok(std::fs::File::open(path)?)
    }
}

fn sha256_reader(
    file: &mut std::fs::File,
    expected_bytes: u64,
    path: &Path,
    cancel: Option<&tokio::sync::watch::Receiver<RunControl>>,
) -> Result<String, SyncTransferPlanError> {
    std::io::Seek::seek(file, std::io::SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        if cancel.is_some_and(|cancel| cancel.borrow().is_terminal()) {
            return Err(SyncTransferPlanError::Cancelled);
        }
        let read = std::io::Read::read(file, &mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > expected_bytes {
            return Err(SyncTransferPlanError::IntegrityMismatch(path.to_path_buf()));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn directory_identity(metadata: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        format!("{:?}", metadata.created())
    }
}

fn metadata_fingerprint(metadata: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    }
    #[cfg(not(unix))]
    {
        format!(
            "{}:{:?}:{:?}",
            metadata.len(),
            metadata.created(),
            metadata.modified()
        )
    }
}

fn target_fingerprint(path: &Path) -> Result<Option<String>, SyncTransferPlanError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata_fingerprint(&metadata))),
        Ok(_) => Err(SyncTransferPlanError::TargetConflict(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn prepare_temp_files(plan: &ReceivePlan) -> Result<(), SyncTransferPlanError> {
    let mut prepared: Vec<TransferTempCleanup> = Vec::new();
    for entry in &plan.entries {
        let parent = TransferParent::open(plan, entry, true)?;
        parent.open_temp(&entry.temp_path, true)?;
        prepared.push(TransferTempCleanup {
            parent,
            path: entry.temp_path.clone(),
            remove: true,
        });
    }
    for mut cleanup in prepared {
        cleanup.remove = false;
        drop(cleanup);
    }
    Ok(())
}

struct TransferParent {
    path: PathBuf,
    #[cfg(unix)]
    directory: std::fs::File,
}

struct TransferTempCleanup {
    parent: TransferParent,
    path: PathBuf,
    remove: bool,
}

impl Drop for TransferTempCleanup {
    fn drop(&mut self) {
        if self.remove {
            self.parent.remove_temp(&self.path);
        }
    }
}

impl TransferParent {
    fn open(
        plan: &ReceivePlan,
        entry: &ReceivePlanEntry,
        create: bool,
    ) -> Result<Self, SyncTransferPlanError> {
        let parent = entry
            .target_path
            .parent()
            .ok_or_else(|| SyncTransferPlanError::InvalidRelativePath(entry.target_path.clone()))?;
        let relative = parent
            .strip_prefix(&plan.target_dir)
            .map_err(|_| SyncTransferPlanError::TargetChanged(parent.to_path_buf()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut directory = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(search_directory_flags() | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&plan.target_dir)?;
            if directory_identity(&directory.metadata()?) != plan.root_identity {
                return Err(SyncTransferPlanError::TargetChanged(
                    plan.target_dir.clone(),
                ));
            }
            for component in relative.components() {
                let name = component.as_os_str();
                if create {
                    use std::os::fd::AsRawFd;
                    let c_name = path_component_cstring(name)?;
                    // The handle fixes the parent even if its pathname is concurrently replaced.
                    let result =
                        unsafe { libc::mkdirat(directory.as_raw_fd(), c_name.as_ptr(), 0o700) };
                    if result != 0 {
                        let error = std::io::Error::last_os_error();
                        if error.kind() != std::io::ErrorKind::AlreadyExists {
                            return Err(error.into());
                        }
                    } else {
                        open_relative(
                            &directory,
                            std::ffi::OsStr::new("."),
                            libc::O_RDONLY | libc::O_DIRECTORY,
                        )?
                        .sync_all()?;
                    }
                }
                directory = open_relative(&directory, name, search_directory_flags())?;
            }
            // Only the actual publication directory needs a descriptor suitable
            // for fsync; traversing its ancestors needs search permission alone.
            let directory = open_relative(
                &directory,
                std::ffi::OsStr::new("."),
                libc::O_RDONLY | libc::O_DIRECTORY,
            )?;
            Ok(Self {
                path: parent.to_path_buf(),
                directory,
            })
        }
        #[cfg(not(unix))]
        {
            if directory_identity(&std::fs::symlink_metadata(&plan.target_dir)?)
                != plan.root_identity
            {
                return Err(SyncTransferPlanError::TargetChanged(
                    plan.target_dir.clone(),
                ));
            }
            let mut path = plan.target_dir.clone();
            for component in relative.components() {
                path.push(component);
                if create && !path.exists() {
                    std::fs::create_dir(&path)?;
                }
                let metadata = std::fs::symlink_metadata(&path)?;
                if !metadata.is_dir()
                    || metadata.file_type().is_symlink()
                    || path.canonicalize()? != path
                {
                    return Err(SyncTransferPlanError::TargetChanged(path));
                }
            }
            Ok(Self { path })
        }
    }

    fn open_temp(&self, path: &Path, create: bool) -> Result<std::fs::File, SyncTransferPlanError> {
        let name = path
            .file_name()
            .ok_or_else(|| SyncTransferPlanError::InvalidRelativePath(path.to_path_buf()))?;
        #[cfg(unix)]
        {
            open_relative(
                &self.directory,
                name,
                libc::O_RDWR
                    | if create {
                        libc::O_CREAT | libc::O_EXCL
                    } else {
                        0
                    },
            )
        }
        #[cfg(not(unix))]
        {
            Ok(std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(create)
                .open(self.path.join(name))?)
        }
    }

    fn remove_temp(&self, path: &Path) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if let Some(name) = path
                .file_name()
                .and_then(|name| path_component_cstring(name).ok())
            {
                unsafe {
                    libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0);
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = std::fs::remove_file(path);
        }
    }

    fn verify_current(
        &self,
        plan: &ReceivePlan,
        entry: &ReceivePlanEntry,
    ) -> Result<(), SyncTransferPlanError> {
        let current = Self::open(plan, entry, false)?;
        #[cfg(unix)]
        if directory_identity(&current.directory.metadata()?)
            != directory_identity(&self.directory.metadata()?)
        {
            return Err(SyncTransferPlanError::TargetChanged(self.path.clone()));
        }
        #[cfg(not(unix))]
        if current.path != self.path {
            return Err(SyncTransferPlanError::TargetChanged(self.path.clone()));
        }
        Ok(())
    }

    fn publish(
        &self,
        entry: &ReceivePlanEntry,
        overwrite: bool,
    ) -> Result<(), SyncTransferPlanError> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let temp = path_component_cstring(entry.temp_path.file_name().unwrap())?;
            let target = path_component_cstring(entry.target_path.file_name().unwrap())?;
            let fd = self.directory.as_raw_fd();
            let result = unsafe {
                if overwrite {
                    libc::renameat(fd, temp.as_ptr(), fd, target.as_ptr())
                } else {
                    // Android app storage disallows hard links. An exclusive
                    // rename publishes atomically without replacing a new target.
                    #[cfg(any(target_os = "linux", target_os = "android"))]
                    {
                        libc::syscall(
                            libc::SYS_renameat2,
                            fd,
                            temp.as_ptr(),
                            fd,
                            target.as_ptr(),
                            libc::RENAME_NOREPLACE,
                        ) as libc::c_int
                    }
                    #[cfg(not(any(target_os = "linux", target_os = "android")))]
                    {
                        libc::linkat(fd, temp.as_ptr(), fd, target.as_ptr(), 0)
                    }
                }
            };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                return if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Err(SyncTransferPlanError::TargetConflict(
                        entry.target_path.clone(),
                    ))
                } else {
                    Err(error.into())
                };
            }
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            if !overwrite {
                self.remove_temp(&entry.temp_path);
            }
            self.directory
                .sync_all()
                .map_err(|error| SyncTransferPlanError::OutcomeUnknown {
                    operation_id: String::new(),
                    detail: format!("file published but parent sync failed: {error}"),
                })?;
        }
        #[cfg(not(unix))]
        {
            if overwrite {
                std::fs::rename(&entry.temp_path, &entry.target_path)?;
            } else {
                std::fs::hard_link(&entry.temp_path, &entry.target_path)?;
                std::fs::remove_file(&entry.temp_path)?;
            }
            rua_core::fs::sync_directory(&self.path).map_err(|error| {
                SyncTransferPlanError::OutcomeUnknown {
                    operation_id: String::new(),
                    detail: format!("file published but parent sync failed: {error}"),
                }
            })?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn search_directory_flags() -> i32 {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        libc::O_PATH | libc::O_DIRECTORY
    }
    #[cfg(target_vendor = "apple")]
    {
        libc::O_SEARCH
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    {
        libc::O_RDONLY | libc::O_DIRECTORY
    }
}

#[cfg(unix)]
fn path_component_cstring(
    name: &std::ffi::OsStr,
) -> Result<std::ffi::CString, SyncTransferPlanError> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(name.as_bytes())
        .map_err(|_| SyncTransferPlanError::InvalidRelativePath(PathBuf::from(name)))
}

#[cfg(unix)]
fn open_relative(
    parent: &std::fs::File,
    name: &std::ffi::OsStr,
    flags: i32,
) -> Result<std::fs::File, SyncTransferPlanError> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let name = path_component_cstring(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

fn verify_received_file(
    file: &mut std::fs::File,
    manifest: &TransferManifestEntry,
) -> Result<String, SyncTransferPlanError> {
    let before = metadata_fingerprint(&file.metadata()?);
    if file.metadata()?.len() != manifest.size
        || !valid_digest(&manifest.sha256)
        || sha256_reader(file, manifest.size, &manifest.relative_path, None)? != manifest.sha256
        || metadata_fingerprint(&file.metadata()?) != before
    {
        return Err(SyncTransferPlanError::IntegrityMismatch(
            manifest.relative_path.clone(),
        ));
    }
    file.sync_all()?;
    Ok(before)
}

fn commit_received_file(
    plan: &ReceivePlan,
    entry: &ReceivePlanEntry,
    manifest: &TransferManifestEntry,
    parent: &TransferParent,
    file: &std::fs::File,
    fingerprint: &str,
) -> Result<TransferFileOutcome, SyncTransferPlanError> {
    if metadata_fingerprint(&file.metadata()?) != fingerprint {
        return Err(SyncTransferPlanError::IntegrityMismatch(
            entry.target_path.clone(),
        ));
    }
    let _commit = TRANSFER_COMMIT_LOCK.lock().expect("transfer commit lock");
    parent.verify_current(plan, entry)?;
    let current = target_fingerprint(&entry.target_path)?;
    if current != entry.expected_target {
        return Err(SyncTransferPlanError::TargetChanged(
            entry.target_path.clone(),
        ));
    }
    let overwritten = current.is_some();
    parent.publish(
        entry,
        matches!(plan.conflict_policy, ReceiveConflictPolicy::Overwrite) && overwritten,
    )?;
    parent
        .verify_current(plan, entry)
        .map_err(|error| SyncTransferPlanError::OutcomeUnknown {
            operation_id: String::new(),
            detail: error.to_string(),
        })?;
    Ok(TransferFileOutcome {
        target_path: entry.target_path.clone(),
        bytes: manifest.size,
        sha256: manifest.sha256.clone(),
        overwritten,
        extra: Default::default(),
    })
}

fn valid_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn same_chat_invocation(left: &ChatTransferContext, right: &ChatTransferContext) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.call.confirmation = None;
    right.call.confirmation = None;
    left == right
}

fn merge_completion_receipts(
    files: &mut Vec<TransferFileOutcome>,
    received: &[TransferFileOutcome],
) {
    for file in received {
        if !files
            .iter()
            .any(|existing| existing.target_path == file.target_path)
        {
            files.push(file.clone());
        }
    }
}

async fn read_json_frame<T, R>(reader: &mut R) -> Result<T, SyncTransferPlanError>
where
    T: for<'de> Deserialize<'de>,
    R: tokio::io::AsyncRead + Unpin,
{
    let len = reader.read_u32().await? as usize;
    if len > MAX_TRANSFER_FRAME_BYTES {
        return Err(SyncTransferPlanError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "sync transfer frame too large",
        )));
    }
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes)
        .map_err(|error| SyncTransferPlanError::Io(std::io::Error::other(error.to_string())))
}

async fn write_json_frame<T, W>(writer: &mut W, value: &T) -> Result<(), SyncTransferPlanError>
where
    T: Serialize,
    W: tokio::io::AsyncWrite + Unpin,
{
    let bytes = serde_json::to_vec(value)
        .map_err(|error| SyncTransferPlanError::Io(std::io::Error::other(error.to_string())))?;
    if bytes.len() > MAX_TRANSFER_FRAME_BYTES {
        return Err(SyncTransferPlanError::Remote(
            "transfer frame exceeds the bounded size".into(),
        ));
    }
    writer.write_u32(bytes.len() as u32).await?;
    writer.write_all(&bytes).await?;
    Ok(())
}

fn push_manifest_file(
    entries: &mut Vec<TransferManifestEntry>,
    source_path: &Path,
    relative_path: PathBuf,
    size: u64,
) -> Result<(), SyncTransferPlanError> {
    check_manifest_count(entries.len().saturating_add(1))?;
    // Normalize native separators before crossing devices. A Unix filename may
    // contain a literal backslash, so replacing characters would change its name.
    let relative_path = PathBuf::from(
        relative_path
            .components()
            .map(|component| match component {
                std::path::Component::Normal(name) => name.to_str().ok_or_else(|| {
                    SyncTransferPlanError::InvalidRelativePath(relative_path.clone())
                }),
                _ => Err(SyncTransferPlanError::InvalidRelativePath(
                    relative_path.clone(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("/"),
    );
    validate_relative_path(&relative_path)?;
    entries.push(TransferManifestEntry {
        source_path: source_path.canonicalize()?,
        relative_path,
        kind: TransferManifestEntryKind::File,
        size,
        sha256: String::new(),
        extra: Default::default(),
    });
    Ok(())
}

fn check_manifest_count(count: usize) -> Result<(), SyncTransferPlanError> {
    if count > MAX_TRANSFER_FILES {
        return Err(SyncTransferPlanError::Remote(format!(
            "one transfer accepts at most {MAX_TRANSFER_FILES} files; select smaller batches"
        )));
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<(), SyncTransferPlanError> {
    // Wire paths use '/' only. A literal Unix backslash cannot be represented as
    // a Windows filename; reject it instead of silently creating another level.
    #[cfg(windows)]
    if path.as_os_str().to_string_lossy().contains('\\') {
        return Err(SyncTransferPlanError::InvalidRelativePath(
            path.to_path_buf(),
        ));
    }
    if path.is_absolute() {
        return Err(SyncTransferPlanError::InvalidRelativePath(
            path.to_path_buf(),
        ));
    }
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => {
                return Err(SyncTransferPlanError::InvalidRelativePath(
                    path.to_path_buf(),
                ))
            }
        }
    }
    Ok(())
}

fn resolve_conflict(
    candidate: PathBuf,
    policy: ReceiveConflictPolicy,
    planned_targets: &BTreeSet<PathBuf>,
) -> Result<PathBuf, SyncTransferPlanError> {
    if std::fs::symlink_metadata(&candidate)
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        && !planned_targets.contains(&candidate)
    {
        return Ok(candidate);
    }
    match policy {
        ReceiveConflictPolicy::Fail => Err(SyncTransferPlanError::TargetConflict(candidate)),
        ReceiveConflictPolicy::Overwrite if !planned_targets.contains(&candidate) => Ok(candidate),
        ReceiveConflictPolicy::Overwrite => Err(SyncTransferPlanError::TargetConflict(candidate)),
        ReceiveConflictPolicy::Rename => next_available_path(candidate, planned_targets),
    }
}

fn next_available_path(
    candidate: PathBuf,
    planned_targets: &BTreeSet<PathBuf>,
) -> Result<PathBuf, SyncTransferPlanError> {
    let parent = candidate
        .parent()
        .ok_or_else(|| SyncTransferPlanError::InvalidRelativePath(candidate.clone()))?;
    let stem = candidate
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| SyncTransferPlanError::InvalidRelativePath(candidate.clone()))?;
    let extension = candidate
        .extension()
        .and_then(|extension| extension.to_str());
    for index in 1.. {
        let file_name = match extension {
            Some(extension) if !extension.is_empty() => {
                format!("{stem} ({index}).{extension}")
            }
            _ => format!("{stem} ({index})"),
        };
        let next = parent.join(file_name);
        if std::fs::symlink_metadata(&next).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            && !planned_targets.contains(&next)
        {
            return Ok(next);
        }
    }
    unreachable!("unbounded rename search should return")
}

impl SyncTransferListClient {
    pub fn new(
        device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    ) -> Self {
        Self {
            device_network,
            timeout: Duration::from_secs(10),
        }
    }

    pub async fn list_directory(
        &self,
        local_device_id: &str,
        target_device_id: &str,
        path: &str,
    ) -> Result<SyncTransferDirectoryListing, String> {
        if target_device_id == local_device_id {
            return list_device_directory(path).map_err(|error| error.to_string());
        }

        let session_manager =
            crate::services::device_session_access::current_session_manager(&self.device_network)
                .await?;
        let response = deliver_sync_transfer_device_command_via_session(
            &session_manager,
            &SyncTransferDeviceCommand {
                command_id: format!("sync-transfer-list-{}", uuid::Uuid::new_v4()),
                source_device_id: local_device_id.to_string(),
                target_device_id: target_device_id.to_string(),
                action: SyncTransferDeviceAction::ListDir {
                    path: path.to_string(),
                },
            },
            self.timeout,
        )
        .await
        .map_err(|error| error.to_string())?;
        match response {
            SyncTransferDeviceResponse::DirEntries { path, entries, .. } => {
                Ok(SyncTransferDirectoryListing { path, entries })
            }
            SyncTransferDeviceResponse::TransferOperation { .. }
            | SyncTransferDeviceResponse::ChatGapTransferOperation { .. } => {
                Err("unexpected transfer control response for directory listing".to_string())
            }
        }
    }
}
