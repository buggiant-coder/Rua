//! Private Shell updater/migration transport. Desktop tools execute in the Runtime.
use rua_core::desktop_control::{DesktopControlCommand, DesktopControlEndpoint};
use rua_core::RuaError;
use serde_json::{json, Value};
use tauri::Manager;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct DesktopControlState {
    inner: Mutex<Option<OwnedControl>>,
}

struct OwnedControl {
    endpoint: DesktopControlEndpoint,
    cancel: CancellationToken,
}

impl Drop for OwnedControl {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub async fn start(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<DesktopControlState>();
    let mut inner = state.inner.lock().await;
    if inner.is_none() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| e.to_string())?;
        let endpoint =
            DesktopControlEndpoint::new(listener.local_addr().map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let cancel = CancellationToken::new();
        let owner = OwnedControl {
            endpoint: endpoint.clone(),
            cancel: cancel.clone(),
        };
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let actions = cancel.clone();
            if let Err(error) = endpoint
                .serve(listener, cancel, move |command| {
                    let app = app.clone();
                    let cancel = actions.child_token();
                    async move {
                        dispatch(app, command, cancel)
                            .await
                            .map_err(|message| RuaError::Runtime { message })
                    }
                })
                .await
            {
                eprintln!("[Rua Desktop] Native control stopped: {error}");
            }
        });
        *inner = Some(owner);
    }
    Ok(())
}

pub async fn environment_value(app: &tauri::AppHandle) -> Result<String, String> {
    let state = app.state::<DesktopControlState>();
    let inner = state.inner.lock().await;
    inner
        .as_ref()
        .ok_or("Desktop app updater has not started")?
        .endpoint
        .environment_value()
        .map_err(|error| error.to_string())
}

pub async fn stop(app: &tauri::AppHandle) {
    app.state::<DesktopControlState>().inner.lock().await.take();
}

async fn dispatch(
    app: tauri::AppHandle,
    command: DesktopControlCommand,
    cancel: CancellationToken,
) -> std::result::Result<Value, String> {
    app.state::<crate::update::DesktopLifecycle>()
        .ensure_running()?;
    use DesktopControlCommand as Command;
    match command {
        Command::LegacyStatus => value(crate::desktop_install::status(&app).await?),
        Command::UpdateStatus => value(crate::update::status(&app)),
        Command::CheckUpdate => value(crate::update::check(&app).await?),
        Command::InstallUpdate => {
            // Return acceptance before the updater stops its own HTTP resident.
            crate::update::schedule_install(&app, cancel)?;
            Ok(json!({ "status": "accepted" }))
        }
        Command::MigrateLegacy { confirmed } => {
            crate::desktop_install::require_migration_confirmation(confirmed)?;
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {},
                    result = crate::desktop_install::migrate_legacy_runtime(&app, true) => {
                        if let Err(error) = result {
                            eprintln!("[Rua Desktop] Legacy migration failed: {error}");
                        }
                    }
                }
            });
            Ok(json!({ "status": "accepted" }))
        }
    }
}

fn value(value: impl serde::Serialize) -> std::result::Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}
