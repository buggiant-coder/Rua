//! One HTTP surface for installation, updates and native desktop control.
use std::collections::BTreeMap;
use std::path::PathBuf;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use rua_core::desktop_automation::PermissionSettings;
use rua_core::desktop_cli::DesktopCliInstallation;
use rua_core::desktop_control::{DesktopControlCommand as Command, DesktopControlEndpoint};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::errors::ApiError;
use crate::state::AppState;

static CLI_INSTALL: Mutex<()> = Mutex::const_new(());

/// Captured once at router assembly, never selected by the HTTP caller.
#[derive(Clone)]
pub struct DesktopControlService {
    endpoint: Option<DesktopControlEndpoint>,
    cli: Option<DesktopCliInstallation>,
}

impl DesktopControlService {
    pub fn new(endpoint: Option<DesktopControlEndpoint>, source: PathBuf, home: PathBuf) -> Self {
        Self {
            endpoint,
            cli: Some(DesktopCliInstallation::new(source, &home)),
        }
    }

    pub fn from_environment() -> Self {
        let endpoint = DesktopControlEndpoint::from_environment().unwrap_or_else(|error| {
            tracing::warn!(%error, "native desktop control unavailable");
            None
        });
        let cli = std::env::current_exe()
            .ok()
            .and_then(|path| path.canonicalize().ok())
            .zip(dirs::home_dir())
            .map(|(executable, home)| {
                // A standalone development server sits beside the unified CLI artifact.
                let name = if cfg!(windows) { "rua.exe" } else { "rua" };
                DesktopCliInstallation::new(executable.with_file_name(name), &home)
            });
        Self { endpoint, cli }
    }

    pub async fn native(&self, command: Command) -> Result<Value, ApiError> {
        let endpoint = self.endpoint.as_ref().ok_or_else(|| {
            ApiError::Unavailable(
                "The execution device has no desktop app updater connected.".into(),
            )
        })?;
        endpoint.call(command).await.map_err(ApiError::from)
    }

    pub fn has_native_host(&self) -> bool {
        self.endpoint.is_some()
    }

    fn cli(&self) -> Result<&DesktopCliInstallation, ApiError> {
        self.cli.as_ref().ok_or_else(|| {
            ApiError::Unavailable(
                "Cannot resolve the execution device's CLI installation directory".into(),
            )
        })
    }

    async fn install_status(&self) -> Result<Value, ApiError> {
        let mut status = if self.has_native_host() {
            self.native(Command::LegacyStatus)
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, "legacy app installation status unavailable");
                    json!({ "legacyRuntime": false, "legacyPort": null })
                })
        } else {
            json!({ "platform": std::env::consts::OS, "legacyRuntime": false, "legacyPort": null })
        };
        status["platform"] = json!(std::env::consts::OS);
        let cli = self.cli()?.status();
        status["cliPath"] = json!(cli.path);
        status["cliInstalled"] = json!(cli.installed);
        status["cliConflict"] = json!(cli.conflict);
        Ok(status)
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/platform/desktop/install", get(install_status))
        .route("/platform/desktop/cli", post(install_cli))
        .route("/platform/desktop/legacy/migrate", post(migrate_legacy))
        .route("/platform/desktop/update", get(update_status))
        .route("/platform/desktop/update/check", post(check_update))
        .route("/platform/desktop/update/install", post(install_update))
        .route("/platform/computer/restart", post(restart_computer))
        .route("/platform/computer/permissions", post(request_permissions))
        .route(
            "/platform/computer/permission-settings",
            post(open_permission_settings),
        )
}

async fn install_status(
    Extension(control): Extension<DesktopControlService>,
) -> Result<Json<Value>, ApiError> {
    control.install_status().await.map(Json)
}

async fn install_cli(
    Extension(control): Extension<DesktopControlService>,
) -> Result<Json<Value>, ApiError> {
    if cfg!(windows) {
        return Err(ApiError::Conflict(
            "The Windows installer owns the rua command and PATH entry".into(),
        ));
    }
    let _installation = CLI_INSTALL.lock().await;
    let cli = control.cli()?.clone();
    tokio::task::spawn_blocking(move || cli.install())
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))??;
    control.install_status().await.map(Json)
}

async fn update_status(
    Extension(control): Extension<DesktopControlService>,
) -> Result<Json<Value>, ApiError> {
    if !control.has_native_host() {
        return Ok(Json(json!({
            "phase": "unavailable", "currentVersion": env!("CARGO_PKG_VERSION"),
            "availableVersion": null, "body": null, "downloaded": 0, "total": null,
            "error": null
        })));
    }
    control.native(Command::UpdateStatus).await.map(Json)
}

async fn check_update(
    Extension(control): Extension<DesktopControlService>,
) -> Result<Json<Value>, ApiError> {
    control.native(Command::CheckUpdate).await.map(Json)
}

async fn install_update(
    Extension(control): Extension<DesktopControlService>,
) -> Result<Response, ApiError> {
    let result = control.native(Command::InstallUpdate).await?;
    Ok((StatusCode::ACCEPTED, Json(result)).into_response())
}

#[derive(Deserialize)]
struct MigrationRequest {
    confirmed: bool,
    #[serde(flatten)]
    _extra: BTreeMap<String, Value>,
}

async fn migrate_legacy(
    Extension(control): Extension<DesktopControlService>,
    Json(request): Json<MigrationRequest>,
) -> Result<Response, ApiError> {
    if !request.confirmed {
        return Err(ApiError::BadRequest(
            "Legacy migration requires confirmation".into(),
        ));
    }
    let result = control
        .native(Command::MigrateLegacy { confirmed: true })
        .await?;
    Ok((StatusCode::ACCEPTED, Json(result)).into_response())
}

async fn restart_computer(
    State(state): State<AppState>,
) -> Json<super::desktop_automation::ComputerStatus> {
    super::desktop_automation::refresh_computer(state, true).await
}

async fn request_permissions(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    state
        .desktop_automation
        .request_permissions()
        .await
        .map(Json)
        .map_err(ApiError::Unavailable)
}

#[derive(Deserialize)]
struct PermissionSettingsRequest {
    setting: PermissionSettings,
    #[serde(flatten)]
    _extra: BTreeMap<String, Value>,
}

async fn open_permission_settings(
    State(state): State<AppState>,
    Json(request): Json<PermissionSettingsRequest>,
) -> Result<Json<Value>, ApiError> {
    state
        .desktop_automation
        .open_permission_settings(request.setting)
        .await
        .map_err(ApiError::Unavailable)?;
    Ok(Json(Value::Null))
}
