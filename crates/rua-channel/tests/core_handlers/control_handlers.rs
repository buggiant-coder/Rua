use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rua_channel::core::control_handlers::dispatch_control_command;
use rua_channel::core::{ApprovalService, ControlContext, SessionCanceller, WorkspaceReloader};

struct MockCanceller {
    has_active: AtomicBool,
}

#[async_trait]
impl SessionCanceller for MockCanceller {
    async fn cancel_session(&self, _session_id: &str) -> bool {
        self.has_active.load(Ordering::SeqCst)
    }
    async fn active_session_count(&self) -> usize {
        0
    }
}

struct MockReloader;
#[async_trait]
impl WorkspaceReloader for MockReloader {
    async fn reload_all(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

struct MockApproval;
#[async_trait]
impl ApprovalService for MockApproval {
    async fn approve_pending(&self, _session_id: &str) -> bool {
        false
    }
    async fn reject_pending(&self, _session_id: &str) -> bool {
        false
    }
}

fn make_ctx(has_active: bool) -> ControlContext {
    ControlContext {
        session_canceller: Arc::new(MockCanceller {
            has_active: AtomicBool::new(has_active),
        }),
        workspace_reloader: Arc::new(MockReloader),
        approval_service: Arc::new(MockApproval),
        started_at: std::time::Instant::now(),
        data_dir: std::env::temp_dir().join("rua-test-control"),
        exit_fn: None,
    }
}

fn payload() -> serde_json::Value {
    serde_json::json!({})
}

#[tokio::test]
async fn stop_with_active_session_returns_cancelled_text() {
    let ctx = make_ctx(true);
    let result = dispatch_control_command(&ctx, "dingtalk", "session-1", "/stop", &payload())
        .await
        .unwrap();
    assert_eq!(result.response_text.as_deref(), Some("已停止当前任务"));
}

#[tokio::test]
async fn stop_without_active_session_returns_no_task_text() {
    let ctx = make_ctx(false);
    let result = dispatch_control_command(&ctx, "dingtalk", "session-1", "/stop", &payload())
        .await
        .unwrap();
    assert_eq!(
        result.response_text.as_deref(),
        Some("当前没有正在执行的任务")
    );
}

#[tokio::test]
async fn unknown_command_returns_no_response() {
    let ctx = make_ctx(false);
    let result =
        dispatch_control_command(&ctx, "dingtalk", "session-1", "/unknown_cmd", &payload())
            .await
            .unwrap();
    assert!(result.response_text.is_none());
}

#[tokio::test]
async fn stop_case_insensitive() {
    let ctx = make_ctx(true);
    let result = dispatch_control_command(&ctx, "dingtalk", "s1", "/STOP", &payload())
        .await
        .unwrap();
    assert_eq!(result.response_text.as_deref(), Some("已停止当前任务"));
}

#[tokio::test]
async fn stop_with_trailing_text() {
    let ctx = make_ctx(true);
    let result = dispatch_control_command(&ctx, "dingtalk", "s1", "/stop please", &payload())
        .await
        .unwrap();
    assert_eq!(result.response_text.as_deref(), Some("已停止当前任务"));
}
