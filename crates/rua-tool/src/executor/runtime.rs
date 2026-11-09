use std::process::Stdio;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchPlan, RuntimeLaunchRequest, ToolchainIntent,
};
use rua_core::runtime_layout::RuntimeLayout;
use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStdin, Command};
use tokio::sync::Mutex;

use crate::process_tree;
use crate::raw_capture::{capture_stream_with_observer, CapturedRawStream, RawCaptureObserver};

use super::live_output::ShellLiveOutputSource;
use super::{CommandError, CommandExecutor, CommandOutput, CommandRequest, RawCommandCapture};

pub use rua_core::runtime_layout::{NativeShellKind as ShellKind, NativeShellSpec as ShellSpec};

const STDIN_OPEN: u8 = 0;
const STDIN_CLOSED_BY_REQUEST: u8 = 1;
const STDIN_CLOSED_BY_PROCESS: u8 = 2;

enum ShellStdinState {
    Open(ChildStdin),
    ClosedByRequest,
    ClosedByProcess,
}

struct ShellStdinSink {
    state: Mutex<ShellStdinState>,
    status: std::sync::atomic::AtomicU8,
}

impl ShellStdinSink {
    fn new(stdin: ChildStdin) -> Self {
        Self {
            state: Mutex::new(ShellStdinState::Open(stdin)),
            status: std::sync::atomic::AtomicU8::new(STDIN_OPEN),
        }
    }
}

#[async_trait]
impl rua_task::TaskInputSink for ShellStdinSink {
    fn declared_capability(&self) -> rua_task::TaskInputCapability {
        rua_task::TaskInputCapability::Stdin
    }

    fn active_capability(&self) -> rua_task::TaskInputCapability {
        if self.status.load(std::sync::atomic::Ordering::SeqCst) == STDIN_OPEN {
            rua_task::TaskInputCapability::Stdin
        } else {
            rua_task::TaskInputCapability::None
        }
    }

    async fn send_input(
        &self,
        input: &rua_task::TaskInput,
    ) -> Result<(), rua_task::TaskInputError> {
        let mut state = self.state.lock().await;
        match input {
            rua_task::TaskInput::Text { text } => match &mut *state {
                ShellStdinState::Open(stdin) => {
                    if let Err(error) = stdin.write_all(text.as_bytes()).await {
                        *state = ShellStdinState::ClosedByProcess;
                        self.status
                            .store(STDIN_CLOSED_BY_PROCESS, std::sync::atomic::Ordering::SeqCst);
                        return Err(rua_task::TaskInputError::Rejected(error.to_string()));
                    }
                    Ok(())
                }
                ShellStdinState::ClosedByRequest | ShellStdinState::ClosedByProcess => {
                    Err(rua_task::TaskInputError::Unavailable)
                }
            },
            rua_task::TaskInput::CloseStdin => {
                let current = std::mem::replace(&mut *state, ShellStdinState::ClosedByProcess);
                match current {
                    ShellStdinState::Open(mut stdin) => {
                        if let Err(error) = stdin.shutdown().await {
                            self.status.store(
                                STDIN_CLOSED_BY_PROCESS,
                                std::sync::atomic::Ordering::SeqCst,
                            );
                            return Err(rua_task::TaskInputError::Rejected(error.to_string()));
                        }
                        *state = ShellStdinState::ClosedByRequest;
                        self.status
                            .store(STDIN_CLOSED_BY_REQUEST, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }
                    ShellStdinState::ClosedByRequest => {
                        *state = ShellStdinState::ClosedByRequest;
                        Ok(())
                    }
                    ShellStdinState::ClosedByProcess => Err(rua_task::TaskInputError::Unavailable),
                }
            }
        }
    }

    async fn close(&self) {
        let mut state = self.state.lock().await;
        if matches!(*state, ShellStdinState::Open(_)) {
            *state = ShellStdinState::ClosedByProcess;
            self.status
                .store(STDIN_CLOSED_BY_PROCESS, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

pub struct RuntimeCommandExecutor;

impl RuntimeCommandExecutor {
    pub fn new() -> Self {
        Self
    }

    pub fn shell_for_unix() -> ShellSpec {
        RuntimeLayout::native_shell_for_unix()
    }

    pub fn shell_for_windows(
        explicit_shell: Option<&std::ffi::OsStr>,
        path: Option<&std::ffi::OsStr>,
    ) -> ShellSpec {
        RuntimeLayout::native_shell_for_windows(explicit_shell, path)
    }

    #[doc(hidden)]
    pub fn windows_taskkill_args(pid: u32) -> Vec<String> {
        process_tree::windows_taskkill_args(pid)
    }

    #[doc(hidden)]
    pub fn shell_launch_plan_for_request(req: &CommandRequest) -> RuntimeLaunchPlan {
        RuntimeAuthority::from_current_process(None, &req.working_dir, std::env::temp_dir()).plan(
            ProcessShape::Shell,
            ToolchainIntent::Shell,
            RuntimeLaunchRequest {
                env: req.env.iter().cloned().collect(),
                env_remove: req.env_remove.iter().cloned().collect(),
                cwd: Some(req.working_dir.clone()),
                ..Default::default()
            },
        )
    }
}

impl Default for RuntimeCommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandExecutor for RuntimeCommandExecutor {
    async fn execute(
        &self,
        req: CommandRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<CommandOutput, CommandError> {
        let authority =
            RuntimeAuthority::from_current_process(None, &req.working_dir, std::env::temp_dir());
        let selected_shell = authority.inventory().shell.clone();
        let shell_plan = Self::shell_launch_plan_for_request(&req);
        if !shell_plan.diagnostics.is_empty() {
            return Err(CommandError::RuntimeUnavailable(
                shell_plan.diagnostics.join("; "),
            ));
        }
        let process = shell_plan
            .process()
            .map_err(|error| CommandError::RuntimeUnavailable(error.to_string()))?;
        let start = Instant::now();

        let task_session = req.task_session.clone();
        let mut cmd = Command::new(&process.command);
        cmd.args(&process.args)
            .arg(&req.command)
            .current_dir(process.cwd.as_ref().unwrap_or(&req.working_dir))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if task_session.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        if shell_plan.runtime_kind == rua_core::runtime_layout::RuntimeKind::LinuxCompat {
            cmd.env_clear();
        }
        cmd.kill_on_drop(true);

        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        for (k, v) in &process.env {
            cmd.env(k, v);
        }
        if cfg!(windows) && matches!(selected_shell.kind, ShellKind::Bash) {
            cmd.env("CHERE_INVOKING", "1");
            cmd.env("MSYS_NO_PATHCONV", "1");
        }
        for k in &req.env_remove {
            cmd.env_remove(k);
        }

        let child = cmd
            .spawn()
            .map_err(|e| CommandError::SpawnFailed(e.to_string()))?;
        let mut child = process_tree::ChildProcessTree::new(child);
        if let Some(session) = task_session.as_ref() {
            let stdin = child.stdin.take().ok_or_else(|| {
                CommandError::RuntimeUnavailable(
                    "interactive Shell process did not expose stdin".to_string(),
                )
            })?;
            if let Err(error) = session.activate(Arc::new(ShellStdinSink::new(stdin))).await {
                process_tree::kill_child_process_tree(&mut child).await;
                return Err(CommandError::RuntimeUnavailable(error.to_string()));
            }
        }

        let timeout_duration = req.timeout;
        let timeout = async move {
            if timeout_duration.is_zero() {
                std::future::pending::<()>().await;
            } else {
                tokio::time::sleep(timeout_duration).await;
            }
        };
        let mut cancel = cancel;

        tokio::pin!(timeout);

        let capture_id = req
            .raw_capture_id
            .as_deref()
            .map(sanitize_capture_id)
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let capture_dir = task_session
            .as_ref()
            .and_then(|session| session.output_dir())
            .unwrap_or_else(|| req.tool_result_store_dir.join(".raw-capture"));
        let stdout_path = capture_dir.join(format!("{capture_id}.stdout"));
        let stderr_path = capture_dir.join(format!("{capture_id}.stderr"));
        let live_output = if let Some(session) = task_session.as_ref() {
            let source = match ShellLiveOutputSource::new(
                capture_dir.join(format!("{capture_id}.live.jsonl")),
                Arc::downgrade(session),
            )
            .await
            {
                Ok(source) => Arc::new(source),
                Err(error) => {
                    process_tree::kill_child_process_tree(&mut child).await;
                    return Err(CommandError::RuntimeUnavailable(error.to_string()));
                }
            };
            if let Err(error) = session.activate_live_output(source.clone()).await {
                process_tree::kill_child_process_tree(&mut child).await;
                return Err(CommandError::RuntimeUnavailable(error.to_string()));
            }
            Some(source)
        } else {
            None
        };
        let stdout_cap = req.raw_capture_max_bytes.div_ceil(2);
        let stderr_cap = req.raw_capture_max_bytes / 2;
        let stdout_observer = live_output
            .as_ref()
            .map(|source| source.clone() as Arc<dyn RawCaptureObserver>);
        let stderr_observer = live_output
            .as_ref()
            .map(|source| source.clone() as Arc<dyn RawCaptureObserver>);
        let stdout_handle = child.stdout.take().map(|s| {
            tokio::spawn(capture_stream_with_observer(
                s,
                stdout_cap,
                stdout_path,
                stdout_observer,
                "stdout",
            ))
        });
        let stderr_handle = child.stderr.take().map(|s| {
            tokio::spawn(capture_stream_with_observer(
                s,
                stderr_cap,
                stderr_path,
                stderr_observer,
                "stderr",
            ))
        });

        let (timed_out, cancelled) = tokio::select! {
            _ = child.wait() => (false, false),
            _ = &mut timeout => (true, false),
            _ = wait_for_cancel(&mut cancel) => (false, true),
        };

        if timed_out || cancelled {
            process_tree::kill_child_process_tree(&mut child).await;
        }
        if let Some(session) = task_session.as_ref() {
            session.close_input().await;
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        let stdout_capture = match stdout_handle {
            Some(handle) => handle.await.unwrap_or_default(),
            None => CapturedRawStream::default(),
        };
        let stderr_capture = match stderr_handle {
            Some(handle) => handle.await.unwrap_or_default(),
            None => CapturedRawStream::default(),
        };
        let live_output_owned = task_session.is_some();
        if let Some(session) = task_session {
            session.seal_live_output().await;
        }

        let exit_code = if timed_out || cancelled {
            None
        } else {
            child
                .try_wait()
                .ok()
                .flatten()
                .map(|s| s.code().unwrap_or(-1))
        };

        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&stdout_capture.retained).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_capture.retained).into_owned(),
            exit_code,
            timed_out,
            cancelled,
            duration_ms,
            raw_capture: Some(RawCommandCapture {
                stdout_path: stdout_capture.path,
                stderr_path: stderr_capture.path,
                stdout_total_bytes: stdout_capture.total_bytes as u64,
                stderr_total_bytes: stderr_capture.total_bytes as u64,
                stdout_omitted_bytes: stdout_capture.omitted_bytes as u64,
                stderr_omitted_bytes: stderr_capture.omitted_bytes as u64,
                live_output_owned,
            }),
        })
    }

    fn executor_type(&self) -> &'static str {
        "runtime-authority"
    }
}

fn sanitize_capture_id(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(96)
        .collect()
}

async fn wait_for_cancel(cancel: &mut tokio::sync::watch::Receiver<bool>) {
    if *cancel.borrow() {
        return;
    }
    loop {
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
        if *cancel.borrow() {
            return;
        }
    }
}
