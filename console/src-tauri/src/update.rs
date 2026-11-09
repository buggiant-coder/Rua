use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::Manager;
use tauri_plugin_updater::UpdaterExt;

#[cfg(any(target_os = "macos", windows))]
use crate::runtime;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    Available,
    UpToDate,
    Downloading,
    Installing,
    Error,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUpdateStatus {
    pub phase: UpdatePhase,
    pub current_version: String,
    pub available_version: Option<String>,
    pub body: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct DesktopUpdateState {
    status: Mutex<DesktopUpdateStatus>,
    request: tokio::sync::Mutex<()>,
    install_scheduled: AtomicBool,
}

#[derive(Default)]
pub struct DesktopLifecycle {
    pub(crate) operation: tokio::sync::Mutex<()>,
    exiting: AtomicBool,
}

impl DesktopLifecycle {
    pub fn request_exit(&self) {
        self.exiting.store(true, Ordering::Release);
    }

    pub fn ensure_running(&self) -> Result<(), String> {
        if self.exiting.load(Ordering::Acquire) {
            Err("Rua Desktop is shutting down".into())
        } else {
            Ok(())
        }
    }

    pub async fn begin_operation(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, String> {
        let operation = self.operation.lock().await;
        self.ensure_running()?;
        Ok(operation)
    }
}

impl DesktopUpdateState {
    pub fn snapshot(&self) -> DesktopUpdateStatus {
        self.status.lock().expect("update status poisoned").clone()
    }

    fn replace(&self, status: DesktopUpdateStatus) {
        *self.status.lock().expect("update status poisoned") = status;
    }

    fn update_download(&self, chunk: usize, total: Option<u64>) {
        let mut status = self.status.lock().expect("update status poisoned");
        status.phase = UpdatePhase::Downloading;
        status.downloaded = status.downloaded.saturating_add(chunk as u64);
        status.total = total;
    }

    fn fail(&self, current_version: String, error: impl ToString) -> String {
        let error = error.to_string();
        self.replace(DesktopUpdateStatus {
            phase: UpdatePhase::Error,
            current_version,
            error: Some(error.clone()),
            ..DesktopUpdateStatus::default()
        });
        error
    }
}

pub fn status(app: &tauri::AppHandle) -> DesktopUpdateStatus {
    let state = app.state::<DesktopUpdateState>();
    let mut status = state.snapshot();
    #[cfg(windows)]
    if let Ok(directory) = app.path().app_local_data_dir() {
        if let Ok(bytes) = std::fs::read(directory.join("desktop-update-result.json")) {
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if value["success"] == false {
                    status.phase = UpdatePhase::Error;
                    status.error = value["error"].as_str().map(str::to_owned);
                }
            }
        }
    }
    if status.current_version.is_empty() {
        status.current_version = app.package_info().version.to_string();
    }
    status
}

pub async fn check(app: &tauri::AppHandle) -> Result<DesktopUpdateStatus, String> {
    let current_version = app.package_info().version.to_string();
    let state = app.state::<DesktopUpdateState>();
    let _request = state
        .request
        .try_lock()
        .map_err(|_| "A Desktop update operation is already running")?;
    state.replace(DesktopUpdateStatus {
        phase: UpdatePhase::Checking,
        current_version: current_version.clone(),
        ..DesktopUpdateStatus::default()
    });

    let updater = app
        .updater()
        .map_err(|error| state.fail(current_version.clone(), error))?;
    match updater
        .check()
        .await
        .map_err(|error| state.fail(current_version.clone(), error))?
    {
        Some(update) => {
            let status = DesktopUpdateStatus {
                phase: UpdatePhase::Available,
                current_version,
                available_version: Some(update.version),
                body: update.body,
                ..DesktopUpdateStatus::default()
            };
            state.replace(status.clone());
            Ok(status)
        }
        None => {
            let status = DesktopUpdateStatus {
                phase: UpdatePhase::UpToDate,
                current_version,
                ..DesktopUpdateStatus::default()
            };
            state.replace(status.clone());
            Ok(status)
        }
    }
}

pub fn schedule_install(
    app: &tauri::AppHandle,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<(), String> {
    let state = app.state::<DesktopUpdateState>();
    if state.install_scheduled.swap(true, Ordering::AcqRel) {
        return Err("A Desktop update operation is already running".into());
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {},
            result = install(&app) => {
                if let Err(error) = result { eprintln!("[Rua Desktop] Update failed: {error}"); }
            }
        }
        app.state::<DesktopUpdateState>()
            .install_scheduled
            .store(false, Ordering::Release);
    });
    Ok(())
}

pub async fn install(app: &tauri::AppHandle) -> Result<(), String> {
    let current_version = app.package_info().version.to_string();
    let state = app.state::<DesktopUpdateState>();
    let _request = state
        .request
        .try_lock()
        .map_err(|_| "A Desktop update operation is already running")?;
    let updater = app
        .updater()
        .map_err(|error| state.fail(current_version.clone(), error))?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|error| state.fail(current_version.clone(), error))?
    else {
        return Err(state.fail(current_version, "no Desktop update is available"));
    };
    state.replace(DesktopUpdateStatus {
        phase: UpdatePhase::Downloading,
        current_version: current_version.clone(),
        available_version: Some(update.version.clone()),
        body: update.body.clone(),
        ..DesktopUpdateStatus::default()
    });
    // Tauri validates the complete archive's pinned updater signature here.
    let bytes = update
        .download(|chunk, total| state.update_download(chunk, total), || {})
        .await
        .map_err(|error| state.fail(current_version.clone(), error))?;
    let executable = std::env::current_exe().map_err(|e| state.fail(current_version.clone(), e))?;
    #[cfg(target_os = "macos")]
    let prepared = {
        let bytes = bytes.clone();
        let version = update.version.clone();
        tokio::task::spawn_blocking(move || {
            crate::update_bundle::MacUpdate::prepare(&bytes, &executable, &version)
        })
        .await
        .map_err(|error| state.fail(current_version.clone(), error))?
        .map_err(|error| state.fail(current_version.clone(), error))?
    };
    #[cfg(windows)]
    let prepared = {
        let directory = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let bytes = bytes.clone();
        let version = update.version.clone();
        tokio::task::spawn_blocking(move || {
            crate::update_bundle::WindowsUpdate::prepare(
                &bytes,
                &executable,
                &version,
                &directory.join("desktop-update-result.json"),
            )
        })
        .await
        .map_err(|error| state.fail(current_version.clone(), error))?
        .map_err(|error| state.fail(current_version.clone(), error))?
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (bytes, executable);
        return Err(state.fail(current_version, "Desktop updates require macOS or Windows"));
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        // Serializes app update, resident startup and retry until handoff.
        let lifecycle = app.state::<DesktopLifecycle>();
        let operation = lifecycle.begin_operation().await?;
        if let Err(error) = runtime::stop_owned(app).await {
            let recovered = runtime::start_owned(app).await;
            return Err(state.fail(
                current_version,
                format!(
                    "Update stopped before installation: {error}; Runtime recovery: {recovered:?}"
                ),
            ));
        }
        state.replace(DesktopUpdateStatus {
            phase: UpdatePhase::Installing,
            current_version: current_version.clone(),
            available_version: Some(update.version.clone()),
            body: update.body.clone(),
            downloaded: bytes.len() as u64,
            total: Some(bytes.len() as u64),
            error: None,
        });
        #[cfg(target_os = "macos")]
        let installed = tokio::task::spawn_blocking(move || prepared.install())
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
        #[cfg(windows)]
        let installed = tokio::task::spawn_blocking(move || prepared.handoff())
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
        if let Err(error) = installed {
            let recovered = runtime::start_owned(app).await;
            return Err(state.fail(
                current_version,
                format!("{error}; Runtime recovery: {recovered:?}"),
            ));
        }
        // No queued startup action may recreate the old resident.
        // Release the mutex before dispatching Tauri's exit event.
        lifecycle.request_exit();
        app.state::<crate::tray::TrayState>().complete_shutdown();
        drop(operation);
        #[cfg(target_os = "macos")]
        app.request_restart();
        #[cfg(windows)]
        app.exit(0);
        Ok(())
    }
}
