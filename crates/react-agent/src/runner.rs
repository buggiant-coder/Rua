use agent_types::{AgentEvent, ToolResult};
use rua_core::event::{AskQuestion, RuntimeEvent};
use rua_core::id::{MessageId, SessionId};
use rua_core::RuaError;

fn ask_user_event_from_tool_result(result: &ToolResult) -> Option<RuntimeEvent> {
    let metadata = result.metadata.get("_ask_user")?;
    let request_id = metadata
        .get("request_id")
        .and_then(serde_json::Value::as_str)?
        .trim();
    if request_id.is_empty() {
        return None;
    }
    let questions =
        serde_json::from_value::<Vec<AskQuestion>>(metadata.get("questions")?.clone()).ok()?;
    if questions.is_empty() {
        return None;
    }
    Some(RuntimeEvent::AskUser {
        request_id: request_id.to_string(),
        questions,
    })
}

pub fn agent_event_to_runtime_events(
    event: AgentEvent,
    session_id: &SessionId,
) -> Vec<RuntimeEvent> {
    match event {
        AgentEvent::ToolResult { result } => {
            let ask_user = ask_user_event_from_tool_result(&result);
            let mut events = vec![RuntimeEvent::ToolResult { result }];
            events.extend(ask_user);
            events
        }
        event => agent_event_to_runtime_event(event, session_id)
            .into_iter()
            .collect(),
    }
}

pub fn agent_event_to_runtime_event(
    event: AgentEvent,
    session_id: &SessionId,
) -> Option<RuntimeEvent> {
    match event {
        AgentEvent::Start { .. } => Some(RuntimeEvent::Start {
            session_id: session_id.clone(),
            agent_id: None,
        }),
        AgentEvent::TextDelta {
            delta, metadata, ..
        } => Some(RuntimeEvent::TextDelta {
            delta,
            message_id: event_message_id(&metadata),
        }),
        AgentEvent::ThinkingDelta {
            delta,
            signature,
            metadata,
            ..
        } => Some(RuntimeEvent::ThinkingDelta {
            delta,
            signature,
            message_id: event_message_id(&metadata),
        }),
        AgentEvent::ModelCallReset { model_call_id } => {
            Some(RuntimeEvent::ModelCallReset { model_call_id })
        }
        AgentEvent::ToolCallDelta {
            id,
            index,
            name,
            arguments_delta,
            ..
        } => Some(RuntimeEvent::ToolCallDelta {
            id,
            index,
            name,
            arguments_delta,
        }),
        AgentEvent::ToolCall { call } => Some(RuntimeEvent::ToolCall { call }),
        AgentEvent::ToolResultDelta {
            id,
            name,
            output_delta,
            ..
        } => Some(RuntimeEvent::ToolResultDelta {
            id,
            name,
            output_delta,
        }),
        AgentEvent::ToolResult { result } => Some(RuntimeEvent::ToolResult { result }),
        AgentEvent::Usage { usage } => Some(RuntimeEvent::Usage {
            input_tokens: Some(usage.prompt_tokens),
            output_tokens: Some(usage.completion_tokens),
            total_tokens: Some(usage.total()),
            cache_read_tokens: usage.cached_tokens,
            cache_write_tokens: usage.cache_write_tokens,
        }),
        AgentEvent::Error { message, .. } => Some(RuntimeEvent::Error {
            error: RuaError::Runtime { message },
        }),
        AgentEvent::End { finish_reason, .. } => Some(RuntimeEvent::End {
            finish_reason: Some(finish_reason.to_string()),
        }),
        AgentEvent::ToolApprovalPending {
            call_id,
            tool_name,
            request_id,
            metadata,
        } => Some(RuntimeEvent::ToolApprovalPending {
            call_id,
            tool_name,
            request_id,
            prompt: metadata
                .get("prompt")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        }),
        AgentEvent::ContextCompaction {
            status,
            mode,
            message,
        } => Some(RuntimeEvent::ContextCompaction {
            status,
            mode,
            message,
        }),
        AgentEvent::IterationStart { iteration } => {
            Some(RuntimeEvent::IterationStart { iteration })
        }
        AgentEvent::IterationEnd {
            iteration,
            has_tool_calls,
        } => Some(RuntimeEvent::IterationEnd {
            iteration,
            has_tool_calls,
        }),
        _ => None,
    }
}

fn event_message_id(metadata: &serde_json::Value) -> Option<MessageId> {
    metadata
        .get(agent_types::conversation_message_identity::CANONICAL_MESSAGE_ID_METADATA_KEY)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(MessageId::new)
}
