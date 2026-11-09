use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct SubprocessConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub startup_timeout: Duration,
}

pub struct McpSubprocess {
    child: Option<Child>,
    pub stdout: Option<BufReader<tokio::process::ChildStdout>>,
    pub stdin: Option<tokio::process::ChildStdin>,
    stderr_task: Option<tokio::task::JoinHandle<Vec<String>>>,
}

impl std::fmt::Debug for McpSubprocess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpSubprocess")
            .field("has_child", &self.child.is_some())
            .field("has_stdout", &self.stdout.is_some())
            .field("has_stdin", &self.stdin.is_some())
            .finish()
    }
}

impl McpSubprocess {
    pub fn spawn(config: &SubprocessConfig) -> Result<Self, SubprocessError> {
        let command = crate::path_expand::expand_tilde(&config.command);
        let mut cmd = Command::new(&command);
        for arg in &config.args {
            cmd.arg(crate::path_expand::expand_tilde(arg));
        }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        for (k, v) in &config.env {
            cmd.env(k, v);
        }

        if let Some(ref cwd) = config.cwd {
            let cwd_str = cwd.to_string_lossy();
            if !cwd_str.is_empty() {
                let expanded = crate::path_expand::expand_tilde(&cwd_str);
                cmd.current_dir(PathBuf::from(expanded));
            }
        }

        #[cfg(unix)]
        unsafe {
            #[allow(unused_imports)]
            use std::os::unix::process::CommandExt as _;
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SubprocessError::CommandNotFound {
                    command: config.command.clone(),
                }
            } else {
                SubprocessError::SpawnFailed {
                    message: format!("failed to spawn '{}': {e}", config.command),
                }
            }
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SubprocessError::SpawnFailed {
                message: "failed to capture stdout".into(),
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SubprocessError::SpawnFailed {
                message: "failed to capture stdin".into(),
            })?;

        let stderr_task = child.stderr.take().map(|stderr| {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                let mut tail = Vec::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    tail.push(line);
                    if tail.len() > 20 {
                        tail.remove(0);
                    }
                }
                tail
            })
        });

        Ok(Self {
            child: Some(child),
            stdout: Some(BufReader::new(stdout)),
            stdin: Some(stdin),
            stderr_task,
        })
    }

    pub async fn kill(&mut self) {
        self.stdin = None;
        self.stdout = None;

        if let Some(mut child) = self.child.take() {
            Self::kill_child(&mut child).await;
        }

        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }

    pub async fn wait_or_kill(&mut self, timeout: Duration) {
        if let Some(ref mut child) = self.child {
            match tokio::time::timeout(timeout, child.wait()).await {
                Ok(Ok(_)) => {
                    self.child = None;
                    return;
                }
                Ok(Err(e)) => {
                    warn!(error = %e, "error waiting for subprocess");
                }
                Err(_) => {
                    warn!(
                        timeout_ms = timeout.as_millis(),
                        "subprocess did not exit in time, killing"
                    );
                }
            }
        }
        self.kill().await;
    }

    pub fn has_child(&self) -> bool {
        self.child.is_some()
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(|c| c.id())
    }

    pub async fn stderr_tail(&mut self) -> Vec<String> {
        if let Some(task) = self.stderr_task.take() {
            task.await.unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    async fn kill_child(child: &mut Child) {
        crate::process_tree::kill_child_process_tree(child).await;
    }
}

impl Drop for McpSubprocess {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            crate::process_tree::start_kill_child_process_tree(child);
        }
    }
}

#[derive(Debug, Clone)]
pub enum SubprocessError {
    CommandNotFound { command: String },
    SpawnFailed { message: String },
}

impl std::fmt::Display for SubprocessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CommandNotFound { command } => write!(f, "command not found: {command}"),
            Self::SpawnFailed { message } => write!(f, "subprocess spawn failed: {message}"),
        }
    }
}

impl std::error::Error for SubprocessError {}
