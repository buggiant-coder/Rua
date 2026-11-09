use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use crate::services::device_context::current_sync_device_id;
use crate::services::sync_transfer::{
    build_transfer_manifest, command_remote_source_send_to, execute_local_transfer_plan,
    plan_receive, send_transfer_stream, ReceiveConflictPolicy, SyncTransferListClient,
    TransferExecutionOptions, TransferJobSnapshot, TransferJobStore, TransferStreamSendRequest,
};

#[derive(Clone)]
pub struct SyncTransferToolService {
    sync_enabled: Arc<AtomicBool>,
    sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    jobs: Arc<TransferJobStore>,
}

impl SyncTransferToolService {
    pub fn new(
        sync_enabled: Arc<AtomicBool>,
        sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
        device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
        jobs: Arc<TransferJobStore>,
    ) -> Self {
        Self {
            sync_enabled,
            sync_device,
            device_network,
            jobs,
        }
    }

    fn ensure_sync_enabled(&self) -> rua_core::Result<String> {
        if !self.sync_enabled.load(Ordering::SeqCst) {
            return Err(tool_error("multi-device sync is disabled"));
        }
        current_sync_device_id(&self.sync_device)
            .ok_or_else(|| tool_error("local sync device is not initialized"))
    }

    async fn list_directory(
        &self,
        target_device_id: &str,
        path: &str,
    ) -> rua_core::Result<serde_json::Value> {
        let local_device_id = self.ensure_sync_enabled()?;
        let client = SyncTransferListClient::new(Arc::clone(&self.device_network));
        let listing = client
            .list_directory(&local_device_id, target_device_id, path)
            .await
            .map_err(tool_error)?;
        Ok(serde_json::json!({
            "device_id": target_device_id,
            "path": listing.path,
            "entries": listing.entries,
        }))
    }

    async fn send(&self, request: SyncSendArgs) -> rua_core::Result<TransferJobSnapshot> {
        let local_device_id = self.ensure_sync_enabled()?;
        if request.source_paths.is_empty() {
            return Err(tool_error("source_paths must not be empty"));
        }
        if request.source_device_id.trim().is_empty()
            || request.target_device_id.trim().is_empty()
            || request.target_dir.trim().is_empty()
        {
            return Err(tool_error(
                "source_device_id, target_device_id, and target_dir must not be empty",
            ));
        }
        let conflict = parse_conflict_policy(request.conflict.as_deref())?;
        let target_dir = PathBuf::from(&request.target_dir);
        let local_source_manifest = if request.source_device_id == local_device_id {
            let source_paths = request
                .source_paths
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            Some(
                build_transfer_manifest(&source_paths)
                    .map_err(|error| tool_error(error.to_string()))?,
            )
        } else {
            None
        };
        let (snapshot, cancel_rx) = self.jobs.start(
            request.source_device_id.clone(),
            request.target_device_id.clone(),
            target_dir.clone(),
            local_source_manifest
                .as_ref()
                .map(|manifest| manifest.total_bytes)
                .unwrap_or(0),
        );
        let job_id = snapshot.job_id.clone();
        let jobs = Arc::clone(&self.jobs);
        let device_network = Arc::clone(&self.device_network);
        let source_paths = request.source_paths.clone();
        let source_device_id = request.source_device_id.clone();
        let target_device_id = request.target_device_id.clone();
        tokio::spawn(async move {
            let result = run_transfer_job(
                jobs.clone(),
                job_id.clone(),
                device_network,
                local_device_id,
                source_device_id,
                source_paths,
                target_device_id,
                target_dir,
                conflict,
                local_source_manifest,
                cancel_rx,
            )
            .await;
            match result {
                Ok(outcome) => jobs.succeed(&job_id, outcome),
                Err(error) => jobs.fail(&job_id, error.to_string()),
            }
        });
        Ok(snapshot)
    }
}

pub struct SyncTransferRuntimeAdapter {
    service: Arc<OnceLock<Arc<SyncTransferToolService>>>,
}

impl SyncTransferRuntimeAdapter {
    pub fn new(service: Arc<OnceLock<Arc<SyncTransferToolService>>>) -> Self {
        Self { service }
    }

    fn service(&self) -> Result<Arc<SyncTransferToolService>, String> {
        self.service
            .get()
            .cloned()
            .ok_or_else(|| "Sync Transfer service unavailable".to_string())
    }
}

#[async_trait]
impl rua_tool::extended::sync_transfer::SyncTransferRuntime for SyncTransferRuntimeAdapter {
    async fn execute(
        &self,
        request: rua_tool::extended::sync_transfer::SyncTransferRequest,
    ) -> Result<serde_json::Value, String> {
        let service = self.service()?;
        match request {
            rua_tool::extended::sync_transfer::SyncTransferRequest::List {
                target_device_id,
                path,
            } => service
                .list_directory(&target_device_id, &path)
                .await
                .map_err(|error| error.to_string()),
            rua_tool::extended::sync_transfer::SyncTransferRequest::Send {
                source_device_id,
                source_paths,
                target_device_id,
                target_dir,
                conflict,
            } => serde_json::to_value(
                service
                    .send(SyncSendArgs {
                        source_device_id,
                        source_paths,
                        target_device_id,
                        target_dir,
                        conflict,
                    })
                    .await
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string()),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_transfer_job(
    jobs: Arc<TransferJobStore>,
    job_id: String,
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    local_device_id: String,
    source_device_id: String,
    source_paths: Vec<String>,
    target_device_id: String,
    target_dir: PathBuf,
    conflict: ReceiveConflictPolicy,
    local_source_manifest: Option<crate::services::sync_transfer::TransferManifest>,
    cancel_rx: tokio::sync::watch::Receiver<rua_core::RunControl>,
) -> Result<
    crate::services::sync_transfer::TransferExecutionOutcome,
    crate::services::sync_transfer::SyncTransferPlanError,
> {
    if let Some(manifest) = local_source_manifest {
        if target_device_id == local_device_id {
            let plan = plan_receive(&target_dir, &manifest, conflict)?;
            let jobs_for_progress = Arc::clone(&jobs);
            let progress_job_id = job_id.clone();
            return execute_local_transfer_plan(
                &manifest,
                &plan,
                TransferExecutionOptions::default(),
                cancel_rx,
                move |progress| {
                    jobs_for_progress.update_progress(&progress_job_id, progress.bytes_transferred);
                },
            )
            .await;
        }
        let session_manager =
            crate::services::device_session_access::current_session_manager(&device_network)
                .await
                .map_err(|error| {
                    crate::services::sync_transfer::SyncTransferPlanError::Io(
                        std::io::Error::other(error),
                    )
                })?;
        let jobs_for_progress = Arc::clone(&jobs);
        let progress_job_id = job_id.clone();
        return send_transfer_stream(
            &session_manager,
            TransferStreamSendRequest {
                operation_id: job_id,
                source_device_id: local_device_id,
                target_device_id,
                manifest,
                target_dir,
                conflict_policy: conflict,
                options: TransferExecutionOptions::default(),
            },
            cancel_rx,
            move |progress| {
                jobs_for_progress.update_progress(&progress_job_id, progress.bytes_transferred);
            },
        )
        .await;
    }

    let session_manager =
        crate::services::device_session_access::current_session_manager(&device_network)
            .await
            .map_err(|error| {
                crate::services::sync_transfer::SyncTransferPlanError::Io(std::io::Error::other(
                    error,
                ))
            })?;
    let source_command = crate::services::device_command_bus::SyncTransferDeviceCommand {
        command_id: format!("sync-transfer-send-{}", uuid::Uuid::new_v4()),
        source_device_id: local_device_id.clone(),
        target_device_id: source_device_id.clone(),
        action: crate::services::device_command_bus::SyncTransferDeviceAction::StartSend {
            operation_id: job_id.clone(),
            source_paths,
            transfer_target_device_id: target_device_id.clone(),
            target_dir: target_dir.to_string_lossy().to_string(),
            conflict: sync_transfer_conflict_for_command(conflict),
        },
    };
    let jobs_for_manifest = Arc::clone(&jobs);
    let manifest_job_id = job_id.clone();
    let jobs_for_progress = Arc::clone(&jobs);
    let progress_job_id = job_id.clone();
    command_remote_source_send_to(
        &session_manager,
        source_command,
        cancel_rx,
        move |total_bytes| jobs_for_manifest.update_total_bytes(&manifest_job_id, total_bytes),
        move |progress| {
            jobs_for_progress.update_progress(&progress_job_id, progress.bytes_transferred);
        },
    )
    .await
}

#[derive(Deserialize)]
struct SyncSendArgs {
    source_device_id: String,
    source_paths: Vec<String>,
    target_device_id: String,
    target_dir: String,
    #[serde(default)]
    conflict: Option<String>,
}

fn parse_conflict_policy(value: Option<&str>) -> rua_core::Result<ReceiveConflictPolicy> {
    match value.unwrap_or("rename") {
        "rename" => Ok(ReceiveConflictPolicy::Rename),
        "fail" => Ok(ReceiveConflictPolicy::Fail),
        "overwrite" => Ok(ReceiveConflictPolicy::Overwrite),
        other => Err(tool_error(format!("unsupported conflict policy: {other}"))),
    }
}

fn sync_transfer_conflict_for_command(
    value: ReceiveConflictPolicy,
) -> crate::services::device_command_bus::SyncTransferConflictPolicy {
    match value {
        ReceiveConflictPolicy::Rename => {
            crate::services::device_command_bus::SyncTransferConflictPolicy::Rename
        }
        ReceiveConflictPolicy::Fail => {
            crate::services::device_command_bus::SyncTransferConflictPolicy::Fail
        }
        ReceiveConflictPolicy::Overwrite => {
            crate::services::device_command_bus::SyncTransferConflictPolicy::Overwrite
        }
    }
}

fn tool_error(message: impl Into<String>) -> rua_core::RuaError {
    rua_core::RuaError::Tool {
        tool: "sync_transfer".into(),
        message: message.into(),
    }
}
