use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use rua_i18n::t;

use crate::acp_chat::{AcpChatController, AcpChatSessionManager};
use crate::CommandOutcome;

#[async_trait]
pub trait ControlCommand: Send + Sync {
    fn name(&self) -> &str;
    fn help(&self) -> &str;
    async fn execute(&self, ctx: &ControlContext, args: &str) -> CommandOutcome;
}

#[async_trait]
pub trait SessionCanceller: Send + Sync {
    async fn cancel_session(&self, channel_id: &str, session_id: &str) -> bool;
    async fn active_session_count(&self) -> usize;
}

#[async_trait]
pub trait QueueClearer: Send + Sync {
    async fn clear_queue(&self, channel_id: &str, session_id: &str) -> usize;
}

#[async_trait]
pub trait SkillsLister: Send + Sync {
    async fn list_skills(&self, channel_id: &str, locale: &str) -> String;
}

#[async_trait]
pub trait ApprovalService: Send + Sync {
    async fn approve_pending(&self, session_id: &str) -> bool;
    async fn reject_pending(&self, session_id: &str) -> bool;
    async fn approve_request(&self, request_id: &str, decided_by: &str) -> bool {
        let _ = (request_id, decided_by);
        false
    }
    async fn reject_request(&self, request_id: &str, decided_by: &str) -> bool {
        let _ = (request_id, decided_by);
        false
    }
}

pub struct ControlContext {
    pub session_id: String,
    pub user_id: String,
    pub channel: String,
    pub session_canceller: Arc<dyn SessionCanceller>,
    pub approval_service: Arc<dyn ApprovalService>,
    pub queue_clearer: Arc<dyn QueueClearer>,
    pub skills_lister: Arc<dyn SkillsLister>,
    pub acp_chat_manager: Arc<AcpChatSessionManager>,
    pub acp_chat_controller: Arc<dyn AcpChatController>,
    pub locale: String,
}

pub struct ControlCommandRegistry {
    commands: HashMap<String, Arc<dyn ControlCommand>>,
}

impl ControlCommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
        }
    }

    pub fn register(&mut self, cmd: Arc<dyn ControlCommand>) {
        let name = cmd.name();
        assert!(
            is_valid_command_name(name),
            "control command name must be non-empty and contain no whitespace: {name:?}"
        );
        self.commands.insert(name.to_string(), cmd);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn ControlCommand>> {
        self.commands.get(name)
    }

    pub fn list(&self) -> Vec<(&str, &str)> {
        let mut cmds: Vec<_> = self
            .commands
            .values()
            .map(|cmd| (cmd.name(), cmd.help()))
            .collect();
        cmds.sort_by_key(|(name, _)| *name);
        cmds
    }
}

impl Default for ControlCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn build_control_registry() -> ControlCommandRegistry {
    let mut registry = ControlCommandRegistry::new();
    registry.register(Arc::new(StopCommand));
    registry.register(Arc::new(SkillsCommand));
    registry
}

fn is_valid_command_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.chars().any(char::is_whitespace)
}

pub struct StopCommand;

#[async_trait]
impl ControlCommand for StopCommand {
    fn name(&self) -> &str {
        "stop"
    }

    fn help(&self) -> &str {
        "Stop the active generation. Usage: /stop [session=<id>]"
    }

    async fn execute(&self, ctx: &ControlContext, args: &str) -> CommandOutcome {
        let target_session = parse_session_arg(args).unwrap_or(&ctx.session_id);
        let channel = &ctx.channel;

        let stopped = ctx
            .session_canceller
            .cancel_session(channel, target_session)
            .await;
        let cleared = ctx.queue_clearer.clear_queue(channel, target_session).await;

        let msg = if stopped || cleared > 0 {
            t("command.stop.stopped", &ctx.locale)
        } else {
            t("command.stop.no_active", &ctx.locale)
        };

        CommandOutcome::text(msg)
    }
}

fn parse_session_arg(args: &str) -> Option<&str> {
    for part in args.split_whitespace() {
        if let Some(val) = part.strip_prefix("session=") {
            if !val.is_empty() {
                return Some(val);
            }
        }
    }
    None
}

pub struct SkillsCommand;

#[async_trait]
impl ControlCommand for SkillsCommand {
    fn name(&self) -> &str {
        "skills"
    }

    fn help(&self) -> &str {
        "List enabled skills for this channel. Usage: /skills"
    }

    async fn execute(&self, ctx: &ControlContext, _args: &str) -> CommandOutcome {
        let text = ctx
            .skills_lister
            .list_skills(&ctx.channel, &ctx.locale)
            .await;
        CommandOutcome::text(text)
    }
}

pub struct AcpCommand;

#[async_trait]
impl ControlCommand for AcpCommand {
    fn name(&self) -> &str {
        "acp"
    }

    fn help(&self) -> &str {
        "Enter ACP chat mode. Usage: /acp <runner> [cwd]"
    }

    async fn execute(&self, ctx: &ControlContext, args: &str) -> CommandOutcome {
        let parts: Vec<&str> = args.split_whitespace().collect();
        let (runner, cwd) = match parts.as_slice() {
            [] => return CommandOutcome::text(t("command.acp.usage", &ctx.locale)),
            [runner] => (*runner, None),
            [runner, cwd, ..] => (*runner, Some(*cwd)),
        };
        let text = ctx
            .acp_chat_controller
            .enter(&ctx.session_id, runner, cwd)
            .await;
        CommandOutcome::text(text)
    }
}

pub struct NoAcpCommand;

#[async_trait]
impl ControlCommand for NoAcpCommand {
    fn name(&self) -> &str {
        "noacp"
    }

    fn help(&self) -> &str {
        "Exit ACP chat mode. Usage: /noacp"
    }

    async fn execute(&self, ctx: &ControlContext, _args: &str) -> CommandOutcome {
        let text = ctx.acp_chat_controller.exit(&ctx.session_id).await;
        CommandOutcome::text(text)
    }
}

pub struct AcphCommand;

#[async_trait]
impl ControlCommand for AcphCommand {
    fn name(&self) -> &str {
        "acph"
    }

    fn help(&self) -> &str {
        "List ACP sessions from a runner. Usage: /acph <runner> [cwd|all] \
         (default: current chat's cwd; `all` lists everything the runner knows)"
    }

    async fn execute(&self, ctx: &ControlContext, args: &str) -> CommandOutcome {
        let parts: Vec<&str> = args.split_whitespace().collect();
        let (runner, cwd_arg) = match parts.as_slice() {
            [] => return CommandOutcome::text(t("command.acp.usage", &ctx.locale)),
            [runner] => (*runner, None),
            [runner, cwd, ..] => (*runner, Some(*cwd)),
        };
        let text = ctx
            .acp_chat_controller
            .list_sessions(&ctx.session_id, runner, cwd_arg)
            .await;
        CommandOutcome::text(text)
    }
}

pub struct AcpRCommand;

#[async_trait]
impl ControlCommand for AcpRCommand {
    fn name(&self) -> &str {
        "acpr"
    }

    fn help(&self) -> &str {
        "Resume ACP session. Usage: /acpr <runner> [cwd] <session_id>"
    }

    async fn execute(&self, ctx: &ControlContext, args: &str) -> CommandOutcome {
        let parts: Vec<&str> = args.split_whitespace().collect();
        let (runner, cwd, acp_session_id) = match parts.as_slice() {
            [runner, acp_session_id] => (*runner, None, *acp_session_id),
            [runner, cwd, acp_session_id, ..] => (*runner, Some(*cwd), *acp_session_id),
            _ => return CommandOutcome::text(t("command.acpr.usage", &ctx.locale)),
        };
        let text = ctx
            .acp_chat_controller
            .resume(&ctx.session_id, runner, acp_session_id, cwd)
            .await;
        CommandOutcome::text(text)
    }
}
