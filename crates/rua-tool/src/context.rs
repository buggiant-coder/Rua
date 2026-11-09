use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::watch;

use crate::executor::CommandExecutor;
use crate::fs::WorkspaceFs;

pub const DEFAULT_READ_LINES: usize = 400;

pub const DEFAULT_MAX_READ_LINES: usize = 10_000;

#[derive(Debug, Clone)]
pub struct SecurityContext {
    pub trusted: bool,

    pub user_id: String,

    pub channel: String,
}

impl Default for SecurityContext {
    fn default() -> Self {
        Self {
            trusted: false,
            user_id: String::new(),
            channel: "console".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolExecutionLimits {
    pub max_edit_file_bytes: u64,

    pub default_read_lines: usize,

    pub max_read_lines: usize,
}

impl Default for ToolExecutionLimits {
    fn default() -> Self {
        Self {
            max_edit_file_bytes: 50 * 1024 * 1024,
            default_read_lines: DEFAULT_READ_LINES,
            max_read_lines: DEFAULT_MAX_READ_LINES,
        }
    }
}

#[derive(Clone)]
pub struct ToolExecutionContext {
    pub workspace_fs: Arc<WorkspaceFs>,

    pub working_dir: PathBuf,

    pub tool_result_store_dir: PathBuf,

    pub session_id: String,

    pub user_id: String,

    pub agent_id: String,

    pub channel: String,

    pub trace_id: String,

    pub cancel: watch::Receiver<bool>,

    pub security_context: SecurityContext,

    pub executor: Arc<dyn CommandExecutor>,

    pub task_session: Option<Arc<rua_task::TaskRunnerSession>>,

    pub limits: ToolExecutionLimits,

    pub execution_output_max_tokens: usize,

    pub raw_capture_max_bytes: usize,

    pub recall_slice_max_tokens: usize,

    pub old_max_bytes: usize,

    pub token_estimate_divisor: f64,
}

impl ToolExecutionContext {
    pub fn is_cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    pub fn to_core_context(&self) -> rua_core::tool::ToolContext {
        rua_core::tool::ToolContext {
            agent_id: self.agent_id.clone(),
            session_id: self.session_id.clone(),
            working_dir: self.working_dir.clone(),
            conversation_store_dir: None,
            tool_result_store_dir: Some(self.tool_result_store_dir.clone()),
            metadata: std::collections::HashMap::new(),
            cancel: Some(self.cancel.clone()),
            task_runtime: None,
            task_session: self.task_session.as_ref().map(|session| {
                Arc::new(crate::extended::task_tools::TaskRunnerSessionHandle::new(
                    Arc::clone(session),
                )) as Arc<dyn std::any::Any + Send + Sync>
            }),
            collaboration_runtime: None,
            sync_transfer_runtime: None,
            event_tx: None,
            execution_output_max_tokens: self.execution_output_max_tokens,
            raw_capture_max_bytes: self.raw_capture_max_bytes,
            recall_slice_max_tokens: self.recall_slice_max_tokens,
            old_max_bytes: self.old_max_bytes,
            token_estimate_divisor: self.token_estimate_divisor,
        }
    }
}
