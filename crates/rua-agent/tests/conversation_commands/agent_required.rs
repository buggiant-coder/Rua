use std::sync::Arc;

use rua_command::conversation::*;
use rua_command::ConversationCommand;

use crate::mock_seams::*;

#[tokio::test]
async fn prompts_lists_files() {
    let mut ctx = ConversationContext::stub("a", "s", "u", "c");
    ctx.locale = "en".into();
    ctx.prompt_files = Some(Arc::new(MockPromptFiles::new(vec![
        "sys.md".to_string(),
        "safety.md".to_string(),
    ])));

    let cmd = rua_agent::command::PromptsCommand;
    assert!(cmd.agent_required());
    let outcome = cmd.execute(&ctx, "").await;
    let text = outcome_text(&outcome);
    let meta = outcome_metadata(&outcome);
    assert!(text.contains("sys.md"));
    assert!(text.contains("safety.md"));
    assert_eq!(meta["ephemeral"], "true");
}

#[tokio::test]
async fn prompts_empty() {
    let mut ctx = ConversationContext::stub("a", "s", "u", "c");
    ctx.locale = "en".into();
    ctx.prompt_files = Some(Arc::new(MockPromptFiles::new(vec![])));

    let cmd = rua_agent::command::PromptsCommand;
    let outcome = cmd.execute(&ctx, "").await;
    assert!(outcome_text(&outcome).contains("none"));
}

#[tokio::test]
async fn prompt_add() {
    let mut ctx = ConversationContext::stub("a", "s", "u", "c");
    ctx.locale = "en".into();
    ctx.prompt_files = Some(Arc::new(MockPromptFiles::new(vec![])));

    let cmd = rua_agent::command::PromptCommand;
    assert!(cmd.agent_required());
    let outcome = cmd.execute(&ctx, "add test.md").await;
    let text = outcome_text(&outcome);
    assert!(text.contains("Prompt added"));
    assert!(text.contains("test.md"));
    assert_eq!(outcome_metadata(&outcome)["ephemeral"], "true");
}

#[tokio::test]
async fn prompt_add_already_active() {
    let mut ctx = ConversationContext::stub("a", "s", "u", "c");
    ctx.locale = "en".into();
    ctx.prompt_files = Some(Arc::new(MockPromptFiles::new(vec!["test.md".to_string()])));

    let cmd = rua_agent::command::PromptCommand;
    let outcome = cmd.execute(&ctx, "add test.md").await;
    assert!(outcome_text(&outcome).contains("Already active"));
}

#[tokio::test]
async fn prompt_remove() {
    let mut ctx = ConversationContext::stub("a", "s", "u", "c");
    ctx.locale = "en".into();
    ctx.prompt_files = Some(Arc::new(MockPromptFiles::new(vec!["test.md".to_string()])));

    let cmd = rua_agent::command::PromptCommand;
    let outcome = cmd.execute(&ctx, "remove test.md").await;
    assert!(outcome_text(&outcome).contains("Prompt removed"));
}

#[tokio::test]
async fn prompt_reset() {
    let mut ctx = ConversationContext::stub("a", "s", "u", "c");
    ctx.locale = "en".into();
    ctx.prompt_files = Some(Arc::new(MockPromptFiles::new(vec!["test.md".to_string()])));

    let cmd = rua_agent::command::PromptCommand;
    let outcome = cmd.execute(&ctx, "reset").await;
    assert!(outcome_text(&outcome).contains("reset to agent defaults"));
}

#[tokio::test]
async fn yes_confirms() {
    let mut ctx = ConversationContext::stub("a", "s", "u", "c");
    ctx.locale = "en".into();
    ctx.pending_action = Some(Arc::new(MockPendingAction::with_action(
        "switch to code mode",
    )));

    let cmd = rua_agent::command::YesCommand;
    assert!(cmd.agent_required());
    let outcome = cmd.execute(&ctx, "").await;
    let text = outcome_text(&outcome);
    assert!(text.contains("Confirmed"));
    assert!(text.contains("switch to code mode"));
    assert_eq!(outcome_metadata(&outcome)["ephemeral"], "true");
}

#[tokio::test]
async fn yes_no_pending() {
    let mut ctx = ConversationContext::stub("a", "s", "u", "c");
    ctx.locale = "en".into();
    ctx.pending_action = Some(Arc::new(MockPendingAction::empty()));

    let cmd = rua_agent::command::YesCommand;
    let outcome = cmd.execute(&ctx, "").await;
    assert!(outcome_text(&outcome).contains("No pending action"));
}

#[tokio::test]
async fn no_rejects() {
    let mut ctx = ConversationContext::stub("a", "s", "u", "c");
    ctx.locale = "en".into();
    ctx.pending_action = Some(Arc::new(MockPendingAction::with_action("delete file")));

    let cmd = rua_agent::command::NoCommand;
    assert!(cmd.agent_required());
    let outcome = cmd.execute(&ctx, "").await;
    let text = outcome_text(&outcome);
    assert!(text.contains("Rejected"));
    assert!(text.contains("delete file"));
}

#[tokio::test]
async fn no_no_pending() {
    let mut ctx = ConversationContext::stub("a", "s", "u", "c");
    ctx.locale = "en".into();
    ctx.pending_action = Some(Arc::new(MockPendingAction::empty()));

    let cmd = rua_agent::command::NoCommand;
    let outcome = cmd.execute(&ctx, "").await;
    assert!(outcome_text(&outcome).contains("No pending action"));
}
