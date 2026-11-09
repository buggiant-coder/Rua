use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::endpoints::sync::require_sync_enabled;
use crate::errors::ApiError;
use crate::state::AppState;
use crate::streaming::{sse_json_event, sse_response};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sync/devices", get(list_devices))
        .route("/sync/devices/activity", get(device_activity))
        .route("/sync/devices/remove", post(remove_device))
        .route("/sync/devices/rename", post(rename_device))
        .route("/sync/devices/status", get(device_status))
}

#[derive(Serialize)]
struct DeviceDto {
    device_id: String,
    public_key: String,
    friendly_name: String,
    label: String,
    added_at_ms: u64,
    last_seen_ms: u64,
    is_self: bool,
    connectivity_state: rua_device_network::DeviceConnectivityState,
    session_path: Option<String>,
    session_rtt_ms: Option<u64>,
}

#[derive(Serialize)]
struct DeviceListResponse {
    devices: Vec<DeviceDto>,
}

#[derive(Deserialize)]
struct RemoveDeviceRequest {
    device_id: String,
}

#[derive(Deserialize)]
struct RenameDeviceRequest {
    device_id: String,
    friendly_name: String,
}

#[derive(Serialize)]
struct DeviceStatusResponse {
    revocation_state: String,
    is_revoked: bool,
}

async fn list_devices(state: axum::extract::State<AppState>) -> Response {
    let Some(local_device) = local_device_dto(&state) else {
        return ApiError::Internal("local Device identity is unavailable".into()).into_response();
    };
    if !state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst) {
        return (
            StatusCode::OK,
            Json(DeviceListResponse {
                devices: vec![local_device],
            }),
        )
            .into_response();
    }
    let sync_dir = state.rua_root.join("sync");
    let device_list_path = sync_dir.join("device_list.json");

    if !device_list_path.exists() {
        return (
            StatusCode::OK,
            Json(DeviceListResponse {
                devices: vec![local_device],
            }),
        )
            .into_response();
    }

    let content = match std::fs::read_to_string(&device_list_path) {
        Ok(c) => c,
        Err(e) => return ApiError::Internal(format!("read device list: {e}")).into_response(),
    };

    let device_list: rua_sync::device_list::DeviceList = match serde_json::from_str(&content) {
        Ok(dl) => dl,
        Err(e) => return ApiError::Internal(format!("parse device list: {e}")).into_response(),
    };

    let local_device_id = local_device.device_id.clone();
    let device_network = state.device_network.lock().await.as_ref().cloned();
    let connectivity = device_network
        .as_ref()
        .map(|runtime| {
            runtime
                .session_manager()
                .diagnostics()
                .sessions
                .into_iter()
                .map(|peer| (peer.peer_device_id.clone(), peer))
                .collect::<std::collections::HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut devices: Vec<DeviceDto> = device_list
        .devices
        .iter()
        .map(|d| {
            let device_id = hex::encode(d.device_id);
            let is_self = local_device_id == device_id;
            let session = connectivity.get(&device_id);
            let connectivity_state = if is_self {
                rua_device_network::DeviceConnectivityState::Connected
            } else {
                session
                    .map(|session| session.state)
                    .unwrap_or(rua_device_network::DeviceConnectivityState::NotConnected)
            };
            let session_path = session
                .filter(|_| {
                    connectivity_state == rua_device_network::DeviceConnectivityState::Connected
                })
                .and_then(|session| match session.path_class.as_str() {
                    "unknown" => None,
                    path => Some(path.to_string()),
                });
            let session_rtt_ms = session_path
                .as_ref()
                .and_then(|_| session.and_then(|session| session.rtt_ms));
            DeviceDto {
                device_id: device_id.clone(),
                public_key: hex::encode(d.public_key),
                friendly_name: d.friendly_name.clone(),
                label: d.label.clone(),
                added_at_ms: d.added_at_ms,
                last_seen_ms: d.last_seen_ms,
                is_self,
                connectivity_state,
                session_path,
                session_rtt_ms,
            }
        })
        .collect();
    if !devices.iter().any(|device| device.is_self) {
        devices.insert(0, local_device);
    }

    (StatusCode::OK, Json(DeviceListResponse { devices })).into_response()
}

fn local_device_dto(state: &AppState) -> Option<DeviceDto> {
    let guard = state.sync_device.read().ok()?;
    let device = guard.as_ref()?;
    let public_key = device.public_key().to_bytes();
    Some(DeviceDto {
        device_id: hex::encode(device.device_id_short()),
        public_key: hex::encode(public_key),
        friendly_name: rua_sync::device_list::default_friendly_name(
            std::env::consts::OS,
            &public_key,
        ),
        label: std::env::consts::OS.to_string(),
        added_at_ms: 0,
        last_seen_ms: 0,
        is_self: true,
        connectivity_state: rua_device_network::DeviceConnectivityState::Connected,
        session_path: None,
        session_rtt_ms: None,
    })
}

async fn device_activity(State(state): State<AppState>) -> Response {
    let Some(runtime) = state.device_network.lock().await.as_ref().cloned() else {
        return ApiError::Unavailable("Device Network Runtime is unavailable".to_string())
            .into_response();
    };
    let receiver = runtime.session_manager().subscribe_connectivity_changes();
    let stream = futures::stream::unfold(receiver, |mut receiver| async move {
        receiver.changed().await.ok()?;
        let revision = *receiver.borrow_and_update();
        Some((
            sse_json_event(
                "device_connectivity",
                &serde_json::json!({ "revision": revision }),
            ),
            receiver,
        ))
    });
    sse_response(stream).into_response()
}

async fn remove_device(
    state: axum::extract::State<AppState>,
    Json(body): Json<RemoveDeviceRequest>,
) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    let device_id_bytes = match parse_device_id(&body.device_id) {
        Ok(b) => b,
        Err(e) => return ApiError::BadRequest(e).into_response(),
    };

    let sync_dir = state.rua_root.join("sync");
    let store = rua_sync::control_store::SyncControlStore::from_sync_dir(&sync_dir);
    let mut device_list = match store.read_device_list() {
        Ok(device_list) => device_list,
        Err(error) => {
            return ApiError::Internal(format!("read device list: {error}")).into_response();
        }
    };

    if is_self_device(&state, &device_id_bytes) {
        return ApiError::BadRequest("cannot remove the local sync device".into()).into_response();
    }

    if !device_list.contains_device(&device_id_bytes) {
        let resp = serde_json::json!({
            "removed": false,
            "device_id": body.device_id,
            "mode": "device_list",
        });
        return (StatusCode::OK, Json(resp)).into_response();
    }

    let local_device_id = match state.sync_device.read().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|device| hex::encode(device.device_id_short()))
    }) {
        Some(device_id) => device_id,
        None => {
            return ApiError::Conflict("local sync device is not initialized".into())
                .into_response();
        }
    };

    device_list.remove_device(&device_id_bytes);
    if !device_list
        .devices
        .iter()
        .any(|entry| hex::encode(entry.device_id) == local_device_id)
    {
        return ApiError::Conflict("device list does not contain the local sync device".into())
            .into_response();
    }

    if let Err(error) = store.write_device_list(&device_list) {
        return ApiError::Internal(format!("write device list: {error}")).into_response();
    }
    let device_network = state.device_network.lock().await.as_ref().cloned();
    if let Some(runtime) = device_network {
        if let Err(error) = runtime
            .session_manager()
            .disconnect_peer(&body.device_id)
            .await
        {
            tracing::warn!(
                device_id = %body.device_id,
                error = %error,
                "removed Device but failed to close its active Device Session"
            );
        }
    }
    let publication = crate::sync_publication::publish_sync_fact(
        &state,
        crate::sync_publication::SyncPublicationFact::DeviceListDeviceRemoved {
            device_id_hex: body.device_id.clone(),
        },
    )
    .await;

    let resp = serde_json::json!({
        "removed": true,
        "device_id": body.device_id,
        "mode": "device_list",
        "publication_status": format!("{:?}", publication.status),
        "appended": publication.appended,
    });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn rename_device(
    state: axum::extract::State<AppState>,
    Json(body): Json<RenameDeviceRequest>,
) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    if body.friendly_name.is_empty() {
        return ApiError::BadRequest("friendly_name must not be empty".into()).into_response();
    }

    let device_id_bytes = match parse_device_id(&body.device_id) {
        Ok(b) => b,
        Err(e) => return ApiError::BadRequest(e).into_response(),
    };

    let sync_dir = state.rua_root.join("sync");

    let device_list_path = sync_dir.join("device_list.json");
    let mut renamed = false;
    if device_list_path.exists() {
        if let Some(mut dl) = std::fs::read_to_string(&device_list_path)
            .ok()
            .and_then(|content| {
                serde_json::from_str::<rua_sync::device_list::DeviceList>(&content).ok()
            })
        {
            renamed = dl.contains_device(&device_id_bytes);
            dl.rename_device(&device_id_bytes, &body.friendly_name);
            if let Err(e) = write_device_list(&device_list_path, &dl) {
                return ApiError::Internal(format!("write device list: {e}")).into_response();
            }
        }
    }

    if renamed {
        let _ = crate::sync_publication::publish_sync_fact(
            &state,
            crate::sync_publication::SyncPublicationFact::DeviceListDeviceRenamed {
                device_id_hex: body.device_id.clone(),
                friendly_name: body.friendly_name.clone(),
            },
        )
        .await;
    }

    let resp = serde_json::json!({
        "renamed": true,
        "device_id": body.device_id,
        "friendly_name": body.friendly_name
    });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn device_status(state: axum::extract::State<AppState>) -> Response {
    if let Some(r) = require_sync_enabled(&state) {
        return r;
    }
    let sync_dir = state.rua_root.join("sync");
    let status_path = sync_dir.join("revocation_state.json");

    if !status_path.exists() {
        return (
            StatusCode::OK,
            Json(DeviceStatusResponse {
                revocation_state: "active".into(),
                is_revoked: false,
            }),
        )
            .into_response();
    }

    let content = match std::fs::read_to_string(&status_path) {
        Ok(c) => c,
        Err(e) => return ApiError::Internal(format!("read revocation state: {e}")).into_response(),
    };

    let detector: rua_sync::revocation::RevocationDetector = match serde_json::from_str(&content) {
        Ok(d) => d,
        Err(e) => {
            return ApiError::Internal(format!("parse revocation state: {e}")).into_response();
        }
    };

    let state_str = match detector.state() {
        rua_sync::revocation::RevocationState::Active => "active",
        rua_sync::revocation::RevocationState::Suspected { .. } => "suspected",
        rua_sync::revocation::RevocationState::Revoked => "revoked",
    };

    (
        StatusCode::OK,
        Json(DeviceStatusResponse {
            revocation_state: state_str.into(),
            is_revoked: detector.is_revoked(),
        }),
    )
        .into_response()
}

fn parse_device_id(hex_str: &str) -> Result<[u8; 8], String> {
    let bytes = hex::decode(hex_str).map_err(|e| format!("invalid device_id hex: {e}"))?;
    if bytes.len() != 8 {
        return Err(format!(
            "device_id must be 8 bytes (16 hex chars), got {}",
            bytes.len()
        ));
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

fn is_self_device(state: &AppState, device_id: &[u8; 8]) -> bool {
    state
        .sync_device
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(|device| device.device_id_short()))
        .as_ref()
        == Some(device_id)
}

fn write_device_list(
    path: &std::path::Path,
    device_list: &rua_sync::device_list::DeviceList,
) -> Result<(), String> {
    rua_sync::control_store::write_device_list_file(path, device_list)
        .map_err(|e| format!("write device list: {e}"))
}
