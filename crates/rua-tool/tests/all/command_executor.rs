use std::time::Duration;

use rua_tool::{
    raw_capture::RawCaptureBuffer, CommandError, CommandExecutor, CommandOutput, CommandRequest,
    RuntimeCommandExecutor,
};
use tempfile::TempDir;
#[cfg(unix)]
use tokio::sync::{Mutex, MutexGuard};

#[cfg(unix)]
static ENV_TEST_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

#[cfg(unix)]
async fn env_test_guard() -> MutexGuard<'static, ()> {
    ENV_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await
}

#[cfg(unix)]
struct EnvGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

#[cfg(unix)]
impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

#[cfg(unix)]
impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = self.previous.take() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

fn request(command: impl Into<String>, working_dir: &std::path::Path) -> CommandRequest {
    CommandRequest {
        command: command.into(),
        working_dir: working_dir.to_path_buf(),
        tool_result_store_dir: working_dir.join("tool_call_logs"),
        timeout: Duration::from_secs(5),
        env: Vec::new(),
        env_remove: Vec::new(),
        raw_capture_max_bytes: 1024 * 1024,
        raw_capture_id: None,
        task_session: None,
    }
}

fn cancel_rx() -> tokio::sync::watch::Receiver<bool> {
    let (_tx, rx) = tokio::sync::watch::channel(false);
    rx
}

fn cancel_pair() -> (
    tokio::sync::watch::Sender<bool>,
    tokio::sync::watch::Receiver<bool>,
) {
    tokio::sync::watch::channel(false)
}

#[test]
fn raw_capture_buffer_retains_bounded_head_and_tail() {
    let mut capture = RawCaptureBuffer::new(16);
    capture.push(b"abcdefgh");
    capture.push(b"ijklmnop");
    capture.push(b"qrstuvwxyz");

    assert_eq!(capture.total_bytes(), 26);
    assert_eq!(capture.retained_bytes(), 16);
    assert_eq!(capture.omitted_bytes(), 10);
    let visible = String::from_utf8(capture.to_bytes_with_marker()).unwrap();
    assert!(visible.starts_with("abcdefgh"));
    assert!(visible.ends_with("stuvwxyz"));
    assert!(visible.contains("10 raw bytes omitted"));
}

#[tokio::test]
async fn failed_raw_capture_does_not_advertise_a_complete_archive() {
    let tmp = TempDir::new().unwrap();
    let parent_file = tmp.path().join("not-a-directory");
    tokio::fs::write(&parent_file, b"keep").await.unwrap();
    let capture = rua_tool::raw_capture::capture_stream(
        &b"still readable"[..],
        1024,
        parent_file.join("output"),
    )
    .await;
    assert!(capture.path.is_none());
    assert_eq!(capture.retained, b"still readable");
    assert_eq!(tokio::fs::read(parent_file).await.unwrap(), b"keep");
}

#[tokio::test]
async fn raw_stream_error_does_not_advertise_a_complete_archive() {
    struct BrokenReader;
    impl tokio::io::AsyncRead for BrokenReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other("fixture stream error")))
        }
    }
    let tmp = TempDir::new().unwrap();
    let capture =
        rua_tool::raw_capture::capture_stream(BrokenReader, 1024, tmp.path().join("output")).await;
    assert!(capture.path.is_none());
}

#[test]
fn command_output_combines_stdout_and_stderr_without_extra_trailing_marker() {
    let stdout_only = CommandOutput {
        stdout: "stdout line\n".into(),
        stderr: String::new(),
        exit_code: Some(0),
        timed_out: false,
        cancelled: false,
        duration_ms: 1,
        raw_capture: None,
    };
    assert_eq!(stdout_only.combined_output(), "stdout line");
    assert!(stdout_only.success());

    let both = CommandOutput {
        stdout: "stdout line\n".into(),
        stderr: "stderr line\n".into(),
        exit_code: Some(2),
        timed_out: false,
        cancelled: false,
        duration_ms: 1,
        raw_capture: None,
    };
    assert_eq!(both.combined_output(), "stdout line\nstderr line");
    assert!(!both.success());

    let whitespace_only = CommandOutput {
        stdout: "\n".into(),
        stderr: "  ".into(),
        exit_code: Some(0),
        timed_out: false,
        cancelled: false,
        duration_ms: 1,
        raw_capture: None,
    };
    assert_eq!(whitespace_only.combined_output(), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_captures_stdout_stderr_cwd_env_and_exit_code() {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::write(tmp.path().join("marker.txt"), "marker").expect("write marker");

    let mut req = request(
        "printf '%s' \"$RUA_EXECUTOR_MARKER\"; test -f marker.txt; printf '%s' ' warning' >&2; exit 7",
        tmp.path(),
    );
    req.env.push(("RUA_EXECUTOR_MARKER".into(), "ok".into()));

    let output = RuntimeCommandExecutor::new()
        .execute(req, cancel_rx())
        .await
        .expect("native command executes");

    assert_eq!(output.stdout, "ok");
    assert_eq!(output.stderr, " warning");
    assert_eq!(output.combined_output(), "ok\nwarning");
    assert_eq!(output.exit_code, Some(7));
    assert!(!output.timed_out);
    assert!(!output.cancelled);
    assert!(!output.success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_caps_stream_memory_and_keeps_full_sidecars() {
    let tmp = TempDir::new().expect("tempdir");
    let archive_root = TempDir::new().expect("archive root");
    let mut req = request(
        "i=0; while [ $i -lt 5000 ]; do printf x; i=$((i+1)); done; \
         i=0; while [ $i -lt 5000 ]; do printf y >&2; i=$((i+1)); done",
        tmp.path(),
    );
    req.raw_capture_max_bytes = 1024;
    req.raw_capture_id = Some("bounded-stream".to_string());
    req.tool_result_store_dir = archive_root.path().join("tool_call_logs");

    let output = RuntimeCommandExecutor::new()
        .execute(req, cancel_rx())
        .await
        .expect("command output");

    assert!(output.stdout.len() + output.stderr.len() < 1_200);
    let capture = output.raw_capture.expect("raw capture metadata");
    assert_eq!(capture.stdout_total_bytes, 5_000);
    assert_eq!(capture.stderr_total_bytes, 5_000);
    assert!(capture.stdout_omitted_bytes > 0);
    assert!(capture.stderr_omitted_bytes > 0);
    let stdout_path = capture.stdout_path.unwrap();
    assert_eq!(std::fs::metadata(&stdout_path).unwrap().len(), 5_000);
    assert!(stdout_path.starts_with(archive_root.path()));
    assert!(!tmp.path().join("tool_call_logs").exists());
    assert_eq!(
        std::fs::metadata(capture.stderr_path.unwrap())
            .unwrap()
            .len(),
        5_000
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_reports_spawn_failure_for_missing_working_dir() {
    let tmp = TempDir::new().expect("tempdir");
    let missing_dir = tmp.path().join("missing");
    let req = request("printf should-not-run", &missing_dir);

    let err = RuntimeCommandExecutor::new()
        .execute(req, cancel_rx())
        .await
        .expect_err("missing working directory should fail to spawn");

    let CommandError::SpawnFailed(message) = err else {
        panic!("expected spawn failure, got {err:?}");
    };
    assert!(
        message.contains("No such file")
            || message.contains("cannot find")
            || message.contains("not found"),
        "spawn failure should name the missing working directory cause: {message}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_reports_missing_executable_as_shell_exit_not_spawn_failure() {
    let tmp = TempDir::new().expect("tempdir");
    let req = request("__rua_missing_binary_for_qwenpaw_alignment__", tmp.path());

    let output = RuntimeCommandExecutor::new()
        .execute(req, cancel_rx())
        .await
        .expect("shell should spawn even when the inner executable is missing");

    assert_ne!(output.exit_code, Some(0));
    assert!(!output.success());
    assert!(!output.timed_out);
    assert!(!output.cancelled);
    assert!(
        output.combined_output().contains("not found")
            || output.combined_output().contains("command not found")
            || output.combined_output().contains("No such file"),
        "missing executable should be visible in shell output: {output:?}"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_env_remove_hides_parent_and_explicit_env_values() {
    let _guard = env_test_guard().await;
    let _env = EnvGuard::set("RUA_EXECUTOR_SECRET", "parent-secret");
    let tmp = TempDir::new().expect("tempdir");

    let mut req = request(
        r#"if [ -z "${RUA_EXECUTOR_SECRET+x}" ]; then printf removed; else printf "leaked:%s" "$RUA_EXECUTOR_SECRET"; fi"#,
        tmp.path(),
    );
    req.env
        .push(("RUA_EXECUTOR_SECRET".into(), "explicit-secret".into()));
    req.env_remove.push("RUA_EXECUTOR_SECRET".into());

    let output = RuntimeCommandExecutor::new()
        .execute(req, cancel_rx())
        .await
        .expect("native command executes");

    assert_eq!(output.stdout, "removed");
    assert_eq!(output.stderr, "");
    assert_eq!(output.exit_code, Some(0));
    assert!(output.success());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_lossy_decodes_non_utf8_stdout_and_stderr() {
    let tmp = TempDir::new().expect("tempdir");
    let req = request("printf '\\377ok'; printf '\\376err' >&2", tmp.path());

    let output = RuntimeCommandExecutor::new()
        .execute(req, cancel_rx())
        .await
        .expect("native command executes with non-UTF-8 output");

    assert_eq!(output.stdout, "\u{FFFD}ok");
    assert_eq!(output.stderr, "\u{FFFD}err");
    assert_eq!(output.combined_output(), "\u{FFFD}ok\n\u{FFFD}err");
    assert_eq!(output.exit_code, Some(0));
    assert!(!output.timed_out);
    assert!(!output.cancelled);
    assert!(output.success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_timeout_marks_output_and_stops_child() {
    let tmp = TempDir::new().expect("tempdir");
    let mut req = request("printf start; sleep 5; printf done", tmp.path());
    req.timeout = Duration::from_millis(100);

    let output = RuntimeCommandExecutor::new()
        .execute(req, cancel_rx())
        .await
        .expect("timeout returns command output");

    assert_eq!(output.exit_code, None);
    assert!(output.timed_out);
    assert!(!output.cancelled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_zero_timeout_disables_the_execution_deadline() {
    let tmp = TempDir::new().expect("tempdir");
    let mut req = request("sleep 0.15; printf done", tmp.path());
    req.timeout = Duration::ZERO;

    let output = tokio::time::timeout(
        Duration::from_secs(2),
        RuntimeCommandExecutor::new().execute(req, cancel_rx()),
    )
    .await
    .expect("the command should finish without an executor deadline")
    .expect("command output");

    assert_eq!(output.exit_code, Some(0));
    assert_eq!(output.stdout, "done");
    assert!(!output.timed_out);
    assert!(!output.cancelled);
}

async fn wait_for_stdin(session: &rua_task::TaskRunnerSession) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while session.input_capability().await != rua_task::TaskInputCapability::Stdin {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("Shell stdin should become active");
}

#[cfg(unix)]
#[tokio::test]
async fn shell_live_output_long_utf8_line_survives_small_pages() {
    let tmp = TempDir::new().unwrap();
    let session = rua_task::TaskRunnerSession::bound("long-output", 1);
    let expected = format!("{}\n尾部", "中文".repeat(100));
    let mut req = request(format!("printf '%s' '{expected}'"), tmp.path());
    req.task_session = Some(session.clone());
    let output = RuntimeCommandExecutor::new()
        .execute(req, cancel_rx())
        .await
        .unwrap();
    assert_eq!(output.exit_code, Some(0));
    let source = session.live_output().await.unwrap();
    let mut cursor = rua_task::TaskLiveOutputCursor::default();
    let mut text = String::new();
    for _ in 0..1000 {
        let batch = source.read_records(&cursor, 16, 17).await.unwrap();
        for record in batch.records {
            text.push_str(&record.text);
        }
        cursor = batch.next_cursor;
        assert!(cursor.pending.values().map(Vec::len).sum::<usize>() <= 4);
        if !batch.has_more {
            break;
        }
    }
    assert_eq!(text, expected);
}

#[cfg(unix)]
#[tokio::test]
async fn shell_live_output_strips_split_ansi_preserves_observed_order_and_raw_bytes() {
    let tmp = TempDir::new().unwrap();
    let session = rua_task::TaskRunnerSession::bound("ansi-output", 1);
    let mut req = request("printf '\\033[3'; sleep 0.05; printf '1mfirst\\033[0m\\tline\\n\\n'; sleep 0.05; printf 'warning\\n' >&2; sleep 0.05; printf 'last\\n'", tmp.path());
    req.task_session = Some(session.clone());
    let output = RuntimeCommandExecutor::new()
        .execute(req, cancel_rx())
        .await
        .unwrap();
    assert!(output.stdout.contains('\u{1b}'));
    let source = session.live_output().await.unwrap();
    let mut cursor = rua_task::TaskLiveOutputCursor::default();
    let mut records: Vec<rua_task::TaskLiveOutputRecord> = Vec::new();
    loop {
        let page = source.read_records(&cursor, 16, 5).await.unwrap();
        for record in page.records {
            assert!(!record.text.contains('\u{1b}'));
            if let Some(last) = records
                .last_mut()
                .filter(|last| last.source == record.source)
            {
                last.text.push_str(&record.text);
            } else {
                records.push(record);
            }
        }
        cursor = page.next_cursor;
        if !page.has_more {
            break;
        }
    }
    assert_eq!(
        records
            .iter()
            .map(|r| (r.source.as_str(), r.text.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("stdout", "first\tline\n\n"),
            ("stderr", "warning\n"),
            ("stdout", "last\n")
        ]
    );
    let mut invalid = cursor;
    invalid.positions.insert("record".into(), u64::MAX);
    assert!(source.read_records(&invalid, 16, 8).await.is_err());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_shell_stdin_preserves_exact_bytes_newlines_and_retries() {
    let tmp = TempDir::new().expect("tempdir");
    let session = rua_task::TaskRunnerSession::bound("stdin-exact", 1);
    let mut req = request(
        "IFS= read -r first; IFS= read -r second; printf '<%s>|<%s>' \"$first\" \"$second\"",
        tmp.path(),
    );
    req.task_session = Some(session.clone());
    req.timeout = Duration::ZERO;
    let mut execution = tokio::spawn(async move {
        RuntimeCommandExecutor::new()
            .execute(req, cancel_rx())
            .await
            .expect("interactive command")
    });
    wait_for_stdin(&session).await;

    let first = rua_task::TaskInputSubmission::new(
        "text-a",
        rua_task::TaskInput::Text {
            text: "alpha".to_string(),
        },
    )
    .unwrap();
    let accepted = session.send_input(first.clone()).await.unwrap();
    assert_eq!(session.send_input(first).await.unwrap(), accepted);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut execution)
            .await
            .is_err(),
        "text without an explicit newline must not satisfy shell read"
    );
    session
        .send_input(
            rua_task::TaskInputSubmission::new(
                "text-b",
                rua_task::TaskInput::Text {
                    text: "\nbeta\n".to_string(),
                },
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let output = execution.await.unwrap();
    assert_eq!(output.stdout, "<alpha>|<beta>");
    assert_eq!(output.exit_code, Some(0));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_shell_stdin_close_is_explicit_and_idempotent() {
    let tmp = TempDir::new().expect("tempdir");
    let session = rua_task::TaskRunnerSession::bound("stdin-eof", 1);
    let mut req = request("cat", tmp.path());
    req.task_session = Some(session.clone());
    req.timeout = Duration::ZERO;
    let mut execution = tokio::spawn(async move {
        RuntimeCommandExecutor::new()
            .execute(req, cancel_rx())
            .await
            .expect("interactive command")
    });
    wait_for_stdin(&session).await;

    session
        .send_input(
            rua_task::TaskInputSubmission::new(
                "body",
                rua_task::TaskInput::Text {
                    text: "without-newline".to_string(),
                },
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut execution)
            .await
            .is_err(),
        "Shell must remain open until close_stdin delivers EOF"
    );
    let close =
        rua_task::TaskInputSubmission::new("close", rua_task::TaskInput::CloseStdin).unwrap();
    let accepted = session.send_input(close.clone()).await.unwrap();
    assert_eq!(session.send_input(close).await.unwrap(), accepted);

    let output = execution.await.unwrap();
    assert_eq!(output.stdout, "without-newline");
    assert_eq!(
        session.input_capability().await,
        rua_task::TaskInputCapability::None
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_rejects_input_after_process_exit() {
    let tmp = TempDir::new().expect("tempdir");
    let session = rua_task::TaskRunnerSession::bound("stdin-exit", 1);
    let mut req = request("exit 0", tmp.path());
    req.task_session = Some(session.clone());
    req.timeout = Duration::ZERO;

    let output = RuntimeCommandExecutor::new()
        .execute(req, cancel_rx())
        .await
        .expect("command output");
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(
        session.input_capability().await,
        rua_task::TaskInputCapability::None
    );
    assert!(session
        .send_input(
            rua_task::TaskInputSubmission::new(
                "late",
                rua_task::TaskInput::Text {
                    text: "too late".to_string(),
                },
            )
            .unwrap(),
        )
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_executor_cancel_marks_output_and_stops_child() {
    let tmp = TempDir::new().expect("tempdir");
    let started = tmp.path().join("started.marker");
    let req = request(
        "printf start; printf ready > started.marker; sleep 5; printf done",
        tmp.path(),
    );
    let (cancel_tx, cancel_rx) = cancel_pair();

    let cancel_task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !started.is_file() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("child should publish started marker");
        cancel_tx.send(true).expect("send cancellation");
    });

    let output = RuntimeCommandExecutor::new()
        .execute(req, cancel_rx)
        .await
        .expect("cancel returns command output");
    cancel_task.await.expect("cancel task joins");

    assert_eq!(output.exit_code, None);
    assert!(!output.timed_out);
    assert!(output.cancelled);
    assert!(output.stdout.contains("start"));
    assert!(!output.stdout.contains("done"));
    assert!(!output.success());
}
