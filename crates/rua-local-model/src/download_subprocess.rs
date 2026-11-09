use std::path::PathBuf;
use std::process::Stdio;

use rua_core::{Result, RuaError};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::download_ipc::IpcMessage;

pub struct DownloadSubprocess {
    child: Child,
    pub pid: u32,

    partial_path: Option<PathBuf>,
}

impl DownloadSubprocess {
    pub fn spawn(rua_binary: &str, model_id: &str) -> Result<Self> {
        let mut cmd = Command::new(rua_binary);
        cmd.arg("internal-download-task")
            .arg(model_id)
            .stdout(Stdio::piped())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        let child = cmd.spawn().map_err(|e| RuaError::Io {
            message: format!("failed to spawn download subprocess: {e}"),
        })?;

        let pid = child.id().ok_or_else(|| RuaError::Runtime {
            message: "download subprocess exited immediately".into(),
        })?;

        Ok(Self {
            child,
            pid,
            partial_path: None,
        })
    }

    pub fn set_partial_path(&mut self, path: PathBuf) {
        self.partial_path = Some(path);
    }

    pub async fn read_message(
        reader: &mut BufReader<tokio::process::ChildStdout>,
    ) -> Result<Option<IpcMessage>> {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| RuaError::Io {
                message: format!("failed to read from download subprocess: {e}"),
            })?;
        if n == 0 {
            return Ok(None);
        }
        let msg: IpcMessage = serde_json::from_str(line.trim()).map_err(|e| RuaError::Runtime {
            message: format!("invalid IPC JSON from subprocess: {e}: {line}"),
        })?;
        Ok(Some(msg))
    }

    pub fn take_stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.child.stdout.take()
    }

    pub async fn kill(&mut self) -> Result<()> {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;

        if let Some(ref path) = self.partial_path {
            if path.exists() {
                let _ = tokio::fs::remove_file(path).await;
            }
        }
        Ok(())
    }
}

impl Drop for DownloadSubprocess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}
