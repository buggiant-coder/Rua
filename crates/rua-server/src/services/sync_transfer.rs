use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::services::conversation_authority_placement::ConversationAuthorityEpoch;
use crate::services::device_command_bus::{
    deliver_sync_transfer_device_command_via_session, list_device_directory,
    SyncTransferDeviceAction, SyncTransferDeviceCommand, SyncTransferDeviceResponse,
    SyncTransferDirectoryListing, SyncTransferFileFingerprint,
};
use crate::services::foreground_operation::ForegroundOperationRegistry;
use rua_core::{ChatAddress, RunControl};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const TRANSFER_RECOVERY_ATTEMPTS: usize = 3;
const TRANSFER_RETRY_BACKOFF: Duration = Duration::from_millis(100);
const TRANSFER_IO_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSFER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSFER_OPERATION_TTL: Duration = Duration::from_secs(10 * 60);
const SOURCE_TRANSFER_CONTROL_TTL: Duration = Duration::from_secs(30);
const MAX_TRANSFER_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_SYNC_BLOB_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct SyncTransferListClient {
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferManifest {
    pub entries: Vec<TransferManifestEntry>,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferManifestEntry {
    pub source_path: PathBuf,
    pub relative_path: PathBuf,
    pub kind: TransferManifestEntryKind,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivePlanEntry {
    pub manifest_index: usize,
    pub target_path: PathBuf,
    pub temp_path: PathBuf,
    pub final_relative_path: PathBuf,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferExecutionProgress {
    pub manifest_index: usize,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferExecutionOutcome {
    pub files_written: usize,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferJobStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
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
            job.snapshot.error = None;
            job.foreground_lease_id.take()
        };
        if let (Some(registry), Some(lease_id)) = (&self.foreground_operations, foreground_lease_id)
        {
            registry.release(&lease_id);
        }
    }

    pub fn fail(&self, job_id: &str, error: String) {
        let foreground_lease_id = {
            let mut jobs = self.jobs.lock().expect("transfer jobs lock");
            let Some(job) = jobs.get_mut(job_id) else {
                return;
            };
            if job.snapshot.status != TransferJobStatus::Running {
                return;
            }
            job.snapshot.status = TransferJobStatus::Failed;
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TransferStreamHeader {
    operation_id: String,
    source_device_id: String,
    target_device_id: String,
    conflict_policy: TransferStreamConflictPolicy,
    target_dir: PathBuf,
    entries: Vec<TransferStreamEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TransferStreamEntry {
    relative_path: PathBuf,
    size: u64,
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
                })
                .collect(),
        }
    }

    fn to_receive_manifest(&self) -> Result<TransferManifest, SyncTransferPlanError> {
        let mut entries = Vec::with_capacity(self.entries.len());
        let mut total_bytes: u64 = 0;
        for entry in &self.entries {
            validate_relative_path(&entry.relative_path)?;
            total_bytes = total_bytes.checked_add(entry.size).ok_or_else(|| {
                SyncTransferPlanError::Remote("transfer manifest size overflow".to_string())
            })?;
            entries.push(TransferManifestEntry {
                source_path: PathBuf::new(),
                relative_path: entry.relative_path.clone(),
                kind: TransferManifestEntryKind::File,
                size: entry.size,
            });
        }
        Ok(TransferManifest {
            entries,
            total_bytes,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TransferStreamRequest {
    Open {
        header: TransferStreamHeader,
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
        authority_epoch: ConversationAuthorityEpoch,
        media_id: String,
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
    },
    Acknowledged {
        operation_id: String,
        manifest_index: usize,
        offset: u64,
        files_written: usize,
        bytes_written: u64,
        complete: bool,
    },
    Complete {
        operation_id: String,
        files_written: usize,
        bytes_written: u64,
    },
    Aborted {
        operation_id: String,
    },
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTransferOperationSnapshot {
    pub operation_id: String,
    pub status: TransferJobStatus,
    pub files_written: usize,
    pub bytes_written: u64,
    pub total_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct SourceTransferOperationState {
    snapshot: SourceTransferOperationSnapshot,
    cancel_tx: tokio::sync::watch::Sender<RunControl>,
    last_control_at: Instant,
    foreground_lease_id: Option<String>,
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
    complete: bool,
    active: bool,
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
}

struct ReceiveOperationGuard {
    operation_id: String,
    operations: Arc<Mutex<HashMap<String, ReceiveTransferOperation>>>,
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
                sha256: sha256_file(&entry.source_path)?,
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
        let Some((target_size, target_sha256)) = target.get(&entry.relative_path) else {
            return true;
        };
        if *target_size != entry.size {
            return true;
        }
        sha256_file(&entry.source_path)
            .map(|source_sha256| source_sha256 != *target_sha256)
            .unwrap_or(true)
    });
    manifest.total_bytes = manifest.entries.iter().map(|entry| entry.size).sum();
    Ok(manifest)
}

pub fn plan_receive(
    target_dir: &Path,
    manifest: &TransferManifest,
    conflict_policy: ReceiveConflictPolicy,
) -> Result<ReceivePlan, SyncTransferPlanError> {
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
        let temp_path = target_path.with_extension(format!(
            "{}.rua-transfer-tmp",
            target_path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
        ));
        entries.push(ReceivePlanEntry {
            manifest_index: index,
            target_path,
            temp_path,
            final_relative_path,
        });
    }
    Ok(ReceivePlan {
        target_dir,
        entries,
    })
}

pub async fn execute_local_transfer_plan<F>(
    manifest: &TransferManifest,
    plan: &ReceivePlan,
    options: TransferExecutionOptions,
    cancel_rx: tokio::sync::watch::Receiver<RunControl>,
    mut on_progress: F,
) -> Result<TransferExecutionOutcome, SyncTransferPlanError>
where
    F: FnMut(TransferExecutionProgress),
{
    let chunk_size = options.chunk_size.max(1);
    let mut files_written = 0;
    let mut bytes_written = 0;
    for plan_entry in &plan.entries {
        if cancel_rx.borrow().is_terminal() {
            return Err(SyncTransferPlanError::Cancelled);
        }
        let manifest_entry = manifest
            .entries
            .get(plan_entry.manifest_index)
            .ok_or_else(|| {
                SyncTransferPlanError::InvalidRelativePath(plan_entry.final_relative_path.clone())
            })?;
        if let Some(parent) = plan_entry.target_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if let Some(parent) = plan_entry.temp_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        match tokio::fs::remove_file(&plan_entry.temp_path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let mut source = tokio::fs::File::open(&manifest_entry.source_path).await?;
        let mut temp = tokio::fs::File::create(&plan_entry.temp_path).await?;
        let mut buffer = vec![0u8; chunk_size];
        loop {
            if cancel_rx.borrow().is_terminal() {
                drop(temp);
                cleanup_temp_file(&plan_entry.temp_path).await?;
                return Err(SyncTransferPlanError::Cancelled);
            }
            let read = source.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            temp.write_all(&buffer[..read]).await?;
            bytes_written += read as u64;
            on_progress(TransferExecutionProgress {
                manifest_index: plan_entry.manifest_index,
                bytes_transferred: bytes_written,
                total_bytes: manifest.total_bytes,
            });
        }
        temp.flush().await?;
        temp.sync_all().await?;
        drop(temp);
        if cancel_rx.borrow().is_terminal() {
            cleanup_temp_file(&plan_entry.temp_path).await?;
            return Err(SyncTransferPlanError::Cancelled);
        }
        tokio::fs::rename(&plan_entry.temp_path, &plan_entry.target_path).await?;
        fsync_parent_best_effort(&plan_entry.target_path).await;
        files_written += 1;
    }

    Ok(TransferExecutionOutcome {
        files_written,
        bytes_written,
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
            error: None,
        };
        operations.insert(
            operation_id,
            SourceTransferOperationState {
                snapshot: snapshot.clone(),
                cancel_tx,
                last_control_at: Instant::now(),
                foreground_lease_id,
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
            operation.snapshot.error = None;
            operation.foreground_lease_id.take()
        };
        if let (Some(registry), Some(lease_id)) = (&self.foreground_operations, foreground_lease_id)
        {
            registry.release(&lease_id);
        }
    }

    pub fn fail(&self, operation_id: &str, error: String) {
        let foreground_lease_id = {
            let mut operations = self.operations.lock().expect("source operations lock");
            let Some(operation) = operations.get_mut(operation_id) else {
                return;
            };
            if operation.snapshot.status != TransferJobStatus::Running {
                return;
            }
            operation.snapshot.status = TransferJobStatus::Failed;
            operation.snapshot.error = Some(error);
            operation.foreground_lease_id.take()
        };
        if let (Some(registry), Some(lease_id)) = (&self.foreground_operations, foreground_lease_id)
        {
            registry.release(&lease_id);
        }
    }

    pub fn cancel(&self, operation_id: &str) -> Option<SourceTransferOperationSnapshot> {
        let (snapshot, foreground_lease_id) = {
            let mut operations = self.operations.lock().expect("source operations lock");
            let operation = operations.get_mut(operation_id)?;
            let _ = operation.cancel_tx.send(RunControl::Stop);
            let foreground_lease_id = if operation.snapshot.status == TransferJobStatus::Running {
                operation.snapshot.status = TransferJobStatus::Cancelled;
                operation.foreground_lease_id.take()
            } else {
                None
            };
            (operation.snapshot.clone(), foreground_lease_id)
        };
        if let (Some(registry), Some(lease_id)) = (&self.foreground_operations, foreground_lease_id)
        {
            registry.release(&lease_id);
        }
        Some(snapshot)
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
        let foreground_lease_id = {
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
            operation.snapshot.status = TransferJobStatus::Cancelled;
            operation.snapshot.error = Some("transfer controller became unavailable".to_string());
            operation.foreground_lease_id.take()
        };
        if let (Some(registry), Some(lease_id)) = (&self.foreground_operations, foreground_lease_id)
        {
            registry.release(&lease_id);
        }
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

    async fn open_operation(
        &self,
        peer_device_id: &str,
        mut header: TransferStreamHeader,
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
            header.target_dir = upload.media_dir;
        }

        for _ in 0..40 {
            let active = self
                .operations
                .lock()
                .expect("receive operations lock")
                .get(&header.operation_id)
                .is_some_and(|operation| operation.active);
            if !active {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        if let Some(response) = {
            let mut operations = self.operations.lock().expect("receive operations lock");
            operations.get_mut(&header.operation_id).map(|operation| {
                if operation.header != header {
                    return Err(SyncTransferPlanError::Remote(
                        "transfer operation was reopened with a different manifest or target"
                            .to_string(),
                    ));
                }
                if operation.active {
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

        let manifest = header.to_receive_manifest()?;
        let plan = plan_receive(&header.target_dir, &manifest, header.conflict_policy.into())?;
        for entry in &plan.entries {
            if let Some(parent) = entry.temp_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            cleanup_temp_file(&entry.temp_path).await?;
        }
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
                    complete,
                    active: true,
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
        let (temp_path, target_path, expected_size) = {
            let operations = self.operations.lock().expect("receive operations lock");
            let operation = operations.get(operation_id).ok_or_else(|| {
                SyncTransferPlanError::Remote(format!(
                    "transfer operation not found: {operation_id}"
                ))
            })?;
            if operation.complete {
                return Err(SyncTransferPlanError::Remote(
                    "transfer operation is already complete".to_string(),
                ));
            }
            if operation.manifest_index != manifest_index || operation.offset != offset {
                return Err(SyncTransferPlanError::Remote(format!(
                    "transfer offset mismatch: expected file {} offset {}, got file {manifest_index} offset {offset}",
                    operation.manifest_index, operation.offset
                )));
            }
            let manifest_entry =
                operation
                    .manifest
                    .entries
                    .get(manifest_index)
                    .ok_or_else(|| {
                        SyncTransferPlanError::Remote(format!(
                            "transfer manifest index out of range: {manifest_index}"
                        ))
                    })?;
            if offset.saturating_add(payload.len() as u64) > manifest_entry.size {
                return Err(SyncTransferPlanError::Remote(
                    "transfer chunk exceeds declared file size".to_string(),
                ));
            }
            let plan_entry = operation.plan.entries.get(manifest_index).ok_or_else(|| {
                SyncTransferPlanError::Remote(format!(
                    "transfer receive plan index out of range: {manifest_index}"
                ))
            })?;
            (
                plan_entry.temp_path.clone(),
                plan_entry.target_path.clone(),
                manifest_entry.size,
            )
        };

        if let Some(parent) = temp_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut temp = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&temp_path)
            .await?;
        let current_len = temp.metadata().await?.len();
        if current_len < offset {
            return Err(SyncTransferPlanError::Remote(format!(
                "partial transfer file is shorter than acknowledged offset {offset}"
            )));
        }
        if current_len > offset {
            temp.set_len(offset).await?;
        }
        temp.seek(std::io::SeekFrom::Start(offset)).await?;
        temp.write_all(payload).await?;
        temp.flush().await?;
        let next_offset = offset + payload.len() as u64;
        let completed_file = next_offset == expected_size;
        if completed_file {
            temp.sync_all().await?;
        }
        drop(temp);
        if completed_file {
            tokio::fs::rename(&temp_path, &target_path).await?;
            fsync_parent_best_effort(&target_path).await;
        }

        let (response, foreground_update) = {
            let mut operations = self.operations.lock().expect("receive operations lock");
            let operation = operations.get_mut(operation_id).ok_or_else(|| {
                SyncTransferPlanError::Remote(format!(
                    "transfer operation disappeared: {operation_id}"
                ))
            })?;
            if operation.manifest_index != manifest_index || operation.offset != offset {
                return Err(SyncTransferPlanError::Remote(
                    "transfer operation changed while a chunk was being committed".to_string(),
                ));
            }
            operation.bytes_written += payload.len() as u64;
            if completed_file {
                operation.files_written += 1;
                operation.manifest_index += 1;
                operation.offset = 0;
                operation.complete = operation.manifest_index == operation.manifest.entries.len();
            } else {
                operation.offset = next_offset;
            }
            operation.last_used = Instant::now();
            (
                TransferStreamResponse::Acknowledged {
                    operation_id: operation_id.to_string(),
                    manifest_index: operation.manifest_index,
                    offset: operation.offset,
                    files_written: operation.files_written,
                    bytes_written: operation.bytes_written,
                    complete: operation.complete,
                },
                operation.foreground_lease_id.clone().map(|lease_id| {
                    (
                        lease_id,
                        operation.bytes_written,
                        operation.manifest.total_bytes,
                    )
                }),
            )
        };
        if let (Some(registry), Some((lease_id, bytes_written, total_bytes))) =
            (&self.foreground_operations, foreground_update)
        {
            registry.update_transfer(&lease_id, bytes_written, total_bytes);
        }
        Ok(response)
    }

    async fn abort_operation(&self, operation_id: &str) {
        let operation = self
            .operations
            .lock()
            .expect("receive operations lock")
            .remove(operation_id);
        if let Some(operation) = operation {
            if let (Some(registry), Some(lease_id)) = (
                &self.foreground_operations,
                operation.foreground_lease_id.as_deref(),
            ) {
                registry.release(lease_id);
            }
            for entry in operation.plan.entries {
                let _ = cleanup_temp_file(&entry.temp_path).await;
            }
        }
    }

    async fn prune_stale_operations(&self) {
        let stale = {
            let mut operations = self.operations.lock().expect("receive operations lock");
            let stale_ids = operations
                .iter()
                .filter(|(_, operation)| {
                    !operation.active && operation.last_used.elapsed() >= TRANSFER_OPERATION_TTL
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
            for entry in operation.plan.entries {
                let _ = cleanup_temp_file(&entry.temp_path).await;
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
                    !operation.active && operation.last_used.elapsed() >= TRANSFER_OPERATION_TTL
                });
            if stale {
                self.abort_operation(&operation_id).await;
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
            authority_epoch,
            media_id,
        } = request
        {
            return self
                .handle_chat_media_read(stream, address, authority_epoch, media_id)
                .await;
        }
        if let TransferStreamRequest::SyncBlobRead { hash } = request {
            return self.handle_sync_blob_read(stream, hash).await;
        }
        let TransferStreamRequest::Open { header } = request else {
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
        let (operation_guard, response) =
            match self.open_operation(&stream.peer_device_id(), header).await {
                Ok(opened) => opened,
                Err(error) => {
                    write_json_frame(
                        stream.send_mut(),
                        &TransferStreamResponse::Error {
                            detail: error.to_string(),
                        },
                    )
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
                            write_transfer_error(&mut stream, &error.to_string()).await?;
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
                    if let Some(registry) = &self.media_transfers {
                        registry.complete_upload(&operation_id)?;
                    }
                    write_json_frame(
                        stream.send_mut(),
                        &TransferStreamResponse::Complete {
                            operation_id: operation_id.clone(),
                            files_written: outcome.files_written,
                            bytes_written: outcome.bytes_written,
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
        authority_epoch: ConversationAuthorityEpoch,
        media_id: String,
    ) -> Result<(), String> {
        let rua_root = self
            .chat_media_root
            .as_deref()
            .ok_or_else(|| "Chat Media read is unavailable".to_string())?;
        let scope = rua_core::chat_store::ChatScopeStore::resolve(rua_root, address.clone());
        let placement =
            crate::services::conversation_authority_target::load_optional_chat_authority_placement(
                scope.root(),
                address.chat_id().as_str(),
            )
            .await?
            .ok_or_else(|| "Conversation Authority placement is not materialized".to_string())?;
        if placement.device_id != self.local_device_id
            || placement.authority_epoch != authority_epoch
        {
            return Err(format!(
                "Conversation Authority is on Device '{}' at epoch '{}'",
                placement.device_id, placement.authority_epoch
            ));
        }
        let opened = crate::services::chat_media::ChatMediaService
            .open(rua_root, address, &media_id)
            .await
            .map_err(|error| error.to_string())?;
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

pub async fn open_chat_media_read_stream(
    session_manager: &rua_device_network::DeviceSessionManager,
    target_device_id: &str,
    address: ChatAddress,
    authority_epoch: ConversationAuthorityEpoch,
    media_id: String,
) -> Result<(String, u64, tokio::io::DuplexStream), String> {
    let mut stream = session_manager
        .open_bulk_transfer_to(target_device_id, TRANSFER_IO_TIMEOUT)
        .await
        .map_err(|error| error.to_string())?;
    write_json_frame(
        stream.send_mut(),
        &TransferStreamRequest::MediaRead {
            address,
            authority_epoch,
            media_id,
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    let ready: TransferStreamResponse = read_json_frame(stream.recv_mut())
        .await
        .map_err(|error| error.to_string())?;
    let TransferStreamResponse::MediaReadReady {
        mime_type,
        byte_size,
    } = ready
    else {
        return Err(format!("unexpected Chat Media read response: {ready:?}"));
    };
    let (reader, mut writer) = tokio::io::duplex(MAX_TRANSFER_CHUNK_BYTES * 2);
    tokio::spawn(async move {
        let mut expected_offset = 0u64;
        loop {
            let response: TransferStreamResponse = match read_json_frame(stream.recv_mut()).await {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!(error = %error, "remote Chat Media read response failed");
                    return;
                }
            };
            match response {
                TransferStreamResponse::MediaReadChunk { offset, len }
                    if offset == expected_offset && len <= MAX_TRANSFER_CHUNK_BYTES =>
                {
                    let mut bytes = vec![0u8; len];
                    if let Err(error) = stream.recv_mut().read_exact(&mut bytes).await {
                        tracing::warn!(error = %error, "remote Chat Media read chunk failed");
                        return;
                    }
                    if writer.write_all(&bytes).await.is_err() {
                        return;
                    }
                    expected_offset += len as u64;
                }
                TransferStreamResponse::MediaReadComplete if expected_offset == byte_size => {
                    let _ = writer.shutdown().await;
                    return;
                }
                TransferStreamResponse::Error { detail } => {
                    tracing::warn!(error = %detail, "remote Chat Media read rejected");
                    return;
                }
                other => {
                    tracing::warn!(?other, "invalid remote Chat Media read response");
                    return;
                }
            }
        }
    });
    Ok((mime_type, byte_size, reader))
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
    let mut last_error = "Bulk Transfer did not start".to_string();
    for attempt in 0..=TRANSFER_RECOVERY_ATTEMPTS {
        if cancel_rx.borrow().is_terminal() {
            abort_transfer_operation(session_manager, &request).await;
            return Err(SyncTransferPlanError::Cancelled);
        }
        match send_transfer_attempt(session_manager, &request, &cancel_rx, &mut on_progress).await {
            Ok(outcome) => return Ok(outcome),
            Err(TransferAttemptError::Fatal(error)) => {
                abort_transfer_operation(session_manager, &request).await;
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
    abort_transfer_operation(session_manager, &request).await;
    Err(SyncTransferPlanError::RecoveryExhausted(last_error))
}

async fn send_transfer_attempt<F>(
    session_manager: &rua_device_network::DeviceSessionManager,
    request: &TransferStreamSendRequest,
    cancel_rx: &tokio::sync::watch::Receiver<RunControl>,
    on_progress: &mut F,
) -> Result<TransferExecutionOutcome, TransferAttemptError>
where
    F: FnMut(TransferExecutionProgress),
{
    let mut stream = session_manager
        .open_bulk_transfer_to(&request.target_device_id, Duration::from_secs(5))
        .await
        .map_err(classify_bulk_open_error)?;
    let header = TransferStreamHeader::from_manifest(
        request.operation_id.clone(),
        request.source_device_id.clone(),
        request.target_device_id.clone(),
        &request.manifest,
        request.target_dir.clone(),
        request.conflict_policy,
    );
    write_transfer_frame(stream.send_mut(), &TransferStreamRequest::Open { header }).await?;
    let response: TransferStreamResponse = read_transfer_frame(stream.recv_mut()).await?;
    let (mut manifest_index, mut offset, bytes_written, complete) = match response {
        TransferStreamResponse::Resume {
            operation_id,
            manifest_index,
            offset,
            bytes_written,
            complete,
            ..
        } if operation_id == request.operation_id => {
            (manifest_index, offset, bytes_written, complete)
        }
        TransferStreamResponse::Error { detail } => {
            return Err(TransferAttemptError::Fatal(SyncTransferPlanError::Remote(
                detail,
            )))
        }
        other => {
            return Err(TransferAttemptError::Fatal(SyncTransferPlanError::Remote(
                format!("unexpected Bulk Transfer open response: {other:?}"),
            )))
        }
    };
    validate_resume_cursor(&request.manifest, manifest_index, offset)
        .map_err(TransferAttemptError::Fatal)?;
    if bytes_written > 0 || complete {
        on_progress(TransferExecutionProgress {
            manifest_index: manifest_index.min(request.manifest.entries.len().saturating_sub(1)),
            bytes_transferred: bytes_written,
            total_bytes: request.manifest.total_bytes,
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
        let mut source = tokio::fs::File::open(&entry.source_path)
            .await
            .map_err(|error| TransferAttemptError::Fatal(error.into()))?;
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
        let (next_manifest_index, next_offset, acknowledged_bytes, complete) = match response {
            TransferStreamResponse::Acknowledged {
                operation_id,
                manifest_index,
                offset,
                bytes_written,
                complete,
                ..
            } if operation_id == request.operation_id => {
                (manifest_index, offset, bytes_written, complete)
            }
            TransferStreamResponse::Error { detail } => {
                return Err(TransferAttemptError::Fatal(SyncTransferPlanError::Remote(
                    detail,
                )))
            }
            other => {
                return Err(TransferAttemptError::Fatal(SyncTransferPlanError::Remote(
                    format!("unexpected Bulk Transfer chunk response: {other:?}"),
                )))
            }
        };
        validate_resume_cursor(&request.manifest, next_manifest_index, next_offset)
            .map_err(TransferAttemptError::Fatal)?;
        manifest_index = next_manifest_index;
        offset = next_offset;
        on_progress(TransferExecutionProgress {
            manifest_index: manifest_index.min(request.manifest.entries.len().saturating_sub(1)),
            bytes_transferred: acknowledged_bytes,
            total_bytes: request.manifest.total_bytes,
        });
        if complete {
            break;
        }
        tokio::task::yield_now().await;
    }

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
        } if operation_id == request.operation_id => {
            let _ = stream.finish();
            Ok(TransferExecutionOutcome {
                files_written,
                bytes_written,
            })
        }
        TransferStreamResponse::Error { detail } => Err(TransferAttemptError::Fatal(
            SyncTransferPlanError::Remote(detail),
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
        let header = TransferStreamHeader::from_manifest(
            request.operation_id.clone(),
            request.source_device_id.clone(),
            request.target_device_id.clone(),
            &request.manifest,
            request.target_dir.clone(),
            request.conflict_policy,
        );
        write_json_frame(stream.send_mut(), &TransferStreamRequest::Open { header })
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
        });
        match operation.status {
            TransferJobStatus::Succeeded => {
                return Ok((
                    TransferExecutionOutcome {
                        files_written: operation.files_written,
                        bytes_written: operation.bytes_written,
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
                    return Err(SyncTransferPlanError::RecoveryExhausted(error.to_string()));
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

fn sha256_file(path: &Path) -> Result<String, SyncTransferPlanError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

async fn cleanup_temp_file(path: &Path) -> Result<(), SyncTransferPlanError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn fsync_parent_best_effort(path: &Path) {
    let Some(parent) = path.parent().map(Path::to_path_buf) else {
        return;
    };
    let _ = tokio::task::spawn_blocking(move || {
        std::fs::File::open(parent).and_then(|file| file.sync_all())
    })
    .await;
}

async fn read_json_frame<T, R>(reader: &mut R) -> Result<T, SyncTransferPlanError>
where
    T: for<'de> Deserialize<'de>,
    R: tokio::io::AsyncRead + Unpin,
{
    let len = reader.read_u32().await? as usize;
    let max_frame_bytes = 2 * 1024 * 1024;
    if len > max_frame_bytes {
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
    validate_relative_path(&relative_path)?;
    entries.push(TransferManifestEntry {
        source_path: source_path.canonicalize()?,
        relative_path,
        kind: TransferManifestEntryKind::File,
        size,
    });
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<(), SyncTransferPlanError> {
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
    if !candidate.exists() && !planned_targets.contains(&candidate) {
        return Ok(candidate);
    }
    match policy {
        ReceiveConflictPolicy::Fail => Err(SyncTransferPlanError::TargetConflict(candidate)),
        ReceiveConflictPolicy::Overwrite => Ok(candidate),
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
        if !next.exists() && !planned_targets.contains(&next) {
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
