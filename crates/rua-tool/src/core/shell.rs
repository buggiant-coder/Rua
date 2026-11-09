use std::time::Duration;

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_core::Result;

use crate::annotations::ExtendedToolAnnotations;
use crate::context::ToolExecutionContext;
use crate::executor::{CommandError, CommandExecutor, CommandOutput, CommandRequest};
use crate::schema::ToolSpec;

use std::sync::Arc;

pub struct ShellTool {
    executor: Arc<dyn CommandExecutor>,
}

impl ShellTool {
    pub fn new(executor: Arc<dyn CommandExecutor>) -> Self {
        Self { executor }
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "bash".to_string(),
            description: "Execute a shell command and return its output. Use the ACP runner integration, not this shell, to delegate work to external coding agents such as Codex, Claude Code, OpenCode, or Qwen Code.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "yield_seconds": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Wait before yielding output. Omit this parameter to use the 10 second default; zero also uses the default."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory for the command"
                    }
                },
                "required": ["command"]
            }),
            annotations: ExtendedToolAnnotations::shell(),
            namespace: "builtin".into(),
        }
    }

    pub async fn execute_with_context(
        &self,
        args: serde_json::Value,
        ctx: &ToolExecutionContext,
    ) -> std::result::Result<ToolResult, crate::error::ToolError> {
        validate_yield_seconds(&args)?;
        let raw_command =
            args["command"]
                .as_str()
                .ok_or_else(|| crate::error::ToolError::SchemaValidation {
                    tool: "bash".into(),
                    message: "missing or invalid 'command' field".into(),
                })?;
        let command = raw_command.to_owned();

        let working_dir = if let Some(cwd) = args["cwd"].as_str() {
            ctx.workspace_fs.resolve(cwd)?
        } else {
            ctx.working_dir.clone()
        };

        let req = CommandRequest {
            command: command.clone(),
            working_dir,
            tool_result_store_dir: ctx.tool_result_store_dir.clone(),
            timeout: Duration::ZERO,
            env: Vec::new(),
            env_remove: vec![
                "AWS_SECRET_ACCESS_KEY".into(),
                "OPENAI_API_KEY".into(),
                "ANTHROPIC_API_KEY".into(),
            ],
            raw_capture_max_bytes: ctx.raw_capture_max_bytes,
            raw_capture_id: Some(ctx.trace_id.clone()),
            task_session: ctx.task_session.clone(),
        };

        let output = self.executor.execute(req, ctx.cancel.clone()).await;

        match output {
            Ok(output) => {
                let call_id = ctx.trace_id.clone();
                let formatted = format_shell_output(&command, &output);

                let mut metadata = serde_json::Map::new();
                if let Some(code) = output.exit_code {
                    metadata.insert("exit_code".into(), serde_json::Value::from(code));
                }
                metadata.insert(
                    "duration_ms".into(),
                    serde_json::Value::from(output.duration_ms),
                );
                if output.timed_out {
                    metadata.insert("timed_out".into(), serde_json::Value::Bool(true));
                }
                if output.cancelled {
                    metadata.insert("cancelled".into(), serde_json::Value::Bool(true));
                }
                if let Some(raw_capture) = output.raw_capture.as_ref() {
                    if raw_capture.live_output_owned {
                        if let Some(path) = raw_capture.stdout_path.as_ref() {
                            metadata.insert(
                                "live_output_path".into(),
                                serde_json::json!(path.with_extension("live.jsonl")),
                            );
                        }
                    }
                    metadata.insert(
                        "raw_capture".into(),
                        serde_json::to_value(raw_capture).unwrap_or_default(),
                    );
                }

                let is_error = output.timed_out || output.cancelled || !output.success();

                let mut result = if is_error {
                    ToolResult::error(call_id, formatted)
                } else {
                    ToolResult::text(call_id, formatted)
                };
                result.duration_ms = Some(output.duration_ms);
                result.metadata = metadata;

                Ok(result)
            }
            Err(CommandError::Disabled(reason)) => {
                Err(crate::error::ToolError::ExecutorDisabled { reason })
            }
            Err(CommandError::Cancelled) => Err(crate::error::ToolError::CommandCancelled),
            Err(CommandError::Timeout(d)) => {
                let timeout_secs = d.as_secs();
                let text = format!(
                    "Command failed with exit code -1.\n\
                     [stderr]\n\
                     \u{26a0}\u{fe0f} TimeoutError: The command execution exceeded \
                     the timeout of {timeout_secs} seconds. \
                     Please consider increasing the timeout value if this command \
                     requires more time to complete."
                );
                let call_id = ctx.trace_id.clone();
                let mut result = ToolResult::error(call_id, text);
                let mut metadata = serde_json::Map::new();
                metadata.insert("exit_code".into(), serde_json::Value::from(-1));
                metadata.insert("timed_out".into(), serde_json::Value::Bool(true));
                result.metadata = metadata;
                Ok(result)
            }
            Err(e) => Err(crate::error::ToolError::CommandFailed {
                message: e.to_string(),
            }),
        }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "bash"
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
        let task_session = crate::extended::task_tools::get_task_runner_session(&ctx);
        let cancel = ctx.cancel.unwrap_or_else(|| {
            let (_, rx) = tokio::sync::watch::channel(false);
            rx
        });

        let workspace_fs =
            Arc::new(crate::fs::WorkspaceFs::new(&ctx.working_dir).map_err(|e| {
                rua_core::RuaError::Tool {
                    tool: "bash".into(),
                    message: e.to_string(),
                }
            })?);

        let trace_id = ctx
            .metadata
            .get("tool_use_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let exec_ctx = ToolExecutionContext {
            workspace_fs,
            tool_result_store_dir,
            working_dir: ctx.working_dir,
            session_id: ctx.session_id,
            user_id: String::new(),
            agent_id: ctx.agent_id,
            channel: "console".into(),
            trace_id,
            cancel,
            security_context: crate::context::SecurityContext::default(),
            executor: self.executor.clone(),
            task_session,
            limits: crate::context::ToolExecutionLimits::default(),
            execution_output_max_tokens: ctx.execution_output_max_tokens,
            raw_capture_max_bytes: ctx.raw_capture_max_bytes,
            recall_slice_max_tokens: ctx.recall_slice_max_tokens,
            old_max_bytes: ctx.old_max_bytes,
            token_estimate_divisor: ctx.token_estimate_divisor,
        };

        self.execute_with_context(args, &exec_ctx)
            .await
            .map_err(rua_core::RuaError::from)
    }
}

fn validate_yield_seconds(
    args: &serde_json::Value,
) -> std::result::Result<(), crate::error::ToolError> {
    if args.get("timeout_ms").is_some() {
        return Err(crate::error::ToolError::SchemaValidation {
            tool: "bash".into(),
            message: "'timeout_ms' was removed; use optional integer 'yield_seconds'".into(),
        });
    }
    if args.get("foreground_wait_seconds").is_some() {
        return Err(crate::error::ToolError::SchemaValidation {
            tool: "bash".into(),
            message: "'foreground_wait_seconds' was removed; use optional integer 'yield_seconds'"
                .into(),
        });
    }

    match args.get("yield_seconds") {
        None | Some(serde_json::Value::Null) => Ok(()),
        Some(value) if value.as_u64().is_some() => Ok(()),
        Some(_) => Err(crate::error::ToolError::SchemaValidation {
            tool: "bash".into(),
            message: "'yield_seconds' must be a non-negative integer".into(),
        }),
    }
}

fn format_shell_output(command: &str, output: &CommandOutput) -> String {
    if output.timed_out {
        let timeout_msg = "\u{26a0}\u{fe0f} TimeoutError: The command executor reported a timeout.";
        let stderr_with_timeout = if output.stderr.is_empty() {
            timeout_msg.to_string()
        } else {
            format!("{}\n{}", output.stderr, timeout_msg)
        };
        let exit_code = output.exit_code.unwrap_or(-1);
        let mut text = format!("Command failed with exit code {exit_code}.");
        if !output.stdout.is_empty() {
            text.push_str(&format!("\n[stdout]\n{}", output.stdout));
        }
        text.push_str(&format!("\n[stderr]\n{}", stderr_with_timeout));
        append_windows_shell_hint(text, command, output)
    } else if output.success() {
        let mut text = if output.stdout.is_empty() {
            "Command executed successfully (no output).".to_string()
        } else {
            output.stdout.clone()
        };
        if !output.stderr.is_empty() {
            text.push_str(&format!("\n[stderr]\n{}", output.stderr));
        }
        text
    } else {
        let exit_code = output.exit_code.unwrap_or(-1);
        let mut text = format!("Command failed with exit code {exit_code}.");
        if !output.stdout.is_empty() {
            text.push_str(&format!("\n[stdout]\n{}", output.stdout));
        }
        if !output.stderr.is_empty() {
            text.push_str(&format!("\n[stderr]\n{}", output.stderr));
        }
        append_windows_shell_hint(text, command, output)
    }
}

fn append_windows_shell_hint(mut text: String, command: &str, output: &CommandOutput) -> String {
    if let Some(hint) = windows_shell_mismatch_hint(command, output, cfg!(windows)) {
        text.push_str("\n\n");
        text.push_str(&hint);
    }
    text
}

pub fn windows_shell_mismatch_hint(
    command: &str,
    output: &CommandOutput,
    is_windows: bool,
) -> Option<String> {
    if !is_windows || output.success() || output.cancelled || output.timed_out {
        return None;
    }

    let command_lower = command.to_ascii_lowercase();
    let combined = output.combined_output().to_ascii_lowercase();
    let shell_reported_missing = combined.contains("command not found")
        || combined.contains("is not recognized as an internal or external command")
        || combined.contains("is not recognized as the name of a cmdlet")
        || combined.contains("the term '")
            && combined.contains("is not recognized")
            && combined.contains("cmdlet");

    let looks_posix = command_looks_posix_or_linux_only(&command_lower);
    if !shell_reported_missing && !looks_posix {
        return None;
    }

    Some(
        "Hint: 当前运行在 Windows 环境，这条命令看起来使用了当前 shell 不支持的 \
         Linux/POSIX 命令或语法。请改用 PowerShell/cmd 等价命令，或安装 Git Bash \
         后重试；如果确实需要完整 Linux 用户态，请显式设置 RUA_WINDOWS_SHELL=wsl.exe。"
            .to_string(),
    )
}

fn command_looks_posix_or_linux_only(command: &str) -> bool {
    let linux_tokens = [
        "apt ",
        "apt-get ",
        "systemctl ",
        "service ",
        "sudo ",
        "chmod ",
        "chown ",
        "grep ",
        "sed ",
        "awk ",
        "find ",
        "xargs ",
        "which ",
        "source ",
        "export ",
        "/etc/",
        "/proc/",
        "/bin/",
        "/usr/bin/",
        "/dev/null",
    ];
    linux_tokens.iter().any(|token| command.contains(token))
        || command.contains(" 2>/dev/null")
        || command.starts_with("export ")
        || command.starts_with("source ")
        || command
            .split_whitespace()
            .next()
            .is_some_and(|head| head.contains('=') && !head.starts_with('$'))
}
