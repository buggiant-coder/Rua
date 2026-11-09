use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::ToolContext;
use rua_tool::core::shell::windows_shell_mismatch_hint;
use rua_tool::{
    CommandError, CommandExecutor, CommandOutput, CommandRequest, MockCommandExecutor,
    MockResponse, ShellTool, ToolTransferPolicy,
};
use serde_json::json;
use tempfile::TempDir;

fn make_ctx(root: &TempDir) -> ToolContext {
    let tool_result_store_dir = root.path().join("tool-results");
    std::fs::create_dir_all(&tool_result_store_dir).expect("tool result store");
    ToolContext {
        agent_id: "agent".into(),
        session_id: "session".into(),
        working_dir: root.path().to_path_buf(),
        tool_result_store_dir: Some(tool_result_store_dir),
        metadata: HashMap::new(),
        cancel: None,
        task_runtime: None,
        ..Default::default()
    }
}

fn text(result: &rua_core::tool::ToolResult) -> String {
    result.text_content()
}

#[test]
fn shell_declares_optional_yield_wait_without_exposing_task_transfer() {
    let definition = ShellTool::spec().to_tool_definition();
    let contract = rua_tool::tool_transfer_contract(&definition);

    assert_eq!(contract.policy, ToolTransferPolicy::Transferable);
    assert_eq!(
        contract.input_capability,
        rua_task::TaskInputCapability::Stdin
    );
    assert_eq!(contract.wait_argument.as_deref(), Some("yield_seconds"));
    assert_eq!(contract.default_wait_seconds, Some(10));
    assert_eq!(
        rua_tool::foreground_wait_limit(&definition, &json!({})),
        Some(Duration::from_secs(10))
    );
    assert_eq!(
        rua_tool::foreground_wait_limit(&definition, &json!({"yield_seconds": 0})),
        Some(Duration::from_secs(10))
    );
    assert_eq!(
        rua_tool::foreground_wait_limit(&definition, &json!({"yield_seconds": 9_000})),
        Some(Duration::from_secs(9_000))
    );
    assert!(definition.parameters["properties"]["yield_seconds"]
        .get("maximum")
        .is_none());
    assert!(definition.parameters["properties"]
        .get("timeout_ms")
        .is_none());
    assert!(definition.parameters["properties"]
        .get("foreground_wait_seconds")
        .is_none());
    assert!(definition.parameters["properties"]["yield_seconds"]
        .get("default")
        .is_none());
    assert_eq!(definition.parameters["required"], json!(["command"]));
    let tool_description = definition.description.to_lowercase();
    let yield_description = definition.parameters["properties"]["yield_seconds"]["description"]
        .as_str()
        .unwrap()
        .to_lowercase();
    for hidden_term in ["task", "background", "transfer"] {
        assert!(!tool_description.contains(hidden_term));
        assert!(!yield_description.contains(hidden_term));
    }
    assert!(yield_description.contains("omit"));
    assert!(yield_description.contains("10"));
    assert!(tool_description.contains("external coding agents"));
    assert!(tool_description.contains("acp runner"));
}

#[test]
fn windows_shell_hint_only_appears_for_windows_posix_mismatch() {
    let output = CommandOutput {
        stdout: String::new(),
        stderr: "grep is not recognized as the name of a cmdlet".into(),
        exit_code: Some(1),
        timed_out: false,
        cancelled: false,
        duration_ms: 1,
        raw_capture: None,
    };

    let hint = windows_shell_mismatch_hint("grep TODO src", &output, true)
        .expect("Windows POSIX mismatch should produce a hint");
    assert!(hint.contains("Windows"));
    assert!(hint.contains("Git Bash"));

    assert!(
        windows_shell_mismatch_hint("grep TODO src", &output, false).is_none(),
        "non-Windows runs should not receive Windows shell advice"
    );
}

#[tokio::test]
async fn bash_formats_success_empty_stderr_and_failure_results() {
    let root = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(MockCommandExecutor::new());
    executor.add_response(
        "echo hello",
        MockResponse {
            stdout: "hello\n".into(),
            stderr: String::new(),
            exit_code: 0,
            delay_ms: 12,
        },
    );
    executor.add_response(
        "warn",
        MockResponse {
            stdout: "ok\n".into(),
            stderr: "careful\n".into(),
            exit_code: 0,
            delay_ms: 1,
        },
    );
    executor.add_response(
        "false",
        MockResponse {
            stdout: "before\n".into(),
            stderr: "error msg\n".into(),
            exit_code: 7,
            delay_ms: 2,
        },
    );
    let tool = ShellTool::new(executor);
    assert_eq!(tool.name(), "bash");

    let success = tool
        .call(json!({"command": "echo hello"}), make_ctx(&root))
        .await
        .expect("success");
    assert!(!success.is_error);
    assert_eq!(text(&success), "hello\n");
    assert_eq!(success.metadata["exit_code"], json!(0));
    assert_eq!(success.duration_ms, Some(12));

    let empty = tool
        .call(json!({"command": "unknown"}), make_ctx(&root))
        .await
        .expect("empty success");
    assert!(!empty.is_error);
    assert_eq!(text(&empty), "Command executed successfully (no output).");

    let stderr = tool
        .call(json!({"command": "warn"}), make_ctx(&root))
        .await
        .expect("stderr success");
    assert!(!stderr.is_error);
    assert_eq!(text(&stderr), "ok\n\n[stderr]\ncareful\n");

    let failed = tool
        .call(json!({"command": "false"}), make_ctx(&root))
        .await
        .expect("failure result");
    assert!(failed.is_error);
    let failed_text = text(&failed);
    assert!(failed_text.contains("Command failed with exit code 7."));
    assert!(failed_text.contains("[stdout]\nbefore\n"));
    assert!(failed_text.contains("[stderr]\nerror msg\n"));
    assert_eq!(failed.metadata["exit_code"], json!(7));
}

#[tokio::test]
async fn bash_preserves_newlines_and_resolves_cwd_without_an_execution_deadline() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(root.path().join("subdir")).expect("subdir");
    let executor = Arc::new(MockCommandExecutor::new());
    executor.add_response(
        "echo first\n  echo second",
        MockResponse {
            stdout: "first\nsecond\n".into(),
            stderr: String::new(),
            exit_code: 0,
            delay_ms: 0,
        },
    );
    executor.add_response(
        "sleepy",
        MockResponse {
            stdout: "done\n".into(),
            stderr: String::new(),
            exit_code: 0,
            delay_ms: 0,
        },
    );
    let tool = ShellTool::new(executor.clone());

    let multiline = tool
        .call(
            json!({
                "command": "echo first\n  echo second",
                "yield_seconds": 3,
                "cwd": "subdir"
            }),
            make_ctx(&root),
        )
        .await
        .expect("multiline command");
    assert!(!multiline.is_error, "got: {}", text(&multiline));

    let long_wait = tool
        .call(
            json!({"command": "sleepy", "yield_seconds": 50}),
            make_ctx(&root),
        )
        .await
        .expect("long foreground wait");
    assert!(!long_wait.is_error, "got: {}", text(&long_wait));

    let history = executor.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].command, "echo first\n  echo second");
    assert_eq!(history[0].timeout, Duration::ZERO);
    assert_eq!(
        history[0].working_dir,
        root.path()
            .join("subdir")
            .canonicalize()
            .expect("canonical cwd")
    );
    assert_eq!(history[1].timeout, Duration::ZERO);
}

#[tokio::test]
async fn bash_requires_integer_yield_seconds() {
    let root = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(MockCommandExecutor::new());
    executor.add_response(
        "string-timeout",
        MockResponse {
            stdout: "ok\n".into(),
            stderr: String::new(),
            exit_code: 0,
            delay_ms: 0,
        },
    );
    let tool = ShellTool::new(executor.clone());

    let valid = tool
        .call(
            json!({"command": "string-timeout seconds", "yield_seconds": 30}),
            make_ctx(&root),
        )
        .await
        .expect("integer foreground wait");
    assert!(!valid.is_error, "got: {}", text(&valid));

    let invalid = tool
        .call(
            json!({"command": "string-timeout invalid", "yield_seconds": "30"}),
            make_ctx(&root),
        )
        .await
        .expect_err("string foreground wait must be rejected");
    assert!(invalid.to_string().contains("yield_seconds"));

    let history = executor.history();
    assert_eq!(history[0].timeout, Duration::ZERO);
    assert_eq!(history.len(), 1);
}

#[tokio::test]
async fn bash_rejects_removed_wait_arguments() {
    let root = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(MockCommandExecutor::new());
    let tool = ShellTool::new(executor.clone());

    let error = tool
        .call(
            json!({"command": "echo old", "timeout_ms": 1_000}),
            make_ctx(&root),
        )
        .await
        .expect_err("removed timeout_ms argument must fail");

    let message = error.to_string();
    assert!(message.contains("timeout_ms"), "got: {message}");
    assert!(message.contains("yield_seconds"), "got: {message}");

    let error = tool
        .call(
            json!({"command": "echo old", "foreground_wait_seconds": 1}),
            make_ctx(&root),
        )
        .await
        .expect_err("removed foreground_wait_seconds argument must fail");
    let message = error.to_string();
    assert!(
        message.contains("foreground_wait_seconds"),
        "got: {message}"
    );
    assert!(message.contains("yield_seconds"), "got: {message}");
    assert!(executor.history().is_empty());
}

#[tokio::test]
async fn bash_preserves_script_bytes_for_the_shell_parser() {
    let root = tempfile::tempdir().unwrap();
    for command in [
        "echo first\n  echo second",
        "echo first\r\n  echo second",
        "echo 'first\nsecond'\n  echo after",
        "echo \"first\nsecond\"\n  echo after",
        "echo \\\n  continued",
        "if true; then\n  echo ok\nfi",
        "echo first &&\n  echo second",
        "echo first ||\n  echo fallback",
        "printf hi |\n  cat",
        "cat <<'PATCH'\n中文内容\nPATCH\n",
        "# comment\nprintf '完成'\n",
        "  printf 'keep whitespace'  \n",
    ] {
        let executor = Arc::new(MockCommandExecutor::new());
        executor.add_response(command, MockResponse::default());
        let tool = ShellTool::new(executor.clone());
        tool.call(json!({"command": command}), make_ctx(&root))
            .await
            .unwrap();
        assert_eq!(executor.history()[0].command, command);
    }
}

#[tokio::test]
async fn bash_requests_sensitive_environment_removal_from_executor() {
    let root = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(MockCommandExecutor::new());
    executor.add_response(
        "env-check",
        MockResponse {
            stdout: "ok\n".into(),
            stderr: String::new(),
            exit_code: 0,
            delay_ms: 0,
        },
    );
    let tool = ShellTool::new(executor.clone());

    let result = tool
        .call(json!({"command": "env-check"}), make_ctx(&root))
        .await
        .expect("shell tool call");

    assert!(!result.is_error, "got: {}", text(&result));
    let history = executor.history();
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].env_remove,
        vec![
            "AWS_SECRET_ACCESS_KEY".to_string(),
            "OPENAI_API_KEY".to_string(),
            "ANTHROPIC_API_KEY".to_string(),
        ],
        "Bash must ask the executor to scrub high-risk inherited credentials"
    );
}

#[tokio::test]
async fn bash_timeout_error_is_returned_as_tool_result() {
    let root = tempfile::tempdir().expect("tempdir");
    let tool = ShellTool::new(Arc::new(FixedExecutor::timeout(Duration::from_secs(3))));

    let result = tool
        .call(
            json!({"command": "sleep 30", "yield_seconds": 3}),
            make_ctx(&root),
        )
        .await
        .expect("timeout tool result");

    assert!(result.is_error);
    assert_eq!(result.metadata["exit_code"], json!(-1));
    assert_eq!(result.metadata["timed_out"], json!(true));
    assert!(text(&result).contains("timeout of 3 seconds"));
}

#[tokio::test]
async fn bash_disabled_executor_remains_transport_error_boundary() {
    let root = tempfile::tempdir().expect("tempdir");
    let tool = ShellTool::new(Arc::new(FixedExecutor::disabled(
        "shell disabled by profile",
    )));

    let err = tool
        .call(json!({"command": "echo nope"}), make_ctx(&root))
        .await
        .expect_err("disabled executor should escape as transport error");

    assert!(
        err.to_string().contains("shell disabled by profile"),
        "got: {err}"
    );
}

#[tokio::test]
async fn bash_long_output_returns_raw_result_before_shared_output_processing() {
    let root = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(MockCommandExecutor::new());
    executor.add_response(
        "long",
        MockResponse {
            stdout: (0..20)
                .map(|i| format!("line {i}: abcdefghijklmnopqrstuvwxyz\n"))
                .collect(),
            stderr: String::new(),
            exit_code: 0,
            delay_ms: 0,
        },
    );
    let tool = ShellTool::new(executor);

    let result = tool
        .call(json!({"command": "long"}), make_ctx(&root))
        .await
        .expect("long output");

    assert!(!result.is_error, "got: {}", text(&result));
    let output = text(&result);
    assert!(result.truncation.is_none());
    assert!(output.contains("line 0: abcdefghijklmnopqrstuvwxyz"));
    assert!(output.contains("line 19: abcdefghijklmnopqrstuvwxyz"));
    assert!(!output.contains("<truncated_output"), "got: {output}");
}

struct FixedExecutor {
    outcome: FixedOutcome,
}

enum FixedOutcome {
    Timeout(Duration),
    Disabled(String),
}

impl FixedExecutor {
    fn timeout(duration: Duration) -> Self {
        Self {
            outcome: FixedOutcome::Timeout(duration),
        }
    }

    fn disabled(reason: impl Into<String>) -> Self {
        Self {
            outcome: FixedOutcome::Disabled(reason.into()),
        }
    }
}

#[async_trait]
impl CommandExecutor for FixedExecutor {
    async fn execute(
        &self,
        _req: CommandRequest,
        _cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<CommandOutput, CommandError> {
        match &self.outcome {
            FixedOutcome::Timeout(duration) => Err(CommandError::Timeout(*duration)),
            FixedOutcome::Disabled(reason) => Err(CommandError::Disabled(reason.clone())),
        }
    }

    fn executor_type(&self) -> &'static str {
        "fixed"
    }
}
