use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseFrame {
    pub event: Option<String>,

    pub data: serde_json::Value,

    pub id: Option<String>,

    pub retry: Option<u64>,
}

impl SseFrame {
    pub fn data(data: serde_json::Value) -> Self {
        Self {
            event: None,
            data,
            id: None,
            retry: None,
        }
    }

    pub fn named(event: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            event: Some(event.into()),
            data,
            id: None,
            retry: None,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn text_delta(delta: &str) -> Self {
        Self::data(serde_json::json!({
            "type": "text_delta",
            "delta": delta,
        }))
    }

    pub fn thinking_delta(delta: &str) -> Self {
        Self::data(serde_json::json!({
            "type": "thinking_delta",
            "delta": delta,
        }))
    }

    pub fn tool_call(call: &serde_json::Value) -> Self {
        Self::data(serde_json::json!({
            "type": "tool_call",
            "call": call,
        }))
    }

    pub fn tool_result(result: &serde_json::Value) -> Self {
        Self::data(serde_json::json!({
            "type": "tool_result",
            "result": result,
        }))
    }

    pub fn error(message: &str) -> Self {
        Self::data(serde_json::json!({
            "type": "error",
            "message": message,
        }))
    }

    pub fn start(session_id: &str, agent_id: Option<&str>) -> Self {
        let mut data = serde_json::json!({
            "type": "start",
            "session_id": session_id,
        });
        if let Some(aid) = agent_id {
            data["agent_id"] = serde_json::Value::String(aid.to_string());
        }
        Self::data(data)
    }

    pub fn end(finish_reason: Option<&str>) -> Self {
        let mut data = serde_json::json!({
            "type": "end",
        });
        if let Some(reason) = finish_reason {
            data["finish_reason"] = serde_json::Value::String(reason.to_string());
        }
        Self::data(data)
    }

    pub fn usage(input_tokens: Option<u64>, output_tokens: Option<u64>) -> Self {
        let prompt_tokens = input_tokens.unwrap_or(0);
        let completion_tokens = output_tokens.unwrap_or(0);
        let data = serde_json::json!({
            "type": "usage",
            "usage": {
                "prompt_tokens": prompt_tokens,
                "completion_tokens": completion_tokens,
                "total_tokens": prompt_tokens + completion_tokens,
            },
        });
        Self::data(data)
    }

    pub fn cancelled() -> Self {
        Self::data(serde_json::json!({
            "type": "cancelled",
        }))
    }

    pub fn to_sse_string(&self) -> String {
        let mut s = String::new();
        if let Some(ref event) = self.event {
            s.push_str(&format!("event: {event}\n"));
        }
        if let Some(ref id) = self.id {
            s.push_str(&format!("id: {id}\n"));
        }
        if let Some(retry) = self.retry {
            s.push_str(&format!("retry: {retry}\n"));
        }
        let data_str = serde_json::to_string(&self.data).unwrap_or_default();
        s.push_str(&format!("data: {data_str}\n\n"));
        s
    }
}

pub fn runtime_event_to_sse_frame(event: &rua_core::RuntimeEvent) -> SseFrame {
    match event {
        rua_core::RuntimeEvent::Start {
            session_id,
            agent_id,
        } => SseFrame::start(session_id.as_str(), agent_id.as_ref().map(|a| a.as_str())),

        rua_core::RuntimeEvent::TextDelta { delta, .. } => SseFrame::text_delta(delta),

        rua_core::RuntimeEvent::ThinkingDelta { delta, .. } => SseFrame::thinking_delta(delta),

        rua_core::RuntimeEvent::ModelCallReset { model_call_id } => {
            SseFrame::data(serde_json::json!({
                "type": "model_call_reset",
                "model_call_id": model_call_id,
            }))
        }

        rua_core::RuntimeEvent::ToolCallDelta {
            id,
            index,
            name,
            arguments_delta,
        } => SseFrame::data(serde_json::json!({
            "type": "tool_call_delta",
            "id": id,
            "index": index,
            "name": name,
            "arguments_delta": arguments_delta,
        })),

        rua_core::RuntimeEvent::ToolCall { call } => {
            let call_json = serde_json::to_value(call).unwrap_or_default();
            SseFrame::tool_call(&call_json)
        }

        rua_core::RuntimeEvent::ToolResultDelta {
            id,
            name,
            output_delta,
        } => SseFrame::data(serde_json::json!({
            "type": "tool_result_delta",
            "id": id,
            "name": name,
            "output_delta": output_delta,
        })),

        rua_core::RuntimeEvent::ToolResult { result } => {
            let result_json = serde_json::to_value(result).unwrap_or_default();
            SseFrame::tool_result(&result_json)
        }

        rua_core::RuntimeEvent::Usage {
            input_tokens,
            output_tokens,
            ..
        } => SseFrame::usage(*input_tokens, *output_tokens),

        rua_core::RuntimeEvent::ContextCompaction {
            status,
            mode,
            message,
        } => SseFrame::data(serde_json::json!({
            "type": "context_compaction",
            "status": status,
            "mode": mode,
            "message": message,
        })),

        rua_core::RuntimeEvent::Error { error } => SseFrame::error(&error.to_string()),

        rua_core::RuntimeEvent::End { finish_reason } => SseFrame::end(finish_reason.as_deref()),

        rua_core::RuntimeEvent::ToolApprovalPending {
            call_id,
            tool_name,
            request_id,
            prompt,
        } => SseFrame::data(serde_json::json!({
            "type": "tool_approval_request",
            "call_id": call_id,
            "tool_name": tool_name,
            "request_id": request_id,
            "prompt": prompt,
        })),

        rua_core::RuntimeEvent::IterationStart { .. }
        | rua_core::RuntimeEvent::IterationEnd { .. }
        | rua_core::RuntimeEvent::ToolApprovalRequest { .. } => {
            SseFrame::data(serde_json::json!(null))
        }

        rua_core::RuntimeEvent::AskUser {
            request_id,
            questions,
        } => SseFrame::data(serde_json::json!({
            "type": "ask_user",
            "request_id": request_id,
            "questions": questions,
        })),
    }
}
