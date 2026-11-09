use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "macos")]
use tauri::menu::CheckMenuItem;
use tauri::menu::{AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};
#[cfg(target_os = "macos")]
use tauri_plugin_dialog::DialogExt;

use crate::preferences::{DesktopLabels, DesktopLocale, DesktopPreferencesStore};
use crate::runtime;

const TRAY_ID: &str = "rua-tray";
const SHOW_MENU_ID: &str = "show";
const SWITCH_RUA_ROOT_MENU_ID: &str = "switch-rua-root";
const QUIT_MENU_ID: &str = "quit";
#[cfg(target_os = "macos")]
const CONFIRM_QUIT_MENU_ID: &str = "confirm-before-quit";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitRequestAction {
    Allow,
    PreventAndQuit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryLaunchAction {
    ShowMainWindow,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCloseAction {
    HideToTray,
}

#[derive(Default)]
pub struct TrayState {
    quitting: AtomicBool,
    confirmation_pending: AtomicBool,
    shutdown_complete: AtomicBool,
}

impl TrayState {
    pub fn exit_request_action(&self) -> ExitRequestAction {
        if self.shutdown_complete.load(Ordering::Acquire) {
            ExitRequestAction::Allow
        } else {
            ExitRequestAction::PreventAndQuit
        }
    }

    pub fn complete_shutdown(&self) {
        self.shutdown_complete.store(true, Ordering::Release);
    }

    pub fn begin_quit_confirmation(&self) -> bool {
        !self.quitting.load(Ordering::SeqCst)
            && !self.confirmation_pending.swap(true, Ordering::SeqCst)
    }

    pub fn finish_quit_confirmation(&self) {
        self.confirmation_pending.store(false, Ordering::SeqCst);
    }

    fn begin_quit(&self) -> bool {
        !self.quitting.swap(true, Ordering::SeqCst)
    }
}

pub fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let locale = DesktopPreferencesStore::for_app(app.handle())?
        .load()?
        .locale();
    let labels = DesktopLabels::for_locale(locale);
    let switch_enabled =
        rua_root_switch_enabled(crate::preferences::rua_root_is_environment_managed().is_some());
    app.set_menu(application_menu(app.handle(), labels, switch_enabled)?)?;
    #[cfg(target_os = "macos")]
    clean_edit_menu(app.handle(), labels.edit)?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&tray_menu(app.handle(), labels, switch_enabled)?)
        .tooltip("Rua")
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

pub fn apply_locale(app: &tauri::AppHandle, locale: DesktopLocale) -> Result<(), String> {
    let labels = DesktopLabels::for_locale(locale);
    let switch_enabled =
        rua_root_switch_enabled(crate::preferences::rua_root_is_environment_managed().is_some());
    let menu = application_menu(app, labels, switch_enabled)
        .map_err(|error| format!("failed to build localized application menu: {error}"))?;
    app.set_menu(menu)
        .map_err(|error| format!("failed to update application menu: {error}"))?;
    #[cfg(target_os = "macos")]
    clean_edit_menu(app, labels.edit)?;
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let menu = tray_menu(app, labels, switch_enabled)
            .map_err(|error| format!("failed to build localized tray menu: {error}"))?;
        tray.set_menu(Some(menu))
            .map_err(|error| format!("failed to update tray menu: {error}"))?;
    }
    Ok(())
}

pub fn handle_menu_event(app: &tauri::AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        SHOW_MENU_ID => show_main_window(app),
        SWITCH_RUA_ROOT_MENU_ID => crate::request_rua_root_switch(app),
        QUIT_MENU_ID => request_quit(app),
        #[cfg(target_os = "macos")]
        CONFIRM_QUIT_MENU_ID => {
            if let Err(error) = toggle_quit_confirmation(app) {
                app.dialog().message(error).show(|_| {});
            }
        }
        _ => {}
    }
}

pub fn handle_window_event(window: &tauri::Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        if window_close_action() == WindowCloseAction::HideToTray {
            let _ = window.hide();
        }
    }
}

pub fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn secondary_launch_action(args: &[String]) -> SecondaryLaunchAction {
    if args.iter().any(|arg| arg == "--quit") {
        SecondaryLaunchAction::Quit
    } else {
        SecondaryLaunchAction::ShowMainWindow
    }
}

pub fn window_close_action() -> WindowCloseAction {
    WindowCloseAction::HideToTray
}

pub fn rua_root_switch_enabled(environment_managed: bool) -> bool {
    !environment_managed
}

pub fn hide_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

pub fn request_quit(app: &tauri::AppHandle) {
    #[cfg(not(target_os = "macos"))]
    quit_app(app);

    #[cfg(target_os = "macos")]
    {
        if !app.state::<TrayState>().begin_quit_confirmation() {
            return;
        }
        let handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            if let Err(error) = confirm_and_quit(&handle) {
                handle.dialog().message(error).show(|_| {});
            }
            handle.state::<TrayState>().finish_quit_confirmation();
        }) {
            app.state::<TrayState>().finish_quit_confirmation();
            eprintln!("[Rua Desktop] Failed to show quit confirmation: {error}");
        }
    }
}

#[cfg(target_os = "macos")]
fn confirm_and_quit(app: &tauri::AppHandle) -> Result<(), String> {
    let store = DesktopPreferencesStore::for_app(app)?;
    let preferences = store.load()?;
    if preferences.confirm_before_quit() {
        let labels = DesktopLabels::for_locale(preferences.locale());
        show_main_window(app);
        let Some(confirm_before_quit) = crate::quit_confirmation::show(labels)? else {
            return Ok(());
        };
        store.set_confirm_before_quit(confirm_before_quit)?;
    }
    quit_app(app);
    Ok(())
}

#[cfg(target_os = "macos")]
fn toggle_quit_confirmation(app: &tauri::AppHandle) -> Result<(), String> {
    let store = DesktopPreferencesStore::for_app(app)?;
    let preferences = store.load()?;
    let result = store.set_confirm_before_quit(!preferences.confirm_before_quit());
    // Rebuild even on failure so the checkmark reflects the persisted setting.
    apply_locale(app, preferences.locale())?;
    result
}

pub fn quit_app(app: &tauri::AppHandle) {
    let state = app.state::<TrayState>();
    if !state.begin_quit() {
        return;
    }

    app.state::<crate::update::DesktopLifecycle>()
        .request_exit();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = runtime::stop_and_wait(&app).await {
            eprintln!("[Rua Desktop] Runtime shutdown escalation: {error}");
        }
        crate::control::stop(&app).await;
        app.state::<TrayState>().complete_shutdown();
        app.exit(0);
    });
}

fn tray_menu(
    app: &tauri::AppHandle,
    labels: DesktopLabels,
    switch_enabled: bool,
) -> tauri::Result<Menu<tauri::Wry>> {
    let show = MenuItem::with_id(app, SHOW_MENU_ID, labels.show, true, None::<&str>)?;
    let switch = MenuItem::with_id(
        app,
        SWITCH_RUA_ROOT_MENU_ID,
        labels.switch_rua_root,
        switch_enabled,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, QUIT_MENU_ID, labels.quit, true, None::<&str>)?;
    Menu::with_items(app, &[&show, &switch, &quit])
}

fn application_menu(
    app: &tauri::AppHandle,
    labels: DesktopLabels,
    switch_enabled: bool,
) -> tauri::Result<Menu<tauri::Wry>> {
    let switch = MenuItem::with_id(
        app,
        SWITCH_RUA_ROOT_MENU_ID,
        labels.switch_rua_root,
        switch_enabled,
        None::<&str>,
    )?;
    let quit_accelerator = if cfg!(target_os = "macos") {
        Some("Cmd+Q")
    } else {
        None
    };
    let quit = MenuItem::with_id(app, QUIT_MENU_ID, labels.quit, true, quit_accelerator)?;
    let about = AboutMetadata {
        name: Some(app.package_info().name.clone()),
        version: Some(app.package_info().version.to_string()),
        ..AboutMetadata::default()
    };

    #[cfg(target_os = "macos")]
    let confirm_quit = CheckMenuItem::with_id(
        app,
        CONFIRM_QUIT_MENU_ID,
        labels.confirm_before_quit,
        true,
        DesktopPreferencesStore::for_app(app)
            .and_then(|store| store.load())
            .map(|preferences| preferences.confirm_before_quit())
            .unwrap_or(true),
        None::<&str>,
    )?;
    #[cfg(target_os = "macos")]
    let app_menu = Submenu::with_items(
        app,
        "Rua",
        true,
        &[
            &PredefinedMenuItem::about(app, Some(labels.about), Some(about.clone()))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, Some(labels.services))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some(labels.hide))?,
            &PredefinedMenuItem::hide_others(app, Some(labels.hide_others))?,
            &PredefinedMenuItem::show_all(app, Some(labels.show_all))?,
            &PredefinedMenuItem::separator(app)?,
            &confirm_quit,
            &quit,
        ],
    )?;

    let file_menu = Submenu::with_items(
        app,
        labels.file,
        true,
        &[
            &switch,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, Some(labels.close_window))?,
            #[cfg(not(target_os = "macos"))]
            &PredefinedMenuItem::separator(app)?,
            #[cfg(not(target_os = "macos"))]
            &quit,
        ],
    )?;
    let edit_menu = Submenu::with_items(
        app,
        labels.edit,
        true,
        &[
            &PredefinedMenuItem::undo(app, Some(labels.undo))?,
            &PredefinedMenuItem::redo(app, Some(labels.redo))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some(labels.cut))?,
            &PredefinedMenuItem::copy(app, Some(labels.copy))?,
            &PredefinedMenuItem::paste(app, Some(labels.paste))?,
            &PredefinedMenuItem::select_all(app, Some(labels.select_all))?,
        ],
    )?;
    let view_menu = Submenu::with_items(
        app,
        labels.view,
        true,
        &[&PredefinedMenuItem::fullscreen(
            app,
            Some(labels.fullscreen),
        )?],
    )?;
    let window_menu = Submenu::with_items(
        app,
        labels.window,
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some(labels.minimize))?,
            &PredefinedMenuItem::maximize(app, Some(labels.maximize))?,
        ],
    )?;
    let help_menu = Submenu::with_items(
        app,
        labels.help,
        true,
        &[
            #[cfg(not(target_os = "macos"))]
            &PredefinedMenuItem::about(app, Some(labels.about), Some(about))?,
        ],
    )?;

    Menu::with_items(
        app,
        &[
            #[cfg(target_os = "macos")]
            &app_menu,
            &file_menu,
            &edit_menu,
            &view_menu,
            &window_menu,
            &help_menu,
        ],
    )
}

#[cfg(target_os = "macos")]
fn clean_edit_menu(app: &tauri::AppHandle, title: &'static str) -> Result<(), String> {
    app.run_on_main_thread(move || {
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            crate::text_input::clean_edit_menu(mtm, title);
        }
    })
    .map_err(|error| format!("failed to configure edit menu: {error}"))
}
