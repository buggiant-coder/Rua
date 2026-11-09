use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_shell::process::{CommandChild, CommandEvent, TerminatedPayload};
use tauri_plugin_shell::ShellExt;
use tokio::sync::watch;
use uuid::Uuid;

use crate::desktop_install::{self, DesktopPaths};
use crate::owner::{self, OwnerRecord, ProcessIdentity, ReconcileDecision, OWNER_RECORD_VERSION};
use crate::preferences;

pub const DESKTOP_READY_PREFIX: &str = "RUA_BACKEND_READY ";
const DESKTOP_SHUTDOWN_PATH: &str = "/api/desktop/shutdown";
const DESKTOP_SHUTDOWN_TOKEN_ENV: &str = "RUA_DESKTOP_SHUTDOWN_TOKEN";
const DESKTOP_SHUTDOWN_TOKEN_HEADER: &str = "x-rua-desktop-shutdown-token";
const READY_VERSION: u8 = 1;
const MAX_CAPTURED_STDERR_CHARS: usize = 4000;
const SHUTDOWN_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const GRACEFUL_EXIT_TIMEOUT: Duration = Duration::from_secs(30);
const FORCED_EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const ORPHAN_GRACEFUL_EXIT_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOwnership {
    #[default]
    None,
    Owned,
    Attached,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub generation: u64,
    pub port: Option<u16>,
    pub error: Option<String>,
    pub terminated: bool,
    pub ownership: RuntimeOwnership,
    pub shutdown_escalated: bool,
}

#[derive(Default)]
pub struct RuntimeTracker {
    generation: AtomicU64,
    status: Mutex<RuntimeStatus>,
}

impl RuntimeTracker {
    pub fn begin_generation(&self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let mut status = self.status.lock().expect("runtime status poisoned");
        *status = RuntimeStatus {
            generation,
            ownership: RuntimeOwnership::Owned,
            ..RuntimeStatus::default()
        };
        generation
    }

    pub fn status(&self) -> RuntimeStatus {
        self.status.lock().expect("runtime status poisoned").clone()
    }

    pub fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    pub fn attach_unowned(&self, port: u16) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let mut status = self.status.lock().expect("runtime status poisoned");
        *status = RuntimeStatus {
            generation,
            port: Some(port),
            ownership: RuntimeOwnership::Attached,
            ..RuntimeStatus::default()
        };
    }

    pub fn mark_ready_if_current(&self, generation: u64, port: u16) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        let mut status = self.status.lock().expect("runtime status poisoned");
        if status.generation != generation {
            return false;
        }
        status.port = Some(port);
        status.error = None;
        status.terminated = false;
        true
    }

    pub fn mark_error_if_current(&self, generation: u64, message: String) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        let mut status = self.status.lock().expect("runtime status poisoned");
        if status.generation != generation {
            return false;
        }
        status.error = Some(message);
        true
    }

    pub fn mark_terminated_if_current(&self, generation: u64, message: String) -> bool {
        if !self.is_current(generation) {
            return false;
        }
        let mut status = self.status.lock().expect("runtime status poisoned");
        if status.generation != generation {
            return false;
        }
        status.terminated = true;
        if status.error.is_none() {
            status.error = Some(message);
        }
        true
    }

    fn mark_shutdown_escalated(&self) {
        self.status
            .lock()
            .expect("runtime status poisoned")
            .shutdown_escalated = true;
    }

    fn clear(&self) {
        let generation = self.generation.load(Ordering::SeqCst);
        *self.status.lock().expect("runtime status poisoned") = RuntimeStatus {
            generation,
            ..RuntimeStatus::default()
        };
    }
}

struct OwnedChild {
    generation: u64,
    child: CommandChild,
}

struct ChildRegistration {
    generation: u64,
    child: CommandChild,
    shutdown_token: String,
    terminated: watch::Receiver<bool>,
    owner_path: PathBuf,
    shell_identity: ProcessIdentity,
    runtime_identity: ProcessIdentity,
}

#[derive(Default)]
struct RuntimeInner {
    child: Option<OwnedChild>,
    port: Option<u16>,
    shutdown_token: Option<String>,
    terminated: Option<watch::Receiver<bool>>,
    stopping: bool,
    owner_path: Option<PathBuf>,
    shell_identity: Option<ProcessIdentity>,
    runtime_identity: Option<ProcessIdentity>,
}

#[derive(Default)]
pub struct DesktopRuntimeState {
    tracker: RuntimeTracker,
    inner: Mutex<RuntimeInner>,
}

enum StopPlan {
    NoProcess,
    Wait(watch::Receiver<bool>),
    Request {
        pid: u32,
        port: Option<u16>,
        shutdown_token: Option<String>,
        terminated: watch::Receiver<bool>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopDisposition {
    NoProcess,
    WaitForSharedStop,
    RequestStop,
}

pub fn stop_disposition(
    ownership: RuntimeOwnership,
    has_process: bool,
    stopping: bool,
) -> StopDisposition {
    if ownership != RuntimeOwnership::Owned || !has_process {
        StopDisposition::NoProcess
    } else if stopping {
        StopDisposition::WaitForSharedStop
    } else {
        StopDisposition::RequestStop
    }
}

impl DesktopRuntimeState {
    pub fn status(&self) -> RuntimeStatus {
        self.tracker.status()
    }

    pub fn tracker(&self) -> &RuntimeTracker {
        &self.tracker
    }

    fn install_child(&self, registration: ChildRegistration) {
        let mut inner = self.inner.lock().expect("runtime state poisoned");
        if !self.tracker.is_current(registration.generation) {
            let _ = registration.child.kill();
            return;
        }
        inner.child = Some(OwnedChild {
            generation: registration.generation,
            child: registration.child,
        });
        inner.port = None;
        inner.shutdown_token = Some(registration.shutdown_token);
        inner.terminated = Some(registration.terminated);
        inner.stopping = false;
        inner.owner_path = Some(registration.owner_path);
        inner.shell_identity = Some(registration.shell_identity);
        inner.runtime_identity = Some(registration.runtime_identity);
    }

    fn persist_owner_if_current(&self, generation: u64, port: u16) -> Result<(), String> {
        if !self.tracker.is_current(generation) {
            return Ok(());
        }
        let (path, shell, runtime, shutdown_token) = {
            let mut inner = self.inner.lock().expect("runtime state poisoned");
            let child_is_current = inner
                .child
                .as_ref()
                .is_some_and(|child| child.generation == generation);
            if !child_is_current {
                return Ok(());
            }
            inner.port = Some(port);
            (
                inner.owner_path.clone(),
                inner.shell_identity.clone(),
                inner.runtime_identity.clone(),
                inner.shutdown_token.clone(),
            )
        };

        let (Some(path), Some(shell), Some(runtime), Some(shutdown_token)) =
            (path, shell, runtime, shutdown_token)
        else {
            return Err("Desktop Runtime ownership metadata is incomplete".to_string());
        };
        owner::write(
            &path,
            &OwnerRecord {
                version: OWNER_RECORD_VERSION,
                shell,
                runtime,
                port,
                shutdown_token,
            },
        )
    }

    fn is_stopping(&self, generation: u64) -> bool {
        let inner = self.inner.lock().expect("runtime state poisoned");
        inner.stopping
            && inner
                .child
                .as_ref()
                .is_some_and(|child| child.generation == generation)
    }

    fn clear_child_if_current(&self, generation: u64) {
        let owner_path = {
            let mut inner = self.inner.lock().expect("runtime state poisoned");
            if inner
                .child
                .as_ref()
                .is_none_or(|child| child.generation != generation)
            {
                return;
            }
            inner.child.take();
            inner.port = None;
            inner.shutdown_token = None;
            inner.terminated = None;
            inner.stopping = false;
            inner.shell_identity = None;
            inner.runtime_identity = None;
            inner.owner_path.take()
        };
        if let Some(path) = owner_path {
            owner::remove(&path);
        }
    }

    fn begin_stop(&self) -> StopPlan {
        let ownership = self.tracker.status().ownership;
        let mut inner = self.inner.lock().expect("runtime state poisoned");
        let has_process = inner.child.is_some() && inner.terminated.is_some();
        match stop_disposition(ownership, has_process, inner.stopping) {
            StopDisposition::NoProcess => return StopPlan::NoProcess,
            StopDisposition::WaitForSharedStop => {
                return inner
                    .terminated
                    .clone()
                    .map(StopPlan::Wait)
                    .unwrap_or(StopPlan::NoProcess);
            }
            StopDisposition::RequestStop => {}
        }

        let Some(terminated) = inner.terminated.clone() else {
            return StopPlan::NoProcess;
        };
        if *terminated.borrow() {
            return StopPlan::NoProcess;
        }
        let Some(pid) = inner.child.as_ref().map(|child| child.child.pid()) else {
            return StopPlan::Wait(terminated);
        };
        inner.stopping = true;
        StopPlan::Request {
            pid,
            port: inner.port,
            shutdown_token: inner.shutdown_token.clone(),
            terminated,
        }
    }

    fn force_kill(&self) {
        let child = self
            .inner
            .lock()
            .expect("runtime state poisoned")
            .child
            .take();
        if let Some(child) = child {
            let _ = child.child.kill();
        }
    }

    fn finish_stop(&self) {
        let owner_path = {
            let mut inner = self.inner.lock().expect("runtime state poisoned");
            inner.child.take();
            inner.port = None;
            inner.shutdown_token = None;
            inner.terminated = None;
            inner.stopping = false;
            inner.shell_identity = None;
            inner.runtime_identity = None;
            inner.owner_path.take()
        };
        if let Some(path) = owner_path {
            owner::remove(&path);
        }
        self.tracker.clear();
    }
}

#[derive(Debug, Deserialize)]
pub struct ReadyEndpoint {
    pub version: u8,
    pub host: IpAddr,
    pub port: u16,
    pub pid: u32,
    pub rua_version: String,
}

pub fn ready_endpoint_from_stdout(text: &str) -> Option<ReadyEndpoint> {
    text.lines().find_map(|line| {
        let payload = line.trim().strip_prefix(DESKTOP_READY_PREFIX)?;
        let endpoint = serde_json::from_str::<ReadyEndpoint>(payload).ok()?;
        (endpoint.version == READY_VERSION && endpoint.host.is_loopback() && endpoint.port > 0)
            .then_some(endpoint)
    })
}

pub async fn start(app: &AppHandle) -> Result<(), String> {
    let lifecycle = app.state::<crate::update::DesktopLifecycle>();
    let _operation = lifecycle.begin_operation().await?;
    start_owned(app).await
}

pub(crate) async fn start_owned(app: &AppHandle) -> Result<(), String> {
    app.state::<crate::update::DesktopLifecycle>()
        .ensure_running()?;
    let state = app.state::<DesktopRuntimeState>();
    if state
        .inner
        .lock()
        .expect("runtime state poisoned")
        .child
        .is_some()
    {
        return Ok(());
    }
    #[cfg(not(debug_assertions))]
    let automation_error = tokio::task::spawn_blocking(|| {
        std::env::current_exe()
            .map_err(|e| e.to_string())
            .and_then(|executable| crate::bundle::verify_current(&executable))
    })
    .await
    .map_err(|error| error.to_string())?
    .err();
    #[cfg(debug_assertions)]
    let automation_error: Option<String> = None;
    let generation = state.tracker.begin_generation();
    let launch = RuntimeLaunch::resolve(app).inspect_err(|message| {
        state
            .tracker
            .mark_error_if_current(generation, message.clone());
    })?;
    std::fs::create_dir_all(&launch.rua_root).map_err(|error| {
        startup_error(
            &state,
            generation,
            format!(
                "failed to create Desktop Runtime data directory {}: {error}",
                launch.rua_root.display()
            ),
        )
    })?;

    reconcile_orphan(&launch)
        .await
        .map_err(|message| startup_error(&state, generation, message))?;

    let desktop_paths = DesktopPaths::resolve(app)?;
    if let Some(legacy) = desktop_install::detect_legacy_runtime(&desktop_paths).await? {
        state.tracker.attach_unowned(legacy.port);
        return Ok(());
    }

    if !launch.binary.is_file() {
        return Err(startup_error(
            &state,
            generation,
            format!(
                "unified rua Runtime not found at {}",
                launch.binary.display()
            ),
        ));
    }
    if !launch.console_static_dir.is_dir() {
        return Err(startup_error(
            &state,
            generation,
            format!(
                "Console build not found at {}; run the desktop dev command to build it",
                launch.console_static_dir.display()
            ),
        ));
    }

    let cua_binary = bundled_driver_path(&launch.binary, "cua-driver");
    let control_endpoint = crate::control::environment_value(app).await?;
    let shutdown_token = Uuid::new_v4().to_string();
    let mut command = app
        .shell()
        .command(&launch.binary)
        .args(launch.args())
        .current_dir(&launch.repo_root)
        .env_clear()
        .envs(std::env::vars_os().filter(|(key, _)| {
            key != "RUA_DESKTOP_AUTOMATION_ERROR" && key != "RUA_CUA_CONNECTION_FILE"
        }))
        .env(DESKTOP_SHUTDOWN_TOKEN_ENV, &shutdown_token)
        .env(rua_core::desktop_control::ENDPOINT_ENV, control_endpoint)
        .env(
            "RUA_BUNDLED_AGENT_BROWSER",
            bundled_browser_path(&launch.binary),
        );
    command = command.env("RUA_BUNDLED_CUA_DRIVER", &cua_binary);
    if let Some(error) = automation_error {
        // A damaged automation component must not prevent ordinary Chats.
        command = command.env("RUA_DESKTOP_AUTOMATION_ERROR", error);
    }
    let (receiver, child) = match command.spawn() {
        Ok(spawned) => spawned,
        Err(error) => {
            return Err(startup_error(
                &state,
                generation,
                format!(
                    "failed to spawn unified rua Runtime at {}: {error}",
                    launch.binary.display()
                ),
            ));
        }
    };

    let runtime_pid = child.pid();
    let Some(shell_identity) = capture_identity_with_retry(std::process::id()).await else {
        let _ = child.kill();
        return Err(startup_error(
            &state,
            generation,
            "failed to capture Desktop Shell process identity".to_string(),
        ));
    };
    let Some(runtime_identity) = capture_identity_with_retry(runtime_pid).await else {
        let _ = child.kill();
        return Err(startup_error(
            &state,
            generation,
            "failed to capture owned Runtime process identity".to_string(),
        ));
    };

    let (terminated_sender, terminated_receiver) = watch::channel(false);
    state.install_child(ChildRegistration {
        generation,
        child,
        shutdown_token,
        terminated: terminated_receiver,
        owner_path: launch.owner_file.clone(),
        shell_identity,
        runtime_identity,
    });
    watch_process(
        app.clone(),
        generation,
        receiver,
        terminated_sender,
        launch.startup_log_path(),
    );
    Ok(())
}

pub async fn restart(app: &AppHandle) -> Result<(), String> {
    let lifecycle = app.state::<crate::update::DesktopLifecycle>();
    let _operation = lifecycle.begin_operation().await?;
    stop_owned(app).await?;
    start_owned(app).await
}

pub async fn stop_and_wait(app: &AppHandle) -> Result<(), String> {
    let operation = app.state::<crate::update::DesktopLifecycle>();
    let _operation = operation.operation.lock().await;
    stop_owned(app).await
}

pub(crate) async fn stop_owned(app: &AppHandle) -> Result<(), String> {
    stop_runtime_and_wait(app).await
}

async fn stop_runtime_and_wait(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<DesktopRuntimeState>();
    let terminated = match state.begin_stop() {
        StopPlan::NoProcess => return Ok(()),
        StopPlan::Wait(terminated) => terminated,
        StopPlan::Request {
            pid,
            port,
            shutdown_token,
            terminated,
        } => {
            if let (Some(port), Some(token)) = (port, shutdown_token) {
                if let Err(error) = request_graceful_shutdown(port, &token).await {
                    eprintln!(
                        "[Rua Desktop] graceful Runtime shutdown request failed for pid {pid}: {error}"
                    );
                    state.tracker.mark_shutdown_escalated();
                    state.force_kill();
                }
            } else {
                state.tracker.mark_shutdown_escalated();
                state.force_kill();
            }
            terminated
        }
    };

    match wait_for_termination(terminated.clone(), GRACEFUL_EXIT_TIMEOUT).await {
        Ok(()) => {
            state.finish_stop();
            Ok(())
        }
        Err(graceful_error) => {
            state.tracker.mark_shutdown_escalated();
            state.force_kill();
            match wait_for_termination(terminated, FORCED_EXIT_TIMEOUT).await {
                Ok(()) => {
                    state.finish_stop();
                    Err(format!("{graceful_error}; Runtime was force-terminated"))
                }
                Err(force_error) => {
                    state.finish_stop();
                    Err(format!(
                        "{graceful_error}; failed to confirm forced termination: {force_error}"
                    ))
                }
            }
        }
    }
}

#[derive(Clone)]
struct RuntimeLaunch {
    repo_root: PathBuf,
    binary: PathBuf,
    rua_root: PathBuf,
    console_static_dir: PathBuf,
    endpoint_file: PathBuf,
    owner_file: PathBuf,
}

impl RuntimeLaunch {
    fn resolve(app: &AppHandle) -> Result<Self, String> {
        #[cfg(debug_assertions)]
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        #[cfg(debug_assertions)]
        let _ = app;

        #[cfg(debug_assertions)]
        let binary = std::env::var_os("RUA_DESKTOP_RUNTIME_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                repo_root
                    .join("target")
                    .join("debug")
                    .join(if cfg!(windows) { "rua.exe" } else { "rua" })
            });
        #[cfg(not(debug_assertions))]
        let binary = packaged_rua_binary()?;

        #[cfg(not(debug_assertions))]
        let repo_root = binary
            .parent()
            .ok_or_else(|| "packaged Runtime has no parent directory".to_string())?
            .to_path_buf();

        #[cfg(debug_assertions)]
        let console_static_dir = std::env::var_os("RUA_DESKTOP_CONSOLE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo_root.join("console").join("dist"));
        #[cfg(not(debug_assertions))]
        let console_static_dir = app
            .path()
            .resource_dir()
            .map_err(|error| format!("failed to resolve application resources: {error}"))?
            .join("console");

        let rua_root = configured_rua_root(app)?;

        Ok(Self {
            repo_root,
            binary,
            console_static_dir,
            endpoint_file: rua_root.join("desktop-endpoint.json"),
            owner_file: rua_root.join("desktop-owner.json"),
            rua_root,
        })
    }

    fn args(&self) -> Vec<String> {
        vec![
            "server".to_string(),
            "start".to_string(),
            "--profile".to_string(),
            "desktop".to_string(),
            "--host".to_string(),
            "127.0.0.1".to_string(),
            "--port".to_string(),
            "0".to_string(),
            "--rua-root".to_string(),
            self.rua_root.to_string_lossy().into_owned(),
            "--console-static-dir".to_string(),
            self.console_static_dir.to_string_lossy().into_owned(),
            "--desktop-endpoint-file".to_string(),
            self.endpoint_file.to_string_lossy().into_owned(),
        ]
    }

    fn startup_log_path(&self) -> PathBuf {
        self.rua_root.join("logs").join("startup.log")
    }
}

pub fn configured_rua_root(app: &AppHandle) -> Result<PathBuf, String> {
    preferences::configured_rua_root(app)
}

#[cfg(not(debug_assertions))]
pub(crate) fn packaged_rua_binary() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("failed to resolve Desktop Shell executable: {error}"))?;
    packaged_binary_next_to(&executable)
        .ok_or_else(|| "Desktop Shell executable has no parent directory".to_string())
}

fn bundled_browser_path(runtime_binary: &Path) -> PathBuf {
    bundled_driver_path(runtime_binary, "agent-browser")
}

pub(crate) fn bundled_driver_path(runtime_binary: &Path, name: &str) -> PathBuf {
    #[cfg(debug_assertions)]
    {
        let target = crate::bundle::TARGET;
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        let staged = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("{name}-{target}{suffix}"));
        if staged.is_file() {
            return staged;
        }
    }
    runtime_binary.with_file_name(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    })
}

#[doc(hidden)]
pub fn packaged_binary_next_to(desktop_executable: &Path) -> Option<PathBuf> {
    desktop_executable
        .parent()
        .map(|parent| parent.join(if cfg!(windows) { "rua.exe" } else { "rua" }))
}

async fn reconcile_orphan(launch: &RuntimeLaunch) -> Result<(), String> {
    let Some(record) = owner::read(&launch.owner_file) else {
        return Ok(());
    };
    let live_shell = owner::capture_process_identity(record.shell.pid);
    let live_runtime = owner::capture_process_identity(record.runtime.pid);
    match owner::reconcile_decision(&record, live_shell.as_ref(), live_runtime.as_ref()) {
        ReconcileDecision::RemoveStaleRecord => {
            owner::remove(&launch.owner_file);
            Ok(())
        }
        ReconcileDecision::PreserveActiveOwner => {
            Err("another Rua Desktop Shell still owns the configured Runtime".to_string())
        }
        ReconcileDecision::StopOrphan => {
            let _ = request_graceful_shutdown(record.port, &record.shutdown_token).await;
            if wait_for_identity_exit(&record.runtime, ORPHAN_GRACEFUL_EXIT_TIMEOUT).await {
                owner::remove(&launch.owner_file);
                return Ok(());
            }

            if !owner::terminate_if_identity_matches(&record.runtime) {
                owner::remove(&launch.owner_file);
                return Ok(());
            }
            if !wait_for_identity_exit(&record.runtime, FORCED_EXIT_TIMEOUT).await {
                return Err(format!(
                    "failed to stop orphaned Runtime pid {}",
                    record.runtime.pid
                ));
            }
            owner::remove(&launch.owner_file);
            Ok(())
        }
    }
}

async fn request_graceful_shutdown(port: u16, token: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(SHUTDOWN_REQUEST_TIMEOUT)
        .build()
        .map_err(|error| format!("failed to create shutdown client: {error}"))?;
    let response = client
        .post(format!("http://127.0.0.1:{port}{DESKTOP_SHUTDOWN_PATH}"))
        .header(DESKTOP_SHUTDOWN_TOKEN_HEADER, token)
        .send()
        .await
        .map_err(|error| format!("shutdown request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "shutdown endpoint returned HTTP {}",
            response.status()
        ));
    }
    Ok(())
}

#[doc(hidden)]
pub async fn wait_for_termination(
    mut terminated: watch::Receiver<bool>,
    timeout: Duration,
) -> Result<(), String> {
    if *terminated.borrow() {
        return Ok(());
    }
    match tokio::time::timeout(timeout, async {
        while !*terminated.borrow() {
            terminated
                .changed()
                .await
                .map_err(|_| "Runtime event stream closed".to_string())?;
        }
        Ok::<(), String>(())
    })
    .await
    {
        Ok(result) => result,
        Err(_) => Err(format!(
            "timed out after {} seconds waiting for Runtime exit",
            timeout.as_secs()
        )),
    }
}

async fn wait_for_identity_exit(identity: &ProcessIdentity, timeout: Duration) -> bool {
    let started = tokio::time::Instant::now();
    while started.elapsed() < timeout {
        let Some(actual) = owner::capture_process_identity(identity.pid) else {
            return true;
        };
        if !owner::identity_matches(identity, &actual) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn capture_identity_with_retry(pid: u32) -> Option<ProcessIdentity> {
    for _ in 0..20 {
        if let Some(identity) = owner::capture_process_identity(pid) {
            return Some(identity);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    None
}

fn watch_process(
    app: AppHandle,
    generation: u64,
    mut receiver: tauri::async_runtime::Receiver<CommandEvent>,
    terminated: watch::Sender<bool>,
    startup_log_path: PathBuf,
) {
    tauri::async_runtime::spawn(async move {
        let mut stderr = String::new();
        let mut saw_termination = false;

        while let Some(event) = receiver.recv().await {
            match event {
                CommandEvent::Stdout(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    if let Some(endpoint) = ready_endpoint_from_stdout(&text) {
                        let state = app.state::<DesktopRuntimeState>();
                        if state
                            .tracker
                            .mark_ready_if_current(generation, endpoint.port)
                        {
                            if let Err(error) =
                                state.persist_owner_if_current(generation, endpoint.port)
                            {
                                state.tracker.mark_error_if_current(generation, error);
                            }
                        }
                    }
                }
                CommandEvent::Stderr(bytes) => {
                    stderr.push_str(&String::from_utf8_lossy(&bytes));
                    trim_stderr(&mut stderr);
                }
                CommandEvent::Error(message) => {
                    app.state::<DesktopRuntimeState>()
                        .tracker
                        .mark_error_if_current(
                            generation,
                            format!("Desktop Runtime process error: {message}"),
                        );
                }
                CommandEvent::Terminated(payload) => {
                    saw_termination = true;
                    terminated.send_replace(true);
                    let state = app.state::<DesktopRuntimeState>();
                    if !state.is_stopping(generation) {
                        state.tracker.mark_terminated_if_current(
                            generation,
                            termination_message(payload, &stderr, &startup_log_path),
                        );
                    }
                    state.clear_child_if_current(generation);
                    break;
                }
                _ => {}
            }
        }

        if !saw_termination {
            terminated.send_replace(true);
            let state = app.state::<DesktopRuntimeState>();
            if !state.is_stopping(generation) {
                state.tracker.mark_terminated_if_current(
                    generation,
                    format!(
                        "Desktop Runtime event stream closed before termination; startup log: {}",
                        startup_log_path.display()
                    ),
                );
            }
            state.clear_child_if_current(generation);
        }
    });
}

fn startup_error(
    state: &tauri::State<'_, DesktopRuntimeState>,
    generation: u64,
    message: String,
) -> String {
    state
        .tracker
        .mark_error_if_current(generation, message.clone());
    message
}

fn termination_message(
    payload: TerminatedPayload,
    stderr: &str,
    startup_log_path: &Path,
) -> String {
    let mut message = match (payload.code, payload.signal) {
        (Some(code), _) => format!("Desktop Runtime exited unexpectedly with code {code}"),
        (_, Some(signal)) => format!("Desktop Runtime exited unexpectedly by signal {signal}"),
        _ => "Desktop Runtime exited unexpectedly".to_string(),
    };
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        message.push_str("\n\nLast stderr:\n");
        message.push_str(stderr);
    }
    message.push_str("\n\nStartup log: ");
    message.push_str(&startup_log_path.display().to_string());
    message
}

fn trim_stderr(stderr: &mut String) {
    let count = stderr.chars().count();
    if count <= MAX_CAPTURED_STDERR_CHARS {
        return;
    }
    *stderr = stderr
        .chars()
        .skip(count - MAX_CAPTURED_STDERR_CHARS)
        .collect();
}
