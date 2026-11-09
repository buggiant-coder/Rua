mod disabled;
mod live_output;
pub(crate) use live_output::plain_text;
pub(crate) use live_output::ShellLiveOutputSource;
mod mock;
mod runtime;

pub use disabled::DisabledCommandExecutor;
pub use mock::{MockCommandExecutor, MockResponse};
pub use runtime::{RuntimeCommandExecutor, ShellKind, ShellSpec};

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct CommandRequest {
    pub command: String,

    pub working_dir: PathBuf,

    pub tool_result_store_dir: PathBuf,

    pub timeout: Duration,

    pub env: Vec<(String, String)>,

    pub env_remove: Vec<String>,

    pub raw_capture_max_bytes: usize,

    pub raw_capture_id: Option<String>,

    pub task_session: Option<std::sync::Arc<rua_task::TaskRunnerSession>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandOutput {
    pub stdout: String,

    pub stderr: String,

    pub exit_code: Option<i32>,

    pub timed_out: bool,

    pub cancelled: bool,

    pub duration_ms: u64,

    #[serde(default)]
    pub raw_capture: Option<RawCommandCapture>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawCommandCapture {
    pub stdout_path: Option<PathBuf>,
    pub stderr_path: Option<PathBuf>,
    pub stdout_total_bytes: u64,
    pub stderr_total_bytes: u64,
    pub stdout_omitted_bytes: u64,
    pub stderr_omitted_bytes: u64,
    #[serde(default)]
    pub live_output_owned: bool,
}

impl CommandOutput {
    pub fn combined_output(&self) -> String {
        [self.stdout.as_str(), self.stderr.as_str()]
            .into_iter()
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("command timed out after {0:?}")]
    Timeout(Duration),

    #[error("command was cancelled")]
    Cancelled,

    #[error("executor disabled: {0}")]
    Disabled(String),

    #[error("failed to spawn process: {0}")]
    SpawnFailed(String),

    #[error("runtime unavailable: {0}")]
    RuntimeUnavailable(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[async_trait]
pub trait CommandExecutor: Send + Sync {
    async fn execute(
        &self,
        req: CommandRequest,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<CommandOutput, CommandError>;

    fn executor_type(&self) -> &'static str;
}

pub fn default_command_executor() -> std::sync::Arc<dyn CommandExecutor> {
    std::sync::Arc::new(RuntimeCommandExecutor::new())
}
