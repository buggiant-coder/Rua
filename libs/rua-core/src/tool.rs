use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use agent_types::{ToolAttachment, ToolCall, ToolResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,

    pub parameters: serde_json::Value,
}

/// Trusted model capability projected into tool calls by the executing Agent context.
pub const MODEL_SUPPORTS_IMAGE_METADATA_KEY: &str = "model_supports_image";

/// Canonical tool disablements captured by the executing Chat/Task configuration.
pub const EFFECTIVE_DISABLED_TOOLS_METADATA_KEY: &str = "effective_disabled_tools";

pub const DEFAULT_TOOL_EXECUTION_OUTPUT_MAX_TOKENS: usize = 10_000;

pub const DEFAULT_TOOL_RAW_CAPTURE_MAX_BYTES: usize = 1024 * 1024;

pub const DEFAULT_TOOL_RECALL_SLICE_MAX_TOKENS: usize = 10_000;

pub const DEFAULT_TOOL_OLD_MAX_BYTES: usize = 3_000;

pub const DEFAULT_TOKEN_ESTIMATE_DIVISOR: f64 = 4.0;

/// Maximum UTF-8 text payload returned to a single foreground file read.
pub const MAX_SHARED_READ_TEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadPageContinuation {
    Lines { start_line: u64, limit: u64 },
    Bytes { byte_offset: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadPageByteRange {
    pub start: u64,
    pub end_exclusive: u64,
    pub total_bytes: u64,
}

/// Locations and tool-call identity are supplied by the original invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedReadPage {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ReadPageContinuation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_range: Option<ReadPageByteRange>,
}

impl SharedReadPage {
    pub fn is_valid(&self) -> bool {
        if self.text.len() > MAX_SHARED_READ_TEXT_BYTES || self.text.contains('\0') {
            return false;
        }
        match (&self.byte_range, &self.continuation) {
            (None, None) => true,
            (None, Some(ReadPageContinuation::Lines { start_line, limit })) => {
                !self.text.is_empty() && *start_line >= 2 && *limit > 0
            }
            (Some(range), next)
                if range.start <= range.end_exclusive
                    && range.end_exclusive <= range.total_bytes
                    && range.end_exclusive - range.start == self.text.len() as u64
                    && (!self.text.is_empty() || range.total_bytes == 0) =>
            {
                if range.end_exclusive == range.total_bytes {
                    next.is_none()
                } else {
                    range.start < range.end_exclusive
                        && matches!(next, Some(ReadPageContinuation::Bytes { byte_offset }) if *byte_offset == range.end_exclusive)
                }
            }
            _ => false,
        }
    }
}

pub enum DeviceReadCheck {
    Unavailable(String),
    Denied(String),
    ConfirmationRequired {
        permission_saved: bool,
    },
    Checked,
    /// The target checked the request and lacked authorization to deliver this file.
    FileNotAuthorized,
    /// The target attempted an authorized read and could not return a text page.
    FileUnavailable,
    Shared(SharedReadPage),
    Executed(Box<ToolResult>),
    OutcomeUnconfirmed(String),
}

/// Per-call timer notifications for the period spent waiting on a visible target approval.
#[derive(Clone)]
pub struct DeviceReadConfirmationWait(
    tokio::sync::mpsc::UnboundedSender<(bool, tokio::time::Instant)>,
);

#[must_use]
pub struct DeviceReadConfirmationWaitGuard(
    tokio::sync::mpsc::UnboundedSender<(bool, tokio::time::Instant)>,
);

impl DeviceReadConfirmationWait {
    pub fn timer(timeout: std::time::Duration) -> (Self, impl std::future::Future<Output = ()>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut deadline = tokio::time::Instant::now() + timeout;
        (Self(tx), async move {
            let mut started = None;
            loop {
                tokio::select! {
                    biased;
                    event = rx.recv() => match event {
                        Some((true, at)) if at >= deadline && started.is_none() => return,
                        Some((true, at)) => { started.get_or_insert(at); }
                        Some((false, at)) => {
                            if let Some(start) = started.take() { deadline += at - start; }
                        }
                        None => { tokio::time::sleep_until(deadline).await; return; }
                    },
                    _ = tokio::time::sleep_until(deadline), if started.is_none() => return,
                }
            }
        })
    }

    pub fn begin(&self) -> DeviceReadConfirmationWaitGuard {
        let _ = self.0.send((true, tokio::time::Instant::now()));
        DeviceReadConfirmationWaitGuard(self.0.clone())
    }
}

impl Drop for DeviceReadConfirmationWaitGuard {
    fn drop(&mut self) {
        let _ = self.0.send((false, tokio::time::Instant::now()));
    }
}

#[async_trait::async_trait]
pub trait DeviceReadAccess: Send + Sync {
    /// The foreground metadata check bounds its network requests and can wait for a user decision.
    fn manages_confirmation_wait(
        &self,
        _ctx: &ToolContext,
        _arguments: &serde_json::Value,
    ) -> bool {
        false
    }

    fn manages_operation_confirmation_wait(
        &self,
        tool: &str,
        ctx: &ToolContext,
        arguments: &serde_json::Value,
    ) -> bool {
        tool == "read" && self.manages_confirmation_wait(ctx, arguments)
    }

    async fn check_operation(
        &self,
        tool: &str,
        ctx: &ToolContext,
        arguments: &serde_json::Value,
    ) -> crate::Result<DeviceReadCheck> {
        if tool == "read" {
            self.check(ctx, arguments).await
        } else {
            Ok(DeviceReadCheck::Unavailable(format!(
                "{tool} is unavailable on this device"
            )))
        }
    }

    /// Ask the file's Device to check this foreground Chat's requested read.
    async fn check(
        &self,
        ctx: &ToolContext,
        arguments: &serde_json::Value,
    ) -> crate::Result<DeviceReadCheck>;
}

#[derive(Clone)]
pub struct ToolContext {
    pub agent_id: String,
    pub session_id: String,
    pub local_device_id: Option<String>,
    pub working_dir: std::path::PathBuf,

    pub conversation_store_dir: Option<std::path::PathBuf>,

    pub tool_result_store_dir: Option<std::path::PathBuf>,
    pub metadata: std::collections::HashMap<String, serde_json::Value>,

    pub cancel: Option<tokio::sync::watch::Receiver<bool>>,

    pub task_runtime: Option<Arc<dyn std::any::Any + Send + Sync>>,

    pub task_session: Option<Arc<dyn std::any::Any + Send + Sync>>,

    pub collaboration_runtime: Option<Arc<dyn std::any::Any + Send + Sync>>,

    pub chat_media_loader: Option<Arc<dyn crate::ChatMediaLoader>>,
    pub device_read_access: Option<Arc<dyn DeviceReadAccess>>,
    pub device_read_confirmation_wait: Option<DeviceReadConfirmationWait>,

    pub sync_transfer_runtime: Option<Arc<dyn std::any::Any + Send + Sync>>,

    pub event_tx: Option<tokio::sync::mpsc::Sender<agent_types::AgentEvent>>,

    pub execution_output_max_tokens: usize,

    pub raw_capture_max_bytes: usize,

    pub recall_slice_max_tokens: usize,

    pub old_max_bytes: usize,

    pub token_estimate_divisor: f64,
}

impl ToolContext {
    pub async fn open_chat_media_reference(
        &self,
        path: &str,
    ) -> crate::Result<Option<crate::ResolvedChatMedia>> {
        let Some(id) = crate::chat_media::reference_id(path) else {
            return Ok(None);
        };
        let loader = self
            .chat_media_loader
            .as_deref()
            .ok_or_else(|| crate::RuaError::Runtime {
                message: format!("Chat media reader is unavailable for {path}"),
            })?;
        loader
            .open(&crate::ChatAddress::new(self.session_id.as_str()), id)
            .await
            .map(Some)
    }
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            session_id: String::new(),
            local_device_id: None,
            working_dir: std::path::PathBuf::new(),
            conversation_store_dir: None,
            tool_result_store_dir: None,
            metadata: std::collections::HashMap::new(),
            cancel: None,
            task_runtime: None,
            task_session: None,
            collaboration_runtime: None,
            chat_media_loader: None,
            device_read_access: None,
            device_read_confirmation_wait: None,
            sync_transfer_runtime: None,
            event_tx: None,
            execution_output_max_tokens: DEFAULT_TOOL_EXECUTION_OUTPUT_MAX_TOKENS,
            raw_capture_max_bytes: DEFAULT_TOOL_RAW_CAPTURE_MAX_BYTES,
            recall_slice_max_tokens: DEFAULT_TOOL_RECALL_SLICE_MAX_TOKENS,
            old_max_bytes: DEFAULT_TOOL_OLD_MAX_BYTES,
            token_estimate_divisor: DEFAULT_TOKEN_ESTIMATE_DIVISOR,
        }
    }
}
