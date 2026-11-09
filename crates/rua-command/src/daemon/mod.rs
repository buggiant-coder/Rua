pub mod handlers;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::ApprovalService;
use crate::CommandOutcome;

pub use handlers::{
    ApprovalCommand, ApproveCommand, DaemonRouterCommand, DenyCommand, LogsCommand,
    ReloadConfigCommand, RestartCommand, StatusCommand, VersionCommand,
};

#[async_trait]
pub trait WorkspaceReloader: Send + Sync {
    async fn reload_all(&self) -> rua_core::Result<()>;
}

#[async_trait]
pub trait DaemonCommand: Send + Sync {
    fn name(&self) -> &str;
    fn help(&self) -> &str;
    async fn execute(&self, ctx: &DaemonContext, args: &str) -> CommandOutcome;
}

pub struct DaemonContext {
    pub data_dir: PathBuf,
    pub started_at: std::time::Instant,
    pub workspace_reloader: Arc<dyn WorkspaceReloader>,
    pub exit_fn: Option<Arc<dyn Fn(i32) + Send + Sync>>,
    pub approval_service: Arc<dyn ApprovalService>,
    pub locale: String,
}

pub struct DaemonCommandRegistry {
    commands: HashMap<String, Arc<dyn DaemonCommand>>,
}

impl DaemonCommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
        }
    }

    pub fn register(&mut self, cmd: Arc<dyn DaemonCommand>) {
        let name = cmd.name();
        assert!(
            is_valid_command_name(name),
            "daemon command name must be non-empty and contain no whitespace: {name:?}"
        );
        self.commands.insert(name.to_string(), cmd);
    }

    pub fn register_alias(&mut self, alias: &str, cmd: Arc<dyn DaemonCommand>) {
        assert!(
            is_valid_command_name(alias),
            "daemon command alias must be non-empty and contain no whitespace: {alias:?}"
        );
        self.commands.insert(alias.to_string(), cmd);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn DaemonCommand>> {
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

impl Default for DaemonCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn is_valid_command_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.chars().any(char::is_whitespace)
}

pub fn build_daemon_registry() -> DaemonCommandRegistry {
    let status: Arc<dyn DaemonCommand> = Arc::new(StatusCommand);
    let restart: Arc<dyn DaemonCommand> = Arc::new(RestartCommand);
    let reload_config: Arc<dyn DaemonCommand> = Arc::new(ReloadConfigCommand);
    let version: Arc<dyn DaemonCommand> = Arc::new(VersionCommand);
    let logs: Arc<dyn DaemonCommand> = Arc::new(LogsCommand);
    let approval: Arc<dyn DaemonCommand> = Arc::new(ApprovalCommand);
    let approve: Arc<dyn DaemonCommand> = Arc::new(ApproveCommand);
    let deny: Arc<dyn DaemonCommand> = Arc::new(DenyCommand);

    let mut sub_commands = HashMap::new();
    sub_commands.insert("status".to_string(), Arc::clone(&status));
    sub_commands.insert("restart".to_string(), Arc::clone(&restart));
    sub_commands.insert("reload-config".to_string(), Arc::clone(&reload_config));
    sub_commands.insert("version".to_string(), Arc::clone(&version));
    sub_commands.insert("logs".to_string(), Arc::clone(&logs));
    sub_commands.insert("approval".to_string(), Arc::clone(&approval));
    sub_commands.insert("approve".to_string(), Arc::clone(&approve));
    sub_commands.insert("deny".to_string(), Arc::clone(&deny));

    let router = Arc::new(DaemonRouterCommand::new(sub_commands));

    let mut registry = DaemonCommandRegistry::new();
    registry.register(status);
    registry.register(restart);
    registry.register(Arc::clone(&reload_config));
    registry.register_alias("reload_config", reload_config);
    registry.register(version);
    registry.register(logs);
    registry.register(approval);
    registry.register(approve);
    registry.register(deny);
    registry.register(router);
    registry
}
