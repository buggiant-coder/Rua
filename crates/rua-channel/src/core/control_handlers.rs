use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

#[async_trait]
pub trait SessionCanceller: Send + Sync {
    async fn cancel_session(&self, session_id: &str) -> bool;
    async fn active_session_count(&self) -> usize;
}

#[async_trait]
pub trait WorkspaceReloader: Send + Sync {
    async fn reload_all(&self) -> rua_core::Result<()>;
}

#[async_trait]
pub trait ApprovalService: Send + Sync {
    async fn approve_pending(&self, session_id: &str) -> bool;
    async fn reject_pending(&self, session_id: &str) -> bool;
}

pub struct ControlContext {
    pub session_canceller: Arc<dyn SessionCanceller>,
    pub workspace_reloader: Arc<dyn WorkspaceReloader>,
    pub approval_service: Arc<dyn ApprovalService>,
    pub started_at: std::time::Instant,
    pub data_dir: PathBuf,

    pub exit_fn: Option<Arc<dyn Fn(i32) + Send + Sync>>,
}

pub struct ControlResult {
    pub response_text: Option<String>,
}

pub async fn dispatch_control_command(
    ctx: &ControlContext,
    channel_id: &str,
    session_id: &str,
    query: &str,
    _payload: &serde_json::Value,
) -> rua_core::Result<ControlResult> {
    let lower = query.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let cmd = words.first().copied().unwrap_or("");

    match cmd {
        "/stop" => handle_stop(ctx, channel_id, session_id).await,
        "/daemon" => handle_daemon_fallback(ctx, channel_id, session_id, &words).await,
        "/status" => handle_daemon_status(ctx, channel_id, session_id).await,
        "/version" => handle_daemon_version(ctx, channel_id, session_id).await,
        _ => {
            tracing::warn!(
                %channel_id,
                %session_id,
                command = %query,
                "unhandled control command"
            );
            Ok(ControlResult {
                response_text: None,
            })
        }
    }
}

async fn handle_daemon_fallback(
    ctx: &ControlContext,
    channel_id: &str,
    session_id: &str,
    words: &[&str],
) -> rua_core::Result<ControlResult> {
    match words.get(1).copied().unwrap_or("status") {
        "status" => handle_daemon_status(ctx, channel_id, session_id).await,
        "version" => handle_daemon_version(ctx, channel_id, session_id).await,
        _ => Ok(ControlResult {
            response_text: None,
        }),
    }
}

async fn handle_daemon_status(
    ctx: &ControlContext,
    channel_id: &str,
    session_id: &str,
) -> rua_core::Result<ControlResult> {
    let uptime = ctx.started_at.elapsed();
    let hours = uptime.as_secs() / 3600;
    let minutes = (uptime.as_secs() % 3600) / 60;
    let seconds = uptime.as_secs() % 60;
    tracing::info!(%channel_id, %session_id, "daemon status requested via legacy fallback");
    Ok(ControlResult {
        response_text: Some(format!(
            "Daemon Status\npid: {}\nuptime: {hours}h {minutes}m {seconds}s\nversion: {}",
            std::process::id(),
            env!("CARGO_PKG_VERSION")
        )),
    })
}

async fn handle_daemon_version(
    _ctx: &ControlContext,
    channel_id: &str,
    session_id: &str,
) -> rua_core::Result<ControlResult> {
    tracing::info!(%channel_id, %session_id, "daemon version requested via legacy fallback");
    Ok(ControlResult {
        response_text: Some(env!("CARGO_PKG_VERSION").to_string()),
    })
}

async fn handle_stop(
    ctx: &ControlContext,
    channel_id: &str,
    session_id: &str,
) -> rua_core::Result<ControlResult> {
    let cancelled = ctx.session_canceller.cancel_session(session_id).await;
    let msg = if cancelled {
        tracing::info!(%channel_id, %session_id, "active task cancelled via /stop");
        "已停止当前任务"
    } else {
        tracing::info!(%channel_id, %session_id, "no active task to cancel via /stop");
        "当前没有正在执行的任务"
    };
    Ok(ControlResult {
        response_text: Some(msg.to_string()),
    })
}
