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

struct NoopReloader;
#[async_trait]
impl WorkspaceReloader for NoopReloader {
    async fn reload_all(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

fn make_ctx() -> DaemonContext {
    DaemonContext {
        data_dir: std::path::PathBuf::from("/tmp/test"),
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
async fn daemon_router_routes_status_subcommand() {
    let registry = build_daemon_registry();
    let router = registry.get("daemon").expect("daemon router registered");
    let ctx = make_ctx();
    let outcome = router.execute(&ctx, "status").await;
    let text = extract_text(&outcome);
    assert!(text.contains("守护进程状态"));
}

#[tokio::test]
async fn daemon_router_routes_version_subcommand() {
    let registry = build_daemon_registry();
    let router = registry.get("daemon").expect("daemon router registered");
    let ctx = make_ctx();
    let outcome = router.execute(&ctx, "version").await;
    let text = extract_text(&outcome);
    assert!(text.contains('.'));
}

#[tokio::test]
async fn daemon_router_unknown_subcommand() {
    let registry = build_daemon_registry();
    let router = registry.get("daemon").expect("daemon router registered");
    let ctx = make_ctx();
    let outcome = router.execute(&ctx, "foobar").await;
    let text = extract_text(&outcome);
    assert!(text.contains("未知子命令"));
}

#[tokio::test]
async fn daemon_router_no_subcommand() {
    let registry = build_daemon_registry();
    let router = registry.get("daemon").expect("daemon router registered");
    let ctx = make_ctx();
    let outcome = router.execute(&ctx, "").await;
    let text = extract_text(&outcome);
    assert!(text.contains("用法"));
}

#[tokio::test]
async fn daemon_router_reload_config_underscore() {
    let registry = build_daemon_registry();
    let router = registry.get("daemon").expect("daemon router registered");
    let ctx = make_ctx();
    let outcome = router.execute(&ctx, "reload_config").await;
    let text = extract_text(&outcome);
    assert!(text.contains("配置已重新加载"));
}

#[tokio::test]
async fn short_alias_status() {
    let registry = build_daemon_registry();
    let cmd = registry.get("status").expect("/status registered");
    let ctx = make_ctx();
    let outcome = cmd.execute(&ctx, "").await;
    assert!(extract_text(&outcome).contains("守护进程状态"));
}

#[tokio::test]
async fn short_alias_version() {
    let registry = build_daemon_registry();
    let cmd = registry.get("version").expect("/version registered");
    let ctx = make_ctx();
    let outcome = cmd.execute(&ctx, "").await;
    assert!(extract_text(&outcome).contains('.'));
}

#[tokio::test]
async fn short_alias_reload_config_hyphen() {
    let registry = build_daemon_registry();
    let cmd = registry
        .get("reload-config")
        .expect("/reload-config registered");
    let ctx = make_ctx();
    let outcome = cmd.execute(&ctx, "").await;
    assert!(extract_text(&outcome).contains("配置已重新加载"));
}

#[tokio::test]
async fn short_alias_reload_config_underscore() {
    let registry = build_daemon_registry();
    let cmd = registry
        .get("reload_config")
        .expect("/reload_config alias registered");
    let ctx = make_ctx();
    let outcome = cmd.execute(&ctx, "").await;
    assert!(extract_text(&outcome).contains("配置已重新加载"));
}

#[tokio::test]
async fn short_alias_approve() {
    let registry = build_daemon_registry();
    let cmd = registry.get("approve").expect("/approve registered");
    let ctx = make_ctx();
    let outcome = cmd.execute(&ctx, "").await;
    assert!(extract_text(&outcome).contains("当前没有待批准的请求"));
}

#[tokio::test]
async fn short_alias_logs() {
    let registry = build_daemon_registry();
    let cmd = registry.get("logs").expect("/logs registered");
    let ctx = make_ctx();
    let outcome = cmd.execute(&ctx, "").await;
    assert!(extract_text(&outcome).contains("日志文件未找到"));
}

#[test]
fn daemon_registry_has_at_least_7_entries() {
    let registry = build_daemon_registry();

    let listed = registry.list();
    assert!(
        listed.len() >= 7,
        "expected >= 7 entries, got {}",
        listed.len()
    );
}

#[tokio::test]
async fn daemon_router_logs_with_n_argument() {
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    let log_file = log_dir.join("rua.log");
    let lines: Vec<String> = (0..50).map(|i| format!("line-{i}")).collect();
    std::fs::write(&log_file, lines.join("\n")).unwrap();

    let ctx = DaemonContext {
        data_dir: dir.path().to_path_buf(),
        started_at: std::time::Instant::now(),
        workspace_reloader: Arc::new(NoopReloader),
        exit_fn: None,
        approval_service: Arc::new(NoopApproval),
        locale: "zh".into(),
    };

    let registry = build_daemon_registry();
    let router = registry.get("daemon").unwrap();
    let outcome = router.execute(&ctx, "logs 10").await;
    let text = extract_text(&outcome);
    let result_lines: Vec<&str> = text.lines().collect();
    assert_eq!(result_lines.len(), 10);
}
