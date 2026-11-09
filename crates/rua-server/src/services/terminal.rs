use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::env;
#[cfg(target_os = "android")]
use std::ffi::CStr;
use std::fmt;
use std::io::{Read, Write};
#[cfg(target_os = "android")]
use std::os::fd::{FromRawFd, OwnedFd};
#[cfg(target_os = "android")]
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(target_os = "android")]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
#[cfg(target_os = "android")]
use std::process::{Child as StdChild, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
#[cfg(not(target_os = "android"))]
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchRequest, ToolchainIntent,
};
use rua_core::runtime_layout::{
    NativeShellKind, NativeShellSpec, ProcessLaunchPlan, RuntimeLayout,
};
use rua_core::RunControl;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio_util::sync::CancellationToken;

use crate::services::active_stream_broadcast::{
    stream_active_broadcast_via_session, ActiveStreamBroadcastFrame, ActiveStreamBroadcastHub,
    ActiveStreamBroadcastSubscribeRequest, ActiveStreamBroadcastViewer, ActiveStreamOwner,
};
use crate::services::device_command_bus::{
    deliver_terminal_device_command_via_session, DeviceCommandDeliveryError, TerminalDeviceAction,
    TerminalDeviceCommand, TerminalDeviceResponse,
};
use crate::services::foreground_operation::ForegroundOperationKind;

const TERMINAL_TERM: &str = "xterm-256color";
const TERMINAL_COLORTERM: &str = "truecolor";
const TERMINAL_REPLAY_MAX_BYTES: usize = 16 * 1024 * 1024;
const TERMINAL_OUTPUT_FRAME_MAX_BYTES: usize = 64 * 1024;

const TERMINAL_OUTPUT_COALESCE_MAX_BYTES: usize = TERMINAL_OUTPUT_FRAME_MAX_BYTES;
const TERMINAL_REMOTE_ACTIVITY_RUNTIME_RETRY_DELAY: Duration = Duration::from_millis(100);
pub const TERMINAL_SCROLLBACK_MIN_LINES: usize = 100;
pub const TERMINAL_SCROLLBACK_MAX_LINES: usize = 100_000;

pub fn terminal_activity_broadcast_after_seq(cursor: TerminalStreamCursor) -> u64 {
    cursor.next_activity_sequence
}

fn terminal_activity_stream_id(session_id: &str) -> String {
    format!("terminal-activity:{session_id}")
}

pub fn terminal_activity_owner(session_id: &str) -> ActiveStreamOwner {
    ActiveStreamOwner::terminal(session_id)
}

fn terminal_utf8_locale() -> String {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .filter_map(|key| env::var(key).ok())
        .find(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("utf-8") || lower.contains("utf8")
        })
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "en_US.UTF-8".to_string()
            } else {
                "C.UTF-8".to_string()
            }
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TerminalViewerIdentity {
    device_id: Option<String>,
    browser_id: String,
}

impl TerminalViewerIdentity {
    pub fn local(browser_id: impl Into<String>) -> Self {
        Self {
            device_id: None,
            browser_id: browser_id.into(),
        }
    }

    pub fn paired(device_id: impl Into<String>, browser_id: impl Into<String>) -> Self {
        Self {
            device_id: Some(device_id.into()),
            browser_id: browser_id.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TerminalAuthorityConfig {
    pub default_home: PathBuf,
    pub detached_ttl: Duration,
}

#[derive(Debug, Clone)]
struct XtermStateBridge {
    tx: Option<mpsc::UnboundedSender<XtermStateCommand>>,
    shutdown: CancellationToken,
    task: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

enum XtermStateCommand {
    Create {
        session_id: String,
        geometry: TerminalPtyGeometry,
        scrollback: usize,
    },
    Write {
        session_id: String,
        data: Vec<u8>,
        control: Arc<dyn TerminalProcessControl>,
    },
    Resize {
        session_id: String,
        geometry: TerminalPtyGeometry,
    },
    SetScrollback {
        session_id: String,
        scrollback: usize,
    },
    SetPalette {
        session_id: String,
        palette: TerminalPalette,
        control: Arc<dyn TerminalProcessControl>,
    },
    Serialize {
        session_id: String,
        reply: oneshot::Sender<Option<String>>,
    },
    Dispose {
        session_id: String,
    },
}

struct XtermStateCommandEnvelope {
    payload: serde_json::Value,
    snapshot_reply: Option<oneshot::Sender<Option<String>>>,
    terminal_reply_control: Option<Arc<dyn TerminalProcessControl>>,
}

impl XtermStateBridge {
    fn new() -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let shutdown = CancellationToken::new();
        let cancelled = shutdown.clone();
        let task = tokio::spawn(async move {
            loop {
                let first = tokio::select! {
                    biased;
                    _ = cancelled.cancelled() => return,
                    command = rx.recv() => match command { Some(command) => command, None => return },
                };
                // Installation may finish after the resident starts. Resolve on demand.
                if let Some(launch) = xterm_headless_bridge_launch() {
                    run_xterm_state_bridge(launch, &mut rx, first, &cancelled).await;
                }
            }
        });
        Self {
            tx: Some(tx),
            shutdown,
            task: Arc::new(tokio::sync::Mutex::new(Some(task))),
        }
    }

    async fn shutdown(&self) {
        self.shutdown.cancel();
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }

    fn create(&self, session_id: String, geometry: TerminalPtyGeometry, scrollback: usize) {
        let Some(tx) = &self.tx else { return };
        let _ = tx.send(XtermStateCommand::Create {
            session_id,
            geometry,
            scrollback,
        });
    }

    fn write(&self, session_id: String, data: Vec<u8>, control: Arc<dyn TerminalProcessControl>) {
        let Some(tx) = &self.tx else { return };
        let _ = tx.send(XtermStateCommand::Write {
            session_id,
            data,
            control,
        });
    }

    fn resize(&self, session_id: String, geometry: TerminalPtyGeometry) {
        let Some(tx) = &self.tx else { return };
        let _ = tx.send(XtermStateCommand::Resize {
            session_id,
            geometry,
        });
    }

    fn set_scrollback(&self, session_id: String, scrollback: usize) {
        let Some(tx) = &self.tx else { return };
        let _ = tx.send(XtermStateCommand::SetScrollback {
            session_id,
            scrollback,
        });
    }

    fn set_palette(
        &self,
        session_id: String,
        palette: TerminalPalette,
        control: Arc<dyn TerminalProcessControl>,
    ) {
        let Some(tx) = &self.tx else { return };
        let _ = tx.send(XtermStateCommand::SetPalette {
            session_id,
            palette,
            control,
        });
    }

    fn begin_serialize(&self, session_id: &str) -> Option<oneshot::Receiver<Option<String>>> {
        let Some(tx) = &self.tx else { return None };
        let (reply, rx) = oneshot::channel();
        let _ = tx.send(XtermStateCommand::Serialize {
            session_id: session_id.to_string(),
            reply,
        });
        Some(rx)
    }

    fn dispose(&self, session_id: String) {
        let Some(tx) = &self.tx else { return };
        let _ = tx.send(XtermStateCommand::Dispose { session_id });
    }
}

fn xterm_headless_bridge_launch() -> Option<ProcessLaunchPlan> {
    let layout = RuntimeLayout::from_current_process(None);
    if layout.host_profile.is_mobile() || layout.os_family == "android" {
        let linux = layout.linux_compat?;
        let bridge = PathBuf::from(env::var_os("RUA_XTERM_BRIDGE")?);
        if !linux.is_available() || !bridge.is_file() {
            tracing::warn!(
                path = %bridge.display(),
                "Android xterm headless bridge unavailable"
            );
            return None;
        }
        let workspace_dir = env::current_dir().unwrap_or_else(|_| env::temp_dir());
        let authority = RuntimeAuthority::from_current_process(
            Some("mobile-apk"),
            workspace_dir,
            env::temp_dir().join("rua-runtime-cache"),
        );
        let plan = authority.plan(
            ProcessShape::Stdio,
            ToolchainIntent::Node,
            RuntimeLaunchRequest {
                args: vec![bridge.to_string_lossy().into_owned()],
                ..Default::default()
            },
        );
        if !plan.diagnostics.is_empty() {
            tracing::warn!(
                diagnostics = %plan.diagnostics.join("; "),
                "Android xterm headless bridge launch unavailable"
            );
            return None;
        }
        return plan.into_process().ok();
    }

    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../console/scripts/xterm-headless-bridge.mjs")
        .components()
        .collect::<PathBuf>();
    script.is_file().then(|| ProcessLaunchPlan {
        command: PathBuf::from("node"),
        args: vec![script.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        cwd: None,
    })
}

async fn run_xterm_state_bridge(
    launch: ProcessLaunchPlan,
    rx: &mut mpsc::UnboundedReceiver<XtermStateCommand>,
    first: XtermStateCommand,
    shutdown: &CancellationToken,
) {
    let mut command = TokioCommand::new(&launch.command);
    if RuntimeLayout::from_current_process(None)
        .host_profile
        .is_mobile()
        || cfg!(target_os = "android")
    {
        command.env_clear();
    }
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command
        .args(&launch.args)
        .envs(&launch.env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    if let Some(cwd) = &launch.cwd {
        command.current_dir(cwd);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(
                command = %launch.command.display(),
                error = %error,
                "xterm headless bridge unavailable"
            );
            return;
        }
    };
    let group = XtermProcessGroup(child.id());
    let Some(mut stdin) = child.stdin.take() else {
        drop(group);
        let _ = child.kill().await;
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        drop(group);
        let _ = child.kill().await;
        return;
    };
    let mut lines = BufReader::new(stdout).lines();
    let mut command_id = 0_u64;

    let work = async {
        let mut pending = Some(first);
        let mut live_sessions = HashSet::new();
        while let Some(command) = match pending.take() {
            Some(command) => Some(command),
            None => rx.recv().await,
        } {
            match &command {
                XtermStateCommand::Create { session_id, .. } => {
                    live_sessions.insert(session_id.clone());
                }
                XtermStateCommand::Dispose { session_id } => {
                    live_sessions.remove(session_id);
                }
                _ => {}
            }
            command_id = command_id.saturating_add(1);
            let envelope = xterm_state_command_payload(command_id, command);
            let Ok(line) = serde_json::to_string(&envelope.payload) else {
                if let Some(reply) = envelope.snapshot_reply {
                    let _ = reply.send(None);
                }
                continue;
            };
            if stdin.write_all(line.as_bytes()).await.is_err()
                || stdin.write_all(b"\n").await.is_err()
                || stdin.flush().await.is_err()
            {
                if let Some(reply) = envelope.snapshot_reply {
                    let _ = reply.send(None);
                }
                break;
            }
            let response = match lines.next_line().await {
                Ok(Some(line)) => serde_json::from_str::<serde_json::Value>(&line).ok(),
                _ => None,
            };
            if let (Some(response), Some(control)) =
                (response.as_ref(), envelope.terminal_reply_control.as_ref())
            {
                if response.get("ok").and_then(|value| value.as_bool()) == Some(true) {
                    for encoded in response
                        .get("replies")
                        .and_then(|value| value.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(|value| value.as_str())
                    {
                        if let Ok(data) = BASE64_STANDARD.decode(encoded) {
                            if let Err(error) = control.write_input(&data) {
                                tracing::warn!(
                                    error = %error,
                                    reply_bytes = data.len(),
                                    "failed to write authority terminal reply"
                                );
                            }
                        }
                    }
                }
            }
            if let Some(reply) = envelope.snapshot_reply {
                let snapshot = response
                    .filter(|value| value.get("ok").and_then(|value| value.as_bool()) == Some(true))
                    .and_then(|value| {
                        value
                            .get("snapshot")
                            .and_then(|snapshot| snapshot.as_str())
                            .map(str::to_string)
                    });
                let _ = reply.send(snapshot);
            }
            // Release the guest lease when the final terminal is closed. The outer
            // loop resolves and starts a fresh bridge for the next queued session.
            if live_sessions.is_empty() {
                break;
            }
        }
    };
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => {},
        _ = work => {},
    }
    drop(group);
    let _ = child.kill().await;
    let _ = child.wait().await;
}

struct XtermProcessGroup(Option<u32>);

impl Drop for XtermProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            unsafe {
                libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        let _ = self.0;
    }
}

fn xterm_state_command_payload(id: u64, command: XtermStateCommand) -> XtermStateCommandEnvelope {
    match command {
        XtermStateCommand::Create {
            session_id,
            geometry,
            scrollback,
        } => XtermStateCommandEnvelope {
            payload: serde_json::json!({
                "id": id,
                "op": "create",
                "sessionId": session_id,
                "cols": geometry.cols,
                "rows": geometry.rows,
                "scrollback": scrollback,
            }),
            snapshot_reply: None,
            terminal_reply_control: None,
        },
        XtermStateCommand::Write {
            session_id,
            data,
            control,
        } => XtermStateCommandEnvelope {
            payload: serde_json::json!({
                "id": id,
                "op": "write",
                "sessionId": session_id,
                "data": BASE64_STANDARD.encode(data),
            }),
            snapshot_reply: None,
            terminal_reply_control: Some(control),
        },
        XtermStateCommand::Resize {
            session_id,
            geometry,
        } => XtermStateCommandEnvelope {
            payload: serde_json::json!({
                "id": id,
                "op": "resize",
                "sessionId": session_id,
                "cols": geometry.cols,
                "rows": geometry.rows,
            }),
            snapshot_reply: None,
            terminal_reply_control: None,
        },
        XtermStateCommand::SetScrollback {
            session_id,
            scrollback,
        } => XtermStateCommandEnvelope {
            payload: serde_json::json!({
                "id": id,
                "op": "set_scrollback",
                "sessionId": session_id,
                "scrollback": scrollback,
            }),
            snapshot_reply: None,
            terminal_reply_control: None,
        },
        XtermStateCommand::SetPalette {
            session_id,
            palette,
            control,
        } => XtermStateCommandEnvelope {
            payload: serde_json::json!({
                "id": id,
                "op": "set_palette",
                "sessionId": session_id,
                "foreground": palette.foreground,
                "background": palette.background,
                "cursor": palette.cursor,
            }),
            snapshot_reply: None,
            terminal_reply_control: Some(control),
        },
        XtermStateCommand::Serialize { session_id, reply } => XtermStateCommandEnvelope {
            payload: serde_json::json!({
                "id": id,
                "op": "serialize",
                "sessionId": session_id,
            }),
            snapshot_reply: Some(reply),
            terminal_reply_control: None,
        },
        XtermStateCommand::Dispose { session_id } => XtermStateCommandEnvelope {
            payload: serde_json::json!({
                "id": id,
                "op": "dispose",
                "sessionId": session_id,
            }),
            snapshot_reply: None,
            terminal_reply_control: None,
        },
    }
}

#[derive(Debug, Clone)]
pub struct OpenTerminalSessionRequest {
    pub viewer: TerminalViewerIdentity,
    pub cwd: Option<PathBuf>,
    pub terminal_profile_id: Option<String>,
    pub scrollback_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalExecutionDevice {
    Local,
    Remote { device_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalPlacementRoute {
    Local,
    MissingSourceDevice {
        target_device_id: String,
    },
    Remote {
        source_device_id: String,
        target_device_id: String,
    },
}

pub fn route_terminal_execution_device(
    execution_device: TerminalExecutionDevice,
    local_device_id: Option<&str>,
) -> TerminalPlacementRoute {
    match execution_device {
        TerminalExecutionDevice::Local => TerminalPlacementRoute::Local,
        TerminalExecutionDevice::Remote { device_id } => {
            if local_device_id == Some(device_id.as_str()) {
                TerminalPlacementRoute::Local
            } else if let Some(source_device_id) = local_device_id {
                TerminalPlacementRoute::Remote {
                    source_device_id: source_device_id.to_string(),
                    target_device_id: device_id,
                }
            } else {
                TerminalPlacementRoute::MissingSourceDevice {
                    target_device_id: device_id,
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TerminalSpawnRequest {
    pub cwd: PathBuf,
    pub terminal_profile_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProfile {
    pub id: String,
    pub label: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProfileList {
    pub os: String,
    pub default_profile_id: String,
    pub profiles: Vec<TerminalProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPtyGeometry {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPalette {
    pub foreground: String,
    pub background: String,
    pub cursor: String,
}

impl Default for TerminalPalette {
    fn default() -> Self {
        Self {
            foreground: "#e9ecef".to_string(),
            background: "#242424".to_string(),
            cursor: "#e9ecef".to_string(),
        }
    }
}

impl TerminalPalette {
    fn validate(&self) -> Result<(), TerminalError> {
        for (name, value) in [
            ("foreground", &self.foreground),
            ("background", &self.background),
            ("cursor", &self.cursor),
        ] {
            if value.len() != 7
                || !value.starts_with('#')
                || !value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(TerminalError::Spawn(format!(
                    "terminal palette {name} must be #RRGGBB"
                )));
            }
        }
        Ok(())
    }
}

impl Default for TerminalPtyGeometry {
    fn default() -> Self {
        Self { cols: 80, rows: 24 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTerminalSpawnRequest {
    pub cwd: PathBuf,
    pub shell: NativeShellSpec,
    pub env: BTreeMap<String, String>,
}

pub struct TerminalProcessHandle {
    pub output: mpsc::UnboundedReceiver<Vec<u8>>,
    pub control: Arc<dyn TerminalProcessControl>,
}

pub trait TerminalProcessControl: Send + Sync {
    fn kill(&self);
    fn write_input(&self, data: &[u8]) -> Result<(), TerminalError>;
    fn resize_pty(&self, _geometry: TerminalPtyGeometry) -> Result<(), TerminalError> {
        Ok(())
    }
}

pub trait TerminalProcessSpawner: Send + Sync {
    fn list_profiles(&self) -> TerminalProfileList {
        native_terminal_profiles_for_current_process()
    }

    fn spawn(&self, request: TerminalSpawnRequest) -> Result<TerminalProcessHandle, TerminalError>;
}

#[async_trait]
pub trait TerminalRemoteCommandClient: Send + Sync {
    async fn deliver(
        &self,
        command: TerminalDeviceCommand,
    ) -> Result<TerminalDeviceResponse, TerminalRemoteCommandError>;
}

#[derive(Debug)]
pub enum TerminalRemoteCommandError {
    Unavailable(String),
    Rejected { reason: String, detail: String },
}

impl fmt::Display for TerminalRemoteCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(detail) => write!(f, "{detail}"),
            Self::Rejected { reason, detail } => write!(f, "{reason}: {detail}"),
        }
    }
}

impl std::error::Error for TerminalRemoteCommandError {}

impl From<DeviceCommandDeliveryError> for TerminalRemoteCommandError {
    fn from(error: DeviceCommandDeliveryError) -> Self {
        match error {
            DeviceCommandDeliveryError::Rejected { reason, detail } => {
                Self::Rejected { reason, detail }
            }
            error => Self::Unavailable(error.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRemoteActivityRequest {
    pub source_device_id: String,
    pub target_device_id: String,
    pub session_id: String,
    pub viewer_id: String,
    pub retain_authority: bool,
    pub cursor: TerminalStreamCursor,
}

#[async_trait]
pub trait TerminalRemoteActivityClient: Send + Sync {
    async fn subscribe(
        &self,
        request: TerminalRemoteActivityRequest,
    ) -> Result<mpsc::UnboundedReceiver<TerminalActivityFrame>, String>;
}

#[derive(Debug)]
pub enum TerminalPlacementError {
    MissingSourceDevice { target_device_id: String },
    Unavailable(String),
    NotFound(String),
    CursorUnavailable(String),
    InputLeaseRequired,
    Internal(String),
}

impl fmt::Display for TerminalPlacementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSourceDevice { target_device_id } => write!(
                f,
                "source device identity is unavailable for terminal target {target_device_id}"
            ),
            Self::Unavailable(message) => write!(f, "{message}"),
            Self::NotFound(message) | Self::CursorUnavailable(message) => write!(f, "{message}"),
            Self::InputLeaseRequired => write!(f, "terminal input lease is required"),
            Self::Internal(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for TerminalPlacementError {}

impl From<TerminalRemoteCommandError> for TerminalPlacementError {
    fn from(error: TerminalRemoteCommandError) -> Self {
        match error {
            TerminalRemoteCommandError::Unavailable(detail) => Self::Unavailable(detail),
            TerminalRemoteCommandError::Rejected { reason, detail } => match reason.as_str() {
                "terminal_not_found" => Self::NotFound(detail),
                "terminal_cursor_unavailable" => Self::CursorUnavailable(detail),
                "terminal_input_lease_required" => Self::InputLeaseRequired,
                "terminal_runtime_unavailable" => Self::Unavailable(detail),
                _ => Self::Internal(detail),
            },
        }
    }
}

impl From<TerminalError> for TerminalPlacementError {
    fn from(error: TerminalError) -> Self {
        match error {
            TerminalError::InputLeaseRequired => Self::InputLeaseRequired,
            TerminalError::NotFound(_) => Self::NotFound(error.to_string()),
            TerminalError::CursorUnavailable { .. } => Self::CursorUnavailable(error.to_string()),
            TerminalError::Spawn(_) | TerminalError::SnapshotUnavailable(_) => {
                Self::Internal(error.to_string())
            }
        }
    }
}

#[derive(Clone)]
pub struct TerminalPlacementService {
    authority: Arc<TerminalSessionAuthority>,
    remote_command_client: Arc<dyn TerminalRemoteCommandClient>,
    remote_activity_client: Arc<dyn TerminalRemoteActivityClient>,
}

impl TerminalPlacementService {
    pub fn new(
        authority: Arc<TerminalSessionAuthority>,
        remote_command_client: Arc<dyn TerminalRemoteCommandClient>,
        remote_activity_client: Arc<dyn TerminalRemoteActivityClient>,
    ) -> Self {
        Self {
            authority,
            remote_command_client,
            remote_activity_client,
        }
    }

    pub async fn list_sessions(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
    ) -> Result<Vec<TerminalSessionSnapshot>, TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => Ok(self.authority.list().await),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::ListSessions,
                })
                .await?
            {
                TerminalDeviceResponse::Sessions { sessions } => Ok(sessions),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn list_profiles(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
    ) -> Result<TerminalProfileList, TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => Ok(self.authority.list_profiles()),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::ListProfiles,
                })
                .await?
            {
                TerminalDeviceResponse::Profiles { profiles } => Ok(profiles),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn open_session(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
        browser_viewer_id: String,
        cwd: Option<String>,
        terminal_profile_id: Option<String>,
        scrollback_lines: usize,
    ) -> Result<TerminalSessionSnapshot, TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => self
                .authority
                .open(OpenTerminalSessionRequest {
                    viewer: TerminalViewerIdentity::local(browser_viewer_id),
                    cwd: cwd.map(std::path::PathBuf::from),
                    terminal_profile_id,
                    scrollback_lines,
                })
                .await
                .map_err(Into::into),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::Open {
                        browser_viewer_id,
                        cwd,
                        terminal_profile_id,
                        scrollback_lines,
                    },
                })
                .await?
            {
                TerminalDeviceResponse::Opened { session } => Ok(session),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn attach_session(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
        session_id: &str,
        browser_viewer_id: String,
        scrollback_lines: usize,
        cursor: Option<TerminalStreamCursor>,
    ) -> Result<TerminalSessionAttachment, TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => self
                .authority
                .attach_with_scrollback(
                    session_id,
                    TerminalViewerIdentity::local(browser_viewer_id),
                    scrollback_lines,
                    cursor,
                )
                .await
                .map_err(Into::into),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::Attach {
                        session_id: session_id.to_string(),
                        browser_viewer_id,
                        scrollback_lines,
                        cursor,
                    },
                })
                .await?
            {
                TerminalDeviceResponse::Attached {
                    session,
                    snapshot,
                    tail,
                    live_cursor,
                } => Ok(TerminalSessionAttachment {
                    session,
                    snapshot,
                    tail,
                    live_cursor,
                }),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn set_scrollback_lines(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
        session_id: &str,
        scrollback_lines: usize,
    ) -> Result<(), TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => self
                .authority
                .set_scrollback_lines(session_id, scrollback_lines)
                .await
                .map_err(Into::into),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::SetScrollback {
                        session_id: session_id.to_string(),
                        scrollback_lines,
                    },
                })
                .await?
            {
                TerminalDeviceResponse::ScrollbackUpdated { .. } => Ok(()),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn detach_session(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
        session_id: &str,
        browser_viewer_id: String,
    ) -> Result<(), TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => self
                .authority
                .detach(
                    session_id,
                    &TerminalViewerIdentity::local(browser_viewer_id),
                )
                .await
                .map_err(Into::into),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::DetachViewer {
                        session_id: session_id.to_string(),
                        browser_viewer_id,
                    },
                })
                .await?
            {
                TerminalDeviceResponse::ViewerDetached { .. } => Ok(()),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn rename_session(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
        session_id: &str,
        display_name: Option<String>,
    ) -> Result<TerminalSessionSnapshot, TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => self
                .authority
                .rename(session_id, display_name)
                .await
                .map_err(Into::into),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::Rename {
                        session_id: session_id.to_string(),
                        display_name,
                    },
                })
                .await?
            {
                TerminalDeviceResponse::Renamed { session } => Ok(session),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn take_input_lease(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
        session_id: &str,
        browser_viewer_id: String,
    ) -> Result<TerminalSessionSnapshot, TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => self
                .authority
                .take_input_lease(session_id, TerminalViewerIdentity::local(browser_viewer_id))
                .await
                .map_err(Into::into),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::TakeInputLease {
                        session_id: session_id.to_string(),
                        browser_viewer_id,
                    },
                })
                .await?
            {
                TerminalDeviceResponse::InputLeaseTaken { session } => Ok(session),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn send_input(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
        session_id: &str,
        browser_viewer_id: String,
        data: Vec<u8>,
    ) -> Result<(), TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => self
                .authority
                .send_input(
                    session_id,
                    &TerminalViewerIdentity::local(browser_viewer_id),
                    &data,
                )
                .await
                .map_err(Into::into),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::SendInput {
                        session_id: session_id.to_string(),
                        browser_viewer_id,
                        data,
                    },
                })
                .await?
            {
                TerminalDeviceResponse::InputAccepted { .. } => Ok(()),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn adapt_pty_geometry(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
        session_id: &str,
        browser_viewer_id: String,
        geometry: TerminalPtyGeometry,
    ) -> Result<TerminalSessionSnapshot, TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => self
                .authority
                .adapt_pty_geometry(
                    session_id,
                    &TerminalViewerIdentity::local(browser_viewer_id),
                    geometry,
                )
                .await
                .map_err(Into::into),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::AdaptPtyGeometry {
                        session_id: session_id.to_string(),
                        browser_viewer_id,
                        cols: geometry.cols,
                        rows: geometry.rows,
                    },
                })
                .await?
            {
                TerminalDeviceResponse::PtyGeometryAdapted { session } => Ok(session),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn update_palette(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
        session_id: &str,
        browser_viewer_id: String,
        palette: TerminalPalette,
    ) -> Result<(), TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => self
                .authority
                .update_palette(
                    session_id,
                    &TerminalViewerIdentity::local(browser_viewer_id),
                    palette,
                )
                .await
                .map_err(Into::into),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::UpdatePalette {
                        session_id: session_id.to_string(),
                        browser_viewer_id,
                        palette,
                    },
                })
                .await?
            {
                TerminalDeviceResponse::PaletteUpdated { .. } => Ok(()),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn kill_session(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
        session_id: &str,
        browser_viewer_id: String,
    ) -> Result<(), TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => self
                .authority
                .kill(
                    session_id,
                    &TerminalViewerIdentity::local(browser_viewer_id),
                )
                .await
                .map_err(Into::into),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => match self
                .deliver_remote(TerminalDeviceCommand {
                    command_id: Self::command_id(),
                    source_device_id,
                    target_device_id,
                    action: TerminalDeviceAction::KillSession {
                        session_id: session_id.to_string(),
                        browser_viewer_id,
                    },
                })
                .await?
            {
                TerminalDeviceResponse::SessionKilled { .. } => Ok(()),
                _ => Err(TerminalPlacementError::Internal(
                    "unexpected terminal response".to_string(),
                )),
            },
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    pub async fn subscribe_activity(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
        session_id: &str,
        viewer_id: &str,
        retain_authority: bool,
        cursor: TerminalStreamCursor,
    ) -> Result<mpsc::UnboundedReceiver<TerminalActivityFrame>, TerminalPlacementError> {
        match self.route(execution_device, local_device_id)? {
            TerminalPlacementRoute::Local => self
                .authority
                .subscribe_activity(session_id, cursor)
                .await
                .map_err(Into::into),
            TerminalPlacementRoute::Remote {
                source_device_id,
                target_device_id,
            } => self
                .remote_activity_client
                .subscribe(TerminalRemoteActivityRequest {
                    source_device_id,
                    target_device_id,
                    session_id: session_id.to_string(),
                    viewer_id: viewer_id.to_string(),
                    retain_authority,
                    cursor,
                })
                .await
                .map_err(TerminalPlacementError::Unavailable),
            TerminalPlacementRoute::MissingSourceDevice { .. } => unreachable!(),
        }
    }

    fn route(
        &self,
        execution_device: TerminalExecutionDevice,
        local_device_id: Option<&str>,
    ) -> Result<TerminalPlacementRoute, TerminalPlacementError> {
        match route_terminal_execution_device(execution_device, local_device_id) {
            TerminalPlacementRoute::MissingSourceDevice { target_device_id } => {
                Err(TerminalPlacementError::MissingSourceDevice { target_device_id })
            }
            route => Ok(route),
        }
    }

    async fn deliver_remote(
        &self,
        command: TerminalDeviceCommand,
    ) -> Result<TerminalDeviceResponse, TerminalPlacementError> {
        self.remote_command_client
            .deliver(command)
            .await
            .map_err(Into::into)
    }

    fn command_id() -> String {
        format!("terminal-command-{}", uuid::Uuid::new_v4())
    }
}

#[derive(Clone)]
pub struct DeviceSessionTerminalRemoteCommandClient {
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    timeout: Duration,
}

impl DeviceSessionTerminalRemoteCommandClient {
    pub fn new(
        device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    ) -> Self {
        Self {
            device_network,
            timeout: Duration::from_secs(10),
        }
    }
}

#[async_trait]
impl TerminalRemoteCommandClient for DeviceSessionTerminalRemoteCommandClient {
    async fn deliver(
        &self,
        command: TerminalDeviceCommand,
    ) -> Result<TerminalDeviceResponse, TerminalRemoteCommandError> {
        let deadline = tokio::time::Instant::now() + self.timeout;
        let session_manager = self
            .device_network
            .session_manager(deadline)
            .await
            .map_err(TerminalRemoteCommandError::Unavailable)?;
        deliver_terminal_device_command_via_session(
            &session_manager,
            &command,
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await
        .map_err(Into::into)
    }
}

#[derive(Clone)]
pub struct DeviceSessionTerminalRemoteActivityClient {
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    timeout: Duration,
}

impl DeviceSessionTerminalRemoteActivityClient {
    pub fn new(
        device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    ) -> Self {
        Self {
            device_network,
            timeout: Duration::from_secs(10),
        }
    }
}

#[async_trait]
impl TerminalRemoteActivityClient for DeviceSessionTerminalRemoteActivityClient {
    async fn subscribe(
        &self,
        request: TerminalRemoteActivityRequest,
    ) -> Result<mpsc::UnboundedReceiver<TerminalActivityFrame>, String> {
        let initial_session_manager = self
            .device_network
            .session_manager(tokio::time::Instant::now() + self.timeout)
            .await?;
        let (tx, rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
        let tx_for_close = tx.clone();
        tokio::spawn(async move {
            tx_for_close.closed().await;
            let _ = cancel_tx.send(RunControl::Stop);
        });
        let timeout = self.timeout;
        let device_network = Arc::clone(&self.device_network);
        tokio::spawn(async move {
            let cursor = Arc::new(Mutex::new(request.cursor));
            let mut session_manager = initial_session_manager;
            loop {
                let current_cursor = *cursor.lock().expect("terminal activity cursor lock");
                let cursor_for_frames = Arc::clone(&cursor);
                let tx_for_frames = tx.clone();
                let result = stream_active_broadcast_via_session(
                    &session_manager,
                    &request.target_device_id,
                    ActiveStreamBroadcastSubscribeRequest {
                        device_delivery_id: String::new(),
                        owner: Some(terminal_activity_owner(&request.session_id)),
                        after_seq_by_stream: BTreeMap::from([(
                            terminal_activity_stream_id(&request.session_id),
                            terminal_activity_broadcast_after_seq(current_cursor),
                        )]),
                        activity_generation: None,
                        conversation_activity_position: None,
                        collapse_replay: false,
                        initial_conversation_control: false,
                        authority_epoch: None,
                        viewer: request
                            .retain_authority
                            .then(|| ActiveStreamBroadcastViewer {
                                source_device_id: request.source_device_id.clone(),
                                viewer_id: request.viewer_id.clone(),
                                kind: ForegroundOperationKind::TerminalViewer,
                            }),
                    },
                    timeout,
                    cancel_rx.clone(),
                    move |frame| {
                        let tx = tx_for_frames.clone();
                        let cursor = Arc::clone(&cursor_for_frames);
                        async move {
                            if let Some(activity) =
                                terminal_activity_from_remote_stream_frame(frame)
                            {
                                advance_terminal_stream_cursor(
                                    &mut cursor.lock().expect("terminal activity cursor lock"),
                                    &activity,
                                );
                                let _ = tx.send(activity);
                            }
                        }
                    },
                )
                .await;
                if tx.is_closed() || matches!(&result, Ok(())) {
                    return;
                }
                tracing::debug!(
                    error = %result.expect_err("terminal activity stream result checked as error"),
                    session_id = %request.session_id,
                    target_device_id = %request.target_device_id,
                    "terminal activity stream ended"
                );

                loop {
                    if tx.is_closed() || cancel_rx.borrow().is_terminal() {
                        return;
                    }
                    match crate::services::device_session_access::current_session_manager(
                        &device_network,
                    )
                    .await
                    {
                        Ok(candidate) if !candidate.same_instance(&session_manager) => {
                            session_manager = candidate;
                            break;
                        }
                        Ok(_) => return,
                        Err(_) => {
                            tokio::select! {
                                _ = tx.closed() => return,
                                _ = tokio::time::sleep(
                                    TERMINAL_REMOTE_ACTIVITY_RUNTIME_RETRY_DELAY
                                ) => {}
                            }
                        }
                    }
                }
            }
        });
        Ok(rx)
    }
}

fn advance_terminal_stream_cursor(
    cursor: &mut TerminalStreamCursor,
    frame: &TerminalActivityFrame,
) {
    cursor.next_activity_sequence = cursor
        .next_activity_sequence
        .max(frame.activity_sequence().saturating_add(1));
    if let TerminalActivityFrame::Output { end_offset, .. } = frame {
        cursor.next_output_offset = cursor.next_output_offset.max(*end_offset);
    }
}

#[derive(Debug, Clone, Default)]
pub struct NativeTerminalProcessSpawner;

impl NativeTerminalProcessSpawner {
    pub fn default_spawn_request(
        cwd: PathBuf,
        terminal_profile_id: Option<&str>,
    ) -> Result<NativeTerminalSpawnRequest, TerminalError> {
        native_terminal_spawn_request(cwd, terminal_profile_id)
    }
}

fn profile(
    id: impl Into<String>,
    label: impl Into<String>,
    kind: impl Into<String>,
) -> TerminalProfile {
    TerminalProfile {
        id: id.into(),
        label: label.into(),
        kind: kind.into(),
    }
}

fn native_terminal_profiles_for_current_process() -> TerminalProfileList {
    let layout = RuntimeLayout::from_current_process(None);
    if layout.host_profile.is_mobile() || layout.os_family == "android" {
        return TerminalProfileList {
            os: "android".to_string(),
            default_profile_id: "proot".to_string(),
            profiles: vec![profile("proot", "Debian", "proot")],
        };
    }
    if cfg!(windows) {
        return TerminalProfileList {
            os: "windows".to_string(),
            default_profile_id: "powershell".to_string(),
            profiles: windows_terminal_profiles(env::var_os("PATH").as_deref()),
        };
    }
    let shell = native_interactive_shell_for_current_process();
    TerminalProfileList {
        os: layout.os_family,
        default_profile_id: "default".to_string(),
        profiles: vec![profile(
            "default",
            terminal_profile_label_for_program(&shell.program),
            shell.contract_kind(),
        )],
    }
}

fn windows_terminal_profiles(path: Option<&std::ffi::OsStr>) -> Vec<TerminalProfile> {
    let mut profiles = Vec::new();
    if find_on_path("cmd.exe", path, ';').is_some() || cfg!(windows) {
        profiles.push(profile("cmd", "Command Prompt", "windows_cmd"));
    }
    if find_on_path("powershell.exe", path, ';').is_some() || cfg!(windows) {
        profiles.push(profile(
            "powershell",
            "Windows PowerShell",
            "windows_powershell",
        ));
    }
    if find_on_path("pwsh.exe", path, ';').is_some() {
        profiles.push(profile("pwsh", "PowerShell 7", "windows_pwsh"));
    }
    if find_on_path("bash.exe", path, ';').is_some()
        || standard_windows_git_bash_candidates()
            .iter()
            .any(|candidate| candidate.is_file())
    {
        profiles.push(profile("git-bash", "Git Bash", "git_bash"));
    }
    if find_on_path("wsl.exe", path, ';').is_some() {
        profiles.push(profile("wsl", "WSL", "wsl"));
    }
    if profiles.is_empty() {
        profiles.push(profile("cmd", "Command Prompt", "windows_cmd"));
    }
    profiles
}

fn native_terminal_spawn_request(
    cwd: PathBuf,
    terminal_profile_id: Option<&str>,
) -> Result<NativeTerminalSpawnRequest, TerminalError> {
    let layout = RuntimeLayout::from_current_process(None);
    if layout.host_profile.is_mobile() || layout.os_family == "android" {
        return android_terminal_spawn_request(cwd, terminal_profile_id);
    }
    if cfg!(windows) {
        return Ok(windows_terminal_spawn_request(cwd, terminal_profile_id));
    }
    Ok(NativeTerminalSpawnRequest {
        cwd,
        shell: native_interactive_shell_for_current_process(),
        env: BTreeMap::new(),
    })
}

fn android_terminal_spawn_request(
    cwd: PathBuf,
    terminal_profile_id: Option<&str>,
) -> Result<NativeTerminalSpawnRequest, TerminalError> {
    if terminal_profile_id.unwrap_or("proot") != "proot" {
        return Err(TerminalError::Spawn(
            "Android terminal requires the Debian profile".to_string(),
        ));
    }

    let authority = RuntimeAuthority::from_current_process(
        Some("mobile-apk"),
        &cwd,
        std::env::temp_dir().join("rua-runtime-cache"),
    );
    let plan = authority.plan(
        ProcessShape::Interactive,
        ToolchainIntent::Shell,
        RuntimeLaunchRequest {
            cwd: Some(cwd.clone()),
            ..Default::default()
        },
    );
    if !plan.diagnostics.is_empty() {
        return Err(TerminalError::Spawn(plan.diagnostics.join("; ")));
    }

    let process = plan
        .into_process()
        .map_err(|error| TerminalError::Spawn(error.to_string()))?;
    Ok(NativeTerminalSpawnRequest {
        cwd: process.cwd.unwrap_or(cwd),
        shell: NativeShellSpec {
            program: process.command.to_string_lossy().into_owned(),
            args: process.args,
            kind: NativeShellKind::Unix,
        },
        env: process.env,
    })
}

fn windows_terminal_spawn_request(
    cwd: PathBuf,
    terminal_profile_id: Option<&str>,
) -> NativeTerminalSpawnRequest {
    let path = env::var_os("PATH");
    let shell = match terminal_profile_id.unwrap_or("powershell") {
        "cmd" => NativeShellSpec {
            program: find_on_path("cmd.exe", path.as_deref(), ';')
                .unwrap_or_else(|| PathBuf::from("cmd.exe"))
                .to_string_lossy()
                .into_owned(),
            args: Vec::new(),
            kind: NativeShellKind::Cmd,
        },
        "pwsh" => NativeShellSpec {
            program: find_on_path("pwsh.exe", path.as_deref(), ';')
                .unwrap_or_else(|| PathBuf::from("pwsh.exe"))
                .to_string_lossy()
                .into_owned(),
            args: vec!["-NoLogo".to_string()],
            kind: NativeShellKind::PowerShell,
        },
        "git-bash" => NativeShellSpec {
            program: find_on_path("bash.exe", path.as_deref(), ';')
                .or_else(|| {
                    standard_windows_git_bash_candidates()
                        .into_iter()
                        .find(|candidate| candidate.is_file())
                })
                .unwrap_or_else(|| PathBuf::from("bash.exe"))
                .to_string_lossy()
                .into_owned(),
            args: vec!["--login".to_string(), "-i".to_string()],
            kind: NativeShellKind::Bash,
        },
        "wsl" => NativeShellSpec {
            program: find_on_path("wsl.exe", path.as_deref(), ';')
                .unwrap_or_else(|| PathBuf::from("wsl.exe"))
                .to_string_lossy()
                .into_owned(),
            args: Vec::new(),
            kind: NativeShellKind::Wsl,
        },
        profile if profile.starts_with("wsl:") => NativeShellSpec {
            program: find_on_path("wsl.exe", path.as_deref(), ';')
                .unwrap_or_else(|| PathBuf::from("wsl.exe"))
                .to_string_lossy()
                .into_owned(),
            args: vec![
                "-d".to_string(),
                profile.trim_start_matches("wsl:").to_string(),
            ],
            kind: NativeShellKind::Wsl,
        },
        _ => NativeShellSpec {
            program: find_on_path("powershell.exe", path.as_deref(), ';')
                .unwrap_or_else(|| PathBuf::from("powershell.exe"))
                .to_string_lossy()
                .into_owned(),
            args: vec!["-NoLogo".to_string()],
            kind: NativeShellKind::PowerShell,
        },
    };
    NativeTerminalSpawnRequest {
        cwd,
        shell,
        env: BTreeMap::new(),
    }
}

fn native_interactive_shell_for_current_process() -> NativeShellSpec {
    if cfg!(unix) {
        if let Some(shell) = env::var_os("SHELL").filter(|value| !value.is_empty()) {
            let shell = PathBuf::from(shell);
            if is_executable_shell_candidate(&shell) {
                return NativeShellSpec {
                    program: shell.to_string_lossy().into_owned(),
                    args: Vec::new(),
                    kind: rua_core::runtime_layout::NativeShellKind::Unix,
                };
            }
        }
        let prefix = env::var("PREFIX").ok();
        for candidate in unix_interactive_shell_candidates(prefix.as_deref()) {
            if is_executable_shell_candidate(&candidate) {
                return NativeShellSpec {
                    program: candidate.to_string_lossy().into_owned(),
                    args: Vec::new(),
                    kind: rua_core::runtime_layout::NativeShellKind::Unix,
                };
            }
        }
    }
    let mut shell = rua_core::runtime_layout::RuntimeLayout::native_shell_for_current_process();
    shell.args.clear();
    shell
}

fn unix_interactive_shell_candidates(prefix: Option<&str>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(prefix) = prefix.filter(|value| !value.is_empty()) {
        let prefix = Path::new(prefix);
        candidates.push(prefix.join("bin/zsh"));
        candidates.push(prefix.join("bin/bash"));
        candidates.push(prefix.join("bin/sh"));
    }
    candidates.push(PathBuf::from("/bin/zsh"));
    candidates.push(PathBuf::from("/bin/bash"));
    candidates.push(PathBuf::from("/bin/sh"));
    candidates.push(PathBuf::from("/system/bin/sh"));
    candidates
}

fn standard_windows_git_bash_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\Program Files\Git\bin\bash.exe"),
        PathBuf::from(r"C:\Program Files\Git\usr\bin\bash.exe"),
        PathBuf::from(r"C:\Program Files (x86)\Git\bin\bash.exe"),
        PathBuf::from(r"C:\Program Files (x86)\Git\usr\bin\bash.exe"),
    ]
}

fn find_on_path(command: &str, path: Option<&std::ffi::OsStr>, separator: char) -> Option<PathBuf> {
    path?
        .to_string_lossy()
        .split(separator)
        .filter(|entry| !entry.trim().is_empty())
        .map(|entry| PathBuf::from(entry).join(command))
        .find(|candidate| candidate.is_file())
}

fn terminal_profile_label_for_program(program: &str) -> String {
    Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Shell")
        .to_string()
}

fn resolve_terminal_profile(
    profiles: &TerminalProfileList,
    requested: Option<&str>,
) -> Result<TerminalProfile, TerminalError> {
    let profile_id = requested
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&profiles.default_profile_id);
    profiles
        .profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .cloned()
        .ok_or_else(|| {
            TerminalError::Spawn(format!("terminal profile not available: {profile_id}"))
        })
}

fn is_executable_shell_candidate(path: &Path) -> bool {
    path.is_file()
}

impl TerminalProcessSpawner for NativeTerminalProcessSpawner {
    fn spawn(&self, request: TerminalSpawnRequest) -> Result<TerminalProcessHandle, TerminalError> {
        let plan =
            Self::default_spawn_request(request.cwd, request.terminal_profile_id.as_deref())?;
        #[cfg(target_os = "android")]
        if xterm_headless_bridge_launch().is_none() {
            return Err(TerminalError::Spawn(
                "Debian/Node terminal environment is not ready; open the app's environment status to view preparation or retry".to_string(),
            ));
        }
        spawn_native_terminal_process(plan)
    }
}

#[cfg(not(target_os = "android"))]
fn spawn_native_terminal_process(
    plan: NativeTerminalSpawnRequest,
) -> Result<TerminalProcessHandle, TerminalError> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: TerminalPtyGeometry::default().rows,
            cols: TerminalPtyGeometry::default().cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| TerminalError::Spawn(error.to_string()))?;
    let mut command = CommandBuilder::new(&plan.shell.program);
    command.args(&plan.shell.args);
    command.cwd(plan.cwd.as_os_str());
    let locale = terminal_utf8_locale();
    command.env("TERM", TERMINAL_TERM);
    command.env("COLORTERM", TERMINAL_COLORTERM);
    command.env("LANG", locale.as_str());
    command.env("LC_CTYPE", locale.as_str());
    for (key, value) in plan.env {
        command.env(key, value);
    }
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| TerminalError::Spawn(error.to_string()))?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| TerminalError::Spawn(error.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| TerminalError::Spawn(error.to_string()))?;
    let (tx, rx) = mpsc::unbounded_channel();
    spawn_reader_thread(reader, tx);
    Ok(TerminalProcessHandle {
        output: rx,
        control: Arc::new(NativeTerminalProcessControl {
            child: Mutex::new(child),
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
        }),
    })
}

#[cfg(target_os = "android")]
fn spawn_native_terminal_process(
    plan: NativeTerminalSpawnRequest,
) -> Result<TerminalProcessHandle, TerminalError> {
    let pty = AndroidPty::open(TerminalPtyGeometry::default())?;
    let slave_for_child = pty.open_slave()?;
    let stdin_fd = duplicate_fd(slave_for_child.as_raw_fd())?;
    let stdout_fd = duplicate_fd(slave_for_child.as_raw_fd())?;
    let stderr_fd = duplicate_fd(slave_for_child.as_raw_fd())?;
    let mut command = Command::new(&plan.shell.program);
    command.env_clear();
    command.args(&plan.shell.args);
    command.current_dir(plan.cwd.as_os_str());
    let locale = terminal_utf8_locale();
    command.env("TERM", TERMINAL_TERM);
    command.env("COLORTERM", TERMINAL_COLORTERM);
    command.env("LANG", locale.as_str());
    command.env("LC_CTYPE", locale.as_str());
    for (key, value) in plan.env {
        command.env(key, value);
    }
    command.stdin(Stdio::from(std::fs::File::from(stdin_fd)));
    command.stdout(Stdio::from(std::fs::File::from(stdout_fd)));
    command.stderr(Stdio::from(std::fs::File::from(stderr_fd)));
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(slave_for_child.as_raw_fd(), libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let reader = pty.reader()?;
    let writer = pty.writer()?;
    let child = command
        .spawn()
        .map_err(|error| TerminalError::Spawn(error.to_string()))?;
    let (tx, rx) = mpsc::unbounded_channel();
    spawn_reader_thread(reader, tx);
    Ok(TerminalProcessHandle {
        output: rx,
        control: Arc::new(NativeTerminalProcessControl {
            child: Mutex::new(Some(child)),
            pty,
            writer: Mutex::new(writer),
        }),
    })
}

fn spawn_reader_thread<R>(mut reader: R, tx: mpsc::UnboundedSender<Vec<u8>>)
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buf = [0_u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

#[cfg(not(target_os = "android"))]
struct NativeTerminalProcessControl {
    child: Mutex<Box<dyn Child + Send + Sync>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
}

#[cfg(not(target_os = "android"))]
impl TerminalProcessControl for NativeTerminalProcessControl {
    fn kill(&self) {
        let _ = self.child.lock().expect("terminal child lock").kill();
    }

    fn write_input(&self, data: &[u8]) -> Result<(), TerminalError> {
        let mut writer = self.writer.lock().expect("terminal pty writer lock");
        writer
            .write_all(data)
            .and_then(|_| writer.flush())
            .map_err(|error| TerminalError::Spawn(error.to_string()))
    }

    fn resize_pty(&self, geometry: TerminalPtyGeometry) -> Result<(), TerminalError> {
        self.master
            .lock()
            .expect("terminal pty master lock")
            .resize(PtySize {
                rows: geometry.rows,
                cols: geometry.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::Spawn(error.to_string()))
    }
}

#[cfg(target_os = "android")]
struct AndroidPty {
    master: OwnedFd,
    slave_name: String,
}

#[cfg(target_os = "android")]
impl AndroidPty {
    fn open(geometry: TerminalPtyGeometry) -> Result<Self, TerminalError> {
        let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
        if master == -1 {
            return Err(TerminalError::Spawn(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        if unsafe { libc::grantpt(master.as_raw_fd()) } == -1 {
            return Err(TerminalError::Spawn(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        if unsafe { libc::unlockpt(master.as_raw_fd()) } == -1 {
            return Err(TerminalError::Spawn(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let mut name = [0 as libc::c_char; 128];
        if unsafe { libc::ptsname_r(master.as_raw_fd(), name.as_mut_ptr(), name.len()) } != 0 {
            return Err(TerminalError::Spawn(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let slave_name = unsafe { CStr::from_ptr(name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let pty = Self { master, slave_name };
        pty.resize(geometry)?;
        Ok(pty)
    }

    fn open_slave(&self) -> Result<OwnedFd, TerminalError> {
        let path = std::ffi::CString::new(self.slave_name.as_str())
            .map_err(|error| TerminalError::Spawn(error.to_string()))?;
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            )
        };
        if fd == -1 {
            return Err(TerminalError::Spawn(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn reader(&self) -> Result<std::fs::File, TerminalError> {
        duplicate_fd(self.master.as_raw_fd()).map(std::fs::File::from)
    }

    fn writer(&self) -> Result<std::fs::File, TerminalError> {
        duplicate_fd(self.master.as_raw_fd()).map(std::fs::File::from)
    }

    fn resize(&self, geometry: TerminalPtyGeometry) -> Result<(), TerminalError> {
        let mut size = libc::winsize {
            ws_row: geometry.rows,
            ws_col: geometry.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        if unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &mut size) } == -1 {
            return Err(TerminalError::Spawn(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(target_os = "android")]
fn duplicate_fd(fd: RawFd) -> Result<OwnedFd, TerminalError> {
    let duplicated = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicated == -1 {
        return Err(TerminalError::Spawn(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
}

#[cfg(target_os = "android")]
struct NativeTerminalProcessControl {
    child: Mutex<Option<StdChild>>,
    pty: AndroidPty,
    writer: Mutex<std::fs::File>,
}

#[cfg(target_os = "android")]
impl TerminalProcessControl for NativeTerminalProcessControl {
    fn kill(&self) {
        let Some(mut child) = self.child.lock().expect("terminal child lock").take() else {
            return;
        };
        // setsid() makes the child the terminal's session leader. Interactive bash
        // puts foreground jobs in separate process groups, so killing only PRoot
        // (or only its group) leaves live guest processes behind.
        // Keep the child unreaped until discovery/signalling completes: its PID,
        // and therefore this session ID, cannot be reused during the scan.
        let session_id = child.id() as libc::pid_t;
        let mut groups = HashSet::new();
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                    continue;
                };
                let Some((_, fields)) = stat.rsplit_once(") ") else {
                    continue;
                };
                let mut fields = fields.split_whitespace().skip(2);
                let group = fields.next().and_then(|v| v.parse::<libc::pid_t>().ok());
                let session = fields.next().and_then(|v| v.parse::<libc::pid_t>().ok());
                if session == Some(session_id) {
                    if let Some(group) = group.filter(|group| *group > 0 && *group != session_id) {
                        groups.insert(group);
                    }
                }
            }
        }
        for group in groups {
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
        unsafe { libc::kill(-session_id, libc::SIGKILL) };
        let _ = child.kill();
        let _ = child.wait();
    }

    fn write_input(&self, data: &[u8]) -> Result<(), TerminalError> {
        let mut writer = self.writer.lock().expect("terminal pty writer lock");
        writer
            .write_all(data)
            .and_then(|_| writer.flush())
            .map_err(|error| TerminalError::Spawn(error.to_string()))
    }

    fn resize_pty(&self, geometry: TerminalPtyGeometry) -> Result<(), TerminalError> {
        self.pty.resize(geometry)
    }
}

#[cfg(target_os = "android")]
impl Drop for NativeTerminalProcessControl {
    fn drop(&mut self) {
        self.kill();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSessionSnapshot {
    pub session_id: String,
    pub cwd: PathBuf,
    pub profile_id: String,
    pub profile_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub viewer_count: usize,
    pub input_lease_holder: Option<TerminalViewerIdentity>,
    pub pty_geometry: TerminalPtyGeometry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TerminalStreamCursor {
    pub next_activity_sequence: u64,
    pub next_output_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub data: String,
    pub cols: u16,
    pub rows: u16,
    pub cursor: TerminalStreamCursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalActivityFrame {
    Output {
        activity_sequence: u64,
        start_offset: u64,
        end_offset: u64,
        #[serde(with = "terminal_output_data_serde")]
        data: Vec<u8>,
    },
    InputLeaseChanged {
        activity_sequence: u64,
        holder: Option<TerminalViewerIdentity>,
    },
    PtyGeometryChanged {
        activity_sequence: u64,
        geometry: TerminalPtyGeometry,
    },
    Exited {
        activity_sequence: u64,
    },
    Error {
        activity_sequence: u64,
        detail: String,
    },
}

impl TerminalActivityFrame {
    pub fn activity_sequence(&self) -> u64 {
        match self {
            Self::Output {
                activity_sequence, ..
            }
            | Self::InputLeaseChanged {
                activity_sequence, ..
            }
            | Self::PtyGeometryChanged {
                activity_sequence, ..
            }
            | Self::Exited { activity_sequence }
            | Self::Error {
                activity_sequence, ..
            } => *activity_sequence,
        }
    }
}

mod terminal_output_data_serde {
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&BASE64_STANDARD.encode(data))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        BASE64_STANDARD
            .decode(String::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

pub fn terminal_activity_frame_json(frame: &TerminalActivityFrame) -> serde_json::Value {
    match frame {
        TerminalActivityFrame::Output {
            activity_sequence,
            start_offset,
            end_offset,
            data,
        } => serde_json::json!({
            "type": "output",
            "activity_sequence": activity_sequence,
            "start_offset": start_offset,
            "end_offset": end_offset,
            "data_base64": BASE64_STANDARD.encode(data),
        }),
        TerminalActivityFrame::InputLeaseChanged {
            activity_sequence,
            holder,
        } => serde_json::json!({
            "type": "input_lease_changed",
            "activity_sequence": activity_sequence,
            "holder": holder,
        }),
        TerminalActivityFrame::PtyGeometryChanged {
            activity_sequence,
            geometry,
        } => serde_json::json!({
            "type": "pty_geometry_changed",
            "activity_sequence": activity_sequence,
            "geometry": geometry,
        }),
        TerminalActivityFrame::Exited { activity_sequence } => serde_json::json!({
            "type": "exited",
            "activity_sequence": activity_sequence,
        }),
        TerminalActivityFrame::Error {
            activity_sequence,
            detail,
        } => serde_json::json!({
            "type": "error",
            "activity_sequence": activity_sequence,
            "detail": detail,
        }),
    }
}

pub fn terminal_activity_frame_from_json(
    value: serde_json::Value,
) -> Result<TerminalActivityFrame, String> {
    let frame_type = value
        .get("type")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "terminal activity payload missing type".to_string())?;
    let activity_sequence = value
        .get("activity_sequence")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| "terminal activity payload missing activity_sequence".to_string())?;
    match frame_type {
        "output" => {
            let data = value
                .get("data_base64")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "terminal output payload missing data".to_string())?;
            let data = BASE64_STANDARD
                .decode(data)
                .map_err(|error| error.to_string())?;
            let start_offset = value
                .get("start_offset")
                .and_then(|value| value.as_u64())
                .ok_or_else(|| "terminal output payload missing start_offset".to_string())?;
            let end_offset = value
                .get("end_offset")
                .and_then(|value| value.as_u64())
                .ok_or_else(|| "terminal output payload missing end_offset".to_string())?;
            Ok(TerminalActivityFrame::Output {
                activity_sequence,
                start_offset,
                end_offset,
                data,
            })
        }
        "input_lease_changed" => {
            let holder = value
                .get("holder")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let holder = serde_json::from_value(holder).map_err(|error| error.to_string())?;
            Ok(TerminalActivityFrame::InputLeaseChanged {
                activity_sequence,
                holder,
            })
        }
        "pty_geometry_changed" => {
            let geometry = value
                .get("geometry")
                .cloned()
                .ok_or_else(|| "terminal geometry payload missing geometry".to_string())?;
            let geometry = serde_json::from_value(geometry).map_err(|error| error.to_string())?;
            Ok(TerminalActivityFrame::PtyGeometryChanged {
                activity_sequence,
                geometry,
            })
        }
        "exited" => Ok(TerminalActivityFrame::Exited { activity_sequence }),
        "error" => {
            let detail = value
                .get("detail")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "terminal error payload missing detail".to_string())?;
            Ok(TerminalActivityFrame::Error {
                activity_sequence,
                detail: detail.to_string(),
            })
        }
        other => Err(format!("unknown terminal activity payload type: {other}")),
    }
}

pub fn terminal_activity_from_remote_stream_frame(
    frame: ActiveStreamBroadcastFrame,
) -> Option<TerminalActivityFrame> {
    match frame {
        ActiveStreamBroadcastFrame::Event { payload, .. } => serde_json::from_str(&payload)
            .ok()
            .and_then(|value| terminal_activity_frame_from_json(value).ok()),
        ActiveStreamBroadcastFrame::Error { detail, seq, .. } => {
            Some(TerminalActivityFrame::Error {
                activity_sequence: seq,
                detail,
            })
        }
        ActiveStreamBroadcastFrame::Gap { .. } => None,
        ActiveStreamBroadcastFrame::Started { .. }
        | ActiveStreamBroadcastFrame::Done { .. }
        | ActiveStreamBroadcastFrame::AuthorityRedirect { .. } => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSessionAttachment {
    pub session: TerminalSessionSnapshot,
    pub snapshot: TerminalSnapshot,
    pub tail: Vec<TerminalActivityFrame>,
    pub live_cursor: TerminalStreamCursor,
}

#[derive(Debug)]
pub enum TerminalError {
    Spawn(String),
    NotFound(String),
    SnapshotUnavailable(String),
    CursorUnavailable {
        session_id: String,
        requested: TerminalStreamCursor,
        earliest: TerminalStreamCursor,
        current: TerminalStreamCursor,
    },
    InputLeaseRequired,
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(message) => write!(f, "terminal spawn failed: {message}"),
            Self::NotFound(session_id) => write!(f, "terminal session not found: {session_id}"),
            Self::SnapshotUnavailable(session_id) => {
                write!(f, "terminal snapshot unavailable for session: {session_id}")
            }
            Self::CursorUnavailable {
                session_id,
                requested,
                earliest,
                current,
            } => write!(
                f,
                "terminal stream cursor unavailable for {session_id}: requested activity {} output {}, available activity {} output {} through activity {} output {}",
                requested.next_activity_sequence,
                requested.next_output_offset,
                earliest.next_activity_sequence,
                earliest.next_output_offset,
                current.next_activity_sequence,
                current.next_output_offset,
            ),
            Self::InputLeaseRequired => write!(f, "terminal input lease is required"),
        }
    }
}

impl std::error::Error for TerminalError {}

struct TerminalSession {
    session_id: String,
    creation_sequence: u64,
    cwd: PathBuf,
    profile: TerminalProfile,
    display_name: Option<String>,
    viewers: HashSet<TerminalViewerIdentity>,
    input_lease_holder: Option<TerminalViewerIdentity>,
    pty_geometry: TerminalPtyGeometry,
    scrollback_lines: usize,
    palette: TerminalPalette,
    detached_generation: u64,
    replay: ReplayBuffer,
    next_activity_sequence: u64,
    next_output_offset: u64,
    subscribers: Vec<mpsc::UnboundedSender<TerminalActivityFrame>>,
    control: Arc<dyn TerminalProcessControl>,
}

impl TerminalSession {
    fn snapshot(&self) -> TerminalSessionSnapshot {
        TerminalSessionSnapshot {
            session_id: self.session_id.clone(),
            cwd: self.cwd.clone(),
            profile_id: self.profile.id.clone(),
            profile_label: self.profile.label.clone(),
            display_name: self.display_name.clone(),
            viewer_count: self.viewers.len(),
            input_lease_holder: self.input_lease_holder.clone(),
            pty_geometry: self.pty_geometry,
        }
    }

    fn cursor(&self) -> TerminalStreamCursor {
        TerminalStreamCursor {
            next_activity_sequence: self.next_activity_sequence,
            next_output_offset: self.next_output_offset,
        }
    }

    fn output_frame(&mut self, data: Vec<u8>) -> TerminalActivityFrame {
        let activity_sequence = self.take_activity_sequence();
        let start_offset = self.next_output_offset;
        let end_offset = start_offset.saturating_add(data.len() as u64);
        self.next_output_offset = end_offset;
        TerminalActivityFrame::Output {
            activity_sequence,
            start_offset,
            end_offset,
            data,
        }
    }

    fn take_activity_sequence(&mut self) -> u64 {
        let activity_sequence = self.next_activity_sequence;
        self.next_activity_sequence = self.next_activity_sequence.saturating_add(1);
        activity_sequence
    }
}

fn coalesce_terminal_output(
    first: Vec<u8>,
    output: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    pending: &mut VecDeque<Vec<u8>>,
) -> Vec<u8> {
    let mut batch = Vec::with_capacity(TERMINAL_OUTPUT_COALESCE_MAX_BYTES.min(first.len()));
    let append = |batch: &mut Vec<u8>, data: Vec<u8>, pending: &mut VecDeque<Vec<u8>>| {
        let remaining = TERMINAL_OUTPUT_COALESCE_MAX_BYTES.saturating_sub(batch.len());
        if data.len() <= remaining {
            batch.extend_from_slice(&data);
        } else {
            batch.extend_from_slice(&data[..remaining]);
            pending.push_front(data[remaining..].to_vec());
        }
    };
    append(&mut batch, first, pending);
    while batch.len() < TERMINAL_OUTPUT_COALESCE_MAX_BYTES && pending.is_empty() {
        let Ok(data) = output.try_recv() else {
            break;
        };
        append(&mut batch, data, pending);
    }
    batch
}

struct ReplayBuffer {
    bytes: usize,
    frames: VecDeque<TerminalActivityFrame>,
}

impl ReplayBuffer {
    fn new() -> Self {
        Self {
            bytes: 0,
            frames: VecDeque::new(),
        }
    }

    fn push_frame(&mut self, frame: TerminalActivityFrame) {
        let frame_len = frame_storage_bytes(&frame);
        self.bytes = self.bytes.saturating_add(frame_len);
        self.frames.push_back(frame);
        while self.bytes > TERMINAL_REPLAY_MAX_BYTES && self.frames.len() > 1 {
            if let Some(removed) = self.frames.pop_front() {
                self.bytes = self.bytes.saturating_sub(frame_storage_bytes(&removed));
            }
        }
    }

    fn tail_from_cursor(
        &self,
        requested: TerminalStreamCursor,
        current: TerminalStreamCursor,
    ) -> Option<Vec<TerminalActivityFrame>> {
        if requested == current {
            return Some(Vec::new());
        }
        let mut cursor = self.earliest_cursor(current);
        if requested == cursor {
            return Some(self.frames.iter().cloned().collect());
        }
        for (index, frame) in self.frames.iter().enumerate() {
            if frame.activity_sequence() != cursor.next_activity_sequence {
                return None;
            }
            cursor.next_activity_sequence = cursor.next_activity_sequence.saturating_add(1);
            if let TerminalActivityFrame::Output {
                start_offset,
                end_offset,
                data,
                ..
            } = frame
            {
                if *start_offset != cursor.next_output_offset
                    || end_offset.saturating_sub(*start_offset) != data.len() as u64
                {
                    return None;
                }
                cursor.next_output_offset = *end_offset;
            }
            if requested == cursor {
                return Some(self.frames.iter().skip(index + 1).cloned().collect());
            }
        }
        None
    }

    fn earliest_cursor(&self, fallback: TerminalStreamCursor) -> TerminalStreamCursor {
        let Some(first) = self.frames.front() else {
            return fallback;
        };
        let next_output_offset = self
            .frames
            .iter()
            .find_map(|frame| match frame {
                TerminalActivityFrame::Output { start_offset, .. } => Some(*start_offset),
                _ => None,
            })
            .unwrap_or(fallback.next_output_offset);
        TerminalStreamCursor {
            next_activity_sequence: first.activity_sequence(),
            next_output_offset,
        }
    }
}

fn frame_storage_bytes(frame: &TerminalActivityFrame) -> usize {
    match frame {
        TerminalActivityFrame::Output { data, .. } => data.len(),
        TerminalActivityFrame::InputLeaseChanged { .. } => 64,
        TerminalActivityFrame::PtyGeometryChanged { .. } => 32,
        TerminalActivityFrame::Exited { .. } => 16,
        TerminalActivityFrame::Error { detail, .. } => detail.len().saturating_add(16),
    }
}

async fn publish_terminal_activity_frame(
    hub: &ActiveStreamBroadcastHub,
    session_id: &str,
    frame: &TerminalActivityFrame,
    _scrollback_lines: usize,
) {
    let stream_id = terminal_activity_stream_id(session_id);
    hub.publish_terminal_event_with_replay_max_bytes(
        &stream_id,
        session_id,
        terminal_activity_frame_json(frame).to_string(),
        TERMINAL_REPLAY_MAX_BYTES,
    )
    .await;
}

#[derive(Clone)]
pub struct TerminalSessionAuthority {
    config: TerminalAuthorityConfig,
    spawner: Arc<dyn TerminalProcessSpawner>,
    sessions: Arc<RwLock<HashMap<String, TerminalSession>>>,
    next_session_sequence: Arc<AtomicU64>,
    active_stream_broadcast_hub: Option<Arc<ActiveStreamBroadcastHub>>,
    xterm_state: XtermStateBridge,
}

impl TerminalSessionAuthority {
    pub fn new(config: TerminalAuthorityConfig, spawner: Arc<dyn TerminalProcessSpawner>) -> Self {
        Self {
            config,
            spawner,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            next_session_sequence: Arc::new(AtomicU64::new(0)),
            active_stream_broadcast_hub: None,
            xterm_state: XtermStateBridge::new(),
        }
    }

    pub fn with_active_stream_broadcast_hub(mut self, hub: Arc<ActiveStreamBroadcastHub>) -> Self {
        self.active_stream_broadcast_hub = Some(hub);
        self
    }

    pub fn list_profiles(&self) -> TerminalProfileList {
        self.spawner.list_profiles()
    }

    pub async fn open(
        &self,
        request: OpenTerminalSessionRequest,
    ) -> Result<TerminalSessionSnapshot, TerminalError> {
        if self.xterm_state.shutdown.is_cancelled() {
            return Err(TerminalError::Spawn(
                "Terminal authority is shutting down".to_string(),
            ));
        }
        let creation_sequence = self.next_session_sequence.fetch_add(1, Ordering::Relaxed);
        let cwd = request
            .cwd
            .unwrap_or_else(|| self.config.default_home.clone());
        let profile = resolve_terminal_profile(
            &self.spawner.list_profiles(),
            request.terminal_profile_id.as_deref(),
        )?;
        let mut handle = self.spawner.spawn(TerminalSpawnRequest {
            cwd: cwd.clone(),
            terminal_profile_id: Some(profile.id.clone()),
        })?;
        let session_id = format!("terminal-session-{}", uuid::Uuid::new_v4());
        let initial_geometry = TerminalPtyGeometry::default();
        self.xterm_state.create(
            session_id.clone(),
            initial_geometry,
            request.scrollback_lines,
        );
        let mut viewers = HashSet::new();
        viewers.insert(request.viewer.clone());
        let mut session = TerminalSession {
            session_id: session_id.clone(),
            creation_sequence,
            cwd,
            profile,
            display_name: None,
            viewers,
            input_lease_holder: Some(request.viewer),
            pty_geometry: initial_geometry,
            scrollback_lines: request.scrollback_lines,
            palette: TerminalPalette::default(),
            detached_generation: 0,
            replay: ReplayBuffer::new(),
            next_activity_sequence: 0,
            next_output_offset: 0,
            subscribers: Vec::new(),
            control: Arc::clone(&handle.control),
        };
        let mut pending_output = VecDeque::new();
        while let Some(data) = pending_output
            .pop_front()
            .or_else(|| handle.output.try_recv().ok())
        {
            let frame_data =
                coalesce_terminal_output(data, &mut handle.output, &mut pending_output);
            self.xterm_state.write(
                session_id.clone(),
                frame_data.clone(),
                Arc::clone(&session.control),
            );
            let frame = session.output_frame(frame_data);
            session.replay.push_frame(frame);
        }
        let snapshot = session.snapshot();
        let mut sessions = self.sessions.write().await;
        if self.xterm_state.shutdown.is_cancelled() {
            session.control.kill();
            return Err(TerminalError::Spawn(
                "Terminal authority is shutting down".to_string(),
            ));
        }
        sessions.insert(session_id.clone(), session);
        drop(sessions);
        self.spawn_output_pump(session_id, handle.output);
        Ok(snapshot)
    }

    pub async fn list(&self) -> Vec<TerminalSessionSnapshot> {
        let sessions = self.sessions.read().await;
        let mut ordered = sessions.values().collect::<Vec<_>>();
        ordered.sort_unstable_by_key(|session| session.creation_sequence);
        ordered.into_iter().map(TerminalSession::snapshot).collect()
    }

    pub async fn attach(
        &self,
        session_id: &str,
        viewer: TerminalViewerIdentity,
    ) -> Result<TerminalSessionAttachment, TerminalError> {
        self.attach_internal(session_id, viewer, None).await
    }

    async fn attach_internal(
        &self,
        session_id: &str,
        viewer: TerminalViewerIdentity,
        known_cursor: Option<TerminalStreamCursor>,
    ) -> Result<TerminalSessionAttachment, TerminalError> {
        let (barrier_cursor, geometry, snapshot_rx, attach_generation) = {
            let mut sessions = self.sessions.write().await;
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
            let attach_generation = if session.viewers.contains(&viewer) {
                None
            } else {
                session.detached_generation = session.detached_generation.wrapping_add(1);
                Some(session.detached_generation)
            };
            let snapshot_rx = self.xterm_state.begin_serialize(session_id);
            (
                session.cursor(),
                session.pty_geometry,
                snapshot_rx,
                attach_generation,
            )
        };
        let snapshot_data = match snapshot_rx {
            Some(rx) => rx.await.ok().flatten(),
            None => None,
        };
        let Some(snapshot_data) = snapshot_data else {
            self.restore_detached_cleanup_after_failed_attach(session_id, attach_generation)
                .await;
            return Err(TerminalError::SnapshotUnavailable(session_id.to_string()));
        };
        let attachment = {
            let mut sessions = self.sessions.write().await;
            (|| {
                let session = sessions
                    .get_mut(session_id)
                    .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
                let live_cursor = session.cursor();
                let tail = session
                    .replay
                    .tail_from_cursor(barrier_cursor, live_cursor)
                    .ok_or_else(|| TerminalError::CursorUnavailable {
                        session_id: session_id.to_string(),
                        requested: barrier_cursor,
                        earliest: session.replay.earliest_cursor(live_cursor),
                        current: live_cursor,
                    })?;
                session.viewers.insert(viewer.clone());
                Ok((session.snapshot(), tail, live_cursor))
            })()
        };
        let (session_snapshot, tail, live_cursor) = match attachment {
            Ok(attachment) => attachment,
            Err(error) => {
                self.restore_detached_cleanup_after_failed_attach(session_id, attach_generation)
                    .await;
                return Err(error);
            }
        };
        let snapshot = TerminalSnapshot {
            data: snapshot_data,
            cols: geometry.cols,
            rows: geometry.rows,
            cursor: barrier_cursor,
        };
        tracing::debug!(
            event = "terminal_attach",
            session_id,
            viewer_id = %viewer.browser_id,
            placement = if viewer.device_id.is_some() { "remote" } else { "local" },
            requested_activity = ?known_cursor.map(|cursor| cursor.next_activity_sequence),
            requested_output = ?known_cursor.map(|cursor| cursor.next_output_offset),
            snapshot_activity = barrier_cursor.next_activity_sequence,
            snapshot_output = barrier_cursor.next_output_offset,
            live_activity = live_cursor.next_activity_sequence,
            live_output = live_cursor.next_output_offset,
            tail_frames = tail.len(),
            snapshot = true,
            recovery_reason = if known_cursor.is_some() { "replacement" } else { "cold_attach" },
        );
        Ok(TerminalSessionAttachment {
            session: session_snapshot,
            snapshot,
            tail,
            live_cursor,
        })
    }

    async fn restore_detached_cleanup_after_failed_attach(
        &self,
        session_id: &str,
        attach_generation: Option<u64>,
    ) {
        let Some(attach_generation) = attach_generation else {
            return;
        };
        let should_cleanup = self
            .sessions
            .read()
            .await
            .get(session_id)
            .is_some_and(|session| {
                session.viewers.is_empty() && session.detached_generation == attach_generation
            });
        if should_cleanup {
            self.spawn_detached_cleanup(session_id.to_string(), attach_generation);
        }
    }

    pub async fn attach_with_scrollback(
        &self,
        session_id: &str,
        viewer: TerminalViewerIdentity,
        scrollback_lines: usize,
        cursor: Option<TerminalStreamCursor>,
    ) -> Result<TerminalSessionAttachment, TerminalError> {
        self.set_scrollback_lines(session_id, scrollback_lines)
            .await?;
        self.attach_internal(session_id, viewer, cursor).await
    }

    pub async fn attach_remote(
        &self,
        session_id: &str,
        viewer: TerminalViewerIdentity,
        scrollback_lines: usize,
        cursor: Option<TerminalStreamCursor>,
    ) -> Result<TerminalSessionAttachment, TerminalError> {
        self.set_scrollback_lines(session_id, scrollback_lines)
            .await?;
        self.attach_internal(session_id, viewer, cursor).await
    }

    pub async fn detach(
        &self,
        session_id: &str,
        viewer: &TerminalViewerIdentity,
    ) -> Result<(), TerminalError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
        if !session.viewers.remove(viewer) {
            return Ok(());
        }
        session.detached_generation = session.detached_generation.wrapping_add(1);
        let cleanup_generation = session
            .viewers
            .is_empty()
            .then_some(session.detached_generation);
        drop(sessions);
        if let Some(generation) = cleanup_generation {
            self.spawn_detached_cleanup(session_id.to_string(), generation);
        }
        Ok(())
    }

    pub async fn set_scrollback_lines(
        &self,
        session_id: &str,
        scrollback_lines: usize,
    ) -> Result<(), TerminalError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
        if session.scrollback_lines == scrollback_lines {
            return Ok(());
        }
        session.scrollback_lines = scrollback_lines;
        self.xterm_state
            .set_scrollback(session_id.to_string(), scrollback_lines);
        Ok(())
    }

    pub async fn update_palette(
        &self,
        session_id: &str,
        viewer: &TerminalViewerIdentity,
        palette: TerminalPalette,
    ) -> Result<(), TerminalError> {
        palette.validate()?;
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
        if session.input_lease_holder.as_ref() != Some(viewer) {
            return Err(TerminalError::InputLeaseRequired);
        }
        if session.palette == palette {
            return Ok(());
        }
        session.palette = palette.clone();
        self.xterm_state.set_palette(
            session_id.to_string(),
            palette,
            Arc::clone(&session.control),
        );
        Ok(())
    }

    pub async fn rename(
        &self,
        session_id: &str,
        display_name: Option<String>,
    ) -> Result<TerminalSessionSnapshot, TerminalError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
        session.display_name = display_name.and_then(|name| {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });
        Ok(session.snapshot())
    }

    pub async fn take_input_lease(
        &self,
        session_id: &str,
        viewer: TerminalViewerIdentity,
    ) -> Result<TerminalSessionSnapshot, TerminalError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
        if session.viewers.insert(viewer.clone()) {
            session.detached_generation = session.detached_generation.wrapping_add(1);
        }
        session.input_lease_holder = Some(viewer);
        let frame = TerminalActivityFrame::InputLeaseChanged {
            activity_sequence: session.take_activity_sequence(),
            holder: session.input_lease_holder.clone(),
        };
        session.replay.push_frame(frame.clone());
        session
            .subscribers
            .retain(|subscriber| subscriber.send(frame.clone()).is_ok());
        let snapshot = session.snapshot();
        let scrollback_lines = session.scrollback_lines;
        drop(sessions);
        self.publish_activity(session_id, frame, scrollback_lines)
            .await;
        Ok(snapshot)
    }

    pub async fn subscribe_activity(
        &self,
        session_id: &str,
        cursor: TerminalStreamCursor,
    ) -> Result<mpsc::UnboundedReceiver<TerminalActivityFrame>, TerminalError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
        let (tx, rx) = mpsc::unbounded_channel();
        let current = session.cursor();
        let earliest = session.replay.earliest_cursor(current);
        let tail = session
            .replay
            .tail_from_cursor(cursor, current)
            .ok_or_else(|| TerminalError::CursorUnavailable {
                session_id: session_id.to_string(),
                requested: cursor,
                earliest,
                current,
            })?;
        for frame in tail {
            let _ = tx.send(frame);
        }
        session.subscribers.push(tx);
        tracing::debug!(
            event = "terminal_activity_subscribe",
            session_id,
            requested_activity = cursor.next_activity_sequence,
            requested_output = cursor.next_output_offset,
            current_activity = current.next_activity_sequence,
            current_output = current.next_output_offset,
            subscriber_count = session.subscribers.len(),
        );
        Ok(rx)
    }

    pub async fn send_input(
        &self,
        session_id: &str,
        viewer: &TerminalViewerIdentity,
        data: &[u8],
    ) -> Result<(), TerminalError> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
        if session.input_lease_holder.as_ref() != Some(viewer) {
            return Err(TerminalError::InputLeaseRequired);
        }
        session.control.write_input(data)
    }

    pub async fn adapt_pty_geometry(
        &self,
        session_id: &str,
        viewer: &TerminalViewerIdentity,
        geometry: TerminalPtyGeometry,
    ) -> Result<TerminalSessionSnapshot, TerminalError> {
        if geometry.cols == 0 || geometry.rows == 0 {
            return Err(TerminalError::Spawn(
                "terminal PTY geometry must be non-zero".to_string(),
            ));
        }
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
        if session.input_lease_holder.as_ref() != Some(viewer) {
            return Err(TerminalError::InputLeaseRequired);
        }
        if session.pty_geometry == geometry {
            tracing::debug!(
                event = "terminal_geometry_adapt",
                session_id,
                cols = geometry.cols,
                rows = geometry.rows,
                changed = false,
            );
            return Ok(session.snapshot());
        }
        session.control.resize_pty(geometry)?;
        session.pty_geometry = geometry;
        self.xterm_state.resize(session_id.to_string(), geometry);
        let frame = TerminalActivityFrame::PtyGeometryChanged {
            activity_sequence: session.take_activity_sequence(),
            geometry,
        };
        session.replay.push_frame(frame.clone());
        session
            .subscribers
            .retain(|subscriber| subscriber.send(frame.clone()).is_ok());
        let snapshot = session.snapshot();
        let scrollback_lines = session.scrollback_lines;
        tracing::debug!(
            event = "terminal_geometry_adapt",
            session_id,
            activity_sequence = frame.activity_sequence(),
            output_offset = session.next_output_offset,
            cols = geometry.cols,
            rows = geometry.rows,
            changed = true,
        );
        drop(sessions);
        self.publish_activity(session_id, frame, scrollback_lines)
            .await;
        Ok(snapshot)
    }

    pub async fn kill(
        &self,
        session_id: &str,
        viewer: &TerminalViewerIdentity,
    ) -> Result<(), TerminalError> {
        let (frame, scrollback_lines) = {
            let mut sessions = self.sessions.write().await;
            let current = sessions
                .get(session_id)
                .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
            if current.input_lease_holder.as_ref() != Some(viewer) {
                return Err(TerminalError::InputLeaseRequired);
            }
            let mut session = sessions
                .remove(session_id)
                .ok_or_else(|| TerminalError::NotFound(session_id.to_string()))?;
            let frame = TerminalActivityFrame::Exited {
                activity_sequence: session.take_activity_sequence(),
            };
            session
                .subscribers
                .retain(|subscriber| subscriber.send(frame.clone()).is_ok());
            session.control.kill();
            (frame, session.scrollback_lines)
        };
        self.xterm_state.dispose(session_id.to_string());
        if let Some(hub) = self.active_stream_broadcast_hub.as_ref() {
            publish_terminal_activity_frame(hub, session_id, &frame, scrollback_lines).await;
            hub.finish_terminal(&terminal_activity_stream_id(session_id), session_id)
                .await;
        }
        Ok(())
    }

    pub async fn shutdown_all(&self) {
        self.xterm_state.shutdown.cancel();
        let sessions = std::mem::take(&mut *self.sessions.write().await);
        for (_, session) in sessions {
            session.control.kill();
        }
        self.xterm_state.shutdown().await;
    }

    async fn publish_activity(
        &self,
        session_id: &str,
        frame: TerminalActivityFrame,
        scrollback_lines: usize,
    ) {
        let Some(hub) = self.active_stream_broadcast_hub.as_ref() else {
            return;
        };
        publish_terminal_activity_frame(hub, session_id, &frame, scrollback_lines).await;
    }

    fn spawn_output_pump(&self, session_id: String, mut output: mpsc::UnboundedReceiver<Vec<u8>>) {
        let sessions = Arc::clone(&self.sessions);
        let active_stream_broadcast_hub = self.active_stream_broadcast_hub.clone();
        let xterm_state = self.xterm_state.clone();
        tokio::spawn(async move {
            let mut pending_output = VecDeque::new();
            loop {
                let data = match pending_output.pop_front() {
                    Some(data) => data,
                    None => match output.recv().await {
                        Some(data) => data,
                        None => break,
                    },
                };
                let data = coalesce_terminal_output(data, &mut output, &mut pending_output);
                let mut sessions = sessions.write().await;
                let Some(session) = sessions.get_mut(&session_id) else {
                    return;
                };
                xterm_state.write(
                    session_id.clone(),
                    data.clone(),
                    Arc::clone(&session.control),
                );
                let frame = session.output_frame(data);
                if let TerminalActivityFrame::Output {
                    activity_sequence,
                    start_offset,
                    end_offset,
                    data,
                } = &frame
                {
                    tracing::trace!(
                        event = "terminal_output_frame",
                        session_id = %session_id,
                        activity_sequence = *activity_sequence,
                        start_offset = *start_offset,
                        end_offset = *end_offset,
                        byte_length = data.len(),
                    );
                }
                session.replay.push_frame(frame.clone());
                session
                    .subscribers
                    .retain(|subscriber| subscriber.send(frame.clone()).is_ok());
                let scrollback_lines = session.scrollback_lines;
                drop(sessions);
                if let Some(hub) = active_stream_broadcast_hub.as_ref() {
                    publish_terminal_activity_frame(hub, &session_id, &frame, scrollback_lines)
                        .await;
                }
            }
            let mut sessions = sessions.write().await;
            let Some(mut session) = sessions.remove(&session_id) else {
                return;
            };
            let frame = TerminalActivityFrame::Exited {
                activity_sequence: session.take_activity_sequence(),
            };
            xterm_state.dispose(session_id.clone());
            session
                .subscribers
                .retain(|subscriber| subscriber.send(frame.clone()).is_ok());
            let scrollback_lines = session.scrollback_lines;
            drop(sessions);
            if let Some(hub) = active_stream_broadcast_hub.as_ref() {
                publish_terminal_activity_frame(hub, &session_id, &frame, scrollback_lines).await;
                hub.finish_terminal(&terminal_activity_stream_id(&session_id), &session_id)
                    .await;
            }
        });
    }

    fn spawn_detached_cleanup(&self, session_id: String, generation: u64) {
        let sessions = Arc::clone(&self.sessions);
        let xterm_state = self.xterm_state.clone();
        let ttl = self.config.detached_ttl;
        tokio::spawn(async move {
            tokio::time::sleep(ttl).await;
            let mut sessions = sessions.write().await;
            let should_cleanup = sessions.get(&session_id).is_some_and(|session| {
                session.viewers.is_empty() && session.detached_generation == generation
            });
            if should_cleanup {
                if let Some(session) = sessions.remove(&session_id) {
                    session.control.kill();
                    xterm_state.dispose(session_id);
                }
            }
        });
    }
}
