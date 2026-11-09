use crate::core::config::BaseChannelConfig;
use crate::core::content::AgentChannelRequest;

pub fn should_respond(req: &AgentChannelRequest, cfg: &BaseChannelConfig) -> bool {
    if !cfg.require_mention {
        return true;
    }

    if !is_group_message(req) {
        return true;
    }

    is_mentioned(req) || has_bot_command(req)
}

pub fn is_group_message(req: &AgentChannelRequest) -> bool {
    if let Some(v) = req.metadata.get("is_group") {
        if let Some(b) = v.as_bool() {
            return b;
        }
    }
    if req
        .metadata
        .get("chat_type")
        .and_then(|v| v.as_str())
        .is_some_and(|ct| ct.eq_ignore_ascii_case("group") || ct.eq_ignore_ascii_case("supergroup"))
    {
        return true;
    }
    req.metadata
        .get("conversation_type")
        .and_then(|v| v.as_str())
        .is_some_and(|ct| ct == "2")
}

fn is_mentioned(req: &AgentChannelRequest) -> bool {
    metadata_bool(&req.metadata, "mentioned") || metadata_bool(&req.metadata, "bot_mentioned")
}

fn has_bot_command(req: &AgentChannelRequest) -> bool {
    metadata_bool(&req.metadata, "has_bot_command")
}

fn metadata_bool(metadata: &serde_json::Value, key: &str) -> bool {
    metadata.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}
