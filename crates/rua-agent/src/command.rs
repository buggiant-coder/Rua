use std::io::Write;
use std::path::Path;

use agent_types::{ModelDialogueItem, Role};
use async_trait::async_trait;
use rua_command::conversation::ConversationContext;
use rua_command::{CommandOutcome, ConversationCommand};

#[derive(Debug, Clone)]
pub struct PendingAction {
    pub action_type: String,
    pub payload: String,
    pub prompt: String,
}

const DEBUG_HISTORY_FILE: &str = "debug_history.jsonl";
const HISTORY_MAX_LENGTH: usize = 8000;

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let half = max_len / 2;
    format!("{}\n...\n{}", &s[..half], &s[s.len() - half..])
}

fn t(key: &str, locale: &str) -> String {
    rua_i18n::t(key, locale)
}

fn is_compact_command_message(message: &ModelDialogueItem) -> bool {
    matches!(message.role, Role::User)
        && rua_command::parse_command(&message.text_content())
            .is_some_and(|(name, _)| name == "compact")
}

fn compactable_messages(messages: Vec<ModelDialogueItem>) -> Vec<ModelDialogueItem> {
    let mut compactable = Vec::with_capacity(messages.len());
    let mut skip_command_response = false;
    for message in messages {
        if skip_command_response {
            skip_command_response = false;
            if !matches!(message.role, Role::User) {
                continue;
            }
        }
        if is_compact_command_message(&message) {
            skip_command_response = true;
            continue;
        }
        compactable.push(message);
    }
    compactable
}

pub struct CompactCommand;

#[async_trait]
impl ConversationCommand for CompactCommand {
    fn name(&self) -> &str {
        "compact"
    }
    fn help(&self) -> &str {
        "Trigger context compaction for this session"
    }
    async fn execute(&self, ctx: &ConversationContext, args: &str) -> CommandOutcome {
        let extra_instruction = args.trim();

        let Some(session) = ctx.session.as_ref() else {
            return CommandOutcome::text(t("command.common.session_not_available", &ctx.locale));
        };

        let messages = match session.load_messages(&ctx.session_id).await {
            Ok(m) => m,
            Err(e) => {
                return CommandOutcome::text(
                    t("command.common.load_messages_error", &ctx.locale)
                        .replace("{error}", &e.to_string()),
                )
            }
        };

        let active_messages = compactable_messages(messages);

        if active_messages.is_empty() {
            return CommandOutcome::text(t("command.compact.no_messages", &ctx.locale));
        }

        let Some(compactor) = ctx.context_compactor.as_ref() else {
            return CommandOutcome::text(t("command.compact.disabled", &ctx.locale));
        };

        let pairs: Vec<(&str, String)> = active_messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    agent_types::Role::User => "user",
                    agent_types::Role::Assistant => "assistant",
                    _ => "system",
                };
                (role, m.text_content())
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = pairs.iter().map(|(r, t)| (*r, t.as_str())).collect();
        let indices = rua_command::agent_input_indices(&borrowed);
        let window_messages: Vec<ModelDialogueItem> = indices
            .iter()
            .map(|&i| active_messages[i].clone())
            .collect();

        if window_messages.is_empty() {
            return CommandOutcome::text(t("command.compact.no_window", &ctx.locale));
        }

        let compact_content = match compactor
            .compact_context(&window_messages, None, extra_instruction)
            .await
        {
            Ok(Some(content)) => content,
            Ok(None) => {
                return CommandOutcome::text(t("command.compact.failed", &ctx.locale));
            }
            Err(e) => {
                return CommandOutcome::text(
                    t("command.compact.error", &ctx.locale).replace("{error}", &e.to_string()),
                )
            }
        };

        let updated_count = window_messages.len();

        if let Err(e) = session
            .publish_context_checkpoint(&ctx.session_id, compact_content.clone())
            .await
        {
            return CommandOutcome::text(
                t("command.compact.set_summary_error", &ctx.locale)
                    .replace("{error}", &e.to_string()),
            );
        }

        CommandOutcome::text(
            t("command.compact.done", &ctx.locale)
                .replace("{count}", &updated_count.to_string())
                .replace("{summary}", &compact_content),
        )
    }
}

pub struct NewCommand;

#[async_trait]
impl ConversationCommand for NewCommand {
    fn name(&self) -> &str {
        "new"
    }
    fn help(&self) -> &str {
        "Start a new conversation (clears history)"
    }
    async fn execute(&self, ctx: &ConversationContext, _args: &str) -> CommandOutcome {
        let Some(session) = ctx.session.as_ref() else {
            return CommandOutcome::text(t("command.common.session_not_available", &ctx.locale));
        };

        let messages = match session.load_messages(&ctx.session_id).await {
            Ok(m) => m,
            Err(e) => {
                return CommandOutcome::text(
                    t("command.common.load_messages_error", &ctx.locale)
                        .replace("{error}", &e.to_string()),
                )
            }
        };

        if messages.is_empty() {
            return CommandOutcome::text(t("command.new.no_messages", &ctx.locale));
        }

        if ctx.context_compactor.is_none() {
            return CommandOutcome::text(t("command.new.disabled", &ctx.locale));
        }

        let _ = session.clear_messages(&ctx.session_id).await;

        CommandOutcome::text(t("command.new.done", &ctx.locale))
    }
}

pub struct ClearCommand;

#[async_trait]
impl ConversationCommand for ClearCommand {
    fn name(&self) -> &str {
        "clear"
    }
    fn help(&self) -> &str {
        "Clear current session history"
    }
    async fn execute(&self, ctx: &ConversationContext, _args: &str) -> CommandOutcome {
        if let Some(session) = ctx.session.as_ref() {
            let _ = session.clear_messages(&ctx.session_id).await;
        }

        CommandOutcome::clear_history(rua_i18n::t("command.clear.body", &ctx.locale))
    }
}

pub struct HistoryCommand;

#[async_trait]
impl ConversationCommand for HistoryCommand {
    fn name(&self) -> &str {
        "history"
    }
    fn help(&self) -> &str {
        "Show session history overview"
    }
    async fn execute(&self, ctx: &ConversationContext, _args: &str) -> CommandOutcome {
        let Some(session) = ctx.session.as_ref() else {
            return CommandOutcome::text(t("command.common.session_not_available", &ctx.locale));
        };

        let messages = match session.load_messages(&ctx.session_id).await {
            Ok(m) => m,
            Err(e) => {
                return CommandOutcome::text(
                    t("command.common.load_messages_error", &ctx.locale)
                        .replace("{error}", &e.to_string()),
                )
            }
        };

        if messages.is_empty() {
            return CommandOutcome::text(t("command.history.empty", &ctx.locale));
        }

        let mut lines = Vec::new();
        for (i, msg) in messages.iter().enumerate() {
            let role = format!("{:?}", msg.role);
            let preview = truncate_str(&msg.text_content(), 80);
            let preview_line = preview.lines().next().unwrap_or("");
            lines.push(format!("  {}. [{}] {}", i + 1, role, preview_line));
        }

        let mut history_str = format!(
            "{}\n\n{}",
            t("command.history.header", &ctx.locale)
                .replace("{count}", &messages.len().to_string()),
            lines.join("\n")
        );

        history_str.push_str(&format!("\n{}", t("command.history.footer", &ctx.locale)));

        let compressed = session
            .get_active_context_summary(&ctx.session_id)
            .await
            .unwrap_or(None);
        if compressed.is_some() {
            history_str.push_str(&format!(
                "\n{}",
                t("command.history.footer_compact", &ctx.locale)
            ));
        }

        CommandOutcome::text(truncate_str(&history_str, HISTORY_MAX_LENGTH))
    }
}

pub struct CompactStrCommand;

#[async_trait]
impl ConversationCommand for CompactStrCommand {
    fn name(&self) -> &str {
        "compact_str"
    }
    fn help(&self) -> &str {
        "Show the context summary for this session"
    }
    async fn execute(&self, ctx: &ConversationContext, _args: &str) -> CommandOutcome {
        let Some(session) = ctx.session.as_ref() else {
            return CommandOutcome::text(t("command.common.session_not_available", &ctx.locale));
        };

        let summary = session
            .get_active_context_summary(&ctx.session_id)
            .await
            .unwrap_or(None);

        match summary {
            Some(text) => CommandOutcome::text(format!(
                "{}\n\n{text}",
                t("command.compact_str.has_summary", &ctx.locale)
            )),
            None => CommandOutcome::text(t("command.compact_str.no_summary", &ctx.locale)),
        }
    }
}

pub struct MessageCommand;

#[async_trait]
impl ConversationCommand for MessageCommand {
    fn name(&self) -> &str {
        "message"
    }
    fn help(&self) -> &str {
        "Show a specific message by index (usage: /message <index>)"
    }
    async fn execute(&self, ctx: &ConversationContext, args: &str) -> CommandOutcome {
        let Some(session) = ctx.session.as_ref() else {
            return CommandOutcome::text(t("command.common.session_not_available", &ctx.locale));
        };

        let messages = match session.load_messages(&ctx.session_id).await {
            Ok(m) => m,
            Err(e) => {
                return CommandOutcome::text(
                    t("command.common.load_messages_error", &ctx.locale)
                        .replace("{error}", &e.to_string()),
                )
            }
        };

        if args.is_empty() {
            return CommandOutcome::text(
                t("command.message.usage", &ctx.locale)
                    .replace("{count}", &messages.len().to_string()),
            );
        }

        let index: usize = match args.trim().parse() {
            Ok(n) => n,
            Err(_) => {
                return CommandOutcome::text(
                    t("command.message.invalid_index", &ctx.locale).replace("{index}", args),
                );
            }
        };

        if messages.is_empty() {
            return CommandOutcome::text(t("command.message.no_messages", &ctx.locale));
        }

        if index < 1 || index > messages.len() {
            return CommandOutcome::text(
                t("command.message.out_of_range", &ctx.locale)
                    .replace("{index}", &index.to_string())
                    .replace("{count}", &messages.len().to_string()),
            );
        }

        let msg = &messages[index - 1];
        let role = format!("{:?}", msg.role);
        let content_str = msg.text_content();

        let (display_content, truncated) = if content_str.len() > HISTORY_MAX_LENGTH {
            (truncate_str(&content_str, HISTORY_MAX_LENGTH), true)
        } else {
            (content_str, false)
        };

        let truncation_hint = if truncated {
            t("command.message.truncated", &ctx.locale)
        } else {
            String::new()
        };

        CommandOutcome::text(format!(
            "{}\n{display_content}{truncation_hint}",
            t("command.message.header", &ctx.locale)
                .replace("{index}", &index.to_string())
                .replace("{total}", &messages.len().to_string())
                .replace("{role}", &role),
        ))
    }
}

pub struct DumpHistoryCommand;

#[async_trait]
impl ConversationCommand for DumpHistoryCommand {
    fn name(&self) -> &str {
        "dump_history"
    }
    fn help(&self) -> &str {
        "Dump session history to a JSONL file"
    }
    async fn execute(&self, ctx: &ConversationContext, _args: &str) -> CommandOutcome {
        if ctx.workspace_dir.is_empty() {
            return CommandOutcome::text(t("command.dump_history.no_workspace", &ctx.locale));
        }

        let Some(session) = ctx.session.as_ref() else {
            return CommandOutcome::text(t("command.common.session_not_available", &ctx.locale));
        };

        let history_file = Path::new(&ctx.workspace_dir).join(DEBUG_HISTORY_FILE);

        let has_summary = session
            .get_active_context_summary(&ctx.session_id)
            .await
            .unwrap_or(None)
            .is_some();

        let messages = match session.load_messages(&ctx.session_id).await {
            Ok(m) => m,
            Err(e) => {
                return CommandOutcome::text(
                    t("command.common.load_messages_error", &ctx.locale)
                        .replace("{error}", &e.to_string()),
                )
            }
        };

        match std::fs::File::create(&history_file) {
            Ok(mut file) => {
                for msg in &messages {
                    let Ok(json) = serde_json::to_string(msg) else {
                        return CommandOutcome::text(t(
                            "command.dump_history.serialization_error",
                            &ctx.locale,
                        ));
                    };
                    if writeln!(file, "{json}").is_err() {
                        return CommandOutcome::text(t(
                            "command.dump_history.write_error",
                            &ctx.locale,
                        ));
                    }
                }

                CommandOutcome::text(
                    t("command.dump_history.done", &ctx.locale)
                        .replace("{count}", &messages.len().to_string())
                        .replace("{has_summary}", &has_summary.to_string())
                        .replace("{path}", &history_file.display().to_string()),
                )
            }
            Err(e) => CommandOutcome::text(
                t("command.dump_history.error", &ctx.locale).replace("{error}", &e.to_string()),
            ),
        }
    }
}

pub struct PromptsCommand;

#[async_trait]
impl ConversationCommand for PromptsCommand {
    fn name(&self) -> &str {
        "prompts"
    }
    fn help(&self) -> &str {
        "List active prompt files"
    }
    fn agent_required(&self) -> bool {
        true
    }
    async fn execute(&self, ctx: &ConversationContext, _args: &str) -> CommandOutcome {
        let Some(pf) = ctx.prompt_files.as_ref() else {
            return CommandOutcome::ephemeral(t("command.prompts.not_available", &ctx.locale));
        };
        let files = pf.get_active_prompt_files();
        if files.is_empty() {
            CommandOutcome::ephemeral(t("command.prompts.empty", &ctx.locale))
        } else {
            let items: String = files.iter().map(|f| format!("- {f}\n")).collect();
            CommandOutcome::ephemeral(
                t("command.prompts.list", &ctx.locale).replace("{items}", &items),
            )
        }
    }
}

pub struct PromptCommand;

#[async_trait]
impl ConversationCommand for PromptCommand {
    fn name(&self) -> &str {
        "prompt"
    }
    fn help(&self) -> &str {
        "Manage prompt files (usage: /prompt add|remove|reset)"
    }
    fn agent_required(&self) -> bool {
        true
    }
    async fn execute(&self, ctx: &ConversationContext, args: &str) -> CommandOutcome {
        let Some(pf) = ctx.prompt_files.as_ref() else {
            return CommandOutcome::ephemeral(t("command.prompts.not_available", &ctx.locale));
        };

        let parts: Vec<&str> = args.splitn(2, char::is_whitespace).collect();
        let sub = parts.first().map(|s| s.to_lowercase()).unwrap_or_default();
        let target = parts.get(1).map(|s| s.trim()).unwrap_or("");

        match sub.as_str() {
            "add" if !target.is_empty() => {
                if pf.add_prompt_file(target) {
                    CommandOutcome::ephemeral(
                        t("command.prompt.added", &ctx.locale).replace("{target}", target),
                    )
                } else {
                    CommandOutcome::ephemeral(
                        t("command.prompt.already_active", &ctx.locale).replace("{target}", target),
                    )
                }
            }
            "remove" if !target.is_empty() => {
                if pf.remove_prompt_file(target) {
                    CommandOutcome::ephemeral(
                        t("command.prompt.removed", &ctx.locale).replace("{target}", target),
                    )
                } else {
                    CommandOutcome::ephemeral(
                        t("command.prompt.not_active", &ctx.locale).replace("{target}", target),
                    )
                }
            }
            "reset" => {
                pf.reset_prompt_files();
                CommandOutcome::ephemeral(t("command.prompt.reset", &ctx.locale))
            }
            _ => CommandOutcome::ephemeral(t("command.prompt.usage", &ctx.locale)),
        }
    }
}

pub struct YesCommand;

#[async_trait]
impl ConversationCommand for YesCommand {
    fn name(&self) -> &str {
        "yes"
    }
    fn help(&self) -> &str {
        "Confirm the current pending action"
    }
    fn agent_required(&self) -> bool {
        true
    }
    async fn execute(&self, ctx: &ConversationContext, _args: &str) -> CommandOutcome {
        let Some(pa) = ctx.pending_action.as_ref() else {
            return CommandOutcome::ephemeral(t("command.common.no_agent_context", &ctx.locale));
        };
        match pa.resolve_pending_action(true) {
            Some(info) => CommandOutcome::ephemeral(
                t("command.yes.confirmed", &ctx.locale).replace("{prompt}", &info.prompt),
            ),
            None => CommandOutcome::ephemeral(t("command.yes.no_pending", &ctx.locale)),
        }
    }
}

pub struct NoCommand;

#[async_trait]
impl ConversationCommand for NoCommand {
    fn name(&self) -> &str {
        "no"
    }
    fn help(&self) -> &str {
        "Reject the current pending action"
    }
    fn agent_required(&self) -> bool {
        true
    }
    async fn execute(&self, ctx: &ConversationContext, _args: &str) -> CommandOutcome {
        let Some(pa) = ctx.pending_action.as_ref() else {
            return CommandOutcome::ephemeral(t("command.common.no_agent_context", &ctx.locale));
        };
        match pa.resolve_pending_action(false) {
            Some(info) => CommandOutcome::ephemeral(
                t("command.no.rejected", &ctx.locale).replace("{prompt}", &info.prompt),
            ),
            None => CommandOutcome::ephemeral(t("command.no.no_pending", &ctx.locale)),
        }
    }
}
