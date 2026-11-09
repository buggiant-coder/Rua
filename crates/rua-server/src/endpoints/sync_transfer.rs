use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use crate::endpoints::sync::require_sync_enabled;
use crate::errors::ApiError;
use crate::services::device_command_bus::{SyncTransferDeviceEntry, SyncTransferDirectoryListing};
use crate::services::device_context::current_sync_device_id;
use crate::services::sync_transfer::{
    build_transfer_manifest, command_remote_source_send_to, execute_local_transfer_plan,
    plan_receive, send_transfer_stream, ReceiveConflictPolicy, SyncTransferListClient,
    TransferExecutionOptions, TransferStreamSendRequest,
};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sync/transfer/list", post(list_directory))
        .route("/sync/transfer/send", post(send_transfer))
        .route("/sync/transfer/jobs", get(list_jobs))
        .route("/sync/transfer/jobs/{job_id}", get(get_job))
        .route("/sync/transfer/jobs/{job_id}/cancel", post(cancel_job))
}

#[derive(Deserialize)]
struct ListDirectoryRequest {
    device_id: String,
    path: String,
}

#[derive(Serialize)]
struct ListDirectoryResponse {
    device_id: String,
    path: PathBuf,
    entries: Vec<SyncTransferDeviceEntry>,
}

#[derive(Deserialize)]
struct SendTransferRequest {
    source_device_id: String,
    source_paths: Vec<String>,
    target_device_id: String,
    target_dir: String,
    #[serde(default)]
    conflict: Option<String>,
}

async fn list_directory(
    state: axum::extract::State<AppState>,
    Json(body): Json<ListDirectoryRequest>,
) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    if body.device_id.trim().is_empty() {
        return ApiError::BadRequest("device_id must not be empty".into()).into_response();
    }
    if body.path.trim().is_empty() {
        return ApiError::BadRequest("path must not be empty".into()).into_response();
    }

    let Some(local_device_id) = current_sync_device_id(&state.sync_device) else {
        return ApiError::Conflict("local sync device is not initialized".into()).into_response();
    };
    let client = SyncTransferListClient::new(std::sync::Arc::clone(&state.device_network));
    match client
        .list_directory(&local_device_id, &body.device_id, &body.path)
        .await
    {
        Ok(SyncTransferDirectoryListing { path, entries }) => Json(ListDirectoryResponse {
            device_id: body.device_id,
            path,
            entries,
        })
        .into_response(),
        Err(error) => ApiError::Internal(error).into_response(),
    }
}

async fn send_transfer(
    state: axum::extract::State<AppState>,
    Json(body): Json<SendTransferRequest>,
) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    let Some(local_device_id) = current_sync_device_id(&state.sync_device) else {
        return ApiError::Conflict("local sync device is not initialized".into()).into_response();
    };
    if body.source_paths.is_empty() {
        return ApiError::BadRequest("source_paths must not be empty".into()).into_response();
    }
    if body.target_device_id.trim().is_empty() || body.target_dir.trim().is_empty() {
        return ApiError::BadRequest("target_device_id and target_dir must not be empty".into())
            .into_response();
    }
    let conflict = match parse_conflict_policy(body.conflict.as_deref()) {
        Ok(policy) => policy,
        Err(error) => return ApiError::BadRequest(error).into_response(),
    };
    let target_dir = PathBuf::from(&body.target_dir);
    let source_paths = body.source_paths.clone();
    let local_source_manifest = if body.source_device_id == local_device_id {
        let source_paths = source_paths.iter().map(PathBuf::from).collect::<Vec<_>>();
        match build_transfer_manifest(&source_paths) {
            Ok(manifest) => Some(manifest),
            Err(error) => return ApiError::BadRequest(error.to_string()).into_response(),
        }
    } else {
        None
    };
    let (snapshot, cancel_rx) = state.sync_transfer_jobs.start(
        body.source_device_id.clone(),
        body.target_device_id.clone(),
        target_dir.clone(),
        local_source_manifest
            .as_ref()
            .map(|manifest| manifest.total_bytes)
            .unwrap_or(0),
    );
    let job_id = snapshot.job_id.clone();
    let jobs = Arc::clone(&state.sync_transfer_jobs);
    let target_device_id = body.target_device_id.clone();
    let source_device_id = body.source_device_id.clone();
    let device_network = Arc::clone(&state.device_network);
    let local_device_id_for_task = local_device_id.clone();
    tokio::spawn(async move {
        let result = if let Some(manifest) = local_source_manifest {
            if target_device_id == local_device_id_for_task {
                match plan_receive(&target_dir, &manifest, conflict) {
                    Ok(plan) => {
                        let jobs_for_progress = Arc::clone(&jobs);
                        let progress_job_id = job_id.clone();
                        execute_local_transfer_plan(
                            &manifest,
                            &plan,
                            TransferExecutionOptions::default(),
                            cancel_rx,
                            move |progress| {
                                jobs_for_progress
                                    .update_progress(&progress_job_id, progress.bytes_transferred);
                            },
                        )
                        .await
                    }
                    Err(error) => Err(error),
                }
            } else {
                match crate::services::device_session_access::current_session_manager(
                    &device_network,
                )
                .await
                {
                    Ok(session_manager) => {
                        let jobs_for_progress = Arc::clone(&jobs);
                        let progress_job_id = job_id.clone();
                        send_transfer_stream(
                            &session_manager,
                            TransferStreamSendRequest {
                                operation_id: job_id.clone(),
                                source_device_id: local_device_id_for_task.clone(),
                                target_device_id: target_device_id.clone(),
                                manifest,
                                target_dir,
                                conflict_policy: conflict,
                                options: TransferExecutionOptions::default(),
                            },
                            cancel_rx,
                            move |progress| {
                                jobs_for_progress
                                    .update_progress(&progress_job_id, progress.bytes_transferred);
                            },
                        )
                        .await
                    }
                    Err(error) => Err(crate::services::sync_transfer::SyncTransferPlanError::Io(
                        std::io::Error::other(error),
                    )),
                }
            }
        } else {
            let session_manager =
                crate::services::device_session_access::current_session_manager(&device_network)
                    .await;
            match session_manager {
                Ok(session_manager) => {
                    let jobs_for_manifest = Arc::clone(&jobs);
                    let manifest_job_id = job_id.clone();
                    let jobs_for_progress = Arc::clone(&jobs);
                    let progress_job_id = job_id.clone();
                    command_remote_source_send_to(
                        &session_manager,
                        crate::services::device_command_bus::SyncTransferDeviceCommand {
                            command_id: format!("sync-transfer-send-{}", uuid::Uuid::new_v4()),
                            source_device_id: local_device_id_for_task.clone(),
                            target_device_id: source_device_id.clone(),
                            action:
                                crate::services::device_command_bus::SyncTransferDeviceAction::StartSend {
                                    operation_id: job_id.clone(),
                                    source_paths: source_paths.clone(),
                                    transfer_target_device_id: target_device_id.clone(),
                                    target_dir: target_dir.to_string_lossy().to_string(),
                                    conflict: sync_transfer_conflict_for_command(conflict),
                                },
                        },
                        cancel_rx,
                        move |total_bytes| {
                            jobs_for_manifest.update_total_bytes(&manifest_job_id, total_bytes);
                        },
                        move |progress| {
                            jobs_for_progress
                                .update_progress(&progress_job_id, progress.bytes_transferred);
                        },
                    )
                    .await
                }
                Err(error) => Err(crate::services::sync_transfer::SyncTransferPlanError::Io(
                    std::io::Error::other(error),
                )),
            }
        };

        match result {
            Ok(outcome) => jobs.succeed(&job_id, outcome),
            Err(error) => jobs.fail(&job_id, error.to_string()),
        }
    });

    Json(snapshot).into_response()
}

async fn list_jobs(state: axum::extract::State<AppState>) -> Response {
    Json(state.sync_transfer_jobs.list()).into_response()
}

async fn get_job(state: axum::extract::State<AppState>, Path(job_id): Path<String>) -> Response {
    match state.sync_transfer_jobs.get(&job_id) {
        Some(job) => Json(job).into_response(),
        None => ApiError::NotFound(format!("transfer job not found: {job_id}")).into_response(),
    }
}

async fn cancel_job(state: axum::extract::State<AppState>, Path(job_id): Path<String>) -> Response {
    match state.sync_transfer_jobs.cancel(&job_id) {
        Some(job) => Json(job).into_response(),
        None => ApiError::NotFound(format!("transfer job not found: {job_id}")).into_response(),
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

fn parse_conflict_policy(value: Option<&str>) -> Result<ReceiveConflictPolicy, String> {
    match value.unwrap_or("rename") {
        "rename" => Ok(ReceiveConflictPolicy::Rename),
        "fail" => Ok(ReceiveConflictPolicy::Fail),
        "overwrite" => Ok(ReceiveConflictPolicy::Overwrite),
        other => Err(format!("unsupported conflict policy: {other}")),
    }
}
