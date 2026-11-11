use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;

use rua_contract::Tool;
use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchPlan, RuntimeLaunchRequest, RuntimeResourceLimits,
    ToolchainIntent,
};
use rua_core::runtime_layout::RuntimeLayout;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::executor::{CommandError, CommandExecutor, CommandOutput, CommandRequest};
use crate::process_tree;
use crate::schema::ToolSpec;

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_CN_PYPI_INDEX: &str = "https://pypi.tuna.tsinghua.edu.cn/simple";

#[derive(Debug, Clone, Deserialize)]
pub struct SandboxLimits {
    pub memory_mb: u64,
    pub max_procs: u64,
    pub max_file_mb: u64,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            memory_mb: 256,
            // Linux counts every thread under the APK UID, including WebView and the resident.
            max_procs: if cfg!(target_os = "android") { 512 } else { 64 },
            max_file_mb: 10,
        }
    }
}

impl SandboxLimits {
    fn runtime_resource_limits(&self) -> RuntimeResourceLimits {
        RuntimeResourceLimits {
            address_space_bytes: self.memory_mb * 1024 * 1024,
            max_processes: self.max_procs,
            max_file_size_bytes: self.max_file_mb * 1024 * 1024,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Args {
    code: String,
    #[serde(default)]
    timeout_secs: Option<u64>,

    #[serde(default)]
    packages: Vec<String>,
}

pub struct ExecutePythonCodeTool {
    pub limits: SandboxLimits,
    executor: Option<Arc<dyn CommandExecutor>>,
}

impl ExecutePythonCodeTool {
    pub fn new() -> Self {
        Self {
            limits: SandboxLimits::default(),
            executor: None,
        }
    }

    pub fn with_limits(limits: SandboxLimits) -> Self {
        Self {
            limits,
            executor: None,
        }
    }

    pub fn with_executor(
        executor: Arc<dyn CommandExecutor>,
        limits: Option<SandboxLimits>,
    ) -> Self {
        Self {
            limits: limits.unwrap_or_default(),
            executor: Some(executor),
        }
    }

    pub fn spec() -> ToolSpec {
        ToolSpec {
            name: "python".to_string(),
            description: "Execute Python code in an isolated subprocess and return output."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python code to execute"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 30)"
                    },
                    "packages": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional Python package requirements to make available with uv --with"
                    }
                },
                "required": ["code"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: false,
                destructive: false,
                idempotent: false,
                requires_shell: true,
                requires_approval: ApprovalPolicy::UnlessTrusted,
                allowed_on_mobile: true,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }

    fn resolve_tmp_dir(ctx: &ToolContext) -> PathBuf {
        let dir = if Self::is_mobile_runtime() {
            Self::resolve_executor_dir(ctx)
        } else {
            std::env::temp_dir().join("rua-python-sandbox")
        };
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn resolve_executor_dir(ctx: &ToolContext) -> PathBuf {
        let base = if !ctx.working_dir.as_os_str().is_empty() {
            ctx.working_dir.clone()
        } else if Self::is_mobile_runtime() {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
        } else {
            std::env::temp_dir()
        };
        let dir = base.join(".rua-python-sandbox");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn is_mobile_runtime() -> bool {
        let layout = RuntimeLayout::from_current_process(None);
        layout.host_profile.is_mobile() || layout.os_family == "android"
    }

    fn python_launch_plan(
        args: Vec<String>,
        cwd: PathBuf,
        env: Vec<(String, String)>,
        resource_limits: Option<RuntimeResourceLimits>,
    ) -> RuntimeLaunchPlan {
        RuntimeAuthority::from_current_process(
            None,
            &cwd,
            std::env::temp_dir().join("rua-runtime-cache"),
        )
        .plan(
            ProcessShape::Direct,
            ToolchainIntent::Python,
            RuntimeLaunchRequest {
                args,
                env: env.into_iter().collect(),
                cwd: Some(cwd),
                resource_limits,
                ..Default::default()
            },
        )
    }

    fn python_shell_launch_plan(
        command: String,
        cwd: PathBuf,
        env: Vec<(String, String)>,
        resource_limits: Option<RuntimeResourceLimits>,
    ) -> RuntimeLaunchPlan {
        RuntimeAuthority::from_current_process(
            None,
            &cwd,
            std::env::temp_dir().join("rua-runtime-cache"),
        )
        .plan(
            ProcessShape::Direct,
            ToolchainIntent::GenericCommand {
                command: "/bin/sh".into(),
            },
            RuntimeLaunchRequest {
                args: vec!["-c".into(), command],
                env: env.into_iter().collect(),
                cwd: Some(cwd),
                resource_limits,
                ..Default::default()
            },
        )
    }

    #[doc(hidden)]
    pub fn runtime_authority_python_launch_plan(
        args: Vec<String>,
        cwd: PathBuf,
        env: Vec<(String, String)>,
    ) -> RuntimeLaunchPlan {
        Self::python_launch_plan(
            args,
            cwd,
            env,
            Some(SandboxLimits::default().runtime_resource_limits()),
        )
    }

    fn python_binary_for_executor(working_dir: &Path) -> rua_core::Result<String> {
        let plan =
            Self::python_launch_plan(Vec::new(), working_dir.to_path_buf(), Vec::new(), None);
        Ok(linux_compat_program_from_plan(&plan)
            .unwrap_or(plan.process()?.command.to_string_lossy().into_owned()))
    }

    fn python_executor_command(args: &Args, working_dir: &Path) -> rua_core::Result<String> {
        Ok(uv_python_command(
            &Self::python_binary_for_executor(working_dir)?,
            &args.code,
            &args.packages,
        ))
    }

    fn python_executor_env() -> Vec<(String, String)> {
        let mut env = Vec::new();
        env.push(("UV_PYTHON_DOWNLOADS".into(), "never".into()));
        if Self::is_mobile_runtime() {
            env.push(("UV_CACHE_DIR".into(), "/root/.cache/uv".into()));
            env.push(("RUA_PYTHON_VENV".into(), "/root/.venv/rua-mobile".into()));
            env.push(("PIP_ROOT_USER_ACTION".into(), "ignore".into()));
        }
        if let Some(index) = select_uv_default_index() {
            env.push(("UV_DEFAULT_INDEX".into(), index));
        }
        env
    }
}

impl Default for ExecutePythonCodeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ExecutePythonCodeTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutePythonCodeTool")
            .field("limits", &self.limits)
            .field(
                "executor",
                &self.executor.as_ref().map(|e| e.executor_type()),
            )
            .finish()
    }
}

#[async_trait]
impl Tool for ExecutePythonCodeTool {
    fn name(&self) -> &str {
        "python"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let args: Args = serde_json::from_value(args).map_err(|e| rua_core::RuaError::Tool {
            tool: "python".into(),
            message: e.to_string(),
        })?;

        let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        if Self::is_mobile_runtime() {
            if let Some(executor) = &self.executor {
                if executor.executor_type() == "mock" {
                    return self.call_via_executor(executor, args, timeout, ctx).await;
                }
            }
        }

        if !args.packages.is_empty() && !Self::is_mobile_runtime() {
            if let Some(executor) = &self.executor {
                return self.call_via_executor(executor, args, timeout, ctx).await;
            }
        }

        let tmp_dir = Self::resolve_tmp_dir(&ctx);
        let resource_limits = Some(self.limits.runtime_resource_limits());

        let plan = if Self::is_mobile_runtime() && !args.packages.is_empty() {
            Self::python_shell_launch_plan(
                uv_python_command("/usr/bin/python3", &args.code, &args.packages),
                tmp_dir.clone(),
                Self::python_executor_env(),
                resource_limits,
            )
        } else {
            Self::python_launch_plan(
                vec!["-I".into(), "-c".into(), args.code.clone()],
                tmp_dir.clone(),
                Vec::new(),
                resource_limits,
            )
        };
        if !plan.diagnostics.is_empty() {
            return Err(rua_core::RuaError::Tool {
                tool: "python".into(),
                message: plan.diagnostics.join("; "),
            });
        }
        #[cfg(unix)]
        let host_resource_limits = plan.host_resource_limits;
        let process = plan.process()?;
        let command_display = process.command.to_string_lossy().into_owned();
        let mut cmd = Command::new(&process.command);
        cmd.args(&process.args)
            .current_dir(process.cwd.as_ref().unwrap_or(&tmp_dir))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if plan.runtime_kind == rua_core::runtime_layout::RuntimeKind::LinuxCompat {
            cmd.env_clear();
        }
        cmd.envs(&process.env);
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

        #[cfg(unix)]
        if let Some(limits) = host_resource_limits {
            unsafe {
                cmd.pre_exec(move || {
                    use libc::{rlim_t, rlimit, setrlimit, RLIMIT_AS, RLIMIT_FSIZE, RLIMIT_NPROC};

                    let set_limit = |resource, value: rlim_t| -> std::io::Result<()> {
                        let lim = rlimit {
                            rlim_cur: value,
                            rlim_max: value,
                        };
                        if setrlimit(resource, &lim) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    };

                    let _ = set_limit(RLIMIT_AS, limits.address_space_bytes as rlim_t);
                    set_limit(RLIMIT_NPROC, limits.max_processes as rlim_t)?;
                    set_limit(RLIMIT_FSIZE, limits.max_file_size_bytes as rlim_t)?;
                    Ok(())
                });
            }
        }

        let child = cmd.spawn().map_err(|e| rua_core::RuaError::Tool {
            tool: "python".into(),
            message: format!("failed to spawn {command_display}: {e}"),
        })?;

        let mut child = process_tree::ChildProcessTree::new(child);
        let capture_id = uuid::Uuid::new_v4().to_string();
        let capture_dir = crate::result_store::archive_dir_for_context(&ctx)?.join(".raw-capture");
        let stdout_cap = ctx.raw_capture_max_bytes.div_ceil(2);
        let stderr_cap = ctx.raw_capture_max_bytes / 2;
        let stdout_handle = child.stdout.take().map(|stdout| {
            tokio::spawn(crate::raw_capture::capture_stream(
                stdout,
                stdout_cap,
                capture_dir.join(format!("{capture_id}.stdout")),
            ))
        });
        let stderr_handle = child.stderr.take().map(|stderr| {
            tokio::spawn(crate::raw_capture::capture_stream(
                stderr,
                stderr_cap,
                capture_dir.join(format!("{capture_id}.stderr")),
            ))
        });

        let mut cancel = ctx.cancel.clone();
        let cancelled = async {
            if let Some(cancel) = cancel.as_mut() {
                loop {
                    if *cancel.borrow_and_update() {
                        return;
                    }
                    if cancel.changed().await.is_err() {
                        break;
                    }
                }
            }
            std::future::pending::<()>().await;
        };
        let mut was_cancelled = false;
        let status = tokio::select! {
            biased;
            _ = cancelled => { was_cancelled = true; None }
            status = tokio::time::timeout(timeout, child.wait()) => status.ok(),
        };
        if status.is_none() {
            process_tree::kill_child_process_tree(&mut child).await;
        }
        let stdout_capture = match stdout_handle {
            Some(handle) => handle.await.unwrap_or_default(),
            None => crate::raw_capture::CapturedRawStream::default(),
        };
        let stderr_capture = match stderr_handle {
            Some(handle) => handle.await.unwrap_or_default(),
            None => crate::raw_capture::CapturedRawStream::default(),
        };
        let raw_capture = crate::executor::RawCommandCapture {
            stdout_path: stdout_capture.path,
            stderr_path: stderr_capture.path,
            stdout_total_bytes: stdout_capture.total_bytes as u64,
            stderr_total_bytes: stderr_capture.total_bytes as u64,
            stdout_omitted_bytes: stdout_capture.omitted_bytes as u64,
            stderr_omitted_bytes: stderr_capture.omitted_bytes as u64,
            live_output_owned: false,
        };

        match status {
            Some(Ok(status)) => {
                let stdout = String::from_utf8_lossy(&stdout_capture.retained).to_string();
                let stderr = String::from_utf8_lossy(&stderr_capture.retained).to_string();
                let exit_code = status.code().unwrap_or(-1);

                let limit_hint = detect_resource_limit_hint(&status, &stderr);
                let text = if let Some(hint) = &limit_hint {
                    format!("{hint}\n{}", format_output(&stdout, &stderr, exit_code))
                } else {
                    format_output(&stdout, &stderr, exit_code)
                };
                let is_error = exit_code != 0;

                let mut result = if is_error {
                    ToolResult::error("python", text)
                } else {
                    ToolResult::text("python", text)
                };

                let mut metadata = serde_json::Map::new();
                metadata.insert("exit_code".into(), serde_json::Value::from(exit_code));
                if limit_hint.is_some() {
                    metadata.insert("resource_limited".into(), serde_json::Value::Bool(true));
                }
                metadata.insert(
                    "raw_capture".into(),
                    serde_json::to_value(raw_capture).unwrap_or_default(),
                );
                result.metadata = metadata;

                Ok(result)
            }
            Some(Err(e)) => {
                remove_raw_command_capture(&raw_capture).await;
                Err(rua_core::RuaError::Tool {
                    tool: "python".into(),
                    message: format!("process error: {e}"),
                })
            }
            None => {
                let secs = timeout.as_secs();
                let text = if was_cancelled {
                    "Python execution was cancelled.".to_string()
                } else {
                    format!(
                        "Python execution timed out after {secs}s. \
                     The process was killed."
                    )
                };
                let mut result = ToolResult::error("python", text);
                let mut metadata = serde_json::Map::new();
                metadata.insert("exit_code".into(), serde_json::Value::from(-1));
                metadata.insert(
                    if was_cancelled {
                        "cancelled"
                    } else {
                        "timed_out"
                    }
                    .into(),
                    serde_json::Value::Bool(true),
                );
                metadata.insert(
                    "raw_capture".into(),
                    serde_json::to_value(raw_capture).unwrap_or_default(),
                );
                result.metadata = metadata;
                Ok(result)
            }
        }
    }
}

impl ExecutePythonCodeTool {
    async fn call_via_executor(
        &self,
        executor: &Arc<dyn CommandExecutor>,
        args: Args,
        timeout: Duration,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let working_dir = Self::resolve_executor_dir(&ctx);
        let command = Self::python_executor_command(&args, &working_dir)?;
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let req = CommandRequest {
            command,
            working_dir,
            tool_result_store_dir: crate::result_store::archive_dir_for_context(&ctx)?,
            timeout,
            env: Self::python_executor_env(),
            env_remove: Vec::new(),
            raw_capture_max_bytes: ctx.raw_capture_max_bytes,
            raw_capture_id: ctx
                .metadata
                .get("tool_use_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            task_session: None,
        };

        let output = executor
            .execute(req, ctx.cancel.unwrap_or(cancel_rx))
            .await
            .map_err(|e| rua_core::RuaError::Tool {
                tool: "python".into(),
                message: format_command_error(e),
            })?;

        Ok(tool_result_from_command_output(output, timeout))
    }
}

fn detect_resource_limit_hint(status: &std::process::ExitStatus, stderr: &str) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return match signal {
                libc::SIGKILL => Some(
                    "[sandbox] Process killed — likely exceeded memory limit (RLIMIT_AS)."
                        .to_string(),
                ),
                libc::SIGXFSZ => Some(
                    "[sandbox] Process killed — exceeded file size limit (RLIMIT_FSIZE)."
                        .to_string(),
                ),
                _ => None,
            };
        }
    }
    if stderr.contains("MemoryError") || stderr.contains("Cannot allocate memory") {
        return Some("[sandbox] Process hit memory limit (RLIMIT_AS).".to_string());
    }
    if stderr.contains("Resource temporarily unavailable") || stderr.contains("fork") {
        return Some("[sandbox] Process hit process limit (RLIMIT_NPROC).".to_string());
    }
    let _ = status;
    None
}

fn linux_compat_program_from_plan(plan: &RuntimeLaunchPlan) -> Option<String> {
    plan.process()
        .ok()?
        .args
        .iter()
        .find(|arg| arg.as_str() == "/usr/bin/python3")
        .cloned()
}

fn format_output(stdout: &str, stderr: &str, exit_code: i32) -> String {
    if exit_code == 0 {
        let mut text = if stdout.is_empty() {
            "Code executed successfully (no output).".to_string()
        } else {
            stdout.to_string()
        };
        if !stderr.is_empty() {
            text.push_str(&format!("\n[stderr]\n{stderr}"));
        }
        text
    } else {
        let mut text = format!("Python exited with code {exit_code}.");
        if !stdout.is_empty() {
            text.push_str(&format!("\n[stdout]\n{stdout}"));
        }
        if !stderr.is_empty() {
            text.push_str(&format!("\n[stderr]\n{stderr}"));
        }
        text
    }
}

fn tool_result_from_command_output(output: CommandOutput, timeout: Duration) -> ToolResult {
    if output.timed_out {
        let secs = timeout.as_secs();
        let text = format!(
            "Python execution timed out after {secs}s. \
             The process was killed."
        );
        let mut result = ToolResult::error("python", text);
        let mut metadata = serde_json::Map::new();
        metadata.insert("exit_code".into(), serde_json::Value::from(-1));
        metadata.insert("timed_out".into(), serde_json::Value::Bool(true));
        if let Some(raw_capture) = output.raw_capture {
            metadata.insert(
                "raw_capture".into(),
                serde_json::to_value(raw_capture).unwrap_or_default(),
            );
        }
        result.metadata = metadata;
        return result;
    }

    if output.cancelled {
        let mut result = ToolResult::error("python", "Python execution was cancelled.");
        let mut metadata = serde_json::Map::new();
        metadata.insert("exit_code".into(), serde_json::Value::from(-1));
        metadata.insert("cancelled".into(), serde_json::Value::Bool(true));
        if let Some(raw_capture) = output.raw_capture {
            metadata.insert(
                "raw_capture".into(),
                serde_json::to_value(raw_capture).unwrap_or_default(),
            );
        }
        result.metadata = metadata;
        return result;
    }

    let exit_code = output.exit_code.unwrap_or(-1);
    let text = format_output(&output.stdout, &output.stderr, exit_code);
    let mut result = if exit_code == 0 {
        ToolResult::text("python", text)
    } else {
        ToolResult::error("python", text)
    };
    let mut metadata = serde_json::Map::new();
    metadata.insert("exit_code".into(), serde_json::Value::from(exit_code));
    if let Some(raw_capture) = output.raw_capture {
        metadata.insert(
            "raw_capture".into(),
            serde_json::to_value(raw_capture).unwrap_or_default(),
        );
    }
    result.metadata = metadata;
    result
}

async fn remove_raw_command_capture(capture: &crate::executor::RawCommandCapture) {
    for path in [&capture.stdout_path, &capture.stderr_path]
        .into_iter()
        .filter_map(Option::as_ref)
    {
        let _ = tokio::fs::remove_file(path).await;
    }
}

fn format_command_error(error: CommandError) -> String {
    match error {
        CommandError::SpawnFailed(message) => format!("failed to spawn python command: {message}"),
        other => format!("process error: {other}"),
    }
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn uv_python_command(python_bin: &str, code: &str, packages: &[String]) -> String {
    let deps = packages
        .iter()
        .map(|dep| dep.trim())
        .filter(|dep| !dep.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        r#"PY={python}
CODE={code}
DEPS={deps}
if ! command -v uv >/dev/null 2>&1; then
  exec "$PY" -I -c "$CODE"
fi
VENV="${{RUA_PYTHON_VENV:-$PWD/.venv/rua-python}}"
export UV_CACHE_DIR="${{UV_CACHE_DIR:-$PWD/.uv-cache}}"
venv_python() {{
  if [ -x "$VENV/bin/python" ]; then
    printf '%s\n' "$VENV/bin/python"
  else
    printf '%s\n' "$PY"
  fi
}}
ensure_venv() {{
  if [ ! -x "$VENV/bin/python" ]; then
    mkdir -p "$(dirname "$VENV")"
    uv venv --python "$PY" "$VENV" >/dev/null
  fi
}}
install_deps() {{
  [ -n "$DEPS" ] || return 0
  ensure_venv
  set -- uv pip install --python "$VENV/bin/python"
  for dep in $DEPS; do
    set -- "$@" "$dep"
  done
  "$@"
}}
run_python() {{
  py="$(venv_python)"
  "$py" -I -c "$CODE"
}}
missing_package_from_stderr() {{
  "$PY" - "$1" <<'PY'
import re
import sys

with open(sys.argv[1], "r", encoding="utf-8", errors="replace") as handle:
    text = handle.read()

match = re.search(r"ModuleNotFoundError: No module named ['\"]([^'\"]+)['\"]", text)
if not match:
    raise SystemExit(0)

name = match.group(1).split(".", 1)[0]
aliases = {{
    "bs4": "beautifulsoup4",
    "cv2": "opencv-python",
    "PIL": "Pillow",
    "sklearn": "scikit-learn",
    "yaml": "PyYAML",
}}
print(aliases.get(name, name.replace("_", "-")))
PY
}}
err_file="${{TMPDIR:-/tmp}}/rua-python-exec-$$.stderr"
trap 'rm -f "$err_file"' EXIT
attempt=0
while :; do
  if install_deps 2>"$err_file" && run_python 2>"$err_file"; then
    if [ "$attempt" -gt 0 ]; then
      printf '[python-deps] resolved in shared uv venv with packages:%s\n' "$DEPS" >&2
    fi
    exit 0
  fi
  status=$?
  missing="$(missing_package_from_stderr "$err_file" || true)"
  if [ -z "$missing" ] || [ "$attempt" -ge 3 ]; then
    cat "$err_file" >&2
    exit "$status"
  fi
  case " $DEPS " in
    *" $missing "*)
      cat "$err_file" >&2
      exit "$status"
      ;;
  esac
  DEPS="${{DEPS:+$DEPS }}$missing"
  attempt=$((attempt + 1))
  printf '[python-deps] retrying with uv pip install %s\n' "$missing" >&2
done"#,
        python = shell_escape(python_bin),
        code = shell_escape(code),
        deps = shell_escape(&deps),
    )
}

fn select_uv_default_index() -> Option<String> {
    if let Some(value) = non_empty_env("RUA_PYTHON_UV_DEFAULT_INDEX") {
        return Some(value);
    }
    if let Some(value) = non_empty_env("UV_DEFAULT_INDEX") {
        return Some(value);
    }

    match non_empty_env("RUA_PYTHON_INDEX_REGION")
        .or_else(|| non_empty_env("RUA_PYTHON_INDEX_MODE"))
        .as_deref()
    {
        Some("cn") | Some("china") | Some("mainland") => Some(DEFAULT_CN_PYPI_INDEX.to_string()),
        Some("official") | Some("global") | Some("pypi") => None,
        Some("auto") | None => {
            if environment_looks_china_mainland() {
                Some(DEFAULT_CN_PYPI_INDEX.to_string())
            } else {
                None
            }
        }
        Some(url) if url.starts_with("http://") || url.starts_with("https://") => {
            Some(url.to_string())
        }
        Some(_) => None,
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn environment_looks_china_mainland() -> bool {
    ["TZ", "LANG", "LC_ALL", "LC_MESSAGES"]
        .into_iter()
        .filter_map(|key| std::env::var(key).ok())
        .any(|value| {
            let normalized = value.to_ascii_lowercase();
            normalized.contains("asia/shanghai")
                || normalized.contains("asia/chongqing")
                || normalized.contains("zh_cn")
                || normalized.contains("zh-cn")
                || normalized.ends_with(".cn")
        })
}
