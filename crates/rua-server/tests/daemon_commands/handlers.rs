use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rua_command::*;

struct NoopApproval;
#[async_trait]
impl ApprovalService for NoopApproval {
    async fn approve_pending(&self, _: &str) -> bool {
        false
    }
    async fn reject_pending(&self, _: &str) -> bool {
        false
    }
}

struct ApproveOnceApproval;
#[async_trait]
impl ApprovalService for ApproveOnceApproval {
    async fn approve_pending(&self, _: &str) -> bool {
        true
    }
    async fn reject_pending(&self, _: &str) -> bool {
        false
    }
}

struct ApprovalById {
    approved: AtomicBool,
    rejected: AtomicBool,
}

#[async_trait]
impl ApprovalService for ApprovalById {
    async fn approve_pending(&self, _: &str) -> bool {
        false
    }
    async fn reject_pending(&self, _: &str) -> bool {
        false
    }
    async fn approve_request(&self, request_id: &str, decided_by: &str) -> bool {
        self.approved.store(
            request_id == "req-1" && decided_by == "approval-card",
            Ordering::SeqCst,
        );
        true
    }
    async fn reject_request(&self, request_id: &str, decided_by: &str) -> bool {
        self.rejected.store(
            request_id == "req-2" && decided_by == "approval-card",
            Ordering::SeqCst,
        );
        true
    }
}

struct NoopReloader;
#[async_trait]
impl WorkspaceReloader for NoopReloader {
    async fn reload_all(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

struct FailingReloader;
#[async_trait]
impl WorkspaceReloader for FailingReloader {
    async fn reload_all(&self) -> rua_core::Result<()> {
        Err(rua_core::RuaError::Runtime {
            message: "test reload failure".into(),
        })
    }
}

fn make_ctx(data_dir: &std::path::Path) -> DaemonContext {
    DaemonContext {
        data_dir: data_dir.to_path_buf(),
        started_at: std::time::Instant::now(),
        workspace_reloader: Arc::new(NoopReloader),
        exit_fn: None,
        approval_service: Arc::new(NoopApproval),
        locale: "zh".into(),
    }
}

fn extract_text(outcome: &CommandOutcome) -> String {
    outcome
        .events
        .iter()
        .filter_map(|e| match e {
            agent_types::AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn status_command_returns_pid_and_version() {
    let ctx = make_ctx(std::path::Path::new("/tmp/test"));
    let cmd = StatusCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = extract_text(&outcome);

    assert!(text.contains("守护进程状态"));
    assert!(text.contains("pid:"));
    assert!(text.contains("运行时间:"));
    assert!(text.contains("版本:"));
}

#[tokio::test]
async fn restart_command_calls_exit_fn() {
    let exit_called = Arc::new(AtomicBool::new(false));
    let exit_code = Arc::new(AtomicI32::new(-1));
    let ec = Arc::clone(&exit_called);
    let ecc = Arc::clone(&exit_code);

    let ctx = DaemonContext {
        data_dir: std::path::PathBuf::from("/tmp/test"),
        started_at: std::time::Instant::now(),
        workspace_reloader: Arc::new(NoopReloader),
        exit_fn: Some(Arc::new(move |code| {
            ec.store(true, Ordering::SeqCst);
            ecc.store(code, Ordering::SeqCst);
        })),
        approval_service: Arc::new(NoopApproval),
        locale: "zh".into(),
    };

    let cmd = RestartCommand;
    cmd.execute(&ctx, "").await;

    assert!(exit_called.load(Ordering::SeqCst));
    assert_eq!(exit_code.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn reload_config_success() {
    let ctx = make_ctx(std::path::Path::new("/tmp/test"));
    let cmd = ReloadConfigCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = extract_text(&outcome);
    assert!(text.contains("配置已重新加载"));
}

#[tokio::test]
async fn reload_config_failure() {
    let ctx = DaemonContext {
        data_dir: std::path::PathBuf::from("/tmp/test"),
        started_at: std::time::Instant::now(),
        workspace_reloader: Arc::new(FailingReloader),
        exit_fn: None,
        approval_service: Arc::new(NoopApproval),
        locale: "zh".into(),
    };
    let cmd = ReloadConfigCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = extract_text(&outcome);
    assert!(text.contains("配置重新加载失败："));
}

#[tokio::test]
async fn version_command_returns_cargo_version() {
    let ctx = make_ctx(std::path::Path::new("/tmp/test"));
    let cmd = VersionCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = extract_text(&outcome);
    assert!(!text.is_empty());

    assert!(text.contains('.'));
}

#[tokio::test]
async fn logs_command_missing_file() {
    let ctx = make_ctx(std::path::Path::new("/tmp/nonexistent-dir"));
    let cmd = LogsCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = extract_text(&outcome);
    assert!(text.contains("日志文件未找到"));
}

#[tokio::test]
async fn logs_command_reads_last_n_lines() {
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    let log_file = log_dir.join("rua.log");

    let lines: Vec<String> = (0..200).map(|i| format!("line-{i}")).collect();
    std::fs::write(&log_file, lines.join("\n")).unwrap();

    let ctx = make_ctx(dir.path());
    let cmd = LogsCommand;

    let outcome = cmd.execute(&ctx, "").await;
    let text = extract_text(&outcome);
    let result_lines: Vec<&str> = text.lines().collect();
    assert_eq!(result_lines.len(), 100);
    assert!(text.contains("line-199"));
    assert!(text.contains("line-100"));
    assert!(!text.contains("line-99\n"));

    let outcome = cmd.execute(&ctx, "30").await;
    let text = extract_text(&outcome);
    let result_lines: Vec<&str> = text.lines().collect();
    assert_eq!(result_lines.len(), 30);
}

#[tokio::test]
async fn logs_command_clamp_to_2000() {
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    let log_file = log_dir.join("rua.log");

    let lines: Vec<String> = (0..3000).map(|i| format!("line-{i}")).collect();
    std::fs::write(&log_file, lines.join("\n")).unwrap();

    let ctx = make_ctx(dir.path());
    let cmd = LogsCommand;

    let outcome = cmd.execute(&ctx, "3000").await;
    let text = extract_text(&outcome);
    let result_lines: Vec<&str> = text.lines().collect();
    assert!(result_lines.len() <= 2000);
}

#[tokio::test]
async fn logs_command_clamp_zero_to_one() {
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    let log_file = log_dir.join("rua.log");

    std::fs::write(&log_file, "only-line\nsecond-line\n").unwrap();

    let ctx = make_ctx(dir.path());
    let cmd = LogsCommand;

    let outcome = cmd.execute(&ctx, "0").await;
    let text = extract_text(&outcome);
    let result_lines: Vec<&str> = text.lines().collect();
    assert_eq!(result_lines.len(), 1);
}

#[tokio::test]
async fn approve_command_no_pending() {
    let ctx = make_ctx(std::path::Path::new("/tmp/test"));
    let cmd = ApproveCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = extract_text(&outcome);
    assert!(text.contains("当前没有待批准的请求"));
}

#[tokio::test]
async fn approve_command_with_pending() {
    let ctx = DaemonContext {
        data_dir: std::path::PathBuf::from("/tmp/test"),
        started_at: std::time::Instant::now(),
        workspace_reloader: Arc::new(NoopReloader),
        exit_fn: None,
        approval_service: Arc::new(ApproveOnceApproval),
        locale: "zh".into(),
    };
    let cmd = ApproveCommand;
    let outcome = cmd.execute(&ctx, "").await;
    let text = extract_text(&outcome);
    assert!(text.contains("已批准待处理请求"));
}

#[tokio::test]
async fn approval_command_resolves_specific_request_id() {
    let service = Arc::new(ApprovalById {
        approved: AtomicBool::new(false),
        rejected: AtomicBool::new(false),
    });
    let ctx = DaemonContext {
        data_dir: std::path::PathBuf::from("/tmp/test"),
        started_at: std::time::Instant::now(),
        workspace_reloader: Arc::new(NoopReloader),
        exit_fn: None,
        approval_service: service.clone(),
        locale: "zh".into(),
    };
    let cmd = ApprovalCommand;

    let approve = cmd.execute(&ctx, "approve req-1").await;
    assert!(extract_text(&approve).contains("req-1 resolved"));
    let deny = cmd.execute(&ctx, "deny req-2").await;
    assert!(extract_text(&deny).contains("req-2 resolved"));

    assert!(service.approved.load(Ordering::SeqCst));
    assert!(service.rejected.load(Ordering::SeqCst));
}

#[test]
fn parse_log_lines_boundary_cases() {
    use rua_command::daemon::handlers::parse_log_lines;

    assert_eq!(parse_log_lines(""), 100);
    assert_eq!(parse_log_lines("1"), 1);
    assert_eq!(parse_log_lines("2000"), 2000);
    assert_eq!(parse_log_lines("3000"), 2000);
    assert_eq!(parse_log_lines("0"), 1);
    assert_eq!(parse_log_lines("abc"), 100);
    assert_eq!(parse_log_lines("foo 50 bar"), 50);
}
