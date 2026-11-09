use crate::content::ContentPart;
use crate::event::AgentEvent;

pub mod message_type {

    pub const MESSAGE: &str = "MESSAGE";

    pub const REASONING: &str = "REASONING";

    pub const PLUGIN_CALL: &str = "PLUGIN_CALL";

    pub const PLUGIN_CALL_OUTPUT: &str = "PLUGIN_CALL_OUTPUT";

    pub const ERROR: &str = "ERROR";
}

pub fn agent_event_message_type(event: &AgentEvent) -> Option<&'static str> {
    match event {
        AgentEvent::TextDelta { .. } => Some(message_type::MESSAGE),
        AgentEvent::ThinkingDelta { .. } => Some(message_type::REASONING),
        AgentEvent::ToolCallDelta { .. } => Some(message_type::PLUGIN_CALL),
        AgentEvent::ToolCall { .. } => Some(message_type::PLUGIN_CALL),
        AgentEvent::ToolResultDelta { .. } => Some(message_type::PLUGIN_CALL_OUTPUT),
        AgentEvent::ToolResult { .. } => Some(message_type::PLUGIN_CALL_OUTPUT),
        AgentEvent::Error { .. } => Some(message_type::ERROR),

        AgentEvent::Start { .. } | AgentEvent::End { .. } | AgentEvent::Usage { .. } => None,

        #[allow(unreachable_patterns)]
        _ => None,
    }
}

pub fn content_part_message_type(part: &ContentPart) -> Option<&'static str> {
    match part {
        ContentPart::Text { .. } => Some(message_type::MESSAGE),
        ContentPart::Thinking { .. } => Some(message_type::REASONING),
        ContentPart::ToolUse { .. } => Some(message_type::PLUGIN_CALL),
        ContentPart::ToolResult { .. } => Some(message_type::PLUGIN_CALL_OUTPUT),

        ContentPart::Image { .. }
        | ContentPart::Audio { .. }
        | ContentPart::Video { .. }
        | ContentPart::File { .. } => Some(message_type::MESSAGE),
        ContentPart::Data { .. } => None,

        #[allow(unreachable_patterns)]
        _ => None,
    }
}

pub fn is_user_visible(event: &AgentEvent) -> bool {
    agent_event_message_type(event).is_some()
}
