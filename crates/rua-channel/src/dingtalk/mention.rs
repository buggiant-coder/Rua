use crate::dingtalk::webhook::DingTalkWebhookPayload;

#[derive(Debug, Clone, Default)]
pub struct MentionConfig {
    pub bot_id: String,
    pub respond_to_unmentioned: bool,
}

#[derive(Debug, Clone)]
pub struct MentionResult {
    pub is_group_chat: bool,
    pub should_respond: bool,
    pub content: String,
}

pub fn bot_in_at_users(payload: &DingTalkWebhookPayload, bot_id: &str) -> bool {
    if bot_id.is_empty() {
        return payload.is_in_at_list;
    }
    if let Some(at_users) = payload.extra.get("atUsers") {
        if let Some(arr) = at_users.as_array() {
            return arr.iter().any(|u| {
                u.get("dingtalkId")
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| id == bot_id)
                    || u.get("staffId")
                        .and_then(|v| v.as_str())
                        .is_some_and(|id| id == bot_id)
            });
        }
    }
    payload.is_in_at_list
}

pub fn strip_at_prefix(content: &str) -> String {
    let mut s = content;
    while s.starts_with('@') {
        if let Some(ws_pos) = s[1..].find(char::is_whitespace) {
            s = s[1 + ws_pos..].trim_start();
        } else {
            break;
        }
    }
    s.to_string()
}

pub fn process_group_mention(
    payload: &DingTalkWebhookPayload,
    config: &MentionConfig,
) -> MentionResult {
    let is_group = payload.is_group();
    let content = payload.text_content();

    if !is_group {
        return MentionResult {
            is_group_chat: false,
            should_respond: true,
            content,
        };
    }

    let mentioned = bot_in_at_users(payload, &config.bot_id);
    let should_respond = mentioned || config.respond_to_unmentioned;

    let processed = if mentioned {
        strip_at_prefix(&content)
    } else {
        content
    };

    MentionResult {
        is_group_chat: true,
        should_respond,
        content: processed,
    }
}
