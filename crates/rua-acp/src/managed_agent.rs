#[cfg(unix)]
use std::io;
use std::process::Stdio;

use agent_client_protocol::schema::v1::{McpServer, McpServerStdio};
use agent_client_protocol::{ByteStreams, ConnectTo, LineDirection, Role};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

type DebugCallback = std::sync::Arc<dyn Fn(&str, LineDirection) + Send + Sync>;

#[derive(Clone)]
pub struct ManagedAcpAgent {
    stdio: McpServerStdio,
    debug_callback: Option<DebugCallback>,
    cwd: Option<std::path::PathBuf>,
    isolated_env: bool,
}

impl ManagedAcpAgent {
    pub fn new(server: McpServer) -> Result<Self, agent_client_protocol::Error> {
        match server {
            McpServer::Stdio(stdio) => Ok(Self {
                stdio,
                debug_callback: None,
                cwd: None,
                isolated_env: false,
            }),
            _ => Err(agent_client_protocol::util::internal_error(
                "only stdio ACP runners are supported",
            )),
        }
    }

    pub fn with_debug<F>(mut self, callback: F) -> Self
    where
        F: Fn(&str, LineDirection) + Send + Sync + 'static,
    {
        self.debug_callback = Some(std::sync::Arc::new(callback));
        self
    }

    pub(crate) fn with_process_context(
        mut self,
        cwd: Option<std::path::PathBuf>,
        isolated_env: bool,
    ) -> Self {
        self.cwd = cwd;
        self.isolated_env = isolated_env;
        self
    }

    fn spawn_process(&self) -> Result<ManagedChild, agent_client_protocol::Error> {
        let mut cmd = Command::new(&self.stdio.command);
        cmd.args(&self.stdio.args);
        cmd.kill_on_drop(true);
        if self.isolated_env {
            cmd.env_clear();
        }
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        for env_var in &self.stdio.env {
            cmd.env(&env_var.name, &env_var.value);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(unix)]
        {
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setpgid(0, 0) == 0 {
                        Ok(())
                    } else {
                        Err(io::Error::last_os_error())
                    }
                });
            }
        }

        let mut child = cmd
            .spawn()
            .map_err(agent_client_protocol::Error::into_internal_error)?;
        let stdin = child.stdin.take().ok_or_else(|| {
            agent_client_protocol::util::internal_error("Failed to open ACP runner stdin")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            agent_client_protocol::util::internal_error("Failed to open ACP runner stdout")
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            agent_client_protocol::util::internal_error("Failed to open ACP runner stderr")
        })?;
        let pid = child.id();

        Ok(ManagedChild {
            child,
            pid,
            stdin,
            stdout,
            stderr,
            debug_callback: self.debug_callback.clone(),
        })
    }
}

impl<R: Role> ConnectTo<R> for ManagedAcpAgent {
    async fn connect_to(
        self,
        client: impl ConnectTo<R::Counterpart>,
    ) -> Result<(), agent_client_protocol::Error> {
        let ManagedChild {
            child,
            pid,
            stdin,
            stdout,
            stderr,
            debug_callback,
        } = self.spawn_process()?;
        let guard = ManagedChildGuard { child, pid };

        let stderr_task = tokio::spawn(read_stderr(stderr, debug_callback));
        let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
        let result = ConnectTo::<R>::connect_to(transport, client).await;
        drop(guard);
        let _ = stderr_task.await;
        result
    }
}

async fn read_stderr(
    mut stderr: tokio::process::ChildStderr,
    debug_callback: Option<DebugCallback>,
) {
    let mut bytes = Vec::new();
    if stderr.read_to_end(&mut bytes).await.is_err() || bytes.is_empty() {
        return;
    }
    let text = String::from_utf8_lossy(&bytes);
    if let Some(callback) = debug_callback {
        for line in text.lines() {
            callback(line, LineDirection::Stderr);
        }
    }
}

struct ManagedChild {
    child: Child,
    pid: Option<u32>,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    debug_callback: Option<DebugCallback>,
}

struct ManagedChildGuard {
    child: Child,
    pid: Option<u32>,
}

impl Drop for ManagedChildGuard {
    fn drop(&mut self) {
        kill_process_group(self.pid);
        let _ = self.child.start_kill();
    }
}

#[cfg(unix)]
fn kill_process_group(pid: Option<u32>) {
    let Some(pid) = pid else {
        return;
    };
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: Option<u32>) {}
