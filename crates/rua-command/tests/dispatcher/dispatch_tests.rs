use rua_command::*;
use std::sync::Arc;

use crate::mock_commands::*;

fn text_from_outcome(outcome: &CommandOutcome) -> String {
    outcome
        .events
        .iter()
        .filter_map(|e| match e {
            agent_types::AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

struct ArgsEchoControlCommand;

#[async_trait::async_trait]
impl ControlCommand for ArgsEchoControlCommand {
    fn name(&self) -> &str {
        "stop"
    }

    fn help(&self) -> &str {
        "mock args echo"
    }

    async fn execute(&self, _ctx: &ControlContext, args: &str) -> CommandOutcome {
        CommandOutcome::text(args)
    }
}

#[tokio::test]
async fn daemon_wins_over_conversation_when_same_name() {
    let mut daemon_reg = DaemonCommandRegistry::new();
    daemon_reg.register(MockDaemonCmd::new("status", "daemon-status"));

    let mut conv_reg = ConversationCommandRegistry::new();
    conv_reg.register(MockConversationCmd::new("status", "conversation-status"));

    let dispatcher = CommandDispatcher::new(daemon_reg, ControlCommandRegistry::new(), conv_reg);
    let ctx = make_dispatch_context();

    let outcome = dispatcher.dispatch("/status", &ctx).await;
    assert_eq!(text_from_outcome(&outcome), "daemon-status");
    assert!(outcome.last);
}

#[tokio::test]
async fn daemon_wins_over_control_and_conversation_when_same_name() {
    let mut daemon_reg = DaemonCommandRegistry::new();
    daemon_reg.register(MockDaemonCmd::new("status", "daemon-status"));

    let mut control_reg = ControlCommandRegistry::new();
    control_reg.register(MockControlCmd::new("status", "control-status"));

    let mut conv_reg = ConversationCommandRegistry::new();
    conv_reg.register(MockConversationCmd::new("status", "conversation-status"));

    let dispatcher = CommandDispatcher::new(daemon_reg, control_reg, conv_reg);
    let ctx = make_dispatch_context();

    let outcome = dispatcher.dispatch("/status trailing args", &ctx).await;
    assert_eq!(text_from_outcome(&outcome), "daemon-status");
    assert!(matches!(outcome.kind, CommandOutcomeKind::Success));
}

#[tokio::test]
async fn control_wins_over_conversation_when_same_name() {
    let mut control_reg = ControlCommandRegistry::new();
    control_reg.register(MockControlCmd::new("stop", "control-stop"));

    let mut conv_reg = ConversationCommandRegistry::new();
    conv_reg.register(MockConversationCmd::new("stop", "conversation-stop"));

    let dispatcher = CommandDispatcher::new(DaemonCommandRegistry::new(), control_reg, conv_reg);
    let ctx = make_dispatch_context();

    let outcome = dispatcher.dispatch("/stop", &ctx).await;
    assert_eq!(text_from_outcome(&outcome), "control-stop");
}

#[tokio::test]
async fn command_token_with_trailing_args_routes_to_registered_command() {
    let mut control_reg = ControlCommandRegistry::new();
    control_reg.register(Arc::new(ArgsEchoControlCommand));

    let dispatcher = CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        control_reg,
        ConversationCommandRegistry::new(),
    );
    let ctx = make_dispatch_context();

    let outcome = dispatcher
        .dispatch("/stop session=s123 extra words", &ctx)
        .await;

    assert_eq!(text_from_outcome(&outcome), "session=s123 extra words");
    assert!(matches!(outcome.kind, CommandOutcomeKind::Success));
}

#[tokio::test]
async fn agent_required_command_returns_agent_required_outcome() {
    let mut conv_reg = ConversationCommandRegistry::new();
    conv_reg.register(MockConversationCmd::agent_required(
        "prompt",
        "should-not-reach",
    ));

    let dispatcher = CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        ControlCommandRegistry::new(),
        conv_reg,
    );
    let ctx = make_dispatch_context();

    let outcome = dispatcher.dispatch("/prompt", &ctx).await;
    assert!(outcome.is_agent_required());
    assert!(!outcome.last);
    assert!(outcome.events.is_empty());
}

#[tokio::test]
async fn agent_required_bypasses_fast_path() {
    let mut conv_reg = ConversationCommandRegistry::new();
    conv_reg.register(MockConversationCmd::agent_required(
        "prompt",
        "should-not-reach",
    ));

    let dispatcher = CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        ControlCommandRegistry::new(),
        conv_reg,
    );
    let ctx = make_dispatch_context();

    let result = run_command_path(&dispatcher, "/prompt", &ctx).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn unknown_command_returns_error_event() {
    let dispatcher = CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        ControlCommandRegistry::new(),
        ConversationCommandRegistry::new(),
    );
    let ctx = make_dispatch_context();

    let outcome = dispatcher.dispatch("/nonexistent", &ctx).await;
    assert!(outcome.last);
    let text = text_from_outcome(&outcome);
    assert!(text.contains("Unknown command"));
    assert!(text.contains("nonexistent"));
}

#[tokio::test]
async fn error_wrapped_as_return_value_not_panic() {
    let dispatcher = CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        ControlCommandRegistry::new(),
        ConversationCommandRegistry::new(),
    );
    let ctx = make_dispatch_context();

    let outcome = dispatcher.dispatch("/doesnotexist", &ctx).await;
    assert!(matches!(outcome.kind, CommandOutcomeKind::Error));
    assert!(outcome.last);
    assert!(!outcome.events.is_empty());
}

#[tokio::test]
async fn non_command_input_returns_none_from_run_command_path() {
    let dispatcher = CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        ControlCommandRegistry::new(),
        ConversationCommandRegistry::new(),
    );
    let ctx = make_dispatch_context();

    let result = run_command_path(&dispatcher, "hello world", &ctx).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn empty_or_bare_slash_input_returns_none_from_run_command_path() {
    let dispatcher = CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        ControlCommandRegistry::new(),
        ConversationCommandRegistry::new(),
    );
    let ctx = make_dispatch_context();

    for query in ["", "   ", "/", "/   "] {
        let result = run_command_path(&dispatcher, query, &ctx).await;
        assert!(
            result.is_none(),
            "empty or incomplete command input should not enter command path: {query:?}"
        );
    }
}

#[tokio::test]
async fn conversation_command_dispatched_when_no_higher_layer_match() {
    let mut conv_reg = ConversationCommandRegistry::new();
    conv_reg.register(MockConversationCmd::new("compact", "compacted"));

    let dispatcher = CommandDispatcher::new(
        DaemonCommandRegistry::new(),
        ControlCommandRegistry::new(),
        conv_reg,
    );
    let ctx = make_dispatch_context();

    let outcome = dispatcher.dispatch("/compact", &ctx).await;
    assert_eq!(text_from_outcome(&outcome), "compacted");
    assert!(matches!(outcome.kind, CommandOutcomeKind::Success));
}
