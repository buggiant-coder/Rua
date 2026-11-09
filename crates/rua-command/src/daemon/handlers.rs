use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use rua_i18n::t;

use super::{DaemonCommand, DaemonContext};
use crate::CommandOutcome;

const DEFAULT_LOG_LINES: usize = 100;
const MAX_LOG_LINES: usize = 2000;

pub struct StatusCommand;

#[async_trait]
impl DaemonCommand for StatusCommand {
    fn name(&self) -> &str {
        "status"
    }

    fn help(&self) -> &str {
        "Show daemon status (pid, uptime, version, active sessions)"
    }

    async fn execute(&self, ctx: &DaemonContext, _args: &str) -> CommandOutcome {
        let pid = std::process::id();
        let uptime = ctx.started_at.elapsed();
        let hours = uptime.as_secs() / 3600;
        let minutes = (uptime.as_secs() % 3600) / 60;
        let seconds = uptime.as_secs() % 60;
        let version = env!("CARGO_PKG_VERSION");
        let uptime_str = format!("{hours}h {minutes}m {seconds}s");

        let msg = t("daemon.status.body", &ctx.locale)
            .replace("{pid}", &pid.to_string())
            .replace("{uptime}", &uptime_str)
            .replace("{version}", version);
        CommandOutcome::text(msg)
    }
}

pub struct RestartCommand;

#[async_trait]
impl DaemonCommand for RestartCommand {
    fn name(&self) -> &str {
        "restart"
    }

    fn help(&self) -> &str {
        "Restart the daemon process"
    }

    async fn execute(&self, ctx: &DaemonContext, _args: &str) -> CommandOutcome {
        tracing::info!("daemon restart requested via daemon command");
        let outcome = CommandOutcome::text(t("daemon.restart.body", &ctx.locale));
        if let Some(exit_fn) = &ctx.exit_fn {
            exit_fn(0);
        } else {
            std::process::exit(0);
        }
        #[allow(unreachable_code)]
        outcome
    }
}

pub struct ReloadConfigCommand;

#[async_trait]
impl DaemonCommand for ReloadConfigCommand {
    fn name(&self) -> &str {
        "reload-config"
    }

    fn help(&self) -> &str {
        "Reload workspace configuration from disk"
    }

    async fn execute(&self, ctx: &DaemonContext, _args: &str) -> CommandOutcome {
        match ctx.workspace_reloader.reload_all().await {
            Ok(()) => {
                tracing::info!("config reloaded via daemon command");
                CommandOutcome::text(t("daemon.reload.ok", &ctx.locale))
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to reload config via daemon command");
                CommandOutcome::text(
                    t("daemon.reload.fail", &ctx.locale).replace("{error}", &e.to_string()),
                )
            }
        }
    }
}

pub struct VersionCommand;

#[async_trait]
impl DaemonCommand for VersionCommand {
    fn name(&self) -> &str {
        "version"
    }

    fn help(&self) -> &str {
        "Show daemon version"
    }

    async fn execute(&self, _ctx: &DaemonContext, _args: &str) -> CommandOutcome {
        CommandOutcome::text(env!("CARGO_PKG_VERSION"))
    }
}

pub struct LogsCommand;

#[async_trait]
impl DaemonCommand for LogsCommand {
    fn name(&self) -> &str {
        "logs"
    }

    fn help(&self) -> &str {
        "Show last N lines of the log file (default 100, max 2000). Usage: /logs [N]"
    }

    async fn execute(&self, ctx: &DaemonContext, args: &str) -> CommandOutcome {
        let n = parse_log_lines(args);

        let log_path = ctx.data_dir.join("logs").join("rua.log");
        if !log_path.exists() {
            return CommandOutcome::text(t("daemon.logs.not_found", &ctx.locale));
        }

        match std::fs::read_to_string(&log_path) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let tail: Vec<&str> = if lines.len() > n {
                    lines[lines.len() - n..].to_vec()
                } else {
                    lines
                };
                CommandOutcome::text(tail.join("\n"))
            }
            Err(e) => CommandOutcome::text(
                t("daemon.logs.read_fail", &ctx.locale).replace("{error}", &e.to_string()),
            ),
        }
    }
}

pub fn parse_log_lines(args: &str) -> usize {
    for token in args.split_whitespace() {
        if let Ok(n) = token.parse::<usize>() {
            return n.clamp(1, MAX_LOG_LINES);
        }
    }
    DEFAULT_LOG_LINES
}

pub struct ApproveCommand;

#[async_trait]
impl DaemonCommand for ApproveCommand {
    fn name(&self) -> &str {
        "approve"
    }

    fn help(&self) -> &str {
        "Approve pending tool-guard request"
    }

    async fn execute(&self, ctx: &DaemonContext, args: &str) -> CommandOutcome {
        let session_id = if args.is_empty() { "" } else { args.trim() };
        let approved = ctx.approval_service.approve_pending(session_id).await;
        if approved {
            CommandOutcome::text(t("daemon.approve.approved", &ctx.locale))
        } else {
            CommandOutcome::text(t("daemon.approve.no_pending", &ctx.locale))
        }
    }
}

pub struct DenyCommand;

#[async_trait]
impl DaemonCommand for DenyCommand {
    fn name(&self) -> &str {
        "deny"
    }

    fn help(&self) -> &str {
        "Reject pending tool-guard request"
    }

    async fn execute(&self, ctx: &DaemonContext, args: &str) -> CommandOutcome {
        let session_id = if args.is_empty() { "" } else { args.trim() };
        let rejected = ctx.approval_service.reject_pending(session_id).await;
        if rejected {
            CommandOutcome::text(t("daemon.deny.rejected", &ctx.locale))
        } else {
            CommandOutcome::text(t("daemon.approve.no_pending", &ctx.locale))
        }
    }
}

pub struct ApprovalCommand;

#[async_trait]
impl DaemonCommand for ApprovalCommand {
    fn name(&self) -> &str {
        "approval"
    }

    fn help(&self) -> &str {
        "Resolve a specific tool-guard request. Usage: /approval <approve|deny> <request_id>"
    }

    async fn execute(&self, ctx: &DaemonContext, args: &str) -> CommandOutcome {
        let mut parts = args.split_whitespace();
        let action = parts.next().unwrap_or_default();
        let request_id = parts.next().unwrap_or_default();
        if request_id.is_empty() {
            return CommandOutcome::text(
                "Usage: /approval <approve|deny> <request_id>".to_string(),
            );
        }
        let ok = match action {
            "approve" => {
                ctx.approval_service
                    .approve_request(request_id, "approval-card")
                    .await
            }
            "deny" | "reject" => {
                ctx.approval_service
                    .reject_request(request_id, "approval-card")
                    .await
            }
            _ => {
                return CommandOutcome::text(
                    "Usage: /approval <approve|deny> <request_id>".to_string(),
                );
            }
        };
        if ok {
            CommandOutcome::text(format!("approval {request_id} resolved"))
        } else {
            CommandOutcome::text(format!("approval {request_id} not pending"))
        }
    }
}

pub struct DaemonRouterCommand {
    sub_commands: HashMap<String, Arc<dyn DaemonCommand>>,
}

impl DaemonRouterCommand {
    pub fn new(sub_commands: HashMap<String, Arc<dyn DaemonCommand>>) -> Self {
        Self { sub_commands }
    }
}

#[async_trait]
impl DaemonCommand for DaemonRouterCommand {
    fn name(&self) -> &str {
        "daemon"
    }

    fn help(&self) -> &str {
        "Daemon management commands. Usage: /daemon <status|restart|reload-config|version|logs|approve|deny>"
    }

    async fn execute(&self, ctx: &DaemonContext, args: &str) -> CommandOutcome {
        let mut parts = args.split_whitespace();
        let sub = match parts.next() {
            Some(s) => s,
            None => {
                return CommandOutcome::text(t("daemon.router.usage", &ctx.locale));
            }
        };

        let sub_normalized = sub.replace('_', "-");
        let sub_args: String = parts.collect::<Vec<_>>().join(" ");

        if let Some(cmd) = self.sub_commands.get(sub_normalized.as_str()) {
            return cmd.execute(ctx, &sub_args).await;
        }

        CommandOutcome::text(t("daemon.router.unknown", &ctx.locale).replace("{sub}", sub))
    }
}
