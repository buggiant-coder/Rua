pub mod bundle;
pub mod control;
pub mod desktop_install;
pub mod native;
pub mod owner;
pub mod preferences;
#[cfg(target_os = "macos")]
mod quit_confirmation;
pub mod runtime;
#[cfg(target_os = "macos")]
pub mod text_input;
pub mod tray;
pub mod update;
pub mod update_bundle;
pub mod user_notification;

use serde::Serialize;
use std::sync::Mutex;
use tauri::{Manager, RunEvent};
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
struct DesktopShellState {
    bootstrap_url: Mutex<Option<tauri::Url>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopBootstrapContext {
    locale: &'static str,
    labels: preferences::DesktopLabels,
    rua_root: String,
    rua_root_environment_managed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuaRootSelection {
    status: RuaRootSelectionStatus,
    rua_root: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum RuaRootSelectionStatus {
    Selected,
    Cancelled,
}

#[tauri::command]
fn desktop_runtime_status(
    state: tauri::State<'_, runtime::DesktopRuntimeState>,
) -> runtime::RuntimeStatus {
    state.status()
}

#[tauri::command]
async fn retry_desktop_runtime(app: tauri::AppHandle) -> Result<(), String> {
    runtime::restart(&app).await
}

#[tauri::command]
fn desktop_bootstrap_context(app: tauri::AppHandle) -> Result<DesktopBootstrapContext, String> {
    let store = preferences::DesktopPreferencesStore::for_app(&app)?;
    let locale = store.load()?.locale();
    Ok(DesktopBootstrapContext {
        locale: locale.tag(),
        labels: preferences::DesktopLabels::for_locale(locale),
        rua_root: runtime::configured_rua_root(&app)?
            .to_string_lossy()
            .into_owned(),
        rua_root_environment_managed: preferences::rua_root_is_environment_managed().is_some(),
    })
}

#[tauri::command]
async fn select_desktop_rua_root(app: tauri::AppHandle) -> Result<RuaRootSelection, String> {
    select_and_restart_desktop_runtime(&app).await
}

#[tauri::command]
fn set_desktop_locale(app: tauri::AppHandle, locale: String) -> Result<(), String> {
    let locale = preferences::DesktopLocale::from_tag(&locale);
    preferences::DesktopPreferencesStore::for_app(&app)?.set_locale(locale)?;
    tray::apply_locale(&app, locale)
}

pub fn run() {
    #[cfg(target_os = "macos")]
    text_input::configure();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            match tray::secondary_launch_action(&args) {
                tray::SecondaryLaunchAction::ShowMainWindow => tray::show_main_window(app),
                tray::SecondaryLaunchAction::Quit => tray::quit_app(app),
            }
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(runtime::DesktopRuntimeState::default())
        .manage(control::DesktopControlState::default())
        .manage(update::DesktopUpdateState::default())
        .manage(update::DesktopLifecycle::default())
        .manage(tray::TrayState::default())
        .manage(DesktopShellState::default())
        .invoke_handler(tauri::generate_handler![
            desktop_runtime_status,
            retry_desktop_runtime,
            desktop_bootstrap_context,
            select_desktop_rua_root,
            set_desktop_locale,
            native::save_desktop_file,
            native::open_external_link,
            native::open_llm_debug_capture_directory,
            native::open_project_resource_directory,
            user_notification::show_user_notification
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                app.state::<DesktopShellState>()
                    .bootstrap_url
                    .lock()
                    .expect("Desktop Shell state poisoned")
                    .replace(window.url()?);
            }
            tray::setup(app)?;
            let app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = control::start(&app).await {
                    eprintln!("[Rua Desktop] Failed to start app updater transport: {error}");
                    return;
                }
                let _ = runtime::start(&app).await;
            });
            Ok(())
        })
        .on_menu_event(tray::handle_menu_event)
        .on_window_event(tray::handle_window_event)
        .build(tauri::generate_context!())
        .unwrap_or_else(|error| {
            eprintln!("[Rua Desktop] fatal startup error: {error}");
            std::process::exit(1);
        });

    app.run(|app_handle, event| match event {
        RunEvent::ExitRequested { api, code, .. } => {
            match app_handle.state::<tray::TrayState>().exit_request_action() {
                tray::ExitRequestAction::Allow => {}
                tray::ExitRequestAction::PreventAndQuit => {
                    api.prevent_exit();
                    if code.is_none() {
                        tray::request_quit(app_handle);
                    } else {
                        // Programmatic exits (including updates) have already been requested.
                        tray::quit_app(app_handle);
                    }
                }
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => tray::show_main_window(app_handle),
        _ => {}
    });
}

pub(crate) fn request_rua_root_switch(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = select_and_restart_desktop_runtime(&app).await {
            eprintln!("[Rua Desktop] working directory switch failed: {error}");
        }
    });
}

async fn select_and_restart_desktop_runtime(
    app: &tauri::AppHandle,
) -> Result<RuaRootSelection, String> {
    if let Some(value) = preferences::rua_root_is_environment_managed() {
        return Err(format!(
            "Rua Root is managed by RUA_DESKTOP_ROOT={}; unset it before changing the root in Rua.",
            value.to_string_lossy()
        ));
    }

    let preferences = preferences::DesktopPreferencesStore::for_app(app)?;
    let locale = preferences.load()?.locale();
    let labels = preferences::DesktopLabels::for_locale(locale);
    let current = runtime::configured_rua_root(app)?;
    let mut dialog = app.dialog().file().set_title(labels.choose_rua_root_title);
    if current.is_dir() {
        dialog = dialog.set_directory(&current);
    }

    let (sender, receiver) = tokio::sync::oneshot::channel();
    dialog.pick_folder(move |path| {
        let _ = sender.send(path);
    });
    let selected = receiver
        .await
        .map_err(|_| "working directory dialog closed unexpectedly".to_string())?;
    let Some(selected) = selected else {
        return Ok(RuaRootSelection {
            status: RuaRootSelectionStatus::Cancelled,
            rua_root: None,
        });
    };
    let selected = selected
        .into_path()
        .map_err(|error| format!("selected Rua Root is invalid: {error}"))?;
    preferences.set_rua_root(&selected)?;

    show_bootstrap(app)?;
    runtime::restart(app).await?;
    tray::show_main_window(app);
    Ok(RuaRootSelection {
        status: RuaRootSelectionStatus::Selected,
        rua_root: Some(selected.to_string_lossy().into_owned()),
    })
}

fn show_bootstrap(app: &tauri::AppHandle) -> Result<(), String> {
    let url = app
        .state::<DesktopShellState>()
        .bootstrap_url
        .lock()
        .map_err(|_| "Desktop Shell state is poisoned".to_string())?
        .clone()
        .ok_or_else(|| "Desktop bootstrap URL is unavailable".to_string())?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Desktop main window is unavailable".to_string())?;
    window
        .navigate(url)
        .map_err(|error| format!("failed to open Desktop startup page: {error}"))
}
