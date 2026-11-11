use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::endpoints::sync::require_sync_enabled;
use crate::errors::ApiError;
use crate::middleware::AgentExtractorContext;
use crate::state::AppState;
use rua_sync::pairing::PairingPin;
use rua_sync::pairing_orchestrator::{self, PairingOutcome};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sync/pairing/initiate", post(initiate_pairing))
        .route("/sync/pairing/status", get(pairing_status))
        .route("/sync/pairing/cancel", post(cancel_pairing))
        .route("/sync/pairing/join", post(join_pairing))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingStatusKind {
    Active,
    Expired,
    Consumed,
    Paired,
    Failed,
    None,
}

#[derive(Debug, Clone)]
pub enum PairingResult {
    Paired,
    Failed(String),
}

#[derive(Default)]
pub struct PairingState {
    pub pin: Option<PairingPin>,
    pub initiator_running: bool,
    pub result: Option<PairingResult>,
}

struct PairingTransaction<'a> {
    state: &'a AppState,
    sync_dir: &'a std::path::Path,
}

impl<'a> PairingTransaction<'a> {
    fn new(state: &'a AppState, sync_dir: &'a std::path::Path) -> Self {
        Self { state, sync_dir }
    }

    async fn commit(&self, outcome: PairingOutcome) -> Result<(), String> {
        handle_pairing_outcome(self.state, outcome, self.sync_dir).await
    }
}

impl PairingState {
    pub fn status(&self) -> PairingStatusKind {
        match &self.pin {
            None => PairingStatusKind::None,
            Some(pin) => match &self.result {
                Some(PairingResult::Paired) => PairingStatusKind::Paired,
                Some(PairingResult::Failed(_)) => PairingStatusKind::Failed,
                None => {
                    if pin.is_consumed() {
                        PairingStatusKind::Consumed
                    } else if pin.is_expired() {
                        PairingStatusKind::Expired
                    } else {
                        PairingStatusKind::Active
                    }
                }
            },
        }
    }
}

#[derive(Serialize)]
struct InitiateResponse {
    pin: String,
    expires_in_secs: u64,
}

#[derive(Serialize)]
struct StatusResponse {
    status: PairingStatusKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remaining_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Deserialize)]
struct JoinRequest {
    pin: String,
}

#[derive(Serialize)]
struct JoinResponse {
    device_public_key: String,
}

#[derive(Serialize)]
struct CancelResponse {
    cancelled: bool,
}

async fn initiate_pairing(
    state: axum::extract::State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    let pairing_state = state.pairing_state();

    let mut guard = pairing_state.lock().await;
    if guard.initiator_running {
        return ApiError::Conflict("pairing already in progress".into()).into_response();
    }

    let pin = PairingPin::generate();
    let code = pin.code().to_string();
    let expires = pin.remaining_secs();
    guard.pin = Some(pin);
    guard.initiator_running = true;
    guard.result = None;
    drop(guard);

    let ns_opt = state.sync_network_secret.read().unwrap().clone();

    let device_opt = state.sync_device.read().unwrap().clone();
    let sync_dir = state.rua_root.join("sync");

    let device = match &device_opt {
        Some(d) => d.clone(),
        None => match rua_sync::identity::DeviceIdentity::generate_or_load(&state.rua_root) {
            Ok(d) => Arc::new(d),
            Err(e) => {
                let mut g = pairing_state.lock().await;
                g.pin = None;
                g.initiator_running = false;
                return ApiError::Internal(format!("device identity: {e}")).into_response();
            }
        },
    };

    let device_list = {
        let dl_path = sync_dir.join("device_list.json");
        std::fs::read_to_string(&dl_path)
            .ok()
            .and_then(|json| serde_json::from_str::<rua_sync::device_list::DeviceList>(&json).ok())
            .unwrap_or_default()
    };

    let created_at_ms = pairing_orchestrator::load_network_created_at(&sync_dir);

    let ps = Arc::clone(&pairing_state);
    let state_clone = state.0.clone();
    tokio::spawn(async move {
        let pin_code = {
            let guard = ps.lock().await;
            match &guard.pin {
                Some(p) => p.code().to_string(),
                None => return,
            }
        };

        let result = tokio::time::timeout(
            Duration::from_secs(5 * 60),
            pairing_orchestrator::run_pairing_listener(
                &pin_code,
                ns_opt.as_deref(),
                &device,
                &device_list,
                &sync_dir,
                created_at_ms,
            ),
        )
        .await;

        match result {
            Ok(Ok(outcome)) => {
                tracing::info!("[pairing] listener completed successfully");
                {
                    let mut guard = ps.lock().await;
                    if let Some(pin) = guard.pin.as_mut() {
                        let _ = pin.consume();
                    }
                }
                let commit_result = PairingTransaction::new(&state_clone, &sync_dir)
                    .commit(outcome)
                    .await;
                let mut guard = ps.lock().await;
                match commit_result {
                    Ok(()) => {
                        guard.result = Some(PairingResult::Paired);
                    }
                    Err(error) => {
                        guard.result = Some(PairingResult::Failed(error));
                    }
                }
                guard.initiator_running = false;
            }
            Ok(Err(e)) => {
                let mut guard = ps.lock().await;
                guard.result = Some(PairingResult::Failed(e.to_string()));
                guard.initiator_running = false;
                tracing::warn!("[pairing] listener failed: {e}");
            }
            Err(_) => {
                let mut guard = ps.lock().await;
                guard.result = Some(PairingResult::Failed("pairing timed out".into()));
                guard.initiator_running = false;
                tracing::warn!("[pairing] listener timed out after 5 minutes");
            }
        }
    });

    (
        StatusCode::OK,
        Json(InitiateResponse {
            pin: code,
            expires_in_secs: expires,
        }),
    )
        .into_response()
}

async fn pairing_status(
    state: axum::extract::State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    let pairing_state = state.pairing_state();
    let guard = pairing_state.lock().await;

    let status = guard.status();
    let (pin, remaining) = match &guard.pin {
        Some(p) if status == PairingStatusKind::Active => {
            (Some(p.code().to_string()), Some(p.remaining_secs()))
        }
        _ => (None, None),
    };
    let error = match &guard.result {
        Some(PairingResult::Failed(msg)) => Some(msg.clone()),
        _ => None,
    };

    (
        StatusCode::OK,
        Json(StatusResponse {
            status,
            pin,
            remaining_secs: remaining,
            error,
        }),
    )
        .into_response()
}

async fn cancel_pairing(
    state: axum::extract::State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    let pairing_state = state.pairing_state();
    let mut guard = pairing_state.lock().await;

    let cancelled = guard.pin.is_some();
    guard.pin = None;
    guard.initiator_running = false;
    guard.result = None;

    (StatusCode::OK, Json(CancelResponse { cancelled })).into_response()
}

async fn join_pairing(
    state: axum::extract::State<AppState>,
    Extension(_ctx): Extension<AgentExtractorContext>,
    Json(body): Json<JoinRequest>,
) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    if body.pin.len() != 8 || !body.pin.chars().all(|c| c.is_ascii_digit()) {
        return ApiError::BadRequest("pin must be exactly 8 digits".into()).into_response();
    }

    let sync_dir = state.rua_root.join("sync");
    let ns_opt = state.sync_network_secret.read().unwrap().clone();

    let device = match state.sync_device.read().unwrap().clone() {
        Some(d) => d,
        None => match rua_sync::identity::DeviceIdentity::generate_or_load(&state.rua_root) {
            Ok(d) => Arc::new(d),
            Err(e) => {
                return ApiError::Internal(format!("device identity: {e}")).into_response();
            }
        },
    };

    let device_list = {
        let dl_path = sync_dir.join("device_list.json");
        std::fs::read_to_string(&dl_path)
            .ok()
            .and_then(|json| serde_json::from_str::<rua_sync::device_list::DeviceList>(&json).ok())
            .unwrap_or_default()
    };

    let created_at_ms = pairing_orchestrator::load_network_created_at(&sync_dir);

    match pairing_orchestrator::run_pairing_connector(
        &body.pin,
        ns_opt.as_deref(),
        &device,
        &device_list,
        &sync_dir,
        created_at_ms,
    )
    .await
    {
        Ok(outcome) => {
            let pubkey_hex = hex::encode(device.public_key().as_bytes());

            if let Err(error) = PairingTransaction::new(&state, &sync_dir)
                .commit(outcome)
                .await
            {
                return ApiError::Internal(error).into_response();
            }

            (
                StatusCode::OK,
                Json(JoinResponse {
                    device_public_key: pubkey_hex,
                }),
            )
                .into_response()
        }
        Err(e) => {
            let msg = e.to_string();
            tracing::warn!("[pairing] connector failed: {msg}");
            let reason = if msg.contains("authentication failed") || msg.contains("SPAKE2") {
                "wrong-pin"
            } else if msg.contains("expired") {
                "expired"
            } else {
                "network"
            };
            let body = serde_json::json!({
                "detail": msg,
                "reason": reason,
            });
            (StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response()
        }
    }
}

async fn handle_pairing_outcome(
    state: &AppState,
    mut outcome: PairingOutcome,
    sync_dir: &std::path::Path,
) -> Result<(), String> {
    {
        let _apply_guard = state.inbound_apply_lock.lock().await;
        let store = rua_sync::control_store::SyncControlStore::from_sync_dir(sync_dir);
        let current = store.read_device_list().map_err(|error| {
            tracing::warn!(%error, "[pairing] failed to read current device_list.json");
            format!("read current device_list.json: {error}")
        })?;
        let device_list = match &mut outcome {
            PairingOutcome::Joined { device_list, .. }
            | PairingOutcome::Shared { device_list }
            | PairingOutcome::PeerAdded { device_list }
            | PairingOutcome::MergeNeeded { device_list, .. } => device_list,
        };

        // The handshake used a captured list; names may have changed while it ran.
        device_list.merge_name_facts_from(&current);
        if let Err(e) = store.write_device_list(device_list) {
            tracing::warn!("[pairing] failed to persist device_list.json: {e}");
            return Err(format!("persist device_list.json: {e}"));
        }
    }

    match outcome {
        PairingOutcome::Joined { .. } => {
            reinit_sync_after_pairing(state, None, "reinit_after_pairing").await?;
        }
        PairingOutcome::MergeNeeded {
            peer_namespace_secret,
            ..
        } => {
            run_network_merge(state, sync_dir, *peer_namespace_secret).await?;
        }
        PairingOutcome::Shared { .. } | PairingOutcome::PeerAdded { .. } => {
            reinit_sync_after_pairing(state, None, "reinit_after_pairing").await?;
        }
    }
    Ok(())
}

async fn reinit_sync_after_pairing(
    state: &AppState,
    frozen_export: Option<crate::services::sync_ops::FullExportSnapshot>,
    reason: &'static str,
) -> Result<(), String> {
    let sync_dir = state.rua_root.join("sync");

    let new_ns = match rua_sync::identity::NetworkSecret::load(&sync_dir) {
        Ok(Some(ns)) => Arc::new(ns),
        Ok(None) => {
            tracing::error!("[pairing] network_secret.key not found after pairing");
            return Err("network_secret.key not found after pairing".to_string());
        }
        Err(e) => {
            tracing::error!(error = %e, "[pairing] failed to load network secret");
            return Err(format!("load network secret after pairing: {e}"));
        }
    };

    let new_device = match rua_sync::identity::DeviceIdentity::generate_or_load(&state.rua_root) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            tracing::error!(error = %e, "[pairing] failed to load device identity");
            return Err(format!("load device identity after pairing: {e}"));
        }
    };

    state.clear_sync_node().await;
    let freeze_guard = state.inbound_apply_lock.lock().await;

    *state.sync_device.write().unwrap() = Some(Arc::clone(&new_device));
    let command_handler = state
        .device_command_handler()
        .ok_or_else(|| "local Device identity is unavailable".to_string())?;
    match rua_sync::endpoint::SyncNode::bind_with_command_handler(
        &sync_dir,
        new_ns.namespace_secret(),
        &new_device,
        None,
        command_handler,
    )
    .await
    {
        Ok(n) => {
            {
                let dl_path = sync_dir.join("device_list.json");
                if let Some(device_list) = std::fs::read_to_string(&dl_path).ok().and_then(|json| {
                    serde_json::from_str::<rua_sync::device_list::DeviceList>(&json).ok()
                }) {
                    let my_pubkey = *new_device.public_key().as_bytes();
                    for entry in &device_list.devices {
                        if entry.public_key == my_pubkey {
                            continue;
                        }
                        if let Ok(peer_pk) = iroh::PublicKey::from_bytes(&entry.public_key) {
                            let peer_addr = match n
                                .network_runtime()
                                .resolve_peer_addr(peer_pk)
                                .await
                            {
                                Ok(Some(peer_addr)) => peer_addr,
                                Ok(None) => iroh::EndpointAddr::new(peer_pk),
                                Err(error) => {
                                    tracing::warn!(
                                        peer = %peer_pk.fmt_short(),
                                        error = %error,
                                        "[pairing] peer address resolver failed; using Address Lookup"
                                    );
                                    iroh::EndpointAddr::new(peer_pk)
                                }
                            };
                            if let Err(e) = n.register_peer(peer_addr).await {
                                tracing::warn!(error = %e, "[pairing] failed to register peer");
                            } else {
                                tracing::info!(peer = %peer_pk.fmt_short(), "[pairing] registered peer");
                            }
                        }
                    }
                }
            }

            state.install_sync_node(n).await;
        }
        Err(e) => {
            tracing::warn!(error = %e, "[pairing] SyncNode rebind failed");
            return Err(format!("sync node rebind after pairing: {e}"));
        }
    }

    *state.sync_network_secret.write().unwrap() = Some(new_ns);
    *state.sync_device.write().unwrap() = Some(new_device);

    let frozen_export = match frozen_export {
        Some(snapshot) => snapshot,
        None => crate::services::sync_ops::capture_full_export_v4_bootstrap(state)
            .await
            .ok_or_else(|| "capture pre-pair Full Export state".to_string())?,
    };
    drop(freeze_guard);

    state.sync_runtime.shutdown_unavailable().await;
    if let Err(e) = state.ensure_sync_runtime_active().await {
        tracing::warn!(
            reason = e.reason(),
            detail = e.detail(),
            "[pairing] sync runtime restart failed after pairing"
        );
        return Err(format!(
            "sync runtime restart after pairing: {}",
            e.detail()
        ));
    }

    tracing::info!("[pairing] sync subsystem reinitialized after pairing");

    schedule_post_pairing_full_export(state, reason, frozen_export);

    Ok(())
}

async fn run_network_merge(
    state: &AppState,
    sync_dir: &std::path::Path,
    peer_ns: rua_sync::identity::NetworkSecret,
) -> Result<(), String> {
    let my_ns = match state.sync_network_secret.read().unwrap().clone() {
        Some(ns) => ns,
        None => {
            tracing::error!("[merge] own network_secret unavailable");
            return Err("own network_secret unavailable".to_string());
        }
    };

    let my_created = pairing_orchestrator::load_network_created_at(sync_dir);

    let my_ns_id = my_ns.namespace_id();
    let peer_ns_id = peer_ns.namespace_id();

    let i_am_winner = match (my_created, None::<u64>) {
        (Some(my_ts), Some(peer_ts)) => {
            if my_ts != peer_ts {
                my_ts < peer_ts
            } else {
                my_ns_id.as_bytes() < peer_ns_id.as_bytes()
            }
        }
        _ => my_ns_id.as_bytes() < peer_ns_id.as_bytes(),
    };

    let device = match state.sync_device.read().unwrap().clone() {
        Some(d) => d,
        None => {
            tracing::error!("[merge] sync_device not available");
            return Err("sync_device not available".to_string());
        }
    };

    let (winner_ns, loser_ns) = if i_am_winner {
        let w = rua_sync::identity::NetworkSecret::from_bytes(&my_ns.to_bytes());
        (w, peer_ns)
    } else {
        let l = rua_sync::identity::NetworkSecret::from_bytes(&my_ns.to_bytes());
        (peer_ns, l)
    };

    tracing::info!(
        i_am_winner,
        winner_ns_id = %hex::encode(winner_ns.namespace_id().as_bytes()),
        loser_ns_id = %hex::encode(loser_ns.namespace_id().as_bytes()),
        "[merge] arbitration decided"
    );

    state.sync_runtime.shutdown_unavailable().await;
    let freeze_guard = state.inbound_apply_lock.lock().await;
    let node = match state.sync_node.lock().await.as_ref().cloned() {
        Some(node) => node,
        None => {
            tracing::error!("[merge] SyncNode not available");
            return Err("SyncNode not available".to_string());
        }
    };
    let frozen_export = crate::services::sync_ops::capture_full_export_v4_bootstrap_for_network(
        state,
        node.blobs_store().clone(),
        &winner_ns,
    )
    .await
    .ok_or_else(|| "capture pre-merge Full Export state".to_string())?;
    drop(freeze_guard);

    {
        if i_am_winner {
            let loser_log = match node.open_opaque_log_for_namespace(&loser_ns, &device).await {
                Ok(log) => log,
                Err(e) => {
                    tracing::error!(error = %e, "[merge] failed to open loser v4 log");
                    return Err(format!("open loser namespace for migration marker: {e}"));
                }
            };
            write_migration_marker(&loser_log, &winner_ns, &device).await;
            state
                .ensure_sync_runtime_active()
                .await
                .map_err(|error| format!("restart winner sync runtime: {}", error.detail()))?;
            schedule_post_pairing_full_export(state, "network_merge_winner", frozen_export);
            return Ok(());
        }

        let winner_log = match node
            .open_opaque_log_for_namespace(&winner_ns, &device)
            .await
        {
            Ok(log) => log,
            Err(e) => {
                tracing::error!(error = %e, "[merge] failed to open winner v4 log");
                return Err(format!("open winner namespace before migration: {e}"));
            }
        };
        drop(winner_log);

        {
            let loser_log = match node.open_opaque_log_for_namespace(&loser_ns, &device).await {
                Ok(log) => log,
                Err(e) => {
                    tracing::error!(error = %e, "[merge] failed to open loser v4 log for marker");
                    return Err(format!("open loser namespace for migration marker: {e}"));
                }
            };
            write_migration_marker(&loser_log, &winner_ns, &device).await;
        }
    }

    if let Err(e) = winner_ns.save(sync_dir) {
        tracing::error!(error = %e, "[merge] failed to save winner network secret");
        return Err(format!("save winner network secret: {e}"));
    }

    tracing::info!("[merge] switching to winner namespace, reinitializing sync");
    reinit_sync_after_pairing(state, Some(frozen_export), "network_merge_loser").await
}

async fn write_migration_marker(
    log: &rua_sync::opaque_replica_log::OpaqueReplicaLog,
    winner_ns: &rua_sync::identity::NetworkSecret,
    device: &rua_sync::identity::DeviceIdentity,
) {
    let device_id_bytes = device.device_id_short();
    let mut clock = rua_sync::hlc::HlcClock::new(device_id_bytes);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let payload = rua_sync::network_migration_adapter::NetworkMigrationPayload {
        target_namespace_id: hex::encode(winner_ns.namespace_id().as_bytes()),
        target_namespace_secret_bytes: winner_ns.to_bytes().to_vec(),
        migrated_at_ms: now_ms,
    };
    let change = rua_sync::network_migration_adapter::NetworkMigrationSyncAdapter::marker_change(
        rua_sync::network_migration_adapter::NetworkMigrationSyncAdapter::new_op_id(),
        hex::encode(device_id_bytes),
        clock.now(),
        payload,
    );

    if let Err(e) = log.append_change(&change).await {
        tracing::error!(error = %e, "[merge] failed to write v4 migration marker");
    } else {
        tracing::info!("[merge] v4 migration marker written to loser namespace");
    }
}

async fn run_bootstrap_export(
    state: &AppState,
    snapshot: crate::services::sync_ops::FullExportSnapshot,
) -> bool {
    use crate::services::sync_ops;

    sync_ops::publish_full_export_snapshot(state, snapshot).await
}

fn schedule_post_pairing_full_export(
    state: &AppState,
    reason: &'static str,
    snapshot: crate::services::sync_ops::FullExportSnapshot,
) {
    let state = state.clone();
    tokio::spawn(async move {
        tracing::info!(
            reason,
            "[pairing] scheduling background v4 bootstrap export"
        );
        let exported = run_bootstrap_export(&state, snapshot).await;
        tracing::info!(
            reason,
            exported,
            "[pairing] background v4 bootstrap export completed"
        );
        if exported {
            match crate::sync_cycle::run_first_sync_cycle_to_all_device_list_peers(
                &state.sync_node,
                &state.rua_root.join("sync"),
            )
            .await
            {
                Ok(result) => tracing::info!(
                    reason,
                    attempted = result.attempted,
                    succeeded = result.succeeded,
                    failed = result.failed,
                    events_sent = result.events_sent,
                    events_received = result.events_received,
                    already_syncing_retries = result.already_syncing_retries,
                    "[pairing] First Sync Cycle completed"
                ),
                Err(error) => tracing::warn!(
                    reason,
                    error = %error,
                    "[pairing] First Sync Cycle failed"
                ),
            }
        }
    });
}
