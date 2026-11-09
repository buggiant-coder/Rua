//! Current execution-device browser settings and protocol diagnostics.
use std::collections::BTreeMap;
use std::path::PathBuf;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use mcp_manager::client::McpClient;
use rua_config::desktop_automation::{preferences_path, DesktopAutomationPreferences};
use rua_core::runtime_authority::{RuntimeAuthority, RuntimeCapability};
use rua_mcp::bundled_browser::{BundledBrowser, BundledBrowserClient, DRIVER_VERSION};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::errors::ApiError;
use crate::state::AppState;

// One local preference file and probe at a time. Browser profiles remain Chat-owned.
static PREFERENCE_UPDATE: Mutex<()> = Mutex::const_new(());

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/platform/browser", get(status).put(select_browser))
        .route("/platform/browser/recheck", post(recheck))
        .route("/platform/computer", get(computer_status))
        .route("/platform/computer/recheck", post(recheck_computer))
}

#[derive(Serialize)]
pub struct BrowserStatus {
    pub device_id: Option<String>,
    pub available: bool,
    pub state: String,
    pub selected_path: Option<PathBuf>,
    pub resolved_path: Option<PathBuf>,
    pub driver_version: &'static str,
    pub diagnostics: Vec<String>,
}

#[derive(Deserialize)]
pub struct BrowserSelection {
    pub path: Option<PathBuf>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn authority(state: &AppState) -> RuntimeAuthority {
    RuntimeAuthority::from_current_process(
        Some(&state.profile.to_string()),
        &state.rua_root,
        &state.cache_dir,
    )
}

fn device_id(state: &AppState) -> Option<String> {
    state.sync_device.read().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|device| hex::encode(device.device_id_short()))
    })
}

async fn probe(state: &AppState, selected: Option<PathBuf>) -> BrowserStatus {
    let authority = authority(state);
    let capability: RuntimeCapability = authority.selected_chromium(selected.as_deref());
    let mut status = BrowserStatus {
        device_id: device_id(state),
        available: false,
        state: "unavailable".into(),
        selected_path: selected,
        resolved_path: capability.path.clone(),
        driver_version: DRIVER_VERSION,
        diagnostics: capability.diagnostics.clone(),
    };
    if !capability.available {
        return status;
    }
    let Some(mut settings) = BundledBrowser::from_runtime(&authority, &state.cache_dir) else {
        status.diagnostics =
            vec!["The execution device has not loaded the bundled Browser Use driver.".into()];
        return status;
    };
    settings.browser = capability;
    settings.preferences_path = None;
    let client = BundledBrowserClient::new(settings);
    match client.connect().await {
        Ok(()) => {
            status.available = true;
            status.state = "ready".into();
            status.diagnostics.clear();
        }
        Err(error) => status.diagnostics = vec![error.to_string()],
    }
    if let Err(error) = client.shutdown().await {
        status.available = false;
        status.state = "unavailable".into();
        status.diagnostics.push(error.to_string());
    }
    status
}

async fn status(State(state): State<AppState>) -> Response {
    let _lock = PREFERENCE_UPDATE.lock().await;
    let preferences = match DesktopAutomationPreferences::load(&preferences_path(&state.cache_dir))
    {
        Ok(preferences) => preferences,
        Err(error) => {
            return ApiError::Internal(format!("Cannot read device browser preference: {error}"))
                .into_response()
        }
    };
    Json(probe(&state, preferences.browser_path).await).into_response()
}

async fn recheck(State(state): State<AppState>) -> Response {
    let _lock = PREFERENCE_UPDATE.lock().await;
    let preferences = match DesktopAutomationPreferences::load(&preferences_path(&state.cache_dir))
    {
        Ok(preferences) => preferences,
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    let result = probe(&state, preferences.browser_path).await;
    refresh_managers(&state).await;
    Json(result).into_response()
}

async fn select_browser(
    State(state): State<AppState>,
    Json(selection): Json<BrowserSelection>,
) -> Response {
    let _lock = PREFERENCE_UPDATE.lock().await;
    let result = probe(&state, selection.path.clone()).await;
    if !result.available {
        return ApiError::BadRequest(result.diagnostics.join("; ")).into_response();
    }
    let path = preferences_path(&state.cache_dir);
    let mut preferences = match DesktopAutomationPreferences::load(&path) {
        Ok(preferences) => preferences,
        Err(error) => return ApiError::Internal(error.to_string()).into_response(),
    };
    preferences.browser_path = selection.path;
    preferences.extra.extend(selection.extra);
    if let Err(error) = preferences.save(&path) {
        return ApiError::Internal(format!("Browser preference could not be saved: {error}"))
            .into_response();
    }
    refresh_managers(&state).await;
    Json(result).into_response()
}

async fn refresh_managers(state: &AppState) {
    if let Ok(workspaces) = state.registry.list().await {
        for workspace in workspaces {
            crate::mcp_reload::reload_from_disk(state, &workspace.agent_id).await;
            if let Some(runtime) = state.registry.get(&workspace.agent_id).await {
                if let Some(manager) = runtime.mcp_manager().await {
                    if let Ok(servers) = manager.list_servers().await {
                        for server in servers.into_iter().filter(|server| {
                            server.enabled
                                && server.transport_type == "bundled_browser"
                                && !server.state.is_available()
                        }) {
                            let _ = manager
                                .connect_server(&mcp_manager::types::McpServerId::new(server.key))
                                .await;
                        }
                    }
                }
            }
        }
    }
}

#[derive(Serialize)]
pub struct ComputerStatus {
    pub device_id: Option<String>,
    pub available: bool,
    pub state: String,
    pub platform: String,
    pub driver_version: &'static str,
    pub permissions: Option<rua_mcp::bundled_computer::ComputerPermissions>,
    pub diagnostics: Vec<String>,
}

pub(super) async fn computer_status(State(state): State<AppState>) -> Json<ComputerStatus> {
    use rua_mcp::bundled_computer::BundledComputerClient;
    let authority = authority(&state);
    let mut result = ComputerStatus {
        device_id: device_id(&state),
        available: false,
        state: "unavailable".into(),
        platform: authority.inventory().os_family.clone(),
        driver_version: rua_mcp::bundled_computer::DRIVER_VERSION,
        permissions: None,
        diagnostics: vec![],
    };
    let Some(settings) = state.desktop_automation.settings().cloned() else {
        result.diagnostics.push(
            "The execution device has not loaded the Computer Use driver and native host.".into(),
        );
        return Json(result);
    };
    let client = BundledComputerClient::new(settings);
    match client.diagnostics().await {
        Ok(permissions) => {
            result.available = permissions.accessibility || permissions.capture_allowed();
            result.state = if permissions.accessibility && permissions.capture_allowed() {
                "ready"
            } else {
                "permission_required"
            }
            .into();
            if !permissions.accessibility {
                result
                    .diagnostics
                    .push("Grant Rua Accessibility permission in system settings.".into());
            }
            if !permissions.capture_allowed() {
                result
                    .diagnostics
                    .push("Grant Rua Screen Recording permission in system settings.".into());
            }
            result.permissions = Some(permissions);
            if client.control_requires_restart().await {
                result.state = "restart_required".into();
                result.diagnostics.push("Previous desktop input outcome is unknown. The desktop Runtime is recovering its executor; observe the target again after recovery. No input is replayed.".into());
            }
        }
        Err(error) => result.diagnostics.push(error.to_string()),
    }
    if let Err(error) = client.shutdown().await {
        result.available = false;
        result.state = "unavailable".into();
        result.diagnostics.push(error.to_string());
    }
    Json(result)
}

async fn recheck_computer(State(state): State<AppState>) -> Json<ComputerStatus> {
    refresh_computer(state, false).await
}

pub(super) async fn refresh_computer(state: AppState, restart: bool) -> Json<ComputerStatus> {
    let error = state.desktop_automation.recheck(restart).await.err();
    let mut status = computer_status(State(state)).await;
    if let Some(error) = error {
        status.available = false;
        status.state = "unavailable".into();
        if !status.diagnostics.contains(&error) {
            status.diagnostics.push(error);
        }
    }
    status
}
