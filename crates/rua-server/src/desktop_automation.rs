//! Runtime-owned Cua lifecycle and OS grants. No Tauri or WebView dependency.
use rua_core::desktop_automation::PermissionSettings;
use rua_mcp::bundled_computer::BundledComputer;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[cfg_attr(
    target_os = "windows",
    allow(
        clippy::needless_return,
        reason = "the hash-pinned SDK source uses explicit returns in platform-specific branches"
    )
)]
pub mod cua_host;
pub use rua_core::desktop_automation::computer_os_supported as supported_os;
pub use rua_mcp::bundled_computer::DRIVER_VERSION;

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn write_descriptor(path: &Path, bytes: &[u8]) -> Result<(), String> {
    rua_config::atomic_write(path, bytes).map_err(|error| error.to_string())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod supported {
    use super::cua_host::{
        EmbeddedCuaDriverHost, EmbeddedDriverHostOptions, EmbeddedPermissionMode,
    };
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct OwnedHost {
        host: Arc<EmbeddedCuaDriverHost>,
        descriptor: PathBuf,
        bundle: String,
        permissions: super::cua_host::MacOsPermissionStatus,
    }

    #[derive(Default)]
    pub struct DesktopAutomationState {
        // The same lock covers start, publication and stop. A concurrent start
        // reuses the owner instead of leaving an untracked native daemon behind.
        inner: Mutex<Option<OwnedHost>>,
    }

    impl DesktopAutomationState {
        pub async fn start(
            &self,
            directory: PathBuf,
            binary: PathBuf,
            bundle: String,
        ) -> Result<PathBuf, String> {
            let mut inner = self.inner.lock().await;
            std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                    .map_err(|e| e.to_string())?;
            }
            let descriptor = directory.join("cua-connection.json");
            if inner.is_none() {
                #[cfg(target_os = "macos")]
                if !supported_os() {
                    write_descriptor(&descriptor, br#"{"error":"Computer Use requires macOS 13 or later with this bundled Cua Driver"}"#)?;
                    return Ok(descriptor);
                }
                let host = EmbeddedCuaDriverHost::with_options(EmbeddedDriverHostOptions {
                    binary_path: binary.to_string_lossy().into_owned(),
                    host_bundle_id: bundle.clone(),
                    socket_path: None,
                    startup_timeout_ms: Some(10000),
                    shutdown_timeout_ms: Some(3000),
                    permission_mode: Some(EmbeddedPermissionMode::Standard),
                    capability_manifest_path: None,
                    approve_capability_manifest: false,
                    session_policy_path: None,
                    approve_session_policy: false,
                    dangerously_bypass_approvals: false,
                    environment: vec![super::cua_host::EmbeddedEnvironmentVariable {
                        name: "CUA_DRIVER_RS_TELEMETRY_ENABLED".into(),
                        value: "0".into(),
                    }],
                    inherit_stderr: true,
                    no_overlay: true,
                })
                .map_err(|e| e.to_string());
                match host {
                    Ok(host) => {
                        *inner = Some(OwnedHost {
                            host,
                            descriptor: descriptor.clone(),
                            bundle,
                            permissions: super::cua_host::current_mac_os_permission_status(),
                        })
                    }
                    Err(error) => {
                        write_descriptor(
                            &descriptor,
                            &serde_json::to_vec(&serde_json::json!({"error":error}))
                                .map_err(|e| e.to_string())?,
                        )?;
                        return Ok(descriptor);
                    }
                }
            }
            let owner = inner.as_ref().expect("host installed");
            let record = match owner.host.clone().start().await {
                Ok(connection) if connection.driver_version == DRIVER_VERSION => {
                    serde_json::json!({
                        "generation": connection.generation, "socket_path": connection.socket_path,
                        "driver_version": connection.driver_version, "host_bundle_id": owner.bundle,
                        "pid": connection.pid, "error": null,
                    })
                }
                Ok(connection) => {
                    owner.host.clone().stop().await.map_err(|e| e.to_string())?;
                    serde_json::json!({"error":format!("Expected Cua Driver {DRIVER_VERSION}, found {}",connection.driver_version)})
                }
                Err(error) => serde_json::json!({"error":error.to_string()}),
            };
            if let Err(error) = write_descriptor(
                &owner.descriptor,
                &serde_json::to_vec(&record).map_err(|e| e.to_string())?,
            ) {
                let _ = owner.host.clone().stop().await;
                return Err(error);
            }
            Ok(owner.descriptor.clone())
        }

        pub async fn refresh_permissions(&self) -> Result<(), String> {
            let mut inner = self.inner.lock().await;
            if let Some(owner) = inner.as_mut() {
                let current = super::cua_host::current_mac_os_permission_status();
                if current != owner.permissions {
                    // TCC answers can be cached in a running daemon. Only an
                    // explicit user recheck replaces it after a grant change.
                    owner.host.clone().stop().await.map_err(|e| e.to_string())?;
                    write_descriptor(
                        &owner.descriptor,
                        br#"{"error":"Refreshing Rua system permissions"}"#,
                    )?;
                    owner.permissions = current;
                }
            }
            Ok(())
        }

        pub async fn stop(&self) -> Result<(), String> {
            let mut inner = self.inner.lock().await;
            if let Some(owner) = inner.take() {
                let result = owner.host.stop().await.map_err(|e| e.to_string());
                write_descriptor(
                    &owner.descriptor,
                    br#"{"error":"Rua desktop host is stopped"}"#,
                )?;
                result?;
            }
            Ok(())
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use supported::DesktopAutomationState;
/// One owner per resident Runtime. MCP clients only connect to its descriptor.
pub struct DesktopAutomationRuntime {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    host: DesktopAutomationState,
    settings: Option<BundledComputer>,
    operation: Mutex<()>,
    cancel: CancellationToken,
}

impl DesktopAutomationRuntime {
    pub fn new(settings: Option<BundledComputer>) -> Self {
        Self {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            host: DesktopAutomationState::default(),
            settings,
            operation: Mutex::new(()),
            cancel: CancellationToken::new(),
        }
    }

    pub fn settings(&self) -> Option<&BundledComputer> {
        self.settings.as_ref()
    }

    fn ensure_running(&self) -> Result<(), String> {
        if self.cancel.is_cancelled() {
            return Err("Rua Runtime is shutting down".into());
        }
        Ok(())
    }

    fn available_settings(&self) -> Result<&BundledComputer, String> {
        self.ensure_running()?;
        if let Ok(error) = std::env::var("RUA_DESKTOP_AUTOMATION_ERROR") {
            return Err(error);
        }
        let settings = self.settings.as_ref().ok_or(
            "Computer Use requires a macOS or Windows execution device with desktop components",
        )?;
        if !rua_core::runtime_authority::browser::executable_file(&settings.driver) {
            return Err("Computer Use driver is missing from the execution device".into());
        }
        Ok(settings)
    }

    pub async fn recheck(&self, restart: bool) -> Result<(), String> {
        let _operation = self.operation.lock().await;
        let settings = self.available_settings()?;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            if restart {
                self.host.stop().await?;
            } else {
                self.host.refresh_permissions().await?;
            }
            let directory = settings
                .connection_file
                .parent()
                .ok_or("Computer Use connection descriptor has no directory")?;
            self.ensure_running()?;
            let result = self
                .host
                .start(
                    directory.to_path_buf(),
                    settings.driver.clone(),
                    "com.rua.desktop".into(),
                )
                .await;
            if self.cancel.is_cancelled() {
                self.host.stop().await?;
                Err("Rua Runtime is shutting down".into())
            } else {
                result.map(|_| ())
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (settings, restart);
            Err("Computer Use requires a macOS or Windows execution device".into())
        }
    }

    pub async fn request_permissions(&self) -> Result<serde_json::Value, String> {
        let _operation = self.operation.lock().await;
        self.available_settings()?;
        #[cfg(target_os = "macos")]
        {
            // TCC attributes these probes to the Runtime's actual launch chain.
            // No WebView or Tauri event loop participates in the request.
            let cancel = self.cancel.clone();
            let request = tokio::task::spawn_blocking(move || {
                if cancel.is_cancelled() {
                    return Err("Rua Runtime is shutting down".to_string());
                }
                Ok(cua_host::request_mac_os_permissions())
            });
            let status = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Err("Rua Runtime is shutting down".into()),
                result = request => result.map_err(|error| error.to_string())??,
            };
            Ok(
                serde_json::json!({ "accessibility": status.accessibility, "screen_recording": status.screen_recording }),
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("System permission prompts are only needed on macOS".into())
        }
    }

    pub async fn open_permission_settings(
        &self,
        setting: PermissionSettings,
    ) -> Result<(), String> {
        self.ensure_running()?;
        #[cfg(target_os = "macos")]
        {
            let mut command = tokio::process::Command::new("/usr/bin/open");
            command
                .arg(permission_settings_url(setting))
                .kill_on_drop(true);
            let result = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Err("Rua Runtime is shutting down".into()),
                result = command.status() => result.map_err(|error| error.to_string())?,
            };
            if result.success() {
                Ok(())
            } else {
                Err("Could not open macOS permission settings".into())
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = setting;
            Err("System permission settings are only needed on macOS".into())
        }
    }

    pub fn start(self: &Arc<Self>, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
        let runtime = self.clone();
        let mut recovery = rua_mcp::computer_control::recovery_requests();
        tokio::spawn(async move {
            if !cancel.is_cancelled() && !runtime.cancel.is_cancelled() {
                let result = runtime.recheck(false).await;
                if let Err(error) = result {
                    tracing::info!(%error, "Computer Use not ready");
                }
                let mut generation = runtime
                    .settings()
                    .and_then(|settings| settings.connection_generation().ok());
                let mut owned_generations = std::collections::BTreeSet::new();
                if let Some(current) = &generation {
                    owned_generations.insert(current.clone());
                }
                let mut retry_delay = std::time::Duration::from_secs(1);
                loop {
                    if cancel.is_cancelled() || runtime.cancel.is_cancelled() {
                        break;
                    }
                    if generation.is_none() {
                        if let Err(error) = runtime.recheck(false).await {
                            tracing::warn!(%error, "Computer Use startup retry failed");
                        }
                    }
                    if let Some(current) = runtime
                        .settings()
                        .and_then(|settings| settings.connection_generation().ok())
                    {
                        owned_generations.insert(current.clone());
                        generation = Some(current);
                    }
                    let mut failed = generation.is_none() && runtime.settings().is_some();
                    let pending = recovery
                        .borrow()
                        .keys()
                        .find(|key| owned_generations.contains(*key))
                        .cloned();
                    if let Some(current) = pending.as_ref() {
                        if let Err(error) = rua_mcp::computer_control::recover(
                            current,
                            runtime.replace_executor(current, &cancel),
                        )
                        .await
                        {
                            tracing::warn!(%error, "Computer Use executor recovery failed");
                            failed = true;
                        }
                    }
                    owned_generations.retain(|owned| {
                        generation.as_ref() == Some(owned) || recovery.borrow().contains_key(owned)
                    });
                    if failed {
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = runtime.cancel.cancelled() => break,
                            _ = tokio::time::sleep(retry_delay) => {}
                        }
                    } else {
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = runtime.cancel.cancelled() => break,
                            changed = recovery.changed() => {
                                if changed.is_err() { break; }
                            }
                        }
                    }
                    retry_delay = if failed {
                        (retry_delay * 2).min(std::time::Duration::from_secs(30))
                    } else {
                        std::time::Duration::from_secs(1)
                    };
                }
            }
            runtime.stop().await;
        })
    }

    async fn replace_executor(
        &self,
        previous_generation: &str,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        if cancel.is_cancelled() {
            return Err("Rua Runtime is shutting down".into());
        }
        self.recheck(true).await?;
        let replacement = self
            .settings()
            .ok_or_else(|| "Computer Use is not configured".to_string())?
            .connection_generation()
            .map_err(|error| error.to_string())?;
        if replacement == previous_generation {
            return Err("Computer Use recovery did not replace its executor".into());
        }
        Ok(())
    }

    pub async fn stop(&self) {
        self.cancel.cancel();
        let _operation = self.operation.lock().await;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Err(error) = self.host.stop().await {
            tracing::warn!(%error, "Computer Use cleanup failed");
        }
    }
}

pub fn permission_settings_url(setting: PermissionSettings) -> &'static str {
    match setting {
        PermissionSettings::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        PermissionSettings::ScreenRecording => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
    }
}
