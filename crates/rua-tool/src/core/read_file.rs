use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_core::Result;
use tokio::fs;

use crate::annotations::ExtendedToolAnnotations;
use crate::context::{ToolExecutionContext, DEFAULT_READ_LINES};
use crate::error::ToolError;
use crate::schema::ToolSpec;

fn file_read_state() -> &'static Mutex<HashMap<PathBuf, SystemTime>> {
    static STATE: OnceLock<Mutex<HashMap<PathBuf, SystemTime>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn record_file_mtime(path: &std::path::Path, mtime: SystemTime) {
    if let Ok(canonical) = path.canonicalize() {
        if let Ok(mut state) = file_read_state().lock() {
            state.insert(canonical, mtime);
        }
    }
}

pub(crate) fn update_file_mtime(path: &std::path::Path, new_mtime: SystemTime) {
    if let Ok(canonical) = path.canonicalize() {
        if let Ok(mut state) = file_read_state().lock() {
            state.insert(canonical, new_mtime);
        }
    }
}

pub struct ReadFileTool;

impl ReadFileTool {
    pub fn new() -> Self {
        Self
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "read".to_string(),
            description: format!(
                "Read a line window from a text file. Each call returns at most `limit` lines \
                 (default {DEFAULT_READ_LINES}) with line numbers formatted as `line | text`; it \
                 does not read past that window. Explicit `limit` windows are returned as complete \
                 results. If the tool applies its default or runtime safety limit, the \
                 output starts with a continuation warning containing the exact read(..., \
                 start_line=..., limit=...) call to continue."
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the text file to read"
                    },
                    "start_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "First line to read (1-based, inclusive). Defaults to 1. Use the continuation hint's start_line to keep reading."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": format!("Maximum number of lines to return. Defaults to {DEFAULT_READ_LINES}; very large requests are capped by a runtime safety limit. Use smaller windows for large files.")
                    }
                },
                "required": ["file_path"]
            }),
            annotations: ExtendedToolAnnotations::read_only(),
            namespace: "builtin".into(),
        }
    }

    pub async fn execute_with_context(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecutionContext,
    ) -> std::result::Result<ToolResult, ToolError> {
        let file_path = args["file_path"]
            .as_str()
            .ok_or_else(|| ToolError::SchemaValidation {
                tool: "read".into(),
                message: "missing or invalid 'file_path' field".into(),
            })?;

        let start_line = match args.get("start_line") {
            Some(value) if !value.is_null() => match value.as_u64() {
                Some(n) => Some(n as usize),
                None => {
                    return Ok(ToolResult::error(
                        &ctx.trace_id,
                        "Error: start_line must be an integer.",
                    ));
                }
            },
            _ => None,
        };
        let limit = match args.get("limit") {
            Some(value) if !value.is_null() => match value.as_u64() {
                Some(n) => Some(n as usize),
                None => {
                    return Ok(ToolResult::error(
                        &ctx.trace_id,
                        "Error: limit must be an integer.",
                    ));
                }
            },
            _ => None,
        };
        if matches!(start_line, Some(0)) {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                "Error: start_line must be greater than or equal to 1.",
            ));
        }
        if matches!(limit, Some(0)) {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                "Error: limit must be greater than or equal to 1.",
            ));
        }
        if args.get("end_line").is_some() {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                "Error: read uses start_line and limit; end_line is not supported.",
            ));
        }

        let resolved = match ctx.workspace_fs.resolve(file_path) {
            Ok(path) => path,
            Err(ToolError::FileNotFound { .. }) => {
                return Ok(ToolResult::error(
                    &ctx.trace_id,
                    format!("Error: The file {file_path} does not exist."),
                ));
            }
            Err(e) => return Err(e),
        };

        let metadata = match fs::metadata(&resolved).await {
            Ok(m) => m,
            Err(_) => {
                return Ok(ToolResult::error(
                    &ctx.trace_id,
                    format!("Error: The file {file_path} does not exist."),
                ));
            }
        };

        if !metadata.is_file() {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                format!("Error: The path {file_path} is not a file."),
            ));
        }

        let raw_bytes = match fs::read(&resolved).await {
            Ok(b) => b,
            Err(e) => {
                return Ok(ToolResult::error(
                    &ctx.trace_id,
                    format!("Error: Read file failed due to \n{e}"),
                ));
            }
        };

        let check_len = raw_bytes.len().min(8192);
        if raw_bytes[..check_len].contains(&0) {
            return Err(ToolError::BinaryFile {
                path: PathBuf::from(file_path),
            });
        }

        let content = String::from_utf8_lossy(&raw_bytes);
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = start_line.map(|n| n.saturating_sub(1)).unwrap_or(0);
        let tool_applied_line_limit =
            limit.is_none() || limit.is_some_and(|requested| requested > ctx.limits.max_read_lines);
        let requested_limit = limit
            .unwrap_or(ctx.limits.default_read_lines)
            .min(ctx.limits.max_read_lines)
            .max(1);
        let end = start.saturating_add(requested_limit).min(total_lines);

        if start >= total_lines {
            return Ok(ToolResult::error(
                &ctx.trace_id,
                format!(
                    "Error: start_line {} exceeds file length ({total_lines} lines).",
                    start + 1
                ),
            ));
        }

        let mut output = String::new();
        let truncated = end < total_lines;

        for (i, line) in lines[start..end].iter().enumerate() {
            let line_num = start + i + 1;
            output.push_str(&format!("{line_num:>6} | {line}\n"));
        }

        let max_output_bytes = ctx.raw_capture_max_bytes.max(1);
        let total_bytes = raw_bytes.len();
        let mut truncation_meta: Option<agent_types::TruncationMeta> = None;
        if output.len() > max_output_bytes {
            let mut cut = max_output_bytes;
            while cut > 0 && !output.is_char_boundary(cut) {
                cut -= 1;
            }
            output.truncate(cut);

            let complete_lines = if let Some(last_newline) = output.rfind('\n') {
                output.truncate(last_newline + 1);
                output.matches('\n').count()
            } else {
                1
            };
            let next_start = start + complete_lines + 1;
            let shown_start = (start + 1) as u64;
            let shown_end = (start + complete_lines) as u64;
            let hint = format!(
                "read(file_path=\"{file_path}\", start_line={next_start}, limit={requested_limit})"
            );
            truncation_meta = Some(crate::result_store::continuation_meta(
                "read",
                Some(total_lines as u64),
                Some(total_bytes as u64),
                shown_start,
                shown_end,
                hint,
            ));
        } else if truncated && tool_applied_line_limit {
            let next_start = end + 1;
            let hint = format!(
                "read(file_path=\"{file_path}\", start_line={next_start}, limit={requested_limit})"
            );
            truncation_meta = Some(crate::result_store::continuation_meta(
                "read",
                Some(total_lines as u64),
                Some(total_bytes as u64),
                (start + 1) as u64,
                end as u64,
                hint,
            ));
        }

        let body = if let Some(ref meta) = truncation_meta {
            let mut s = crate::result_store::format_truncation_warning(meta);
            s.push_str(&output);
            s
        } else {
            output
        };

        if let Ok(mtime) = metadata.modified() {
            record_file_mtime(&resolved, mtime);
        }

        let mut result = ToolResult::text(&ctx.trace_id, body);
        result.truncation = truncation_meta;
        Ok(result)
    }
}

impl Default for ReadFileTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(&self, args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult> {
        let tool_result_store_dir = crate::result_store::archive_dir_for_context(&ctx)?;
        let cancel = ctx.cancel.unwrap_or_else(|| {
            let (_, rx) = tokio::sync::watch::channel(false);
            rx
        });

        let workspace_fs =
            Arc::new(crate::fs::WorkspaceFs::new(&ctx.working_dir).map_err(|e| {
                rua_core::RuaError::Tool {
                    tool: "read".into(),
                    message: e.to_string(),
                }
            })?);

        let exec_ctx = ToolExecutionContext {
            workspace_fs,
            tool_result_store_dir,
            working_dir: ctx.working_dir,
            session_id: ctx.session_id,
            user_id: String::new(),
            agent_id: ctx.agent_id,
            channel: "console".into(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            cancel,
            security_context: crate::context::SecurityContext::default(),
            executor: Arc::new(crate::executor::DisabledCommandExecutor::default()),
            task_session: None,
            limits: crate::context::ToolExecutionLimits::default(),

            execution_output_max_tokens: rua_core::tool::DEFAULT_TOOL_EXECUTION_OUTPUT_MAX_TOKENS,
            raw_capture_max_bytes: rua_core::tool::DEFAULT_TOOL_RAW_CAPTURE_MAX_BYTES,
            recall_slice_max_tokens: rua_core::tool::DEFAULT_TOOL_RECALL_SLICE_MAX_TOKENS,

            old_max_bytes: rua_core::tool::DEFAULT_TOOL_OLD_MAX_BYTES,

            token_estimate_divisor: rua_core::tool::DEFAULT_TOKEN_ESTIMATE_DIVISOR,
        };

        self.execute_with_context(args, &exec_ctx)
            .await
            .map_err(rua_core::RuaError::from)
    }
}
