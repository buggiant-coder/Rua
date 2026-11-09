use agent_types::AgentEvent;

use crate::control::ControlCommandRegistry;
use crate::conversation::ConversationCommandRegistry;
use crate::daemon::DaemonCommandRegistry;
use crate::{CommandOutcome, DispatchContext};

pub struct CommandDispatcher {
    pub daemon: DaemonCommandRegistry,
    pub control: ControlCommandRegistry,
    pub conversation: ConversationCommandRegistry,
}

impl CommandDispatcher {
    pub fn new(
        daemon: DaemonCommandRegistry,
        control: ControlCommandRegistry,
        conversation: ConversationCommandRegistry,
    ) -> Self {
        Self {
            daemon,
            control,
            conversation,
        }
    }

    pub async fn dispatch(&self, query: &str, ctx: &DispatchContext) -> CommandOutcome {
        let Some((name, args)) = parse_command(query) else {
            return CommandOutcome::error("Not a slash command");
        };

        if self.conversation.is_agent_required(name) {
            return CommandOutcome::agent_required(name);
        }

        if let Some(cmd) = self.daemon.get(name) {
            return cmd.execute(&ctx.daemon, args).await;
        }

        if let Some(cmd) = self.control.get(name) {
            return cmd.execute(&ctx.control, args).await;
        }

        if let Some(cmd) = self.conversation.get(name) {
            return cmd.execute(&ctx.conversation, args).await;
        }

        CommandOutcome::error(format!(
            "Unknown command: /{name}. Type /help for available commands."
        ))
    }
}

pub async fn run_command_path(
    dispatcher: &CommandDispatcher,
    query: &str,
    ctx: &DispatchContext,
) -> Option<CommandOutcome> {
    if !is_command(query) {
        return None;
    }

    let (name, _) = parse_command(query)?;

    if dispatcher.conversation.is_agent_required(name) {
        return None;
    }

    Some(dispatcher.dispatch(query, ctx).await)
}

pub fn is_command(input: &str) -> bool {
    parse_command(input).is_some()
}

pub fn parse_command(input: &str) -> Option<(&str, &str)> {
    let input = input.trim();
    if !input.starts_with('/') {
        return None;
    }
    let without_slash = &input[1..];
    if let Some(space_pos) = without_slash.find(char::is_whitespace) {
        let name = &without_slash[..space_pos];
        if !is_valid_command_token(name) {
            return None;
        }
        Some((name, without_slash[space_pos..].trim()))
    } else if is_valid_command_token(without_slash) {
        Some((without_slash, ""))
    } else {
        None
    }
}

fn is_valid_command_token(name: &str) -> bool {
    !name.is_empty() && !name.contains('/')
}

impl CommandOutcome {
    pub(crate) fn error(message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            events: vec![AgentEvent::TextDelta {
                delta: msg,
                metadata: serde_json::Value::Null,
            }],
            last: true,
            kind: CommandOutcomeKind::Error,
        }
    }

    pub(crate) fn agent_required(command_name: &str) -> Self {
        Self {
            events: Vec::new(),
            last: false,
            kind: CommandOutcomeKind::AgentRequired {
                command: command_name.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub enum CommandOutcomeKind {
    Success,
    Error,
    AgentRequired { command: String },
    DaemonNotWired,
}
