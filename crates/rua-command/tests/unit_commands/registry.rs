use std::sync::Arc;

use async_trait::async_trait;
use rua_command::{
    CommandOutcome, ControlCommand, ControlCommandRegistry, ControlContext, ConversationCommand,
    ConversationCommandRegistry, ConversationContext, DaemonCommand, DaemonCommandRegistry,
    DaemonContext,
};

struct MockConversationCmd {
    name: &'static str,
    requires_agent: bool,
}

impl MockConversationCmd {
    fn new(name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            name,
            requires_agent: false,
        })
    }

    fn agent_required(name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            name,
            requires_agent: true,
        })
    }
}

#[async_trait]
impl ConversationCommand for MockConversationCmd {
    fn name(&self) -> &str {
        self.name
    }

    fn help(&self) -> &str {
        "mock conversation command"
    }

    fn agent_required(&self) -> bool {
        self.requires_agent
    }

    async fn execute(&self, _ctx: &ConversationContext, _args: &str) -> CommandOutcome {
        CommandOutcome::text(self.name)
    }
}

struct MockControlCmd {
    name: &'static str,
}

impl MockControlCmd {
    fn new(name: &'static str) -> Arc<Self> {
        Arc::new(Self { name })
    }
}

#[async_trait]
impl ControlCommand for MockControlCmd {
    fn name(&self) -> &str {
        self.name
    }

    fn help(&self) -> &str {
        "mock control command"
    }

    async fn execute(&self, _ctx: &ControlContext, _args: &str) -> CommandOutcome {
        CommandOutcome::text(self.name)
    }
}

struct MockDaemonCmd {
    name: &'static str,
}

impl MockDaemonCmd {
    fn new(name: &'static str) -> Arc<Self> {
        Arc::new(Self { name })
    }
}

#[async_trait]
impl DaemonCommand for MockDaemonCmd {
    fn name(&self) -> &str {
        self.name
    }

    fn help(&self) -> &str {
        "mock daemon command"
    }

    async fn execute(&self, _ctx: &DaemonContext, _args: &str) -> CommandOutcome {
        CommandOutcome::text(self.name)
    }
}

#[test]
fn conversation_registry_lists_commands_by_name() {
    let mut registry = ConversationCommandRegistry::new();
    registry.register(MockConversationCmd::new("skills"));
    registry.register(MockConversationCmd::new("clear"));
    registry.register(MockConversationCmd::new("compact"));

    let names: Vec<_> = registry.list().into_iter().map(|(name, _)| name).collect();

    assert_eq!(names, vec!["clear", "compact", "skills"]);
}

#[test]
fn conversation_registry_reports_agent_required_only_for_registered_command() {
    let mut registry = ConversationCommandRegistry::new();
    registry.register(MockConversationCmd::agent_required("mode"));
    registry.register(MockConversationCmd::new("clear"));

    assert!(registry.is_agent_required("mode"));
    assert!(!registry.is_agent_required("clear"));
    assert!(!registry.is_agent_required("missing"));
}

#[test]
fn control_registry_replaces_duplicate_name_with_latest_command() {
    let mut registry = ControlCommandRegistry::new();
    registry.register(MockControlCmd::new("stop"));
    registry.register(MockControlCmd::new("stop"));

    let listed = registry.list();

    assert_eq!(listed, vec![("stop", "mock control command")]);
    assert!(registry.get("stop").is_some());
}

#[test]
#[should_panic(expected = "control command name must be non-empty")]
fn control_registry_rejects_empty_command_name() {
    let mut registry = ControlCommandRegistry::new();
    registry.register(MockControlCmd::new(""));
}

#[test]
#[should_panic(expected = "control command name must be non-empty")]
fn control_registry_rejects_whitespace_command_name() {
    let mut registry = ControlCommandRegistry::new();
    registry.register(MockControlCmd::new("bad name"));
}

#[test]
#[should_panic(expected = "control command name must be non-empty")]
fn control_registry_rejects_path_like_command_name() {
    let mut registry = ControlCommandRegistry::new();
    registry.register(MockControlCmd::new("usr/local/bin"));
}

#[test]
#[should_panic(expected = "conversation command name must be non-empty")]
fn conversation_registry_rejects_empty_command_name() {
    let mut registry = ConversationCommandRegistry::new();
    registry.register(MockConversationCmd::new(""));
}

#[test]
#[should_panic(expected = "conversation command name must be non-empty")]
fn conversation_registry_rejects_path_like_command_name() {
    let mut registry = ConversationCommandRegistry::new();
    registry.register(MockConversationCmd::new("tmp/project"));
}

#[test]
fn daemon_registry_alias_points_to_same_public_command_help() {
    let mut registry = DaemonCommandRegistry::new();
    let reload = MockDaemonCmd::new("reload-config");
    registry.register(reload.clone());
    registry.register_alias("reload_config", reload);

    assert_eq!(
        registry.get("reload-config").unwrap().name(),
        "reload-config"
    );
    assert_eq!(
        registry.get("reload_config").unwrap().name(),
        "reload-config"
    );

    let names: Vec<_> = registry.list().into_iter().map(|(name, _)| name).collect();
    assert_eq!(names, vec!["reload-config", "reload-config"]);
}

#[test]
#[should_panic(expected = "daemon command name must be non-empty")]
fn daemon_registry_rejects_empty_command_name() {
    let mut registry = DaemonCommandRegistry::new();
    registry.register(MockDaemonCmd::new(""));
}

#[test]
#[should_panic(expected = "daemon command alias must be non-empty")]
fn daemon_registry_rejects_empty_alias() {
    let mut registry = DaemonCommandRegistry::new();
    registry.register_alias("", MockDaemonCmd::new("status"));
}

#[test]
#[should_panic(expected = "daemon command alias must be non-empty")]
fn daemon_registry_rejects_path_like_alias() {
    let mut registry = DaemonCommandRegistry::new();
    registry.register_alias("daemon/status", MockDaemonCmd::new("status"));
}
