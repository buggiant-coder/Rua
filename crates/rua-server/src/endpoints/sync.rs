use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use serde::{Deserialize, Serialize};

use crate::errors::ApiError;
use crate::state::AppState;
use crate::sync_network::BoundSyncNetworkError;

pub fn require_sync_enabled(state: &AppState) -> Option<Response> {
    if !state.sync_enabled.load(Ordering::SeqCst) {
        Some(error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "not_enabled",
            "sync is disabled; enable via POST /sync/enable".into(),
        ))
    } else {
        None
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sync/cycle", post(trigger_sync_cycle))
        .route("/sync/cycle/target", post(trigger_targeted_sync_cycle))
        .route(
            "/sync/scheduled-background",
            post(trigger_scheduled_background_sync),
        )
        .route("/sync/conflicts", get(get_conflict_history))
        .route("/sync/conflicts/recent", get(get_recent_conflicts))
        .route(
            "/sync/conflict-policy",
            get(get_conflict_policy).put(set_conflict_policy),
        )
        .route(
            "/sync/power-policy",
            get(get_power_policy).put(set_power_policy),
        )
        .route("/sync/conflicts/pending", get(get_pending_conflicts))
        .route("/sync/conflicts/resolve", post(resolve_conflict))
        .route("/sync/enable", post(enable_sync))
        .route("/sync/network/create", post(create_sync_network))
        .route("/sync/disable", post(disable_sync))
        .route("/sync/config", get(get_sync_config))
        .route("/sync/status", get(get_sync_status))
        .route("/sync/diagnostics", get(get_sync_diagnostics))
        .route("/sync/inbox/retry-failed", post(retry_failed_inbox))
        .route("/sync/repair/scan", post(repair_scan_inbox))
        .route("/sync/repair/full-export", post(repair_full_export))
}

#[derive(Serialize)]
struct SyncCycleResponse {
    events_received: usize,
    events_sent: usize,
    peer_node_id: String,
    duration_ms: u64,
    transport_duration_ms: u64,
    local_apply_duration_ms: u64,
    conflicts_detected: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    repair: Option<SyncRepairScanResponse>,
}

#[derive(Deserialize)]
struct TargetedSyncCycleRequest {
    target_device_id: String,
}

#[derive(Serialize)]
struct ScheduledBackgroundSyncResponse {
    action: &'static str,
    host_profile: rua_sync::power_policy::SyncRuntimeHostProfile,
    scheduled: bool,
    duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    events_received: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    peer_node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conflicts_detected: Option<usize>,
}

#[derive(Serialize)]
struct SyncErrorResponse {
    reason: &'static str,
    detail: String,
}

#[derive(Serialize)]
struct SyncRepairFullExportResponse {
    exported: bool,
}

#[derive(Serialize)]
struct RetryFailedInboxResponse {
    retried: usize,
}

fn error_response(status: StatusCode, reason: &'static str, detail: String) -> Response {
    (status, Json(SyncErrorResponse { reason, detail })).into_response()
}

fn sync_runtime_error_response(
    error: crate::sync_runtime_controller::SyncRuntimeError,
) -> Response {
    let status = match error.reason() {
        "not_paired" | "runtime_suspended" | "runtime_unavailable" => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, error.reason(), error.detail().to_string())
}

async fn trigger_sync_cycle(state: axum::extract::State<AppState>) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    let start = std::time::Instant::now();
    let app_state = state.0.clone();
    let result = app_state
        .sync_runtime
        .trigger_manual_sync({
            let app_state = app_state.clone();
            move || async move { app_state.build_sync_runtime_parts().await }
        })
        .await;
    let transport_duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(cycle_result) => {
            let local_apply_started = std::time::Instant::now();
            let report = match app_state.repair_scan_and_drain_sync_inbox_staged().await {
                Ok(report) if report.errors.is_empty() => report,
                Ok(report) => {
                    let local_apply_duration_ms = local_apply_started.elapsed().as_millis() as u64;
                    record_sync_now_diagnostics(
                        &app_state,
                        Some(cycle_result.peer_node_id.clone()),
                        rua_sync::stage_diagnostics::SyncStageStatus::Success,
                        rua_sync::stage_diagnostics::SyncNowLocalCompletionStatus::Failure,
                        transport_duration_ms,
                        local_apply_duration_ms,
                        start.elapsed().as_millis() as u64,
                        Some(rua_sync::stage_diagnostics::SyncFailureStage::RepairScan),
                    );
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "repair_scan_failed",
                        format!(
                            "Repair Scan completed with {} error(s): {}",
                            report.errors.len(),
                            report.errors.join("; ")
                        ),
                    );
                }
                Err(crate::sync_inbound_apply::LocalSyncCompletionError::RepairScan(error)) => {
                    let local_apply_duration_ms = local_apply_started.elapsed().as_millis() as u64;
                    record_sync_now_diagnostics(
                        &app_state,
                        Some(cycle_result.peer_node_id.clone()),
                        rua_sync::stage_diagnostics::SyncStageStatus::Success,
                        rua_sync::stage_diagnostics::SyncNowLocalCompletionStatus::Failure,
                        transport_duration_ms,
                        local_apply_duration_ms,
                        start.elapsed().as_millis() as u64,
                        Some(rua_sync::stage_diagnostics::SyncFailureStage::RepairScan),
                    );
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "repair_scan_failed",
                        error.to_string(),
                    );
                }
                Err(crate::sync_inbound_apply::LocalSyncCompletionError::InboxDrain(error)) => {
                    let local_apply_duration_ms = local_apply_started.elapsed().as_millis() as u64;
                    record_sync_now_diagnostics(
                        &app_state,
                        Some(cycle_result.peer_node_id.clone()),
                        rua_sync::stage_diagnostics::SyncStageStatus::Success,
                        rua_sync::stage_diagnostics::SyncNowLocalCompletionStatus::Failure,
                        transport_duration_ms,
                        local_apply_duration_ms,
                        start.elapsed().as_millis() as u64,
                        Some(rua_sync::stage_diagnostics::SyncFailureStage::InboundInbox),
                    );
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "inbox_drain_failed",
                        error.to_string(),
                    );
                }
            };
            let local_apply_duration_ms = local_apply_started.elapsed().as_millis() as u64;
            record_sync_now_diagnostics(
                &app_state,
                Some(cycle_result.peer_node_id.clone()),
                rua_sync::stage_diagnostics::SyncStageStatus::Success,
                rua_sync::stage_diagnostics::SyncNowLocalCompletionStatus::Success,
                transport_duration_ms,
                local_apply_duration_ms,
                start.elapsed().as_millis() as u64,
                None,
            );
            let resp = SyncCycleResponse {
                events_received: cycle_result.events_received,
                events_sent: cycle_result.events_sent,
                peer_node_id: cycle_result.peer_node_id,
                duration_ms: start.elapsed().as_millis() as u64,
                transport_duration_ms,
                local_apply_duration_ms,
                conflicts_detected: cycle_result.conflicts.len(),
                repair: Some(SyncRepairScanResponse::from(report)),
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => {
            let selected_peer_node_id =
                crate::sync_cycle::load_transport_stats(&app_state.rua_root.join("sync"))
                    .last_cycle
                    .as_ref()
                    .map(|cycle| cycle.peer_node_id.clone());
            record_sync_now_diagnostics(
                &app_state,
                selected_peer_node_id,
                rua_sync::stage_diagnostics::SyncStageStatus::Failure,
                rua_sync::stage_diagnostics::SyncNowLocalCompletionStatus::NotStarted,
                transport_duration_ms,
                0,
                start.elapsed().as_millis() as u64,
                Some(rua_sync::stage_diagnostics::SyncFailureStage::Transport),
            );
            if e.reason() == "cycle_failed" {
                let detail = e.detail().to_string();
                let reason = if detail.contains("not_paired") {
                    "not_paired"
                } else if detail.contains("no_eligible_peer") {
                    "no_eligible_peer"
                } else if detail.contains("Timeout") || detail.contains("timeout") {
                    "timeout"
                } else {
                    "peer_unreachable"
                };
                let status = if reason == "not_paired" || reason == "no_eligible_peer" {
                    StatusCode::UNPROCESSABLE_ENTITY
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                };
                error_response(status, reason, detail)
            } else {
                sync_runtime_error_response(e)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn record_sync_now_diagnostics(
    state: &AppState,
    selected_peer_node_id: Option<String>,
    transport_status: rua_sync::stage_diagnostics::SyncStageStatus,
    local_completion_status: rua_sync::stage_diagnostics::SyncNowLocalCompletionStatus,
    transport_duration_ms: u64,
    local_apply_duration_ms: u64,
    duration_ms: u64,
    failure_stage: Option<rua_sync::stage_diagnostics::SyncFailureStage>,
) {
    state
        .sync_stage_recorder
        .record_sync_now(rua_sync::stage_diagnostics::SyncNowStageSample {
            completed_at_ms: rua_sync::stage_diagnostics::wall_now_ms(),
            selected_peer_node_id,
            transport_status,
            local_completion_status,
            transport_duration_ms,
            local_apply_duration_ms,
            duration_ms,
            failure_stage,
        });
}

async fn trigger_targeted_sync_cycle(
    state: axum::extract::State<AppState>,
    Json(body): Json<TargetedSyncCycleRequest>,
) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    let start = std::time::Instant::now();
    let app_state = state.0.clone();
    if let Err(error) = app_state
        .sync_runtime
        .ensure_active({
            let app_state = app_state.clone();
            move || async move { app_state.build_sync_runtime_parts().await }
        })
        .await
    {
        return sync_runtime_error_response(error);
    }

    let result = crate::sync_cycle::run_sync_cycle_to_device_id(
        &app_state.sync_node,
        &app_state.rua_root.join("sync"),
        &body.target_device_id,
    )
    .await;
    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(cycle_result) => {
            let resp = SyncCycleResponse {
                events_received: cycle_result.events_received,
                events_sent: cycle_result.events_sent,
                peer_node_id: cycle_result.peer_node_id,
                duration_ms,
                transport_duration_ms: duration_ms,
                local_apply_duration_ms: 0,
                conflicts_detected: cycle_result.conflicts.len(),
                repair: None,
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(detail) => {
            let reason = if detail.contains(crate::sync_cycle::SYNC_CYCLE_QUEUE_TIMEOUT) {
                crate::sync_cycle::SYNC_CYCLE_QUEUE_TIMEOUT
            } else if detail.contains(crate::sync_cycle::SYNC_CYCLE_DEADLINE_TIMEOUT) {
                crate::sync_cycle::SYNC_CYCLE_DEADLINE_TIMEOUT
            } else if detail.contains("not_paired") {
                "not_paired"
            } else if detail.contains("target device not found")
                || detail.contains("invalid target device id")
                || detail.contains("target device is this device")
            {
                "invalid_target"
            } else if detail.contains("Timeout") || detail.contains("timeout") {
                "timeout"
            } else {
                "peer_unreachable"
            };
            let status = if reason == crate::sync_cycle::SYNC_CYCLE_QUEUE_TIMEOUT
                || reason == crate::sync_cycle::SYNC_CYCLE_DEADLINE_TIMEOUT
            {
                StatusCode::SERVICE_UNAVAILABLE
            } else if reason == "invalid_target" || reason == "not_paired" {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            error_response(status, reason, detail)
        }
    }
}

async fn trigger_scheduled_background_sync(state: axum::extract::State<AppState>) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }

    let host_profile = state.sync_runtime_host_profile();
    if host_profile != rua_sync::power_policy::SyncRuntimeHostProfile::MobileApk {
        return (
            StatusCode::ACCEPTED,
            Json(ScheduledBackgroundSyncResponse {
                action: "unsupported_host_profile",
                host_profile,
                scheduled: false,
                duration_ms: 0,
                events_received: None,
                peer_node_id: None,
                conflicts_detected: None,
            }),
        )
            .into_response();
    }

    let policy = state.effective_sync_power_policy();
    if !policy.allow_background_scheduled_sync {
        return (
            StatusCode::ACCEPTED,
            Json(ScheduledBackgroundSyncResponse {
                action: "scheduled_sync_disabled",
                host_profile,
                scheduled: false,
                duration_ms: 0,
                events_received: None,
                peer_node_id: None,
                conflicts_detected: None,
            }),
        )
            .into_response();
    }

    let start = std::time::Instant::now();
    let app_state = state.0.clone();
    let result = app_state
        .sync_runtime
        .trigger_scheduled_background_sync_with_suspend(
            {
                let app_state = app_state.clone();
                move || async move { app_state.build_sync_runtime_parts().await }
            },
            policy.allow_background_runtime || app_state.foreground_operations.has_active(),
            {
                let app_state = app_state.clone();
                move || async move {
                    app_state.suspend_device_network_runtime().await;
                }
            },
        )
        .await;
    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(cycle_result) => (
            StatusCode::OK,
            Json(ScheduledBackgroundSyncResponse {
                action: "scheduled_sync_completed",
                host_profile,
                scheduled: true,
                duration_ms,
                events_received: Some(cycle_result.events_received),
                peer_node_id: Some(cycle_result.peer_node_id),
                conflicts_detected: Some(cycle_result.conflicts.len()),
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(
                reason = e.reason(),
                detail = e.detail(),
                "[sync] scheduled background sync attempt failed"
            );
            sync_runtime_error_response(e)
        }
    }
}

#[derive(Serialize)]
struct ConflictHistoryResponse {
    conflicts: Vec<rua_sync::conflict::ConflictRecord>,
    count: usize,
}

async fn get_conflict_history(state: axum::extract::State<AppState>) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }

    let conflicts: Vec<rua_sync::conflict::ConflictRecord> = Vec::new();
    let count = conflicts.len();
    (
        StatusCode::OK,
        Json(ConflictHistoryResponse { conflicts, count }),
    )
        .into_response()
}

#[derive(Serialize)]
struct RecentConflictEntry {
    id: usize,
    target_id: String,
    before_value: String,
    after_value: String,
    originating_device: String,
    hlc: String,
    observed_at: u64,
}

#[derive(Serialize)]
struct RecentConflictsResponse {
    conflicts: Vec<RecentConflictEntry>,
    count: usize,
}

async fn get_recent_conflicts(state: axum::extract::State<AppState>) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }

    let conflicts: Vec<RecentConflictEntry> = Vec::new();
    let count = conflicts.len();
    (
        StatusCode::OK,
        Json(RecentConflictsResponse { conflicts, count }),
    )
        .into_response()
}

use std::sync::atomic::Ordering;

#[derive(Serialize)]
struct SyncStatusResponse {
    enabled: bool,
    paired: bool,
    membership: crate::sync_network::SyncNetworkMembership,
    peer_count: usize,
    host_profile: rua_sync::power_policy::SyncRuntimeHostProfile,
    effective_power_policy: rua_sync::power_policy::EffectiveSyncPowerPolicy,
}

#[derive(Serialize)]
struct SyncConfigResponse {
    enabled: bool,
}

#[derive(Serialize)]
struct SyncDiagnosticsResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    device_network: Option<rua_device_network::DeviceNetworkDiagnostics>,
    host_profile: rua_sync::power_policy::SyncRuntimeHostProfile,
    policy: Option<rua_sync::power_policy::SyncPowerPolicyConfig>,
    effective_policy: rua_sync::power_policy::EffectiveSyncPowerPolicy,
    runtime_state: crate::sync_runtime_controller::SyncRuntimeEffectiveStatus,
    background_grace: crate::sync_runtime_controller::SyncRuntimeBackgroundGraceStatus,
    warm_suspended: bool,
    scheduled: SyncScheduledDiagnostics,
    identity: SyncIdentityDiagnostics,
    inbox: rua_sync::inbound_inbox::InboxDiagnostics,
    merge: rua_sync::sidecar::SidecarDiagnostics,
    progress: SyncProgressDiagnostics,
    transport: crate::sync_cycle::SyncTransportStatsState,
    blob: rua_sync::sync_blob_store::BlobDownloadDiagnostics,
    stages: rua_sync::stage_diagnostics::SyncStageDiagnosticsSnapshot,
    peers: crate::sync_cycle::PeerHealthDiagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SyncProgressState {
    Disabled,
    NotPaired,
    IdentityMismatch,
    Syncing,
    UpToDate,
    ApplyingChanges,
    WaitingForContent,
    WaitingForDevice,
    NeedsAttention,
}

#[derive(Serialize)]
struct SyncProgressDiagnostics {
    state: SyncProgressState,
    is_caught_up: bool,
    apply_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_lane: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_lane: Option<&'static str>,
    pending_inbound: u64,
    retryable_inbound: u64,
    dormant_inbound: u64,
    failed_inbound: u64,
    pending_conflicts: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_cycle_status: Option<crate::sync_cycle::SyncTransportCycleStatus>,
}

#[derive(Serialize)]
struct SyncScheduledDiagnostics {
    allow_background_scheduled_sync: bool,
    allow_background_runtime: bool,
}

#[derive(Serialize)]
struct SyncIdentityDiagnostics {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Serialize)]
struct SyncRepairScanResponse {
    scanned: usize,
    recorded: usize,
    already_applied: usize,
    errors: Vec<String>,
    duration_ms: u64,
}

impl From<rua_sync::inbound_inbox::RepairScanReport> for SyncRepairScanResponse {
    fn from(report: rua_sync::inbound_inbox::RepairScanReport) -> Self {
        Self {
            scanned: report.scanned,
            recorded: report.recorded,
            already_applied: report.already_applied,
            errors: report.errors,
            duration_ms: report.duration_ms,
        }
    }
}

async fn get_sync_status(state: axum::extract::State<AppState>) -> Response {
    let enabled = state.sync_enabled.load(Ordering::SeqCst);
    let membership = state.sync_network_membership().await;
    let paired = matches!(
        membership.state,
        crate::sync_network::SyncNetworkMembershipState::Bound
    );
    let peer_count = {
        let dl_path = state.rua_root.join("sync").join("device_list.json");
        std::fs::read_to_string(&dl_path)
            .ok()
            .and_then(|c| serde_json::from_str::<rua_sync::device_list::DeviceList>(&c).ok())
            .map(|dl| dl.devices.len())
            .unwrap_or(0)
    };
    (
        StatusCode::OK,
        Json(SyncStatusResponse {
            enabled,
            paired,
            membership,
            peer_count,
            host_profile: state.profile.sync_runtime_host_profile(),
            effective_power_policy: state
                .sync_power_policy
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .effective_policy
                .clone(),
        }),
    )
        .into_response()
}

async fn get_sync_config(state: axum::extract::State<AppState>) -> Response {
    (
        StatusCode::OK,
        Json(SyncConfigResponse {
            enabled: state.sync_enabled.load(Ordering::SeqCst),
        }),
    )
        .into_response()
}

#[derive(Serialize)]
struct SyncPowerPolicyResponse {
    host_profile: rua_sync::power_policy::SyncRuntimeHostProfile,
    stored_policy: Option<rua_sync::power_policy::SyncPowerPolicyConfig>,
    effective_policy: rua_sync::power_policy::EffectiveSyncPowerPolicy,
}

impl From<rua_sync::power_policy::LoadedSyncPowerPolicy> for SyncPowerPolicyResponse {
    fn from(policy: rua_sync::power_policy::LoadedSyncPowerPolicy) -> Self {
        Self {
            host_profile: policy.host_profile,
            stored_policy: policy.stored_policy,
            effective_policy: policy.effective_policy,
        }
    }
}

#[derive(Deserialize)]
struct SyncPowerPolicyRequest {
    mode: String,
    periodic_interval_secs: Option<u64>,
    background_flush_grace_secs: Option<u64>,
    allow_background_runtime: Option<bool>,
    allow_background_scheduled_sync: Option<bool>,
}

impl TryFrom<SyncPowerPolicyRequest> for rua_sync::power_policy::SyncPowerPolicyConfig {
    type Error = rua_sync::power_policy::PowerPolicyError;

    fn try_from(request: SyncPowerPolicyRequest) -> Result<Self, Self::Error> {
        let mode = request.mode.parse()?;
        let policy = Self {
            mode,
            periodic_interval_secs: request.periodic_interval_secs,
            background_flush_grace_secs: request.background_flush_grace_secs,
            allow_background_runtime: request.allow_background_runtime,
            allow_background_scheduled_sync: request.allow_background_scheduled_sync,
        };
        policy.validate()?;
        Ok(policy)
    }
}

async fn get_power_policy(state: axum::extract::State<AppState>) -> Response {
    let host_profile = state.profile.sync_runtime_host_profile();
    match rua_sync::power_policy::LoadedSyncPowerPolicy::load(&state.rua_root, host_profile) {
        Ok(policy) => {
            *state
                .sync_power_policy
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = policy.clone();
            (StatusCode::OK, Json(SyncPowerPolicyResponse::from(policy))).into_response()
        }
        Err(e) => power_policy_load_error(e),
    }
}

async fn set_power_policy(
    state: axum::extract::State<AppState>,
    Json(request): Json<SyncPowerPolicyRequest>,
) -> Response {
    let policy = match rua_sync::power_policy::SyncPowerPolicyConfig::try_from(request) {
        Ok(policy) => policy,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_power_policy",
                e.to_string(),
            );
        }
    };
    let host_profile = state.profile.sync_runtime_host_profile();
    match rua_sync::power_policy::LoadedSyncPowerPolicy::save(&state.rua_root, host_profile, policy)
    {
        Ok(policy) => {
            *state
                .sync_power_policy
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = policy.clone();
            (StatusCode::OK, Json(SyncPowerPolicyResponse::from(policy))).into_response()
        }
        Err(e) => power_policy_save_error(e),
    }
}

fn power_policy_load_error(error: rua_sync::SyncError) -> Response {
    match error {
        rua_sync::SyncError::Json(e) => error_response(
            StatusCode::BAD_REQUEST,
            "invalid_power_policy",
            format!("failed to parse sync power policy: {e}"),
        ),
        rua_sync::SyncError::Store(e) => {
            error_response(StatusCode::BAD_REQUEST, "invalid_power_policy", e)
        }
        e => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "power_policy_load_failed",
            format!("failed to load sync power policy: {e}"),
        ),
    }
}

fn power_policy_save_error(error: rua_sync::SyncError) -> Response {
    match error {
        rua_sync::SyncError::Store(e) => {
            error_response(StatusCode::BAD_REQUEST, "invalid_power_policy", e)
        }
        e => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "power_policy_save_failed",
            format!("failed to save sync power policy: {e}"),
        ),
    }
}

async fn get_sync_diagnostics(state: axum::extract::State<AppState>) -> Response {
    let sync_enabled = state.sync_enabled.load(Ordering::SeqCst);
    let mut identity_mismatch = false;
    let mut not_paired = false;
    let (identity, self_pubkey_bytes) = if sync_enabled {
        let node = state.sync_node.lock().await.as_ref().cloned();
        let self_pubkey_bytes = match node.as_ref() {
            Some(node) => *node.node_id().as_bytes(),
            None => {
                not_paired = true;
                [0u8; 32]
            }
        };

        if not_paired {
            (
                SyncIdentityDiagnostics {
                    status: "not_paired",
                    namespace_id: None,
                    detail: None,
                },
                None,
            )
        } else {
            let network_secret = state
                .sync_network_secret
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().map(Arc::clone));
            let device = state
                .sync_device
                .read()
                .ok()
                .and_then(|guard| guard.as_ref().map(Arc::clone));
            match crate::sync_network::BoundSyncNetwork::from_optional_parts(
                &state.rua_root.join("sync"),
                node.as_ref(),
                network_secret,
                device,
            ) {
                Ok(bound) => (
                    SyncIdentityDiagnostics {
                        status: "bound",
                        namespace_id: Some(bound.namespace_id_hex()),
                        detail: None,
                    },
                    Some(self_pubkey_bytes),
                ),
                Err(
                    BoundSyncNetworkError::NotPaired | BoundSyncNetworkError::MissingDiskSecret,
                ) => (
                    SyncIdentityDiagnostics {
                        status: "not_paired",
                        namespace_id: None,
                        detail: None,
                    },
                    Some(self_pubkey_bytes),
                ),
                Err(error) => {
                    identity_mismatch = true;
                    (
                        SyncIdentityDiagnostics {
                            status: "identity_mismatch",
                            namespace_id: None,
                            detail: Some(error.to_string()),
                        },
                        Some(self_pubkey_bytes),
                    )
                }
            }
        }
    } else {
        (
            SyncIdentityDiagnostics {
                status: "disabled",
                namespace_id: None,
                detail: None,
            },
            None,
        )
    };
    let inbox = match state.inbound_inbox.diagnostics(20) {
        Ok(diagnostics) => diagnostics,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "diagnostics_failed",
                format!("read inbound inbox diagnostics failed: {e}"),
            );
        }
    };
    let merge_store =
        rua_sync::merge_store::MergeStore::from_shared(Arc::clone(&state.sidecar_store));
    let merge = match merge_store.diagnostics() {
        Ok(diagnostics) => diagnostics,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "diagnostics_failed",
                format!("read merge diagnostics failed: {e}"),
            );
        }
    };
    let pending_conflicts = match merge_store.list_pending_conflicts() {
        Ok(conflicts) => conflicts.len(),
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "diagnostics_failed",
                format!("read pending conflict diagnostics failed: {e}"),
            );
        }
    };
    let transport = crate::sync_cycle::load_transport_stats(&state.rua_root.join("sync"));
    let peers = self_pubkey_bytes
        .as_ref()
        .map(|self_pubkey_bytes| {
            crate::sync_cycle::load_peer_health_diagnostics(
                &state.rua_root.join("sync"),
                self_pubkey_bytes,
            )
        })
        .unwrap_or_default();
    let policy = state
        .sync_power_policy
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let runtime_state = state.sync_runtime.effective_status().await;
    let device_network = state
        .device_network
        .lock()
        .await
        .as_ref()
        .map(rua_device_network::DeviceNetworkRuntime::diagnostics);
    let background_grace = runtime_state.background_grace.clone();
    let warm_suspended = runtime_state.warm_suspended;
    let scheduled = SyncScheduledDiagnostics {
        allow_background_scheduled_sync: policy.effective_policy.allow_background_scheduled_sync,
        allow_background_runtime: policy.effective_policy.allow_background_runtime,
    };
    let repair = state.sync_repair_diagnostics();
    let blob = state
        .sync_node
        .lock()
        .await
        .as_ref()
        .map(|node| node.blob_downloader().diagnostics())
        .unwrap_or_default();
    let stages = state.sync_stage_recorder.snapshot();
    let progress = sync_progress_diagnostics(SyncProgressInput {
        sync_enabled,
        not_paired,
        identity_mismatch,
        has_remote_peers: peers.total_peers > 0,
        active_lane: repair.active_lane.map(|lane| lane.as_str()),
        pending_lane: repair.pending_lane.map(|lane| lane.as_str()),
        inbox: &inbox,
        pending_conflicts,
        transport: &transport,
    });
    (
        StatusCode::OK,
        Json(SyncDiagnosticsResponse {
            device_network,
            host_profile: state.sync_runtime_host_profile(),
            policy: policy.stored_policy,
            effective_policy: policy.effective_policy,
            runtime_state,
            background_grace,
            warm_suspended,
            scheduled,
            identity,
            inbox,
            merge,
            progress,
            transport,
            blob,
            stages,
            peers,
        }),
    )
        .into_response()
}

struct SyncProgressInput<'a> {
    sync_enabled: bool,
    not_paired: bool,
    identity_mismatch: bool,
    has_remote_peers: bool,
    active_lane: Option<&'static str>,
    pending_lane: Option<&'static str>,
    inbox: &'a rua_sync::inbound_inbox::InboxDiagnostics,
    pending_conflicts: usize,
    transport: &'a crate::sync_cycle::SyncTransportStatsState,
}

fn sync_progress_diagnostics(input: SyncProgressInput<'_>) -> SyncProgressDiagnostics {
    let counts = &input.inbox.counts;
    let pending_inbound = counts.pending as u64;
    let retryable_inbound = counts.retryable as u64;
    let dormant_inbound = counts.dormant as u64;
    let failed_inbound = counts.failed as u64;
    let last_failure_kind = input.transport.last_cycle.as_ref().and_then(|cycle| {
        if cycle.status != crate::sync_cycle::SyncTransportCycleStatus::Failure {
            return None;
        }
        cycle.failure_kind.or_else(|| {
            cycle.error.as_deref().and_then(|error| {
                error.starts_with("peer unreachable:").then_some(
                    crate::sync_peer_directory::SyncTransportFailureKind::PeerUnavailable,
                )
            })
        })
    });
    let has_attention = failed_inbound > 0
        || input.pending_conflicts > 0
        || matches!(
            last_failure_kind,
            Some(crate::sync_peer_directory::SyncTransportFailureKind::Internal)
        );
    let waiting_for_device = matches!(
        last_failure_kind,
        Some(crate::sync_peer_directory::SyncTransportFailureKind::PeerUnavailable)
    );
    let has_apply_work = pending_inbound > 0 || retryable_inbound > 0;
    let has_waiting_content = dormant_inbound > 0;
    let state = if !input.sync_enabled {
        SyncProgressState::Disabled
    } else if input.not_paired {
        SyncProgressState::NotPaired
    } else if input.identity_mismatch {
        SyncProgressState::IdentityMismatch
    } else if input.transport.active_cycle.is_some() {
        SyncProgressState::Syncing
    } else if input.active_lane.is_some() {
        SyncProgressState::ApplyingChanges
    } else if has_attention {
        SyncProgressState::NeedsAttention
    } else if has_apply_work {
        SyncProgressState::ApplyingChanges
    } else if has_waiting_content {
        SyncProgressState::WaitingForContent
    } else if !input.has_remote_peers {
        SyncProgressState::NotPaired
    } else if waiting_for_device {
        SyncProgressState::WaitingForDevice
    } else {
        SyncProgressState::UpToDate
    };
    let is_caught_up = !has_apply_work
        && !has_waiting_content
        && input.transport.active_cycle.is_none()
        && failed_inbound == 0
        && input.pending_conflicts == 0
        && input.active_lane.is_none()
        && !matches!(
            state,
            SyncProgressState::Disabled
                | SyncProgressState::NotPaired
                | SyncProgressState::IdentityMismatch
        );
    SyncProgressDiagnostics {
        state,
        is_caught_up,
        apply_active: input.active_lane.is_some(),
        active_lane: input.active_lane,
        pending_lane: input.pending_lane,
        pending_inbound,
        retryable_inbound,
        dormant_inbound,
        failed_inbound,
        pending_conflicts: input.pending_conflicts,
        last_cycle_status: input
            .transport
            .last_cycle
            .as_ref()
            .map(|cycle| cycle.status),
    }
}

async fn repair_scan_inbox(state: axum::extract::State<AppState>) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    tracing::info!("[sync-v4-repair] repair scan started");
    match state.repair_scan_and_drain_sync_inbox().await {
        Ok(report) => {
            tracing::info!(
                scanned = report.scanned,
                recorded = report.recorded,
                already_applied = report.already_applied,
                errors = report.errors.len(),
                "[sync-v4-repair] repair scan completed"
            );
            (StatusCode::OK, Json(SyncRepairScanResponse::from(report))).into_response()
        }
        Err(e) => {
            let detail = format!("repair scan failed: {e}");
            let status = if detail.contains("sync node is not initialized") {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            let reason = if status == StatusCode::UNPROCESSABLE_ENTITY {
                "not_paired"
            } else {
                "repair_scan_failed"
            };
            error_response(status, reason, detail)
        }
    }
}

async fn retry_failed_inbox(state: axum::extract::State<AppState>) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    let retried = match state.inbound_inbox.retry_failed() {
        Ok(retried) => retried,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "retry_failed_inbox_failed",
                format!("retry failed inbox entries: {error}"),
            );
        }
    };
    if retried > 0 {
        if let Err(error) = state.drain_sync_inbox_once().await {
            tracing::warn!(
                retried,
                error = %error,
                "manual retry reactivated failed inbox entries but immediate drain was incomplete"
            );
        }
    }
    (StatusCode::OK, Json(RetryFailedInboxResponse { retried })).into_response()
}

async fn repair_full_export(state: axum::extract::State<AppState>) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    tracing::info!("[sync-v4-repair] full export started");
    let exported = crate::services::sync_ops::full_export_v4(&state).await;
    tracing::info!(exported, "[sync-v4-repair] full export completed");
    (
        StatusCode::OK,
        Json(SyncRepairFullExportResponse { exported }),
    )
        .into_response()
}

#[derive(Serialize)]
struct EnableResponse {
    enabled: bool,
    identity_created: bool,
    membership: crate::sync_network::SyncNetworkMembership,
}

#[derive(Serialize)]
struct CreateNetworkResponse {
    enabled: bool,
    created: bool,
    namespace_id: String,
}

async fn enable_sync(state: axum::extract::State<AppState>) -> Response {
    let _control = state.sync_control_lock.lock().await;
    if state.sync_enabled.load(Ordering::SeqCst) {
        return (
            StatusCode::OK,
            Json(EnableResponse {
                enabled: true,
                identity_created: false,
                membership: state.sync_network_membership().await,
            }),
        )
            .into_response();
    }

    let sync_dir = state.rua_root.join("sync");
    std::fs::create_dir_all(&sync_dir).ok();
    if let Err(error) = state
        .sidecar_store
        .reopen()
        .and_then(|()| state.inbound_inbox.reopen())
    {
        let _ = state.sidecar_store.close();
        let _ = state.inbound_inbox.close();
        return ApiError::Internal(format!("open sync state: {error}")).into_response();
    }

    let ns = match rua_sync::identity::NetworkSecret::load(&sync_dir) {
        Ok(Some(ns)) => Some(ns),
        Ok(None) => None,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "load_network_secret_failed",
                format!("failed to load network secret: {e}"),
            );
        }
    };

    let device = match rua_sync::identity::DeviceIdentity::generate_or_load(&state.rua_root) {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "device_identity_failed",
                format!("failed to load device identity: {e}"),
            );
        }
    };

    let device = Arc::new(device);

    if let Some(ns) = ns {
        *state.sync_device.write().unwrap() = Some(Arc::clone(&device));
        let Some(command_handler) = state.device_command_handler() else {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "device_command_handler_unavailable",
                "local Device identity is unavailable".to_string(),
            );
        };
        let node_result = rua_sync::endpoint::SyncNode::bind_with_command_handler(
            &sync_dir,
            ns.namespace_secret(),
            &device,
            None,
            command_handler,
        )
        .await;
        match node_result {
            Ok(n) => {
                state.install_sync_node(n).await;
            }
            Err(e) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "sync_node_bind_failed",
                    format!("SyncNode bind failed: {e}"),
                );
            }
        }

        *state.sync_network_secret.write().unwrap() = Some(Arc::new(ns));

        state.sync_enabled.store(true, Ordering::SeqCst);
        if let Err(e) = state.ensure_sync_runtime_active().await {
            state.sync_enabled.store(false, Ordering::SeqCst);
            state.clear_sync_node().await;
            let _ = state.sidecar_store.close();
            let _ = state.inbound_inbox.close();
            return sync_runtime_error_response(e);
        }
    } else {
        *state.sync_device.write().unwrap() = Some(device);
        *state.sync_network_secret.write().unwrap() = None;
        state.clear_sync_node().await;
        state.sync_runtime.mark_not_paired().await;
    }

    state.sync_enabled.store(true, Ordering::SeqCst);

    if let Err(e) =
        rua_sync::control_store::SyncControlStore::from_sync_dir(&sync_dir).write_enabled(true)
    {
        tracing::error!(error = %e, "[sync] failed to write enabled flag");
    }

    (
        StatusCode::OK,
        Json(EnableResponse {
            enabled: true,
            identity_created: false,
            membership: state.sync_network_membership().await,
        }),
    )
        .into_response()
}

async fn create_sync_network(state: axum::extract::State<AppState>) -> Response {
    let _control = state.sync_control_lock.lock().await;
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }

    let sync_dir = state.rua_root.join("sync");
    std::fs::create_dir_all(&sync_dir).ok();

    let device = match rua_sync::identity::DeviceIdentity::generate_or_load(&state.rua_root) {
        Ok(d) => d,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "device_identity_failed",
                format!("failed to load device identity: {e}"),
            );
        }
    };

    let (ns, created) = match rua_sync::identity::NetworkSecret::load(&sync_dir) {
        Ok(Some(ns)) => (ns, false),
        Ok(None) => {
            let ns = rua_sync::identity::NetworkSecret::generate();
            if let Err(e) = ns.save(&sync_dir) {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "save_network_secret_failed",
                    format!("failed to save network secret: {e}"),
                );
            }
            (ns, true)
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "load_network_secret_failed",
                format!("failed to load network secret: {e}"),
            );
        }
    };

    let namespace_id = hex::encode(ns.namespace_id().as_bytes());
    let device = Arc::new(device);
    *state.sync_device.write().unwrap() = Some(Arc::clone(&device));
    let Some(command_handler) = state.device_command_handler() else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "device_command_handler_unavailable",
            "local Device identity is unavailable".to_string(),
        );
    };
    match rua_sync::endpoint::SyncNode::bind_with_command_handler(
        &sync_dir,
        ns.namespace_secret(),
        &device,
        None,
        command_handler,
    )
    .await
    {
        Ok(n) => {
            state.install_sync_node(n).await;
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "sync_node_bind_failed",
                format!("SyncNode bind failed: {e}"),
            );
        }
    }

    *state.sync_network_secret.write().unwrap() = Some(Arc::new(ns));

    if let Err(e) = state.ensure_sync_runtime_active().await {
        return sync_runtime_error_response(e);
    }

    (
        StatusCode::OK,
        Json(CreateNetworkResponse {
            enabled: true,
            created,
            namespace_id,
        }),
    )
        .into_response()
}

#[derive(Serialize)]
struct DisableResponse {
    enabled: bool,
}

async fn disable_sync(state: axum::extract::State<AppState>) -> Response {
    let _control = state.sync_control_lock.lock().await;
    let sync_dir = state.rua_root.join("sync");

    if state.sync_enabled.load(Ordering::SeqCst) {
        let device_id = state
            .sync_device
            .read()
            .unwrap()
            .as_ref()
            .map(|d| d.device_id_short());

        if let Some(device_id) = device_id {
            let device_id_hex = hex::encode(device_id);
            let _ = crate::sync_publication::publish_sync_fact(
                &state,
                crate::sync_publication::SyncPublicationFact::DeviceListDeviceRemoved {
                    device_id_hex,
                },
            )
            .await;
        }
    }

    // Block new work before draining the runtime and releasing persistent stores.
    state.sync_enabled.store(false, Ordering::SeqCst);
    state.sync_runtime.shutdown_unavailable().await;
    state.clear_sync_node().await;
    let _apply_guard = state.inbound_apply_lock.lock().await;
    let sidecar_closed = state.sidecar_store.close();
    let inbox_closed = state.inbound_inbox.close();
    if let Err(error) = sidecar_closed.and(inbox_closed) {
        return ApiError::Internal(format!("close sync state: {error}")).into_response();
    }

    if let Err(e) = rua_sync::identity::NetworkSecret::delete(&sync_dir) {
        tracing::warn!(error = %e, "[sync] failed to delete network secret");
    }

    let iroh_docs_dir = sync_dir.join("iroh-docs");
    if iroh_docs_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&iroh_docs_dir) {
            tracing::warn!(error = %e, "[sync] failed to remove iroh-docs directory");
        }
    }

    *state.sync_network_secret.write().unwrap() = None;

    {
        let mut trigger_guard = state.sync_trigger.write().await;
        *trigger_guard = None;
    }

    let control_store = rua_sync::control_store::SyncControlStore::from_sync_dir(&sync_dir);
    let _ = control_store.remove_device_list();
    let _ = std::fs::remove_file(sync_dir.join("revocation_state.json"));
    let pending_ops_dir = sync_dir.join("pending_ops");
    if pending_ops_dir.exists() {
        let _ = std::fs::remove_dir_all(&pending_ops_dir);
    }

    if let Err(e) = control_store.write_enabled(false) {
        return ApiError::Internal(format!("persist disabled sync state: {e}")).into_response();
    }

    (StatusCode::OK, Json(DisableResponse { enabled: false })).into_response()
}

#[derive(Serialize)]
struct ConflictPolicyResponse {
    policy: rua_sync::sidecar::ConflictPolicy,
}

async fn get_conflict_policy(state: axum::extract::State<AppState>) -> Response {
    let policy = match state.sidecar_store.conflict_policy() {
        Ok(p) => p,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "store_error",
                format!("failed to read conflict policy: {e}"),
            );
        }
    };
    (StatusCode::OK, Json(ConflictPolicyResponse { policy })).into_response()
}

#[derive(Deserialize)]
struct SetConflictPolicyRequest {
    policy: rua_sync::sidecar::ConflictPolicy,
}

async fn set_conflict_policy(
    state: axum::extract::State<AppState>,
    Json(body): Json<SetConflictPolicyRequest>,
) -> Response {
    if let Err(e) = state.sidecar_store.set_conflict_policy(body.policy) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "store_error",
            format!("failed to set conflict policy: {e}"),
        );
    }
    (
        StatusCode::OK,
        Json(ConflictPolicyResponse {
            policy: body.policy,
        }),
    )
        .into_response()
}

#[derive(Serialize)]
struct PendingConflictsResponse {
    conflicts: Vec<rua_sync::sidecar::ConflictEntry>,
    count: usize,
}

async fn get_pending_conflicts(state: axum::extract::State<AppState>) -> Response {
    let conflicts = match state.sidecar_store.list_pending_conflicts() {
        Ok(c) => c,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "store_error",
                format!("failed to list pending conflicts: {e}"),
            );
        }
    };
    let count = conflicts.len();
    (
        StatusCode::OK,
        Json(PendingConflictsResponse { conflicts, count }),
    )
        .into_response()
}

#[derive(Deserialize)]
struct ResolveConflictRequest {
    id: String,
    chosen: String,
}

#[derive(Serialize)]
struct ResolveConflictResponse {
    resolved: bool,
}

async fn resolve_conflict(
    state: axum::extract::State<AppState>,
    Json(body): Json<ResolveConflictRequest>,
) -> Response {
    if let Err(e) = state.sidecar_store.resolve_conflict(&body.id, &body.chosen) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "store_error",
            format!("failed to resolve conflict: {e}"),
        );
    }
    (
        StatusCode::OK,
        Json(ResolveConflictResponse { resolved: true }),
    )
        .into_response()
}
